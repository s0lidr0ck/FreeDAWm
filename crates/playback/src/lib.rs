//! Realtime playback through the system audio device (cpal: CoreAudio, WASAPI, ALSA/Pulse/
//! PipeWire via ALSA, JACK, WebAudio).
//!
//! The UI thread owns a [`Player`]; the audio callback owns the [`MixEngine`] and a snapshot of
//! the session (`Arc<Session>`) that the UI swaps whenever the document changes. Commands go to
//! the callback through a channel; position and meters come back through atomics and a
//! `try_lock`ed snapshot, so the callback never blocks. When no device is available the player
//! falls back to a silent clock thread so the transport, meters and automation still run.
//!
//! Outputs: the device is opened with as many output channels as it offers, up to 64. The main
//! mix (SMPTE/WAV order) goes to device channels from its output's first channel, and each
//! hardware output ([`MixEngine::hardware_out`]) to its own channels; channels the device lacks
//! are dropped, never wrapped. When the device is narrower than a main mix that starts at channel
//! 1, surround is folded down to stereo (ITU-R BS.775), and a mono device receives the average of
//! the stereo channels, before output clipping (see `outmap`).
//!
//! Latency: [`Player::reported_latency`] reads what the device callbacks, the input feed and the
//! mix report, live; [`Player::measure_latency`] measures the true round trip with a click
//! through a loopback (see `measure`).
//!
//! Plugins and threads: hosted third-party plugins (CLAP/VST3) are created, given their stored
//! state and prepared on the thread that calls [`Player::update_session`] (the UI thread), and
//! travel to the audio thread together with the session snapshot that needs them; the mix
//! engine adopts them (see `soundcraft_mix`'s crate docs). Instances the engine retires, and the
//! session snapshots the callback replaces, come back on return channels and are dropped on the UI
//! thread ([`Player::idle`]). Plugin editors are driven, and plugin states for saving read
//! ([`Player::capture_states`]), on the UI thread
//! through handles taken from each instance before it leaves ([`Player::open_editor`]).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod devices;
pub mod feed;
pub mod jack;
pub mod measure;
pub mod midi_in;
mod outmap;
pub mod record;
pub mod stream;

pub use devices::AudioConfig;
pub use measure::{Latency, estimate};

use measure::MeasureBuf;
use outmap::{MAX_DEVICE_CHANNELS, OutMap};
use soundcraft_band::library::LibraryData;
use soundcraft_band::runtime::{BandRuntime, Retired};
use soundcraft_dsp::{Plugin, PluginEditor, PluginStateReader};
pub use soundcraft_mix::CountOff;
use soundcraft_mix::scout::ScoutRig;
use soundcraft_mix::{BandSongCache, BandUpdate, LiveInstances, MAX_CHANNELS, MixEngine, PreparedInstance, StripMeter, main_channels};
use soundcraft_model::{ModuleId, Session, TrackId};
use soundcraft_time::{Range, Samples};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};

/// The largest mix block for a device buffer of `buffer` frames (`None`: the device default):
/// room for callbacks up to twice the requested size. This is a capacity: the blocks rendered
/// follow the callbacks (see [`AudioState::fill`]).
pub fn max_block_for(buffer: Option<u32>) -> usize {
    buffer.map_or(2048, |b| (b as usize).saturating_mul(2)).clamp(256, 8192)
}

/// Measurements the return channel holds: the current one, and cancelled ones on their way back.
const MEASURE_RETURNS: usize = 4;

/// Room for replaced snapshots on their way back; `update_session` empties it before each send.
const OLD_SESSIONS: usize = 16;

/// Most Scout rigs out on the audio side at once (armed, or on their way back), and the room on
/// their way back: the bounded return channel and the callback's hold. `Side::arm` refuses more,
/// so the audio thread always has room for every rig and never drops one.
const SCOUT_RETURNS: usize = 8;

/// The realtime audio thread marker lives in the mix engine, which carries it into the strips it
/// renders on worker threads; the cpal callbacks here set it.
pub use soundcraft_mix::{SlotKey, mark_audio_thread, on_audio_thread};

/// Meter values for the UI.
#[derive(Debug, Clone, Default)]
pub struct MeterSnapshot {
    pub tracks: HashMap<TrackId, StripMeter>,
    pub main: StripMeter,
}

/// Plugin states read from the live instances: `(track, module or instrument, state)`.
pub type PluginStates = Vec<(TrackId, SlotKey, Vec<u8>)>;

/// A parameter edit made in a plugin's own editor: `(track, slot, param id, value)`.
pub type EditorEdit = (TrackId, SlotKey, String, f32);

enum Cmd {
    /// A new snapshot plus the third-party instances it needs (created on the UI thread).
    Session(Arc<Session>, Vec<PreparedInstance>),
    /// Where the callback sends the snapshots it replaces (freeing one takes thousands of frees).
    OldSessions(SyncSender<Arc<Session>>),
    /// Live input, and a planar buffer for it (channels × `max_block`) to replace the mix's input:
    /// both allocated off the audio thread; the old ones go back as garbage.
    Input(Arc<feed::InputFeed>, Vec<Vec<f32>>),
    /// Where recording blocks are captured; the old one goes back as garbage.
    Capture(Arc<record::Capture>),
    Recording(bool),
    /// The MIDI input hub (`midi_in`): its rings are drained once per mix block.
    Midi(Arc<midi_in::MidiHub>),
    Play {
        from: Samples,
        end: Option<Samples>,
        looped: Option<Range>,
        /// Count off first: the transport holds at `from` (nothing plays or records, live input
        /// is heard) while the count-off clicks, then rolls from `from`.
        countoff: Option<CountOff>,
    },
    Stop,
    Locate(Samples),
    /// A one-off action for a module's live plugin ([`Plugin::action`]).
    Action(TrackId, ModuleId, &'static str),
    /// A note played on a track's instrument now (note-off at velocity 0): auditions.
    Audition(TrackId, u8, u8),
    /// Where the callback sends Scout rigs back (finished, or refused while another is armed).
    ScoutReturn(SyncSender<Box<ScoutRig>>),
    /// Arm a Scout capture (built on the UI thread with `ScoutRig::new`).
    ScoutArm(Box<ScoutRig>),
    /// Arm a loop capture and, in the same batch, play `range` looped from its start: the
    /// capture is exactly the loop, however the commands are batched.
    ScoutLoop {
        rig: Box<ScoutRig>,
        range: Range,
    },
    /// End the armed Scout capture.
    ScoutStop,
    /// Start a loopback measurement into this preallocated buffer ([`Player::measure_latency`]).
    Measure(Box<MeasureBuf>),
    /// Where the callback sends finished (or interrupted) measurements back.
    MeasureReturn(SyncSender<Box<MeasureBuf>>),
    /// Abandon the measurement running: it goes back interrupted (and is dropped there).
    MeasureCancel,
    /// New band settings (built on the UI thread by `soundcraft_mix::prepare_band`).
    Band(Box<BandUpdate>),
    /// Lend the mix engine (every live plugin instance in it) to an offline render on the UI
    /// thread, through this channel (room for one, made on the UI thread), and run the spare
    /// engine, silent, meanwhile ([`Player::lend_instances`]). The transport stops.
    Lend(Box<MixEngine>, SyncSender<Box<MixEngine>>),
    /// The lent engine comes back; the spare goes to the UI thread as garbage.
    Return(Box<MixEngine>),
    /// The band's runtime, built on the UI thread before the first [`Cmd::Band`], so the audio
    /// thread never builds one.
    BandRuntime(Box<BandRuntime>),
    /// The player is going away (plugin stability, part 2): hand every plugin instance back as
    /// the last [`Garbage`] and output silence from now on, so instances are terminated on the
    /// UI thread (`Player::drop` waits for it), never on the audio thread with the stream.
    /// While the engine is lent that garbage holds the spare's (nothing); the lent engine goes
    /// back as garbage too when its [`Cmd::Return`] comes, and a [`Cmd::Lend`] after this is
    /// refused (the spare goes back, the channel closes). The one allocation on the audio
    /// thread: `MixEngine::drain_plugins` grows the retired lists, once, at the end of the
    /// stream's life (it replaces frees on the audio thread, which are worse).
    /// (Not on the web: one thread, nothing to wait for.)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Shutdown,
}

struct Shared {
    position: AtomicI64,
    /// Session frames rolled since the last Play (continuous across loop wraps), and when the
    /// block that moved it was rendered ([`clock_ns`]): the LAN remote's take clock
    /// ([`TransportProbe`]).
    rolled: AtomicI64,
    rolled_at_ns: AtomicU64,
    /// A Play is on its way to the audio thread (which resets `rolled`): no take clock meanwhile.
    play_pending: AtomicBool,
    playing: AtomicBool,
    /// A count-off runs before the transport rolls ([`Player::play_counted`]), and the frames of
    /// it played so far. `playing` stays false meanwhile: the take clock starts when it ends.
    counting: AtomicBool,
    countoff_at: AtomicI64,
    /// Speed × 1000.
    speed: AtomicU32,
    meters: Mutex<MeterSnapshot>,
    /// Callbacks that came late or after a gap (at most one per callback).
    underruns: AtomicU64,
    /// The peak share of a callback's period spent filling it since the last read, in ‰.
    load_permille: AtomicU32,
    /// The callback renders the device's frames 1:1 (equal rates, speed 1): no resampler.
    direct: AtomicBool,
    /// A take records (set by [`Player::set_recording`]): half speed waits.
    recording: AtomicBool,
    /// Reported latencies, read live for [`Player::reported_latency`]: the output callback's
    /// (playback − callback) and the input callback's (callback − capture), in ns; the input
    /// feed's own and the mix's compensation after the last render, in frames.
    out_latency_ns: AtomicU64,
    in_latency_ns: AtomicU64,
    feed_latency: AtomicU32,
    mix_latency: AtomicU32,
    /// Channels of the live input (0: none).
    input_channels: AtomicUsize,
    /// Frames the last output callback asked for (0: none yet).
    callback_frames: AtomicU32,
    /// The player is going away ([`Cmd::Shutdown`] sent): a lent engine coming back after this
    /// is dropped where it is returned (the UI thread), never sent to the dying audio thread.
    shut: AtomicBool,
    /// The remote mix stream: the host's tap on its main mix, the performer's stream in.
    tap: Arc<stream::MixTap>,
    stream_in: Arc<stream::StreamIn>,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            tap: Arc::new(stream::MixTap::default()),
            stream_in: Arc::new(stream::StreamIn::default()),
            shut: AtomicBool::new(false),
            position: AtomicI64::new(0),
            rolled: AtomicI64::new(0),
            rolled_at_ns: AtomicU64::new(0),
            play_pending: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            counting: AtomicBool::new(false),
            countoff_at: AtomicI64::new(0),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            underruns: AtomicU64::new(0),
            load_permille: AtomicU32::new(0),
            direct: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            out_latency_ns: AtomicU64::new(0),
            in_latency_ns: AtomicU64::new(0),
            feed_latency: AtomicU32::new(0),
            mix_latency: AtomicU32::new(0),
            input_channels: AtomicUsize::new(0),
            callback_frames: AtomicU32::new(0),
        }
    }
}

/// True when filling a callback took longer than the device's real period (`period` frames at
/// `device_rate`: the larger of the configured buffer and recent callbacks, not one small
/// callback's frames): the device ran dry.
#[cfg(not(target_arch = "wasm32"))]
fn late(elapsed: std::time::Duration, period: usize, device_rate: f64) -> bool {
    elapsed.as_secs_f64() > period as f64 / device_rate.max(1.0)
}

/// True when a callback's audio plays (`actual`, stream time) later than the end of the previous
/// callback's audio (`expected`) by more than 3/4 of the real period (`period` frames at `rate`):
/// the device played silence in between. Callback sizes that vary (WASAPI shared mode's
/// 480 → 240 → 480) stay inside the tolerance; audio that plays early is no gap.
#[cfg(not(target_arch = "wasm32"))]
fn gap(expected: std::time::Duration, actual: std::time::Duration, period: usize, rate: f64) -> bool {
    actual.as_secs_f64() - expected.as_secs_f64() > 0.75 * period as f64 / rate.max(1.0)
}

/// What stopped the audio output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTrouble {
    /// A block panicked; the output stays silent.
    Bug,
    /// The device went away.
    Gone,
    /// The device stopped asking for audio while playing.
    Stalled,
}

/// How the audio output is doing, for the UI.
#[derive(Default)]
struct Fault {
    /// A block panicked: output stays silent from then on.
    failed: AtomicBool,
    gone: AtomicBool,
    /// Blocks so far, to tell a device that stopped asking for audio.
    beats: std::sync::atomic::AtomicU64,
    /// The UI has been told.
    reported: AtomicBool,
}

impl Fault {
    /// What stopped the output, once.
    fn trouble(&self, stalled: bool) -> Option<OutputTrouble> {
        let trouble = if self.failed.load(Ordering::Relaxed) {
            OutputTrouble::Bug
        } else if self.gone.load(Ordering::Relaxed) {
            OutputTrouble::Gone
        } else if stalled {
            OutputTrouble::Stalled
        } else {
            return None;
        };
        (!self.reported.swap(true, Ordering::Relaxed)).then_some(trouble)
    }
}

/// How long a playing output may go without a block before it counts as stalled.
#[cfg(not(target_arch = "wasm32"))]
const STALL: std::time::Duration = std::time::Duration::from_secs(2);
/// Seconds of audio played from disk read before the transport rolls ([`Player::play`]), and
/// the longest Play waits for them (a slow disk: the rest streams, a block late is silence).
const PREPARE_SECS: f64 = 1.0;
const PREPARE_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// Nanoseconds on a process-wide monotonic clock (0 on the web, which has no `Instant`): the
/// time base of [`TransportProbe`] readings.
pub fn clock_ns() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let epoch = EPOCH.get_or_init(std::time::Instant::now);
        u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// One reading of the transport through a [`TransportProbe`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeReading {
    pub playing: bool,
    /// Timeline position (session samples) after the last rendered block.
    pub position: Samples,
    /// Session frames rolled since the last Play (continuous across loop wraps).
    pub rolled: i64,
    /// When the last block was rendered ([`clock_ns`]).
    pub at_ns: u64,
}

/// Reads the player's transport from any thread (atomics only; nothing on the audio thread waits).
#[derive(Clone)]
pub struct TransportProbe {
    shared: Arc<Shared>,
}

impl TransportProbe {
    pub fn read(&self) -> ProbeReading {
        let s = &self.shared;
        ProbeReading {
            playing: s.playing.load(Ordering::Relaxed) && !s.play_pending.load(Ordering::Acquire),
            position: s.position.load(Ordering::Relaxed),
            rolled: s.rolled.load(Ordering::Relaxed),
            at_ns: s.rolled_at_ns.load(Ordering::Relaxed),
        }
    }
}

/// True once the block count `beats` has stood still for `STALL` while playing; `seen` is the
/// count last seen and since when.
#[cfg(not(target_arch = "wasm32"))]
fn stalled(seen: &mut Option<(u64, std::time::Instant)>, beats: u64, playing: bool) -> bool {
    match *seen {
        Some((b, since)) if playing && b == beats => since.elapsed() >= STALL,
        _ => {
            *seen = Some((beats, std::time::Instant::now()));
            false
        }
    }
}

/// Fill one block with `fill`. If it panics, the block and every later one are silent and the
/// transport stops, rather than the panic taking the app down.
fn guarded_block(fault: &Fault, shared: &Shared, out: &mut [f32], fill: impl FnOnce(&mut [f32])) {
    fault.beats.fetch_add(1, Ordering::Relaxed);
    if fault.failed.load(Ordering::Relaxed) || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill(out))).is_err() {
        fault.failed.store(true, Ordering::Relaxed);
        shared.playing.store(false, Ordering::Relaxed);
        out.fill(0.0);
    }
}

/// State owned by the audio callback.
struct AudioState {
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
    session: Arc<Session>,
    mix: MixEngine,
    pos: Samples,
    playing: bool,
    end: Option<Samples>,
    looped: Option<Range>,
    /// The count-off under way before the transport rolls from `pos`, and the frames of it
    /// rendered so far.
    countoff: Option<CountOff>,
    countoff_at: i64,
    /// Rendered frames waiting to be consumed (planar, device layout: one per device output
    /// channel) and the fractional read index.
    pending: Vec<Vec<f32>>,
    /// The main mix as the engine renders it (planar), mapped into `pending`: room for the widest
    /// main format, allocated once, of which the first `main_width` are in use (a format change
    /// on the audio thread never allocates).
    main: Vec<Vec<f32>>,
    main_width: usize,
    /// Where the main mix and the hardware outputs land on the device.
    out: OutMap,
    pending_len: usize,
    read: f64,
    device_rate: f64,
    /// The largest block the mix renders ([`max_block_for`] the device buffer); `pending` and the
    /// mix's input are this long.
    max_block: usize,
    /// The last fill took the direct path (see [`AudioState::fill`]).
    direct: bool,
    /// The last frame of the block before `pending`, per channel: the resampler interpolates
    /// from it into the block (`read` in -1..0).
    carry: [f32; MAX_DEVICE_CHANNELS],
    /// Live input, popped into `mix.input` once per mix block.
    feed: Option<Arc<feed::InputFeed>>,
    /// Input frames popped during the current callback.
    popped: usize,
    /// [`Cmd::Shutdown`] seen: the instances are gone, every fill is silence.
    shut: bool,
    /// Where recording blocks are captured, and the samples it still owes as silence.
    capture: Option<Arc<record::Capture>>,
    owed: usize,
    /// Input is heard: an input and an armed or monitored track (set each fill).
    monitoring: bool,
    /// Input was heard and no longer is: the mix renders one more block, so the strips let go
    /// of the notes live MIDI held (else they would sound again when input is heard again).
    let_go: bool,
    /// Input was heard at the last fill.
    heard: bool,
    recording: bool,
    /// Hardware MIDI input ([`Player::set_midi`]).
    midi: Option<Arc<midi_in::MidiHub>>,
    /// Loop recording: the pass under way (each wrap of the loop starts the next).
    midi_pass: u16,
    /// A recording's live MIDI clock (set each fill): the hub's time at the callback's start, and
    /// the timeline frames from what the callback starts playing to the next block rendered
    /// (the previous callback's unplayed frames, then each block rendered, through a loop wrap).
    midi_now: u64,
    midi_lead: i64,
    /// Where the take starts, until the first block after a Play (without a count-off) has
    /// rolled: MIDI recorded in that block never lands before it (a note played between the
    /// Record press and the transport rolling is the take's first, not lost before it).
    midi_floor: Option<Samples>,
    /// Retired plugin instances (and analysis handles) go back to the UI thread to be dropped there.
    garbage: Sender<Garbage>,
    old_sessions: Option<SyncSender<Arc<Session>>>,
    /// Where finished (or refused) Scout rigs go back to the UI thread; never dropped here.
    scout_done: Option<SyncSender<Box<ScoutRig>>>,
    /// Rigs that could not be sent yet (channel full or not set up), retried every callback.
    scout_held: [Option<Box<ScoutRig>>; SCOUT_RETURNS],
    /// The engine is lent to an offline render ([`Cmd::Lend`]): the callback outputs silence and
    /// the transport cannot start until it is back.
    lent: bool,
    /// The loopback measurement running, where it goes back, and one that could not go yet.
    measure: Option<Box<MeasureBuf>>,
    measure_done: Option<SyncSender<Box<MeasureBuf>>>,
    measure_held: Option<Box<MeasureBuf>>,
}

/// What the audio thread hands back to the UI thread to drop: retired plugin instances and the
/// analysis handles of retired chain modules.
#[derive(Default)]
struct Garbage {
    plugins: Vec<Box<dyn Plugin>>,
    analysis: Vec<Arc<soundcraft_dsp::analysis::Analysis>>,
    /// Module assets (an Amp's cabinet IR) the engine and its plugins let go of.
    assets: Vec<Arc<dyn soundcraft_dsp::SampleSource>>,
    /// Link group slabs the engine took out of service.
    links: Vec<Arc<soundcraft_dsp::LinkSlab>>,
    /// The spare engine a lent engine's return replaced ([`Cmd::Return`]).
    engine: Option<Box<MixEngine>>,
    /// A replaced input feed, its buffer, and a replaced capture.
    feed: Option<Arc<feed::InputFeed>>,
    input: Vec<Vec<f32>>,
    capture: Option<Arc<record::Capture>>,
    /// What the band let go of: old songs and Libraries, a replaced update, a refused runtime.
    band: Retired,
    band_update: Option<Box<BandUpdate>>,
    band_runtime: Option<Box<BandRuntime>>,
    /// The answer to [`Cmd::Shutdown`]: every instance the audio thread held. Nothing follows
    /// but a lent engine's [`Cmd::Return`] drained after it, which comes as `engine` with `last`
    /// set too (the audio thread never keeps it).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    last: bool,
}

/// The silent clock's audio state. Its thread fills stereo frames, so the layout is stereo from
/// the start (not the main's width, which the first fill would lay out again).
#[cfg(not(target_arch = "wasm32"))]
fn clock_state(rx: Receiver<Cmd>, shared: Arc<Shared>, session: Arc<Session>, garbage: Sender<Garbage>) -> AudioState {
    let rate = f64::from(session.sample_rate.hz());
    AudioState::new(rx, shared, session, rate, max_block_for(None), garbage).with_device_channels(2)
}

/// Sends what `mix`'s band let go of (old songs and Libraries, replaced updates, and `runtime`
/// when given) to the UI thread.
fn send_band_garbage(mix: &mut MixEngine, garbage: &Sender<Garbage>, runtime: Option<Box<BandRuntime>>) {
    let mut band = Some(mix.take_retired_band()).filter(|b| !b.is_empty());
    let mut runtime = runtime;
    while let Some(u) = mix.take_spent_band() {
        let _ =
            garbage.send(Garbage { band: band.take().unwrap_or_default(), band_update: Some(u), band_runtime: runtime.take(), ..Garbage::default() });
    }
    if band.is_some() || runtime.is_some() {
        let _ = garbage.send(Garbage { band: band.unwrap_or_default(), band_runtime: runtime, ..Garbage::default() });
    }
}

fn new_engine(sr: f32, max_block: usize) -> MixEngine {
    let mut m = MixEngine::new(sr, max_block);
    m.set_external_instances(true);
    m.metronome = true;
    m
}

impl AudioState {
    fn new(rx: Receiver<Cmd>, shared: Arc<Shared>, session: Arc<Session>, device_rate: f64, max_block: usize, garbage: Sender<Garbage>) -> Self {
        let sr = session.sample_rate.as_f64() as f32;
        let nch = main_channels(&session);
        AudioState {
            garbage,
            old_sessions: None,
            scout_done: None,
            scout_held: Default::default(),
            out: OutMap::new(&session, nch),
            rx,
            shared,
            mix: new_engine(sr, max_block),
            session,
            pos: 0,
            playing: false,
            end: None,
            looped: None,
            countoff: None,
            countoff_at: 0,
            pending: vec![vec![0.0; max_block]; nch],
            main: vec![vec![0.0; max_block]; MAX_CHANNELS],
            main_width: nch,
            pending_len: 0,
            read: 0.0,
            device_rate,
            max_block,
            direct: false,
            carry: [0.0; MAX_DEVICE_CHANNELS],
            feed: None,
            popped: 0,
            shut: false,
            capture: None,
            owed: 0,
            monitoring: false,
            let_go: false,
            heard: false,
            recording: false,
            midi: None,
            midi_pass: 0,
            midi_now: 0,
            midi_lead: 0,
            midi_floor: None,
            lent: false,
            measure: None,
            measure_done: None,
            measure_held: None,
        }
    }

    /// Lays `pending` out for a device with `channels` outputs (off the audio thread: it
    /// allocates).
    fn with_device_channels(mut self, channels: usize) -> Self {
        let channels = channels.clamp(1, MAX_DEVICE_CHANNELS);
        self.pending = vec![vec![0.0; self.max_block]; channels];
        self.out = OutMap::new(&self.session, channels);
        (self.pending_len, self.read) = (0, 0.0);
        self
    }

    fn drain(&mut self) {
        let mut changed = false;
        // A Play, Stop or Locate resets the plugins once the batch is in: after a session that
        // came in the same batch is synced, at the final position and with that session's
        // automation (`MixEngine::reset_at`).
        let mut reset = false;
        while let Ok(c) = self.rx.try_recv() {
            match c {
                Cmd::Input(feed, buffers) => self.attach_input(feed, buffers),
                Cmd::Capture(c) => {
                    if let Some(old) = self.capture.replace(c) {
                        let _ = self.garbage.send(Garbage { capture: Some(old), ..Garbage::default() });
                    }
                }
                Cmd::Recording(r) => {
                    self.recording = r;
                    self.midi_pass = 0;
                }
                // The app keeps its own handle: the one replaced is never the last.
                Cmd::Midi(h) => self.midi = Some(h),
                Cmd::Session(s, instances) => {
                    changed = true;
                    if (s.sample_rate.as_f64() - self.session.sample_rate.as_f64()).abs() > 0.5 {
                        let mut old = std::mem::replace(&mut self.mix, new_engine(s.sample_rate.as_f64() as f32, self.max_block));
                        // The band's runtime goes back to be dropped; the UI thread sends one built
                        // for the new rate (`Side::prepare` sees the rate change).
                        let runtime = old.take_band();
                        send_band_garbage(&mut old, &self.garbage, runtime);
                        // A capture at the old rate ends here; its rig goes back, not down with the engine.
                        old.stop_scout();
                        if let Some(rig) = old.scout_done() {
                            self.send_scout(rig);
                        }
                        let plugins = old.drain_plugins();
                        let assets = old.take_retired_assets();
                        let links = old.take_retired_links();
                        let _ = self.garbage.send(Garbage { plugins, assets, links, ..Garbage::default() });
                        // The shared meter snapshot still holds copies of these handles: they leave
                        // with the new engine's garbage, after the next meter copy (see `send_garbage`).
                        self.mix.keep_retired_analysis(old.take_retired_analysis());
                    }
                    self.mix.adopt(instances);
                    // The main format may have changed: buffers for the widest are already here.
                    self.main_width = main_channels(&s).min(self.main.len());
                    // Outputs may have moved: the device layout stays, the map follows.
                    self.out = OutMap::new(&s, self.out.device_channels());
                    let old = std::mem::replace(&mut self.session, s);
                    if let Some(tx) = &self.old_sessions {
                        let _ = tx.try_send(old);
                    }
                }
                Cmd::OldSessions(tx) => self.old_sessions = Some(tx),
                Cmd::Play { from, end, looped, countoff } => {
                    self.shared.rolled.store(0, Ordering::Relaxed);
                    self.shared.rolled_at_ns.store(clock_ns(), Ordering::Relaxed);
                    self.shared.play_pending.store(false, Ordering::Release);
                    self.midi_pass = 0;
                    self.pos = from;
                    self.shared.position.store(from, Ordering::Relaxed);
                    self.end = end;
                    self.looped = looped.filter(|r| r.len() > 64);
                    // With a count-off the transport rolls once it is over (`render_next`).
                    self.countoff = countoff.filter(|c| !c.is_empty());
                    self.countoff_at = 0;
                    self.playing = self.countoff.is_none();
                    // After a count-off nothing is moved in: what was played during it is not taken.
                    self.midi_floor = self.playing.then_some(from);
                    self.pending_len = 0;
                    self.read = 0.0;
                    reset = true;
                }
                Cmd::Stop => {
                    self.playing = false;
                    // A count-off stopped is over: the transport never rolls, nothing is taken.
                    self.countoff = None;
                    self.end = None;
                    self.looped = None;
                    // What was rendered ahead is playback: not heard after Stop.
                    self.pending_len = 0;
                    self.read = 0.0;
                    reset = true;
                    // The transport stopping ends a Scout capture.
                    self.mix.stop_scout();
                }
                Cmd::Locate(at) => {
                    self.pos = at;
                    self.pending_len = 0;
                    self.read = 0.0;
                    // A jump keeps the Scout capture: it records a new played range.
                    reset = true;
                }
                Cmd::Action(track, module, name) => {
                    // (no allocation: a lookup and the plugin's own flag)
                    self.mix.module_action(track, module, name);
                }
                Cmd::Audition(track, note, velocity) => {
                    // (no allocation: a lookup and the plugin's event queue)
                    self.mix.audition(track, note, velocity);
                }
                Cmd::Shutdown => {
                    // Plugin stability, part 2: the instances go back to the UI thread, which
                    // terminates them there (VST3 and CLAP want that off the audio thread).
                    self.playing = false;
                    self.mix.stop_scout();
                    let plugins = self.mix.drain_plugins();
                    let assets = self.mix.take_retired_assets();
                    let links = self.mix.take_retired_links();
                    let _ = self.garbage.send(Garbage { plugins, assets, links, last: true, ..Garbage::default() });
                    self.shut = true;
                }
                Cmd::ScoutReturn(tx) => self.scout_done = Some(tx),
                Cmd::ScoutArm(rig) => {
                    // A capture that just ended goes back first, so it does not refuse this one.
                    self.return_scout();
                    if let Some(refused) = self.mix.arm_scout(rig) {
                        self.send_scout(refused);
                    }
                }
                Cmd::ScoutLoop { rig, range } => {
                    self.return_scout();
                    match self.mix.arm_scout(rig) {
                        Some(refused) => self.send_scout(refused),
                        None => {
                            self.pos = range.start;
                            self.end = None;
                            self.looped = Some(range).filter(|r| r.len() > 64);
                            self.playing = true;
                            self.pending_len = 0;
                            self.read = 0.0;
                            reset = true;
                        }
                    }
                }
                Cmd::ScoutStop => {
                    self.mix.stop_scout();
                    self.return_scout();
                }
                Cmd::Measure(m) => {
                    if let Some(mut old) = self.measure.replace(m) {
                        old.interrupted = true;
                        self.send_measure(old);
                    }
                }
                Cmd::MeasureReturn(tx) => self.measure_done = Some(tx),
                Cmd::MeasureCancel => {
                    if let Some(mut m) = self.measure.take() {
                        m.interrupted = true;
                        self.send_measure(m);
                    }
                }
                Cmd::BandRuntime(rt) => {
                    if let Some(refused) = self.mix.install_band(rt) {
                        let _ = self.garbage.send(Garbage { band_runtime: Some(refused), ..Garbage::default() });
                    }
                }
                Cmd::Band(u) => {
                    self.mix.adopt_band(u);
                    self.send_band_garbage();
                }
                Cmd::Lend(spare, _tx) if self.shut => {
                    // Shut: nothing to lend. The spare goes back to be dropped on the UI thread
                    // and the channel closes with `_tx`, which ends the loan (`Side::settle`).
                    let _ = self.garbage.send(Garbage { engine: Some(spare), ..Garbage::default() });
                }
                Cmd::Return(live) if self.shut => {
                    // The engine came back after the shutdown: never seated in a dying state (it
                    // would go down with the stream, its instances terminated on this thread).
                    // Back to the UI thread as the shutdown's garbage; with nobody left to take
                    // it (the player already gone), leaked rather than freed here.
                    self.lent = false;
                    if let Err(std::sync::mpsc::SendError(g)) = self.garbage.send(Garbage { engine: Some(live), last: true, ..Garbage::default() })
                        && let Some(live) = g.engine
                    {
                        let _ = Box::leak(live);
                    }
                }
                Cmd::Lend(mut spare, tx) => {
                    // The transport stops (what was rendered ahead is not heard), a Scout capture
                    // ends, and the live input keeps flowing into the spare.
                    self.playing = false;
                    self.end = None;
                    self.looped = None;
                    self.pending_len = 0;
                    self.read = 0.0;
                    self.mix.stop_scout();
                    self.return_scout();
                    // (the engines swap places by value: nothing is allocated or freed here)
                    std::mem::swap(&mut spare.input, &mut self.mix.input);
                    std::mem::swap(&mut self.mix, &mut *spare);
                    let live = spare;
                    match tx.try_send(live) {
                        Ok(()) => self.lent = true,
                        Err(TrySendError::Full(mut live) | TrySendError::Disconnected(mut live)) => {
                            // Nobody is waiting for it: back in service, the spare dropped elsewhere.
                            std::mem::swap(&mut self.mix, &mut *live);
                            let mut spare = live;
                            std::mem::swap(&mut spare.input, &mut self.mix.input);
                            let _ = self.garbage.send(Garbage { engine: Some(spare), ..Garbage::default() });
                        }
                    }
                    reset = true;
                }
                Cmd::Return(mut live) => {
                    std::mem::swap(&mut self.mix, &mut *live);
                    let mut spare = live;
                    std::mem::swap(&mut spare.input, &mut self.mix.input);
                    self.lent = false;
                    let _ = self.garbage.send(Garbage { engine: Some(spare), ..Garbage::default() });
                    reset = true;
                }
            }
        }
        if self.lent {
            // Nothing plays through the spare: a Play while lent waits for the engine.
            self.playing = false;
            self.countoff = None;
            self.end = None;
            self.looped = None;
        }
        if changed {
            // Place adopted instances now (not only once playback starts), so editors see them;
            // `sync` creates no third-party plugins here.
            self.mix.ensure_synced(&self.session);
        }
        if reset {
            self.mix.reset_at(&self.session, self.pos);
        }
        self.send_garbage(false);
        self.return_scout();
        self.shared.playing.store(self.playing, Ordering::Relaxed);
        self.publish_countoff();
    }

