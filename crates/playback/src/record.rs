//! Audio input capture for recording.
//!
//! The input stream pushes each block into an [`InputFeed`] (lock-free, preallocated); the output
//! callback pops it into the mix and, while a take records, copies the same frames into the
//! [`Capture`] ring. So a take holds exactly what the mix heard, at the session rate, and its first
//! frame has an exact timeline position ([`Recorder::start_pos`]). A writer thread streams the ring
//! into one WAV file per take, rewriting the header every second so a crash leaves a playable file.
//!
//! The input stream is the player's own when input and output are one device (duplex, see
//! [`crate::duplex`] and [`crate::Player::duplex_recorder`]), else the [`Recorder`]'s
//! ([`Recorder::open_with`]).

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use soundcraft_model::TrackId;
use soundcraft_time::Samples;

use crate::feed::{InputFeed, SampleRing};

/// How take files are written: WAV (BWF with `bwf`) or AIFF, at a bit depth. The default is a
/// plain 32-bit float WAV.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeFormat {
    pub format: soundcraft_audio_io::FileFormat,
    pub depth: soundcraft_audio_io::BitDepth,
    /// The `bext` chunk of a WAV take (its description becomes the file's name); its time
    /// reference is set once the take is placed.
    pub bwf: Option<soundcraft_audio_io::BwfInfo>,
}

impl Default for TakeFormat {
    fn default() -> Self {
        TakeFormat { format: soundcraft_audio_io::FileFormat::Wav, depth: soundcraft_audio_io::BitDepth::Float32, bwf: None }
    }
}

/// What [`Recorder::finish`] returns: the takes that hold audio and the first problem met.
pub type Takes = (Vec<(TrackId, PathBuf)>, Option<String>);

/// Frames each block of a [`LiveTake`] sums up.
pub const LIVE_BLOCK: usize = 128;
/// Blocks a live take keeps per channel at most (three hours at 48 kHz); its file records on.
const LIVE_BLOCKS: usize = 1 << 22;

/// A take while it records, summed up for drawing: per channel, the (min, max) of each
/// [`LIVE_BLOCK`] frames so far.
pub type LiveTake = Vec<Vec<(f32, f32)>>;

const RING_SECONDS: usize = 4;
/// Ring room for recorded bus channels besides the device's (more fit, for less time).
const TAP_ROOM: usize = 8;
/// How long a recording input may go without a callback before it counts as gone.
const STALL: Duration = Duration::from_secs(2);
const GONE: u8 = 1;
const BROKEN: u8 = 2;

/// Why the input can't go on.
#[derive(Debug, Clone, PartialEq)]
pub enum Trouble {
    /// The device went away or stopped sending audio, or its callback hit a bug.
    Device(String),
    /// Writing a take failed (a full disk, say).
    Disk(String),
}

/// What the output callback, the input stream, the writer thread and the [`Recorder`] share.
pub(crate) struct Capture {
    pub(crate) armed: AtomicBool,
    /// Interleaved frames of every input channel, at the session rate (output callback → writer).
    pub(crate) ring: SampleRing,
    /// Samples lost while the ring was full.
    dropped: AtomicUsize,
    /// Input callbacks so far, to tell a device that stopped sending.
    beats: AtomicU64,
    /// Set from the input stream when it can't go on: `GONE` or `BROKEN`.
    fault: AtomicU8,
    /// The first failure writing a take, until it's reported.
    failed: Mutex<Option<String>>,
    /// The timeline position of the take's first frame, once `started`.
    pub(crate) start_pos: AtomicI64,
    /// The input feed's latency when the take's first frame was captured, once `started`.
    pub(crate) start_feed: AtomicU32,
    /// The mix's own delay (PDC, frames) after the take's first block rendered, once `started`.
    pub(crate) start_mix: AtomicU32,
    /// How far behind the timeline each tap ran when the take's first frame was captured
    /// (frames, see [`soundcraft_mix::MixEngine::record_tap`]), once `started`.
    pub(crate) start_tap: [AtomicU32; MAX_TAPS],
    pub(crate) started: AtomicBool,
    /// Pre-record (Options › Recording › Pre-Record): the last seconds of device input heard
    /// while no take records, kept by the output callback ([`History::push`], under `try_lock`:
    /// a block it cannot take is skipped) and put before the take's first frame by the writer.
    pub(crate) history: Mutex<History>,
    /// `history` has room: the callback keeps it.
    pub(crate) pre_on: AtomicBool,
    /// `history`'s size in samples (the UI thread's, to tell a change).
    pre_len: AtomicUsize,
    /// The take started with what `history` holds: the callback leaves it alone until the writer
    /// has put it into the take.
    pub(crate) pre_ready: AtomicBool,
    /// Frames the writer put before the current take's first frame.
    pre_frames: AtomicU64,
    /// Takes armed or disarmed while the take records ([`Recorder::add_take`],
    /// [`Recorder::end_take`]): the UI thread and the writer only, never the audio thread.
    late: Mutex<LateTakes>,
}

/// Takes started or ended while a recording runs (Arm or disarm during a take). The device
/// input's every channel is in each frame of the ring already, so a track armed late only needs
/// its own file, written from the frame the writer is at.
#[derive(Default)]
struct LateTakes {
    /// Opened by the UI thread, for the writer to start at its next frame.
    add: Vec<TakeWriter>,
    /// Tracks whose take ends at the writer's next frame.
    end: Vec<TrackId>,
    /// Each late take's file and the take frame its first frame is (none of the pre-recorded
    /// input in it); a take armed before the first frame is an ordinary one and not listed.
    placed: Vec<(PathBuf, u64)>,
}

/// Pre-record's ring of the device input's last frames ([`Capture::history`]), interleaved.
#[derive(Debug, Default)]
pub(crate) struct History {
    buf: Vec<f32>,
    /// Channels per frame (the input's), set by the first block pushed.
    width: usize,
    /// The frame the next push writes.
    at: usize,
    /// Frames that hold input (at most the ring's).
    filled: usize,
}

impl History {
    fn frames(&self) -> usize {
        self.buf.len().checked_div(self.width).unwrap_or(0)
    }

    /// Keep `n` frames of `input` (one buffer per channel). Never allocates.
    pub(crate) fn push(&mut self, input: &[Vec<f32>], n: usize) {
        if input.is_empty() || self.buf.is_empty() {
            return;
        }
        if self.width != input.len() {
            (self.width, self.at, self.filled) = (input.len(), 0, 0);
        }
        let (cap, width) = (self.frames(), self.width);
        if cap == 0 {
            return;
        }
        for f in 0..n {
            let at = self.at % cap;
            for (c, ch) in input.iter().enumerate() {
                if let Some(o) = self.buf.get_mut(at * width + c) {
                    *o = crate::feed::finite(ch.get(f).copied().unwrap_or(0.0));
                }
            }
            self.at = (at + 1) % cap;
        }
        self.filled = self.filled.saturating_add(n).min(cap);
    }

    /// Append what it holds, oldest first, to `out` as frames `channels` wide (channels past the
    /// input's are silence), and empty it. Returns the frames appended.
    fn drain(&mut self, channels: usize, out: &mut Vec<f32>) -> usize {
        let (cap, width, n) = (self.frames(), self.width, self.filled);
        if cap == 0 || n == 0 || channels == 0 {
            return 0;
        }
        let first = (self.at + cap - n % cap) % cap;
        out.reserve(n.saturating_mul(channels));
        for k in 0..n {
            let f = (first + k) % cap;
            out.extend((0..channels).map(|c| if c < width { self.buf.get(f * width + c).copied().unwrap_or(0.0) } else { 0.0 }));
        }
        self.filled = 0;
        n
    }
}

/// Seconds of input pre-record keeps (Options › Recording › Pre-Record).
pub const PRE_RECORD_SECONDS: u32 = 10;
/// Samples pre-record keeps at most, whatever the channel count (128 MB of f32).
const PRE_RECORD_MAX_SAMPLES: usize = 1 << 25;

/// Bus-input tracks recorded at most at once (internal recording).
pub const MAX_TAPS: usize = 32;

/// What a recording frame holds: `device` input channels, then each tapped track's channels in
/// order (a bus-input track's tap, [`soundcraft_mix::MixEngine::record_tap`]). Fixed size, so the
/// audio thread takes it without allocating.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Taps {
    pub device: usize,
    taps: [(TrackId, usize); MAX_TAPS],
    n: usize,
}

impl Taps {
    pub fn new(device: usize, list: &[(TrackId, usize)]) -> Taps {
        let mut t = Taps { device, ..Taps::default() };
        for (slot, x) in t.taps.iter_mut().zip(list) {
            *slot = (x.0, x.1.min(64));
        }
        t.n = list.len().min(MAX_TAPS);
        t
    }