    /// The count-off state for [`Player::countoff_elapsed`] (two atomic stores).
    fn publish_countoff(&self) {
        self.shared.countoff_at.store(self.countoff_at, Ordering::Relaxed);
        self.shared.counting.store(self.countoff.is_some(), Ordering::Relaxed);
    }

    /// Sends the armed Scout rig back once its capture has ended, and retries a held one. No
    /// allocation: the channel is bounded and preallocated.
    fn return_scout(&mut self) {
        for i in 0..SCOUT_RETURNS {
            if let Some(rig) = self.scout_held.get_mut(i).and_then(Option::take) {
                self.send_scout(rig);
            }
        }
        if let Some(rig) = self.mix.scout_done() {
            self.send_scout(rig);
        }
    }

    /// Hands a rig to the UI thread, or holds it for the next callback when it cannot go yet.
    fn send_scout(&mut self, rig: Box<ScoutRig>) {
        let rig = match &self.scout_done {
            Some(tx) => match tx.try_send(rig) {
                Ok(()) => return,
                Err(TrySendError::Full(r) | TrySendError::Disconnected(r)) => r,
            },
            None => rig,
        };
        // At most SCOUT_RETURNS rigs are ever out (`Side::arm` refuses more), so a slot is free.
        if let Some(slot) = self.scout_held.iter_mut().find(|x| x.is_none()) {
            *slot = Some(rig);
        } else {
            // Cannot happen (see above). Never free memory on the audio thread: leak it instead.
            let _ = Box::leak(rig);
        }
    }

    /// Sends retired instances to the UI thread. Retired analysis handles go too when
    /// `meters_copied`: only once the shared meter snapshot has been refreshed since they were
    /// retired, so the snapshot's stale copies are dropped (here) while these still hold the
    /// handle, and the last reference is dropped on the UI thread.
    fn send_garbage(&mut self, meters_copied: bool) {
        let plugins = self.mix.take_retired();
        let analysis = if meters_copied { self.mix.take_retired_analysis() } else { Vec::new() };
        let assets = self.mix.take_retired_assets();
        let links = self.mix.take_retired_links();
        if !plugins.is_empty() || !analysis.is_empty() || !assets.is_empty() || !links.is_empty() {
            let _ = self.garbage.send(Garbage { plugins, analysis, assets, links, ..Garbage::default() });
        }
    }

    /// Sends what the band let go of (old songs and Libraries, replaced updates) to the UI thread.
    fn send_band_garbage(&mut self) {
        send_band_garbage(&mut self.mix, &self.garbage, None);
    }

    /// Take `feed` as the live input, with `buffers` as the mix's input; the old ones go back to
    /// the UI thread. Buffers that don't fit the feed and `max_block` are refused (no resizing
    /// here).
    fn attach_input(&mut self, feed: Arc<feed::InputFeed>, buffers: Vec<Vec<f32>>) {
        if buffers.len() != feed.channels() || buffers.iter().any(|b| b.len() < self.max_block) {
            let _ = self.garbage.send(Garbage { feed: Some(feed), input: buffers, ..Garbage::default() });
            return;
        }
        let input = std::mem::replace(&mut self.mix.input, buffers);
        let old = self.feed.replace(feed);
        if old.is_some() || !input.is_empty() {
            let _ = self.garbage.send(Garbage { feed: old, input, ..Garbage::default() });
        }
    }

    /// Pop `frames` of input that nothing hears, so the feed keeps its timing.
    fn discard_input(&mut self, frames: usize) {
        let Some(feed) = &self.feed else { return };
        let mut left = frames;
        while left > 0 {
            let n = left.min(self.max_block);
            feed.pop_into(&mut self.mix.input, n);
            left -= n;
        }
    }

    /// Render the next block at the session rate into `pending`.
    fn render_next(&mut self, quantum: usize) {
        let mb = self.max_block;
        let mut n = quantum.clamp(1, mb);
        if let Some(l) = self.looped
            && self.pos >= l.end
        {
            self.pos = l.start;
            self.midi_pass = self.midi_pass.saturating_add(1);
        }
        if let Some(c) = &self.countoff {
            // The count-off ends on a block edge: the take's first block starts at the record point.
            n = n.min(usize::try_from((c.len() - self.countoff_at).max(1)).unwrap_or(mb));
        } else if let Some(l) = self.looped {
            n = n.min(usize::try_from((l.end - self.pos).max(1)).unwrap_or(mb));
        } else if let Some(e) = self.end {
            if self.pos >= e {
                self.playing = false;
                self.end = None;
                // The transport stopping by itself ends a Scout capture, as a Stop does.
                self.mix.stop_scout();
                self.shared.playing.store(false, Ordering::Relaxed);
                // Monitored input carries on (monitor-only); else the block is silence.
                if !self.monitoring {
                    n = 0;
                }
            } else {
                n = n.min(usize::try_from(e - self.pos).unwrap_or(mb));
            }
        }
        if n == 0 {
            for c in &mut self.pending {
                c.iter_mut().for_each(|x| *x = 0.0);
            }
            self.pending_len = mb;
            return;
        }
        if let Some(feed) = &self.feed {
            feed.pop_into(&mut self.mix.input, n);
            self.popped += n;
        }
        // A recording block: the very input frames the mix hears, captured with the block's
        // position (the take's first frame has an exact timeline position).
        let mut first = false;
        match &self.capture {
            Some(cap) if self.recording && self.playing && cap.armed.load(Ordering::Acquire) => {
                if !cap.started.load(Ordering::Relaxed) {
                    first = true;
                    cap.start_pos.store(self.pos, Ordering::Relaxed);
                    // The feed's latency now, at the take's start (a drift feed's moves).
                    cap.start_feed.store(self.feed.as_ref().map_or(0, |f| f.extra_latency()), Ordering::Relaxed);
                }
                let input = &self.mix.input;
                let ch = input.len();
                if ch > 0 {
                    cap.capture_with(n * ch, |i| input.get(i % ch).and_then(|c| c.get(i / ch)).copied().unwrap_or(0.0), &mut self.owed);
                }
            }
            _ => self.owed = 0,
        }
        self.mix.monitor_only = !self.playing;
        self.mix.recording = self.recording;
        self.mix.countoff = self.countoff.map(|c| (c, self.countoff_at));
        // Hardware MIDI input: what came in since the last block, at its offsets in this one; a
        // recording keeps each event at the timeline position it arrived at, from its raw age:
        // the position the callback started playing (this block's, less the frames before it this
        // callback), less the event's age then. Through a loop wrap that position is in the pass
        // before, and the take moves it back there. The output's and the mix's latencies are
        // taken off when the take is placed.
        if let Some(h) = &self.midi {
            let rate = self.session.sample_rate.as_f64();
            h.drain_into(&mut self.mix.live_midi, h.now_us(), n, rate);
            if self.recording && self.playing {
                let floor = self.midi_floor.unwrap_or(i64::MIN);
                h.record_block(&self.mix.live_midi, self.pos.saturating_sub(self.midi_lead), self.midi_now, rate, self.midi_pass, floor);
            }
        }
        if self.playing {
            self.midi_floor = None;
        }
        let width = self.main_width.min(self.main.len());
        let main = self.main.get_mut(..width).unwrap_or_default();
        self.mix.render(&self.session, self.pos, n, main);
        // The remote mix stream: the host's main mix out to the network thread, the performer's
        // stream in from it (both lock-free, see `stream`).
        if self.shared.tap.is_on() || self.shared.stream_in.is_on() {
            // The mix delays everything by its plugin delay compensation: this block holds the
            // timeline from that much earlier (the host's tap), and the stream added to it now is
            // heard that much before the mix it rides with (the performer's mark), so both are
            // tagged that much earlier, as each PC's take clock takes it off too.
            let pdc = i64::try_from(self.mix.latency).unwrap_or(0);
            let rolled = self.shared.rolled.load(Ordering::Relaxed).saturating_sub(pdc);
            let pos = self.pos.saturating_sub(pdc);
            if let Some((first, rest)) = main.split_first_mut() {
                let second = rest.first_mut();
                match second {
                    Some(r) => {
                        self.shared.tap.push_block(first, r, n, rolled, pos, self.playing);
                        self.shared.stream_in.mix_into(first, r, n, rolled, self.playing);
                    }
                    None => {
                        self.shared.tap.push_block(first, first, n, rolled, pos, self.playing);
                        self.shared.stream_in.mix_into(first, &mut [], n, rolled, self.playing);
                    }
                }
            }
        }
        self.let_go = false;
        self.out.map(main, self.mix.hardware_out(), &mut self.pending, n);
        let mix_latency = u32::try_from(self.mix.latency).unwrap_or(u32::MAX);
        self.shared.mix_latency.store(mix_latency, Ordering::Relaxed);
        if first && let Some(cap) = &self.capture {
            // The mix's delay as this very block rendered (LLM drops it), then the take starts.
            cap.start_mix.store(mix_latency, Ordering::Relaxed);
            cap.started.store(true, Ordering::Release);
        }
        self.pending_len = n;
        let mut meters_copied = false;
        if let Ok(mut m) = self.shared.meters.try_lock() {
            meters_copied = true;
            m.main.copy_from(&self.mix.main_meter);
            for (k, v) in &self.mix.meters {
                match m.tracks.get_mut(k) {
                    Some(d) => d.copy_from(v),
                    None => {
                        m.tracks.insert(*k, v.clone());
                    }
                }
            }
            // Meters of tracks the engine no longer has (an engine swap) must not keep handles.
            m.tracks.retain(|k, _| self.mix.meters.contains_key(k));
        }
        // After the meter copy: the snapshot no longer holds retired analysis handles.
        self.send_garbage(meters_copied);
        // The live MIDI clock runs on whether the timeline moved or not: an event played during
        // a count-off block lands before the record point (and is not taken).
        self.midi_lead = self.midi_lead.saturating_add(n as i64);
        if self.playing {
            self.pos += n as i64;
            self.shared.position.store(self.pos, Ordering::Relaxed);
            // Where the disk thread reads ahead from (atomic stores only).
            soundcraft_model::disk::set_playhead(self.pos, true);
            // Lock-free: one clock read and two atomic stores per block.
            self.shared.rolled.fetch_add(n as i64, Ordering::Relaxed);
            self.shared.rolled_at_ns.store(clock_ns(), Ordering::Relaxed);
        } else if let Some(c) = self.countoff {
            self.countoff_at = self.countoff_at.saturating_add(n as i64);
            if self.countoff_at >= c.len() {
                // Over: the transport rolls from the record point with the next block, and the
                // take clock (the LAN remote's) starts from zero now, not at the button press.
                self.countoff = None;
                self.countoff_at = 0;
                self.playing = true;
                self.shared.rolled.store(0, Ordering::Relaxed);
                self.shared.rolled_at_ns.store(clock_ns(), Ordering::Relaxed);
                self.shared.playing.store(true, Ordering::Relaxed);
            }
            self.publish_countoff();
        }
    }

    /// Fill an interleaved device buffer.
    ///
    /// At equal device and session rates and speed 1 this is the direct path: the callback is
    /// covered by mix blocks of its own size (chunks of at most `max_block`; a loop end or the
    /// end of play cuts one short and the next carries on), copied 1:1. Otherwise blocks are
    /// rendered at the session rate and resampled to the device. Both read `pending` from `read`,
    /// so switching paths plays out what the other rendered rather than dropping it.
    fn fill(&mut self, out: &mut [f32], channels: usize) {
        self.drain();
        if self.shut {
            out.iter_mut().for_each(|x| *x = 0.0);
            return;
        }
        self.publish_input();
        self.return_measure();
        let channels = channels.max(1);
        let frames = out.len() / channels;
        self.shared.callback_frames.store(u32::try_from(frames).unwrap_or(u32::MAX), Ordering::Relaxed);
        self.popped = 0;
        if channels.min(MAX_DEVICE_CHANNELS) != self.out.device_channels() {
            // Not a live stream's case (its channels are fixed at open): a caller with another
            // layout. What was rendered for the old one is dropped.
            self.out = OutMap::new(&self.session, channels);
            (self.pending_len, self.read) = (0, 0.0);
        }
        if self.measure.is_some() && self.fill_measure(out, channels) {
            return;
        }
        let audio_in = self.feed.is_some() && self.session.tracks.iter().any(|t| t.mixer.may_hear_input());
        // Live MIDI into an armed or monitored instrument plays with the transport stopped too.
        let midi_in = self.midi.as_ref().is_some_and(|h| {
            let thru = h.thru();
            h.any_open() && self.session.tracks.iter().any(|t| soundcraft_mix::hears_live_midi(t, thru))
        });
        let hears = audio_in || midi_in;
        if std::mem::replace(&mut self.heard, hears) && !hears {
            self.let_go = true;
        }
        // An instrument releasing what live input held keeps the mix rendering until it is done.
        // So does a count-off (its clicks, with the transport held at the record point).
        let monitoring = hears || self.let_go || self.mix.live_tail() || self.countoff.is_some();
        self.monitoring = monitoring;
        // A Scout capture runs while the transport is stopped too (it measures silence, or the
        // live input): the mix renders, the output stays silent.
        let capturing = self.mix.scout_armed();
        if self.lent || (!self.playing && !monitoring && !capturing) {
            out.iter_mut().for_each(|x| *x = 0.0);
            if let Ok(mut m) = self.shared.meters.try_lock() {
                m.main = StripMeter::default();
                m.tracks.clear();
            }
            // MIDI that came in unheard is not played late.
            if let Some(h) = &self.midi {
                h.discard();
            }
            // The callback's input, unheard, still leaves the feed.
            let ratio = self.session.sample_rate.as_f64() / self.device_rate.max(1.0);
            let n = (frames as f64 * ratio).round() as usize;
            if let Some(feed) = &self.feed {
                feed.begin(n);
            }
            self.discard_input(n);
            return;
        }
        // The live MIDI clock: what this callback starts playing is the rest of the last block.
        if let Some(h) = &self.midi {
            self.midi_now = h.now_us();
            self.midi_lead = (self.pending_len as f64 - self.read).max(0.0).round() as i64;
        }
        let speed_milli = self.shared.speed.load(Ordering::Relaxed);
        let direct = (self.device_rate - self.session.sample_rate.as_f64()).abs() < 0.5 && speed_milli == 1000;
        if direct && let Some(feed) = &self.feed {
            // This callback pops exactly its frames (in chunks of up to `max_block`).
            feed.begin(frames);
        }
        self.direct = direct;
        self.shared.direct.store(direct, Ordering::Relaxed);
        if direct {
            self.fill_direct(out, channels, monitoring, capturing);
            // Frames nothing rendered (play ended, nothing monitored): their input leaves too.
            self.discard_input(frames.saturating_sub(self.popped));
        } else {
            let speed = f64::from(speed_milli) / 1000.0;
            let step = self.session.sample_rate.as_f64() / self.device_rate.max(1.0) * speed.clamp(0.1, 4.0);
            let quantum = ((frames as f64 * step).ceil() as usize).clamp(1, self.max_block);
            if let Some(feed) = &self.feed {
                // The pops this callback will make, and the input that comes per callback.
                let arrive = (frames as f64 * self.session.sample_rate.as_f64() / self.device_rate.max(1.0)).round() as usize;
                feed.begin_elastic(resampled_pops(self.read, self.pending_len, frames, step, quantum), arrive);
            }
            self.fill_resampled(out, channels, monitoring, capturing, step, quantum);
        }
        self.return_scout();
    }

    /// The live input's latencies and channels, for [`Player::reported_latency`] and
    /// [`Player::measure_latency`] (read live: a drift feed's latency moves).
    fn publish_input(&self) {
        let (extra, input, channels) = self.feed.as_ref().map_or((0, 0, 0), |f| (f.extra_latency(), f.input_latency_ns(), f.channels()));
        self.shared.feed_latency.store(extra, Ordering::Relaxed);
        self.shared.in_latency_ns.store(input, Ordering::Relaxed);
        self.shared.input_channels.store(channels, Ordering::Relaxed);
    }

    /// A loopback measurement's callback: the click into its device output channel and its input
    /// channel recorded, frame for frame (as on the direct path: the input frames the mix would
    /// hear with this callback's output frames), every other output silent (nothing monitored
    /// can feed back). Once it is full, or when it cannot go on (the transport started, the
    /// input went, the rates differ), it goes back to the UI thread. False when this callback is
    /// an ordinary one after all. Never allocates.
    fn fill_measure(&mut self, out: &mut [f32], channels: usize) -> bool {
        let direct = (self.device_rate - self.session.sample_rate.as_f64()).abs() < 0.5 && self.shared.speed.load(Ordering::Relaxed) == 1000;
        let Some(feed) = self.feed.as_ref().filter(|_| direct && !self.playing && !self.recording) else {
            if let Some(mut m) = self.measure.take() {
                m.interrupted = true;
                self.send_measure(m);
            }
            return false;
        };
        let Some(m) = self.measure.as_mut() else { return false };
        let frames = out.len() / channels;
        out.iter_mut().for_each(|x| *x = 0.0);
        feed.begin(frames);
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.max_block);
            feed.pop_into(&mut self.mix.input, n);
            let input = self.mix.input.get(m.input);
            for i in 0..n {
                if m.len == m.lead {
                    m.feed = feed.extra_latency();
                }
                let click = m.click();
                if m.output < channels
                    && let Some(o) = out.get_mut((done + i) * channels + m.output)
                {
                    *o = click;
                }
                m.record(input.and_then(|c| c.get(i)).copied().unwrap_or(0.0));
            }
            done += n;
        }
        if m.full()
            && let Some(m) = self.measure.take()
        {
            self.send_measure(m);
        }
        true
    }

    /// Hands a measurement to the UI thread, or holds it for the next callback when it cannot go
    /// yet. One runs at a time ([`Player::measure_latency`] refuses more) and the return channel
    /// has room for cancelled ones besides, so the hold has room.
    fn send_measure(&mut self, m: Box<MeasureBuf>) {
        let m = match &self.measure_done {
            Some(tx) => match tx.try_send(m) {
                Ok(()) => return,
                Err(TrySendError::Full(m) | TrySendError::Disconnected(m)) => m,
            },
            None => m,
        };
        match self.measure_held.take() {
            None => self.measure_held = Some(m),
            Some(held) => {
                // Cannot happen (see above). Never free memory on the audio thread: leak it.
                self.measure_held = Some(held);
                let _ = Box::leak(m);
            }
        }
    }

    /// Retries a measurement that could not go back yet.
    fn return_measure(&mut self) {
        if let Some(m) = self.measure_held.take() {
            self.send_measure(m);
        }
    }

    /// The direct path of [`AudioState::fill`]. Frames the resampler rendered and has not read
    /// yet (`pending[read..pending_len]`) play first; then blocks of exactly the frames still
    /// missing, so nothing is rendered ahead.
    fn fill_direct(&mut self, out: &mut [f32], channels: usize, monitoring: bool, capturing: bool) {
        let frames = out.len() / channels;
        let mut done = 0;
        while done < frames {
            let mut start = self.read.max(0.0).floor() as usize;
            let mut ended = false;
            if start >= self.pending_len {
                if !self.playing && !monitoring && !capturing {
                    break;
                }
                let was_playing = self.playing;
                self.render_next((frames - done).min(self.max_block));
                ended = was_playing && !self.playing;
                start = 0;
            }
            let n = self.pending_len.saturating_sub(start).min(frames - done);
            if n == 0 {
                break;
            }
            let sounding = self.playing || monitoring;
            let pending = &self.pending;
            if let Some(dest) = out.get_mut(done * channels..(done + n) * channels) {
                for (i, frame) in dest.chunks_mut(channels).enumerate() {
                    let at = start + i;
                    write_frame(frame, sounding, |c| pending.get(c).and_then(|v| v.get(at)).copied().unwrap_or(0.0));
                }
            }
            done += n;
            // Play just ended: the rest of its silent block is not owed to the next callback.
            self.read = if ended { self.pending_len as f64 } else { (start + n) as f64 };
        }
        // What is left (play ended) is silent.
        out.get_mut(done * channels..).into_iter().flatten().for_each(|x| *x = 0.0);
    }

    /// The resampling path of [`AudioState::fill`]: session-rate blocks of `quantum` read at
    /// `step` per device frame.
    fn fill_resampled(&mut self, out: &mut [f32], channels: usize, monitoring: bool, capturing: bool, step: f64, quantum: usize) {
        for frame in out.chunks_mut(channels) {
            // The next block is needed as soon as the read falls between this block's last frame
            // and the next block's first: that last frame is carried over to interpolate from.
            while self.read > self.pending_len as f64 - 1.0 {
                if let Some(last) = self.pending_len.checked_sub(1) {
                    for (k, ch) in self.carry.iter_mut().zip(&self.pending) {
                        *k = ch.get(last).copied().unwrap_or(0.0);
                    }
                }
                self.read -= self.pending_len as f64;
                if !self.playing && !monitoring && !capturing {
                    // Nothing more to hear: start afresh next time.
                    (self.read, self.pending_len) = (0.0, 0);
                    break;
                }
                self.render_next(quantum);
                if self.pending_len == 0 {
                    break;
                }
            }
            // A read in -1..0 falls between the carried frame and the block's first.
            let (i0, fr) = if self.read < 0.0 {
                (None, (self.read + 1.0).clamp(0.0, 1.0) as f32)
            } else {
                let i = self.read.floor();
                (Some(i as usize), (self.read - i) as f32)
            };
            let sounding = self.playing || monitoring;
            let (pending, carry, len) = (&self.pending, &self.carry, self.pending_len);
            let sample = |c: usize| {
                let ch = pending.get(c);
                let (a, b) = match i0 {
                    Some(i) => (ch.and_then(|v| v.get(i)).copied(), ch.and_then(|v| v.get(i + 1)).filter(|_| i + 1 < len).copied()),
                    None => (carry.get(c).copied(), ch.and_then(|v| v.first()).copied()),
                };
                let a = a.unwrap_or(0.0);
                let b = b.unwrap_or(a);
                a + (b - a) * fr
            };
            write_frame(frame, sounding, sample);
            self.read += step;
        }
    }
}

/// One device frame: device channel `c` is `sample(c)` (already mapped and clipped), or silence.
fn write_frame(frame: &mut [f32], sounding: bool, sample: impl Fn(usize) -> f32) {
    for (c, o) in frame.iter_mut().enumerate() {
        *o = if sounding { sample(c) } else { 0.0 };
    }
}

/// The frames the resampler will pop this callback: the whole blocks of `quantum` rendered
/// while `frames` device frames are read at `step` from `read` in a block of `len` (the loop in
/// [`AudioState::fill_resampled`]; a loop or play end cuts a block short, so this is a guide).
fn resampled_pops(read: f64, len: usize, frames: usize, step: f64, quantum: usize) -> usize {
    let Some(last) = frames.checked_sub(1) else { return 0 };
    let over = read + last as f64 * step - (len as f64 - 1.0);
    if over <= 0.0 || !over.is_finite() {
        return 0;
    }
    let blocks = (over / quantum.max(1) as f64).ceil();
    (blocks.min(1e6) as usize).saturating_mul(quantum)
}

/// UI-thread bookkeeping of the third-party instances the audio thread holds.
struct Side {
    /// Per slot: the plugin id, channel count and `state_load` its instance was made for, and
    /// whether it could be made.
    shadow: HashMap<(TrackId, SlotKey), (String, usize, u64, bool)>,
    /// Session sample rate the instances were prepared for.
    rate: f32,
    /// The largest block they are prepared for (the player's `max_block`).
    max_block: usize,
    /// Editor handles of the live instances.
    editors: HashMap<(TrackId, SlotKey), Box<dyn PluginEditor>>,
    /// State readers of the live instances.
    states: BTreeMap<(TrackId, SlotKey), Box<dyn PluginStateReader>>,
    /// Instances that refused their stored state: saves keep that state until the plugin's editor
    /// is opened.
    refused: HashSet<(TrackId, SlotKey)>,
    /// Slots whose plugin refused a preset (a new `state_load`) both in place and when made
    /// anew, until [`Player::take_refused_presets`].
    refused_presets: Vec<(TrackId, SlotKey)>,
    garbage: Receiver<Garbage>,
    old_sessions: Option<Receiver<Arc<Session>>>,
    /// Scout rigs coming back from the audio thread.
    scout_rx: Option<Receiver<Box<ScoutRig>>>,
    /// Rigs back and waiting for `Player::take_scout`, oldest first.
    scout_ready: std::collections::VecDeque<Box<ScoutRig>>,
    /// Progress of the capture last armed, until its rig is back.
    scout_progress: Option<Arc<AtomicU64>>,
    /// Rigs sent to the audio thread and not back yet (at most `SCOUT_RETURNS`).
    scout_out: usize,
    /// Measurements coming back from the audio thread, and whether one is out there.
    measure_rx: Option<Receiver<Box<MeasureBuf>>>,
    measure_out: bool,
    /// The feed latency the measurement last back saw at its click.
    measured_feed: u32,
    /// The id of the measurement out there; buffers with another (cancelled, timed out) are
    /// dropped when they come back.
    measure_id: u64,
    /// When the measurement out there was sent (it times out after `measure::TIMEOUT`).
    #[cfg(not(target_arch = "wasm32"))]
    measure_since: Option<std::time::Instant>,
    /// The live third-party instances' names (`info().name`), for the band's "auto" targets.
    names: HashMap<(TrackId, SlotKey), String>,
    /// The Library the band plays from (`None`: no band until the host hands one over).
    band_lib: Option<Arc<LibraryData>>,
    /// The lanes the last band update was built from (an unchanged song keeps its timeline).
    band_cache: BandSongCache,
    /// The audio thread has been sent a band runtime.
    band_installed: bool,
    /// The engine is lent to an offline render ([`Player::lend_instances`]).
    loan: Option<Loan>,
    /// Instances made ahead, a step at a time, for a session about to be handed over
    /// ([`Player::preload_begin`]); the next `prepare` for that session takes them.
    preload: Option<Preload>,
}

/// Third-party instances being made ahead of a session's hand-over (the progress window's
/// "Plugins" phase): the specs still to make, the instances made, and the slots that failed.
struct Preload {
    specs: Vec<soundcraft_mix::InstanceSpec>,
    next: usize,
    made: Vec<Preloaded>,
    /// `(track, slot, id, ch, state_load)` of the plugins that could not be made.
    failed: Vec<((TrackId, SlotKey), String, usize, u64)>,
    /// The session rate they were prepared for.
    rate: f32,
}

/// One instance made ahead, with what `Side` keeps for a live one.
struct Preloaded {
    instance: PreparedInstance,
    state_load: u64,
    name: String,
    editor: Option<Box<dyn PluginEditor>>,
    reader: Option<Box<dyn PluginStateReader>>,
    refused_state: bool,
}

/// Where a preload stands ([`Player::preload_step`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreloadProgress {
    /// Slots dealt with (made or failed) and slots in all.
    pub done: usize,
    pub total: usize,
    /// The id of the plugin about to be made (`None` once every slot is dealt with).
    pub next: Option<String>,
    /// The ids of the plugins that could not be made so far.
    pub failed: Vec<String>,
}

impl PreloadProgress {
    pub fn finished(&self) -> bool {
        self.done >= self.total
    }
}

impl Preload {
    fn progress(&self) -> PreloadProgress {
        PreloadProgress {
            done: self.next.min(self.specs.len()),
            total: self.specs.len(),
            next: self.specs.get(self.next).map(|x| x.id.clone()),
            failed: self.failed.iter().map(|(_, id, _, _)| id.clone()).collect(),
        }
    }

    /// Makes the next instance (on the calling thread). False once there is none left.
    fn step(&mut self, max_block: usize) -> bool {
        let Some(spec) = self.specs.get(self.next) else { return false };
        self.next = self.next.saturating_add(1);
        let key = (spec.track, spec.slot);
        match soundcraft_mix::create_instance(&spec.id, spec.state.as_deref(), self.rate, max_block, spec.ch) {
            Some(soundcraft_mix::NewInstance { mut plugin, refused_state }) => {
                let name = plugin.info().name.to_string();
                let editor = plugin.editor();
                let reader = plugin.state_reader();
                self.made.push(Preloaded {
                    instance: PreparedInstance { track: spec.track, slot: spec.slot, id: spec.id.clone(), ch: spec.ch, plugin },
                    state_load: spec.state_load,
                    name,
                    editor,
                    reader,
                    refused_state,
                });
            }
            None => self.failed.push((key, spec.id.clone(), spec.ch, spec.state_load)),
        }
        log::debug!("plugin instance preloaded for {:?} {:?}: {} ({} of {})", key.0, key.1, spec.id, self.next, self.specs.len());
        true
    }
}

/// The way a lent engine arrives from the audio thread, shared by the [`LendingInstances`] that
/// polls it and the [`Loan`] on the UI side. It is never dropped while the audio thread may still
/// answer the [`Cmd::Lend`]: an engine that arrives after the lend was called off (the receiver
/// gone, std's channel would discard the engine, every live instance with it, and the audio
/// thread would stay `lent` for ever) is found by [`Side::settle`] and sent straight back.
type LendRx = Arc<Mutex<Receiver<Box<MixEngine>>>>;

/// A lent engine's bookkeeping on the UI side: the state of every live instance when it left
/// (put back when it returns, so a render leaves each plugin as it was), and the session update
/// held back meanwhile (sent once it is back).
struct Loan {
    returned: Arc<AtomicBool>,
    /// The lend was called off before the engine arrived (the [`LendingInstances`] dropped
    /// unpolled): the engine still on its way goes straight back when it arrives.
    cancelled: Arc<AtomicBool>,
    rx: LendRx,
    snapshot: Vec<((TrackId, SlotKey), Vec<u8>)>,
    pending: Option<Arc<Session>>,
}

impl Side {
    fn new(rate: f32, max_block: usize, garbage: Receiver<Garbage>) -> Side {
        Side {
            shadow: HashMap::new(),
            rate,
            max_block,
            editors: HashMap::new(),
            states: BTreeMap::new(),
            refused: HashSet::new(),
            refused_presets: Vec::new(),
            garbage,
            old_sessions: None,
            scout_rx: None,
            scout_ready: std::collections::VecDeque::new(),
            scout_progress: None,
            scout_out: 0,
            measure_rx: None,
            measure_out: false,
            measured_feed: 0,
            measure_id: 0,
            #[cfg(not(target_arch = "wasm32"))]
            measure_since: None,
            names: HashMap::new(),
            band_lib: None,
            band_cache: BandSongCache::default(),
            band_installed: false,
            loan: None,
            preload: None,
        }
    }

    /// Begins making the third-party instances `s` needs ahead of its hand-over; an earlier
    /// preload is dropped here. Returns how many there are to make.
    fn preload_begin(&mut self, s: &Session) -> usize {
        self.collect_garbage();
        self.preload = None;
        let specs = soundcraft_mix::instance_specs(s);
        let n = specs.len();
        self.preload = Some(Preload { specs, next: 0, made: Vec::new(), failed: Vec::new(), rate: s.sample_rate.as_f64() as f32 });
        n
    }

    /// Takes the instances made ahead that `specs` (of the session now handed over) still ask
    /// for, as live ones; the rest are dropped here. Returns the instances for the audio thread.
    fn take_preloaded(&mut self, specs: &[soundcraft_mix::InstanceSpec], rate: f32) -> Vec<PreparedInstance> {
        let Some(pre) = self.preload.take() else { return Vec::new() };
        let mut out = Vec::new();
        if (pre.rate - rate).abs() > 0.5 {
            // Prepared for another rate: of no use (dropped here, on this thread).
            return out;
        }
        let wanted = |key: (TrackId, SlotKey), id: &str, ch: usize, load: u64| {
            specs.iter().any(|x| (x.track, x.slot) == key && x.id == id && x.ch == ch && x.state_load == load)
        };
        for m in pre.made {
            let key = (m.instance.track, m.instance.slot);
            if !wanted(key, &m.instance.id, m.instance.ch, m.state_load) {
                continue;
            }
            self.forget(key);
            self.shadow.insert(key, (m.instance.id.clone(), m.instance.ch, m.state_load, true));
            self.names.insert(key, m.name);
            if let Some(e) = m.editor {
                self.editors.insert(key, e);
            }
            if let Some(r) = m.reader {
                self.states.insert(key, r);
            }
            if m.refused_state {
                self.refused.insert(key);
            }
            out.push(m.instance);
        }
        for (key, id, ch, load) in pre.failed {
            if wanted(key, &id, ch, load) {
                self.forget(key);
                self.shadow.insert(key, (id, ch, load, false));
            }
        }
        out
    }

    /// Sends `s` to the audio thread with the instances it newly needs (or holds it while the
    /// engine is lent: the render must not have the session change under it).
    fn send_session(&mut self, tx: &Sender<Cmd>, s: Arc<Session>) {
        self.settle(tx);
        if let Some(l) = &mut self.loan {
            l.pending = Some(s);
            return;
        }
        let instances = self.prepare(&s);
        let band = self.prepare_band(&s);
        // The disk thread keeps this session's audio played from disk ready around the playhead.
        soundcraft_model::disk::focus(Some(Arc::clone(&s)));
        let _ = tx.send(Cmd::Session(s, instances));
        if let Some(u) = band {
            if !self.band_installed {
                // The runtime is built here, never on the audio thread.
                let _ = tx.send(Cmd::BandRuntime(soundcraft_mix::new_band_runtime(self.rate, &u)));
                self.band_installed = true;
            }
            let _ = tx.send(Cmd::Band(u));
        }
    }

    /// Once a lent engine is back: every instance's state is put back as it was when it left
    /// (only where the render changed it), and the session update held back meanwhile goes out.
    fn settle(&mut self, tx: &Sender<Cmd>) {
        let Some(l) = &self.loan else { return };
        if !l.returned.load(Ordering::Acquire) {
            if !l.cancelled.load(Ordering::Acquire) {
                return;
            }
            // Called off before the engine arrived: it may have arrived since (the audio
            // thread answered a lend nobody waits for any more), in which case it goes straight
            // back, dropped here on the UI thread when the audio thread is gone. Disconnected:
            // the audio thread never took the lend (no audio thread at all), nothing will come.
            match l.rx.lock().unwrap_or_else(PoisonError::into_inner).try_recv() {
                Ok(e) => {
                    let _ = tx.send(Cmd::Return(e));
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {}
            }
        }
        let Some(l) = self.loan.take() else { return };
        for (key, state) in l.snapshot {
            let Some(r) = self.states.get_mut(&key) else { continue };
            if r.save_state().as_deref() != Some(state.as_slice()) && !r.load_state(&state) {
                log::warn!("plugin {:?} {:?} did not take its state back after the render", key.0, key.1);
            }
        }
        if let Some(s) = l.pending {
            self.send_session(tx, s);
        }
    }

    /// Whether the engine is out (or its return has not been settled yet).
    fn lent(&mut self, tx: &Sender<Cmd>) -> bool {
        self.settle(tx);
        self.loan.is_some()
    }

    /// Sets up the way back for measurements (one out at a time).
    fn return_measure(&mut self, tx: &Sender<Cmd>) {
        // Room for a cancelled measurement or two besides the current one.
        let (mtx, mrx) = sync_channel(MEASURE_RETURNS);
        self.measure_rx = Some(mrx);
        let _ = tx.send(Cmd::MeasureReturn(mtx));
    }

    /// The current measurement, once back from the audio thread. Cancelled and timed-out ones
    /// that come back are dropped here, on the UI thread.
    fn take_measure(&mut self) -> Option<Box<MeasureBuf>> {
        let rx = self.measure_rx.as_ref()?;
        while let Ok(m) = rx.try_recv() {
            if self.measure_out && m.id == self.measure_id {
                self.measure_out = false;
                return Some(m);
            }
        }
        None
    }

    /// Forgets the measurement out there: Measure may start another at once.
    fn abandon_measure(&mut self, tx: &Sender<Cmd>) {
        if self.measure_out {
            self.measure_out = false;
            self.measure_id = self.measure_id.wrapping_add(1);
            let _ = tx.send(Cmd::MeasureCancel);
        }
    }

    fn return_scout(&mut self, tx: &Sender<Cmd>) {
        let (stx, srx) = sync_channel(SCOUT_RETURNS);
        self.scout_rx = Some(srx);
        let _ = tx.send(Cmd::ScoutReturn(stx));
    }

    /// Moves rigs the audio thread sent back into `scout_ready`.
    fn collect_scout(&mut self) {
        if let Some(rx) = &self.scout_rx {
            for rig in rx.try_iter() {
                if self.scout_progress.as_ref().is_some_and(|p| Arc::ptr_eq(p, &rig.progress())) {
                    self.scout_progress = None;
                }
                self.scout_out = self.scout_out.saturating_sub(1);
                self.scout_ready.push_back(rig);
            }
        }
    }

    /// Sends a rig to the audio thread in the command `make` builds (`ScoutArm`, `ScoutLoop`).
    /// With `SCOUT_RETURNS` rigs already out, or no audio thread (the web's silent clock), it is
    /// refused here and comes straight back unfinished, never captured. True when it was sent.
    fn arm(&mut self, tx: &Sender<Cmd>, rig: Box<ScoutRig>, make: impl FnOnce(Box<ScoutRig>) -> Cmd) -> bool {
        self.collect_scout();
        if self.scout_out >= SCOUT_RETURNS {
            self.scout_ready.push_back(rig);
            return false;
        }
        let progress = rig.progress();
        match tx.send(make(rig)) {
            Ok(()) => {
                self.scout_out += 1;
                if self.scout_progress.is_none() {
                    self.scout_progress = Some(progress);
                }
                true
            }
            Err(std::sync::mpsc::SendError(Cmd::ScoutArm(rig) | Cmd::ScoutLoop { rig, .. })) => {
                self.scout_ready.push_back(rig);
                false
            }
            Err(_) => false,
        }
    }

    /// [`Player::scout_loop`]: shows the transport playing from the loop start at once, and puts
    /// it back as it was when the rig is refused (nothing will play).
    fn scout_loop(&mut self, shared: &Shared, tx: &Sender<Cmd>, rig: Box<ScoutRig>, range: Range) -> bool {
        let was = (shared.position.load(Ordering::Relaxed), shared.playing.load(Ordering::Relaxed));
        shared.position.store(range.start, Ordering::Relaxed);
        shared.playing.store(true, Ordering::Relaxed);
        let accepted = self.arm(tx, rig, |rig| Cmd::ScoutLoop { rig, range });
        if !accepted {
            shared.position.store(was.0, Ordering::Relaxed);
            shared.playing.store(was.1, Ordering::Relaxed);
        }
        accepted
    }

    fn take_scout(&mut self) -> Option<Box<ScoutRig>> {
        self.collect_scout();
        self.scout_ready.pop_front()
    }

    /// Drops retired instances (closing their editors first, inside the plugin's drop) and
    /// replaced session snapshots.
    fn collect_garbage(&mut self) {
        self.collect_scout();
        while let Ok(g) = self.garbage.try_recv() {
            for p in g.plugins {
                log::debug!("plugin instance released: {}", p.info().name);
                drop(p);
            }
            drop(g.analysis);
            drop(g.engine);
            drop(g.assets);
            drop(g.links);
            drop(g.feed);
            drop(g.input);
            drop(g.capture);
            drop(g.band);
            drop(g.band_update);
            drop(g.band_runtime);
        }
        if let Some(rx) = &self.old_sessions {
            rx.try_iter().for_each(drop);
        }
    }

    fn return_old_sessions(&mut self, tx: &Sender<Cmd>) {
        let (otx, orx) = sync_channel(OLD_SESSIONS);
        self.old_sessions = Some(orx);
        let _ = tx.send(Cmd::OldSessions(otx));
    }

    fn forget(&mut self, key: (TrackId, SlotKey)) {
        self.shadow.remove(&key);
        self.names.remove(&key);
        self.states.remove(&key);
        self.refused.remove(&key);
        if let Some(mut e) = self.editors.remove(&key) {
            e.close();
        }
    }

    /// Forgets the slots whose plugin could not be created, so that the next
    /// [`Side::prepare`] tries them again. Returns how many.
    fn retry_missing(&mut self) -> usize {
        let missing: Vec<(TrackId, SlotKey)> = self.shadow.iter().filter(|(_, (_, _, _, made))| !*made).map(|(k, _)| *k).collect();
        for k in &missing {
            self.forget(*k);
        }
        missing.len()
    }
}

/// How long a dropped player waits for the audio thread to hand its plugin instances back.
#[cfg(not(target_arch = "wasm32"))]
const SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Plugin stability, part 2: a replaced player's plugin instances are terminated here, on the
/// thread that drops it (the UI thread), not on the audio thread with its stream: VST3 and CLAP
/// require `terminate`/`destroy` off the audio thread, and plugins crash otherwise. The audio
/// thread answers [`Cmd::Shutdown`] with the instances as its last garbage; a dead stream (the
/// device went away) does not answer, and after [`SHUTDOWN_WAIT`] the instances go down with it
/// as before.
#[cfg(not(target_arch = "wasm32"))]
impl Drop for Player {
    fn drop(&mut self) {
        // A lent engine returned from now on (the render dropped after the player, on this
        // thread) is dropped where it is returned, not sent to the dying audio thread.
        self.shared.shut.store(true, Ordering::Release);
        if self.tx.send(Cmd::Shutdown).is_err() {
            return;
        }
        let side = self.side();
        let deadline = std::time::Instant::now() + SHUTDOWN_WAIT;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match side.garbage.recv_timeout(left) {
                Ok(g) => {
                    let last = g.last;
                    // Dropped here: editors closed inside the plugins' drop, on this thread.
                    drop(g);
                    if last {
                        break;
                    }
                }
                Err(_) => {
                    log::warn!("audio thread did not hand its plugin instances back within {SHUTDOWN_WAIT:?}; they go down with the stream");
                    break;
                }
            }
        }
    }
}

/// The transport and audio device.
pub struct Player {
    shared: Arc<Shared>,
    fault: Arc<Fault>,
    /// The block count last seen while playing, and since when.
    #[cfg(not(target_arch = "wasm32"))]
    watch: Mutex<Option<(u64, std::time::Instant)>>,
    tx: Sender<Cmd>,
    side: Mutex<Side>,
    #[cfg(not(target_arch = "wasm32"))]
    _stream: Option<cpal::Stream>,
    #[cfg(target_arch = "wasm32")]
    _stream: Option<cpal::Stream>,
    /// The input stream opened with the output on one device (duplex), and what it feeds.
    duplex: Option<record::InputStream>,
    /// The device is an ASIO driver: its input can only open with the output (duplex).
    asio: bool,
    /// The ASIO driver's own input and output latency in device frames (`ASIOGetLatencies`),
    /// read once at open on the UI thread after the streams started (it changes only with the
    /// buffer, which a reopen changes). `None` off ASIO, without the `asio` feature, or when the
    /// driver gives no usable figure: [`Player::reported_latency`] then uses the timestamps.
    driver_latency: Option<(u32, u32)>,
    /// The audio system (cpal host) the device opened on: "ASIO", "WASAPI"… ("None" for the
    /// silent clock).
    pub system: String,
    pub device_name: String,
    /// The input device a recorder will open, resolved at open without opening it: ASIO's own
    /// device, else the chosen input, else the system's default input (`None`: none, or no
    /// device). The record offset's key uses it, so it doesn't change when the input opens.
    pub input_name: Option<String>,
    pub device_rate: u32,
    /// Output channels the device was opened with (0 for the silent clock).
    pub device_channels: u16,
    /// The device buffer in frames when one was chosen (`None`: the device default).
    pub device_buffer: Option<u32>,
    /// The largest mix block, and the block size plugins are prepared for
    /// ([`max_block_for`] the device buffer).
    pub max_block: usize,
    /// True when no audio device could be opened (silent clock).
    pub silent: bool,
}

impl Player {
    /// The device this player has open, for [`devices::audio_systems_with`] (the settings window
    /// must not probe a streaming ASIO driver). `in_channels` is unknown here: the recorder opens
    /// the input.
    pub fn live_device(&self) -> devices::LiveDevice {
        devices::LiveDevice {
            system: self.system.clone(),
            output: self.device_name.clone(),
            input: self.input_name.clone(),
            rate: self.device_rate,
            out_channels: self.device_channels,
            in_channels: None,
            buffer: self.device_buffer,
        }
    }

    /// Open the default output device. Never fails: without a device it runs a silent clock.
    pub fn new(session: Arc<Session>) -> Player {
        Player::with_config(session, &AudioConfig::default())
    }

    /// Open the output device `cfg` chooses (system, device, rate, buffer size), falling back to the
    /// defaults for whatever is unavailable. Never fails: without a device it runs a silent clock.
    pub fn with_config(session: Arc<Session>, cfg: &AudioConfig) -> Player {
        let p = Player::open(Arc::clone(&session), cfg);
        p.side().return_old_sessions(&p.tx);
        p.side().return_scout(&p.tx);
        p.side().return_measure(&p.tx);
        // Hand the initial session's third-party plugins over.
        p.update_session(session);
        p
    }

    /// What stopped the audio output, once: a bug in the engine, or the device went away or
    /// stopped asking for audio while playing.
    pub fn trouble(&self) -> Option<OutputTrouble> {
        #[cfg(not(target_arch = "wasm32"))]
        let stalled = {
            let playing = self.shared.playing.load(Ordering::Relaxed) && !self.silent;
            stalled(&mut self.watch.lock().unwrap_or_else(PoisonError::into_inner), self.fault.beats.load(Ordering::Relaxed), playing)
        };
        #[cfg(target_arch = "wasm32")]
        let stalled = false;
        self.fault.trouble(stalled)
    }

    fn side(&self) -> std::sync::MutexGuard<'_, Side> {
        self.side.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn open(session: Arc<Session>, cfg: &AudioConfig) -> Player {
        let shared = Arc::new(Shared::new());
        let want = session.sample_rate.hz();
        let side_rate = session.sample_rate.as_f64() as f32;
        let new_side = |max_block: usize, garbage: Receiver<Garbage>| Mutex::new(Side::new(side_rate, max_block, garbage));
        match open_device(want, cfg) {
            Ok((host, device, config, format, name, route)) => {
                // Duplex: input and output on one device and one clock. With ASIO it is the very
                // `cpal::Device` value the output uses: cpal's ASIO streams share the driver's one
                // buffer set, and asio-sys calls the callbacks in the order the streams were
                // added, so the input stream is built first and each period's input is in the
                // feed before the output callback pops it. Elsewhere (WASAPI) the input is a
                // separate endpoint, looked up by the same name among the input devices only (no
                // fallback to the default input: a miss means no duplex), and its callbacks come
                // in no guaranteed order relative to the output's.
                let asio = host.id().name().eq_ignore_ascii_case("ASIO");
                let named_input = (!asio && duplex(host.id().name(), cfg))
                    .then(|| cfg.input.as_deref().and_then(|n| devices::named_device(&host, n, false)))
                    .flatten();
                let duplex_device = if asio { Some(&device) } else { named_input.as_ref() };
                let input_route = named_input.as_ref().and_then(|d| jack_input_route(&host, d, cfg.input.as_deref()));
                // The chosen buffer size first; if the device refuses it, its default.
                let mut attempts = vec![config.clone()];
                if config.buffer_size != cpal::BufferSize::Default {
                    attempts.push(cpal::StreamConfig { buffer_size: cpal::BufferSize::Default, ..config });
                }
                let opened = first_started(
                    attempts,
                    |config: &cpal::StreamConfig| -> Result<Opened, cpal::BuildStreamError> {
                        let rate = config.sample_rate.0;
                        let channels = usize::from(config.channels);
                        let max_block = max_block_of(config);
                        let (tx, rx) = channel();
                        let (gtx, grx) = channel();
                        let mut state = AudioState::new(rx, Arc::clone(&shared), Arc::clone(&session), f64::from(rate), max_block, gtx)
                            .with_device_channels(channels);
                        // The input stream first, then the output (see above).
                        let input = duplex_device.and_then(|d| match duplex_input(d, config, want, max_block, asio, input_route.as_ref()) {
                            Ok(i) => Some(i),
                            Err(e) => {
                                log::warn!("cannot open “{name}” for input with the output: {e}; the output opens alone");
                                None
                            }
                        });
                        if let Some(i) = &input {
                            state.attach_input(Arc::clone(&i.feed), vec![vec![0.0; max_block]; i.feed.channels()]);
                            state.capture = Some(Arc::clone(&i.capture));
                        }
                        let fault = Arc::new(Fault::default());
                        let stream = match format {
                            cpal::SampleFormat::I32 => build_output::<i32>(&device, config, state, channels, &fault, &shared),
                            cpal::SampleFormat::I16 => build_output::<i16>(&device, config, state, channels, &fault, &shared),
                            _ => build_output::<f32>(&device, config, state, channels, &fault, &shared),
                        }?;
                        Ok(Opened { stream, tx, grx, fault, input, input_started: std::cell::Cell::new(false) })
                    },
                    |opened| {
                        // The input starts first too; if it won't, the output plays alone (and the
                        // input opens on its own when needed).
                        opened.input_started.set(opened.input.as_ref().is_some_and(|i| start_stream(&i.stream)));
                        start_stream(&opened.stream)
                    },
                );
                match opened {
                    Some((config, Opened { stream, tx, grx, fault, input, input_started })) => {
                        let duplex = input.filter(|_| input_started.get());
                        if let Some(r) = &route {
                            r.connect();
                        }
                        if let Some(r) = input_route.as_ref().filter(|_| duplex.is_some()) {
                            r.connect();
                        }
                        // The streams run, so the driver's buffers exist: its latency is valid now.
                        let driver_latency = if asio { asio_driver_latency() } else { None };
                        let rate = config.sample_rate.0;
                        let channels = usize::from(config.channels);
                        let buffer = match config.buffer_size {
                            cpal::BufferSize::Fixed(n) => Some(n),
                            cpal::BufferSize::Default => None,
                        };
                        log::info!(
                            "audio device: {name} @ {rate} Hz, {channels} ch, {format}, buffer {}",
                            buffer.map_or("default".to_string(), |n| n.to_string())
                        );
                        Player {
                            shared,
                            fault,
                            #[cfg(not(target_arch = "wasm32"))]
                            watch: Mutex::new(None),
                            tx,
                            side: new_side(max_block_of(&config), grx),
                            _stream: Some(stream),
                            duplex,
                            asio,
                            driver_latency,
                            system: host.id().name().to_string(),
                            input_name: if asio { Some(name.clone()) } else { devices::input_device_name(&host, cfg.input.as_deref()) },
                            device_name: name,
                            device_rate: rate,
                            device_channels: config.channels,
                            device_buffer: buffer,
                            max_block: max_block_of(&config),
                            silent: false,
                        }
                    }
                    None => {
                        log::warn!("no usable audio stream; using a silent clock");
                        let (tx, rx) = channel();
                        let (gtx, grx) = channel();
                        Player::silent_clock(shared, tx, rx, want, new_side(max_block_for(None), grx), gtx)
                    }
                }
            }
            Err(e) => {
                log::warn!("no audio output: {e}; using a silent clock");
                let (tx, rx) = channel();
                let (gtx, grx) = channel();
                Player::silent_clock_with(shared, tx, rx, Arc::clone(&session), new_side(max_block_for(None), grx), gtx)
            }
        }
    }

    fn silent_clock(shared: Arc<Shared>, tx: Sender<Cmd>, rx: Receiver<Cmd>, rate: u32, side: Mutex<Side>, gtx: Sender<Garbage>) -> Player {
        let s = Arc::new(Session::new("Untitled", soundcraft_time::SampleRate::new(rate).unwrap_or_default()));
        Player::silent_clock_with(shared, tx, rx, s, side, gtx)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn silent_clock_with(
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
        rx: Receiver<Cmd>,
        session: Arc<Session>,
        side: Mutex<Side>,
        gtx: Sender<Garbage>,
    ) -> Player {
        let rate = session.sample_rate.hz();
        let mut state = clock_state(rx, Arc::clone(&shared), session, gtx);
        let fault = Arc::new(Fault::default());
        let failed = Arc::clone(&fault);
        let spawn = std::thread::Builder::new().name("soundcraft-clock".into()).spawn(move || {
            let mut buf = vec![0.0f32; 4096];
            let mut last = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let now = std::time::Instant::now();
                let frames = ((now - last).as_secs_f64() * state.device_rate) as usize;
                last = now;
                let n = (frames * 2).min(buf.len());
                if let Some(b) = buf.get_mut(..n) {
                    let shared = Arc::clone(&state.shared);
                    guarded_block(&failed, &shared, b, |b| state.fill(b, 2));
                }
                if Arc::strong_count(&state.shared) <= 1 {
                    break;
                }
            }
        });
        if let Err(e) = spawn {
            log::warn!("clock thread failed: {e}");
        }
        Player {
            shared,
            fault,
            watch: Mutex::new(None),
            tx,
            side,
            _stream: None,
            duplex: None,
            asio: false,
            driver_latency: None,
            system: "None".into(),
            device_name: "No audio device (silent)".into(),
            input_name: None,
            device_rate: rate,
            device_channels: 0,
            device_buffer: None,
            max_block: max_block_for(None),
            silent: true,
        }
    }

    /// A player with no device and no thread, and the audio state its commands go to, for tests
    /// of a device that stalls (nothing answers until the test fills it) in other crates. Not
    /// for use outside tests.
    #[cfg(not(target_arch = "wasm32"))]
    #[doc(hidden)]
    pub fn stalled(session: Arc<Session>, device_rate: u32, max_block: usize, device_channels: usize) -> (Player, StalledAudio) {
        let (p, st) = Player::offline(session, device_rate, max_block, device_channels);
        (p, StalledAudio(st))
    }

    /// A player with no device and no thread, and the audio state its commands go to, for tests
    /// to drive by hand: a `device_channels` device at `device_rate`.
    #[cfg(not(target_arch = "wasm32"))]
    fn offline(session: Arc<Session>, device_rate: u32, max_block: usize, device_channels: usize) -> (Player, AudioState) {
        let (shared, (tx, rx), (gtx, grx)) = (Arc::new(Shared::new()), channel(), channel());
        let side = Mutex::new(Side::new(session.sample_rate.as_f64() as f32, max_block, grx));
        let state = AudioState::new(rx, Arc::clone(&shared), session, f64::from(device_rate), max_block, gtx).with_device_channels(device_channels);
        let p = Player {
            shared,
            fault: Arc::default(),
            watch: Mutex::new(None),
            tx,
            side,
            _stream: None,
            duplex: None,
            asio: false,
            driver_latency: None,
            system: "Offline".into(),
            device_name: "Offline".into(),
            input_name: None,
            device_rate,
            device_channels: u16::try_from(device_channels).unwrap_or(u16::MAX),
            device_buffer: None,
            max_block,
            silent: false,
        };
        p.side().return_measure(&p.tx);
        (p, state)
    }

    #[cfg(target_arch = "wasm32")]
    fn silent_clock_with(
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
        _rx: Receiver<Cmd>,
        session: Arc<Session>,
        side: Mutex<Side>,
        _gtx: Sender<Garbage>,
    ) -> Player {
        Player {
            shared,
            fault: Arc::default(),
            tx,
            side,
            _stream: None,
            duplex: None,
            asio: false,
            driver_latency: None,
            system: "None".into(),
            device_name: "No audio device".into(),
            input_name: None,
            device_rate: session.sample_rate.hz(),
            device_channels: 0,
            device_buffer: None,
            max_block: max_block_for(None),
            silent: true,
        }
    }

    /// Sends a new session snapshot to the audio thread, with freshly created (state-restored,
    /// prepared) instances of any third-party plugin it newly needs. Call on the UI thread.
    pub fn update_session(&self, s: Arc<Session>) {
        self.side().send_session(&self.tx, s);
    }

    /// Begins making, a step at a time, the third-party plugin instances `s` needs, ahead of
    /// [`Player::update_session`]`(s)`: that call then hands them over instead of making them all
    /// at once, so a host can create them one frame at a time (progress shown) while the old
    /// session keeps playing. An earlier preload is dropped. Returns how many there are to make.
    /// Call on the UI thread.
    pub fn preload_begin(&self, s: &Session) -> usize {
        self.side().preload_begin(s)
    }