    pub fn list(&self) -> &[(TrackId, usize)] {
        self.taps.get(..self.n).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The frame's width: the device channels and every tap's.
    pub fn width(&self) -> usize {
        self.list().iter().fold(self.device, |w, (_, c)| w.saturating_add(*c))
    }

    /// Which tap (index) and which of its channels the `k`-th channel after the device's is.
    pub(crate) fn locate(&self, mut k: usize) -> (usize, usize) {
        for (i, (_, c)) in self.list().iter().enumerate() {
            if k < *c {
                return (i, k);
            }
            k -= c;
        }
        (MAX_TAPS, 0)
    }
}

impl Capture {
    /// A capture with room for a few seconds of `channels` at `rate`, and of a few recorded
    /// busses besides ([`Taps`]).
    pub(crate) fn for_rate(rate: u32, channels: usize) -> Arc<Capture> {
        let width = channels.saturating_add(TAP_ROOM).max(1);
        Arc::new(Capture::new(SampleRing::new((rate as usize).saturating_mul(width * RING_SECONDS).min(1 << 25))))
    }

    fn new(ring: SampleRing) -> Capture {
        let (beats, fault, failed) = (AtomicU64::new(0), AtomicU8::new(0), Mutex::new(None));
        Capture {
            armed: AtomicBool::new(false),
            ring,
            dropped: AtomicUsize::new(0),
            beats,
            fault,
            failed,
            start_pos: AtomicI64::new(0),
            start_feed: AtomicU32::new(0),
            start_mix: AtomicU32::new(0),
            start_tap: std::array::from_fn(|_| AtomicU32::new(0)),
            started: AtomicBool::new(false),
            history: Mutex::new(History::default()),
            pre_on: AtomicBool::new(false),
            pre_len: AtomicUsize::new(0),
            pre_ready: AtomicBool::new(false),
            pre_frames: AtomicU64::new(0),
            late: Mutex::new(LateTakes::default()),
        }
    }

    /// Run the input callback's `work`. After a panic in it, the input is broken and does
    /// nothing more.
    fn guarded(&self, work: impl FnOnce()) {
        self.beats.fetch_add(1, Ordering::Relaxed);
        if self.fault.load(Ordering::Relaxed) != BROKEN && std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
            self.fault.store(BROKEN, Ordering::Relaxed);
        }
    }

    /// Queue `len` samples (`sample(i)` is the i-th) of a recording block. Samples that don't fit
    /// are owed (`owed`) and queued as silence before the next block, so the take keeps its
    /// timing. The output callback's work; it never allocates.
    pub(crate) fn capture_with(&self, len: usize, sample: impl Fn(usize) -> f32, owed: &mut usize) {
        *owed -= self.ring.push(*owed, |_| 0.0);
        let kept = if *owed == 0 { self.ring.push(len, |i| crate::feed::finite(sample(i))) } else { 0 };
        *owed = owed.saturating_add(len - kept);
        self.dropped.fetch_add(len - kept, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn capture(&self, data: &[f32], owed: &mut usize) {
        self.capture_with(data.len(), |i| data.get(i).copied().unwrap_or(0.0), owed);
    }
}

pub struct Recorder {
    capture: Arc<Capture>,
    /// Takes the frame holds besides the device's channels ([`Taps`]), set by [`Recorder::arm`].
    taps: usize,
    /// No input device: it records busses only (internal recording), so no input callback beats.
    internal: bool,
    /// The input stream when the recorder owns it (split); a duplex input is the player's.
    #[cfg(not(target_os = "freebsd"))]
    _stream: Option<cpal::Stream>,
    pub device_name: String,
    /// The device's channel names, in channel order, when it gives them (JACK's ports); empty
    /// otherwise.
    pub channel_names: Vec<String>,
    /// The input device's rate.
    pub device_rate: u32,
    /// The rate takes are written at: the session's (they are captured from the mix's input).
    pub sample_rate: u32,
    pub channels: usize,
    /// The take writer, and what each track's take holds so far.
    writer: Option<(Writer, Vec<(TrackId, Arc<Mutex<LiveTake>>)>)>,
    /// How the takes are written (for a take armed late), and the tracks whose take records now.
    format: TakeFormat,
    recording: Vec<TrackId>,
    /// Tests: no writer thread; [`Recorder::pump`] writes a step at a time.
    #[cfg(test)]
    stepped: bool,
    /// The callback count last seen, and since when.
    seen: (u64, Instant),
}

/// How the takes are written.
enum Writer {
    /// A thread streams the ring into the take files.
    Thread(std::thread::JoinHandle<Takes>),
    /// No thread (tests): [`Recorder::pump`] runs one step at a time.
    #[cfg(test)]
    Steps(Steps),
}

/// The writer's work between two looks at the ring: whole frames into every take.
struct Steps {
    takes: Vec<TakeWriter>,
    /// Takes disarmed while recording: finished, header written.
    ended: Vec<TakeWriter>,
    /// Frames of the take written so far (after the pre-recorded input).
    frames: u64,
    /// Popped samples not yet a whole frame.
    block: Vec<f32>,
    channels: usize,
    reported: bool,
    /// The pre-recorded input went into the takes (or there was none).
    pre_done: bool,
}

impl Steps {
    fn new(takes: Vec<TakeWriter>, channels: usize) -> Steps {
        Steps { takes, ended: Vec::new(), frames: 0, block: Vec::new(), channels, reported: false, pre_done: false }
    }

    /// Once the take has started, what pre-record kept goes first into every take, once. The
    /// callback sets `pre_ready` before it queues the take's first frame, so a pop that got any
    /// frame sees it.
    fn put_pre_record(&mut self, shared: &Capture) {
        if self.pre_done || !shared.pre_ready.load(Ordering::Acquire) {
            return;
        }
        self.pre_done = true;
        let mut pre = Vec::new();
        let frames = shared.history.lock().unwrap_or_else(PoisonError::into_inner).drain(self.channels, &mut pre);
        shared.pre_ready.store(false, Ordering::Release);
        for t in &mut self.takes {
            t.pre = t.frames.saturating_add(frames as u64);
            t.write(&pre, self.channels);
        }
        shared.pre_frames.store(frames as u64, Ordering::Release);
    }

    /// Takes armed since the last look start at the frame the writer is at (a take armed before
    /// the first frame is an ordinary one: it gets the pre-recorded input too); takes disarmed end
    /// there, their header written.
    fn follow_arms(&mut self, shared: &Capture) {
        let mut late = shared.late.lock().unwrap_or_else(PoisonError::into_inner);
        for id in std::mem::take(&mut late.end) {
            if let Some(at) = self.takes.iter().rposition(|t| t.id == id) {
                let mut t = self.takes.remove(at);
                t.write_header();
                self.ended.push(t);
            }
        }
        let is_late = self.frames > 0 || self.pre_done;
        for t in std::mem::take(&mut late.add) {
            if is_late {
                late.placed.push((t.path.clone(), self.frames));
            }
            self.takes.push(t);
        }
    }

    /// Write what the ring holds; the first failure is reported to `shared` once.
    fn run(&mut self, shared: &Capture) {
        // The ring holds interleaved samples and a pop can end mid-frame: write whole frames and
        // keep the remainder for the next pop, so channels never shift.
        shared.ring.pop_all(&mut self.block);
        self.put_pre_record(shared);
        self.follow_arms(shared);
        let whole = self.block.len() - self.block.len() % self.channels.max(1);
        let (block, channels) = (&self.block[..whole], self.channels);
        self.takes.iter_mut().for_each(|t| t.write(block, channels));
        self.frames = self.frames.saturating_add((whole / channels.max(1)) as u64);
        self.block.drain(..whole);
        if !self.reported
            && let Some(e) = self.takes.iter().find_map(|t| t.error.clone())
        {
            *shared.failed.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
            self.reported = true;
        }
    }

    /// The takes that hold audio and the first problem met; a file holding nothing is removed.
    fn finish(self) -> Takes {
        let (mut done, mut problem) = (Vec::new(), None);
        for mut t in self.ended.into_iter().chain(self.takes) {
            t.write_header();
            problem = problem.or(t.error);
            if t.frames == 0 {
                drop(t.file);
                let _ = std::fs::remove_file(&t.path);
            } else {
                done.push((t.id, t.path));
            }
        }
        (done, problem)
    }
}

/// The input stream: device samples are converted to f32 in a scratch buffer allocated here (so
/// the callback never allocates), then pushed into the feed. It does not capture: the output
/// callback does, from the frames the mix sees.
#[cfg(not(target_os = "freebsd"))]
fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    capture: &Arc<Capture>,
    feed: &Arc<InputFeed>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::Sample;
    use cpal::traits::DeviceTrait;
    let (sh, errors) = (Arc::clone(capture), Arc::clone(capture));
    let feed = Arc::clone(feed);
    let frame = usize::from(config.channels).max(1);
    let mut scratch = vec![0.0f32; 8192 * frame];
    let chunk = scratch.len();
    crate::guarded_build(move || {
        device.build_input_stream(
            *config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                crate::mark_audio_thread();
                // Realtime priority for the capture thread too, asked for once.
                soundcraft_mix::rt::promote_capture_thread();
                // The reported input latency: from capture until this callback.
                let at = info.timestamp();
                if let Some(d) = at.callback.checked_duration_since(at.capture) {
                    feed.set_input_latency(d);
                }
                sh.guarded(|| {
                    for part in data.chunks(chunk) {
                        let Some(s) = scratch.get_mut(..part.len()) else { return };
                        for (o, x) in s.iter_mut().zip(part) {
                            *o = f32::from_sample(*x);
                        }
                        feed.push(s);
                    }
                });
            },
            move |e| {
                // Possibly the audio thread (see `crate::mark_audio_thread`).
                crate::mark_audio_thread();
                if e.kind() == cpal::ErrorKind::DeviceNotAvailable {
                    let _ = errors.fault.compare_exchange(0, GONE, Ordering::Relaxed, Ordering::Relaxed);
                }
                log::warn!("input stream error: {e}");
            },
            None,
        )
    })
}