    /// Makes the next instance, and more while `budget` lasts (at least one per call). Where the
    /// preload stands afterwards; `None` without one. Call on the UI thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn preload_step(&self, budget: std::time::Duration) -> Option<PreloadProgress> {
        let mut side = self.side();
        let max_block = side.max_block;
        let pre = side.preload.as_mut()?;
        let started = std::time::Instant::now();
        while pre.step(max_block) && started.elapsed() < budget {}
        Some(pre.progress())
    }

    /// The browser has no `Instant`: one instance per call.
    #[cfg(target_arch = "wasm32")]
    pub fn preload_step(&self, _budget: std::time::Duration) -> Option<PreloadProgress> {
        let mut side = self.side();
        let max_block = side.max_block;
        let pre = side.preload.as_mut()?;
        pre.step(max_block);
        Some(pre.progress())
    }

    /// Where the preload stands; `None` without one.
    pub fn preload_progress(&self) -> Option<PreloadProgress> {
        self.side().preload.as_ref().map(Preload::progress)
    }

    /// Drops the instances made ahead (here, on the calling thread; never on the audio thread).
    /// Call on the UI thread.
    pub fn preload_cancel(&self) {
        self.side().preload = None;
    }

    /// Lends the audio thread's engine, with every live plugin instance in it, to an offline
    /// render (Bounce, Freeze, Commit) on this thread: the render goes through the very
    /// instances that play, Pro Tools offline-bounce style. The audio callback outputs silence
    /// and the transport cannot start until the engine is back. Refused while playing and while
    /// a render already has it. The engine arrives through [`LendingInstances::poll`] within a
    /// callback or two; dropping what `poll` gave (the render done, or cancelled) restores every
    /// instance's state as it was and hands the engine back. Call on the UI thread.
    pub fn lend_instances(&self, session: Arc<Session>) -> Result<LendingInstances, String> {
        if self.is_playing() {
            return Err("stop the transport first".into());
        }
        let mut side = self.side();
        if side.lent(&self.tx) {
            return Err("a render is already running".into());
        }
        let snapshot = side.states.iter_mut().filter_map(|(&k, r)| Some((k, r.save_state()?))).collect();
        let returned = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (etx, erx) = sync_channel(1);
        let rx: LendRx = Arc::new(Mutex::new(erx));
        side.loan = Some(Loan { returned: Arc::clone(&returned), cancelled: Arc::clone(&cancelled), rx: Arc::clone(&rx), snapshot, pending: None });
        let spare = Box::new(new_engine(side.rate, self.max_block));
        let _ = self.tx.send(Cmd::Lend(spare, etx));
        Ok(LendingInstances {
            rx,
            tx: self.tx.clone(),
            shared: Arc::clone(&self.shared),
            session,
            returned,
            cancelled,
            taken: false,
            #[cfg(not(target_arch = "wasm32"))]
            since: std::time::Instant::now(),
        })
    }

    /// True while the engine is lent to a render ([`Player::lend_instances`]).
    pub fn is_lent(&self) -> bool {
        self.side().lent(&self.tx)
    }

    /// Creates again the third-party plugins `s` needs that could not be created before (a plugin
    /// scan has just found them), sending the session with them to the audio thread. Returns how
    /// many slots were missing. Call on the UI thread.
    pub fn retry_missing_plugins(&self, s: Arc<Session>) -> usize {
        let n = self.side().retry_missing();
        if n > 0 {
            self.update_session(s);
        }
        n
    }

    /// The Library the band plays from (the engine's, `Engine::band_library`). Taken by the next
    /// [`Player::update_session`]. Call on the UI thread.
    pub fn set_band_library(&self, lib: Arc<LibraryData>) {
        self.side().band_lib = Some(lib);
    }

    /// The slots whose plugin refused a preset loaded into it since the last call (it kept its
    /// defaults), for the status line.
    pub fn take_refused_presets(&self) -> Vec<(TrackId, SlotKey)> {
        std::mem::take(&mut self.side().refused_presets)
    }

    /// Restores `state` into the running third-party plugin of a slot (main thread). `false`
    /// when there is none there or it refused the state.
    pub fn load_state(&self, track: TrackId, slot: SlotKey, state: &[u8]) -> bool {
        self.side().states.get_mut(&(track, slot)).is_some_and(|r| r.load_state(state))
    }

    /// Loads one of the plugin's own presets into the running third-party plugin of a slot (see
    /// [`PluginStateReader::load_preset`]; main thread). `false` when there is none there or it
    /// refused.
    pub fn load_preset(&self, track: TrackId, slot: SlotKey, kind: u32, location: Option<&str>, load_key: Option<&str>) -> bool {
        self.side().states.get_mut(&(track, slot)).is_some_and(|r| r.load_preset(kind, location, load_key))
    }

    /// Reads the state of every live third-party plugin, for saving the session.
    pub fn capture_states(&self) -> PluginStates {
        self.side().capture_states()
    }

    /// Reads the state of one live third-party plugin (`None`: no such instance, or nothing to
    /// read yet), so a module copied, moved or removed keeps what was set in its editor.
    pub fn capture_state(&self, track: TrackId, slot: SlotKey) -> Option<Vec<u8>> {
        self.side().capture_state((track, slot))
    }

    /// Reads a slot's state with `reader` from now on, as if a plugin instance there had one.
    /// For tests of the state paths without a third-party plugin installed.
    #[doc(hidden)]
    pub fn attach_state_reader(&self, track: TrackId, slot: SlotKey, reader: Box<dyn PluginStateReader>) {
        self.side().states.insert((track, slot), reader);
    }

    /// Per-UI-frame housekeeping: drops retired instances and services plugin editors. Returns
    /// the parameter edits made in open editors since the last call.
    pub fn idle(&self) -> Vec<EditorEdit> {
        let mut side = self.side();
        side.settle(&self.tx);
        side.collect_garbage();
        let mut out = Vec::new();
        for (&(t, slot), e) in side.editors.iter_mut() {
            for (id, v) in e.idle() {
                out.push((t, slot, id, v));
            }
        }
        out
    }

    /// True when the plugin in this slot has its own editor.
    pub fn has_editor(&self, track: TrackId, slot: SlotKey) -> bool {
        self.side().editors.contains_key(&(track, slot))
    }

    /// Opens the plugin's own editor window. Main thread only.
    pub fn open_editor(&self, track: TrackId, slot: SlotKey) -> Result<(), String> {
        self.side().open_editor((track, slot))
    }

    pub fn close_editor(&self, track: TrackId, slot: SlotKey) {
        if let Some(e) = self.side().editors.get_mut(&(track, slot)) {
            e.close();
        }
    }

    /// Closes every plugin editor window (main thread). Call before replacing the player: its
    /// instances may be dropped on the audio thread, where editor windows cannot be closed.
    pub fn close_editors(&self) {
        for e in self.side().editors.values_mut() {
            e.close();
        }
    }

    /// A picture of the slot's open plugin editor as `(width, height, RGBA pixels)` (see
    /// `PluginEditor::capture`): `None` when it is closed or cannot be captured. Main thread
    /// only; it reads the editor's window, never the audio thread.
    pub fn capture_editor(&self, track: TrackId, slot: SlotKey) -> Option<(u32, u32, Vec<u8>)> {
        self.side().capture_editor((track, slot))
    }

    /// Drives a slot's editor with `editor` from now on, as if a plugin instance there had one.
    /// For tests of the editor paths without a third-party plugin installed.
    #[doc(hidden)]
    pub fn attach_editor(&self, track: TrackId, slot: SlotKey, editor: Box<dyn PluginEditor>) {
        self.side().editors.insert((track, slot), editor);
    }

    pub fn editor_open(&self, track: TrackId, slot: SlotKey) -> bool {
        self.side().editors.get(&(track, slot)).is_some_and(|e| e.is_open())
    }

    pub fn any_editor_open(&self) -> bool {
        self.side().editors.values().any(|e| e.is_open())
    }

    /// Shows a parameter change made elsewhere in the plugin's open editor.
    pub fn editor_set_param(&self, track: TrackId, slot: SlotKey, id: &str, value: f32) {
        if let Some(e) = self.side().editors.get_mut(&(track, slot)) {
            e.set_param(id, value);
        }
    }

    /// The plugin's own text for a parameter value when SoundCraft's format would be wrong for it
    /// (see `PluginEditor::value_text`). Main thread only.
    pub fn editor_value_text(&self, track: TrackId, slot: SlotKey, id: &str, value: f32) -> Option<String> {
        self.side().editors.get_mut(&(track, slot))?.value_text(id, value)
    }

    /// Arms a Scout capture (build the rig on this thread with `ScoutRig::new`). The audio thread
    /// feeds it until it ends (30 s, its `end`, [`Player::stop_scout`], a transport Stop or the
    /// end of playback; a Locate or Play only starts a new played range) and then sends it back
    /// for [`Player::take_scout`]. A rig refused because another capture is armed also comes
    /// back, with `finished()` false. For a loop capture use [`Player::scout_loop`].
    pub fn arm_scout(&self, rig: Box<ScoutRig>) {
        let _ = self.side().arm(&self.tx, rig, Cmd::ScoutArm);
    }

    /// Arms a loop capture (a rig built with the loop's start and end) and plays `range` looped
    /// from its start, in one step on the audio thread, so the capture is exactly the loop.
    /// A refused rig (see [`Player::arm_scout`]) leaves the transport as it was.
    /// Returns whether the audio thread took the rig (a refused one comes back unfinished).
    pub fn scout_loop(&self, rig: Box<ScoutRig>, range: Range) -> bool {
        self.side().scout_loop(&self.shared, &self.tx, rig, range)
    }

    /// Ends the armed Scout capture; its rig comes back through [`Player::take_scout`].
    pub fn stop_scout(&self) {
        let _ = self.tx.send(Cmd::ScoutStop);
    }

    /// A Scout rig back from the audio thread, oldest first. Finish it here, off the audio
    /// thread (`ScoutRig::finish`), and store the result with `scout.store`.
    pub fn take_scout(&self) -> Option<Box<ScoutRig>> {
        self.side().take_scout()
    }

    /// Frames captured so far by the capture last armed, while its rig is out.
    pub fn scout_progress(&self) -> Option<Arc<AtomicU64>> {
        self.side().scout_progress.clone()
    }

    /// Start playback at `from`, stopping at `end`, or looping `looped`.
    pub fn play(&self, from: Samples, end: Option<Samples>, looped: Option<Range>) {
        self.play_counted(from, end, looped, None);
    }

    /// [`Player::play`] after `countoff`: the transport holds at `from` while the count-off
    /// clicks (live input heard, nothing played or recorded), then rolls from `from`. Meanwhile
    /// [`Player::is_playing`] is true, [`Player::countoff_elapsed`] says how far it is, and the
    /// [`TransportProbe`] reads stopped (the take clock starts when the transport rolls).
    pub fn play_counted(&self, from: Samples, end: Option<Samples>, looped: Option<Range>, countoff: Option<CountOff>) {
        if self.is_lent() {
            log::info!("play refused: the engine is lent to a render");
            return;
        }
        let countoff = countoff.filter(|c| !c.is_empty());
        // Audio played from disk: the first moments from `from` (and the loop start) are read
        // before the transport rolls, waiting a moment at most; the rest streams.
        soundcraft_model::disk::set_playhead(from, false);
        soundcraft_model::disk::prepare_focused(from, PREPARE_SECS, PREPARE_WAIT);
        self.shared.position.store(from, Ordering::Relaxed);
        // The take clock is reset by the audio thread when the Play arrives (never here: a block
        // of the old pass still to render would count on from zero); until then the probe
        // reads no clock.
        self.shared.play_pending.store(true, Ordering::Release);
        self.shared.countoff_at.store(0, Ordering::Relaxed);
        self.shared.counting.store(countoff.is_some(), Ordering::Relaxed);
        self.shared.playing.store(countoff.is_none(), Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Play { from, end, looped, countoff });
    }

    /// Frames of the count-off played so far, while one runs before the transport rolls.
    pub fn countoff_elapsed(&self) -> Option<i64> {
        self.shared.counting.load(Ordering::Relaxed).then(|| self.shared.countoff_at.load(Ordering::Relaxed))
    }

    /// Feed live input into the mix (monitoring) and recording blocks into `capture`, replacing
    /// the input there was. The mix's input buffer is allocated here, off the audio thread.
    pub(crate) fn attach_input(&self, feed: Arc<feed::InputFeed>, capture: Arc<record::Capture>) {
        let buffers = vec![vec![0.0; self.max_block]; feed.channels()];
        let _ = self.tx.send(Cmd::Input(feed, buffers));
        let _ = self.tx.send(Cmd::Capture(capture));
    }

    /// A recorder on the input opened with the output (duplex), when there is one: it captures
    /// from the same clock as the output. `None`: open one with [`record::Recorder::open_with`].
    pub fn duplex_recorder(&self) -> Option<record::Recorder> {
        let d = self.duplex.as_ref()?;
        Some(record::Recorder::attached(Arc::clone(&d.capture), d.name.clone(), d.rate, self.session_rate(), d.channels))
    }

    /// The input to record and monitor from: the duplex one when it opened with the output, else
    /// a stream of its own (split). With ASIO there is no split: a second stream would restart
    /// the running driver.
    pub fn open_input(&self, cfg: &AudioConfig) -> Result<record::Recorder, String> {
        match self.duplex_recorder() {
            Some(r) => Ok(r),
            None => split_input(self.asio).and_then(|()| record::Recorder::open_with(cfg, self)),
        }
    }

    /// The session rate the mix renders at (and takes are written at).
    pub fn session_rate(&self) -> u32 {
        let r = self.side().rate;
        if r.is_finite() && r >= 1.0 { r.round() as u32 } else { 48_000 }
    }

    /// Whether a take may start: not at half speed (or any speed but 1), where the mix reads the
    /// input elastically and a take would lose input.
    pub fn can_record(&self) -> Result<(), String> {
        if self.shared.speed.load(Ordering::Relaxed) == 1000 {
            Ok(())
        } else {
            Err("Recording needs full speed: at half speed the take would lose input. Stop, then record.".into())
        }
    }

    /// Lends the audio thread the MIDI input hub: its ports play into armed and monitored
    /// instrument and MIDI tracks from the next block (keep a handle: the audio thread never drops
    /// the last one).
    pub fn set_midi(&self, hub: Arc<midi_in::MidiHub>) {
        let _ = self.tx.send(Cmd::Midi(hub));
    }

    pub fn set_recording(&self, on: bool) {
        self.shared.recording.store(on, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Recording(on));
    }

    /// The latencies the devices and the input feed report now, in frames at the session rate
    /// (see [`Latency`]). All read live: a drift feed's latency moves while it runs. On ASIO the
    /// driver's own figures, read at open, replace the callback timestamps' input and output.
    pub fn reported_latency(&self) -> Latency {
        let rate = f64::from(self.session_rate());
        let ns = |a: &AtomicU64| std::time::Duration::from_nanos(a.load(Ordering::Relaxed));
        let (input, output) = match self.driver_latency {
            // Device frames to session frames: the same duration at the session rate.
            Some((i, o)) => {
                let at = |f: u32| measure::frames(std::time::Duration::from_secs_f64(f64::from(f) / f64::from(self.device_rate.max(1))), rate);
                (at(i), at(o))
            }
            None => (measure::frames(ns(&self.shared.in_latency_ns), rate), measure::frames(ns(&self.shared.out_latency_ns), rate)),
        };
        Latency { input, output, feed: self.shared.feed_latency.load(Ordering::Relaxed), mix: self.shared.mix_latency.load(Ordering::Relaxed) }
    }

    /// Starts a loopback measurement: a click out of device channel `output_channel`, recorded
    /// on input channel `input_channel` (both 0-based) for two seconds. Poll
    /// [`Player::measure_result`] for the round trip. Refused while the transport plays or
    /// records, without an input, off the direct path (another rate, or half speed), on a
    /// channel the devices lack, and while a measurement runs.
    pub fn measure_latency(&self, output_channel: usize, input_channel: usize) -> Result<(), String> {
        if self.is_playing() || self.shared.recording.load(Ordering::Relaxed) {
            return Err("stop the transport first".into());
        }
        let inputs = self.shared.input_channels.load(Ordering::Relaxed);
        if inputs == 0 {
            return Err("no audio input".into());
        }
        if self.device_rate != self.session_rate() || self.shared.speed.load(Ordering::Relaxed) != 1000 {
            return Err("Measure needs the session and the device at the same rate".into());
        }
        if output_channel >= usize::from(self.device_channels) {
            return Err(format!("output {} is not on the device", output_channel.saturating_add(1)));
        }
        if input_channel >= inputs {
            return Err(format!("input {} is not on the device", input_channel.saturating_add(1)));
        }
        let mut side = self.side();
        if side.measure_out {
            return Err("a measurement is already running".into());
        }
        // Stale buffers out of the way first (the return channel has room for the new one).
        let _ = side.take_measure();
        let mut buf = MeasureBuf::new(output_channel, input_channel, self.device_rate);
        side.measure_id = side.measure_id.wrapping_add(1);
        buf.id = side.measure_id;
        self.tx.send(Cmd::Measure(buf)).map_err(|_| "the audio output is not running".to_string())?;
        side.measure_out = true;
        #[cfg(not(target_arch = "wasm32"))]
        {
            side.measure_since = Some(std::time::Instant::now());
        }
        Ok(())
    }

    /// Abandons the measurement out there (the UI's Cancel): Measure may start another at once,
    /// and the abandoned one is dropped when it comes back.
    pub fn cancel_measure(&self) {
        self.side().abandon_measure(&self.tx);
    }

    /// The round trip of the measurement last started, once it is back (once): `Ok(frames)`, or
    /// why there is none (no click came back, it was interrupted, or it never came back within
    /// `measure::TIMEOUT`: the output stopped). Poll on the UI thread.
    pub fn measure_result(&self) -> Option<Result<i64, String>> {
        let mut side = self.side();
        if let Some(m) = side.take_measure() {
            side.measured_feed = m.feed;
            return Some(m.result());
        }
        #[cfg(not(target_arch = "wasm32"))]
        if side.measure_out && side.measure_since.is_some_and(|t| t.elapsed() >= measure::TIMEOUT) {
            side.abandon_measure(&self.tx);
            return Some(Err("Measure timed out: the audio output stopped".into()));
        }
        None
    }

    /// The input feed's latency (frames) at the click of the measurement last back from
    /// [`Player::measure_result`]: the record offset swaps it for each take's own.
    pub fn measure_feed(&self) -> u32 {
        self.side().measured_feed
    }

    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        self.shared.counting.store(false, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Stop);
    }

    /// Sends a one-off action ([`Plugin::action`], one of [`soundcraft_dsp::plugin_actions`]) to
    /// module `module` on `track` of the playing session (the engine's `mix.module_action`).
    pub fn module_action(&self, track: TrackId, module: ModuleId, name: &'static str) {
        let _ = self.tx.send(Cmd::Action(track, module, name));
    }

    /// Plays `note` on `track`'s instrument now, whether or not the track is armed (Juke's pad
    /// audition): a note-on at `velocity`, a note-off at 0.
    pub fn audition(&self, track: TrackId, note: u8, velocity: u8) {
        let _ = self.tx.send(Cmd::Audition(track, note, velocity));
    }

    pub fn locate(&self, at: Samples) {
        soundcraft_model::disk::set_playhead(at, self.shared.playing.load(Ordering::Relaxed));
        self.shared.position.store(at, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Locate(at));
    }

    /// Sets the playback rate. While a take records only full speed is taken (see
    /// [`Player::can_record`]).
    pub fn set_speed(&self, speed: f32) {
        let v = if speed.is_finite() { (speed.clamp(0.1, 4.0) * 1000.0) as u32 } else { 1000 };
        if v != 1000 && self.shared.recording.load(Ordering::Relaxed) {
            log::info!("half speed is not available while recording");
            return;
        }
        self.shared.speed.store(v, Ordering::Relaxed);
    }

    /// Playback rate last requested, 1.0 at full speed.
    pub fn speed(&self) -> f32 {
        self.shared.speed.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn position(&self) -> Samples {
        self.shared.position.load(Ordering::Relaxed)
    }

    /// A handle other threads (the LAN remote's network threads) read the transport through,
    /// lock-free, without the player.
    pub fn probe(&self) -> TransportProbe {
        TransportProbe { shared: Arc::clone(&self.shared) }
    }

    /// The host's tap on its main mix (the remote mix stream reads it on a network thread).
    pub fn mix_tap(&self) -> Arc<stream::MixTap> {
        Arc::clone(&self.shared.tap)
    }

    /// The performer's end of the remote mix stream (fed on a network thread).
    pub fn stream_in(&self) -> Arc<stream::StreamIn> {
        Arc::clone(&self.shared.stream_in)
    }

    /// The transport rolls, or counts off to roll.
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed) || self.shared.counting.load(Ordering::Relaxed)
    }

    /// True while the callback renders the device's frames 1:1 (equal rates, speed 1), without
    /// the resampler.
    pub fn direct(&self) -> bool {
        self.shared.direct.load(Ordering::Relaxed)
    }

    /// Frames the device's last output callback asked for (on the direct path, the mix block),
    /// once one came.
    pub fn callback_frames(&self) -> Option<u32> {
        Some(self.shared.callback_frames.load(Ordering::Relaxed)).filter(|n| *n > 0)
    }

    /// Callbacks that came late or after a gap since the last [`Player::reset_underruns`].
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    pub fn reset_underruns(&self) {
        self.shared.underruns.store(0, Ordering::Relaxed);
    }

    /// The peak share of a callback's period spent filling it since the last call (1.0: the
    /// whole period).
    pub fn dsp_load(&self) -> f32 {
        self.shared.load_permille.swap(0, Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn meters(&self) -> MeterSnapshot {
        self.shared.meters.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Copy meters into reusable UI storage. Once track/channel counts are stable this performs
    /// no allocation, unlike cloning a new snapshot every video frame.
    pub fn copy_meters(&self, out: &mut MeterSnapshot) {
        let source = self.shared.meters.lock().unwrap_or_else(PoisonError::into_inner);
        copy_meter_snapshot(&source, out);
    }
}

fn copy_meter_snapshot(source: &MeterSnapshot, out: &mut MeterSnapshot) {
    out.tracks.retain(|id, _| source.tracks.contains_key(id));
    for (id, meter) in &source.tracks {
        out.tracks.entry(*id).or_default().copy_from(meter);
    }
    out.main.copy_from(&source.main);
}

impl Side {
    fn capture_state(&mut self, key: (TrackId, SlotKey)) -> Option<Vec<u8>> {
        if self.refused.contains(&key) {
            return None;
        }
        self.states.get_mut(&key)?.save_state()
    }

    fn capture_states(&mut self) -> PluginStates {
        self.states.iter_mut().filter(|(k, _)| !self.refused.contains(*k)).filter_map(|(&(t, slot), r)| Some((t, slot, r.save_state()?))).collect()
    }

    fn capture_editor(&mut self, key: (TrackId, SlotKey)) -> Option<(u32, u32, Vec<u8>)> {
        self.editors.get_mut(&key)?.capture()
    }

    fn open_editor(&mut self, key: (TrackId, SlotKey)) -> Result<(), String> {
        let e = self.editors.get_mut(&key).ok_or_else(|| "this plugin has no editor (or it is not loaded)".to_string())?;
        e.open()?;
        self.refused.remove(&key);
        Ok(())
    }

    /// The band's update for `s` (after [`Side::prepare`], so the instances' names are known):
    /// `None` without a Library or without a musician input.
    fn prepare_band(&mut self, s: &Session) -> Option<Box<BandUpdate>> {
        let Side { names, band_lib, band_cache, .. } = self;
        let lib = band_lib.as_ref()?;
        // The instrument's `info().name`, as the offline render reads it from its instances: a
        // live third-party instance's own, a built-in's from the registry.
        let name = |t: TrackId| {
            names.get(&(t, SlotKey::Instrument)).cloned().or_else(|| {
                let id = &s.track(t)?.instrument.as_ref()?.plugin;
                soundcraft_dsp::plugin_info(id).map(|i| i.name.to_string())
            })
        };
        soundcraft_mix::prepare_band_cached(s, lib, &name, band_cache)
    }

    /// Creates the third-party instances `s` needs that the audio thread does not have yet.
    fn prepare(&mut self, s: &Session) -> Vec<PreparedInstance> {
        self.collect_garbage();
        let sr = s.sample_rate.as_f64() as f32;
        if (sr - self.rate).abs() > 0.5 {
            // The audio thread rebuilds its engine: everything is created anew.
            let keys: Vec<(TrackId, SlotKey)> = self.shadow.keys().copied().collect();
            for k in keys {
                self.forget(k);
            }
            self.rate = sr;
            // ... the band's runtime too: the next update sends one built for this rate.
            self.band_installed = false;
        }
        let specs = soundcraft_mix::instance_specs(s);
        let stale: Vec<(TrackId, SlotKey)> = self.shadow.keys().filter(|k| !specs.iter().any(|x| (x.track, x.slot) == **k)).copied().collect();
        for k in stale {
            self.forget(k);
        }
        // Instances made ahead for this session go first; the loop below sees them as current.
        let mut out = self.take_preloaded(&specs, sr);
        for spec in specs {
            let key = (spec.track, spec.slot);
            let same = self.shadow.get(&key).filter(|(id, ch, _, _)| *id == spec.id && *ch == spec.ch).map(|(_, _, load, _)| *load);
            if same == Some(spec.state_load) {
                continue;
            }
            // A preset loaded into the module (a new `state_load`): into the running instance,
            // which keeps its editor and its audio; made anew only when the plugin refuses it.
            let preset = same.is_some();
            if preset
                && let (Some(state), Some(r)) = (spec.state.as_deref(), self.states.get_mut(&key))
                && r.load_state(state)
            {
                self.refused.remove(&key);
                if let Some(e) = self.shadow.get_mut(&key) {
                    e.2 = spec.state_load;
                }
                continue;
            }
            self.forget(key);
            let made = soundcraft_mix::create_instance(&spec.id, spec.state.as_deref(), sr, self.max_block, spec.ch);
            log::debug!("plugin instance for {:?} {:?}: {} ({} ch, created: {})", spec.track, spec.slot, spec.id, spec.ch, made.is_some());
            self.shadow.insert(key, (spec.id.clone(), spec.ch, spec.state_load, made.is_some()));
            if let Some(soundcraft_mix::NewInstance { mut plugin, refused_state }) = made {
                self.names.insert(key, plugin.info().name.to_string());
                if let Some(e) = plugin.editor() {
                    self.editors.insert(key, e);
                }
                if let Some(r) = plugin.state_reader() {
                    self.states.insert(key, r);
                }
                if refused_state {
                    self.refused.insert(key);
                    if preset {
                        self.refused_presets.push(key);
                    }
                }
                out.push(PreparedInstance { track: spec.track, slot: spec.slot, id: spec.id, ch: spec.ch, plugin });
            }
        }
        out
    }
}

/// A lend in progress ([`Player::lend_instances`]): the engine arrives from the audio thread
/// within a callback or two. Dropped before it arrives, the lend is called off: the engine goes
/// straight back, now or when it arrives (the player keeps polling for it, see [`LendRx`]).
pub struct LendingInstances {
    rx: LendRx,
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
    session: Arc<Session>,
    returned: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    /// `poll` handed the engine over: nothing is on its way any more.
    taken: bool,
    #[cfg(not(target_arch = "wasm32"))]
    since: std::time::Instant,
}

impl LendingInstances {
    /// The instances, once the audio thread has handed them over. Each call polls once.
    pub fn poll(&mut self) -> Option<LiveInstances> {
        let eng = self.rx.lock().unwrap_or_else(PoisonError::into_inner).try_recv().ok()?;
        self.taken = true;
        let tx = self.tx.clone();
        let shared = Arc::clone(&self.shared);
        let returned = Arc::clone(&self.returned);
        Some(LiveInstances::new(eng, Arc::clone(&self.session), move |e| {
            // The player gone (shut) or its audio thread gone (the send fails): the engine is
            // dropped here, on the UI thread, never handed to a dying audio thread.
            if !shared.shut.load(Ordering::Acquire) {
                let _ = tx.send(Cmd::Return(e));
            }
            returned.store(true, Ordering::Release);
        }))
    }

    /// How long the lend has been waiting for the audio thread (0 in the browser).
    pub fn waited(&self) -> std::time::Duration {
        #[cfg(not(target_arch = "wasm32"))]
        return self.since.elapsed();
        #[cfg(target_arch = "wasm32")]
        std::time::Duration::ZERO
    }
}

impl Drop for LendingInstances {
    fn drop(&mut self) {
        if self.taken {
            return;
        }
        // Arrived but never taken: straight back. Not yet arrived: the receiver stays with the
        // player's loan, which sends the engine back when it arrives (`Side::settle`); the loan
        // is only over once the audio thread has answered.
        match self.rx.lock().unwrap_or_else(PoisonError::into_inner).try_recv() {
            Ok(e) => {
                let _ = self.tx.send(Cmd::Return(e));
                self.returned.store(true, Ordering::Release);
            }
            Err(_) => self.cancelled.store(true, Ordering::Release),
        }
    }
}

/// The audio state of a [`Player::stalled`] player: fill it to make its "audio thread" answer.
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub struct StalledAudio(AudioState);

#[cfg(not(target_arch = "wasm32"))]
impl StalledAudio {
    /// One callback of `out.len() / channels` frames.
    pub fn fill(&mut self, out: &mut [f32], channels: usize) {
        self.0.fill(out, channels);
    }

    /// The session the audio side renders now (the one last swapped in).
    pub fn session(&self) -> &Arc<Session> {
        &self.0.session
    }

    /// How many hosted (third-party) instances the audio side holds.
    pub fn hosted_instances(&self) -> usize {
        self.0.mix.hosted_instances()
    }
}

/// Start a freshly built output stream. `false` when the device refuses to start it, so the caller
/// treats it like a stream that failed to build and tries the next configuration.
fn start_stream(stream: &impl cpal::traits::StreamTrait) -> bool {
    match stream.play() {
        Ok(()) => true,
        Err(e) => {
            log::warn!("audio stream failed to start: {e}");
            false
        }
    }
}

/// [`max_block_for`] the buffer size of a stream configuration.
fn max_block_of(config: &cpal::StreamConfig) -> usize {
    max_block_for(match config.buffer_size {
        cpal::BufferSize::Fixed(n) => Some(n),
        cpal::BufferSize::Default => None,
    })
}

/// A built output stream and the channels and fault flag its callback shares with the player.
struct Opened {
    stream: cpal::Stream,
    tx: Sender<Cmd>,
    grx: Receiver<Garbage>,
    fault: Arc<Fault>,
    /// The duplex input built with it, and whether it started.
    input: Option<record::InputStream>,
    input_started: std::cell::Cell<bool>,
}

/// True when input and output open as one device on one clock (duplex): always with ASIO, and
/// elsewhere when the chosen input and output are the same named device.
pub fn duplex(host: &str, cfg: &AudioConfig) -> bool {
    host.eq_ignore_ascii_case("ASIO") || (cfg.input.is_some() && cfg.input == cfg.output)
}

/// The ASIO driver's own latencies (see `Player::driver_latency`). UI thread only, after the
/// streams started; any error leaves the timestamp estimate in place.
#[cfg(all(feature = "asio", windows))]
fn asio_driver_latency() -> Option<(u32, u32)> {
    match soundcraft_asio_latency::driver_latencies() {
        Ok(l) => {
            log::info!("ASIO driver latency: {} in, {} out (frames)", l.0, l.1);
            Some(l)
        }
        Err(e) => {
            log::warn!("ASIO driver latency unavailable ({e}); using the callback timestamps");
            None
        }
    }
}

#[cfg(not(all(feature = "asio", windows)))]
fn asio_driver_latency() -> Option<(u32, u32)> {
    None
}

/// Whether the input may open as a stream of its own: not with ASIO, where it can only open with
/// the output.
fn split_input(asio: bool) -> Result<(), String> {
    if asio { Err("The ASIO input could not open with the output; reopen the device".into()) } else { Ok(()) }
}

/// The feed of a duplex input of `channels` at `rate`: at the session rate a fixed one (one
/// clock, no conversion), with no cushion for ASIO (input then output, always) and one block
/// queued ahead elsewhere (callback order not guaranteed); else a drift feed that converts.
fn duplex_feed(asio: bool, channels: usize, rate: u32, session_rate: u32, max_block: usize) -> Arc<feed::InputFeed> {
    if rate != session_rate {
        feed::InputFeed::drift(channels, rate, session_rate, max_block)
    } else if asio {
        feed::InputFeed::fixed(channels, max_block)
    } else {
        feed::InputFeed::fixed_cushioned(channels, max_block)
    }
}

/// The duplex input on `device` at the output's rate and buffer (see [`duplex_feed`]).
fn duplex_input(
    device: &cpal::Device,
    output: &cpal::StreamConfig,
    session_rate: u32,
    max_block: usize,
    asio: bool,
    route: Option<&jack::Route>,
) -> Result<record::InputStream, String> {
    let buffer = match output.buffer_size {
        cpal::BufferSize::Fixed(n) => Some(n),
        cpal::BufferSize::Default => None,
    };
    record::build_input_stream(device, Some(output.sample_rate.0), buffer, session_rate, route, |ch, rate| {
        duplex_feed(asio, ch, rate, session_rate, max_block)
    })
}

/// On JACK, the route for an input stream on `device` (cpal's one input) to the device named
/// `want` (else the first); `None` elsewhere.
pub(crate) fn jack_input_route(host: &cpal::Host, device: &cpal::Device, want: Option<&str>) -> Option<jack::Route> {
    if !jack::is_jack(host.id().name()) {
        return None;
    }
    #[allow(deprecated)]
    let cpal_name = cpal::traits::DeviceTrait::name(device).ok()?;
    jack::Route::prepare(&cpal_name, want, false)
}

/// Build and start the attempts in order; the first that builds and starts wins. A build error or
/// a stream that will not start is logged, and the next attempt is tried. `None` when none works.
fn first_started<A: std::fmt::Debug, S, E: std::fmt::Display>(
    attempts: impl IntoIterator<Item = A>,
    mut build: impl FnMut(&A) -> Result<S, E>,
    start: impl Fn(&S) -> bool,
) -> Option<(A, S)> {
    for attempt in attempts {
        match build(&attempt) {
            Ok(built) if start(&built) => return Some((attempt, built)),
            // `start` has already logged why the stream would not start.
            Ok(_) => {}
            Err(e) => log::warn!("cannot open audio stream ({attempt:?}): {e}"),
        }
    }
    None
}

/// The output stream: the mix renders f32 into a scratch buffer allocated here (so the callback
/// never allocates), converted to the device's sample type.
fn build_output<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut state: AudioState,
    channels: usize,
    fault: &Arc<Fault>,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let frame = channels.max(1);
    let mut scratch = vec![0.0f32; 8192 * frame];
    let chunk = scratch.len();
    let (failed, guard, gone) = (Arc::clone(fault), Arc::clone(shared), Arc::clone(fault));
    #[cfg(not(target_arch = "wasm32"))]
    let (rate, mut timing) = (f64::from(config.sample_rate.0), Timing::new(config.buffer_size));
    guarded_build(move || {
        device.build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                mark_audio_thread();
                #[cfg(not(target_arch = "wasm32"))]
                let started = std::time::Instant::now();
                for out in data.chunks_mut(chunk) {
                    let Some(s) = scratch.get_mut(..out.len()) else { return };
                    guarded_block(&failed, &guard, s, |d| state.fill(d, channels));
                    for (o, x) in out.iter_mut().zip(s.iter()) {
                        *o = T::from_sample(*x);
                    }
                }
                // The reported output latency: from this callback until its audio plays.
                let at = info.timestamp();
                if let Some(d) = at.playback.duration_since(&at.callback) {
                    guard.out_latency_ns.store(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
                }
                #[cfg(not(target_arch = "wasm32"))]
                timing.callback(&guard, at.playback, started.elapsed(), data.len() / frame, rate);
            },
            // cpal may call this on the audio thread (on ALSA in a loop while the device stays
            // broken): marked, so the logger hands the record off instead of writing it.
            move |e: cpal::StreamError| {
                mark_audio_thread();
                if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                    gone.gone.store(true, Ordering::Relaxed);
                }
                log::warn!("audio stream error: {e}");
            },
            None,
        )
    })
}

/// Builds a stream, a panic inside the backend reported as a build error (cpal's JACK host
/// unwraps the client's activation): the caller falls back as for any device that will not open.
pub(crate) fn guarded_build(build: impl FnOnce() -> Result<cpal::Stream, cpal::BuildStreamError>) -> Result<cpal::Stream, cpal::BuildStreamError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)).unwrap_or_else(|p| {
        let description = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
        Err(cpal::BuildStreamError::BackendSpecific {
            err: cpal::BackendSpecificError { description: format!("the audio backend failed: {description}") },
        })
    })
}

/// Callback sizes remembered to judge the device's real period.
#[cfg(not(target_arch = "wasm32"))]
const RECENT: usize = 8;

/// The output callback's timing, to count underruns and the DSP load.
#[cfg(not(target_arch = "wasm32"))]
struct Timing {
    /// The playback time of the first callback; later ones are measured from it.
    origin: Option<cpal::StreamInstant>,
    /// When the last callback's audio ends playing (since `origin`): the next should start then.
    expected: Option<std::time::Duration>,
    /// The configured buffer in frames (0: the device default).
    configured: usize,
    /// The frames of the last [`RECENT`] callbacks, round robin.
    recent: [usize; RECENT],
    next: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl Timing {
    fn new(buffer: cpal::BufferSize) -> Timing {
        let configured = match buffer {
            cpal::BufferSize::Fixed(n) => n as usize,
            cpal::BufferSize::Default => 0,
        };
        Timing { origin: None, expected: None, configured, recent: [0; RECENT], next: 0 }
    }

    /// The device's real period in frames: the larger of the configured buffer and the recent
    /// callbacks (WASAPI shared mode may hand 480, then 240, then 480).
    fn period(&self) -> usize {
        self.recent.iter().copied().fold(self.configured, usize::max)
    }

    /// Records a callback of `frames` whose audio plays at stream time `at` and that took
    /// `elapsed` to fill: counts one underrun when it was late or its audio starts after a gap,
    /// and raises the peak load (a share of the real period).
    fn callback(&mut self, shared: &Shared, at: cpal::StreamInstant, elapsed: std::time::Duration, frames: usize, rate: f64) {
        if frames == 0 {
            return;
        }
        if let Some(slot) = self.recent.get_mut(self.next) {
            *slot = frames;
        }
        self.next = (self.next + 1) % RECENT;
        let period = self.period();
        let origin = *self.origin.get_or_insert(at);
        let now = at.duration_since(&origin);
        let gapped = matches!((self.expected, now), (Some(expected), Some(now)) if gap(expected, now, period, rate));
        if late(elapsed, period, rate) || gapped {
            shared.underruns.fetch_add(1, Ordering::Relaxed);
        }
        let length = std::time::Duration::try_from_secs_f64(frames as f64 / rate.max(1.0)).ok();
        self.expected = now.zip(length).and_then(|(n, l)| n.checked_add(l));
        let period = period as f64 / rate.max(1.0);
        let load = (elapsed.as_secs_f64() / period * 1000.0).clamp(0.0, f64::from(u32::MAX)) as u32;
        shared.load_permille.fetch_max(load, Ordering::Relaxed);
    }
}

/// Open the output device `cfg` chooses (else the default) at the chosen rate (else the session
/// rate) if possible, in the best sample format it offers (f32, else 32- or 16-bit integers),
/// with as many channels as it offers up to 64 (at least stereo when available: the main mix and
/// the hardware outputs land on their own channels), and the
/// chosen buffer size when the device allows it.
#[allow(clippy::type_complexity)]
fn open_device(
    want_rate: u32,
    cfg: &AudioConfig,
) -> Result<(cpal::Host, cpal::Device, cpal::StreamConfig, cpal::SampleFormat, String, Option<jack::Route>), String> {
    use cpal::traits::DeviceTrait;
    let host = devices::host(cfg);
    let device = devices::device(&host, cfg.output.as_deref(), true).ok_or_else(|| "no output device".to_string())?;
    #[allow(deprecated)]
    let name = device.name().unwrap_or_else(|_| "Audio device".into());
    // On JACK the stream is cpal's one output, connected to the chosen device once it runs; it
    // goes by that device's name.
    let route = jack::is_jack(host.id().name()).then(|| jack::Route::prepare(&name, cfg.output.as_deref(), true)).flatten();
    let name = route.as_ref().map_or(name, |r| r.group.name.clone());
    // Prefer a config at the chosen (else session) rate; else the device default (we resample).
    let rates: Vec<u32> = cfg.sample_rate.into_iter().chain(Some(want_rate)).collect();
    let mut chosen: Option<(cpal::StreamConfig, cpal::SupportedBufferSize, cpal::SampleFormat)> = None;
    // Better: a preferred format first, then more channels.
    let better = |c: &cpal::SupportedStreamConfigRange, x: &(cpal::StreamConfig, cpal::SupportedBufferSize, cpal::SampleFormat)| {
        let (rc, rx) = (devices::format_rank(c.sample_format()).unwrap_or(usize::MAX), devices::format_rank(x.2).unwrap_or(usize::MAX));
        rc < rx || (rc == rx && c.channels() > x.0.channels)
    };
    if let Ok(configs) = device.supported_output_configs() {
        let configs: Vec<cpal::SupportedStreamConfigRange> = configs.collect();
        for rate in rates {
            for c in &configs {
                if devices::format_rank(c.sample_format()).is_some()
                    && c.min_sample_rate().0 <= rate
                    && c.max_sample_rate().0 >= rate
                    && c.channels() >= 2
                    && usize::from(c.channels()) <= MAX_DEVICE_CHANNELS
                    && chosen.as_ref().is_none_or(|x| better(c, x))
                {
                    chosen = Some((c.with_sample_rate(cpal::SampleRate(rate)).config(), *c.buffer_size(), c.sample_format()));
                }
            }
            if chosen.is_some() {
                break;
            }
        }
    }
    let (mut config, format) = match chosen {
        Some((mut c, buffers, format)) => {
            c.buffer_size = devices::buffer_size(cfg.buffer_size, &buffers);
            (c, format)
        }
        None => {
            let d = device.default_output_config().map_err(|e| e.to_string())?;
            if devices::format_rank(d.sample_format()).is_none() {
                return Err(format!("unsupported device sample format {:?}", d.sample_format()));
            }
            let buffers = *d.buffer_size();
            let format = d.sample_format();
            let mut c = d.config();
            c.buffer_size = devices::buffer_size(cfg.buffer_size, &buffers);
            (c, format)
        }
    };
    if let Some(r) = &route {
        config.channels = r.channels();
    }
    Ok((host, device, config, format, name, route))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that builds; `starts` says whether the device lets it play (it can refuse after a
    /// reconnect).
    struct Fake {
        starts: bool,
    }

    impl cpal::traits::StreamTrait for Fake {
        fn play(&self) -> Result<(), cpal::PlayStreamError> {
            if self.starts { Ok(()) } else { Err(cpal::PlayStreamError::DeviceNotAvailable) }
        }
        fn pause(&self) -> Result<(), cpal::PauseStreamError> {
            Ok(())
        }
    }

    /// Needs a JACK server (`jackd --no-realtime -d dummy -C 2 -P 2`); skipped without one.
    #[cfg(target_os = "linux")]
    #[test]
    fn jack_streams_open_one_channel_per_port_and_connect_to_the_device() {
        let Some(server) = jack::probe() else {
            println!("SKIPPED: no JACK server");
            return;
        };
        let (Some(source), Some(sink)) = (server.sources.first().cloned(), server.sinks.first().cloned()) else {
            println!("SKIPPED: the JACK server has no physical ports");
            return;
        };
        // Listed with one device per JACK client, at the server's rate.
        let systems = devices::audio_systems();
        let listed = systems.iter().find(|s| jack::is_jack(&s.name)).expect("JACK is listed while a server runs");
        assert_eq!(listed.inputs.first().map(|d| (d.name.as_str(), usize::from(d.max_channels))), Some((source.name.as_str(), source.ports.len())));
        assert_eq!(listed.outputs.first().map(|d| d.sample_rates.clone()), Some(vec![server.rate]));
        let cfg =
            AudioConfig { system: Some("JACK".into()), output: Some(sink.name.clone()), input: Some(source.name.clone()), ..Default::default() };
        // Each of `ours` is connected to exactly its partner in `theirs`.
        let check = |prefix: &str, theirs: &[String]| {
            let (c, _) = ::jack::Client::new("FreeDAWm test", ::jack::ClientOptions::NO_START_SERVER).unwrap();
            let ours = jack::in_channel_order(c.ports(Some(prefix), None, ::jack::PortFlags::empty()));
            assert_eq!(ours.len(), theirs.len(), "{ours:?}");
            for (o, t) in ours.iter().zip(theirs) {
                assert_eq!(c.port_by_name(o).unwrap().get_connections(), vec![t.clone()], "{o}");
            }
        };
        // The output: the sink's name, one channel per port, connected.
        let player = Player::with_config(Arc::new(Session::new("JACK", soundcraft_time::SampleRate::new(server.rate).unwrap_or_default())), &cfg);
        assert_eq!(player.system, "JACK");
        assert_eq!(player.device_name, sink.name);
        assert_eq!(usize::from(player.device_channels), sink.ports.len());
        check("^cpal_client_out", &sink.ports);
        // The input, on its own stream: the source's name, one channel per port, connected.
        let recorder = player.open_input(&cfg).expect("the JACK input opens");
        assert_eq!(recorder.device_name, source.name);
        assert_eq!(recorder.channels, source.ports.len());
        check("^cpal_client_in", &source.ports);
        drop(recorder);
        drop(player);
    }

    #[test]
    fn a_stream_that_will_not_start_is_reported_as_failed() {
        assert!(!start_stream(&Fake { starts: false }));
        assert!(start_stream(&Fake { starts: true }));
    }

    #[test]
    fn the_first_config_that_starts_wins_and_later_ones_are_not_tried() {
        let mut tried = Vec::new();
        let won = first_started(
            [1u32, 2, 3],
            |&n| {
                tried.push(n);
                Ok::<_, String>(Fake { starts: n != 1 })
            },
            start_stream,
        );
        assert_eq!(won.map(|(n, _)| n), Some(2));
        assert_eq!(tried, vec![1, 2]);
    }

    #[test]
    fn a_config_that_fails_to_build_falls_through_to_the_next() {
        let won =
            first_started([1u32, 2], |&n| if n == 1 { Err("no such buffer size".to_string()) } else { Ok(Fake { starts: true }) }, start_stream);
        assert_eq!(won.map(|(n, _)| n), Some(2));
    }

    #[test]
    fn no_config_that_starts_yields_none_so_the_caller_falls_back_to_a_silent_clock() {
        let mut tried = 0;
        let won = first_started(
            [1u32, 2],
            |_| {
                tried += 1;
                Ok::<_, String>(Fake { starts: false })
            },
            start_stream,
        );
        assert!(won.is_none());
        assert_eq!(tried, 2);
    }

    #[test]
    fn reusable_meter_snapshot_updates_and_drops_stale_tracks() {
        let keep = TrackId(1);
        let stale = TrackId(2);
        let mut source = MeterSnapshot::default();
        source.tracks.insert(keep, StripMeter { peak: [0.5, 0.25], peaks: vec![0.5, 0.25], gain_reduction_db: 3.0, ..StripMeter::default() });
        source.main.peaks = vec![0.75, 0.5];
        let mut out = MeterSnapshot::default();
        out.tracks.insert(stale, StripMeter::default());
        copy_meter_snapshot(&source, &mut out);
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[&keep], source.tracks[&keep]);
        assert_eq!(out.main, source.main);
    }

    #[test]
    fn a_block_that_panics_goes_silent_and_is_reported_once() {
        let shared = {
            let s = Shared::new();
            s.playing.store(true, Ordering::Relaxed);
            s
        };
        let fault = Fault::default();
        let mut out = [0.5f32; 8];
        guarded_block(&fault, &shared, &mut out, |o| o.fill(0.25));
        assert_eq!((out, fault.trouble(false)), ([0.25; 8], None));
        guarded_block(&fault, &shared, &mut out, |_| std::panic::panic_any("a bug in the mix"));
        guarded_block(&fault, &shared, &mut out, |o| o.fill(0.25));
        assert_eq!(out, [0.0; 8]);
        assert!(!shared.playing.load(Ordering::Relaxed));
        assert_eq!((fault.trouble(false), fault.trouble(false)), (Some(OutputTrouble::Bug), None));
        assert_eq!(fault.beats.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn a_lost_or_stalled_device_is_reported_once() {
        let ago = |seen: &mut Option<(u64, std::time::Instant)>| {
            if let Some(s) = seen {
                s.1 = std::time::Instant::now().checked_sub(STALL).unwrap();
            }
        };
        let mut seen = None;
        assert!(!stalled(&mut seen, 5, true));
        ago(&mut seen);
        assert!(stalled(&mut seen, 5, true));
        assert!(!stalled(&mut seen, 6, true), "it asked again");
        ago(&mut seen);
        assert!(!stalled(&mut seen, 6, false), "stopped, so nothing to ask for");
        assert_eq!(Fault::default().trouble(true), Some(OutputTrouble::Stalled));
        let fault = Fault::default();
        fault.gone.store(true, Ordering::Relaxed);
        assert_eq!((fault.trouble(false), fault.trouble(true)), (Some(OutputTrouble::Gone), None));
    }

    #[test]
    fn audio_state_plays_and_stops_at_end() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, max_block_for(None), channel().0);
        tx.send(Cmd::Play { from: 0, end: Some(1000), looped: None, countoff: None }).unwrap();
        let mut out = vec![0.0f32; 4096];
        st.fill(&mut out, 2);
        assert!(shared.position.load(Ordering::Relaxed) >= 1000);
        assert!(!shared.playing.load(Ordering::Relaxed));
    }

    /// An instrument track (the built-in synth) taking MIDI from all inputs, armed or not, and
    /// a player state lent a MIDI hub with one open port; the port's sender.
    fn midi_state(armed: bool) -> (AudioState, Sender<Cmd>, Arc<midi_in::MidiHub>, midi_in::PortSink) {
        midi_state_at(armed, 48_000.0)
    }

    /// [`midi_state`] on a device at `device_rate` (another rate than the session's resamples).
    fn midi_state_at(armed: bool, device_rate: f64) -> (AudioState, Sender<Cmd>, Arc<midi_in::MidiHub>, midi_in::PortSink) {
        use soundcraft_model::{ChannelFormat, Insert, MidiIn, Route, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let tr = s.track_mut(t).unwrap();
        tr.instrument = Some(Insert::new("subtractive_synth"));
        tr.mixer.input = Route::Midi(MidiIn::all());
        tr.mixer.record_arm = armed;
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::new(Shared::new()), Arc::new(s), device_rate, max_block_for(None), channel().0);
        st.mix.set_external_instances(false);
        let hub = midi_in::MidiHub::new();
        let keys = hub.typing();
        tx.send(Cmd::Midi(Arc::clone(&hub))).unwrap();
        (st, tx, hub, keys)
    }

    fn peak(out: &[f32]) -> f32 {
        out.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    #[test]
    fn an_armed_instrument_plays_the_keyboard_with_the_transport_stopped() {
        let (mut st, _tx, hub, keys) = midi_state(true);
        let mut out = vec![0.0f32; 2 * 256];
        st.fill(&mut out, 2);
        // Played a block ago: at the start of the next block.
        keys.send_at(hub.now_us().saturating_sub(5_400), &[0x90, 60, 110]);
        let mut loud = 0.0f32;
        for _ in 0..8 {
            st.fill(&mut out, 2);
            loud = loud.max(peak(&out));
        }
        assert!(loud > 1e-3, "the synth sounds: {loud}");
        assert!(!st.playing);

        // Not armed: the callback stays silent and the message is not played later.
        let (mut st, _tx, _hub, keys) = midi_state(false);
        keys.send(&[0x90, 60, 110]);
        let mut loud = 0.0f32;
        for _ in 0..8 {
            st.fill(&mut out, 2);
            loud = loud.max(peak(&out));
        }
        assert_eq!(loud, 0.0);
    }

    #[test]
    fn a_key_held_with_the_transport_stopped_keeps_sounding() {
        let (mut st, _tx, hub, keys) = midi_state(true);
        let mut out = vec![0.0f32; 2 * 256];
        st.fill(&mut out, 2);
        keys.send_at(hub.now_us().saturating_sub(5_400), &[0x90, 60, 110]);
        // Half a second of stopped blocks at the same position, the key still down.
        for _ in 0..100 {
            st.fill(&mut out, 2);
        }
        assert!(peak(&out) > 1e-3, "still sounding: {}", peak(&out));
    }

    #[test]
    fn disarming_the_only_live_track_lets_go_of_its_held_key() {
        let (mut st, tx, hub, keys) = midi_state(true);
        let mut out = vec![0.0f32; 2 * 256];
        st.fill(&mut out, 2);
        keys.send_at(hub.now_us().saturating_sub(5_400), &[0x90, 60, 110]);
        for _ in 0..8 {
            st.fill(&mut out, 2);
        }
        assert!(peak(&out) > 1e-3);
        // Disarmed with the key down: nothing hears live input now, so the mix stops rendering.
        let mut s = (*st.session).clone();
        s.tracks[0].mixer.record_arm = false;
        tx.send(Cmd::Session(Arc::new(s.clone()), Vec::new())).unwrap();
        // The instrument runs on while the note releases (heard, not frozen), then the mix stops.
        st.fill(&mut out, 2);
        let mut tail = peak(&out);
        st.fill(&mut out, 2);
        tail = tail.max(peak(&out));
        assert!(tail > 1e-4, "the release is heard: {tail}");
        for _ in 0..1000 {
            st.fill(&mut out, 2);
        }
        assert!(!st.monitoring && !st.mix.live_tail(), "released, the mix stops rendering");
        assert_eq!(peak(&out), 0.0);
        // Armed again a while later (the key's note-off was never heard): the note must not
        // come back sounding.
        s.tracks[0].mixer.record_arm = true;
        tx.send(Cmd::Session(Arc::new(s), Vec::new())).unwrap();
        for _ in 0..400 {
            st.fill(&mut out, 2);
        }
        assert!(peak(&out) < 1e-4, "a stuck note: {}", peak(&out));
    }

    #[test]
    fn a_recorded_event_is_where_it_arrived_whatever_the_block_size() {
        // A callback of two mix blocks: the event that just came lands at the first block's end
        // (one block behind its arrival); recorded, it is back where it arrived, the block's
        // start, not a callback's length earlier.
        let (mut st, tx, hub, keys) = midi_state(true);
        tx.send(Cmd::Recording(true)).unwrap();
        tx.send(Cmd::Play { from: 48_000, end: None, looped: None, countoff: None }).unwrap();
        let mut out = vec![0.0f32; 2 * 4096];
        st.fill(&mut out, 2);
        keys.send(&[0x90, 60, 110]);
        st.fill(&mut out, 2);
        let mut got = Vec::new();
        hub.take_recorded(&mut got);
        assert_eq!(got.len(), 1);
        assert!((52_096 - 50..=52_096).contains(&got[0].at), "{}", got[0].at);
    }

    #[test]
    fn a_recorded_event_keeps_its_age_in_a_block_cut_short_by_the_loop() {
        // A 1040-frame loop in 256-frame callbacks: the fifth callback starts 16 frames before
        // the loop's end (a 16-frame block, then the wrap). An event 100 samples old then was
        // played 100 samples before that callback's start, not at the short block's start.
        let (mut st, tx, hub, keys) = midi_state(true);
        tx.send(Cmd::Recording(true)).unwrap();
        tx.send(Cmd::Play { from: 48_000, end: None, looped: Some(Range::new(48_000, 49_040)), countoff: None }).unwrap();
        let mut out = vec![0.0f32; 2 * 256];
        for _ in 0..4 {
            st.fill(&mut out, 2);
        }
        // (The hub's clock must be older than the event's age.)
        std::thread::sleep(std::time::Duration::from_millis(3));
        keys.send_at(hub.now_us() - 2_083, &[0x90, 60, 110]);
        st.fill(&mut out, 2);
        let mut got = Vec::new();
        hub.take_recorded(&mut got);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].pass, 0);
        assert!((49_024 - 104..=49_024 - 98).contains(&got[0].at), "{}", got[0].at);
    }