/// An input stream, built but not started, and what it feeds.
pub(crate) struct InputStream {
    #[cfg(not(target_os = "freebsd"))]
    pub(crate) stream: cpal::Stream,
    pub(crate) feed: Arc<InputFeed>,
    pub(crate) capture: Arc<Capture>,
    pub(crate) name: String,
    pub(crate) rate: u32,
    pub(crate) channels: usize,
}

/// Build an input stream on `device` at `rate` when the device offers it (else its default), with
/// `buffer` frames when it allows (else its default). `shape` gives the stream another name and
/// channel count (JACK's and PipeWire's inputs, connected to their sources once they run). It feeds `make_feed(channels, device
/// rate)`, and its capture holds frames at `session_rate`. Integer devices (ASIO drivers usually
/// are) are converted to f32.
#[cfg(not(target_os = "freebsd"))]
pub(crate) fn build_input_stream(
    device: &cpal::Device,
    rate: Option<u32>,
    buffer: Option<u32>,
    session_rate: u32,
    shape: Option<(String, u16)>,
    make_feed: impl Fn(usize, u32) -> Arc<InputFeed>,
) -> Result<InputStream, String> {
    use cpal::traits::DeviceTrait;
    let name = shape.as_ref().map(|s| s.0.clone()).unwrap_or_else(|| device.description().map_or_else(|_| "Input".into(), |d| d.name().to_string()));
    // At the chosen rate when the device offers it, so input and output run together: the
    // best format first, then the most channels.
    let at_rate = rate.and_then(|r| {
        device
            .supported_input_configs()
            .ok()?
            .filter(|c| crate::devices::format_rank(c.sample_format()).is_some() && c.min_sample_rate() <= r && c.max_sample_rate() >= r)
            .min_by_key(|c| (crate::devices::format_rank(c.sample_format()), std::cmp::Reverse(c.channels())))
            .map(|c| c.with_sample_rate(r))
    });
    let cfg = match at_rate {
        Some(c) => c,
        None => device.default_input_config().map_err(|e| e.to_string())?,
    };
    let format = cfg.sample_format();
    if crate::devices::format_rank(format).is_none() {
        return Err(format!("unsupported input sample format {format:?}"));
    }
    let mut config = cfg.config();
    config.buffer_size = crate::devices::buffer_size(buffer, cfg.buffer_size());
    if let Some((_, channels)) = shape {
        config.channels = channels.clamp(1, 64);
    }
    let channels = usize::from(config.channels).max(1);
    let feed = make_feed(channels, config.sample_rate);
    let capture = Capture::for_rate(session_rate, channels);
    let build = |config: &cpal::StreamConfig| match format {
        cpal::SampleFormat::I32 => build_input::<i32>(device, config, &capture, &feed),
        cpal::SampleFormat::I16 => build_input::<i16>(device, config, &capture, &feed),
        _ => build_input::<f32>(device, config, &capture, &feed),
    };
    // The chosen buffer size first; if the device refuses it, its default.
    let stream = match build(&config) {
        Ok(s) => s,
        Err(e) if config.buffer_size != cpal::BufferSize::Default => {
            log::warn!("cannot open the input with buffer {:?}: {e}; using the device default", config.buffer_size);
            config.buffer_size = cpal::BufferSize::Default;
            build(&config).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    Ok(InputStream { stream, feed, capture, name, rate: config.sample_rate, channels })
}

#[cfg(target_os = "freebsd")]
pub(crate) fn build_input_stream(
    _device: &cpal::Device,
    _rate: Option<u32>,
    _buffer: Option<u32>,
    _session_rate: u32,
    _shape: Option<(String, u16)>,
    _make_feed: impl Fn(usize, u32) -> Arc<InputFeed>,
) -> Result<InputStream, String> {
    Err("recording is not supported on this platform yet".into())
}

/// An input fed by hand, for tests of recording in other crates ([`Recorder::synthetic`]): each
/// [`SyntheticInput::push`] is one input callback. Not for use outside tests.
#[doc(hidden)]
pub struct SyntheticInput {
    feed: Arc<InputFeed>,
    capture: Arc<Capture>,
}

impl SyntheticInput {
    /// One input callback holding `interleaved` frames.
    pub fn push(&self, interleaved: &[f32]) {
        self.capture.guarded(|| self.feed.push(interleaved));
    }

    /// The device goes away (as an unplugged interface's stream reports it).
    pub fn unplug(&self) {
        self.capture.fault.store(GONE, Ordering::Relaxed);
    }
}

impl Recorder {
    /// Tests in other crates: a recorder on an input of `channels` fed by hand
    /// ([`SyntheticInput`]), attached to `player` as an input on the output's clock is. Not for
    /// use outside tests.
    #[doc(hidden)]
    pub fn synthetic(player: &crate::Player, channels: usize) -> (Recorder, SyntheticInput) {
        let (rate, channels) = (player.session_rate(), channels.max(1));
        let feed = InputFeed::fixed(channels, player.max_block());
        let capture = Capture::for_rate(rate, channels);
        player.attach_input(Arc::clone(&feed), Arc::clone(&capture));
        let r = Recorder::attached(Arc::clone(&capture), "Synthetic input".into(), rate, rate, channels);
        (r, SyntheticInput { feed, capture })
    }

    /// A recorder over `capture`, whose input is `device_name` at `device_rate` with `channels`.
    pub(crate) fn attached(capture: Arc<Capture>, device_name: String, device_rate: u32, session_rate: u32, channels: usize) -> Recorder {
        Recorder {
            capture,
            #[cfg(not(target_os = "freebsd"))]
            _stream: None,
            device_name,
            channel_names: Vec::new(),
            device_rate,
            sample_rate: session_rate.max(1),
            channels: channels.max(1),
            taps: 0,
            internal: false,
            writer: None,
            format: TakeFormat::default(),
            recording: Vec::new(),
            #[cfg(test)]
            stepped: false,
            seen: (0, Instant::now()),
        }
    }

    /// Tests: writes what the input captured since the last call into the takes of a stepped
    /// recorder (a thread does it otherwise).
    #[cfg(test)]
    fn pump(&mut self) {
        if let Some((Writer::Steps(s), _)) = &mut self.writer {
            s.run(&self.capture);
        }
    }

    /// A recorder with no input device ([`crate::Player::internal_recorder`]): its takes record
    /// busses only, so `channels` is 0.
    pub(crate) fn internal(capture: Arc<Capture>, session_rate: u32) -> Recorder {
        let mut r = Recorder::attached(capture, "Internal".into(), session_rate, session_rate, 1);
        (r.channels, r.internal) = (0, true);
        r
    }

    /// Whether it has no input device (it records busses only).
    pub fn is_internal(&self) -> bool {
        self.internal
    }

    /// How far behind the timeline tap `k` ([`Taps`]) ran at the current take's first frame
    /// (frames), once it has one: its take is placed earlier by this.
    pub fn start_tap_latency(&self, k: usize) -> Option<u32> {
        let at = self.capture.start_tap.get(k)?;
        self.capture.started.load(Ordering::Acquire).then(|| at.load(Ordering::Relaxed))
    }

    /// Open the input device `audio` chooses (else the default) as its own stream (split: not the
    /// player's device), at the chosen rate and buffer size when the device offers them. Its
    /// input reaches `player` through a drift-correcting feed. Errors when there is no input
    /// (the caller shows a message).
    #[cfg(not(target_os = "freebsd"))]
    pub fn open_with(audio: &crate::AudioConfig, player: &crate::Player) -> Result<Recorder, String> {
        use cpal::traits::StreamTrait;
        if audio.all_sources() {
            return Recorder::open_all_sources(audio, player);
        }
        let host = crate::devices::host(audio);
        let device = crate::devices::device(&host, audio.input.as_deref(), false).ok_or("no audio input device")?;
        let (session_rate, max_block) = (player.session_rate(), player.max_block);
        let route = crate::jack_input_route(&host, &device, audio.input.as_deref());
        let shape = route.as_ref().map(|r| (r.group.name.clone(), r.channels()));
        let input = build_input_stream(&device, audio.sample_rate, audio.buffer_size, session_rate, shape, |ch, rate| {
            InputFeed::drift(ch, rate, session_rate, max_block)
        })?;
        input.stream.play().map_err(|e| e.to_string())?;
        if let Some(r) = &route {
            r.connect();
        }
        player.attach_input(Arc::clone(&input.feed), Arc::clone(&input.capture));
        let mut r = Recorder::attached(input.capture, input.name, input.rate, session_rate, input.channels);
        r.channel_names = route.as_ref().map(crate::jack::Route::channel_names).unwrap_or_default();
        r._stream = Some(input.stream);
        Ok(r)
    }

    /// PipeWire's "All sources" (see `pw`): one stream, one channel per chosen source, linked to
    /// them once it runs.
    #[cfg(not(target_os = "freebsd"))]
    fn open_all_sources(audio: &crate::AudioConfig, player: &crate::Player) -> Result<Recorder, String> {
        use cpal::traits::StreamTrait;
        let keys: Vec<String> = crate::pw::effective_sources(&audio.input_sources).into_iter().take(64).collect();
        if keys.is_empty() {
            return Err("PipeWire has no source to record from".into());
        }
        let (_host, device) = crate::devices::all_sources_input().ok_or("PipeWire is not running")?;
        let (session_rate, max_block) = (player.session_rate(), player.max_block);
        let shape = Some((crate::pw::ALL_SOURCES.to_string(), u16::try_from(keys.len()).unwrap_or(64)));
        let input = build_input_stream(&device, audio.sample_rate, None, session_rate, shape, |ch, rate| {
            InputFeed::drift(ch, rate, session_rate, max_block)
        })?;
        input.stream.play().map_err(|e| e.to_string())?;
        match crate::pw::connect(&keys) {
            Ok(n) => log::info!("PipeWire: {n} of {} source(s) linked to the input", keys.len()),
            Err(e) => log::warn!("PipeWire: the input is not linked: {e}"),
        }
        player.attach_input(Arc::clone(&input.feed), Arc::clone(&input.capture));
        let mut r = Recorder::attached(input.capture, input.name, input.rate, session_rate, input.channels);
        r.channel_names = crate::pw::labels(&keys, &crate::pw::sources());
        r._stream = Some(input.stream);
        Ok(r)
    }

    #[cfg(target_os = "freebsd")]
    pub fn open_with(_audio: &crate::AudioConfig, _player: &crate::Player) -> Result<Recorder, String> {
        Err("recording is not supported on this platform yet".into())
    }

    /// Create each take's file and start recording into them: a track's take records the given
    /// input channels into a new file.
    pub fn arm(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>) -> Result<(), String> {
        self.arm_with_taps(takes, 0)
    }

    /// [`Recorder::arm`] with `taps` channels of recorded busses after the device's in each frame
    /// (the player told the same with [`crate::Player::set_record_taps`]).
    pub fn arm_with_taps(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>, taps: usize) -> Result<(), String> {
        self.arm_as(takes, taps, &TakeFormat::default())
    }

    /// [`Recorder::arm_with_taps`] writing the take files as `format` says.
    pub fn arm_as(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>, taps: usize, format: &TakeFormat) -> Result<(), String> {
        if self.writer.is_some() {
            return Err("already recording".into());
        }
        let rate = self.sample_rate;
        let takes =
            takes.into_iter().map(|(id, inputs, path)| TakeWriter::create_as(id, inputs, path, rate, format)).collect::<Result<Vec<_>, _>>()?;
        let live = takes.iter().map(|t| (t.id, Arc::clone(&t.live))).collect();
        self.recording = takes.iter().map(|t| t.id).collect();
        self.format = format.clone();
        *self.capture.late.lock().unwrap_or_else(PoisonError::into_inner) = LateTakes::default();
        self.capture.ring.clear();
        *self.capture.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.capture.started.store(false, Ordering::Release);
        self.capture.pre_ready.store(false, Ordering::Release);
        self.capture.pre_frames.store(0, Ordering::Release);
        self.capture.armed.store(true, Ordering::Release);
        self.taps = taps;
        #[cfg(test)]
        if self.stepped {
            self.writer = Some((Writer::Steps(Steps::new(takes, self.width())), live));
            return Ok(());
        }
        let (capture, channels) = (Arc::clone(&self.capture), self.width());
        match std::thread::Builder::new().name("take writer".into()).spawn(move || write_takes(&capture, takes, channels)) {
            Ok(w) => self.writer = Some((Writer::Thread(w), live)),
            Err(e) => {
                self.capture.armed.store(false, Ordering::Release);
                return Err(e.to_string());
            }
        }
        Ok(())
    }

    /// Pre-record: keep the last `seconds` of input heard while no take records, and start each
    /// take with them (0 turns it off and frees the memory). Cheap when nothing changes, so it
    /// can be called every frame.
    pub fn set_pre_record(&self, seconds: u32) {
        let want = (self.sample_rate as usize).saturating_mul(seconds as usize).saturating_mul(self.channels).min(PRE_RECORD_MAX_SAMPLES);
        let want = if self.internal { 0 } else { want };
        if self.capture.pre_len.load(Ordering::Relaxed) == want {
            return;
        }
        // Made here, swapped under the lock, the old one freed here: the callback never waits.
        let mut fresh = History { buf: vec![0.0; want], ..History::default() };
        {
            let mut h = self.capture.history.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::swap(&mut *h, &mut fresh);
        }
        self.capture.pre_len.store(want, Ordering::Relaxed);
        self.capture.pre_on.store(want > 0, Ordering::Release);
    }

    /// Frames of pre-recorded input before the current (or last) take's first frame: the take
    /// file's frame `k` belongs at `start_pos + k - pre_frames` (less the input latency).
    pub fn pre_frames(&self) -> u64 {
        self.capture.pre_frames.load(Ordering::Acquire)
    }

    /// The timeline position of the current take's first frame, once it has one.
    pub fn start_pos(&self) -> Option<Samples> {
        self.capture.started.load(Ordering::Acquire).then(|| self.capture.start_pos.load(Ordering::Acquire))
    }

    /// The input feed's latency (frames at the session rate) when the current take's first frame
    /// was captured, once it has one. Read there, live, because a drift feed's latency moves:
    /// the record offset for this take uses it rather than a figure from when the input opened.
    pub fn start_feed_latency(&self) -> Option<u32> {
        self.capture.started.load(Ordering::Acquire).then(|| self.capture.start_feed.load(Ordering::Relaxed))
    }

    /// The mix's own delay (frames) while the current take's first block played, once it has
    /// one. Read there, live: it drops while low-latency monitoring bypasses latent plugins.
    pub fn start_mix_latency(&self) -> Option<u32> {
        self.capture.started.load(Ordering::Acquire).then(|| self.capture.start_mix.load(Ordering::Relaxed))
    }

    /// What the take recording onto `track` holds so far (its latest, armed again during the
    /// take).
    pub fn live_take(&self, track: TrackId) -> Option<MutexGuard<'_, LiveTake>> {
        let (_, take) = self.writer.as_ref()?.1.iter().rev().find(|(id, _)| *id == track)?;
        Some(take.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Whether `track` has a take recording now.
    pub fn records(&self, track: TrackId) -> bool {
        self.writer.is_some() && self.recording.contains(&track)
    }

    /// The tracks whose take records now.
    pub fn recording_tracks(&self) -> &[TrackId] {
        if self.writer.is_some() { &self.recording } else { &[] }
    }

    /// Arm during a take: a take onto `track` of the frame's channels `inputs` (the device's: a
    /// recorded bus's channels are only in the frame when armed at Record) into a new file at
    /// `path`, from the frame the writer is at. Errors when nothing records, a channel is not in
    /// the frame, or the file cannot be made.
    pub fn add_take(&mut self, track: TrackId, inputs: Vec<usize>, path: PathBuf) -> Result<(), String> {
        let Some((_, live)) = &mut self.writer else { return Err("not recording".into()) };
        if inputs.iter().any(|&i| i >= self.channels && i != usize::MAX) || self.internal {
            return Err("a track armed during a take records the device input only".into());
        }
        let t = TakeWriter::create_as(track, inputs, path, self.sample_rate, &self.format)?;
        live.push((track, Arc::clone(&t.live)));
        self.capture.late.lock().unwrap_or_else(PoisonError::into_inner).add.push(t);
        self.recording.push(track);
        Ok(())
    }

    /// Disarm during a take: `track`'s take ends at the frame the writer is at (its file keeps
    /// what it recorded).
    pub fn end_take(&mut self, track: TrackId) {
        if !self.records(track) {
            return;
        }
        self.recording.retain(|t| *t != track);
        self.capture.late.lock().unwrap_or_else(PoisonError::into_inner).end.push(track);
    }

    /// For a take armed during the recording (by its file): the take frame its first frame is,
    /// once the writer has started it. `None` for a take armed at Record.
    pub fn late_start(&self, path: &std::path::Path) -> Option<u64> {
        let late = self.capture.late.lock().unwrap_or_else(PoisonError::into_inner);
        late.placed.iter().find(|(p, _)| p == path).map(|(_, f)| *f)
    }

    /// Why the input can't go on, if it can't: the device went away or, while recording, stopped
    /// sending audio; its callback hit a bug; or writing a take failed (reported once).
    pub fn trouble(&mut self) -> Option<Trouble> {
        if let Some(e) = self.capture.failed.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Some(Trouble::Disk(e));
        }
        if self.internal {
            return None;
        }
        let name = &self.device_name;
        match self.capture.fault.load(Ordering::Relaxed) {
            GONE => return Some(Trouble::Device(format!("“{name}” was disconnected"))),
            BROKEN => return Some(Trouble::Device("the audio input hit an internal error (see the log)".into())),
            _ => {}
        }
        self.stalled()
    }

    fn stalled(&mut self) -> Option<Trouble> {
        let name = &self.device_name;
        let beats = self.capture.beats.load(Ordering::Relaxed);
        if beats != self.seen.0 || !self.capture.armed.load(Ordering::Relaxed) {
            self.seen = (beats, Instant::now());
        }
        (self.seen.1.elapsed() >= STALL).then(|| Trouble::Device(format!("“{name}” stopped sending audio")))
    }

    /// Input the current take has lost to a slow disk so far, in milliseconds.
    /// Input callbacks so far.
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn beats(&self) -> u64 {
        self.capture.beats.load(Ordering::Relaxed)
    }

    /// A recording frame's width: the device's channels and the recorded busses'.
    fn width(&self) -> usize {
        self.channels.saturating_add(self.taps).max(1)
    }

    pub fn lost_ms(&self) -> u64 {
        let frames = self.capture.dropped.load(Ordering::Relaxed) / self.width();
        (frames as u64).saturating_mul(1000) / u64::from(self.sample_rate.max(1))
    }

    pub fn finish(&mut self) -> Takes {
        self.capture.armed.store(false, Ordering::Release);
        let (takes, problem) = match self.writer.take() {
            Some((Writer::Thread(w), _)) => w.join().unwrap_or_default(),
            #[cfg(test)]
            Some((Writer::Steps(mut s), _)) => {
                s.run(&self.capture);
                s.finish()
            }
            None => Takes::default(),
        };
        // A take too short for the writer to see: what pre-record kept is the next take's.
        self.capture.pre_ready.store(false, Ordering::Release);
        let lost = self.capture.dropped.swap(0, Ordering::Relaxed) / self.width();
        let slow =
            (lost > 0).then(|| format!("{} ms of input lost: the disk was too slow", lost.saturating_mul(1000) / self.sample_rate.max(1) as usize));
        (takes, problem.or(slow))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.finish();
    }
}

fn write_takes(shared: &Capture, takes: Vec<TakeWriter>, channels: usize) -> Takes {
    let (mut steps, mut header_written) = (Steps::new(takes, channels), Instant::now());
    loop {
        let stopping = !shared.armed.load(Ordering::Acquire);
        steps.run(shared);
        if stopping {
            break;
        }
        if header_written.elapsed() >= Duration::from_secs(1) {
            header_written = Instant::now();
            steps.takes.iter_mut().for_each(TakeWriter::write_header);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    steps.finish()
}

/// One take's file. After an error it stops writing; its header keeps covering what was written.
struct TakeWriter {
    id: TrackId,
    inputs: Vec<usize>,
    path: PathBuf,
    file: File,
    rate: u32,
    format: TakeFormat,
    frames: u64,
    /// Frames of pre-recorded input at the file's start (not drawn: the live take starts at the
    /// take's first frame).
    pre: u64,
    error: Option<String>,
    live: Arc<Mutex<LiveTake>>,
}

impl TakeWriter {
    #[cfg(test)]
    fn create(id: TrackId, inputs: Vec<usize>, path: PathBuf, rate: u32) -> Result<TakeWriter, String> {
        TakeWriter::create_as(id, inputs, path, rate, &TakeFormat::default())
    }

    fn create_as(id: TrackId, inputs: Vec<usize>, path: PathBuf, rate: u32, format: &TakeFormat) -> Result<TakeWriter, String> {
        let file = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| File::create_new(&path));
        let file = file.map_err(|e| format!("{}: {e}", path.display()))?;
        TakeWriter::on(id, inputs, path, file, rate, format)
    }

    fn on(id: TrackId, inputs: Vec<usize>, path: PathBuf, file: File, rate: u32, format: &TakeFormat) -> Result<TakeWriter, String> {
        let live = Arc::new(Mutex::new(vec![Vec::new(); inputs.len()]));
        let mut format = format.clone();
        if let Some(b) = &mut format.bwf {
            b.description = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        let mut take = TakeWriter { id, inputs, path, file, rate, format, frames: 0, pre: 0, error: None, live };
        take.write_header();
        take.error.take().map_or(Ok(take), Err)
    }

    /// Append this take's channels of `block` (interleaved, `channels` wide).
    fn write(&mut self, block: &[f32], channels: usize) {
        let channels = channels.max(1);
        if self.error.is_some() {
            return;
        }
        let (fmt, depth) = (self.format.format, self.format.depth);
        let per = soundcraft_audio_io::take_sample_bytes(depth);
        let mut bytes = Vec::with_capacity((block.len() / channels).saturating_mul(self.inputs.len()).saturating_mul(per));
        for f in block.chunks_exact(channels) {
            for &i in &self.inputs {
                soundcraft_audio_io::take_sample(fmt, depth, f.get(i).copied().unwrap_or(0.0), &mut bytes);
            }
        }
        match self.file.write_all(&bytes) {
            Ok(()) => {
                self.summarize(block, channels);
                self.frames += (block.len() / channels) as u64;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Add this take's channels of `block` to its live take.
    fn summarize(&self, block: &[f32], channels: usize) {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        for (f, n) in block.chunks_exact(channels).zip(self.frames..) {
            // Pre-recorded frames come before `pre`: none are drawn.
            let Some(n) = n.checked_sub(self.pre) else { continue };
            let at = (n / LIVE_BLOCK as u64) as usize;
            for (blocks, &i) in live.iter_mut().zip(&self.inputs) {
                let v = f.get(i).copied().unwrap_or(0.0);
                match blocks.get_mut(at) {
                    Some(b) => *b = (b.0.min(v), b.1.max(v)),
                    None if at < LIVE_BLOCKS => blocks.push((v, v)),
                    None => {}
                }
            }
        }
    }

    fn write_header(&mut self) {
        let f = &self.format;
        let header = match soundcraft_audio_io::take_header(f.format, self.inputs.len(), self.rate, f.depth, f.bwf.as_ref(), self.frames) {
            Ok(h) => h,
            Err(e) => return self.fail(e),
        };
        let f = &mut self.file;
        if let Err(e) = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&header)).and_then(|()| f.seek(SeekFrom::End(0))) {
            self.fail(e);
        }
    }

    fn fail(&mut self, e: impl std::fmt::Display) {
        self.error.get_or_insert_with(|| format!("{}: {e}", self.path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(path: &PathBuf) -> Vec<Vec<f32>> {
        soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), None).unwrap().1.channels
    }

    #[test]
    fn a_full_ring_owes_the_lost_samples_as_silence() {
        let shared = Capture::new(SampleRing::new(8));
        let (mut owed, mut out) = (0, Vec::new());
        shared.capture(&[1.0; 6], &mut owed);
        shared.capture(&[2.0; 4], &mut owed);
        shared.ring.pop_all(&mut out);
        shared.capture(&[3.0; 2], &mut owed);
        shared.ring.pop_all(&mut out);
        assert_eq!(out, [vec![1.0; 6], vec![2.0; 2], vec![0.0; 2], vec![3.0; 2]].concat());
        assert_eq!((owed, shared.dropped.load(Ordering::Relaxed)), (0, 2));
    }

    #[test]
    fn nan_and_infinite_input_records_as_silence() {
        let shared = Capture::new(SampleRing::new(8));
        let (mut owed, mut out) = (0, Vec::new());
        shared.capture(&[0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5], &mut owed);
        shared.ring.pop_all(&mut out);
        assert_eq!(out, [0.5, 0.0, 0.0, 0.0, -0.5]);
    }

    fn recorder(shared: &Arc<Capture>) -> Recorder {
        Recorder::attached(Arc::clone(shared), "Mic In".into(), 48_000, 48_000, 1)
    }

    #[test]
    fn a_bug_a_stall_or_a_lost_device_is_trouble() {
        let shared = Arc::new(Capture::new(SampleRing::new(64)));
        let mut r = recorder(&shared);
        shared.armed.store(true, Ordering::Release);
        assert_eq!(r.trouble(), None);
        r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
        assert_eq!(r.trouble(), Some(Trouble::Device("“Mic In” stopped sending audio".into())));
        shared.guarded(|| {});
        assert_eq!(r.trouble(), None, "it sent again");
        shared.armed.store(false, Ordering::Release);
        r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
        assert_eq!(r.trouble(), None, "a quiet input that isn't recording is fine");
        shared.guarded(|| std::panic::panic_any("a bug in the input"));
        let mut ran = false;
        shared.guarded(|| ran = true);
        assert!(!ran);
        assert_eq!(r.trouble(), Some(Trouble::Device("the audio input hit an internal error (see the log)".into())));
        let gone = Arc::new(Capture::new(SampleRing::new(64)));
        gone.fault.store(GONE, Ordering::Relaxed);
        assert_eq!(recorder(&gone).trouble(), Some(Trouble::Device("“Mic In” was disconnected".into())));
    }

    #[test]
    fn a_failed_write_is_trouble_while_still_recording() {
        let dir = std::env::temp_dir().join(format!("soundcraft-failed-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut take = TakeWriter::create(TrackId(1), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
        take.file = File::open(&take.path).unwrap();
        let shared = Arc::new(Capture::new(SampleRing::new(1 << 12)));
        shared.armed.store(true, Ordering::Release);
        let mut r = recorder(&shared);
        let sh = Arc::clone(&shared);
        let writer = std::thread::spawn(move || write_takes(&sh, vec![take], 1));
        shared.capture(&[0.5; 480], &mut 0);
        let trouble = (0..300).find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            r.trouble()
        });
        assert!(matches!(&trouble, Some(Trouble::Disk(e)) if e.contains("Vox_01.wav")), "{trouble:?}");
        assert_eq!(r.trouble(), None, "reported once");
        shared.armed.store(false, Ordering::Release);
        assert!(writer.join().unwrap().1.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn takes_stream_into_their_files() {
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gtr = TakeWriter::create(TrackId(1), vec![1, 2], dir.join("Gtr_01.wav"), 48_000).unwrap();
        let mut vox = TakeWriter::create(TrackId(2), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
        assert!(TakeWriter::create(TrackId(3), vec![0], dir.join("Vox_01.wav"), 48_000).is_err(), "never overwrites a file");
        vox.write(&[0.5; 300], 3);
        vox.write_header();
        vox.write(&[0.25; 150], 3);
        assert_eq!(decode(&dir.join("Vox_01.wav"))[0].len(), 100, "the header covers what it has seen");

        let shared = Arc::new(Capture::new(SampleRing::new(1 << 16)));
        shared.armed.store(true, Ordering::Release);
        let sh = Arc::clone(&shared);
        let writer = std::thread::spawn(move || write_takes(&sh, vec![gtr, vox], 3));
        let input: Vec<f32> = (0..3 * 4_800).map(|i| i as f32 / 20_000.0).collect();
        let mut owed = 0;
        input.chunks(3 * 480).for_each(|block| shared.capture(block, &mut owed));
        shared.armed.store(false, Ordering::Release);
        let (done, problem) = writer.join().unwrap();
        let channel = |c: usize| input.iter().skip(c).step_by(3).copied().collect::<Vec<f32>>();
        assert_eq!((done.len(), problem), (2, None));
        assert_eq!(decode(&done[0].1), vec![channel(1), channel(2)]);
        assert_eq!(decode(&done[1].1), vec![[vec![0.5; 100], vec![0.25; 50], channel(0)].concat()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Arm and disarm during a take: a track armed while it records gets its own file from the
    /// frame the writer is at (its start told by `late_start`), one disarmed stops there; both
    /// come back from `finish` with the rest.
    #[test]
    fn a_take_armed_or_disarmed_while_recording_starts_or_ends_there() {
        let dir = std::env::temp_dir().join(format!("soundcraft-late-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let shared = Arc::new(Capture::new(SampleRing::new(1 << 16)));
        let mut r = Recorder::attached(Arc::clone(&shared), "In".into(), 48_000, 48_000, 2);
        r.stepped = true;
        let (vox, gtr) = (dir.join("Vox_01.wav"), dir.join("Gtr_01.wav"));
        r.arm(vec![(TrackId(1), vec![0], vox.clone())]).unwrap();
        assert!(r.records(TrackId(1)) && !r.records(TrackId(2)));
        let input: Vec<f32> = (0..2 * 3_000).map(|i| i as f32 / 10_000.0).collect();
        let mut owed = 0;
        let (a, rest) = input.split_at(2 * 1_000);
        let (b, c) = rest.split_at(2 * 1_000);
        shared.capture(a, &mut owed);
        r.pump();
        r.add_take(TrackId(2), vec![1], gtr.clone()).unwrap();
        assert!(r.records(TrackId(2)));
        assert!(r.add_take(TrackId(3), vec![5], dir.join("Bass_01.wav")).is_err(), "not a channel of the frame");
        shared.capture(b, &mut owed);
        r.pump();
        assert_eq!(r.late_start(&gtr), Some(1_000));
        assert_eq!(r.late_start(&vox), None, "armed at Record");
        r.end_take(TrackId(1));
        assert_eq!(r.recording_tracks(), &[TrackId(2)]);
        shared.capture(c, &mut owed);
        let (done, problem) = r.finish();
        assert_eq!(problem, None);
        let channel = |k: usize, from: usize, to: usize| input.iter().skip(k).step_by(2).skip(from).take(to - from).copied().collect::<Vec<f32>>();
        assert_eq!(done.iter().map(|d| d.0).collect::<Vec<_>>(), vec![TrackId(1), TrackId(2)]);
        assert_eq!(decode(&vox), vec![channel(0, 0, 2_000)], "ends where it was disarmed");
        assert_eq!(decode(&gtr), vec![channel(1, 1_000, 3_000)], "starts where it was armed");
        assert!(r.add_take(TrackId(2), vec![1], dir.join("Gtr_02.wav")).is_err(), "not recording");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn takes_are_written_at_the_bit_depth_and_file_type_asked_for() {
        use soundcraft_audio_io::{BitDepth, BwfInfo, FileFormat, SampleFormat};
        let dir = std::env::temp_dir().join(format!("soundcraft-take-formats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let input: Vec<f32> = (0..2 * 480).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect();
        let bwf = BwfInfo { originator: "FreeDAWm".into(), ..BwfInfo::default() };
        for (fmt, depth, name, want, tol) in [
            (FileFormat::Wav, BitDepth::Int16, "Vox_16.wav", SampleFormat::Int16, 1.0 / 16_000.0),
            (FileFormat::Wav, BitDepth::Int24, "Vox_24.wav", SampleFormat::Int24, 1e-6),
            (FileFormat::Wav, BitDepth::Float32, "Vox_32.wav", SampleFormat::Float32, 0.0),
            (FileFormat::Aiff, BitDepth::Int24, "Vox_24.aif", SampleFormat::Int24, 1e-6),
            (FileFormat::Aiff, BitDepth::Float32, "Vox_32.aif", SampleFormat::Float32, 0.0),
        ] {
            let format = TakeFormat { format: fmt, depth, bwf: (fmt == FileFormat::Wav).then(|| bwf.clone()) };
            let path = dir.join(name);
            let mut t = TakeWriter::create_as(TrackId(1), vec![1], path.clone(), 48_000, &format).unwrap();
            t.write(&input[..2 * 240], 2);
            // A header written mid-take covers what is there; the file stays readable.
            t.write_header();
            t.write(&input[2 * 240..], 2);
            t.write_header();
            assert_eq!(t.error, None);
            let bytes = std::fs::read(&path).unwrap();
            let info = soundcraft_audio_io::probe(&bytes, None).unwrap();
            assert_eq!((info.format, info.sample_format, info.frames), (fmt, want, 480), "{name}");
            if fmt == FileFormat::Wav {
                let b = info.bwf.unwrap_or_else(|| panic!("{name}: no bext"));
                assert_eq!((b.originator.as_str(), b.description.as_str()), ("FreeDAWm", path.file_stem().unwrap().to_str().unwrap()));
                assert!(soundcraft_audio_io::bext_time_reference_offset(&bytes).is_some());
            }
            let got = decode(&path);
            let channel: Vec<f32> = input.iter().skip(1).step_by(2).copied().collect();
            assert!(got[0].iter().zip(&channel).all(|(a, b)| (a - b).abs() <= tol), "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_take_sums_itself_up_for_drawing_while_it_records() {
        let dir = std::env::temp_dir().join(format!("soundcraft-live-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut take = TakeWriter::create(TrackId(4), vec![2, 0], dir.join("Gtr_01.wav"), 48_000).unwrap();
        let input: Vec<f32> = (0..300).flat_map(|f| [f as f32, -1.0, -(f as f32)]).collect();
        take.write(&input[..3 * 100], 3);
        take.write(&input[3 * 100..], 3);
        let live = take.live.lock().unwrap().clone();
        assert_eq!(live, [[(-127.0, 0.0), (-255.0, -128.0), (-299.0, -256.0)], [(0.0, 127.0), (128.0, 255.0), (256.0, 299.0)]]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An offline output callback: an armed mono track, a stereo input through `feed` into
    /// `capture`, recording, played from `from` on a 48 kHz device with blocks of up to 256.
    fn recording_state(feed: &Arc<InputFeed>, capture: &Arc<Capture>, from: Samples) -> crate::AudioState {
        counted_recording_state(feed, capture, from, None).0
    }

    /// [`recording_state`] started with `countoff`, and its shared transport and command channel.
    fn counted_recording_state(
        feed: &Arc<InputFeed>,
        capture: &Arc<Capture>,
        from: Samples,
        countoff: Option<crate::CountOff>,
    ) -> (crate::AudioState, Arc<crate::Shared>, std::sync::mpsc::Sender<crate::Cmd>) {
        use soundcraft_model::{ChannelFormat, Session, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(crate::Shared::new());
        let st = crate::AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, 256, std::sync::mpsc::channel().0);
        tx.send(crate::Cmd::Input(Arc::clone(feed), vec![vec![0.0; 256]; feed.channels()])).unwrap();
        tx.send(crate::Cmd::Capture(Arc::clone(capture))).unwrap();
        tx.send(crate::Cmd::Recording(true)).unwrap();
        tx.send(crate::Cmd::Play { from, end: None, looped: None, countoff }).unwrap();
        (st, shared, tx)
    }

    /// One bar of four 300-frame beats: 1200 frames, which 128-frame callbacks cut mid-block.
    const SHORT_COUNTOFF: crate::CountOff = crate::CountOff { bars: 1, beats_per_bar: 4, beat: 300.0 };

    #[test]
    fn nothing_is_captured_during_the_count_off_and_the_take_starts_at_the_record_point() {
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        capture.armed.store(true, Ordering::Release);
        let (mut st, shared, _tx) = counted_recording_state(&feed, &capture, 1000, Some(SHORT_COUNTOFF));
        let probe = crate::TransportProbe { shared: Arc::clone(&shared) };
        let mut out = vec![0.0f32; 2 * 128];
        let mut sent = Vec::new();
        let mut clicks = 0.0f32;
        for b in 0..20 {
            let block = input_block(b);
            feed.push(&block);
            sent.extend_from_slice(&block);
            st.fill(&mut out, 2);
            if b < 9 {
                // Counting off (1152 of 1200 frames by the ninth callback): the transport holds at
                // the record point, its take clock still, and nothing is taken yet.
                clicks = clicks.max(out.iter().fold(0.0, |m, x| m.max(x.abs())));
                let r = probe.read();
                assert!(!r.playing && shared.counting.load(Ordering::Relaxed), "block {b}");
                assert_eq!((r.position, r.rolled), (1000, 0), "block {b}");
                assert!(!capture.started.load(Ordering::Acquire), "block {b}: no take during the count-off");
            }
        }
        assert!(clicks > 0.05, "the count-off is heard: {clicks}");
        assert!(!shared.counting.load(Ordering::Relaxed) && shared.playing.load(Ordering::Relaxed));
        // 20 callbacks of 128 frames: 1200 counting off, then 1360 rolled from the record point.
        let r = probe.read();
        assert_eq!((r.position, r.rolled), (1000 + 1360, 1360));
        assert_eq!(recorder(&capture).start_pos(), Some(1000), "the take starts at the record point");
        let mut got = Vec::new();
        capture.ring.pop_all(&mut got);
        assert_eq!(got, sent[2 * 1200..], "the input from the count-off's end on, none of it before");
    }

    #[test]
    fn stop_during_the_count_off_takes_nothing() {
        let dir = std::env::temp_dir().join(format!("soundcraft-countoff-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        let mut r = Recorder::attached(Arc::clone(&capture), "In".into(), 48_000, 48_000, 2);
        r.arm(vec![(TrackId(1), vec![0], dir.join("Vox_01.wav"))]).unwrap();
        let (mut st, shared, tx) = counted_recording_state(&feed, &capture, 1000, Some(SHORT_COUNTOFF));
        let mut out = vec![0.0f32; 2 * 128];
        for b in 0..5 {
            feed.push(&input_block(b));
            st.fill(&mut out, 2);
        }
        tx.send(crate::Cmd::Stop).unwrap();
        for b in 5..20 {
            feed.push(&input_block(b));
            st.fill(&mut out, 2);
        }
        assert!(!shared.counting.load(Ordering::Relaxed) && !shared.playing.load(Ordering::Relaxed));
        assert_eq!(shared.position.load(Ordering::Relaxed), 1000, "the transport never rolled");
        assert_eq!(r.start_pos(), None);
        let (done, problem) = r.finish();
        assert_eq!((done.len(), problem), (0, None), "no take");
        assert!(!dir.join("Vox_01.wav").exists(), "its empty file is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An offline output callback recording a mono "Print" track whose input is a bus that a
    /// constant 0.5 clip plays into, with or without a device input; its tap after the device's
    /// channels in each frame. Returns the state, the print track and its command channel.
    fn bus_recording_state(capture: &Arc<Capture>, feed: Option<&Arc<InputFeed>>) -> (crate::AudioState, TrackId) {
        use soundcraft_model::{ChannelFormat, Clip, Route, Session, SourceAudio, SourceId, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        // (a constant level from the first frame: no automatic edge fade on the clip's start)
        s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
        let print = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Print"));
        let src = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Synth"));
        let bus = s.add_bus("Print Bus", ChannelFormat::Mono);
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]] };
        s.pool.insert(SourceId(7), Arc::new(SourceAudio::new(buf)));
        let id = s.new_clip_id();
        let t = s.track_mut(src).unwrap();
        t.mixer.output = Route::Bus(bus);
        t.mixer.pan = vec![0.0];
        t.playlist_mut().unwrap().clips.push(Clip::audio(id, "dc", SourceId(7), 0, 0, 48_000));
        let p = s.track_mut(print).unwrap();
        p.mixer.input = Route::Bus(bus);
        p.mixer.record_arm = true;
        let (tx, rx) = std::sync::mpsc::channel();
        let st = crate::AudioState::new(rx, Arc::new(crate::Shared::new()), Arc::new(s), 48_000.0, 256, std::sync::mpsc::channel().0);
        let device = feed.map_or(0, |f| f.channels());
        if let Some(f) = feed {
            tx.send(crate::Cmd::Input(Arc::clone(f), vec![vec![0.0; 256]; f.channels()])).unwrap();
        }
        tx.send(crate::Cmd::Capture(Arc::clone(capture))).unwrap();
        tx.send(crate::Cmd::Taps(Taps::new(device, &[(print, 1)]))).unwrap();
        tx.send(crate::Cmd::Recording(true)).unwrap();
        tx.send(crate::Cmd::Play { from: 0, end: None, looped: None, countoff: None }).unwrap();
        // The state drops its receiver otherwise: keep the sender alive with it.
        std::mem::forget(tx);
        (st, print)
    }

    #[test]
    fn a_bus_input_track_records_the_bus_with_no_input_device() {
        let dir = std::env::temp_dir().join(format!("soundcraft-bus-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let capture = Capture::for_rate(48_000, 0);
        let mut r = Recorder::internal(Arc::clone(&capture), 48_000);
        assert!(r.is_internal() && r.channels == 0);
        let (mut st, print) = bus_recording_state(&capture, None);
        r.arm_with_taps(vec![(print, vec![0], dir.join("Print_01.wav"))], 1).unwrap();
        let mut out = vec![0.0f32; 2 * 128];
        for _ in 0..8 {
            st.fill(&mut out, 2);
        }
        assert_eq!(r.start_pos(), Some(0));
        assert_eq!(r.start_tap_latency(0), Some(0));
        // No input callback ever beats: not a stalled device.
        r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
        assert_eq!(r.trouble(), None);
        let (done, problem) = r.finish();
        assert_eq!((done.len(), problem), (1, None));
        let take = decode(&done[0].1);
        assert_eq!(take[0].len(), 8 * 128);
        let level = take[0][500];
        assert!(level > 0.3 && level <= 0.5 + 1e-6 && take[0].iter().all(|x| (x - level).abs() < 1e-6), "{level}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bus_tap_follows_the_device_channels_in_each_frame() {
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        capture.armed.store(true, Ordering::Release);
        let (mut st, _) = bus_recording_state(&capture, Some(&feed));
        let mut out = vec![0.0f32; 2 * 128];
        let mut sent = Vec::new();
        for b in 0..4 {
            let block = input_block(b);
            feed.push(&block);
            sent.extend_from_slice(&block);
            st.fill(&mut out, 2);
        }
        let mut got = Vec::new();
        capture.ring.pop_all(&mut got);
        assert_eq!(got.len(), 3 * 4 * 128, "two device channels and one tapped");
        let device: Vec<f32> = got.chunks_exact(3).flat_map(|f| [f[0], f[1]]).collect();
        assert_eq!(device, sent, "the device's frames as they came");
        let tap: Vec<f32> = got.chunks_exact(3).map(|f| f[2]).collect();
        assert!(tap.iter().all(|x| (x - tap[0]).abs() < 1e-6) && tap[0] > 0.3, "{}", tap[0]);
    }

    #[test]
    fn taps_lay_out_their_channels_after_the_devices() {
        let t = Taps::new(4, &[(TrackId(1), 2), (TrackId(2), 1)]);
        assert_eq!((t.width(), t.list().len()), (7, 2));
        assert_eq!([t.locate(0), t.locate(1), t.locate(2)], [(0, 0), (0, 1), (1, 0)]);
        assert_eq!(t.locate(3), (MAX_TAPS, 0), "past the last: nothing");
        let many: Vec<_> = (0..40).map(|i| (TrackId(i), 1)).collect();
        assert_eq!(Taps::new(0, &many).list().len(), MAX_TAPS);
    }

    /// Block `b` of a stereo input: 128 interleaved frames, every sample distinct and non-zero.
    fn input_block(b: usize) -> Vec<f32> {
        (0..256).map(|i| 1e-4 * (b * 256 + i + 1) as f32).collect()
    }

    #[test]
    fn pre_record_history_keeps_the_last_frames_oldest_first() {
        let mut h = History { buf: vec![0.0; 6], ..History::default() };
        let input = vec![vec![1.0, 2.0, 3.0, 4.0, 5.0], vec![-1.0, -2.0, -3.0, -4.0, -5.0]];
        h.push(&input, 5);
        let mut out = Vec::new();
        assert_eq!(h.drain(3, &mut out), 3, "three frames of two channels fit");
        assert_eq!(out, [3.0, -3.0, 0.0, 4.0, -4.0, 0.0, 5.0, -5.0, 0.0], "a wider frame's extra channel is silence");
        assert_eq!(h.drain(2, &mut out), 0, "drained once");
    }

    #[test]
    fn pre_record_puts_the_input_heard_before_record_at_the_takes_front() {
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        let r = Recorder::attached(Arc::clone(&capture), "Mic In".into(), 48_000, 48_000, 2);
        r.set_pre_record(1);
        capture.armed.store(true, Ordering::Release);
        let (mut st, _shared, tx) = counted_recording_state(&feed, &capture, 0, None);
        tx.send(crate::Cmd::Recording(false)).unwrap();
        let mut out = vec![0.0f32; 2 * 128];
        let mut want = Vec::new();
        for b in 0..8 {
            if b == 4 {
                tx.send(crate::Cmd::Recording(true)).unwrap();
            }
            let block = input_block(b);
            feed.push(&block);
            want.extend_from_slice(&block);
            st.fill(&mut out, 2);
        }
        let dir = std::env::temp_dir().join(format!("soundcraft-pre-record-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let take = TakeWriter::create(TrackId(1), vec![0, 1], dir.join("Vox_01.wav"), 48_000).unwrap();
        let live = Arc::clone(&take.live);
        let mut steps = Steps::new(vec![take], 2);
        steps.run(&capture);
        assert_eq!(r.pre_frames(), 4 * 128, "the four blocks before Record");
        assert!(!capture.pre_ready.load(Ordering::Acquire), "the callback may keep the history again");
        assert_eq!(live.lock().unwrap()[0].len(), 4 * 128 / LIVE_BLOCK, "the live take is drawn from the take's first frame");
        let (takes, problem) = steps.finish();
        assert_eq!((takes.len(), problem), (1, None));
        let chans = decode(&takes[0].1);
        let left: Vec<f32> = want.iter().step_by(2).copied().collect();
        assert_eq!(chans[0], left, "the file holds the pre-recorded frames, then the take");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_starts_at_the_block_recording_starts() {
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        capture.armed.store(true, Ordering::Release);
        let mut st = recording_state(&feed, &capture, 1000);
        let mut out = vec![0.0f32; 2 * 128];
        let mut sent = Vec::new();
        for b in 0..10 {
            let block = input_block(b);
            feed.push(&block);
            sent.extend_from_slice(&block);
            st.fill(&mut out, 2);
        }
        let r = recorder(&capture);
        assert_eq!(r.start_pos(), Some(1000));
        let mut got = Vec::new();
        capture.ring.pop_all(&mut got);
        assert_eq!(got, sent, "the captured frames are the input, interleaved, in order");
    }

    #[test]
    fn a_take_keeps_the_feed_latency_at_its_start() {
        // Read live when the take's first block is captured, not when the input opened: a
        // cushioned feed's offset is only known once it has primed.
        let (feed, capture) = (InputFeed::fixed_cushioned(2, 256), Capture::for_rate(48_000, 2));
        assert_eq!(feed.extra_latency(), 0, "not primed yet");
        let r = recorder(&capture);
        assert_eq!(r.start_feed_latency(), None, "no take yet");
        capture.armed.store(true, Ordering::Release);
        let mut st = recording_state(&feed, &capture, 0);
        let mut out = vec![0.0f32; 2 * 128];
        for b in 0..10 {
            feed.push(&input_block(b));
            st.fill(&mut out, 2);
        }
        assert_eq!(r.start_feed_latency(), Some(128));
    }

    #[test]
    fn a_missing_input_block_records_silence_and_keeps_timing() {
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        capture.armed.store(true, Ordering::Release);
        let mut st = recording_state(&feed, &capture, 0);
        let mut out = vec![0.0f32; 2 * 128];
        let mut want = Vec::new();
        for b in 0..30 {
            if (10..20).contains(&b) {
                want.extend_from_slice(&[0.0; 256]);
            } else {
                let block = input_block(b);
                feed.push(&block);
                want.extend_from_slice(&block);
            }
            st.fill(&mut out, 2);
        }
        let mut got = Vec::new();
        capture.ring.pop_all(&mut got);
        assert_eq!(got.len(), 30 * 256, "the take is as long as what played");
        assert_eq!(got, want, "silence where the input was missing; the rest in place");
        assert_eq!(feed.extra_latency(), 0, "the offset did not move");
    }

    #[test]
    fn takes_are_written_at_the_session_rate() {
        let dir = std::env::temp_dir().join(format!("soundcraft-session-rate-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A 44.1 kHz input device (split, drift feed) into a 48 kHz session.
        let (feed, capture) = (InputFeed::drift(1, 44_100, 48_000, 256), Capture::for_rate(48_000, 1));
        let mut r = Recorder::attached(Arc::clone(&capture), "Mic In".into(), 44_100, 48_000, 1);
        assert_eq!((r.sample_rate, r.device_rate), (48_000, 44_100));
        r.arm(vec![(TrackId(1), vec![0], dir.join("Vox_01.wav"))]).unwrap();
        let mut st = recording_state(&feed, &capture, 0);
        let mut out = vec![0.0f32; 2 * 480];
        for _ in 0..20 {
            feed.push(&[0.25; 441]);
            st.fill(&mut out, 2);
        }
        let (done, problem) = r.finish();
        assert_eq!((done.len(), problem), (1, None));
        let (_, take) = soundcraft_audio_io::decode(&std::fs::read(&done[0].1).unwrap(), None).unwrap();
        assert_eq!((take.sample_rate, take.channels[0].len()), (48_000, 20 * 480), "48 kHz, every frame played");
        assert!(take.channels[0][19 * 480..].iter().all(|x| (x - 0.25).abs() < 1e-6), "the input, once the feed filled");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