    #[test]
    fn a_note_played_as_record_starts_from_stopped_monitoring_is_taken_at_the_record_point() {
        // A note played as Record is pressed, before the transport rolls (here a few µs before
        // the first callback after Play, with stopped monitoring blocks rendered ahead), is the
        // take's first note: at the record point, not just before it (where it was dropped).
        let (mut st, tx, hub, keys) = midi_state_at(true, 44_100.0);
        let mut out = vec![0.0f32; 2 * 256];
        keys.send(&[0x90, 60, 110]);
        let mut left_over = false;
        for _ in 0..6 {
            st.fill(&mut out, 2);
            left_over |= st.pending_len as f64 - st.read >= 1.0;
        }
        assert!(left_over && !st.playing, "stopped, monitoring, frames rendered ahead");
        tx.send(Cmd::Recording(true)).unwrap();
        tx.send(Cmd::Play { from: 48_000, end: None, looped: None, countoff: None }).unwrap();
        keys.send(&[0x80, 60, 0]);
        st.fill(&mut out, 2);
        let mut got = Vec::new();
        hub.take_recorded(&mut got);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].msg, [0x80, 60, 0]);
        assert!((48_000..48_000 + 16).contains(&got[0].at), "at the record point, not before it: {}", got[0].at);
    }

    #[test]
    fn live_midi_is_heard_during_the_count_off_but_only_recorded_from_the_record_point() {
        // One bar of four 2400-frame beats: 9600 frames.
        let countoff = CountOff { bars: 1, beats_per_bar: 4, beat: 2400.0 };
        let start = |play: bool| {
            let (mut st, tx, hub, keys) = midi_state(true);
            tx.send(Cmd::Recording(true)).unwrap();
            tx.send(Cmd::Play { from: 48_000, end: None, looped: None, countoff: Some(countoff) }).unwrap();
            let mut out = vec![0.0f32; 2 * 256];
            st.fill(&mut out, 2);
            if play {
                keys.send(&[0x90, 60, 110]);
            }
            // The energy of the next 12 callbacks: the clicks, and the synth when a key is down.
            let mut energy = 0.0f32;
            for _ in 0..12 {
                st.fill(&mut out, 2);
                energy += out.iter().map(|x| x * x).sum::<f32>();
            }
            assert!(st.countoff.is_some() && !st.playing, "still counting off");
            (st, tx, hub, keys, energy)
        };
        let (.., clicks_only) = start(false);
        let (mut st, _tx, hub, keys, with_key) = start(true);
        assert!(clicks_only > 0.0, "the count-off clicks");
        assert!(with_key > clicks_only * 1.5 + 0.01, "the keyboard is heard through the instrument: {with_key} vs {clicks_only}");
        let mut out = vec![0.0f32; 2 * 256];
        for _ in 0..40 {
            if st.playing {
                break;
            }
            st.fill(&mut out, 2);
        }
        assert!(st.playing && st.countoff.is_none(), "rolling after the count-off");
        st.fill(&mut out, 2);
        keys.send(&[0x80, 60, 0]);
        st.fill(&mut out, 2);
        let mut got = Vec::new();
        hub.take_recorded(&mut got);
        assert_eq!(got.len(), 1, "only the note-off, played after the record point: {got:?}");
        assert_eq!(got[0].msg, [0x80, 60, 0]);
        assert!((48_000..48_000 + 4 * 256).contains(&got[0].at), "{}", got[0].at);
    }

    #[test]
    fn recording_hands_the_heard_midi_back_with_position_and_pass() {
        let (mut st, tx, hub, keys) = midi_state(true);
        tx.send(Cmd::Recording(true)).unwrap();
        tx.send(Cmd::Play { from: 48_000, end: None, looped: Some(Range::new(48_000, 48_000 + 1024)), countoff: None }).unwrap();
        let mut out = vec![0.0f32; 2 * 256];
        st.fill(&mut out, 2);
        keys.send(&[0x90, 60, 110]);
        st.fill(&mut out, 2);
        // Past the loop's end: the next pass.
        for _ in 0..4 {
            st.fill(&mut out, 2);
        }
        keys.send(&[0x80, 60, 0]);
        st.fill(&mut out, 2);
        let mut got = Vec::new();
        hub.take_recorded(&mut got);
        assert_eq!(got.len(), 2, "{got:?}");
        let (on, off) = (got[0], got[1]);
        assert_eq!((on.msg, on.pass, on.port as usize), ([0x90, 60, 110], 0, midi_in::TYPING_SLOT));
        assert!((48_000..=48_256).contains(&on.at), "{}", on.at);
        assert_eq!((off.msg, off.pass), ([0x80, 60, 0], 1));
        assert!((48_000..49_024).contains(&off.at), "{}", off.at);
        // Not recording: nothing comes back.
        tx.send(Cmd::Recording(false)).unwrap();
        keys.send(&[0x90, 61, 1]);
        st.fill(&mut out, 2);
        hub.take_recorded(&mut got);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn short_device_callbacks_render_short_mix_blocks() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, max_block_for(None), channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        let mut out = vec![0.0f32; 2 * 64];
        st.fill(&mut out, 2);
        assert_eq!(shared.position.load(Ordering::Relaxed), 64);
    }

    fn audio_output_state(format: soundcraft_model::ChannelFormat, channels: Vec<Vec<f32>>) -> AudioState {
        use soundcraft_model::{Clip, SourceAudio, SourceId, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        s.outputs[0].format = format;
        let t = s.add_track(TrackKind::Audio, format, None);
        let frames = channels[0].len();
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "output", SourceId(5), 0, 0, frames as i64));
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let st = AudioState::new(rx, shared, Arc::new(s), 48_000.0, max_block_for(None), channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        st
    }

    #[test]
    fn mono_device_averages_stereo_before_clipping() {
        use soundcraft_model::ChannelFormat;
        // Right only, left only, identical, opposite polarity, and levels that must be averaged
        // before clipping either side to the device's range.
        let left = [0.0, 0.8, 0.8, 0.8, 2.0, -2.0, 2.0];
        let right = [0.8, 0.0, 0.8, -0.8, -0.5, 0.5, 2.0];
        let expected = [0.4, 0.4, 0.8, 0.0, 0.75, -0.75, 1.0];
        for callback in [1, 37, 512] {
            let channels = [left, right].map(|pattern| (0..2048).map(|i| pattern[i % pattern.len()]).collect());
            let mut st = audio_output_state(ChannelFormat::Stereo, channels.into());
            let mut out = vec![0.0; callback];
            for block in 0..3 {
                st.fill(&mut out, 1);
                for (i, actual) in out.iter().enumerate() {
                    let at = block * callback + i;
                    assert!((actual - expected[at % expected.len()]).abs() < 1e-6, "callback {callback}, sample {at}: {actual}");
                }
            }
        }
    }

    #[test]
    fn mono_device_preserves_centered_mono_level_and_includes_surround_right_channels() {
        use soundcraft_model::ChannelFormat;
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let mut mono = audio_output_state(ChannelFormat::Mono, vec![vec![0.8; 512]]);
        let mut out = [0.0; 128];
        mono.fill(&mut out, 1);
        let centered = 0.8 * soundcraft_dsp::db_to_gain(-3.0);
        assert!(out.iter().all(|x| (*x - centered).abs() < 1e-6), "a centered mono track keeps its rendered level under the -3 dB pan law");

        // Independently exercise each SMPTE 5.1 channel: L, R, C, LFE, Ls, Rs.
        let channels = (0..6).map(|c| (0..512).map(|i| if i % 6 == c { 0.8 } else { 0.0 }).collect()).collect();
        let mut surround = audio_output_state(ChannelFormat::Surround51, channels);
        surround.fill(&mut out, 1);
        let expected = [0.4, 0.4, 0.8 * h, 0.0, 0.4 * h, 0.4 * h];
        for (i, actual) in out.iter().enumerate() {
            assert!((actual - expected[i % expected.len()]).abs() < 1e-6, "surround sample {i}: {actual}");
        }
    }

    #[test]
    fn mono_device_contains_overflow_from_opposite_surround_sides() {
        use soundcraft_model::ChannelFormat;
        let channels = [f32::MAX, -f32::MAX, 0.0, 0.0, f32::MAX, -f32::MAX].map(|v| vec![v; 128]);
        let mut st = audio_output_state(ChannelFormat::Surround51, channels.into());
        let mut out = [0.5; 128];
        st.fill(&mut out, 1);
        assert!(out.iter().all(|x| *x == 0.0), "overflow in the stereo fold-down must not send NaN to a mono device");
    }

    #[test]
    fn surround_main_maps_to_device_channels_or_folds_down() {
        use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
        let mut s = soundcraft_model::Session::default();
        s.outputs[0].format = ChannelFormat::Surround51;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Surround51, None);
        let chans: Vec<Vec<f32>> = (0..6).map(|c| vec![0.1 * (c + 1) as f32; 48_000]).collect();
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: chans })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 48_000));
        let s = Arc::new(s);
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::clone(&s), 48_000.0, max_block_for(None), channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        // An 8-channel device: channels in order, the rest silent.
        let mut out = vec![0.0f32; 8 * 256];
        st.fill(&mut out, 8);
        let f = &out[8 * 100..8 * 101];
        for c in 0..6 {
            assert!((f[c] - 0.1 * (c + 1) as f32).abs() < 1e-5, "{f:?}");
        }
        assert!(f[6] == 0.0 && f[7] == 0.0);
        // A stereo device: ITU fold-down.
        let mut out = vec![0.0f32; 2 * 256];
        st.fill(&mut out, 2);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let l = (0.1 + h * 0.3 + h * 0.5f32).min(1.0);
        assert!((out[200] - l).abs() < 1e-4, "{} vs {l}", out[200]);
        assert_eq!(shared.meters.lock().unwrap().main.peaks.len(), 6);
    }

    #[test]
    fn the_band_arrives_built_and_what_it_lets_go_of_leaves_through_the_return_channel() {
        use soundcraft_model::{ChannelFormat, Insert, Route, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("subtractive_synth"));
        s.track_mut(t).unwrap().mixer.input = Route::Musician(0);
        let lib = soundcraft_band::library::embedded_library();
        let update = || soundcraft_mix::prepare_band(&s, &lib, &|_| None).unwrap();
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s.clone()), 48_000.0, max_block_for(None), gtx);
        let u = update();
        tx.send(Cmd::BandRuntime(soundcraft_mix::new_band_runtime(48_000.0, &u))).unwrap();
        tx.send(Cmd::Band(u)).unwrap();
        tx.send(Cmd::Band(update())).unwrap();
        st.drain();
        assert!(st.mix.has_band());
        let g: Vec<Garbage> = grx.try_iter().collect();
        assert_eq!(g.iter().filter(|g| g.band_update.is_some()).count(), 1, "the replaced update goes back");
        // A second runtime is refused, and goes back too.
        tx.send(Cmd::BandRuntime(soundcraft_mix::new_band_runtime(48_000.0, &update()))).unwrap();
        st.drain();
        assert!(grx.try_iter().any(|g| g.band_runtime.is_some()));
        // A new sample rate builds a new engine: the old runtime goes back, never re-prepared here;
        // the UI thread sends one built for the new rate.
        let mut s44 = s.clone();
        s44.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
        tx.send(Cmd::Session(Arc::new(s44), Vec::new())).unwrap();
        st.drain();
        assert!(!st.mix.has_band());
        let g: Vec<Garbage> = grx.try_iter().collect();
        assert!(g.iter().any(|g| g.band_runtime.is_some()), "the old runtime goes back");
        assert!(g.iter().any(|g| g.band_update.is_some()), "so does the update it played");
        let u = update();
        tx.send(Cmd::BandRuntime(soundcraft_mix::new_band_runtime(44_100.0, &u))).unwrap();
        tx.send(Cmd::Band(u)).unwrap();
        st.drain();
        assert!(st.mix.has_band());
    }

    #[test]
    fn a_rate_change_sends_a_fresh_band_runtime() {
        use soundcraft_model::{ChannelFormat, Insert, Route, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("subtractive_synth"));
        s.track_mut(t).unwrap().mixer.input = Route::Musician(0);
        let mut side = Side::new(48_000.0, max_block_for(None), channel().1);
        side.band_lib = Some(soundcraft_band::library::embedded_library());
        side.band_installed = true;
        let _ = side.prepare(&s);
        assert!(side.band_installed, "same rate: the runtime stays");
        let mut s44 = s.clone();
        s44.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
        let _ = side.prepare(&s44);
        assert!(!side.band_installed, "a new rate: a fresh runtime goes with the next update");
        assert!(side.prepare_band(&s44).is_some());
    }

    #[test]
    fn instances_arrive_with_the_session_and_leave_through_the_return_channel() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let m = s.track_mut(t).unwrap().mixer.add_module(Insert::new("clap:test.fake"), None).unwrap();
        let s = Arc::new(s);
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(soundcraft_model::Session::default()), 48_000.0, max_block_for(None), gtx);
        let plugin = soundcraft_dsp::create("gain").unwrap();
        tx.send(Cmd::Session(
            Arc::clone(&s),
            vec![PreparedInstance { track: t, slot: SlotKey::Module(m), id: "clap:test.fake".into(), ch: 2, plugin }],
        ))
        .unwrap();
        let mut out = vec![0.0f32; 256];
        st.fill(&mut out, 2);
        assert!(grx.try_recv().is_err(), "nothing retired yet");
        // A session without the insert retires the instance to the return channel.
        tx.send(Cmd::Session(Arc::new(soundcraft_model::Session::default()), Vec::new())).unwrap();
        st.fill(&mut out, 2);
        assert_eq!(grx.try_recv().unwrap().plugins.len(), 1);
        // A sample-rate change rebuilds the engine without dropping plugins on this thread.
        tx.send(Cmd::Session(
            Arc::clone(&s),
            vec![PreparedInstance {
                track: t,
                slot: SlotKey::Module(m),
                id: "clap:test.fake".into(),
                ch: 2,
                plugin: soundcraft_dsp::create("gain").unwrap(),
            }],
        ))
        .unwrap();
        st.fill(&mut out, 2);
        let mut s44 = (*s).clone();
        s44.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
        tx.send(Cmd::Session(Arc::new(s44), Vec::new())).unwrap();
        st.fill(&mut out, 2);
        assert_eq!(grx.try_recv().unwrap().plugins.len(), 1);
    }

    #[test]
    fn plugin_states_are_read_without_the_audio_thread() {
        let mut side = Side::new(48_000.0, max_block_for(None), channel().1);
        let (a, b) = (TrackId(1), TrackId(2));
        let m = |n| SlotKey::Module(soundcraft_model::ModuleId(n));
        side.states.insert((b, m(1)), Box::new(|| Some(b"b0".to_vec())));
        side.states.insert((a, SlotKey::Instrument), Box::new(|| Some(b"synth".to_vec())));
        side.states.insert((a, m(3)), Box::new(|| Some(b"a3".to_vec())));
        side.states.insert((a, m(4)), Box::new(|| None));
        let want = vec![(a, m(3), b"a3".to_vec()), (a, SlotKey::Instrument, b"synth".to_vec()), (b, m(1), b"b0".to_vec())];
        assert_eq!(side.capture_states(), want);
        assert_eq!(side.capture_state((a, m(3))), Some(b"a3".to_vec()));
        assert_eq!(side.capture_state((a, m(4))), None);
        assert_eq!(side.capture_state((a, m(9))), None);
        side.forget((a, m(3)));
        assert_eq!(side.capture_states(), want[1..]);
    }

    struct Window;

    impl PluginEditor for Window {
        fn open(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn close(&mut self) {}
        fn is_open(&self) -> bool {
            false
        }
        fn idle(&mut self) -> Vec<(String, f32)> {
            Vec::new()
        }
    }

    /// An editor whose capture is a fixed 2×1 picture.
    struct Pictured;

    impl PluginEditor for Pictured {
        fn open(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn close(&mut self) {}
        fn is_open(&self) -> bool {
            true
        }
        fn idle(&mut self) -> Vec<(String, f32)> {
            Vec::new()
        }
        fn capture(&mut self) -> Option<(u32, u32, Vec<u8>)> {
            Some((2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]))
        }
    }

    #[test]
    fn editors_are_captured_without_the_audio_thread() {
        // No audio state exists here at all: a capture is the UI side's (and the editor's) alone.
        let mut side = Side::new(48_000.0, max_block_for(None), channel().1);
        let m = |n| SlotKey::Module(soundcraft_model::ModuleId(n));
        let (a, b) = ((TrackId(1), m(1)), (TrackId(1), m(2)));
        side.editors.insert(a, Box::new(Pictured));
        side.editors.insert(b, Box::new(Window));
        assert_eq!(side.capture_editor(a), Some((2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255])));
        assert_eq!(side.capture_editor(b), None, "an editor that cannot capture");
        assert_eq!(side.capture_editor((TrackId(2), m(1))), None, "no editor there");
    }

    #[test]
    fn a_refused_state_is_kept_until_the_editor_opens() {
        let mut side = Side::new(48_000.0, max_block_for(None), channel().1);
        let k = (TrackId(1), SlotKey::Module(soundcraft_model::ModuleId(1)));
        side.states.insert(k, Box::new(|| Some(b"defaults".to_vec())));
        side.editors.insert(k, Box::new(Window));
        side.refused.insert(k);
        assert!(side.capture_states().is_empty());
        assert_eq!(side.capture_state(k), None, "a refused state is not read back either");
        side.open_editor(k).unwrap();
        assert_eq!(side.capture_states(), vec![(k.0, k.1, b"defaults".to_vec())]);
    }

    /// A state handle whose state is whatever was last loaded (refusing `b"bad"`).
    struct Stateful(Arc<std::sync::Mutex<Vec<u8>>>);

    impl PluginStateReader for Stateful {
        fn save_state(&mut self) -> Option<Vec<u8>> {
            Some(self.0.lock().unwrap().clone())
        }
        fn load_state(&mut self, data: &[u8]) -> bool {
            if data == b"bad" {
                return false;
            }
            *self.0.lock().unwrap() = data.to_vec();
            true
        }
    }

    /// An editor that is open until closed (and counts closes).
    struct OpenEditor(Arc<AtomicU64>);

    impl PluginEditor for OpenEditor {
        fn open(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn close(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn is_open(&self) -> bool {
            true
        }
        fn idle(&mut self) -> Vec<(String, f32)> {
            Vec::new()
        }
    }

    #[test]
    fn a_preset_loads_into_the_running_instance_keeping_its_editor() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut side = Side::new(48_000.0, max_block_for(None), channel().1);
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let m = s.track_mut(t).unwrap().mixer.add_module(Insert::new("vst3:0123456789ABCDEF0123456789ABCDEF"), None).unwrap();
        let k = (t, SlotKey::Module(m));
        side.prepare(&s);
        // Stand-ins for the running instance's state and editor.
        let state = Arc::new(std::sync::Mutex::new(b"user".to_vec()));
        let closes = Arc::new(AtomicU64::new(0));
        side.states.insert(k, Box::new(Stateful(Arc::clone(&state))));
        side.editors.insert(k, Box::new(OpenEditor(Arc::clone(&closes))));
        let ins = s.track_mut(t).unwrap().mixer.module_mut(m).unwrap();
        ins.set_state_bytes(b"preset");
        ins.state_load = 9;
        assert!(side.prepare(&s).is_empty(), "no new instance");
        assert_eq!(*state.lock().unwrap(), b"preset");
        assert!(side.editors.get(&k).is_some_and(|e| e.is_open()) && closes.load(Ordering::Relaxed) == 0, "the editor stays open");
        assert_eq!(side.shadow.get(&k).map(|e| e.2), Some(9));
        assert!(side.refused_presets.is_empty());
        // Refused in place: made anew (the editor goes with the old instance).
        let ins = s.track_mut(t).unwrap().mixer.module_mut(m).unwrap();
        ins.set_state_bytes(b"bad");
        ins.state_load = 10;
        side.prepare(&s);
        assert_eq!(closes.load(Ordering::Relaxed), 1);
        assert_eq!(side.shadow.get(&k).map(|e| e.2), Some(10));
        assert!(!side.states.contains_key(&k), "the old handles are gone");
    }

    #[test]
    fn side_creates_each_missing_instance_once() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut side = Side::new(48_000.0, max_block_for(None), channel().1);
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let missing = s.track_mut(t).unwrap().mixer.add_module(Insert::new("clap:does.not.exist"), None).unwrap();
        s.track_mut(t).unwrap().mixer.add_module(Insert::new("gain"), None).unwrap();
        assert!(side.prepare(&s).is_empty(), "the plugin is not installed; built-ins stay with the engine");
        assert_eq!(side.shadow.get(&(t, SlotKey::Module(missing))), Some(&("clap:does.not.exist".to_string(), 1, 0, false)));
        assert!(side.prepare(&s).is_empty());
        assert_eq!(side.shadow.len(), 1, "failures are remembered, not retried every update");
        // After a plugin scan, the missing ones are tried again (once more).
        assert_eq!(side.retry_missing(), 1);
        assert!(side.shadow.is_empty());
        assert!(side.prepare(&s).is_empty());
        assert_eq!(side.shadow.get(&(t, SlotKey::Module(missing))), Some(&("clap:does.not.exist".to_string(), 1, 0, false)), "tried again");
        assert_eq!(side.retry_missing(), 1);
        side.prepare(&s);
        // A preset loaded into the module: its instance is made again (from the new state).
        s.track_mut(t).unwrap().mixer.module_mut(missing).unwrap().state_load = 7;
        side.prepare(&s);
        assert_eq!(side.shadow.get(&(t, SlotKey::Module(missing))), Some(&("clap:does.not.exist".to_string(), 1, 7, false)), "made again");
        s.track_mut(t).unwrap().mixer.remove_module(missing);
        side.prepare(&s);
        assert!(side.shadow.is_empty());
    }

    #[test]
    fn replaced_sessions_are_freed_on_the_ui_thread() {
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, shared, Arc::new(soundcraft_model::Session::default()), 48_000.0, max_block_for(None), gtx);
        let mut side = Side::new(48_000.0, max_block_for(None), grx);
        side.return_old_sessions(&tx);
        let mut out = vec![0.0f32; 256];
        let mut watched = Vec::new();
        for _ in 0..3 * OLD_SESSIONS {
            let s = Arc::new(soundcraft_model::Session::default());
            watched.push(Arc::downgrade(&s));
            side.collect_garbage();
            tx.send(Cmd::Session(s, Vec::new())).unwrap();
            st.fill(&mut out, 2);
            let replaced = watched.len().checked_sub(2).and_then(|i| watched.get(i));
            assert!(replaced.is_none_or(|w| w.strong_count() == 1), "freed on the audio thread");
        }
        side.collect_garbage();
        assert_eq!(watched.iter().filter(|w| w.strong_count() > 0).count(), 1, "only the live snapshot is left");
    }

    #[test]
    fn looping_wraps() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, max_block_for(None), channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 1000)), countoff: None }).unwrap();
        let mut out = vec![0.0f32; 20_000];
        st.fill(&mut out, 2);
        let p = shared.position.load(Ordering::Relaxed);
        assert!(p <= 1000, "position {p} should stay inside the loop");
        // The take clock keeps counting across the wraps: every frame played.
        let probe = TransportProbe { shared: Arc::clone(&shared) };
        let r = probe.read();
        assert!(r.playing);
        assert_eq!(r.rolled, 10_000, "10 000 stereo frames rolled");
        assert!(r.at_ns > 0 || cfg!(target_arch = "wasm32"));
        assert!(shared.playing.load(Ordering::Relaxed));
    }

    #[test]
    fn a_module_action_reaches_the_playing_instance() {
        // LevelUp's Deliver Reset: its integrated reading starts again at the next block.
        use soundcraft_model::{ChannelFormat, Clip, Insert, SourceAudio, SourceId, TrackKind};
        let mut s = soundcraft_model::Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let tone: Vec<f32> = (0..96_000).map(|i| 0.3 * (i as f32 * 0.13).sin()).collect();
        s.pool.insert(
            SourceId(5),
            Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![tone.clone(), tone] })),
        );
        let id = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 96_000));
        let m = tr.mixer.add_module(Insert::new("fs_levelup"), None).unwrap();
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, max_block_for(None), channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        let mut out = vec![0.0f32; 2 * 512];
        let integrated = |shared: &Shared| shared.meters.lock().unwrap().tracks[&t].modules.iter().find(|x| x.module == m).unwrap().readouts[3];
        for _ in 0..60 {
            st.fill(&mut out, 2);
        }
        assert!(integrated(&shared) > -69.0, "{}", integrated(&shared));
        tx.send(Cmd::Action(t, m, "reset_delivery")).unwrap();
        st.fill(&mut out, 2);
        assert_eq!(integrated(&shared), -100.0);
    }

    #[test]
    fn locate_while_playing_drops_the_old_position_audio() {
        use soundcraft_model::{ChannelFormat, Clip, Insert, SourceAudio, SourceId, TrackKind};
        for (plugin, params) in [("compressor", &[("lookahead", 5.0), ("ratio", 1.0)][..]), ("room_reverb", &[][..])] {
            let mut s = soundcraft_model::Session::default();
            s.edit.click = false;
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            s.pool.insert(
                SourceId(5),
                Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800]] })),
            );
            let id = s.new_clip_id();
            let tr = s.track_mut(t).unwrap();
            tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 4800));
            let mut ins = Insert::new(plugin);
            for (k, v) in params {
                ins.params.insert((*k).into(), *v);
            }
            tr.mixer.add_module(ins, None).unwrap();
            let shared = Arc::new(Shared::new());
            let (tx, rx) = channel();
            let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, max_block_for(None), channel().0);
            tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
            let mut out = vec![0.0f32; 2 * 512];
            let mut before = 0.0f32;
            for _ in 0..9 {
                st.fill(&mut out, 2);
                before = out.iter().fold(before, |m, x| m.max(x.abs()));
            }
            assert!(before > 0.1, "{plugin}: the clip plays before the locate");
            tx.send(Cmd::Locate(1_000_000)).unwrap();
            for _ in 0..8 {
                st.fill(&mut out, 2);
                assert!(out.iter().all(|x| x.abs() < 1e-6), "{plugin}: audio from before the locate");
            }
        }
    }

    #[test]
    fn armed_tracks_are_heard_and_metered_after_playback_stops() {
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, max_block_for(None), channel().0);
        let feed = feed::InputFeed::fixed(1, max_block_for(None));
        attach(&tx, &feed, max_block_for(None));
        tx.send(Cmd::Play { from: 0, end: Some(1000), looped: None, countoff: None }).unwrap();
        let mut out = vec![0.0f32; 4096];
        st.fill(&mut out, 2);
        assert!(!shared.playing.load(Ordering::Relaxed));
        feed.push(&[0.5; 2048]);
        st.fill(&mut out, 2);
        assert!(out.iter().any(|x| x.abs() > 0.1), "the input is heard");
        assert!(shared.meters.lock().unwrap().tracks.get(&t).is_some_and(|m| m.peak[0] > 0.1), "and metered");
    }

    /// Send `feed` to the audio state as its input, with a mix input buffer of `max_block`.
    fn attach(tx: &Sender<Cmd>, feed: &Arc<feed::InputFeed>, max_block: usize) {
        tx.send(Cmd::Input(Arc::clone(feed), vec![vec![0.0; max_block]; feed.channels()])).unwrap();
    }

    /// A stereo clip of `frames` (a tone on each side, well inside ±1), click off, plus `inserts`
    /// on its track.
    fn clip_session(frames: usize, inserts: &[&str]) -> Session {
        use soundcraft_model::{ChannelFormat, Clip, Insert, SourceAudio, SourceId, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let left: Vec<f32> = (0..frames).map(|i| 0.4 * (i as f32 * 0.031).sin()).collect();
        let right: Vec<f32> = (0..frames).map(|i| 0.3 * (i as f32 * 0.017).cos()).collect();
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![left, right] })));
        let id = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, frames as i64));
        for i in inserts {
            tr.mixer.add_module(Insert::new(*i), None).unwrap();
        }
        s
    }

    /// An `AudioState` playing `s` from 0 on a device at `device_rate`.
    fn playing(s: &Session, device_rate: f64, max_block: usize) -> (AudioState, Arc<Shared>, Sender<Cmd>) {
        playing_from(s, device_rate, max_block, Cmd::Play { from: 0, end: None, looped: None, countoff: None })
    }

    /// An `AudioState` on a device at `device_rate`, sent `play`.
    fn playing_from(s: &Session, device_rate: f64, max_block: usize, play: Cmd) -> (AudioState, Arc<Shared>, Sender<Cmd>) {
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s.clone()), device_rate, max_block, channel().0);
        // Synced first, so the Play's reset seats the plugins as an offline render does.
        tx.send(Cmd::Session(Arc::new(s.clone()), Vec::new())).unwrap();
        tx.send(play).unwrap();
        (st, shared, tx)
    }

    /// Stereo callbacks of these sizes, deinterleaved.
    fn callbacks(st: &mut AudioState, sizes: &[usize]) -> [Vec<f32>; 2] {
        let mut got = [Vec::new(), Vec::new()];
        for &n in sizes {
            let mut out = vec![0.0f32; 2 * n];
            st.fill(&mut out, 2);
            for f in out.chunks(2) {
                got[0].push(f[0]);
                got[1].push(f[1]);
            }
        }
        got
    }

    fn assert_matches_offline(got: &[Vec<f32>; 2], s: &Session) {
        let len = got[0].len();
        let want = soundcraft_mix::render_range(s, Range::new(0, len as i64), 512);
        for c in 0..2 {
            assert!(got[c].iter().all(|x| x.is_finite()));
            for (i, (a, b)) in got[c].iter().zip(&want[c]).enumerate() {
                assert_eq!(a.to_bits(), b.to_bits(), "channel {c}, sample {i}: {a} vs {b}");
            }
        }
        assert!(want[0].iter().any(|x| x.abs() > 0.1), "the clip is heard");
    }

    #[test]
    fn max_block_for_bounds() {
        let got: Vec<usize> = [None, Some(96), Some(128), Some(1024), Some(8192), Some(u32::MAX)].into_iter().map(max_block_for).collect();
        assert_eq!(got, [2048, 256, 256, 2048, 8192, 8192]);
    }

    #[test]
    fn a_128_frame_callback_renders_one_128_frame_block() {
        let s = clip_session(4800, &[]);
        let (mut st, shared, _tx) = playing(&s, 48_000.0, max_block_for(Some(128)));
        let mut got = [Vec::new(), Vec::new()];
        for k in 1..=10 {
            let block = callbacks(&mut st, &[128]);
            assert_eq!(shared.position.load(Ordering::Relaxed), 128 * k, "one 128-frame block per callback");
            assert_eq!((st.pending_len, st.read), (128, 128.0), "nothing rendered ahead");
            got[0].extend(&block[0]);
            got[1].extend(&block[1]);
        }
        assert_eq!(shared.position.load(Ordering::Relaxed), 1280);
        assert!(shared.direct.load(Ordering::Relaxed));
        assert_matches_offline(&got, &s);
    }

    #[test]
    fn odd_and_oversized_callbacks_render_in_chunks_without_allocating() {
        let s = clip_session(20_000, &[]);
        let (mut st, shared, tx) = playing(&s, 48_000.0, 256);
        attach(&tx, &feed::InputFeed::fixed(2, 256), 256);
        let sizes = [441, 1056, 5000];
        let mut got = callbacks(&mut st, &sizes);
        assert_eq!(shared.position.load(Ordering::Relaxed), 441 + 1056 + 5000);
        let buffers = |st: &AudioState| {
            let p: Vec<(usize, *const f32)> = st.pending.iter().chain(&st.mix.input).map(|c| (c.capacity(), c.as_ptr())).collect();
            (st.pending.capacity(), st.mix.input.capacity(), p)
        };
        let before = buffers(&st);
        assert_eq!(st.mix.input.len(), 2, "the input is in use");
        assert!(st.pending.iter().chain(&st.mix.input).all(|c| c.len() == 256), "sized to max_block");
        let more = callbacks(&mut st, &[441, 1056, 5000, 7]);
        assert_eq!(buffers(&st), before, "no buffer grew or moved");
        assert_eq!(shared.position.load(Ordering::Relaxed), 2 * (441 + 1056 + 5000) + 7);
        got[0].extend(&more[0]);
        got[1].extend(&more[1]);
        assert_matches_offline(&got, &s);
    }

    #[test]
    fn rate_mismatch_or_speed_uses_the_resampler() {
        let s = clip_session(4800, &[]);
        let (mut st, shared, _tx) = playing(&s, 44_100.0, max_block_for(Some(441)));
        callbacks(&mut st, &[441]);
        let p = shared.position.load(Ordering::Relaxed);
        assert!((479..=481).contains(&p), "position {p}");
        assert!(!shared.direct.load(Ordering::Relaxed));

        let (mut st, shared, _tx) = playing(&s, 48_000.0, 256);
        callbacks(&mut st, &[128]);
        assert!(shared.direct.load(Ordering::Relaxed));
        shared.speed.store(1500, Ordering::Relaxed);
        callbacks(&mut st, &[128]);
        assert!(!shared.direct.load(Ordering::Relaxed), "speed ≠ 1 resamples");
        assert_eq!(shared.position.load(Ordering::Relaxed), 128 + 192, "the switch starts from what was played");
    }

    /// The remote mix stream's two ends, with a plugin delaying the mix (a compressor's 1 ms
    /// lookahead): the host's tap tags each block by the timeline it holds (the click at 1000
    /// comes out tagged 1000, not 1048), the performer's marks by when it is heard with the mix.
    #[test]
    fn the_stream_ends_take_the_mixs_plugin_delay_off() {
        use soundcraft_model::{ChannelFormat, Clip, Insert, SourceAudio, SourceId, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let mut c = vec![0.0f32; 4800];
        c[1000] = 0.8;
        s.pool
            .insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![c.clone(), c] })));
        let id = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 4800));
        let mut comp = Insert::new("compressor");
        comp.params.insert("lookahead".into(), 1.0);
        comp.params.insert("ratio".into(), 1.0);
        tr.mixer.add_module(comp, None).unwrap();
        let (mut st, shared, _tx) = playing(&s, 48_000.0, 256);
        shared.tap.set_on(true);
        shared.stream_in.set_on(true);
        shared.stream_in.push(&[0.0; 4096]);
        callbacks(&mut st, &[256; 10]);
        let pdc = st.mix.latency as i64;
        assert_eq!(pdc, 48, "the lookahead delays the mix");
        let mut blocks = Vec::new();
        shared.tap.read(&mut blocks);
        let mut click = None;
        for b in &blocks {
            if let Some(i) = b.samples.iter().step_by(2).position(|x| x.abs() > 0.4) {
                click = Some((b.position + i as i64, b.rolled + i as i64));
            }
        }
        assert_eq!(click, Some((1000, 1000)), "the click is tagged where it sits on the timeline");
        let m = shared.stream_in.mark();
        assert_eq!(m.rolled, 256 * 9 - pdc, "the last block's stream is heard with the mix of 48 frames before");
    }

    #[test]
    fn a_play_resets_the_take_clock_on_the_audio_thread_only() {
        let s = clip_session(4800, &[]);
        let (mut st, shared, tx) = playing(&s, 48_000.0, 256);
        callbacks(&mut st, &[256; 4]);
        let probe = TransportProbe { shared: Arc::clone(&shared) };
        assert_eq!(probe.read().rolled, 1024);
        // A second Play while rolling: the UI thread leaves the clock alone; the probe reads none
        // until the audio thread takes the Play.
        shared.play_pending.store(true, Ordering::Release);
        assert!(!probe.read().playing);
        assert_eq!(shared.rolled.load(Ordering::Relaxed), 1024);
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        callbacks(&mut st, &[256]);
        let r = probe.read();
        assert!(r.playing);
        assert_eq!(r.rolled, 256, "counted from the new Play");
    }

    #[test]
    fn builtin_plugins_render_any_block_size() {
        let s = clip_session(4800, &["gain", "compressor"]);
        let (mut st, shared, _tx) = playing(&s, 48_000.0, 256);
        let got = callbacks(&mut st, &[96, 128, 17, 256, 96, 128, 17, 256]);
        assert_eq!(shared.position.load(Ordering::Relaxed), 2 * (96 + 128 + 17 + 256));
        assert_matches_offline(&got, &s);
    }

    #[test]
    fn late_and_gapped_callbacks_count_as_underruns() {
        use std::time::Duration;
        // A real period of 128 frames at 48 kHz lasts 2.667 ms; a gap is 3/4 of it (2 ms) late.
        assert!(!late(Duration::from_micros(2600), 128, 48_000.0));
        assert!(late(Duration::from_micros(2700), 128, 48_000.0));
        assert!(!late(Duration::ZERO, 0, 0.0), "a hostile rate is not a panic");
        let at = Duration::from_millis(10);
        assert!(!gap(at, at, 128, 48_000.0), "on time");
        assert!(!gap(at, at + Duration::from_micros(1900), 128, 48_000.0), "jitter");
        assert!(gap(at, at + Duration::from_micros(2100), 128, 48_000.0), "silence played in between");
        assert!(!gap(at, at - Duration::from_millis(1), 128, 48_000.0), "audio that plays early");
    }

    /// Feeds `Timing` callbacks of `(playback ms, frames, fill ms)` at 48 kHz.
    fn timed(t: &mut Timing, shared: &Shared, calls: &[(u64, usize, u64)]) {
        use std::time::Duration;
        for &(ms, frames, fill) in calls {
            let at = cpal::StreamInstant::new(1, u32::try_from(ms * 1_000_000).unwrap());
            t.callback(shared, at, Duration::from_millis(fill), frames, 48_000.0);
        }
    }

    #[test]
    fn varying_callback_sizes_are_not_underruns_but_a_stall_is() {
        // WASAPI shared mode: 480, 240, 480 frames every 10 ms. The 240-frame callbacks take
        // longer than their own 5 ms, but not longer than the real 10 ms period.
        let shared = Shared::new();
        let mut t = Timing::new(cpal::BufferSize::Default);
        timed(&mut t, &shared, &[(0, 480, 2), (10, 240, 6), (20, 480, 2), (30, 480, 2), (40, 240, 6), (50, 480, 2)]);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
        assert!(shared.load_permille.load(Ordering::Relaxed) <= 600, "load is a share of the real period");
        // A 30 ms stall: the next audio plays at 90 ms instead of 60 ms.
        timed(&mut t, &shared, &[(90, 480, 2), (100, 480, 2), (110, 480, 2)]);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_late_callback_after_a_gap_counts_one_underrun_and_peaks_the_load() {
        let shared = Shared::new();
        let mut t = Timing::new(cpal::BufferSize::Fixed(480));
        timed(&mut t, &shared, &[(0, 480, 1), (10, 480, 1)]);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
        timed(&mut t, &shared, &[(40, 480, 12)]);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 1, "late and gapped: one underrun");
        assert!(shared.load_permille.load(Ordering::Relaxed) > 1000);
    }

    /// The offline render of `s` over `range` (stereo).
    fn offline(s: &Session, range: std::ops::Range<i64>) -> Vec<Vec<f32>> {
        soundcraft_mix::render_range(s, Range::new(range.start, range.end), 512)
    }

    fn assert_bits(got: &[f32], want: &[f32], what: &str) {
        assert_eq!(got.len(), want.len(), "{what}: length");
        for (i, (a, b)) in got.iter().zip(want).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "{what}, sample {i}: {a} vs {b}");
        }
    }

    #[test]
    fn switching_to_the_direct_path_plays_out_what_the_resampler_rendered() {
        let s = clip_session(4800, &[]);
        let (mut st, shared, _tx) = playing(&s, 48_000.0, 256);
        // At ×1.25 a 10-frame callback renders 13 frames and reads up to 12.5.
        shared.speed.store(1250, Ordering::Relaxed);
        callbacks(&mut st, &[10]);
        assert_eq!(shared.position.load(Ordering::Relaxed), 13);
        assert!(!shared.direct.load(Ordering::Relaxed));
        shared.speed.store(1000, Ordering::Relaxed);
        let got = callbacks(&mut st, &[100]);
        assert!(shared.direct.load(Ordering::Relaxed));
        assert_eq!(shared.position.load(Ordering::Relaxed), 112, "frame 12 was left over; 13.. are new");
        let want = offline(&s, 0..112);
        for c in 0..2 {
            assert_bits(&got[c], &want[c][12..112], "after the switch");
        }
    }

    #[test]
    fn switching_to_the_resampler_starts_where_the_direct_path_stopped() {
        let s = clip_session(4800, &[]);
        let (mut st, shared, _tx) = playing(&s, 48_000.0, 256);
        callbacks(&mut st, &[100]);
        shared.speed.store(2000, Ordering::Relaxed);
        let got = callbacks(&mut st, &[50]);
        assert!(!shared.direct.load(Ordering::Relaxed));
        assert_eq!(shared.position.load(Ordering::Relaxed), 200);
        let want = offline(&s, 0..200);
        for c in 0..2 {
            let every_other: Vec<f32> = (0..50).map(|i| want[c][100 + 2 * i]).collect();
            assert_bits(&got[c], &every_other, "at ×2 from frame 100");
        }
    }

    #[test]
    fn the_direct_path_loops_and_ends_on_the_exact_frame() {
        let s = clip_session(4800, &[]);
        let looped = Cmd::Play { from: 1000, end: None, looped: Some(Range::new(1000, 1700)), countoff: None };
        let (mut st, shared, _tx) = playing_from(&s, 48_000.0, 256, looped);
        let got = callbacks(&mut st, &[441, 333, 1056, 17, 441]);
        assert_eq!(shared.position.load(Ordering::Relaxed), 1000 + 2288 % 700);
        let lap = offline(&s, 1000..1700);
        for c in 0..2 {
            let want: Vec<f32> = lap[c].iter().copied().cycle().take(2288).collect();
            assert_bits(&got[c], &want, "the loop, lap after lap");
        }

        let (mut st, shared, _tx) = playing_from(&s, 48_000.0, 256, Cmd::Play { from: 0, end: Some(300), looped: None, countoff: None });
        let got = callbacks(&mut st, &[441]);
        assert_eq!(shared.position.load(Ordering::Relaxed), 300);
        assert!(!shared.playing.load(Ordering::Relaxed));
        let want = offline(&s, 0..300);
        for c in 0..2 {
            assert_bits(&got[c][..300], &want[c], "up to the end");
            assert!(got[c][300..].iter().all(|x| x.to_bits() == 0), "silent after the end");
        }
    }

    #[test]
    fn monitored_input_arrives_intact_at_any_block_size() {
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        let (mut st, _shared, tx) = playing_from(&s, 48_000.0, 256, Cmd::Stop);
        let feed = feed::InputFeed::fixed(1, 256);
        attach(&tx, &feed, 256);
        let input: Vec<f32> = (0..4096).map(|i| 0.2 + 0.5 * (i as f32 * 0.05).sin()).collect();
        let sizes = [96, 128, 17, 256, 441];
        let mut got: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
        let mut at = 0;
        for n in sizes {
            // One clock: each callback's input arrives just before it.
            feed.push(&input[at..at + n]);
            at += n;
            let block = callbacks(&mut st, &[n]);
            got[0].extend(&block[0]);
            got[1].extend(&block[1]);
        }
        let frames: usize = sizes.iter().sum();
        assert_eq!(got[0].len(), frames);
        // A centred mono track: the same gain on both sides, every input sample in order.
        let g = got[0][0] / input[0];
        assert!(g > 0.5 && g <= 1.0, "gain {g}");
        for (c, side) in got.iter().enumerate() {
            for (i, (o, x)) in side.iter().zip(&input).enumerate() {
                assert!((o - g * x).abs() < 1e-5, "channel {c}, frame {i}: {o} vs {}", g * x);
            }
        }
    }

    #[test]
    fn duplex_feeds_cushion_where_callback_order_is_not_guaranteed() {
        assert_eq!(duplex_feed(true, 2, 48_000, 48_000, 256).describe(), "fixed", "ASIO: input then output, always");
        assert_eq!(duplex_feed(false, 2, 48_000, 48_000, 256).describe(), "cushioned", "WASAPI: either order");
        assert_eq!(duplex_feed(true, 2, 44_100, 48_000, 256).describe(), "drift", "another rate converts");
        // The cushion in use: a block queued ahead, so an output callback that runs first plays on.
        let feed = duplex_feed(false, 1, 48_000, 48_000, 256);
        let mut out = vec![vec![0.0f32; 128]];
        feed.push(&[0.5; 128]);
        feed.pop_into(&mut out, 128);
        assert_eq!(feed.extra_latency(), 128);
        feed.pop_into(&mut out, 128);
        assert!(out[0].iter().all(|x| *x == 0.5), "the output ran first: the queued block plays");
    }

    #[test]
    fn asio_never_opens_a_second_input_stream() {
        assert_eq!(split_input(true), Err("The ASIO input could not open with the output; reopen the device".to_string()));
        assert_eq!(split_input(false), Ok(()));
    }

    #[test]
    fn duplex_rule() {
        let cfg = |input: Option<&str>, output: Option<&str>| AudioConfig {
            input: input.map(Into::into),
            output: output.map(Into::into),
            ..AudioConfig::default()
        };
        assert!(duplex("ASIO", &cfg(None, None)), "ASIO is one driver, always duplex");
        assert!(duplex("asio", &cfg(Some("In"), Some("Out"))));
        assert!(duplex("WASAPI", &cfg(Some("Scarlett"), Some("Scarlett"))));
        assert!(!duplex("WASAPI", &cfg(Some("Mic"), Some("Speakers"))));
        assert!(!duplex("WASAPI", &cfg(None, None)), "two defaults may be two devices");
        assert!(!duplex("WASAPI", &cfg(Some("Scarlett"), None)));
    }

    /// An armed mono track (input heard while stopped) plus the stereo clip, on a device at
    /// `device_rate`, with a fixed feed of silence attached.
    fn monitored_clip(device_rate: f64, play: Cmd) -> (AudioState, Arc<Shared>, Sender<Cmd>, Arc<feed::InputFeed>, Session) {
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut s = clip_session(48_000, &[]);
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        let (st, shared, tx) = playing_from(&s, device_rate, 256, play);
        let feed = feed::InputFeed::fixed(1, 256);
        attach(&tx, &feed, 256);
        (st, shared, tx, feed, s)
    }

    #[test]
    fn stop_drops_what_was_rendered_ahead() {
        // The resampler renders ahead: at 44.1 kHz a 100-frame callback renders 109 frames.
        let (mut st, _shared, tx, feed, _s) = monitored_clip(44_100.0, Cmd::Play { from: 0, end: None, looped: None, countoff: None });
        feed.push(&[0.0; 100]);
        let played = callbacks(&mut st, &[100]);
        assert!(played[0].iter().any(|x| x.abs() > 0.05), "the clip plays");
        assert!(st.pending_len as f64 > st.read, "frames are left over");
        tx.send(Cmd::Stop).unwrap();
        feed.push(&[0.0; 100]);
        let after = callbacks(&mut st, &[100]);
        for side in &after {
            assert!(side.iter().all(|x| x.abs() < 1e-6), "playback heard after Stop: {:?}", &side[..12]);
        }
    }

    #[test]
    fn the_resampler_interpolates_into_the_next_block() {
        // At ×1.25 a 10-frame callback renders 13 frames and reads 0..11.25; the next starts at
        // 12.5, between that block's last frame and the next block's first.
        let s = clip_session(4800, &[]);
        let (mut st, shared, _tx) = playing(&s, 48_000.0, 256);
        shared.speed.store(1250, Ordering::Relaxed);
        let got = callbacks(&mut st, &[10; 40]);
        let want = offline(&s, 0..600);
        for c in 0..2 {
            for (k, g) in got[c].iter().enumerate() {
                let at = k as f64 * 1.25;
                let i = at.floor() as usize;
                let fr = (at - i as f64) as f32;
                let (a, b) = (want[c][i], want[c][i + 1]);
                let w = a + (b - a) * fr;
                assert!((g - w).abs() < 1e-6, "channel {c}, frame {k} (read {at}): {g} vs {w}");
            }
        }
    }

    #[test]
    fn monitored_input_carries_on_when_play_ends_mid_callback() {
        let (mut st, shared, _tx, feed, _s) = monitored_clip(48_000.0, Cmd::Play { from: 0, end: Some(300), looped: None, countoff: None });
        let input: Vec<f32> = (0..441).map(|i| 0.3 + 0.2 * (i as f32 * 0.07).sin()).collect();
        feed.push(&input);
        let got = callbacks(&mut st, &[441]);
        assert!(!shared.playing.load(Ordering::Relaxed));
        assert_eq!(shared.position.load(Ordering::Relaxed), 300);
        // After the end only the monitored input sounds: every input frame, at the track's gain.
        let g = got[0][300] / input[300];
        assert!(g > 0.5 && g <= 1.0, "gain {g}: the input is heard after the end");
        for (i, x) in input.iter().enumerate().skip(300) {
            assert!((got[0][i] - g * x).abs() < 1e-5, "frame {i}: {} vs {}", got[0][i], g * x);
        }
        assert_eq!(feed.queued(), 0, "the callback took its input");
    }

    #[test]
    fn monitored_input_has_no_gaps_under_varispeed() {
        // Speed 0.999 with 441-frame callbacks and 256-frame blocks: the resampler pops one or
        // two blocks a callback. One clock, so each callback's input arrives just before it.
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        let (mut st, shared, tx) = playing_from(&s, 48_000.0, 256, Cmd::Stop);
        let feed = feed::InputFeed::fixed(1, 256);
        attach(&tx, &feed, 256);
        shared.speed.store(999, Ordering::Relaxed);
        let mut at = 0usize;
        let mut got = Vec::new();
        for _ in 0..200 {
            let input: Vec<f32> = (at..at + 441).map(|i| 0.3 + 0.2 * (i as f32 * 0.01).sin()).collect();
            at += 441;
            feed.push(&input);
            got.extend(callbacks(&mut st, &[441])[0].iter().copied());
        }
        // The input never drops below 0.1: a gap is (interpolated) silence. The first callbacks
        // may pop ahead of the input until the slower reading has queued some.
        let gaps = got.iter().skip(10 * 441).filter(|x| x.abs() < 0.05).count();
        assert_eq!(gaps, 0, "monitored input has {gaps} silent frames under varispeed");
    }

    /// A stereo DC clip (0.25 left, 0.5 right) on a track routed to `output`, in a session
    /// `edit` set up, on a `device`-channel device at `rate`: one 128-frame callback,
    /// deinterleaved per device channel.
    fn device_callback(device: usize, rate: f64, output: soundcraft_model::Route, edit: impl FnOnce(&mut Session)) -> Vec<Vec<f32>> {
        use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        edit(&mut s);
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let channels = vec![vec![0.25; 4800], vec![0.5; 4800]];
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels })));
        let id = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 4800));
        tr.mixer.output = output;
        let (st, _shared, _tx) = playing(&s, rate, 256);
        let mut st = st.with_device_channels(device);
        let mut out = vec![0.0f32; device * 128];
        st.fill(&mut out, device);
        (0..device).map(|c| out.iter().skip(c).step_by(device).copied().collect()).collect()
    }

    fn sounds(ch: &[f32]) -> bool {
        ch.iter().skip(16).all(|x| x.abs() > 0.1)
    }

    fn mon_out(s: &mut Session) {
        use soundcraft_model::{ChannelFormat, OutputPath};
        s.outputs.push(OutputPath { name: "Mon 5-6".into(), first_channel: 4, format: ChannelFormat::Stereo });
    }

    #[test]
    fn main_follows_its_first_channel() {
        for rate in [48_000.0, 44_100.0] {
            let got = device_callback(8, rate, soundcraft_model::Route::Main, |s| s.outputs[0].first_channel = 2);
            for (c, ch) in got.iter().enumerate() {
                match c {
                    2 | 3 => assert!(sounds(ch), "{rate} Hz: main on device channel {c}"),
                    _ => assert!(ch.iter().all(|x| *x == 0.0), "{rate} Hz: device channel {c} is silent"),
                }
            }
            assert!(got[3][64] > got[2][64], "left and right in order");
        }
    }

    #[test]
    fn a_track_routed_to_a_hardware_output_plays_on_its_channels() {
        use soundcraft_model::Route;
        for rate in [48_000.0, 44_100.0] {
            let got = device_callback(32, rate, Route::Hardware("Mon 5-6".into()), mon_out);
            for (c, ch) in got.iter().enumerate() {
                match c {
                    4 | 5 => assert!(sounds(ch), "{rate} Hz: Mon 5-6 on device channel {c}"),
                    _ => assert!(ch.iter().all(|x| *x == 0.0), "{rate} Hz: device channel {c} is silent"),
                }
            }
        }
    }

    #[test]
    fn outputs_beyond_the_device_are_dropped_not_wrapped() {
        use soundcraft_model::{ChannelFormat, Clip, Route, SourceAudio, SourceId, TrackKind};
        // A second track on the main mix: only it sounds on a stereo device.
        let got = device_callback(2, 48_000.0, Route::Hardware("Mon 5-6".into()), |s| {
            mon_out(s);
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            s.pool.insert(
                SourceId(6),
                Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.3; 4800]] })),
            );
            let id = s.new_clip_id();
            s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "m", SourceId(6), 0, 0, 4800));
        });
        // The centred mono track alone: equal on both sides (Mon 5-6 would make them differ).
        assert!(sounds(&got[0]) && sounds(&got[1]));
        assert!(got[0].iter().zip(&got[1]).all(|(l, r)| (l - r).abs() < 1e-6), "only the main sounds");
        let only_hw = device_callback(2, 48_000.0, Route::Hardware("Mon 5-6".into()), mon_out);
        assert!(only_hw.iter().flatten().all(|x| *x == 0.0), "Mon 5-6 does not wrap onto 1-2");
    }

    #[test]
    fn device_layout_survives_a_session_change_without_reallocating() {
        let s = clip_session(4800, &[]);
        let (st, _shared, tx) = playing(&s, 48_000.0, 256);
        let mut st = st.with_device_channels(8);
        let ids = |st: &AudioState| -> Vec<(usize, *const f32)> { st.pending.iter().map(|c| (c.capacity(), c.as_ptr())).collect() };
        let mut out = vec![0.0f32; 8 * 128];
        st.fill(&mut out, 8);
        let before = ids(&st);
        assert_eq!(before.len(), 8);
        let mut moved = s.clone();
        moved.outputs[0].first_channel = 6;
        tx.send(Cmd::Session(Arc::new(moved), Vec::new())).unwrap();
        st.fill(&mut out, 8);
        assert_eq!(ids(&st), before);
        assert!(out.chunks(8).all(|f| f[0] == 0.0 && f[1] == 0.0), "the main left 1-2");
        assert!(out.chunks(8).any(|f| f[6].abs() > 0.1) && out.chunks(8).any(|f| f[7].abs() > 0.1), "the main moved to 7-8");
    }

    /// An audio state and its UI side with the Scout return channel set up, over a session with
    /// one mono audio track holding a 0.5 clip over its first second.
    fn scout_rig_state() -> (AudioState, Side, Sender<Cmd>, Arc<Shared>, Arc<Session>, TrackId) {
        scout_rig_state_with(true)
    }

    fn scout_rig_state_with(returns: bool) -> (AudioState, Side, Sender<Cmd>, Arc<Shared>, Arc<Session>, TrackId) {
        use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
        let mut s = soundcraft_model::Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.pool.insert(
            SourceId(5),
            Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]] })),
        );
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 48_000));
        let s = Arc::new(s);
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let st = AudioState::new(rx, Arc::clone(&shared), Arc::clone(&s), 48_000.0, max_block_for(None), gtx);
        let mut side = Side::new(48_000.0, max_block_for(None), grx);
        if returns {
            side.return_scout(&tx);
        }
        (st, side, tx, shared, s, t)
    }

    fn rig(s: &Session, t: TrackId, start: Option<Samples>, end: Option<Samples>) -> Box<ScoutRig> {
        Box::new(ScoutRig::new(s, &[t], start, end))
    }

    #[test]
    fn rig_comes_back_after_stop_and_after_30_seconds() {
        let (mut st, mut side, tx, _, s, t) = scout_rig_state();
        let mut out = vec![0.0f32; 2 * 1024];
        // Stopped by the Scout button.
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        let progress = side.scout_progress.clone().unwrap();
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        for _ in 0..4 {
            st.fill(&mut out, 2);
        }
        assert!(side.take_scout().is_none(), "still capturing");
        assert_eq!(progress.load(Ordering::Relaxed), 4096);
        tx.send(Cmd::ScoutStop).unwrap();
        st.fill(&mut out, 2);
        let back = side.take_scout().expect("the rig comes back after a stop");
        assert!(back.finished());
        assert_eq!(back.frames(), 4096);
        assert_eq!(back.played(), &[Range::new(0, 4096)]);
        assert!(side.scout_progress.is_none(), "no capture is out");
        assert_eq!(side.scout_out, 0);
        assert!(back.finish(&s).iter().all(|x| x.note.is_none()));

        // Ended by itself at 30 s, while the transport keeps playing.
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        let mut back = None;
        for _ in 0..(30 * 48_000 / 1024 + 4) {
            st.fill(&mut out, 2);
            side.collect_garbage();
            if back.is_none() {
                back = side.take_scout();
            }
        }
        let back = back.expect("the rig comes back after 30 seconds");
        assert!(back.finished());
        assert_eq!(back.frames(), 30 * 48_000);
        assert!(st.playing, "the capture ending does not stop the transport");
        assert!(side.take_scout().is_none(), "returned once");
    }

    #[test]
    fn moving_the_playhead_keeps_one_capture_and_stop_ends_it() {
        let (mut st, mut side, tx, _, s, t) = scout_rig_state();
        let mut out = vec![0.0f32; 2 * 512];
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        st.fill(&mut out, 2);
        tx.send(Cmd::Locate(96_000)).unwrap();
        st.fill(&mut out, 2);
        tx.send(Cmd::Play { from: 10_000, end: None, looped: None, countoff: None }).unwrap();
        st.fill(&mut out, 2);
        assert!(side.take_scout().is_none(), "a Locate or Play does not end the capture");
        tx.send(Cmd::Stop).unwrap();
        st.fill(&mut out, 2);
        let back = side.take_scout().expect("a transport Stop ends it");
        assert!(back.finished());
        assert_eq!(back.played(), &[Range::new(0, 512), Range::new(96_000, 96_512), Range::new(10_000, 10_512)]);
        assert_eq!(back.frames(), 3 * 512);

        // Playback reaching its end stops the transport, and the capture with it.
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        tx.send(Cmd::Play { from: 0, end: Some(2000), looped: None, countoff: None }).unwrap();
        for _ in 0..8 {
            st.fill(&mut out, 2);
        }
        let back = side.take_scout().expect("the rig comes back when playback ends");
        assert_eq!(back.played(), &[Range::new(0, 2000)]);
        assert_eq!(back.frames(), 2000);
    }

    #[test]
    fn a_capture_runs_while_the_transport_is_stopped() {
        let (mut st, mut side, tx, shared, s, t) = scout_rig_state();
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        let mut out = vec![0.5f32; 2 * 1024];
        st.fill(&mut out, 2);
        assert!(out.iter().all(|x| *x == 0.0), "the output stays silent");
        assert!(!shared.playing.load(Ordering::Relaxed));
        assert_eq!(side.scout_progress.as_ref().unwrap().load(Ordering::Relaxed), 1024, "the capture runs");
        // A Locate while stopped keeps it running (an unbounded capture feeds until the cap or a stop).
        tx.send(Cmd::Locate(24_000)).unwrap();
        st.fill(&mut out, 2);
        assert!(side.take_scout().is_none());
        assert_eq!(side.scout_progress.as_ref().unwrap().load(Ordering::Relaxed), 2048);
        tx.send(Cmd::ScoutStop).unwrap();
        st.fill(&mut out, 2);
        let back = side.take_scout().unwrap();
        assert!(back.played().is_empty(), "nothing on the timeline played");
        assert_eq!(back.frames(), 2048);
        // The clip under the playhead is not playing: PRE measured silence.
        let strips = back.finish(&s);
        let pre = &strips[0].base;
        assert_eq!(pre.activity_ratio, 0.0, "{pre:?}");
        assert!(pre.peak_dbfs <= -99.0, "{pre:?}");

        // The same clip while playing is heard by PRE (the check above is not vacuous).
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        for _ in 0..8 {
            st.fill(&mut out, 2);
        }
        tx.send(Cmd::ScoutStop).unwrap();
        st.fill(&mut out, 2);
        let strips = side.take_scout().unwrap().finish(&s);
        assert!(strips[0].base.peak_dbfs > -10.0, "{:?}", strips[0].base);
    }

    #[test]
    fn a_bounded_capture_is_exactly_the_loop() {
        let (mut st, mut side, tx, _, s, t) = scout_rig_state();
        let lp = Range::new(10_000, 30_000);
        let mut out = vec![0.0f32; 2 * 1024];
        // Armed while stopped with the playhead inside the loop: nothing is fed until it plays,
        // and then only from the block containing the loop start.
        tx.send(Cmd::Locate(20_000)).unwrap();
        side.arm(&tx, rig(&s, t, Some(lp.start), Some(lp.end)), Cmd::ScoutArm);
        st.fill(&mut out, 2);
        assert_eq!(side.scout_progress.as_ref().unwrap().load(Ordering::Relaxed), 0, "stopped: nothing fed");
        tx.send(Cmd::Play { from: 20_000, end: None, looped: Some(lp), countoff: None }).unwrap();
        let mut back = None;
        for _ in 0..80 {
            st.fill(&mut out, 2);
            back = back.or_else(|| side.take_scout());
        }
        let back = back.expect("the rig comes back at the loop end");
        assert_eq!(back.played(), &[lp]);
        assert_eq!(back.frames(), 20_000);
        assert!(st.playing, "the loop keeps playing");

        // `scout_loop`'s command arms, locates and plays in one batch, wherever the playhead was.
        tx.send(Cmd::Locate(50_000)).unwrap();
        side.arm(&tx, rig(&s, t, Some(lp.start), Some(lp.end)), |rig| Cmd::ScoutLoop { rig, range: lp });
        let mut back = None;
        for _ in 0..40 {
            st.fill(&mut out, 2);
            back = back.or_else(|| side.take_scout());
        }
        let back = back.expect("the loop capture comes back");
        assert_eq!(back.played(), &[lp]);
        assert_eq!(back.frames(), 20_000);
    }

    #[test]
    fn refused_rigs_and_an_engine_swap_send_rigs_back() {
        let (mut st, mut side, tx, _, s, t) = scout_rig_state();
        let mut out = vec![0.0f32; 2 * 512];
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        let first = side.scout_progress.clone().unwrap();
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        st.fill(&mut out, 2);
        // A second capture while one runs is refused and handed back unfinished.
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        st.fill(&mut out, 2);
        let refused = side.take_scout().expect("the refused rig comes back");
        assert!(!refused.finished());
        assert!(Arc::ptr_eq(side.scout_progress.as_ref().unwrap(), &first), "the running capture's progress is kept");
        // A sample-rate change rebuilds the engine: the armed rig comes back first.
        let mut s2 = (*s).clone();
        s2.sample_rate = soundcraft_time::SampleRate::HZ_44100;
        tx.send(Cmd::Session(Arc::new(s2), Vec::new())).unwrap();
        st.fill(&mut out, 2);
        let back = side.take_scout().expect("the armed rig comes back with the old engine");
        assert!(back.finished() && back.frames() > 0);
        assert!(!st.mix.scout_armed());
    }

    #[test]
    fn rigs_are_held_not_dropped_until_the_way_back_is_set_up() {
        let (mut st, mut side, tx, _, s, t) = scout_rig_state_with(false);
        let mut out = vec![0.0f32; 2 * 512];
        // One armed, the rest refused, before the return channel exists: they wait in the hold.
        let watched: Vec<Arc<AtomicU64>> = (0..SCOUT_RETURNS)
            .map(|_| {
                let r = rig(&s, t, None, None);
                let p = r.progress();
                tx.send(Cmd::ScoutArm(r)).unwrap();
                p
            })
            .collect();
        st.fill(&mut out, 2);
        st.fill(&mut out, 2);
        assert!(watched.iter().all(|p| Arc::strong_count(p) == 2), "no rig was dropped on the audio thread");
        side.return_scout(&tx);
        tx.send(Cmd::ScoutStop).unwrap();
        st.fill(&mut out, 2);
        side.collect_scout();
        assert_eq!(side.scout_ready.len(), SCOUT_RETURNS, "every rig comes back");
        // The UI refuses to send more than SCOUT_RETURNS rigs out at once.
        side.scout_out = SCOUT_RETURNS;
        side.arm(&tx, rig(&s, t, None, None), Cmd::ScoutArm);
        assert_eq!(side.scout_ready.len(), SCOUT_RETURNS + 1);
        assert_eq!(side.scout_out, SCOUT_RETURNS);
    }

    #[test]
    fn a_refused_loop_capture_leaves_the_transport_as_it_was() {
        let (st, mut side, tx, shared, s, t) = scout_rig_state();
        shared.position.store(777, Ordering::Relaxed);
        let range = Range::new(10_000, 30_000);
        // Too many rigs out: refused on this side.
        side.scout_out = SCOUT_RETURNS;
        assert!(!side.scout_loop(&shared, &tx, rig(&s, t, Some(range.start), Some(range.end)), range), "refused");
        assert!(!shared.playing.load(Ordering::Relaxed), "nothing plays");
        assert_eq!(shared.position.load(Ordering::Relaxed), 777);
        assert!(side.take_scout().is_some_and(|r| !r.finished()), "the rig comes back unfinished");
        // No audio thread (the web's silent clock): the send fails.
        side.scout_out = 0;
        drop(st);
        assert!(!side.scout_loop(&shared, &tx, rig(&s, t, Some(range.start), Some(range.end)), range));
        assert!(!shared.playing.load(Ordering::Relaxed));
        assert_eq!(shared.position.load(Ordering::Relaxed), 777);
        assert!(side.take_scout().is_some_and(|r| !r.finished()));
    }

    #[test]
    fn an_accepted_loop_capture_shows_the_transport_playing_from_the_loop() {
        let (_st, mut side, tx, shared, s, t) = scout_rig_state();
        let range = Range::new(10_000, 30_000);
        assert!(side.scout_loop(&shared, &tx, rig(&s, t, Some(range.start), Some(range.end)), range), "accepted");
        assert!(shared.playing.load(Ordering::Relaxed));
        assert_eq!(shared.position.load(Ordering::Relaxed), 10_000);
        assert_eq!(side.scout_out, 1);
    }

    #[test]
    fn main_buffers_survive_a_main_format_change_without_reallocating() {
        use soundcraft_model::ChannelFormat;
        let s = clip_session(4800, &[]);
        let (mut st, _shared, tx) = playing(&s, 48_000.0, 256);
        let ids = |st: &AudioState| -> (usize, *const Vec<f32>, Vec<(usize, *const f32)>) {
            (st.main.capacity(), st.main.as_ptr(), st.main.iter().map(|c| (c.capacity(), c.as_ptr())).collect())
        };
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        let before = ids(&st);
        for format in [ChannelFormat::Surround51, ChannelFormat::Atmos712, ChannelFormat::Stereo] {
            let mut changed = s.clone();
            changed.outputs[0].format = format;
            tx.send(Cmd::Session(Arc::new(changed), Vec::new())).unwrap();
            st.fill(&mut out, 2);
            assert_eq!(ids(&st), before, "{format:?}");
            assert!(out.iter().any(|x| x.abs() > 0.05), "{format:?}: the clip still plays");
        }
    }

    #[test]
    fn recording_is_refused_off_speed() {
        // Half speed reads the input elastically: a take would lose input. Refused before it
        // starts, and half speed waits while a take records.
        let (player, _st) = Player::offline(Arc::new(Session::default()), 48_000, 256, 2);
        player.set_speed(0.5);
        let err = player.can_record().unwrap_err();
        assert_eq!(err, "Recording needs full speed: at half speed the take would lose input. Stop, then record.");
        player.set_speed(1.0);
        assert_eq!(player.can_record(), Ok(()));
        player.set_recording(true);
        player.set_speed(0.5);
        assert_eq!(player.speed(), 1.0, "no half speed while a take records");
        player.set_recording(false);
        player.set_speed(0.5);
        assert_eq!(player.speed(), 0.5);
    }

    #[test]
    fn the_silent_clock_lays_out_a_stereo_device() {
        // Its thread fills stereo frames: a 5.1 session's layout is stereo from the start, so
        // the first fill doesn't lay it out again (dropping what was rendered).
        let mut s = Session::default();
        s.outputs[0].format = soundcraft_model::ChannelFormat::Surround51;
        let st = clock_state(channel().1, Arc::new(Shared::new()), Arc::new(s), channel().0);
        assert_eq!((st.pending.len(), st.out.device_channels()), (2, 2));
    }

    #[test]
    fn reported_latency_reads_the_device_feed_and_mix_live() {
        let (player, mut st) = Player::offline(Arc::new(clip_session(4800, &[])), 48_000, 256, 2);
        assert_eq!(player.reported_latency(), Latency::default());
        let feed = feed::InputFeed::fixed_cushioned(1, 256);
        st.attach_input(Arc::clone(&feed), vec![vec![0.0; 256]]);
        feed.set_input_latency(std::time::Duration::from_micros(2667));
        player.shared.out_latency_ns.store(5_000_000, Ordering::Relaxed);
        let mut out = vec![0.0f32; 2 * 128];
        for _ in 0..3 {
            feed.push(&[0.1; 128]);
            st.fill(&mut out, 2);
        }
        assert_eq!(player.reported_latency(), Latency { input: 128, output: 240, feed: 128, mix: st.mix.latency as u32 });
        assert_eq!(estimate(player.reported_latency()), 496);
    }

    #[test]
    fn asio_latency_replaces_the_timestamp_estimate() {
        let (mut player, _st) = Player::offline(Arc::new(clip_session(4800, &[])), 48_000, 256, 2);
        player.shared.in_latency_ns.store(2_666_667, Ordering::Relaxed);
        player.shared.out_latency_ns.store(5_000_000, Ordering::Relaxed);
        let l = player.reported_latency();
        assert_eq!((l.input, l.output), (128, 240), "no driver figure: the timestamps");
        player.driver_latency = Some((96, 160));
        let l = player.reported_latency();
        assert_eq!((l.input, l.output), (96, 160), "the driver's figure replaces them");
        // The driver counts device frames: at another device rate they become session frames.
        let (mut player, _st) = Player::offline(Arc::new(clip_session(4800, &[])), 96_000, 256, 2);
        player.driver_latency = Some((192, 320));
        let l = player.reported_latency();
        assert_eq!((l.input, l.output), (96, 160));
    }

    /// On the owner's machine: the figure comes from the very driver cpal opened (the shim and
    /// cpal share asio-sys's one SDK library). `SC_ASIO_DEVICE` picks the driver.
    #[cfg(all(feature = "asio", windows))]
    #[test]
    #[ignore = "opens the real ASIO driver"]
    fn asio_driver_latency_comes_from_the_open_driver() {
        let cfg = AudioConfig { system: Some("ASIO".into()), output: std::env::var("SC_ASIO_DEVICE").ok(), ..AudioConfig::default() };
        let player = Player::with_config(Arc::new(Session::default()), &cfg);
        eprintln!("{} (asio {}): driver {:?}, reported {:?}", player.device_name, player.asio, player.driver_latency, player.reported_latency());
        assert!(player.asio, "no ASIO device opened");
        assert!(player.driver_latency.is_some_and(|(i, o)| i > 0 && o > 0));
        assert_eq!(soundcraft_asio_latency::driver_latencies().ok(), player.driver_latency, "the same driver, read again");
    }

    /// Measures on an offline player (48 kHz, stereo device) whose input is `feed`, in callbacks
    /// of `block` in ASIO order (each callback's input first). The input is device output 1
    /// through `delay` frames when there is one (a loopback cable), plus white noise of RMS
    /// `noise`. The measurement starts once the feed has settled.
    fn measure_loop(feed: &Arc<feed::InputFeed>, block: usize, delay: Option<usize>, noise: f32) -> Result<i64, String> {
        let mut s = Session::default();
        s.edit.click = false;
        let (player, mut st) = Player::offline(Arc::new(s), 48_000, 256, 2);
        st.attach_input(Arc::clone(feed), vec![vec![0.0; 256]; feed.channels()]);
        let mut line: std::collections::VecDeque<f32> = std::iter::repeat_n(0.0, delay.unwrap_or(0)).collect();
        let mut rng = 0x1234_5678u32;
        let mut out = vec![0.0f32; 2 * block];
        for k in 0..20_000 {
            let input: Vec<f32> = (0..block)
                .map(|_| {
                    rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let white = ((rng >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 3f32.sqrt();
                    let looped = if delay.is_some() { line.pop_front().unwrap_or(0.0) } else { 0.0 };
                    looped + noise * white
                })
                .collect();
            feed.push(&input);
            st.fill(&mut out, 2);
            if delay.is_some() {
                line.extend(out.chunks(2).map(|f| f[0]));
            }
            if k == 8 {
                player.measure_latency(0, 0)?;
            }
            if let Some(r) = player.measure_result() {
                return r;
            }
        }
        Err("the measurement never came back".into())
    }

    #[test]
    fn measure_finds_a_known_round_trip() {
        for delay in [37, 300, 1500] {
            let feed = feed::InputFeed::fixed(1, 256);
            assert_eq!(measure_loop(&feed, 32, Some(delay), 0.0), Ok(delay as i64), "delay {delay}");
        }
        // With the fixed feed's offset of 128 (one block queued ahead): the round trip has it too.
        for delay in [300, 1500] {
            let feed = feed::InputFeed::fixed_cushioned(1, 256);
            assert_eq!(measure_loop(&feed, 128, Some(delay), 0.0), Ok(delay as i64 + 128), "delay {delay}");
            assert_eq!(feed.extra_latency(), 128);
        }
        // Over a quiet input (−60 dBFS noise).
        assert_eq!(measure_loop(&feed::InputFeed::fixed(1, 256), 32, Some(300), 0.001), Ok(300));
    }

    /// Records a known click through the loopback harness after a measurement, and returns
    /// where the take places it (with [`measure::record_latency`], the record offset) and where
    /// it is on the timeline. A mono click track plays an impulse at 12 000 out of device output
    /// 1, which comes back on input 1 through `delay` frames into `feed`. An armed track
    /// (monitoring Off, so nothing feeds back) holds a latency-adding insert: with LLM off the
    /// mix delays the click track to line up with it; with `llm` on it is bypassed and the mix
    /// adds nothing. Callbacks of 128 in ASIO order.
    fn loopback_take(feed: &Arc<feed::InputFeed>, delay: usize, llm: bool) -> (i64, i64) {
        use soundcraft_model::{ChannelFormat, Clip, InputMonitor, Insert, SourceAudio, SourceId, TrackKind};
        const CLICK_AT: usize = 12_000;
        let mut s = Session::default();
        s.edit.click = false;
        s.edit.set_flag("options.low_latency_monitoring", llm);
        let click = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut audio = vec![0.0f32; 48_000];
        audio[CLICK_AT] = 0.5;
        s.pool.insert(SourceId(9), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![audio] })));
        let id = s.new_clip_id();
        s.track_mut(click).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "click", SourceId(9), 0, 0, 48_000));
        let armed = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let t = s.track_mut(armed).unwrap();
        t.mixer.record_arm = true;
        t.mixer.input_monitor = InputMonitor::Off;
        t.mixer.add_module(Insert::new("maximizer"), None).unwrap();
        let s = Arc::new(s);
        let (player, mut st) = Player::offline(Arc::clone(&s), 48_000, 256, 2);
        player.update_session(Arc::clone(&s));
        st.attach_input(Arc::clone(feed), vec![vec![0.0; 256]; feed.channels()]);
        let capture = record::Capture::for_rate(48_000, feed.channels());
        st.capture = Some(Arc::clone(&capture));
        let mut line: std::collections::VecDeque<f32> = std::iter::repeat_n(0.0, delay).collect();
        let mut out = vec![0.0f32; 2 * 128];
        let mut measured = None;
        let mut take = Vec::new();
        for k in 0..20_000 {
            let input: Vec<f32> = (0..128).map(|_| line.pop_front().unwrap_or(0.0)).collect();
            feed.push(&input);
            st.fill(&mut out, 2);
            line.extend(out.chunks(2).map(|f| f[0]));
            if k == 8 {
                player.measure_latency(0, 0).unwrap();
            }
            if measured.is_none()
                && let Some(r) = player.measure_result()
            {
                // The feed's latency when the measurement came back goes with it.
                assert_eq!(player.measure_feed(), feed.extra_latency(), "the feed at the click");
                measured = Some((r.unwrap(), player.measure_feed()));
                capture.armed.store(true, Ordering::Release);
                player.set_recording(true);
                player.play(0, None, None);
            }
            capture.ring.pop_all(&mut take);
            if player.position() > 24_000 {
                break;
            }
        }
        assert!(capture.started.load(Ordering::Acquire), "the take started");
        let start = capture.start_pos.load(Ordering::Acquire);
        let mix = capture.start_mix.load(Ordering::Acquire);
        assert_eq!(mix > 0, !llm, "the mix delays the click track only with LLM off ({mix})");
        let at_start = Latency { feed: capture.start_feed.load(Ordering::Acquire), mix, ..Latency::default() };
        let latency = measure::record_latency(measured, 0, at_start);
        let peak = take.iter().enumerate().fold((0, 0.0f32), |best, (i, x)| if x.abs() > best.1 { (i, x.abs()) } else { best }).0;
        (start + peak as i64 - latency, CLICK_AT as i64)
    }

    #[test]
    fn a_recorded_click_lands_in_time_after_a_measure() {
        // ASIO-like: one clock, the fixed feed (no offset), and one with a block queued ahead.
        for (name, delay) in [("fixed", 300), ("fixed", 1500), ("cushioned", 300)] {
            let feed = if name == "fixed" { feed::InputFeed::fixed(1, 256) } else { feed::InputFeed::fixed_cushioned(1, 256) };
            let (landed, want) = loopback_take(&feed, delay, false);
            assert!((landed - want).abs() <= 1, "{name}, delay {delay}: landed at {landed}, the click is at {want}");
        }
    }

    #[test]
    fn llm_does_not_move_where_takes_land() {
        // The mix's own delay drops under LLM (the latent insert is bypassed): the take still
        // lands on the click, with LLM on and off.
        for llm in [false, true] {
            let (landed, want) = loopback_take(&feed::InputFeed::fixed(1, 256), 300, llm);
            assert!((landed - want).abs() <= 1, "LLM {llm}: landed at {landed}, the click is at {want}");
        }
    }

    #[test]
    fn the_last_callback_size_is_kept() {
        let (player, mut st) = Player::offline(Arc::new(Session::default()), 48_000, 256, 2);
        assert_eq!(player.callback_frames(), None, "no callback yet");
        let mut out = vec![0.0f32; 2 * 96];
        st.fill(&mut out, 2);
        assert_eq!(player.callback_frames(), Some(96));
    }

    #[test]
    fn measure_reports_no_click_when_nothing_comes_back() {
        // A silent input, and −40 dBFS white noise, without the loop.
        for noise in [0.0, 0.01] {
            let r = measure_loop(&feed::InputFeed::fixed(1, 256), 128, None, noise);
            assert_eq!(r, Err("No click came back on input 1: patch output 1 to it".to_string()), "noise {noise}");
        }
    }

    #[test]
    fn measure_is_refused_while_playing_or_without_input() {
        let s = Arc::new(Session::default());
        let (player, mut st) = Player::offline(Arc::clone(&s), 48_000, 256, 2);
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        assert_eq!(player.measure_latency(0, 0), Err("no audio input".to_string()));
        st.attach_input(feed::InputFeed::fixed(2, 256), vec![vec![0.0; 256]; 2]);
        st.fill(&mut out, 2);
        assert_eq!(player.measure_latency(0, 2), Err("input 3 is not on the device".to_string()));
        assert_eq!(player.measure_latency(2, 0), Err("output 3 is not on the device".to_string()));
        assert!(player.measure_latency(usize::MAX, usize::MAX).is_err(), "hostile channels");
        player.play(0, None, None);
        assert_eq!(player.measure_latency(0, 0), Err("stop the transport first".to_string()));
        player.stop();
        player.set_recording(true);
        assert_eq!(player.measure_latency(0, 0), Err("stop the transport first".to_string()));
        player.set_recording(false);
        player.set_speed(0.5);
        assert_eq!(player.measure_latency(0, 0), Err("Measure needs the session and the device at the same rate".to_string()));
        player.set_speed(1.0);
        let (other, mut ost) = Player::offline(Arc::clone(&s), 44_100, 256, 2);
        ost.attach_input(feed::InputFeed::drift(1, 44_100, 48_000, 256), vec![vec![0.0; 256]]);
        ost.fill(&mut out, 2);
        assert_eq!(other.measure_latency(0, 0), Err("Measure needs the session and the device at the same rate".to_string()));
        assert_eq!(player.measure_latency(1, 1), Ok(()));
        assert_eq!(player.measure_latency(0, 0), Err("a measurement is already running".to_string()));
    }

    #[test]
    fn a_measure_ends_when_the_transport_starts() {
        let (player, mut st) = Player::offline(Arc::new(Session::default()), 48_000, 256, 2);
        let feed = feed::InputFeed::fixed(1, 256);
        st.attach_input(Arc::clone(&feed), vec![vec![0.0; 256]]);
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        player.measure_latency(0, 0).unwrap();
        st.fill(&mut out, 2);
        assert_eq!(player.measure_result(), None, "still running");
        player.play(0, None, None);
        st.fill(&mut out, 2);
        let r = player.measure_result();
        assert!(matches!(&r, Some(Err(e)) if e.contains("interrupted")), "{r:?}");
        assert_eq!(player.measure_result(), None, "reported once");
    }

    #[test]
    fn measure_buffers_do_not_allocate_on_the_audio_thread() {
        let s = Arc::new(Session::default());
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::new(Shared::new()), s, 48_000.0, 256, channel().0).with_device_channels(2);
        let feed = feed::InputFeed::fixed(1, 256);
        st.attach_input(Arc::clone(&feed), vec![vec![0.0; 256]]);
        let (mtx, mrx) = sync_channel(1);
        tx.send(Cmd::MeasureReturn(mtx)).unwrap();
        let mut out = vec![0.0f32; 2 * 441];
        feed.push(&[0.0; 441]);
        st.fill(&mut out, 2);
        let buffers =
            |st: &AudioState| -> Vec<(usize, *const f32)> { st.pending.iter().chain(&st.mix.input).map(|c| (c.capacity(), c.as_ptr())).collect() };
        let held = buffers(&st);
        let m = measure::MeasureBuf::new(0, 0, 48_000);
        let sent = (m.samples.capacity(), m.samples.as_ptr(), std::ptr::from_ref::<measure::MeasureBuf>(&m));
        tx.send(Cmd::Measure(m)).unwrap();
        let back = (0..1000)
            .find_map(|_| {
                feed.push(&[0.0; 441]);
                st.fill(&mut out, 2);
                mrx.try_recv().ok()
            })
            .unwrap();
        assert_eq!((back.samples.capacity(), back.samples.as_ptr(), std::ptr::from_ref::<measure::MeasureBuf>(&back)), sent);
        assert!(back.full() && !back.interrupted);
        assert_eq!(buffers(&st), held);
    }

    #[test]
    fn a_measure_that_never_comes_back_times_out() {
        // The output callback stopped (device gone) while a measurement was out: it times out,
        // and Measure works again without reopening the device.
        let (player, mut st) = Player::offline(Arc::new(Session::default()), 48_000, 256, 2);
        st.attach_input(feed::InputFeed::fixed(1, 256), vec![vec![0.0; 256]]);
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        player.measure_latency(0, 0).unwrap();
        assert_eq!(player.measure_result(), None, "not yet");
        player.side().measure_since = std::time::Instant::now().checked_sub(measure::TIMEOUT);
        assert_eq!(player.measure_result(), Some(Err("Measure timed out: the audio output stopped".to_string())));
        assert_eq!(player.measure_result(), None, "reported once");
        assert_eq!(player.measure_latency(0, 0), Ok(()), "not still running");
    }

    #[test]
    fn a_cancelled_measure_frees_measure_and_its_buffer_is_ignored() {
        let (player, mut st) = Player::offline(Arc::new(Session::default()), 48_000, 256, 2);
        let feed = feed::InputFeed::fixed(1, 256);
        st.attach_input(Arc::clone(&feed), vec![vec![0.0; 256]]);
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        player.measure_latency(0, 0).unwrap();
        st.fill(&mut out, 2);
        player.cancel_measure();
        assert_eq!(player.measure_latency(0, 0), Ok(()), "a cancelled measurement is not running");
        // The cancelled one comes back interrupted and is dropped; the new one reports.
        let r = (0..2000).find_map(|_| {
            feed.push(&[0.0; 128]);
            st.fill(&mut out, 2);
            player.measure_result()
        });
        assert_eq!(r, Some(Err("No click came back on input 1: patch output 1 to it".to_string())));
    }

    /// A plugin that records the thread it is dropped (terminated) on.
    struct DropSpy {
        inner: Box<dyn Plugin>,
        dropped_on: Arc<Mutex<Option<String>>>,
    }

    impl Plugin for DropSpy {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.inner.process(io, frames);
        }
    }

    impl Drop for DropSpy {
        fn drop(&mut self) {
            *self.dropped_on.lock().unwrap() = Some(std::thread::current().name().unwrap_or("?").to_string());
        }
    }

    #[test]
    fn a_replaced_players_plugin_instances_are_dropped_on_the_dropping_thread_not_the_audio_thread() {
        let session = Arc::new(Session::default());
        let (player, mut st) = Player::offline(Arc::clone(&session), 48_000, 256, 2);
        let dropped_on = Arc::new(Mutex::new(None));
        let spy = DropSpy { inner: soundcraft_dsp::create("gain").unwrap(), dropped_on: Arc::clone(&dropped_on) };
        let instance = PreparedInstance { track: TrackId(1), slot: SlotKey::Instrument, id: "vst3:fake".into(), ch: 2, plugin: Box::new(spy) };
        player.tx.send(Cmd::Session(session, vec![instance])).unwrap();
        // The "audio thread": fills until the shutdown, then goes away with its state.
        let audio = std::thread::Builder::new()
            .name("fake-audio".into())
            .spawn(move || {
                let mut out = vec![0.0f32; 2 * 128];
                while !st.shut {
                    st.fill(&mut out, 2);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                st.fill(&mut out, 2);
                assert!(out.iter().all(|x| *x == 0.0), "silence after the shutdown");
            })
            .unwrap();
        assert!(dropped_on.lock().unwrap().is_none(), "held by the audio thread");
        drop(player);
        audio.join().unwrap();
        let me = std::thread::current().name().unwrap_or("?").to_string();
        assert_eq!(dropped_on.lock().unwrap().as_deref(), Some(me.as_str()), "dropped where the player was dropped");
    }

    #[test]
    fn a_lent_engine_returned_after_the_players_shutdown_is_dropped_on_the_dropping_thread_not_the_audio_thread() {
        // The player goes while its engine is out on a render (the app closing mid-render): the
        // shutdown's garbage holds the spare's nothing, and the render's instances come back
        // later, while the stream may still drain commands. They are dropped here.
        let session = Arc::new(Session::default());
        let (player, mut st) = Player::offline(Arc::clone(&session), 48_000, 256, 2);
        let dropped_on = Arc::new(Mutex::new(None));
        let spy = DropSpy { inner: soundcraft_dsp::create("gain").unwrap(), dropped_on: Arc::clone(&dropped_on) };
        let instance = PreparedInstance { track: TrackId(1), slot: SlotKey::Instrument, id: "vst3:fake".into(), ch: 2, plugin: Box::new(spy) };
        player.tx.send(Cmd::Session(Arc::clone(&session), vec![instance])).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        // The "audio thread" keeps draining commands after the shutdown (the stream is not down
        // yet) until the test ends it.
        let audio = std::thread::Builder::new()
            .name("fake-audio".into())
            .spawn(move || {
                let mut out = vec![0.0f32; 2 * 128];
                while !stopping.load(Ordering::Relaxed) {
                    st.fill(&mut out, 2);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert!(st.shut);
                assert_eq!(st.mix.hosted_instances(), 0, "the returned engine was never seated");
            })
            .unwrap();
        let mut lending = player.lend_instances(Arc::clone(&session)).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let live = loop {
            if let Some(l) = lending.poll() {
                break l;
            }
            assert!(std::time::Instant::now() < deadline, "the audio thread never handed its engine over");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(live.hosted_instances(), 1);
        drop(player);
        assert!(dropped_on.lock().unwrap().is_none(), "the render still holds the instance");
        drop(live);
        let me = std::thread::current().name().unwrap_or("?").to_string();
        assert_eq!(dropped_on.lock().unwrap().as_deref(), Some(me.as_str()), "dropped where the render was dropped");
        stop.store(true, Ordering::Relaxed);
        audio.join().unwrap();
    }

    /// `(plugin id, thread)` of every fake the test factory made (the factory stays installed:
    /// the tests below run in parallel and tell their fakes apart by id).
    static PRELOAD_MADE_ON: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    fn preload_factory(id: &str) -> Option<Box<dyn Plugin>> {
        if !id.starts_with("clap:test.preload") {
            return None;
        }
        PRELOAD_MADE_ON.lock().unwrap().push((id.to_string(), std::thread::current().name().unwrap_or("?").to_string()));
        Some(Box::new(DropSpy { inner: soundcraft_dsp::create("gain").unwrap(), dropped_on: Arc::new(Mutex::new(None)) }))
    }

    /// A session with `n` fake hosted modules on one track.
    fn preload_session(tag: &str, n: usize) -> Arc<Session> {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        for i in 0..n {
            s.track_mut(t).unwrap().mixer.add_module(Insert::new(format!("clap:test.preload.{tag}.{i}")), None).unwrap();
        }
        Arc::new(s)
    }

    /// The threads the fakes tagged `tag` were made on.
    fn made_on(tag: &str) -> Vec<String> {
        let prefix = format!("clap:test.preload.{tag}.");
        PRELOAD_MADE_ON.lock().unwrap().iter().filter(|(id, _)| id.starts_with(&prefix)).map(|(_, t)| t.clone()).collect()
    }

    #[test]
    fn preloaded_instances_are_made_a_step_at_a_time_on_the_calling_thread_and_handed_over_at_the_swap() {
        soundcraft_mix::set_test_plugin_factory(Some(preload_factory));
        let old = Arc::new(Session::default());
        let (player, mut st) = Player::offline(Arc::clone(&old), 48_000, 256, 2);
        let new = preload_session("swap", 3);
        assert_eq!(player.preload_begin(&new), 3);
        let p = player.preload_step(std::time::Duration::ZERO).unwrap();
        assert_eq!((p.done, p.total, p.next.as_deref()), (1, 3, Some("clap:test.preload.swap.1")), "one per step with no budget");
        assert!(!p.finished());
        let p = player.preload_step(std::time::Duration::from_secs(5)).unwrap();
        assert!(p.finished(), "{p:?}");
        assert_eq!(p.next, None);
        assert!(p.failed.is_empty());
        let me = std::thread::current().name().unwrap_or("?").to_string();
        assert_eq!(made_on("swap"), vec![me.clone(), me.clone(), me.clone()], "made on the caller's thread");
        // The audio thread still runs the old session: nothing hosted yet.
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        assert_eq!(st.mix.hosted_instances(), 0);
        // The swap hands the three over without making any more.
        player.update_session(Arc::clone(&new));
        st.fill(&mut out, 2);
        assert_eq!(st.mix.hosted_instances(), 3);
        assert_eq!(made_on("swap").len(), 3, "no instance made again at the swap");
        assert!(player.preload_progress().is_none(), "the preload is spent");
        // Another update of the same session makes nothing either (they are current).
        player.update_session(Arc::clone(&new));
        assert_eq!(made_on("swap").len(), 3);
    }

    #[test]
    fn a_cancelled_preload_drops_its_instances_on_the_calling_thread_and_the_old_session_plays_on() {
        soundcraft_mix::set_test_plugin_factory(Some(preload_factory));
        let old = Arc::new(Session::default());
        let (player, mut st) = Player::offline(Arc::clone(&old), 48_000, 256, 2);
        let new = preload_session("cancel", 2);
        player.preload_begin(&new);
        player.preload_step(std::time::Duration::from_secs(5));
        // One spy of our own among them, to see where it is dropped.
        let dropped_on = Arc::new(Mutex::new(None));
        {
            let mut side = player.side();
            let pre = side.preload.as_mut().unwrap();
            let spy = DropSpy { inner: soundcraft_dsp::create("gain").unwrap(), dropped_on: Arc::clone(&dropped_on) };
            pre.made[0].instance.plugin = Box::new(spy);
        }
        player.preload_cancel();
        let me = std::thread::current().name().unwrap_or("?").to_string();
        assert_eq!(dropped_on.lock().unwrap().as_deref(), Some(me.as_str()), "dropped where it was cancelled");
        assert!(player.preload_progress().is_none());
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        assert_eq!(st.mix.hosted_instances(), 0, "the old session is untouched");
        assert!(Arc::ptr_eq(&st.session, &old));
    }
}

#[cfg(test)]
mod lend_tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Insert, TrackKind};
    use std::sync::mpsc::channel;

    /// A session with a hosted module on one track, and a prepared (fake) instance for it.
    fn hosted() -> (Arc<Session>, PreparedInstance) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let m = s.track_mut(t).unwrap().mixer.add_module(Insert::new("clap:test.fake"), None).unwrap();
        let plugin = soundcraft_dsp::create("gain").unwrap();
        (Arc::new(s), PreparedInstance { track: t, slot: SlotKey::Module(m), id: "clap:test.fake".into(), ch: 2, plugin })
    }

    #[test]
    fn the_audio_thread_lends_its_engine_and_takes_it_back() {
        let (s, instance) = hosted();
        let shared = Arc::new(Shared::new());
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::clone(&s), 48_000.0, max_block_for(None), gtx);
        tx.send(Cmd::Session(Arc::clone(&s), vec![instance])).unwrap();
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        let mut out = vec![0.0f32; 256];
        st.fill(&mut out, 2);
        assert!(st.playing);
        assert_eq!(st.mix.hosted_instances(), 1);
        // Lend: the transport stops, the live engine comes out through the channel, the spare
        // plays silence, and a Play meanwhile does not start.
        let (etx, erx) = sync_channel(1);
        tx.send(Cmd::Lend(Box::new(new_engine(48_000.0, max_block_for(None))), etx)).unwrap();
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        st.fill(&mut out, 2);
        assert!(st.lent && !st.playing && !shared.playing.load(Ordering::Relaxed));
        let live = erx.try_recv().expect("the engine came out");
        assert_eq!(live.hosted_instances(), 1, "the live instance went with it");
        assert_eq!(st.mix.hosted_instances(), 0, "the spare has none");
        assert!(grx.try_recv().is_err(), "nothing retired or dropped");
        // Return: the spare goes to the UI thread as garbage and play works again.
        tx.send(Cmd::Return(live)).unwrap();
        tx.send(Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        st.fill(&mut out, 2);
        assert!(!st.lent && st.playing);
        assert_eq!(st.mix.hosted_instances(), 1);
        let g = grx.try_recv().expect("the spare came back");
        assert!(g.engine.is_some() && g.plugins.is_empty());
    }

    #[test]
    fn a_lend_nobody_waits_for_is_called_off() {
        let (s, instance) = hosted();
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, Arc::new(Shared::new()), Arc::clone(&s), 48_000.0, max_block_for(None), gtx);
        tx.send(Cmd::Session(Arc::clone(&s), vec![instance])).unwrap();
        let (etx, erx) = sync_channel(1);
        drop(erx);
        tx.send(Cmd::Lend(Box::new(new_engine(48_000.0, max_block_for(None))), etx)).unwrap();
        let mut out = vec![0.0f32; 256];
        st.fill(&mut out, 2);
        assert!(!st.lent, "the engine stays in service");
        assert_eq!(st.mix.hosted_instances(), 1);
        assert!(grx.try_recv().unwrap().engine.is_some(), "the spare is dropped on the UI thread");
    }

    #[test]
    fn a_shutdown_while_lent_answers_at_once_and_the_engine_returned_after_it_goes_back_as_garbage() {
        let (s, instance) = hosted();
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, Arc::new(Shared::new()), Arc::clone(&s), 48_000.0, max_block_for(None), gtx);
        tx.send(Cmd::Session(Arc::clone(&s), vec![instance])).unwrap();
        let (etx, erx) = sync_channel(1);
        tx.send(Cmd::Lend(Box::new(new_engine(48_000.0, max_block_for(None))), etx)).unwrap();
        let mut out = vec![0.0f32; 256];
        st.fill(&mut out, 2);
        let live = erx.try_recv().expect("the engine came out");
        assert_eq!(live.hosted_instances(), 1);
        // Shutdown while lent: the answer comes at once (the player's drop waits for it on the
        // very thread the render holds the engine on) with the spare's nothing.
        tx.send(Cmd::Shutdown).unwrap();
        st.fill(&mut out, 2);
        assert!(st.shut);
        let g = grx.try_recv().expect("the shutdown's answer");
        assert!(g.last && g.plugins.is_empty() && g.engine.is_none());
        // The engine returned after it is never seated: it goes back whole, as garbage.
        tx.send(Cmd::Return(live)).unwrap();
        st.fill(&mut out, 2);
        assert!(out.iter().all(|x| *x == 0.0), "silence after the shutdown");
        let g = grx.try_recv().expect("the returned engine");
        assert!(g.last && g.plugins.is_empty());
        assert_eq!(g.engine.as_ref().map(|e| e.hosted_instances()), Some(1), "with its instance, to be dropped off the audio thread");
        assert_eq!(st.mix.hosted_instances(), 0, "the audio thread kept nothing");
        assert!(!st.lent);
    }

    #[test]
    fn a_lend_after_the_shutdown_is_refused() {
        let (s, instance) = hosted();
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, Arc::new(Shared::new()), Arc::clone(&s), 48_000.0, max_block_for(None), gtx);
        tx.send(Cmd::Session(Arc::clone(&s), vec![instance])).unwrap();
        tx.send(Cmd::Shutdown).unwrap();
        let mut out = vec![0.0f32; 256];
        st.fill(&mut out, 2);
        let g = grx.try_recv().expect("the shutdown's answer");
        assert!(g.last && g.plugins.len() == 1);
        let (etx, erx) = sync_channel(1);
        tx.send(Cmd::Lend(Box::new(new_engine(48_000.0, max_block_for(None))), etx)).unwrap();
        st.fill(&mut out, 2);
        assert!(!st.lent);
        assert!(matches!(erx.try_recv(), Err(TryRecvError::Disconnected)), "nothing comes, and the channel closes");
        let g = grx.try_recv().expect("the spare comes back");
        assert!(g.engine.is_some() && !g.last, "to be dropped on the UI thread");
    }

    /// A state reader standing in for a live plugin's: the state the "plugin" has now, and how
    /// often a state was loaded into it.
    struct FakeState(Arc<Mutex<Vec<u8>>>, Arc<AtomicUsize>);

    impl PluginStateReader for FakeState {
        fn save_state(&mut self) -> Option<Vec<u8>> {
            Some(self.0.lock().unwrap().clone())
        }
        fn load_state(&mut self, data: &[u8]) -> bool {
            *self.0.lock().unwrap() = data.to_vec();
            self.1.fetch_add(1, Ordering::Relaxed);
            true
        }
    }

    #[test]
    fn a_render_is_refused_while_playing_and_the_transport_cannot_start_during_one() {
        let s = Arc::new(Session::default());
        let p = Player::new(Arc::clone(&s));
        let state = Arc::new(Mutex::new(b"preset".to_vec()));
        let loads = Arc::new(AtomicUsize::new(0));
        p.attach_state_reader(TrackId(1), SlotKey::Instrument, Box::new(FakeState(Arc::clone(&state), Arc::clone(&loads))));
        p.play(0, None, None);
        assert!(p.is_playing());
        assert_eq!(p.lend_instances(Arc::clone(&s)).err().as_deref(), Some("stop the transport first"));
        p.stop();
        let mut lending = p.lend_instances(Arc::clone(&s)).unwrap();
        assert!(p.is_lent());
        assert_eq!(p.lend_instances(Arc::clone(&s)).err().as_deref(), Some("a render is already running"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let live = loop {
            if let Some(l) = lending.poll() {
                break l;
            }
            assert!(std::time::Instant::now() < deadline, "the audio thread never handed its engine over");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        // The transport cannot start during the render.
        p.play(0, None, None);
        assert!(!p.is_playing(), "play is refused while the engine is lent");
        // The render changes the plugin's state (automation moved a knob); it comes back as it was.
        *state.lock().unwrap() = b"moved".to_vec();
        drop(live);
        let _ = p.idle();
        assert_eq!(*state.lock().unwrap(), b"preset".to_vec(), "the state is as before the render");
        assert_eq!(loads.load(Ordering::Relaxed), 1);
        assert!(!p.is_lent());
        p.play(0, None, None);
        assert!(p.is_playing(), "and the transport starts again");
        // A render that leaves the state alone loads nothing back into the plugin.
        p.stop();
        let mut lending = p.lend_instances(Arc::clone(&s)).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while lending.poll().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drop(lending);
        let _ = p.idle();
        assert_eq!(loads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_lend_called_off_before_the_audio_thread_answers_gets_its_engine_back() {
        // The UI cancelled the render (Esc in the progress window) a frame after asking: the
        // audio thread answers the lend anyway. The engine, with every live instance in it, must
        // not be lost with the receiver, and the audio thread must not stay lent for ever.
        let (s, instance) = hosted();
        let (p, mut st) = Player::offline(Arc::clone(&s), 48_000, 256, 2);
        p.tx.send(Cmd::Session(Arc::clone(&s), vec![instance])).unwrap();
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        assert_eq!(st.mix.hosted_instances(), 1);
        let lending = p.lend_instances(Arc::clone(&s)).unwrap();
        drop(lending);
        assert!(p.is_lent(), "the loan is not over until the audio thread has answered");
        // The audio thread answers the lend now.
        st.fill(&mut out, 2);
        assert!(st.lent, "the engine went out");
        assert_eq!(st.mix.hosted_instances(), 0);
        // The player finds the late engine and sends it straight back.
        let _ = p.idle();
        assert!(!p.is_lent());
        st.fill(&mut out, 2);
        assert!(!st.lent, "the engine is back in service");
        assert_eq!(st.mix.hosted_instances(), 1, "with its instance");
        let _ = p.idle();
        p.play(0, None, None);
        st.fill(&mut out, 2);
        assert!(st.playing, "and the player plays");
        assert_eq!(p.lend_instances(Arc::clone(&s)).err().as_deref(), Some("stop the transport first"));
    }

    #[test]
    fn a_lend_the_audio_thread_never_takes_ends_when_the_channel_closes() {
        // No audio thread at all (the web's silent clock): the lend command is dropped unsent,
        // so the channel closes and the loan is over.
        let s = Arc::new(Session::default());
        let (p, st) = Player::offline(Arc::clone(&s), 48_000, 256, 2);
        drop(st);
        let mut lending = p.lend_instances(Arc::clone(&s)).unwrap();
        assert!(lending.poll().is_none());
        drop(lending);
        assert!(!p.is_lent(), "nothing will come: the loan is over");
        p.play(0, None, None);
        assert!(p.is_playing());
    }
}
