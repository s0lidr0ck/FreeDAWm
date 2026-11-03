//! The SoundCraft mix engine.
//!
//! [`MixEngine::render`] produces one block of the main output for a session at a timeline
//! position: clips (with clip gain and fades) → trim/phase → inserts → pre-fader sends → fader and
//! mute (with automation) → pan → post-fader sends → busses → aux inputs → main → master faders.
//! The same code drives realtime playback and offline bounces, so what you hear is what you get.
//! Steady-state rendering does not allocate.
//!
//! ## Surround
//! The main mix has the format of the session's main output path ([`Session::main_format`]:
//! stereo by default, up to 16 channels such as 7.1.4 or 9.1.6) and every bus has its own format.
//! Channels are in SMPTE / WAV order (L R C LFE Ls Rs …, see
//! [`ChannelFormat::speakers`](soundcraft_model::ChannelFormat::speakers)). Routing a strip into a
//! destination builds a gain matrix per block:
//! - mono/stereo sources into a stereo destination use the stereo pan exactly as before;
//! - into a multichannel destination they use the track's [`SurroundPan`] (pairwise
//!   constant-power panner over the speaker layout, see `soundcraft_dsp::pan::surround_gains`),
//!   or go to L/R with the stereo pan when the track has no surround pan;
//! - sources whose format equals the destination's pass straight through, channel for channel;
//! - other multichannel sources fold to stereo with ITU-R BS.775 coefficients (C and surrounds at
//!   -3 dB, LFE dropped), or map speaker by speaker into other multichannel layouts.
//!
//! ## Plugin instances and threads
//! [`MixEngine::sync`] brings plugin instances in line with the session. Built-in plugins are
//! cheap (allocation only) and are always created there, on whichever thread renders. Hosted
//! third-party plugins (`clap:` / `vst3:` ids, see [`is_third_party`]) are expensive and have
//! main-thread rules, so a realtime host keeps them off the audio thread:
//! - with [`MixEngine::set_external_instances`]`(true)` (realtime playback), `sync` never creates a
//!   third-party plugin. The host's control (UI) thread creates them with [`create_instance`] from
//!   [`instance_specs`] (restoring each insert's stored [`Insert::state`] and calling `prepare`
//!   there), and hands them over with [`MixEngine::adopt`]; `sync` then moves the matching one
//!   into its slot. A slot whose instance has not arrived yet passes audio through.
//! - instances are keyed by [`SlotKey`]: a chain module's stable id, never its position, so a
//!   module that moves in the chain keeps its instance (and its state, editor and automation).
//! - every instance `sync` replaces or removes (built-in or not) goes to a retired list instead of
//!   being dropped; the host takes it with [`MixEngine::take_retired`] and drops it on the control
//!   thread, so plugin destruction never runs on the audio thread either.
//! - without external instances (the default: offline [`render_range`], bounces, tests) `sync`
//!   creates third-party plugins itself, synchronously, restores their stored state before the
//!   first `process`, and drops retired instances at the end of `sync`.
//!
//! An instance starts from its stored state; the insert's parameter values are then applied on
//! top before the first block (the session's values are kept in step with the plugin, including
//! edits made in its own editor, so both agree; the state carries everything else). A host reads
//! states back for saving through each instance's [`Plugin::state_reader`], on its UI thread.
//!
//! A built-in instrument's parameter values are applied the same way. A sample-playing
//! instrument (the Sampler) is handed the decoded audio of its insert's
//! [`Insert::sample`] source straight from the session's pool, shared and never copied.
//!
//! [`Insert::state`]: soundcraft_model::Insert::state
//! [`Insert::sample`]: soundcraft_model::Insert::sample
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use soundcraft_dsp::pan::{PanLaw, SpeakerPos, SurroundParams, gains};
use soundcraft_dsp::{LinkSlab, MidiOut, Plugin, SampleSource, Transport, db_to_gain};
use soundcraft_midi::CtrlKind;
use soundcraft_model::{
    AutoParam, BusId, ChannelFormat, Clip, ClipContent, FrozenAudio, InputMonitor, ModuleId, ModuleRef, Route, Session, SoloScope, SourceAudio,
    Speaker, SurroundPan, Track, TrackId, TrackKind, track_input_channel,
};
use soundcraft_time::{Range, Samples, TICKS_PER_QUARTER};
use std::collections::HashMap;
use std::ops::Bound;
use std::sync::Arc;

pub mod band;
pub use band::{
    BAND_SLOTS, BandSongCache, BandUpdate, SLOT_CAPACITY, SlotBuf, auto_target, band_transport, band_transport_within, new_band_runtime,
    prepare_band, prepare_band_cached,
};
mod band_lanes;
pub use band_lanes::{band_song_end, half_ticks, lane_song, lanes_from_song};
mod delay_line;
use delay_line::DelayLine;
mod link;
pub mod live_midi;
pub use live_midi::{HeldNotes, LIVE_MIDI_CAPACITY, LiveEvent, LiveMidi, MAX_MIDI_PORTS, MidiFilter, port_id};
pub mod scout;
pub use link::{LinkProblem, LinkRoute, describe_link, link_refusal, link_regressions, link_routes, link_routes_with, short_link_problem};
mod sidechain;
use sidechain::{KeySlot, KeySource, fill_key};
pub use sidechain::{
    ModuleInputRoute, SidechainProblem, SidechainRoute, describe_route, module_input_refusal, module_input_routes, short_problem, sidechain_refusal,
    sidechain_regressions, sidechain_routes, sidechain_routes_with,
};

/// Default (stereo) main mix width. The actual width is [`main_channels`].
pub const MAIN_CHANNELS: usize = 2;

/// Widest strip, bus or main mix (9.1.6 / 3rd-order Ambisonics). Wider formats are truncated.
pub const MAX_CHANNELS: usize = 16;

/// Number of channels of a session's main mix.
pub fn main_channels(s: &Session) -> usize {
    s.main_format().channels().clamp(MAIN_CHANNELS, MAX_CHANNELS)
}

/// Which plugin of a track an instance, editor, state or failure belongs to: a chain module (by
/// its stable id, never its position) or the track's instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum SlotKey {
    Module(ModuleId),
    Instrument,
}

/// Hosted third-party plugin ids (CLAP or VST3): created off the audio thread in realtime use.
pub fn is_third_party(id: &str) -> bool {
    id.starts_with(soundcraft_clap_host::ID_PREFIX)
        || id.starts_with(soundcraft_vst3_host::ID_PREFIX)
        || id.starts_with(soundcraft_au_host::ID_PREFIX)
}

/// A third-party plugin instance a session needs: which slot, which plugin, how many channels,
/// and the state to restore.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceSpec {
    pub track: TrackId,
    /// The chain module, or the instrument.
    pub slot: SlotKey,
    pub id: String,
    /// Channel count the instance is prepared for.
    pub ch: usize,
    /// Decoded [`Insert::state`](soundcraft_model::Insert::state).
    pub state: Option<Vec<u8>>,
    /// [`Insert::state_load`](soundcraft_model::Insert::state_load): when it changes, the
    /// instance is created again from `state` (a preset was loaded into the module).
    pub state_load: u64,
}

/// A plugin instance created and prepared off the audio thread, handed to [`MixEngine::adopt`].
pub struct PreparedInstance {
    pub track: TrackId,
    pub slot: SlotKey,
    pub id: String,
    pub ch: usize,
    pub plugin: Box<dyn Plugin>,
}

/// Every third-party plugin slot (inserts and instruments) of a session, with the channel count
/// [`MixEngine::sync`] prepares it for. Inactive and master tracks are included (they have strips).
pub fn instance_specs(s: &Session) -> Vec<InstanceSpec> {
    let main_ch = main_channels(s);
    let mut out = Vec::new();
    for t in &s.tracks {
        let ch = strip_channels(t, main_ch);
        for ins in t.mixer.inserts.iter().filter(|i| is_third_party(&i.plugin)) {
            out.push(InstanceSpec {
                track: t.id,
                slot: SlotKey::Module(ins.id),
                id: ins.plugin.clone(),
                ch,
                state: ins.state_bytes(),
                state_load: ins.state_load,
            });
        }
        if let Some(ins) = &t.instrument
            && is_third_party(&ins.plugin)
        {
            out.push(InstanceSpec {
                track: t.id,
                slot: SlotKey::Instrument,
                id: ins.plugin.clone(),
                ch: ch.max(2),
                state: ins.state_bytes(),
                state_load: ins.state_load,
            });
        }
    }
    out
}

/// A plugin made by [`create_instance`].
pub struct NewInstance {
    pub plugin: Box<dyn Plugin>,
    /// The plugin refused the stored state and starts from its defaults.
    pub refused_state: bool,
}

/// Creates a plugin (built-in or hosted), restores `state` (if any) and prepares it. Expensive
/// for third-party plugins: in realtime use, call it on the control thread.
pub fn create_instance(id: &str, state: Option<&[u8]>, sample_rate: f32, max_block: usize, ch: usize) -> Option<NewInstance> {
    create_instance_for(id, state, sample_rate, max_block, ch, false)
}

/// [`create_instance`] for an offline render when `offline` (a bounce, freeze or print): the
/// plugin is told before it is prepared ([`Plugin::set_offline`]).
pub fn create_instance_for(id: &str, state: Option<&[u8]>, sample_rate: f32, max_block: usize, ch: usize, offline: bool) -> Option<NewInstance> {
    let mut plugin = create_plugin(id)?;
    plugin.set_offline(offline);
    let refused_state = state.is_some_and(|st| !plugin.load_state(st));
    if refused_state {
        log::warn!("{id}: the plugin did not accept its stored state");
    }
    plugin.prepare(sample_rate, max_block, ch);
    Some(NewInstance { plugin, refused_state })
}

/// Peak meter values for one strip (linear, per channel, max over the last block).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StripMeter {
    /// First two channels (mono strips repeat channel 0), for stereo meters.
    pub peak: [f32; 2],
    /// Every channel of the strip (or of the main mix).
    pub peaks: Vec<f32>,
    /// Gain reduction of the first dynamics insert, dB.
    pub gain_reduction_db: f32,
    pub plugin_failures: PluginFailures,
    /// One entry per chain module, in chain order (resized only when the chain's structure
    /// changes, never while rendering).
    pub modules: Vec<ModuleMeter>,
    /// Sample streams of the chain modules whose plugin has any, in chain order (filled in
    /// `sync`, never while rendering). The `Arc` lives as long as the instance, so it survives
    /// reorders.
    pub analysis: Vec<(ModuleId, Arc<soundcraft_dsp::analysis::Analysis>)>,
    /// Low-latency monitoring: bit `i` set means chain module `i` is bypassed by it (modules
    /// from 64 on aren't reported). Zero while it's off.
    pub llm_bypassed: u64,
}

/// Live values of one chain module for the last block. A module with no instance (missing, not
/// adopted yet) or one that is bypassed, inactive or already failed passes the signal through:
/// its `in_peak` and `out_peak` are equal and `gr_db` is 0. On the one block where an instance
/// trips the NaN/infinity containment, `failed` is set and its output is silenced, so `out_peak`
/// is 0 (not `in_peak`) and `gr_db` is 0. Levels are always finite; a readout is finite or NaN.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleMeter {
    pub module: ModuleId,
    /// Linear maximum over the block and all channels, before the module.
    pub in_peak: f32,
    /// The same, after the module.
    pub out_peak: f32,
    /// Gain reduction, dB (non-negative).
    pub gr_db: f32,
    /// Engine-specific live values; see [`Plugin::readouts`] and each plugin's impl. NaN: no
    /// reading (a slot the plugin doesn't use, or the module didn't run this block).
    pub readouts: [f32; 8],
    /// The instance was taken out of the signal path for putting out NaN or infinity.
    pub failed: bool,
}

impl Default for ModuleMeter {
    fn default() -> Self {
        ModuleMeter { module: ModuleId::default(), in_peak: 0.0, out_peak: 0.0, gr_db: 0.0, readouts: [f32::NAN; 8], failed: false }
    }
}

/// Fills `out` with the plugin's readouts: NaN where it wrote nothing (or wrote infinity).
fn read_readouts(plugin: &dyn Plugin, out: &mut [f32; 8]) {
    *out = [f32::NAN; 8];
    plugin.readouts(out);
    for r in out.iter_mut().filter(|r| r.is_infinite()) {
        *r = f32::NAN;
    }
}

/// How often a strip's plugins were taken out of the signal path for putting out NaN or infinity.
/// Each stays out until the engine is next reset (playback starts or stops).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PluginFailures {
    /// It only grows.
    pub count: u32,
    /// The plugin of the latest (`None` until the first).
    pub last: Option<SlotKey>,
}

impl PluginFailures {
    fn record(&mut self, slot: SlotKey) {
        self.count = self.count.wrapping_add(1);
        self.last = Some(slot);
    }
}

impl StripMeter {
    /// Copy another meter without reallocating once the channel count is stable.
    pub fn copy_from(&mut self, o: &StripMeter) {
        self.peak = o.peak;
        self.peaks.clone_from(&o.peaks);
        self.gain_reduction_db = o.gain_reduction_db;
        self.plugin_failures = o.plugin_failures;
        self.modules.clone_from(&o.modules);
        self.analysis.clone_from(&o.analysis);
        self.llm_bypassed = o.llm_bypassed;
    }

    fn measure(&mut self, bufs: &[Vec<f32>], frames: usize, gr: f32) {
        if self.peaks.len() != bufs.len() {
            self.peaks.resize(bufs.len(), 0.0);
        }
        for (p, b) in self.peaks.iter_mut().zip(bufs.iter()) {
            *p = peak(b.get(..frames).unwrap_or(&[]));
        }
        let last = self.peaks.len().saturating_sub(1);
        self.peak = [self.peaks.first().copied().unwrap_or(0.0), self.peaks.get(1.min(last)).copied().unwrap_or(0.0)];
        self.gain_reduction_db = gr;
    }
}

/// A bus: its format and one buffer per channel.
struct BusBuf {
    fmt: ChannelFormat,
    buf: Vec<Vec<f32>>,
}

struct PluginSlot {
    /// The chain module this instance belongs to (unused for the instrument).
    module: ModuleId,
    /// Stamped when the instance was placed, unique per engine: tells a kept instance from a
    /// re-created one.
    serial: u64,
    id: String,
    /// Channel count the plugin was prepared for.
    ch: usize,
    /// The insert's `state_load` the instance was made for (see [`InstanceSpec::state_load`]).
    state_load: u64,
    plugin: Box<dyn Plugin>,
    /// Last values pushed to the plugin, in the insert's parameter-map order.
    applied: Vec<f32>,
    /// Whether each of those is driven by an automation lane, as last told to the plugin
    /// (`Plugin::set_param_automated`).
    laned: Vec<bool>,
    /// A fingerprint of the stored params' keys `applied` and `laned` line up with (`keys_hash`):
    /// when the key set changes (even at the same length) both are rebuilt.
    keys: u64,
    failed: bool,
    /// The [`PreparedState::rev`](soundcraft_model::PreparedState) last handed to the plugin
    /// (`Plugin::set_prepared_state`); 0: none.
    prepared: u64,
    /// The assets handed to the plugin (`Plugin::set_asset`), by key: the engine's own references,
    /// so the plugin never holds the last one.
    assets: Vec<(String, Arc<dyn SampleSource>)>,
    /// The link group slab handed to the plugin (`Plugin::set_link`) and the member index.
    link: Option<(Arc<LinkSlab>, usize)>,
    /// Low-latency monitoring bypasses it (or is crossfading to that), see `llm_step`.
    llm_off: bool,
    /// Samples into that crossfade (negative: a resumed plugin is still priming its latency);
    /// done at the crossfade length (`LLM_SETTLED` when none ran).
    llm_pos: i64,
    /// Not run since it was made or reset: a low-latency monitoring change applies at once.
    llm_fresh: bool,
    /// The plugin declares MIDI output (`PluginInfo::midi_out`): `midi` is filled after each
    /// `process` and read by the strips whose input is this module.
    emits: bool,
    /// The module's MIDI of the block just run (made with the slot: never grown while rendering).
    midi: MidiOut,
}

struct Strip {
    buf: Vec<Vec<f32>>,
    /// Pre-fader copy for pre-fader sends.
    pre: Vec<Vec<f32>>,
    scratch: Vec<f32>,
    /// One entry per chain module, in chain order (`None` while a plugin is missing or waiting
    /// to be adopted). Each instance carries its module id: `sync` matches by id, not position.
    plugins: Vec<Option<PluginSlot>>,
    instrument: Option<PluginSlot>,
    /// The pool audio handed to the instrument with `set_sample` (sample-playing instruments).
    sample: Option<Arc<SourceAudio>>,
    /// MIDI notes currently sounding on the instrument (pitch), for note-offs at stops/seek.
    held: Vec<u8>,
    /// The musician this strip's instrument last played (`Route::Musician`), `None` for clips.
    musician: Option<usize>,
    /// Notes live MIDI input (`Route::Midi`) holds on the instrument: let go when the track
    /// stops hearing it, hears another port or channel, or the port they came from closes.
    live_held: HeldNotes,
    /// The live input the strip heard last block (`None`: none).
    live_filter: Option<MidiFilter>,
    /// The modules whose MIDI this strip's instrument last played (`Route::Modules`), in source
    /// order; empty for clips or a musician. Capacity reserved in `sync`.
    module_in: Vec<ModuleRef>,
    /// Those sources as `sync` resolved them: (index in the input's list, source track index,
    /// module); a source that plays nothing (missing, a cycle, unsupported) is left out.
    midi_src: Vec<(u8, usize, ModuleId)>,
    /// Their events of this block, merged in sample order (copied from the source strips, which
    /// rendered in earlier levels; a fixed ring).
    midi_in: Box<StripMidi>,
    /// Notes the module sources started that still sound: (source index, channel, pitch), so a
    /// source taken out of the set closes only its own. Capacity reserved in `Strip::new`.
    midi_held: Vec<(u8, u8, u8)>,
    /// The instrument's next block starts somewhere new: send it the controllers in effect there.
    chase: bool,
    muted: bool,
    gr: f32,
    /// Per-clip effect chains (Clip Effects) and a scratch buffer to render clips into.
    clip_fx: HashMap<u64, ClipFx>,
    clipbuf: Vec<Vec<f32>>,
    /// Total latency of the active inserts (samples).
    latency: usize,
    /// Delay compensation: align this strip with the slowest one (post- and pre-fader paths),
    /// plus extra delay for direct-to-main outputs so they line up with aux returns.
    align: DelayLine,
    align_pre: DelayLine,
    out_extra: DelayLine,
    outbuf: Vec<Vec<f32>>,
    failures: PluginFailures,
    /// Per-module meter values of the last block, parallel to the chain (sized in `sync`).
    mods: Vec<ModuleMeter>,
    /// Analysis handles of the chain's plugins (collected in `sync`).
    analysis: Vec<(ModuleId, Arc<soundcraft_dsp::analysis::Analysis>)>,
    /// Sidechain keys of the chain's keyed modules (built in `sync`, see `sidechain`).
    keys: Vec<KeySlot>,
    /// Rendering level among non-aux strips (a key source renders in an earlier level).
    level: u32,
    /// The block (`MixEngine::block`) this strip last rendered in: a key is read only from a
    /// source rendered in the current block.
    stamp: u64,
    /// Timeline delay of the strip's input (an aux reads busses already delay-compensated).
    base: usize,
    /// Timeline delay of `buf` (post-fader) and `pre` as they stand, for the keys read from them.
    post_at: usize,
    pre_at: usize,
    /// Some module keys from this strip pre-fader: `pre` is filled every block.
    pre_key: bool,
    /// A key line needed more than its capacity this block (the next render grows it).
    key_regrow: bool,
    /// Assets the chain's plugins let go of while rendering (`Plugin::take_released_asset`), for
    /// the host to drop off the audio thread (capacity reserved in `sync`).
    released: Vec<Arc<dyn SampleSource>>,
    /// This strip's Scout PRE/POST taps while a capture runs (see [`scout`]).
    scout: Option<Box<scout::StripTaps>>,
    /// Where the strip's output goes, resolved in `sync` (no name lookups while rendering).
    dest: Dest,
    /// The device input channels (first, width) the strip hears when monitoring, resolved in
    /// `sync`; `None`: an unknown input (silence).
    in_span: Option<(usize, usize)>,
    /// The strip heard live input in its last render (monitored audio input, or live MIDI into
    /// its instrument): the mix's tail must not cut what is still being played.
    hears_live: bool,
    /// On the low-latency monitoring path this block (`llm_scope`, and the mode is on).
    llm: bool,
    /// The modules low-latency monitoring bypasses (`StripMeter::llm_bypassed`), last block.
    llm_bits: u64,
    /// Their latency (not in `latency`), last block.
    llm_latency: usize,
    /// A module's input while low-latency monitoring crossfades it (channels x `max_block`,
    /// sized in `sync` for strips with modules).
    llm_dry: Vec<Vec<f32>>,
    /// How many chain modules (from the top) the frozen render held last block: a module that
    /// comes back to life (thawed, or a freeze point moved up) starts clean.
    frozen_mods: usize,
    /// The instrument was frozen last block (it didn't run): thawed, it starts clean.
    frozen_inst: bool,
    /// Live input let go with the transport stopped: the instrument keeps running (the frames
    /// left, at most [`LIVE_TAIL_SECS`]) until its release dies away, so the release is heard and
    /// no voice is left half released to sound when the input is heard again.
    live_tail: usize,
    /// Frames in a row the instrument has been silent while `live_tail` runs: the release is over
    /// only after [`TAIL_QUIET_SECS`] of it (a quiet gap in a gated or arpeggiated release is not
    /// its end).
    tail_quiet: usize,
}

/// The longest release a strip's instrument runs after its live input is let go, stopped. The
/// whole mix renders on at most this long after a release begins (see `MixEngine::tail_on`).
const LIVE_TAIL_SECS: f64 = 4.0;

/// How long a release (an instrument's, or the mix's after it) must stay below [`TAIL_SILENCE`]
/// to be over.
const TAIL_QUIET_SECS: f64 = 0.05;

/// Below this peak a release counts as silent.
const TAIL_SILENCE: f32 = 1e-5;

/// Events a strip's merged module MIDI holds per block (several sources' rings).
const STRIP_MIDI: usize = 256;
/// Sounding module notes a strip keeps track of at most.
const MIDI_HELD: usize = 512;

/// One event of a strip's merged module MIDI: the message at its offset, which source (index in
/// the input's list) emitted it and in what order (`seq`), for a stable sort by offset.
#[derive(Debug, Clone, Copy, Default)]
struct StripMidiEvent {
    offset: u32,
    msg: [u8; 3],
    src: u8,
    seq: u16,
}

/// A strip's module MIDI of one block: the sources' events merged in sample order, stable by
/// source order. A fixed ring: full, it drops and counts.
struct StripMidi {
    events: [StripMidiEvent; STRIP_MIDI],
    len: usize,
    dropped: u32,
}

impl StripMidi {
    fn new() -> Self {
        StripMidi { events: [StripMidiEvent::default(); STRIP_MIDI], len: 0, dropped: 0 }
    }

    fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }

    /// Adds source `src`'s events (in its order).
    fn add(&mut self, src: u8, out: &MidiOut) {
        for e in out.events() {
            match self.events.get_mut(self.len) {
                Some(slot) => {
                    *slot = StripMidiEvent { offset: e.offset, msg: e.msg, src, seq: self.len as u16 };
                    self.len += 1;
                }
                None => self.dropped = self.dropped.saturating_add(1),
            }
        }
    }

    /// Orders by offset, then source, then arrival (unique keys: no allocation needed).
    fn sort(&mut self) {
        if let Some(e) = self.events.get_mut(..self.len) {
            e.sort_unstable_by_key(|e| (e.offset, e.src, e.seq));
        }
    }

    fn events(&self) -> &[StripMidiEvent] {
        self.events.get(..self.len).unwrap_or(&[])
    }
}

/// Most latency delay compensation handles: 2^20 samples, about 22 s at 48 kHz. A strip reports
/// at most this, whatever its plugins claim, so a bogus value can't stall offline renders.
const MAX_LATENCY: usize = 1 << 20;

/// Length of the linear crossfade between the old and the new delay tap when a strip's
/// compensation delay changes while audio runs through it.
const CROSSFADE_MS: f32 = 10.0;

/// Headroom `sync` reserves in every delay-compensation line on top of the current latency, so a
/// module whose latency grows while playing is re-aligned without allocating.
const HEADROOM_S: f32 = 0.25;

/// Clip Effects: an EQ and a compressor per clip, configured from `edit.values`
/// (`clip_fx.<clip>.eq.<param>`, `clip_fx.<clip>.comp.<param>`, `clip_fx.<clip>.gain`).
struct ClipFx {
    eq: Option<Box<dyn Plugin>>,
    comp: Option<Box<dyn Plugin>>,
    gain_db: f32,
    sr: f32,
    ch: usize,
}

impl Strip {
    fn new() -> Self {
        Strip {
            buf: Vec::new(),
            pre: Vec::new(),
            scratch: Vec::new(),
            plugins: Vec::new(),
            instrument: None,
            sample: None,
            held: Vec::new(),
            musician: None,
            live_held: HeldNotes::default(),
            live_filter: None,
            module_in: Vec::new(),
            midi_src: Vec::new(),
            midi_in: Box::new(StripMidi::new()),
            midi_held: Vec::with_capacity(MIDI_HELD),
            chase: true,
            muted: false,
            gr: 0.0,
            clip_fx: HashMap::new(),
            clipbuf: Vec::new(),
            latency: 0,
            align: DelayLine::default(),
            align_pre: DelayLine::default(),
            out_extra: DelayLine::default(),
            outbuf: Vec::new(),
            failures: PluginFailures::default(),
            mods: Vec::new(),
            analysis: Vec::new(),
            keys: Vec::new(),
            level: 0,
            stamp: 0,
            base: 0,
            post_at: 0,
            pre_at: 0,
            pre_key: false,
            key_regrow: false,
            released: Vec::new(),
            scout: None,
            dest: Dest::None,
            in_span: None,
            hears_live: false,
            llm: false,
            llm_bits: 0,
            llm_latency: 0,
            llm_dry: Vec::new(),
            frozen_mods: 0,
            frozen_inst: false,
            live_tail: 0,
            tail_quiet: 0,
        }
    }

    /// The merged module MIDI this strip played in the block it last rendered.
    fn module_midi_in(&self) -> impl Iterator<Item = (u32, [u8; 3])> + '_ {
        self.midi_in.events().iter().map(|e| (e.offset, e.msg))
    }

    /// The MIDI module `m` of this strip emitted in the block it last rendered.
    fn module_midi(&self, m: ModuleId) -> Option<&MidiOut> {
        self.plugins.iter().flatten().find(|p| p.module == m && p.emits).map(|p| &p.midi)
    }

    /// What this strip offers a sidechain key now.
    fn key_source(&self) -> KeySource<'_> {
        KeySource { buf: &self.buf, pre: &self.pre, post_at: self.post_at, pre_at: self.pre_at, muted: self.muted, stamp: self.stamp }
    }
}

/// Renders sessions. Keep one per playback stream; plugin state persists across blocks.
pub struct MixEngine {
    sample_rate: f32,
    max_block: usize,
    strips: HashMap<TrackId, Strip>,
    busses: HashMap<BusId, BusBuf>,
    main: Vec<Vec<f32>>,
    main_fmt: ChannelFormat,
    /// Hardware output buffers, indexed like `Session::outputs`: index 0 (the main) is always
    /// empty, entry `i >= 1` has the output's channels of `max_block`. Sized in `sync`, cleared
    /// in `render`; offline renders never read them.
    hw: Vec<Vec<Vec<f32>>>,
    /// Delay lines for the hardware outputs (parallel to `hw`): the main mix passes through the
    /// master faders' inserts and hardware outputs don't, so they wait for that latency.
    hw_delay: Vec<DelayLine>,
    /// The master faders' insert latency of the last block (what `hw_delay` waits for).
    master_latency: usize,
    /// The low-latency monitoring path of this block (`llm_scope`; reserved in `sync`).
    llm: LlmScope,
    /// Which tracks a solo leaves audible (reserved by `sync`, filled each block).
    solo: SoloScope,
    /// The largest strip compensation with low-latency monitoring off, whatever it is now
    /// (`reserve_delays` sizes the lines for it, so a toggle never grows them).
    full_latency: usize,
    /// The master faders' insert latency with low-latency monitoring off.
    full_master_latency: usize,
    /// An offline render (bounce, stem, export): low-latency monitoring never applies.
    offline: bool,
    pub meters: HashMap<TrackId, StripMeter>,
    pub main_meter: StripMeter,
    pub pan_law: PanLaw,
    /// Last rendered position end (to detect seeks).
    last_end: Samples,
    /// Where the last block started, and whether it rendered with the transport stopped: a
    /// stopped transport renders the same position again and again, which is no seek.
    last_start: Samples,
    last_stopped: bool,
    /// The live MIDI port identities of the last block, and the slots whose port is gone this
    /// block (bit per slot): the notes held from them never get a note-off.
    last_ports: [u64; MAX_MIDI_PORTS],
    ports_gone: u16,
    /// Snapshot key of the session the strips/order were synced to.
    synced: (usize, usize, usize),
    order: Vec<usize>,
    /// Current total compensation delay (samples) of the main mix: the slowest track and aux
    /// returns, and the master faders' inserts.
    pub latency: usize,
    /// Delay already applied to audio written onto busses: the slowest track insert.
    /// A bus render drops this so the bus lines up with the timeline. The extra delay
    /// that lines direct outputs up with aux returns stays in `latency` and is removed
    /// only from the main mix.
    track_latency: usize,
    /// Busses an aux returns onto, and the latency of the slowest aux return: audio written onto
    /// such a bus waits for it, so a bounce of the bus drops it as well.
    aux_fed: Vec<BusId>,
    aux_latency: usize,
    /// Live input for the next block (planar), set by the audio host for input monitoring.
    pub input: Vec<Vec<f32>>,
    /// Hardware MIDI input for the next block (`live_midi`), filled by the audio host before
    /// [`MixEngine::render`], which empties it.
    pub live_midi: LiveMidi,
    /// Transport stopped: only monitored inputs sound (no clips).
    pub monitor_only: bool,
    /// Recording: record-armed tracks hear their input (auto input).
    pub recording: bool,
    /// Mix the metronome (Options > Click) into the output. Playback turns this on; offline renders
    /// (bounces, stems, exports) leave it off so the click is never printed into a file.
    pub metronome: bool,
    /// A count-off under way before a take, and the frames of it already played at the start of
    /// the next block: its clicks are mixed in (with `metronome`, whatever Options > Click says).
    /// The host sets it each block while it counts off, with the transport held at the record
    /// point (`monitor_only`).
    pub countoff: Option<(CountOff, i64)>,
    /// Third-party instances come from [`MixEngine::adopt`] instead of being created in `sync`.
    external: bool,
    /// Instances handed over by the host, waiting for `sync` to place them.
    adopt: HashMap<(TrackId, SlotKey), PreparedInstance>,
    /// Instances `sync` took out of service, for the host to drop (see [`MixEngine::take_retired`]).
    retired: Vec<Box<dyn Plugin>>,
    /// Lent to an offline render ([`LiveInstances`]): `sync` parks the third-party slots it would
    /// retire in `parked` instead, so the render's session (a track soloed, a chain cut at a
    /// freeze point) never costs the host an instance; the sync back to the host's session
    /// seats them again, bookkeeping and all.
    lent: bool,
    parked: HashMap<(TrackId, SlotKey), PluginSlot>,
    /// Analysis handles of the instances `sync` took out of service (see
    /// [`MixEngine::take_retired_analysis`]).
    retired_analysis: Vec<Arc<soundcraft_dsp::analysis::Analysis>>,
    /// Module assets taken out of service (see [`MixEngine::take_retired_assets`]).
    retired_assets: Vec<Arc<dyn SampleSource>>,
    /// A delay-compensation line needed more than its capacity: the next render re-sizes them.
    regrow: bool,
    /// Last instance serial handed out (see `PluginSlot::serial`).
    serial: u64,
    /// Ownership workspace reused for parallel strip processing.
    work: Vec<(usize, Strip)>,
    /// Blocks rendered (stamps strips, so keys are read only from this block's sources).
    block: u64,
    /// A live release runs with the transport stopped: the mix renders on until its whole
    /// output (inserts, sends, aux returns, hardware outputs) has been quiet for
    /// [`TAIL_QUIET_SECS`], at most [`LIVE_TAIL_SECS`] (`tail_age`) after the release began;
    /// `tail_quiet` counts the quiet frames in a row.
    tail_on: bool,
    tail_age: usize,
    tail_quiet: usize,
    /// The value bus and members of each link group (`Insert::link_group`), made in `sync`.
    links: Vec<link::LinkGroup>,
    /// Link slabs taken out of service (see [`MixEngine::take_retired_links`]).
    retired_links: Vec<Arc<LinkSlab>>,
    /// The armed Scout capture (see [`MixEngine::arm_scout`]).
    scout: Option<Box<scout::ScoutRig>>,
    /// The Band (see [`band`]): its runtime, the update it plays, updates it replaced, the `Arc`s
    /// it let go of, its events of this block per musician, and whether it was playing.
    band: Option<Box<soundcraft_band::runtime::BandRuntime>>,
    band_update: Option<Box<BandUpdate>>,
    band_spent: [Option<Box<BandUpdate>>; 4],
    band_retired: soundcraft_band::runtime::Retired,
    band_out: [SlotBuf; BAND_SLOTS],
    band_playing: bool,
}

/// What `sync` needs to fill a slot.
struct SlotCtx<'a> {
    sr: f32,
    mb: usize,
    external: bool,
    /// Instances made here are for an offline render.
    offline: bool,
    adopt: &'a mut HashMap<(TrackId, SlotKey), PreparedInstance>,
    retired: &'a mut Vec<Box<dyn Plugin>>,
    retired_analysis: &'a mut Vec<Arc<soundcraft_dsp::analysis::Analysis>>,
    retired_assets: &'a mut Vec<Arc<dyn SampleSource>>,
    retired_links: &'a mut Vec<Arc<LinkSlab>>,
    serial: &'a mut u64,
    lent: bool,
    parked: &'a mut HashMap<(TrackId, SlotKey), PluginSlot>,
}

impl SlotCtx<'_> {
    /// Takes `slot` (of track `t`, slot `key`) out of service: its assets and link go back to the
    /// host, and the instance is retired, or parked while the engine is lent (a third-party one:
    /// built-in instances are cheap to make again).
    fn retire(&mut self, t: TrackId, key: SlotKey, slot: Option<PluginSlot>) {
        if let Some(mut s) = slot {
            while let Some(a) = s.plugin.take_released_asset() {
                self.retired_assets.push(a);
            }
            self.retired_assets.extend(s.assets.drain(..).map(|(_, a)| a));
            if let Some((l, _)) = s.link.take() {
                // The retired instance leaves its group now (its slot freed for a newcomer), not
                // whenever it is dropped; the slab itself is retired, never freed here.
                self.retired_links.push(l);
                s.plugin.set_link(None, 0);
            }
            if self.lent && is_third_party(&s.id) {
                if let Some(old) = self.parked.insert((t, key), s) {
                    self.retired.push(old.plugin);
                }
            } else {
                self.retired.push(s.plugin);
            }
        }
    }

    /// True when the host handed over an instance for this slot. It replaces whatever the slot
    /// holds, even an instance of the same plugin: the host made it because it no longer tracks
    /// the old one (it was removed and re-added between two renders), and drives its editor.
    fn waiting(&self, t: TrackId, key: SlotKey) -> bool {
        self.external && self.adopt.contains_key(&(t, key))
    }

    /// A new instance for `ins` on track `t`, slot `key`, prepared for `ch` channels.
    fn make(&mut self, t: TrackId, key: SlotKey, ins: &soundcraft_model::Insert, ch: usize) -> Option<PluginSlot> {
        let third = is_third_party(&ins.plugin);
        // A slot parked while the engine is lent comes back as it was (its bookkeeping too: the
        // values already sent are not sent again).
        if third && self.lent && self.parked.get(&(t, key)).is_some_and(|p| p.id == ins.plugin && p.ch == ch) {
            let mut slot = self.parked.remove(&(t, key))?;
            slot.state_load = ins.state_load;
            return Some(slot);
        }
        let plugin = if third && self.external {
            match self.adopt.remove(&(t, key)) {
                Some(p) if p.id == ins.plugin && p.ch == ch => Some(p.plugin),
                Some(p) if self.lent => {
                    // Not for this session's channel count: it waits for the host's session.
                    self.adopt.insert((t, key), p);
                    None
                }
                Some(p) => {
                    self.retired.push(p.plugin);
                    None
                }
                None => None,
            }
        } else {
            let state = if third { ins.state_bytes() } else { None };
            create_instance_for(&ins.plugin, state.as_deref(), self.sr, self.mb, ch, self.offline).map(|n| n.plugin)
        }?;
        *self.serial = self.serial.wrapping_add(1);
        let emits = plugin.info().midi_out;
        let module = match key {
            SlotKey::Module(m) => m,
            SlotKey::Instrument => ModuleId::default(),
        };
        // Room for the per-param bookkeeping now (off the audio thread), so `apply_values` only
        // fills it in place unless the stored params later grow past it.
        let n = ins.params.len().max(plugin.info().params.len());
        // A hosted instrument restored from its stored state starts in step with the session:
        // the state (captured from the live instance: its preset, the edits in its editor) is
        // what the player heard, and the insert's stored values (the scan's defaults until an
        // editor edit or a restart reports otherwise) are taken as already applied rather than
        // pushed over it. A hosted module keeps the documented order (state, then the session's
        // values, which its editor edits keep in step). Only a value the session changes from
        // here on is sent.
        let (applied, laned, keys) = if third && key == SlotKey::Instrument && ins.state.is_some() {
            let mut applied = Vec::with_capacity(n);
            applied.extend(ins.params.values().copied());
            let mut laned = Vec::with_capacity(n);
            laned.resize(ins.params.len(), false);
            (applied, laned, keys_hash(ins))
        } else {
            (Vec::with_capacity(n), Vec::with_capacity(n), 0)
        };
        Some(PluginSlot {
            module,
            serial: *self.serial,
            id: ins.plugin.clone(),
            ch,
            state_load: ins.state_load,
            plugin,
            applied,
            laned,
            keys,
            failed: false,
            prepared: 0,
            assets: Vec::new(),
            link: None,
            llm_off: false,
            llm_pos: LLM_SETTLED,
            llm_fresh: true,
            emits,
            midi: MidiOut::new(),
        })
    }
}

impl MixEngine {
    /// The module MIDI (`Route::Modules`) track `t`'s instrument was given in the last block
    /// rendered: `(offset within the block, message)`, in delivery order. Empty for any other
    /// input. Offline printing reads it block by block; never allocates.
    pub fn module_midi_in(&self, t: TrackId) -> impl Iterator<Item = (u32, [u8; 3])> + '_ {
        self.strips.get(&t).into_iter().flat_map(Strip::module_midi_in)
    }

    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        let max_block = max_block.clamp(16, 16_384);
        MixEngine {
            sample_rate: if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 },
            max_block,
            strips: HashMap::new(),
            busses: HashMap::new(),
            main: vec![vec![0.0; max_block]; MAIN_CHANNELS],
            main_fmt: ChannelFormat::Stereo,
            hw: Vec::new(),
            hw_delay: Vec::new(),
            master_latency: 0,
            llm: LlmScope::default(),
            solo: SoloScope::default(),
            full_latency: 0,
            full_master_latency: 0,
            offline: false,
            meters: HashMap::new(),
            main_meter: StripMeter::default(),
            pan_law: PanLaw::Minus3,
            last_end: 0,
            last_start: 0,
            last_stopped: false,
            last_ports: [0; MAX_MIDI_PORTS],
            ports_gone: 0,
            synced: (0, 0, 0),
            order: Vec::new(),
            latency: 0,
            track_latency: 0,
            aux_fed: Vec::new(),
            aux_latency: 0,
            input: Vec::new(),
            live_midi: LiveMidi::new(),
            monitor_only: false,
            recording: false,
            metronome: false,
            countoff: None,
            external: false,
            adopt: HashMap::new(),
            retired: Vec::new(),
            lent: false,
            parked: HashMap::new(),
            retired_analysis: Vec::new(),
            retired_assets: Vec::new(),
            regrow: false,
            serial: 0,
            work: Vec::new(),
            block: 0,
            tail_on: false,
            tail_age: 0,
            tail_quiet: 0,
            links: Vec::new(),
            retired_links: Vec::new(),
            scout: None,
            band: None,
            band_update: None,
            band_spent: Default::default(),
            band_retired: soundcraft_band::runtime::Retired::default(),
            band_out: band::new_slots(),
            band_playing: false,
        }
    }

    /// Arms a Scout capture (built off the audio thread with [`scout::ScoutRig::new`]): the next
    /// render hands each strip its taps (the master's stay in the rig) and starts feeding them.
    /// No allocation. While another capture is armed the rig is refused and handed back, so it is
    /// never dropped here.
    #[must_use = "a refused rig must be handed back, not dropped on the audio thread"]
    pub fn arm_scout(&mut self, rig: Box<scout::ScoutRig>) -> Option<Box<scout::ScoutRig>> {
        if self.scout.is_some() {
            return Some(rig);
        }
        self.scout = Some(rig);
        None
    }

    /// An instrument is still releasing what its live input held (see `Strip::live_tail`), or the
    /// mix is still sounding after it (its inserts, sends and aux returns: see `tail_on`): the
    /// host keeps rendering, stopped, until it is done. No allocation.
    pub fn live_tail(&self) -> bool {
        self.tail_on || self.strips.values().any(|s| s.live_tail > 0)
    }

    /// A Scout capture is armed (running, or ended and not yet handed back).
    pub fn scout_armed(&self) -> bool {
        self.scout.is_some()
    }

    /// Ends the armed capture; [`MixEngine::scout_done`] hands it back. No allocation.
    pub fn stop_scout(&mut self) {
        if let Some(r) = &mut self.scout {
            r.stop();
        }
    }

    /// The armed capture, once it has ended (its 30 s budget filled, its `end` reached, or
    /// [`MixEngine::stop_scout`]), with every strip's taps moved back into it. Returned once; the
    /// caller finishes it off the audio thread ([`scout::ScoutRig::finish`]). No allocation.
    #[must_use = "the finished rig must be finished off the audio thread, not dropped"]
    pub fn scout_done(&mut self) -> Option<Box<scout::ScoutRig>> {
        if !self.scout.as_ref().is_some_and(|r| r.finished()) {
            return None;
        }
        let mut rig = self.scout.take()?;
        for st in self.strips.values_mut() {
            if let Some(t) = st.scout.take() {
                rig.put_back(t);
            }
        }
        Some(rig)
    }

    /// Address and capacity of every heap block of the armed capture and its taps, wherever they
    /// sit, to check capturing does not reallocate.
    #[doc(hidden)]
    pub fn scout_buffers(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if let Some(r) = &self.scout {
            r.heap_blocks(&mut out);
        }
        for st in self.strips.values() {
            if let Some(t) = &st.scout {
                t.heap_blocks(&mut out);
            }
        }
        out
    }

    /// Hands the armed rig's taps to their strips (after `sync`, so every strip exists). The
    /// master's stay in the rig. No allocation.
    fn place_scout(&mut self, s: &Session) {
        let Some(rig) = self.scout.as_mut().filter(|r| !r.placed) else { return };
        // A track deleted between arming and this first render never had a strip to leave.
        rig.mark_absent(s);
        for t in s.tracks.iter().filter(|t| t.kind != TrackKind::Master) {
            if let Some(st) = self.strips.get_mut(&t.id)
                && st.scout.is_none()
                && let Some(taps) = rig.take_taps(t.id)
            {
                st.scout = Some(taps);
            }
        }
        rig.placed = true;
    }

    /// Realtime mode: third-party plugins are created by the host and handed over with
    /// [`MixEngine::adopt`] instead of being created in `sync` (see the crate docs).
    pub fn set_external_instances(&mut self, on: bool) {
        self.external = on;
    }

    /// Takes instances created off the audio thread; the next render's `sync` places them.
    /// An earlier instance waiting for the same slot is retired.
    pub fn adopt(&mut self, instances: Vec<PreparedInstance>) {
        for p in instances {
            if let Some(old) = self.adopt.insert((p.track, p.slot), p) {
                self.retired.push(old.plugin);
            }
        }
        // Force a re-sync so waiting slots pick their instance up.
        self.synced = (usize::MAX, usize::MAX, usize::MAX);
    }

    /// Serial of the instance a chain module currently has (`None` without one). A module that
    /// keeps its instance across a sync keeps its serial; a re-created instance gets a new one.
    #[doc(hidden)]
    pub fn instance_serial(&self, t: TrackId, m: ModuleId) -> Option<u64> {
        self.strips.get(&t)?.plugins.iter().flatten().find(|p| p.module == m).map(|p| p.serial)
    }

    /// How long the track's chain rings on after its input stops: the sum of its modules' tails
    /// (bypassed and inactive modules included, as a guess erring long). 0 for an unknown track.
    pub fn chain_tail(&self, t: TrackId) -> usize {
        self.strips.get(&t).map_or(0, |st| st.plugins.iter().flatten().fold(0usize, |n, p| n.saturating_add(p.plugin.tail_samples())))
    }

    /// Address and capacity of every channel buffer of a strip's delay-compensation lines, to
    /// check they are not reallocated while rendering.
    #[doc(hidden)]
    pub fn delay_buffers(&self, t: TrackId) -> Vec<(usize, usize)> {
        let Some(st) = self.strips.get(&t) else { return Vec::new() };
        [&st.align, &st.align_pre, &st.out_extra].iter().flat_map(|l| l.buffer_ids()).collect()
    }

    /// Address and capacity of every buffer of a strip's sidechain keys (key buffers and their
    /// alignment lines), to check they are not reallocated while rendering.
    #[doc(hidden)]
    pub fn key_buffers(&self, t: TrackId) -> Vec<(usize, usize)> {
        let Some(st) = self.strips.get(&t) else { return Vec::new() };
        st.keys
            .iter()
            .flat_map(|k| {
                k.buf
                    .iter()
                    .map(|b| (b.as_ptr() as usize, b.capacity()))
                    .chain(k.delay.buffer_ids())
                    .chain(k.main_delay.buffer_ids())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Address and capacity of every buffer a strip renders with (delay lines, output, meters,
    /// analysis handles, released assets, each plugin's parameter cache and assets), plus each
    /// plugin's serial, to check rendering neither reallocates nor re-syncs.
    #[doc(hidden)]
    pub fn strip_buffers(&self, t: TrackId) -> Vec<(usize, usize)> {
        let Some(st) = self.strips.get(&t) else { return Vec::new() };
        let mut out: Vec<(usize, usize)> = [&st.align, &st.align_pre, &st.out_extra].iter().flat_map(|l| l.buffer_ids()).collect();
        out.extend(st.outbuf.iter().map(|b| (b.as_ptr() as usize, b.capacity())));
        out.push((st.outbuf.as_ptr() as usize, st.outbuf.capacity()));
        out.push((st.mods.as_ptr() as usize, st.mods.capacity()));
        out.push((st.analysis.as_ptr() as usize, st.analysis.capacity()));
        out.push((st.released.as_ptr() as usize, st.released.capacity()));
        out.extend(st.llm_dry.iter().map(|b| (b.as_ptr() as usize, b.capacity())));
        out.push((st.llm_dry.as_ptr() as usize, st.llm_dry.capacity()));
        out.push((st.plugins.as_ptr() as usize, st.plugins.capacity()));
        for slot in st.plugins.iter().flatten() {
            out.push((slot.serial as usize, 0));
            out.push((slot.applied.as_ptr() as usize, slot.applied.capacity()));
            out.push((slot.assets.as_ptr() as usize, slot.assets.capacity()));
        }
        out
    }

    /// Address and capacity of the engine-wide buffers rendering reuses (the low-latency path
    /// and the hardware outputs' delay lines), to check rendering does not reallocate them.
    #[doc(hidden)]
    pub fn engine_buffers(&self) -> Vec<(usize, usize)> {
        let l = &self.llm;
        let mut out = vec![
            (l.tracks.as_ptr() as usize, l.tracks.capacity()),
            (l.fed.as_ptr() as usize, l.fed.capacity()),
            (l.live.as_ptr() as usize, l.live.capacity()),
            (self.hw_delay.as_ptr() as usize, self.hw_delay.capacity()),
        ];
        out.extend(self.hw_delay.iter().flat_map(DelayLine::buffer_ids));
        out
    }

    /// Sends a one-off action ([`Plugin::action`]: FireStation LevelUp's `"reset_delivery"`) to
    /// the live instance of module `module` on `track`. `false` when there is no such instance or
    /// its plugin has no such action. Allocation-free: callable on the audio thread.
    pub fn module_action(&mut self, track: TrackId, module: ModuleId, name: &str) -> bool {
        let Some(strip) = self.strips.get_mut(&track) else { return false };
        strip.plugins.iter_mut().flatten().find(|p| p.module == module).is_some_and(|p| p.plugin.action(name))
    }

    /// The value bus of link group `group` as of the last sync (`None`: no module is in it).
    pub fn link_slab(&self, group: u32) -> Option<&Arc<LinkSlab>> {
        self.links.iter().find(|g| g.group == group).map(|g| &g.slab)
    }

    /// The member index module `module` on `track` holds in its link group (`None`: not linked).
    pub fn link_member(&self, track: TrackId, module: ModuleId) -> Option<usize> {
        self.links.iter().find_map(|g| g.member_of(track, module))
    }

    /// Link slabs taken out of service since the last call (replaced by a longer one, a group
    /// emptied, or held by a retired instance's slot): drop them off the audio thread. The engine
    /// keeps every slab it handed to a plugin until then, so a plugin letting go never frees one.
    /// Does not allocate.
    pub fn take_retired_links(&mut self) -> Vec<Arc<LinkSlab>> {
        std::mem::take(&mut self.retired_links)
    }

    /// Instances taken out of service since the last call (drop them off the audio thread).
    /// Does not allocate.
    pub fn take_retired(&mut self) -> Vec<Box<dyn Plugin>> {
        std::mem::take(&mut self.retired)
    }

    /// Analysis handles of the chain modules taken out of service since the last call, with the
    /// engine's own copies of them (strip and meter), to drop off the audio thread like the
    /// instances. A host that copies the meters elsewhere (a meter snapshot) should hand these
    /// over only after refreshing that copy, so the copy never holds the last reference.
    /// Does not allocate.
    pub fn take_retired_analysis(&mut self) -> Vec<Arc<soundcraft_dsp::analysis::Analysis>> {
        std::mem::take(&mut self.retired_analysis)
    }

    /// Module assets taken out of service since the last call: replaced or cleared ones, those of
    /// retired instances, and those the plugins let go of while rendering (an IR a crossfade has
    /// finished with). Drop them off the audio thread. Does not allocate or free: a strip's
    /// released list goes out whole (its reserved Vec, swapped for an empty one; the next `sync`
    /// reserves again), and one that does not fit the spare capacity already taken waits for the
    /// next call.
    pub fn take_retired_assets(&mut self) -> Vec<Arc<dyn SampleSource>> {
        // An empty list keeps its capacity here: handing it out would free it on the caller's side.
        let mut out = if self.retired_assets.is_empty() { Vec::new() } else { std::mem::take(&mut self.retired_assets) };
        for st in self.strips.values_mut() {
            if st.released.is_empty() {
                continue;
            }
            if out.is_empty() {
                std::mem::swap(&mut out, &mut st.released);
            } else if out.capacity() - out.len() >= st.released.len() {
                out.append(&mut st.released);
            }
        }
        out
    }

    /// Queues analysis handles (of another engine this one replaces) to come back out of
    /// [`MixEngine::take_retired_analysis`].
    pub fn keep_retired_analysis(&mut self, handles: Vec<Arc<soundcraft_dsp::analysis::Analysis>>) {
        self.retired_analysis.extend(handles);
    }

    /// Every instance this engine holds (strips, instruments, clip effects excluded) plus the
    /// retired and waiting ones, leaving the engine empty: hand them to another thread to drop
    /// before replacing the engine. Their analysis handles move to [`MixEngine::take_retired_analysis`],
    /// the link slabs to [`MixEngine::take_retired_links`].
    pub fn drain_plugins(&mut self) -> Vec<Box<dyn Plugin>> {
        let mut out = std::mem::take(&mut self.retired);
        for st in self.strips.values_mut() {
            for slot in st.plugins.iter_mut().flatten() {
                self.retired_assets.extend(slot.assets.drain(..).map(|(_, a)| a));
                self.retired_links.extend(slot.link.take().map(|(l, _)| l));
            }
            self.retired_assets.append(&mut st.released);
            out.extend(st.plugins.drain(..).flatten().map(|s| s.plugin));
            out.extend(st.instrument.take().map(|s| s.plugin));
            self.retired_analysis.extend(st.analysis.drain(..).map(|(_, a)| a));
        }
        for m in self.meters.values_mut() {
            self.retired_analysis.extend(m.analysis.drain(..).map(|(_, a)| a));
        }
        out.extend(self.adopt.drain().map(|(_, p)| p.plugin));
        out.extend(self.parked.drain().map(|(_, p)| p.plugin));
        self.retired_links.extend(self.links.drain(..).map(|g| g.slab));
        self.synced = (usize::MAX, usize::MAX, usize::MAX);
        out
    }

    /// What the last `render` put on each hardware output, indexed like `Session::outputs`
    /// (index 0, the main, is empty). The player maps them onto device channels. Each channel
    /// holds `max_block` samples, of which the rendered frames lead.
    pub fn hardware_out(&self) -> &[Vec<Vec<f32>>] {
        &self.hw
    }

    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Format of the main mix as of the last sync.
    pub fn main_format(&self) -> ChannelFormat {
        self.main_fmt
    }

    /// Channels of the main mix as of the last sync.
    pub fn main_channels(&self) -> usize {
        self.main.len()
    }

    /// [`MixEngine::reset`] at timeline position `pos` (Play, Stop, Locate): every module first
    /// takes its parameters as automation has them at `pos` (as the first block there would set
    /// them), so a plugin's reset seats its smoothers at those values instead of gliding from
    /// wherever the transport was before. Does not allocate (unless a module's stored params grew
    /// since its last block, as rendering would).
    pub fn reset_at(&mut self, s: &Session, pos: Samples) {
        for t in &s.tracks {
            let Some(st) = self.strips.get_mut(&t.id) else { continue };
            for (ins, slot) in t.mixer.inserts.iter().zip(st.plugins.iter_mut()) {
                if let Some(slot) = slot.as_mut().filter(|p| p.module == ins.id && p.id == ins.plugin) {
                    apply_params(slot, ins, t, Some(ins.id), pos);
                }
            }
            if let (Some(ins), Some(slot)) = (&t.instrument, st.instrument.as_mut())
                && slot.id == ins.plugin
            {
                apply_params(slot, ins, t, None, pos);
            }
        }
        self.reset();
    }

    /// Reset all plugin state (on stop / seek). Prefer [`MixEngine::reset_at`], which seats the
    /// plugins at the automation of the new position.
    pub fn reset(&mut self) {
        for s in self.strips.values_mut() {
            for p in s.plugins.iter_mut().flatten() {
                p.plugin.reset();
                p.failed = false;
                p.llm_pos = LLM_SETTLED;
                p.llm_fresh = true;
            }
            if let Some(i) = &mut s.instrument {
                i.plugin.all_notes_off();
                i.plugin.reset();
                i.failed = false;
            }
            s.held.clear();
            s.midi_held.clear();
            s.chase = true;
            // Every voice was just let go: no release is left to run.
            s.live_tail = 0;
            s.tail_quiet = 0;
            s.align.clear();
            s.align_pre.clear();
            s.out_extra.clear();
            for k in &mut s.keys {
                k.delay.clear();
                k.main_delay.clear();
                k.buf.iter_mut().for_each(|b| b.fill(0.0));
            }
            for fx in s.clip_fx.values_mut() {
                fx.reset();
            }
        }
        self.hw_delay.iter_mut().for_each(DelayLine::clear);
        self.meters.clear();
        self.main_meter = StripMeter::default();
        self.tail_on = false;
    }

    /// Bring plugin instances in line with the session (creates/destroys as needed). Allocates;
    /// call outside the realtime thread when the document changes, or let `render` do it.
    pub fn sync(&mut self, s: &Session) {
        let mb = self.max_block;
        let main_ch = main_channels(s);
        self.main_fmt = s.main_format();
        if self.main.len() != main_ch {
            self.main = vec![vec![0.0; mb]; main_ch];
        }
        self.hw.resize_with(s.outputs.len(), Vec::new);
        self.hw_delay.resize_with(s.outputs.len(), DelayLine::default);
        self.llm.reserve(s);
        self.solo.reserve(s);
        for (i, (h, o)) in self.hw.iter_mut().zip(&s.outputs).enumerate() {
            let n = if i == 0 { 0 } else { o.format.channels().clamp(1, MAX_CHANNELS) };
            if h.len() != n || h.iter().any(|c| c.len() != mb) {
                *h = vec![vec![0.0; mb]; n];
            }
        }
        let mut cx = SlotCtx {
            sr: self.sample_rate,
            mb,
            external: self.external,
            offline: self.offline,
            adopt: &mut self.adopt,
            retired: &mut self.retired,
            retired_analysis: &mut self.retired_analysis,
            retired_assets: &mut self.retired_assets,
            retired_links: &mut self.retired_links,
            serial: &mut self.serial,
            lent: self.lent,
            parked: &mut self.parked,
        };
        let plan = sidechain::resolve(s);
        // Link groups: kept for groups still in use (their values and member indices stay), made
        // for new ones; replaced and unused slabs are retired, never dropped here.
        let old_links = std::mem::take(&mut self.links);
        self.links = link::sync_groups(s, &plan.links, old_links, cx.retired_links);
        let gone: Vec<TrackId> = self.strips.keys().filter(|id| s.track(**id).is_none()).copied().collect();
        for id in gone {
            if let Some(mut st) = self.strips.remove(&id) {
                // Scout taps of a removed strip go back to the rig, reported as removed (the
                // master's never left it) unless the capture had already ended.
                if let Some(rig) = self.scout.as_mut() {
                    rig.mark_removed(id);
                    if let Some(taps) = st.scout.take() {
                        rig.orphan(taps);
                    }
                }
                for p in st.plugins.drain(..) {
                    let key = p.as_ref().map_or(SlotKey::Instrument, |p| SlotKey::Module(p.module));
                    cx.retire(id, key, p);
                }
                cx.retire(id, SlotKey::Instrument, st.instrument.take());
                cx.retired_analysis.extend(st.analysis.drain(..).map(|(_, a)| a));
            }
            // The meter keeps its copies of the handles: hand those over too.
            if let Some(m) = self.meters.get_mut(&id) {
                cx.retired_analysis.extend(m.analysis.drain(..).map(|(_, a)| a));
            }
        }
        for (ti, t) in s.tracks.iter().enumerate() {
            let ch = strip_channels(t, main_ch);
            let strip = self.strips.entry(t.id).or_insert_with(Strip::new);
            if strip.buf.len() != ch || strip.buf.first().map_or(0, Vec::len) != mb {
                strip.buf = vec![vec![0.0; mb]; ch];
            }
            // Sidechain keys: one per keyed module, keeping a module's lines (and their history).
            strip.level = plan.level.get(ti).copied().unwrap_or(0);
            // Module MIDI inputs: the accepted sources, and room for the list (`process_strip`
            // copies it without allocating).
            let sources = t.mixer.input.module_sources();
            strip.midi_src.clear();
            strip.midi_src.extend(
                sources
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| plan.midi_ok(t.id, **m))
                    .filter_map(|(k, m)| Some((u8::try_from(k).ok()?, s.tracks.iter().position(|x| x.id == m.track)?, m.module))),
            );
            strip.module_in.reserve(sources.len());
            strip.pre_key = false;
            let mut old_keys = std::mem::take(&mut strip.keys);
            for ins in &t.mixer.inserts {
                let Some(sc) = ins.sidechain else { continue };
                let mut k = match old_keys.iter().position(|k| k.module == ins.id) {
                    Some(at) => old_keys.swap_remove(at),
                    None => KeySlot::new(ins.id),
                };
                k.pre_fader = sc.pre_fader;
                k.source = plan.source(s, t.id, ins.id);
                if k.buf.len() != ch || k.buf.iter().any(|b| b.len() != mb) {
                    k.buf = vec![vec![0.0; mb]; ch];
                }
                strip.keys.push(k);
            }
            // Match instances to modules by id: a moved module keeps its instance, a removed one's
            // is retired, a new one gets a new instance.
            let mut old = std::mem::take(&mut strip.plugins);
            let mut chain = Vec::with_capacity(t.mixer.inserts.len());
            for ins in &t.mixer.inserts {
                let key = SlotKey::Module(ins.id);
                let mut slot = old.iter_mut().find_map(|p| p.take_if(|p| p.module == ins.id));
                // Without a host making instances (offline renders), a preset loaded into a
                // hosted module (a new `state_load`) makes its instance anew from the state; the
                // realtime host loads it into its instance in place.
                let reload = |p: &PluginSlot| !cx.external && p.state_load != ins.state_load;
                if slot.as_ref().is_none_or(|p| p.id != ins.plugin || p.ch != ch || reload(p)) || cx.waiting(t.id, key) {
                    cx.retire(t.id, key, slot.take());
                    slot = cx.make(t.id, key, ins, ch);
                }
                chain.push(slot);
            }
            for p in old {
                let key = p.as_ref().map_or(SlotKey::Instrument, |p| SlotKey::Module(p.module));
                cx.retire(t.id, key, p);
            }
            let rate = s.sample_rate.hz();
            for (slot, ins) in chain.iter_mut().zip(&t.mixer.inserts) {
                if let Some(slot) = slot.as_mut() {
                    sync_assets(slot, ins, s, rate, cx.retired_assets);
                    sync_link(slot, t.id, ins, &self.links, cx.retired_links);
                }
            }
            if strip.released.capacity() < RELEASED_RESERVE {
                strip.released.reserve(RELEASED_RESERVE);
            }
            strip.plugins = chain;
            strip.mods.resize(t.mixer.inserts.len(), ModuleMeter::default());
            // Handles of instances that left the chain go with them, so the last reference is
            // never dropped on the audio thread (see `take_retired_analysis`).
            let old_analysis = std::mem::take(&mut strip.analysis);
            strip.analysis.extend(strip.plugins.iter().flatten().filter_map(|p| Some((p.module, p.plugin.analysis()?))));
            for (_, a) in old_analysis {
                if !strip.analysis.iter().any(|(_, b)| Arc::ptr_eq(&a, b)) {
                    cx.retired_analysis.push(a);
                }
            }
            // The meter's copies of retired handles go now, while the retired ones are still held
            // (an inactive track's meter is not refreshed by `render`).
            if let Some(m) = self.meters.get_mut(&t.id) {
                m.analysis.clone_from(&strip.analysis);
            }
            for (m, ins) in strip.mods.iter_mut().zip(&t.mixer.inserts) {
                m.module = ins.id;
            }
            let want = t.instrument.as_ref();
            let ich = ch.max(2);
            let load = |l: u64| if cx.external { 0 } else { l };
            if strip.instrument.as_ref().map(|p| (p.id.as_str(), p.ch, load(p.state_load)))
                != want.map(|i| (i.plugin.as_str(), ich, load(i.state_load)))
                || (want.is_some() && cx.waiting(t.id, SlotKey::Instrument))
            {
                let old = strip.instrument.take();
                cx.retire(t.id, SlotKey::Instrument, old);
                strip.instrument = want.and_then(|i| cx.make(t.id, SlotKey::Instrument, i, ich));
                strip.sample = None;
            }
            sync_sample(strip, want, s);
            let n = if t.mixer.inserts.is_empty() { 0 } else { ch };
            if strip.llm_dry.len() != n || strip.llm_dry.iter().any(|b| b.len() != mb) {
                strip.llm_dry = vec![vec![0.0; mb]; n];
            }
            strip.dest = out_dest(s, &t.mixer.output);
            // A named input (or a legacy "In N" / "In N-M") gives its device channels; no input
            // is "In 1" (a stereo track hears device channel 1 on both sides); any other route
            // gives track channel k device channel k.
            strip.in_span = match &t.mixer.input {
                Route::Hardware(h) => s.input_channels(h),
                // The Band's musicians and a module's MIDI have no audio input: they hear what
                // "no input" hears.
                Route::None | Route::Musician(_) | Route::Modules(_) | Route::Midi(_) => Some((0, 1)),
                _ => Some((0, ch)),
            };
        }
        // Sources tapped before their fader keep their pre-fader copy, sized here.
        for r in plan.routes.iter().filter(|r| r.pre_fader && r.result.is_ok()) {
            if let Some(st) = self.strips.get_mut(&r.source) {
                st.pre_key = true;
                let ch = st.buf.len();
                if st.pre.len() != ch || st.pre.iter().any(|b| b.capacity() < mb) {
                    st.pre = vec![Vec::with_capacity(mb); ch];
                }
            }
        }
        // Instances nobody claimed (the session changed again before they arrived). While lent
        // they wait for the host's session instead (see `parked`).
        let leftovers: Vec<(TrackId, SlotKey)> = if cx.lent { Vec::new() } else { cx.adopt.keys().copied().collect() };
        for k in leftovers {
            if let Some(p) = cx.adopt.remove(&k) {
                cx.retired.push(p.plugin);
            }
        }
        if !self.external {
            self.retired.clear();
            self.retired_analysis.clear();
            self.retired_assets.clear();
            self.retired_links.clear();
            for st in self.strips.values_mut() {
                st.released.clear();
            }
        }
        for b in &s.busses {
            let n = b.format.channels().clamp(1, MAX_CHANNELS);
            let bus = self.busses.entry(b.id).or_insert_with(|| BusBuf { fmt: b.format, buf: Vec::new() });
            bus.fmt = b.format;
            if bus.buf.len() != n {
                bus.buf = vec![vec![0.0; mb]; n];
            }
        }
        self.busses.retain(|id, _| s.bus(*id).is_some());
        self.order = plan.order;
        self.reserve_delays(s);
    }

    /// Crossfade length between delay taps, in samples.
    fn fade_len(&self) -> usize {
        (CROSSFADE_MS * 0.001 * self.sample_rate).round().max(1.0) as usize
    }

    /// Sizes every strip's delay-compensation lines for the current latency plus `HEADROOM_S`,
    /// so a latency change while playing re-aligns without allocating. Lines only grow (keeping
    /// their history); lines a strip does not use stay empty. Allocates: part of `sync`.
    fn reserve_delays(&mut self, s: &Session) {
        let headroom = (HEADROOM_S * self.sample_rate) as usize;
        // Room for the compensation with low-latency monitoring on and off alike.
        let base = self.track_latency.max(self.aux_latency).max(self.full_latency);
        let cap = |l: &DelayLine| base.max(l.need()).saturating_add(headroom).min(MAX_LATENCY);
        let mb = self.max_block;
        for t in &s.tracks {
            let Some(st) = self.strips.get_mut(&t.id) else { continue };
            // Strips `render` never delays (a reactivated track re-syncs).
            if t.inactive || matches!(t.kind, TrackKind::Master | TrackKind::Vca) {
                continue;
            }
            let ch = st.buf.len();
            st.align.reserve(cap(&st.align), ch);
            for k in &mut st.keys {
                k.delay.reserve(cap(&k.delay), ch);
                k.main_delay.reserve(cap(&k.main_delay), ch);
            }
            if t.mixer.sends.iter().flatten().any(|x| x.pre_fader) || st.align_pre.active() {
                st.align_pre.reserve(cap(&st.align_pre), ch);
            }
            let aux = matches!(t.kind, TrackKind::Aux | TrackKind::Folder);
            if !aux && matches!(t.mixer.output, Route::Main | Route::Bus(_) | Route::Hardware(_)) {
                st.out_extra.reserve(cap(&st.out_extra), ch);
                if st.outbuf.len() != ch || st.outbuf.iter().any(|b| b.capacity() < mb) {
                    st.outbuf = vec![Vec::with_capacity(mb); ch];
                }
            }
        }
        let mcap = |l: &DelayLine| self.master_latency.max(self.full_master_latency).max(l.need()).saturating_add(headroom).min(MAX_LATENCY);
        for (h, l) in self.hw.iter().zip(self.hw_delay.iter_mut()) {
            if !h.is_empty() {
                let c = mcap(l);
                l.reserve(c, h.len());
            }
        }
        self.regrow = false;
    }

    /// Re-syncs plugin instances and the routing order when the session's structure changed
    /// since the last sync (or instances were adopted). `render` calls it; a realtime host may
    /// also call it right after swapping sessions so slots are filled before playback starts.
    pub fn ensure_synced(&mut self, s: &Session) {
        let key = (structure_fingerprint(s), s.tracks.len(), s.busses.len());
        if key != self.synced {
            // `sync` also takes the rendering order from its sidechain plan.
            self.sync(s);
            self.synced = key;
        } else if self.regrow {
            self.reserve_delays(s);
        }
    }

    /// Render `frames` (≤ max_block) samples starting at `pos` into `out` (≥ 2 channels).
    pub fn render(&mut self, s: &Session, pos: Samples, frames: usize, out: &mut [Vec<f32>]) {
        let frames = frames.min(self.max_block);
        let stopped = self.monitor_only;
        // Stopped, the transport stays where it is: the same block again is no seek (a held live
        // note would be cut every block). A Locate resets the engine itself (`reset_at`).
        let still = stopped && self.last_stopped && pos == self.last_start;
        let seek = pos != self.last_end && !still;
        if seek {
            // Seek (a locate, a loop wrap): silence held notes and ringing plugins. A strip a
            // live input holds notes on lets go of its clips' notes only: the player's keys
            // still down keep sounding (across a loop wrap too) until they come up.
            for (id, st) in self.strips.iter_mut() {
                if st.live_held.is_empty() {
                    if let Some(i) = &mut st.instrument {
                        i.plugin.all_notes_off();
                    }
                    st.held.clear();
                    st.midi_held.clear();
                } else {
                    release_clip_notes(s, *id, st);
                }
                st.chase = true;
            }
        } else if stopped && !self.last_stopped {
            // The transport stopped by itself (the end of play) with clip notes sounding: they
            // never get their note-offs from the clips now. Live notes keep sounding.
            for (id, st) in self.strips.iter_mut() {
                if !st.held.is_empty() || !st.midi_held.is_empty() {
                    release_clip_notes(s, *id, st);
                    st.chase = true;
                }
            }
        }
        self.last_end = pos.saturating_add(frames as i64);
        self.last_start = pos;
        self.last_stopped = stopped;
        self.ports_gone = 0;
        for (k, (old, now)) in self.last_ports.iter().zip(self.live_midi.ports).enumerate() {
            if *old != 0 && *old != now {
                self.ports_gone |= 1 << k;
            }
        }
        self.last_ports = self.live_midi.ports;
        self.ensure_synced(s);
        self.place_scout(s);
        // The band plays once for the block, before the strips (they only read its slots).
        self.render_band(s, pos, frames, seek);
        // Low-latency monitoring: the live path of this block (never in offline renders).
        let mut scope = std::mem::take(&mut self.llm);
        // The flag is read here, not in `sync`: a toggle while tracking never re-syncs.
        if !self.offline && s.edit.flag(LLM_FLAG) {
            llm_scope(s, &mut scope);
        } else {
            scope.clear();
        }
        let on_path = |i: usize| scope.on(i);
        // The frames of this block a Scout capture measures (offset, count), the same on every tap.
        let scout_win = self.scout.as_ref().and_then(|r| r.window(pos, frames, !self.monitor_only));
        // Captured strips that do not render this block (inactive, or a VCA) say so in their note
        // instead of silently missing frames.
        if scout_win.is_some() {
            for t in s.tracks.iter().filter(|t| t.inactive || t.kind == TrackKind::Vca) {
                if let Some(taps) = self.strips.get_mut(&t.id).and_then(|st| st.scout.as_deref_mut()) {
                    taps.mark_inactive();
                } else if let Some(taps) = self.scout.as_mut().and_then(|r| r.taps_for(t.id)) {
                    taps.mark_inactive();
                }
            }
        }
        for b in self.busses.values_mut() {
            for c in b.buf.iter_mut() {
                c.iter_mut().take(frames).for_each(|x| *x = 0.0);
            }
        }
        for c in self.main.iter_mut().chain(self.hw.iter_mut().flatten()) {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        // Solo follows the signal path; read every block (a solo toggle never re-syncs).
        let mut solo = std::mem::take(&mut self.solo);
        solo.fill(s);
        let order = std::mem::take(&mut self.order);
        let live = |t: &Track| !(t.kind == TrackKind::Master || t.inactive || t.kind == TrackKind::Vca);
        let is_aux = |t: &Track| matches!(t.kind, TrackKind::Aux | TrackKind::Folder);
        // Phase 1: independent strips (audio, instrument, MIDI) in parallel.
        let mut work = std::mem::take(&mut self.work);
        work.clear();
        for &i in &order {
            let Some(t) = s.tracks.get(i).filter(|t| live(t) && !is_aux(t)) else { continue };
            if let Some(mut st) = self.strips.remove(&t.id) {
                st.llm = on_path(i);
                work.push((i, st));
            }
        }
        for (_, st) in work.iter_mut() {
            if st.scratch.len() < self.max_block {
                st.scratch.resize(self.max_block, 0.0);
            }
        }
        self.block = self.block.wrapping_add(1);
        let block = self.block;
        let pdc = s.edit.delay_compensation;
        let xf = self.fade_len();
        let busses = std::mem::take(&mut self.busses);
        let live_in = LiveInput {
            input: &self.input,
            monitor_only: self.monitor_only,
            recording: self.recording,
            pdc,
            xf,
            scout: scout_win,
            band: &self.band_out,
            midi: &self.live_midi,
            ports_gone: self.ports_gone,
        };
        // In levels (`order` sorts them): a strip keyed by another renders after it, each level in
        // parallel. Without sidechains there is one level.
        let mut at = 0;
        while at < work.len() {
            let level = work.get(at).map_or(0, |(_, st)| st.level);
            let end = at + work.get(at..).map_or(0, |w| w.iter().take_while(|(_, st)| st.level == level).count()).max(1);
            let (done, rest) = work.split_at_mut(at);
            let Some(run) = rest.get_mut(..end - at) else { break };
            for (_, st) in run.iter_mut() {
                st.base = 0;
                for k in &mut st.keys {
                    let src = k.source.and_then(|(si, _)| done.iter().find(|(i, _)| *i == si)).map(|(_, x)| x.key_source());
                    fill_key(k, src, block, frames);
                }
                // Module MIDI inputs: this block's events of each source module (rendered in an
                // earlier level; nothing from one that did not render this block), merged.
                st.midi_in.clear();
                for &(k, si, m) in &st.midi_src {
                    if let Some((_, src)) = done.iter().find(|(i, x)| *i == si && x.stamp == block)
                        && let Some(out) = src.module_midi(m)
                    {
                        st.midi_in.add(k, out);
                    }
                }
                st.midi_in.sort();
            }
            run_strips(run, |(i, st)| {
                if let Some(t) = s.tracks.get(*i) {
                    process_strip(s, t, st, &busses, &live_in, pos, frames, solo.muted(*i));
                }
                st.stamp = block;
            });
            at = end;
        }
        self.busses = busses;
        // Delay compensation: align every strip to the slowest. Direct-to-main outputs also wait
        // for the slowest aux return. Aux latency has to be read before those outputs are routed,
        // or the first block (and a one-block bounce probe) still sees zero.
        // Low-latency monitoring: strips on the live path add no compensation and hold nothing
        // back; the others are compensated among themselves only.
        if pdc {
            for (_, t) in s.tracks.iter().enumerate().filter(|(i, t)| is_aux(t) && !t.inactive && !on_path(*i)) {
                if let Some(st) = self.strips.get_mut(&t.id) {
                    note_insert_latency(t, st, pos);
                }
            }
        }
        // The master faders' inserts delay the whole main mix, so their latency is part of the
        // mix's too (the faders run one after the other: it adds up).
        let mut lmax_m = 0usize;
        if pdc {
            for (_, t) in s.tracks.iter().enumerate().filter(|(i, t)| t.kind == TrackKind::Master && !t.inactive && !on_path(*i)) {
                if let Some(st) = self.strips.get_mut(&t.id) {
                    note_insert_latency(t, st, pos);
                    lmax_m = lmax_m.saturating_add(st.latency);
                }
            }
        }
        let lmax_t = if pdc { work.iter().filter(|(i, _)| !on_path(*i)).map(|(_, st)| st.latency).max().unwrap_or(0) } else { 0 };
        let lmax_a = if pdc {
            s.tracks
                .iter()
                .enumerate()
                .filter(|(i, t)| is_aux(t) && !on_path(*i))
                .filter_map(|(_, t)| self.strips.get(&t.id))
                .map(|st| st.latency)
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        self.track_latency = lmax_t;
        self.latency = lmax_t + lmax_a + lmax_m;
        // With low-latency monitoring off the live strips' bypassed modules would count again.
        self.full_latency = if pdc {
            let full = |st: &Strip| st.latency.saturating_add(st.llm_latency);
            let t_full = work.iter().map(|(_, st)| full(st)).max().unwrap_or(0);
            let a_full = s.tracks.iter().filter(|t| is_aux(t)).filter_map(|t| self.strips.get(&t.id)).map(full).max().unwrap_or(0);
            t_full.max(a_full).min(MAX_LATENCY)
        } else {
            0
        };
        // An aux that returns onto a bus delays it by the slowest aux return, so the tracks routed
        // to that bus wait as well (as the ones routed to the main mix do) and the bus lines up.
        self.aux_latency = lmax_a;
        self.aux_fed.clear();
        if pdc {
            for t in s.tracks.iter().filter(|t| is_aux(t) && live(t)) {
                if let Route::Bus(b) = &t.mixer.output
                    && !self.aux_fed.contains(b)
                {
                    self.aux_fed.push(*b);
                }
            }
        }
        // A changed delay crossfades to its new tap without allocating; one beyond the reserved
        // capacity is clamped until the next render re-sizes the lines (`regrow`).
        let mut fits = true;
        for (i, st) in work.iter_mut() {
            let Some(t) = s.tracks.get(*i) else { continue };
            let ch = st.buf.len();
            fits &= !std::mem::take(&mut st.key_regrow);
            // On the live path: no compensation delay.
            let lmax = if st.llm { 0 } else { lmax_t };
            fits &= st.align.set(lmax.saturating_sub(st.latency), ch, xf);
            st.align.process(&mut st.buf, frames);
            st.post_at = st.latency.max(lmax);
            if t.mixer.sends.iter().flatten().any(|x| x.pre_fader) {
                fits &= st.align_pre.set(lmax.saturating_sub(st.latency), ch, xf);
                st.align_pre.process(&mut st.pre, frames);
                st.pre_at = st.post_at;
            }
            // A hardware output waits for the aux returns as the main mix does, so what it plays
            // lines up with the main mix.
            // So does a bus an aux on the low-latency path reads: that aux adds no compensation.
            let waits = !st.llm
                && match st.dest {
                    Dest::Main | Dest::Hw(_) => true,
                    Dest::Bus(b) => self.aux_fed.contains(&b) || scope.live_bus(b),
                    Dest::None => false,
                };
            fits &= st.out_extra.set(if waits { lmax_a } else { 0 }, ch, xf);
        }
        for (i, st) in work.iter_mut() {
            if let Some(t) = s.tracks.get(*i) {
                self.route_strip(s, t, st, pos, frames);
            }
        }
        for (i, st) in work.drain(..) {
            if let Some(t) = s.tracks.get(i) {
                self.strips.insert(t.id, st);
            }
        }
        self.work = work;
        let is_aux = |t: &Track| matches!(t.kind, TrackKind::Aux | TrackKind::Folder);
        // Phase 2: auxes in dependency order (they read busses).
        for (i, t) in order.iter().filter_map(|&i| Some((i, s.tracks.get(i)?))).filter(|(_, t)| live(t) && is_aux(t)) {
            let Some(mut st) = self.strips.remove(&t.id) else { continue };
            st.llm = on_path(i);
            if st.scratch.len() < self.max_block {
                st.scratch.resize(self.max_block, 0.0);
            }
            let live_in = LiveInput {
                input: &self.input,
                monitor_only: self.monitor_only,
                recording: self.recording,
                pdc,
                xf,
                scout: scout_win,
                band: &self.band_out,
                midi: &self.live_midi,
                ports_gone: self.ports_gone,
            };
            // An aux reads busses that already carry the tracks' compensation delay. An aux
            // without a bus input hears nothing but its own modules; it is compensated as if its
            // input were the tracks' (it returns alongside the other auxes), so its main input
            // counts as `track_latency` too (test `an_aux_consumer_lines_its_key_up_with_the_mix`).
            st.base = match &t.mixer.input {
                Route::Bus(b) if pdc => self.bus_latency(*b),
                _ if pdc => self.track_latency,
                _ => 0,
            };
            for k in &mut st.keys {
                let src = k.source.and_then(|(_, id)| self.strips.get(&id)).map(Strip::key_source);
                fill_key(k, src, block, frames);
            }
            process_strip(s, t, &mut st, &self.busses, &live_in, pos, frames, solo.muted(i));
            st.stamp = block;
            fits &= !std::mem::take(&mut st.key_regrow);
            let ch = st.buf.len();
            let lmax = if pdc && !st.llm { lmax_a } else { 0 };
            fits &= st.align.set(lmax.saturating_sub(st.latency), ch, xf);
            st.align.process(&mut st.buf, frames);
            st.post_at = st.base.saturating_add(st.latency.max(lmax));
            fits &= st.out_extra.set(0, ch, xf);
            self.route_strip(s, t, &mut st, pos, frames);
            self.strips.insert(t.id, st);
        }
        self.order = order;
        self.solo = solo;
        // Master faders process the main mix.
        let transport = transport_at(s, pos, !self.monitor_only, self.recording);
        let mut full_m = 0usize;
        for (mi, t) in s.tracks.iter().enumerate().filter(|(_, t)| t.kind == TrackKind::Master && !t.inactive) {
            if let Some(strip) = self.strips.get_mut(&t.id) {
                strip.llm = on_path(mi);
                for (dst, src) in strip.buf.iter_mut().zip(self.main.iter()) {
                    if let (Some(d), Some(sr)) = (dst.get_mut(..frames), src.get(..frames)) {
                        d.copy_from_slice(sr);
                    }
                }
                // Scout PRE: the main mix as it reaches the master's first insert.
                if let (Some(win), Some(taps)) = (scout_win, self.scout.as_mut().and_then(|r| r.taps_for(t.id))) {
                    taps.feed_pre(&strip.buf, win);
                }
                let mut level = chain_peak(&strip.buf, frames);
                let mut bits = 0u64;
                for (i, ins) in t.mixer.inserts.iter().enumerate() {
                    let mut meter = ModuleMeter { module: ins.id, in_peak: level, out_peak: level, ..ModuleMeter::default() };
                    if let Some(Some(slot)) = strip.plugins.get_mut(i)
                        && slot.module == ins.id
                    {
                        meter.failed = slot.failed;
                        if ins.active && !ins.bypass && !slot.failed {
                            apply_params(slot, ins, t, Some(ins.id), pos);
                            full_m = full_m.saturating_add(slot.plugin.latency());
                            let run = llm_step(slot, strip.llm && slot.plugin.latency() > 0, xf);
                            if slot.llm_off && i < 64 {
                                bits |= 1 << i;
                            }
                            if run == LlmRun::Skip {
                                if let Some(m) = strip.mods.get_mut(i) {
                                    *m = meter;
                                }
                                continue;
                            }
                            if run == LlmRun::Fade {
                                copy_dry(&strip.buf, &mut strip.llm_dry, frames);
                            }
                            slot.plugin.set_transport(&transport);
                            slot.plugin.process(&mut strip.buf, frames);
                            if run == LlmRun::Fade {
                                llm_blend(slot, &mut strip.buf, &strip.llm_dry, frames, xf);
                            }
                            collect_released(slot, &mut strip.released);
                            if contain_failure(slot, &mut strip.buf, frames) {
                                strip.failures.record(SlotKey::Module(ins.id));
                                meter.failed = true;
                            } else {
                                read_readouts(&*slot.plugin, &mut meter.readouts);
                                meter.gr_db = slot.plugin.gain_reduction_db();
                            }
                            level = chain_peak(&strip.buf, frames);
                            meter.out_peak = level;
                        }
                    }
                    if let Some(m) = strip.mods.get_mut(i) {
                        *m = meter;
                    }
                }
                // Scout POST: after the master's last insert, before its fader.
                if let (Some(win), Some(taps)) = (scout_win, self.scout.as_mut().and_then(|r| r.taps_for(t.id))) {
                    taps.feed_post(&strip.buf, win);
                }
                // Match track faders: the next block's first sample must not ramp this one.
                let last = pos.saturating_add(frames.saturating_sub(1) as i64);
                let v0 = volume_db_at(t, pos);
                let v1 = volume_db_at(t, last);
                for (ch, src) in strip.buf.iter().enumerate().take(self.main.len()) {
                    if let Some(m) = self.main.get_mut(ch) {
                        for i in 0..frames {
                            let g = lerp_gain(v0, v1, i, frames.saturating_sub(1));
                            if let (Some(d), Some(x)) = (m.get_mut(i), src.get(i)) {
                                *d = x * g;
                            }
                        }
                    }
                }
                strip.llm_bits = bits;
                let m = self.meters.entry(t.id).or_default();
                m.measure(&self.main, frames, 0.0);
                m.llm_bypassed = bits;
                m.plugin_failures = strip.failures;
                m.modules.clone_from(&strip.mods);
                m.analysis.clone_from(&strip.analysis);
            }
        }
        self.llm = scope;
        // Hardware outputs skip the master faders: they wait for their inserts' latency, so they
        // line up with the main mix.
        self.master_latency = lmax_m;
        self.full_master_latency = if pdc { full_m.min(MAX_LATENCY) } else { 0 };
        for (h, l) in self.hw.iter_mut().zip(self.hw_delay.iter_mut()) {
            if !h.is_empty() {
                fits &= l.set(lmax_m, h.len(), xf);
                l.process(h, frames);
            }
        }
        self.regrow |= !fits;
        // Before the clicks: they are not the mix's tail.
        self.track_tail(s, frames);
        // Metronome: summed after the master fader, so it reaches the speakers without passing
        // through any track, send or insert. Only the playback engine sets `metronome`.
        // Not while monitoring only (transport stopped): the position does not advance there, so the
        // start of the same beat would repeat every block as a buzz.
        if self.metronome && !self.monitor_only && s.edit.click && (!s.edit.flag("click.only_during_record") || self.recording) {
            click::render(s, pos, frames, &mut self.main);
        }
        // The count-off before a take: its own clicks, counted from its start (the transport
        // stands at the record point meanwhile, so the tempo map cannot place them).
        if self.metronome
            && let Some((c, elapsed)) = self.countoff
        {
            click::render_countoff(s, &c, elapsed, frames, &mut self.main);
        }
        // Output: min(out.len(), main channels) channels; any further output channels are silent.
        for (c, o) in out.iter_mut().enumerate() {
            let src = self.main.get(c);
            for i in 0..frames {
                let v = src.and_then(|m| m.get(i)).copied().unwrap_or(0.0);
                if let Some(d) = o.get_mut(i) {
                    *d = if v.is_finite() { v } else { 0.0 };
                }
            }
        }
        self.main_meter.measure(&self.main, frames, 0.0);
        if let Some(r) = &mut self.scout {
            r.advance(pos, frames, scout_win, !self.monitor_only);
        }
        // Heard once: an offline render through this engine never replays it.
        self.live_midi.clear();
    }

    /// The mix's own tail after a live release, stopped (see `tail_on`), once its outputs for the
    /// block are in. A strip that did not render this block (inactive, gone) has no release
    /// running: nothing else would ever end it. No allocation.
    fn track_tail(&mut self, s: &Session, frames: usize) {
        let block = self.block;
        let (mut strips, mut heard) = (false, false);
        for st in self.strips.values_mut() {
            if st.stamp != block {
                st.live_tail = 0;
            }
            strips |= st.live_tail > 0;
            heard |= st.stamp == block && st.hears_live;
        }
        if !self.monitor_only {
            self.tail_on = false;
            return;
        }
        if strips && !self.tail_on {
            (self.tail_on, self.tail_age, self.tail_quiet) = (true, 0, 0);
        }
        if !self.tail_on {
            return;
        }
        self.tail_age = self.tail_age.saturating_add(frames);
        let level = self.hw.iter().fold(chain_peak(&self.main, frames), |m, h| m.max(chain_peak(h, frames)));
        self.tail_quiet = if level < TAIL_SILENCE { self.tail_quiet.saturating_add(frames) } else { 0 };
        if strips {
            return;
        }
        let rate = s.sample_rate.as_f64();
        if self.tail_quiet >= (rate * TAIL_QUIET_SECS) as usize {
            self.tail_on = false;
        } else if self.tail_age >= (rate * LIVE_TAIL_SECS) as usize && heard {
            // Out of time, but other live input is still heard (a monitored mic, another armed
            // keyboard): the mix renders on for it anyway, so nothing is faded or reset under it.
            self.tail_on = false;
        } else if self.tail_age >= (rate * LIVE_TAIL_SECS) as usize {
            // Out of time and still sounding: faded out over this block, and every plugin let go
            // (a reverb left ringing would sound again when the mix next renders).
            for c in self.main.iter_mut().chain(self.hw.iter_mut().flatten()) {
                fade_out(c, frames);
            }
            self.reset();
        }
    }

    /// The delay-compensation latency written onto `bus`: the slowest track insert, plus the
    /// slowest aux return when an aux returns onto this bus.
    fn bus_latency(&self, bus: BusId) -> usize {
        self.track_latency + if self.aux_fed.contains(&bus) { self.aux_latency } else { 0 }
    }

    /// Copy one rendered block of `bus` into `out`. A missing bus stays silent.
    fn copy_bus(&self, bus: BusId, frames: usize, out: &mut [Vec<f32>]) {
        for o in out.iter_mut() {
            for x in o.iter_mut().take(frames) {
                *x = 0.0;
            }
        }
        let Some(src) = self.busses.get(&bus) else { return };
        for (o, c) in out.iter_mut().zip(src.buf.iter()) {
            for i in 0..frames {
                let v = c.get(i).copied().unwrap_or(0.0);
                if let Some(d) = o.get_mut(i) {
                    *d = if v.is_finite() { v } else { 0.0 };
                }
            }
        }
    }

    /// Send, meter and pan a processed strip into the busses / main mix / hardware outputs.
    fn route_strip(&mut self, s: &Session, t: &Track, strip: &mut Strip, pos: Samples, frames: usize) {
        let m = self.meters.entry(t.id).or_default();
        m.measure(&strip.buf, frames, strip.gr);
        m.llm_bypassed = strip.llm_bits;
        m.plugin_failures = strip.failures;
        m.modules.clone_from(&strip.mods);
        m.analysis.clone_from(&strip.analysis);
        let src_fmt = strip_format(t, strip.buf.len());
        let law = self.pan_law;
        for (i, snd) in t.mixer.sends.iter().enumerate() {
            let Some(snd) = snd else { continue };
            if send_muted(t, i, snd.mute, pos) || strip.muted {
                continue;
            }
            let lvl = send_level(t, i, snd.level_db, pos);
            let gain = db_to_gain(lvl);
            let src = if snd.pre_fader { &strip.pre } else { &strip.buf };
            let Route::Bus(b) = &snd.target else { continue };
            let Some(dst) = self.busses.get_mut(b) else { continue };
            if gain <= 0.0 {
                continue;
            }
            // Mono sends use the send's own pan; stereo sends go straight to L/R.
            let stereo = if src.len() == 1 { [send_pan(t, i, snd.pan, pos), 0.0] } else { [-1.0, 1.0] };
            let surround = snd.surround.as_ref().or(if snd.follow_main_pan { t.mixer.surround.as_ref() } else { None });
            let m = build_matrix(src_fmt, src.len(), dst.fmt, dst.buf.len(), &PanSpec { stereo, surround }, law);
            apply_matrix(src, frames, &mut dst.buf, &m, gain);
        }
        let dest = strip.dest;
        let src: &[Vec<f32>] = match dest {
            // The line runs even at delay 0, so it keeps the history a later change crossfades from.
            Dest::Main | Dest::Bus(_) | Dest::Hw(_) if strip.out_extra.active() => {
                if strip.outbuf.len() != strip.buf.len() {
                    strip.outbuf = vec![Vec::new(); strip.buf.len()];
                }
                for (d, src) in strip.outbuf.iter_mut().zip(strip.buf.iter()) {
                    d.clear();
                    d.extend_from_slice(src.get(..frames).unwrap_or(&[]));
                }
                strip.out_extra.process(&mut strip.outbuf, frames);
                &strip.outbuf
            }
            _ => &strip.buf,
        };
        let (dst, dst_fmt) = match dest {
            Dest::Main => (&mut self.main, self.main_fmt),
            Dest::Bus(b) => match self.busses.get_mut(&b) {
                Some(d) => (&mut d.buf, d.fmt),
                None => return,
            },
            // Not sized yet (the outputs changed since the last sync): silent this block.
            Dest::Hw(i) => match (self.hw.get_mut(i), s.outputs.get(i)) {
                (Some(d), Some(o)) if !d.is_empty() => (d, o.format),
                _ => return,
            },
            Dest::None => return,
        };
        let mut start = 0;
        while start < frames {
            let (at, end) = pan_segment(t, pos, start, frames);
            let spec = PanSpec { stereo: [pan_at(t, 0, at), pan_at(t, 1, at)], surround: t.mixer.surround.as_ref() };
            let m = build_matrix(src_fmt, src.len(), dst_fmt, dst.len(), &spec, law);
            apply_matrix_span(src, start..end, dst, &m, 1.0);
            start = end;
        }
    }
}

/// The live path for low-latency monitoring (`options.low_latency_monitoring`), filled by
/// [`llm_scope`]. Make it with [`LlmScope::reserve`] (off the audio thread); filling it then
/// doesn't allocate.
#[derive(Debug, Default)]
pub struct LlmScope {
    /// Indexed like `s.tracks`: on the live path.
    pub tracks: Vec<bool>,
    /// Position of each bus in `s.busses`.
    bus_at: HashMap<BusId, usize>,
    /// Indexed like `s.busses`: fed by a strip on the path (while filling).
    fed: Vec<bool>,
    /// Indexed like `s.busses`: the input of an aux on the path.
    live: Vec<bool>,
}

impl LlmScope {
    /// Room for the tracks and busses of `s`. Allocates (only what grew or changed size).
    pub fn reserve(&mut self, s: &Session) {
        self.tracks.clear();
        self.tracks.reserve(s.tracks.len());
        self.bus_at.clear();
        self.bus_at.extend(s.busses.iter().enumerate().map(|(i, b)| (b.id, i)));
        // Kept when the bus count is unchanged (a re-sync keeps its allocation).
        if self.fed.len() != s.busses.len() {
            self.fed = vec![false; s.busses.len()];
            self.live = vec![false; s.busses.len()];
        }
    }

    /// Nothing on the path (the mode is off).
    pub fn clear(&mut self) {
        self.tracks.clear();
        self.live.iter_mut().for_each(|x| *x = false);
    }

    /// Track `i` (an index into `s.tracks`) is on the path.
    pub fn on(&self, i: usize) -> bool {
        self.tracks.get(i).copied().unwrap_or(false)
    }

    /// `b` is the input bus of an aux on the path.
    pub fn live_bus(&self, b: BusId) -> bool {
        self.bus_at.get(&b).and_then(|i| self.live.get(*i)).copied().unwrap_or(false)
    }

    fn mark(flags: &mut [bool], bus_at: &HashMap<BusId, usize>, b: BusId) {
        if let Some(x) = bus_at.get(&b).and_then(|i| flags.get_mut(*i)) {
            *x = true;
        }
    }

    fn is(flags: &[bool], bus_at: &HashMap<BusId, usize>, b: BusId) -> bool {
        bus_at.get(&b).and_then(|i| flags.get(*i)).copied().unwrap_or(false)
    }
}

/// Fills `out` with the live path of `s` (see [`LlmScope`]): active audio tracks that are
/// record-armed (whatever their input monitoring, so direct monitoring with Off counts too) or
/// monitoring their input (On); the active auxes and folders whose input bus such a track feeds
/// through its output, transitively; and, when any track is live, the active master faders.
/// Sends never extend it (effect returns keep their plugins). Each pass is linear in the tracks.
/// Doesn't allocate once `out` is reserved for `s` (`MixEngine::sync` does).
pub fn llm_scope(s: &Session, out: &mut LlmScope) {
    let LlmScope { tracks, bus_at, fed, live } = out;
    tracks.clear();
    tracks.extend(
        s.tracks.iter().map(|t| !t.inactive && t.kind == TrackKind::Audio && (t.mixer.record_arm || t.mixer.input_monitor == InputMonitor::On)),
    );
    live.iter_mut().for_each(|x| *x = false);
    if !tracks.iter().any(|x| *x) {
        return;
    }
    // Each pass adds at least one aux, or ends: bounded by the track count.
    for _ in 0..s.tracks.len() {
        fed.iter_mut().for_each(|x| *x = false);
        for (t, on) in s.tracks.iter().zip(tracks.iter()) {
            if let (true, Route::Bus(b)) = (*on, &t.mixer.output) {
                LlmScope::mark(fed, bus_at, *b);
            }
        }
        let mut grew = false;
        for (t, on) in s.tracks.iter().zip(tracks.iter_mut()) {
            if *on || t.inactive || !matches!(t.kind, TrackKind::Aux | TrackKind::Folder) {
                continue;
            }
            if let Route::Bus(b) = &t.mixer.input
                && LlmScope::is(fed, bus_at, *b)
            {
                *on = true;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    for (t, on) in s.tracks.iter().zip(tracks.iter_mut()) {
        *on |= t.kind == TrackKind::Master && !t.inactive;
        if let (true, TrackKind::Aux | TrackKind::Folder, Route::Bus(b)) = (*on, t.kind, &t.mixer.input) {
            LlmScope::mark(live, bus_at, *b);
        }
    }
}

/// What an active insert does this block under low-latency monitoring.
#[derive(Clone, Copy, PartialEq)]
enum LlmRun {
    /// Runs as usual.
    Run,
    /// Bypassed by low-latency monitoring: skipped, as a bypassed insert is.
    Skip,
    /// Runs, its output crossfaded with its input (see `llm_blend`).
    Fade,
}

/// `llm_pos` of an insert with no crossfade going on.
const LLM_SETTLED: i64 = i64::MAX / 4;

/// Moves an insert towards `off` (bypassed by low-latency monitoring) and says what it does this
/// block. A change crossfades over `xf` samples between the plugin's output and its input, so
/// there is no click. A plugin coming back after a full bypass is first reset (its parameters are
/// already applied for this block: no glide from defaults, no stale audio), then heard only once
/// its latency has passed. A change on a slot that has not run since it was made or reset (a
/// Play/Stop/Locate) applies at once. Does not allocate (`Plugin::reset` does not).
fn llm_step(slot: &mut PluginSlot, off: bool, xf: usize) -> LlmRun {
    let xf = xf.max(1) as i64;
    if off != slot.llm_off {
        slot.llm_off = off;
        slot.llm_pos = if slot.llm_fresh {
            LLM_SETTLED
        } else if slot.llm_pos < xf {
            // Reversed mid-way: carry on from the same mix.
            xf - slot.llm_pos.max(0)
        } else if off {
            0
        } else {
            slot.plugin.reset();
            -(slot.plugin.latency().min(MAX_LATENCY) as i64)
        };
    }
    slot.llm_fresh = false;
    match (slot.llm_pos >= xf, slot.llm_off) {
        (true, true) => LlmRun::Skip,
        (true, false) => LlmRun::Run,
        (false, _) => LlmRun::Fade,
    }
}

/// Crossfades a faded insert's output `buf` with its input `dry` (`llm_step`), and advances it.
fn llm_blend(slot: &mut PluginSlot, buf: &mut [Vec<f32>], dry: &[Vec<f32>], frames: usize, xf: usize) {
    let xf = xf.max(1) as i64;
    for (b, d) in buf.iter_mut().zip(dry) {
        for (i, (x, y)) in b.iter_mut().zip(d.iter()).take(frames).enumerate() {
            let w = slot.llm_pos.saturating_add(i as i64).clamp(0, xf) as f32 / xf as f32;
            let wet = if slot.llm_off { 1.0 - w } else { w };
            *x = *y + (*x - *y) * wet;
        }
    }
    slot.llm_pos = slot.llm_pos.saturating_add(frames as i64);
}

/// Copies the first `frames` of `buf` into `dry` (sized in `sync`; a missing channel is skipped,
/// never allocated).
fn copy_dry(buf: &[Vec<f32>], dry: &mut [Vec<f32>], frames: usize) {
    for (d, b) in dry.iter_mut().zip(buf) {
        let n = frames.min(d.len()).min(b.len());
        if let (Some(d), Some(b)) = (d.get_mut(..n), b.get(..n)) {
            d.copy_from_slice(b);
        }
    }
}

/// The edit flag that turns low-latency monitoring on (`options.low_latency_monitoring`).
const LLM_FLAG: &str = "options.low_latency_monitoring";

/// Where a strip's output goes.
#[derive(Clone, Copy)]
enum Dest {
    Main,
    Bus(BusId),
    /// Hardware output `i >= 1` (an index into `Session::outputs`).
    Hw(usize),
    None,
}

/// Resolves an output route. A hardware output named like the main (`outputs[0]`) is the main
/// mix; an unknown name goes nowhere. Doesn't allocate (the audio thread calls it).
fn out_dest(s: &Session, r: &Route) -> Dest {
    match r {
        Route::Main => Dest::Main,
        Route::Bus(b) => Dest::Bus(*b),
        Route::Hardware(h) => match s.output_index(h) {
            Some(0) => Dest::Main,
            Some(i) => Dest::Hw(i),
            None => Dest::None,
        },
        // A musician, a module or MIDI input is an input only.
        Route::None | Route::Musician(_) | Route::Modules(_) | Route::Midi(_) => Dest::None,
    }
}

/// Source → trim → inserts → (pre-fader copy) → mute/fader for one strip. Independent of other
/// strips except that aux inputs read `busses`, so non-aux strips can run in parallel.
/// Live input routed to monitored tracks.
struct LiveInput<'a> {
    input: &'a [Vec<f32>],
    monitor_only: bool,
    recording: bool,
    /// Delay compensation is on (sidechain keys are aligned), and the crossfade length.
    pdc: bool,
    xf: usize,
    /// The frames of this block Scout taps measure (offset, count); `None`: no capture.
    scout: Option<(usize, usize)>,
    /// The band's events of this block, per musician.
    band: &'a [SlotBuf; BAND_SLOTS],
    /// Hardware MIDI input of this block.
    midi: &'a LiveMidi,
    /// The MIDI input port slots whose port is gone since the last block (bit per slot).
    ports_gone: u16,
}

fn process_strip(
    s: &Session,
    t: &Track,
    strip: &mut Strip,
    busses: &HashMap<BusId, BusBuf>,
    live: &LiveInput<'_>,
    pos: Samples,
    frames: usize,
    solo_muted: bool,
) {
    for c in strip.buf.iter_mut() {
        c.iter_mut().take(frames).for_each(|x| *x = 0.0);
    }
    let transport = transport_at(s, pos, !live.monitor_only, live.recording);
    // A musician input plays the band's events instead of the clips, stopped too (its notes close).
    let musician = match t.mixer.input {
        Route::Musician(k) => Some(usize::from(k)),
        _ => None,
    };
    // Modules' MIDI input plays those modules' events of this block (the sources rendered first).
    let sources = t.mixer.input.module_sources();
    let module_in = !sources.is_empty();
    if strip.module_in.as_slice() != sources && !strip.module_in.is_empty() && module_in {
        // The set changed: a source taken out closes only the notes it started; the others play
        // on (their held notes follow their source's new index).
        if let Some(inst) = strip.instrument.as_mut() {
            let old = &strip.module_in;
            strip.midi_held.retain_mut(|(k, ch, pitch)| {
                let Some(m) = old.get(usize::from(*k)) else { return false };
                match sources.iter().position(|x| x == m).and_then(|i| u8::try_from(i).ok()) {
                    Some(i) => {
                        *k = i;
                        true
                    }
                    None => {
                        inst.plugin.midi_raw(0, [0x80 | (*ch & 0x0F), *pitch & 0x7F, 0]);
                        false
                    }
                }
            });
        }
    } else if strip.musician != musician || strip.module_in.as_slice() != sources {
        // The input moved off a musician or the modules (to clips or another live source), or
        // off clips with notes sounding: what the instrument was playing would never get its
        // note-off.
        if strip.musician.is_some() || !strip.module_in.is_empty() || !strip.held.is_empty() {
            if let Some(inst) = strip.instrument.as_mut() {
                inst.plugin.all_notes_off();
            }
            strip.held.clear();
            strip.midi_held.clear();
        }
        // Back on the clips mid-play, their controllers are chased as after a locate.
        strip.chase = true;
    }
    strip.musician = musician;
    // Live MIDI input: an armed track (with MIDI Thru) or an input-monitored one plays it through
    // its instrument, stopped too. Notes it held are let go when it stops hearing it.
    let live_filter = match &t.mixer.input {
        Route::Midi(m) if hears_live_midi(t, live.midi.thru) => Some(MidiFilter::new(m.device.as_deref(), m.channel)),
        _ => None,
    };
    // Notes (and pedals) the input held are let go when the track stops hearing it or hears
    // another port or channel (their note-offs would not come through), and those from a port
    // slot whose port closed (or changed) when it goes.
    let gone = if live_filter != strip.live_filter { u16::MAX } else { live.ports_gone };
    let was_live = strip.live_filter.is_some();
    strip.live_filter = live_filter;
    strip.hears_live = live_filter.is_some();
    if gone != 0 && !strip.live_held.is_empty() {
        match strip.instrument.as_mut() {
            Some(inst) => strip.live_held.release_ports(gone, |m| inst.plugin.midi_raw(0, m)),
            None => strip.live_held = HeldNotes::default(),
        }
    }
    // No longer hearing live input: whatever it played may still be releasing (keys let go just
    // now, or just before, their release still sounding), so the instrument runs on until it is.
    if was_live && live_filter.is_none() && strip.instrument.is_some() {
        strip.live_tail = (s.sample_rate.as_f64() * LIVE_TAIL_SECS) as usize;
        strip.tail_quiet = 0;
    }
    if live_filter.is_some() || !live.monitor_only {
        strip.live_tail = 0;
    }
    if strip.module_in.as_slice() != sources && strip.module_in.capacity() >= sources.len() {
        strip.module_in.clear();
        strip.module_in.extend_from_slice(sources);
    }
    // A frozen track plays its render (clips through the chain down to the freeze point,
    // pre-fader) instead: no source, no trim, and the frozen modules don't run (that is the point:
    // they cost nothing while frozen). Modules after the freeze point process the render live.
    let frozen = t.frozen_audio().filter(|_| plays_frozen(t));
    let frozen_mods = if frozen.is_some() { t.frozen_modules() } else { 0 };
    // A frozen instrument doesn't run. Frozen, its held notes are let go (they'd hang); thawed, it
    // starts clean (no voice from before the freeze rings on) and chases the clips' controllers.
    let inst_frozen = frozen.is_some() && t.instrument.is_some();
    if inst_frozen != strip.frozen_inst {
        if let Some(inst) = strip.instrument.as_mut() {
            inst.plugin.all_notes_off();
            if !inst_frozen {
                inst.plugin.reset();
            }
        }
        strip.held.clear();
        strip.chase = true;
        strip.frozen_inst = inst_frozen;
    }
    if frozen_mods < strip.frozen_mods {
        // Thawed, or the freeze point moved up: the modules that come back to life start clean,
        // not from what they held when they froze. Live modules after the point keep ringing.
        for slot in strip.plugins.iter_mut().take(strip.frozen_mods).skip(frozen_mods).flatten() {
            slot.plugin.reset();
        }
    }
    strip.frozen_mods = frozen_mods;
    // The instrument ran its release this block: a strip whose instrument did not run (frozen,
    // failed, gone, or no longer an instrument track) has no release to wait for.
    let mut tail_ran = false;
    match t.kind {
        _ if frozen.is_some() => {
            if let Some(fz) = frozen.filter(|_| !live.monitor_only) {
                play_frozen(s, fz, pos, frames, &mut strip.buf);
            }
        }
        TrackKind::Audio if !live.input.is_empty() && t.mixer.hears_input(live.monitor_only, live.recording) => {
            // Input monitoring: the track hears its hardware input instead of its clips. A named
            // input (or a legacy "In N" / "In N-M") gives its device channels; with no hardware
            // input, track channel k hears device channel k, as before.
            // An unknown input, or a channel the device lacks, is silence: never wrapped onto
            // another channel.
            strip.hears_live = true;
            if let Some((first, width)) = strip.in_span {
                for (k, dst) in strip.buf.iter_mut().enumerate() {
                    if let Some(src) = live.input.get(track_input_channel(k, first, width)) {
                        for (d, x) in dst.iter_mut().zip(src.iter()).take(frames) {
                            *d = *x;
                        }
                    }
                }
            }
        }
        TrackKind::Audio if live.monitor_only => {}
        TrackKind::Instrument | TrackKind::Midi
            if live.monitor_only && musician.is_none() && !module_in && live_filter.is_none() && strip.live_tail == 0 => {}
        TrackKind::Audio => {
            let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
            for clip in t.clips() {
                if clip.muted || !clip.is_audio() || !clip.range().overlaps(&block) {
                    continue;
                }
                match clip_fx_params(s, clip.id.0) {
                    None => render_clip(s, clip, pos, frames, &mut strip.buf, &mut strip.scratch),
                    Some(params) => {
                        let nch = strip.buf.len();
                        if strip.clipbuf.len() != nch || strip.clipbuf.first().map_or(0, Vec::len) < frames {
                            strip.clipbuf = vec![vec![0.0; frames.max(strip.scratch.len())]; nch];
                        }
                        for c in strip.clipbuf.iter_mut() {
                            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
                        }
                        render_clip(s, clip, pos, frames, &mut strip.clipbuf, &mut strip.scratch);
                        let fx = strip.clip_fx.entry(clip.id.0).or_insert_with(|| ClipFx::new(s.sample_rate.as_f64() as f32, nch));
                        fx.configure(params);
                        fx.process(&mut strip.clipbuf, frames);
                        for (d, c) in strip.buf.iter_mut().zip(strip.clipbuf.iter()) {
                            for (x, y) in d.iter_mut().zip(c.iter()).take(frames) {
                                *x += *y;
                            }
                        }
                    }
                }
            }
        }
        TrackKind::Aux | TrackKind::Folder => {
            if let Route::Bus(b) = &t.mixer.input
                && let Some(src) = busses.get(b)
            {
                let (ns, nd) = (src.buf.len(), strip.buf.len());
                let dst_fmt = strip_format(t, nd);
                let m = if src.fmt == dst_fmt && ns == nd {
                    identity_matrix(ns)
                } else if ns == 1 && nd == 2 {
                    // A mono bus feeds both sides of a stereo aux.
                    let mut m = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
                    m[0][0] = 1.0;
                    m[0][1] = 1.0;
                    m
                } else {
                    let stereo = if ns == 1 { [0.0, 0.0] } else { [-1.0, 1.0] };
                    build_matrix(src.fmt, ns, dst_fmt, nd, &PanSpec { stereo, surround: None }, PanLaw::Minus3)
                };
                apply_matrix(&src.buf, frames, &mut strip.buf, &m, 1.0);
            }
        }
        TrackKind::Instrument | TrackKind::Midi => {
            if let Some(inst) = strip.instrument.as_mut().filter(|i| !i.failed) {
                // #152: push the instrument's stored parameter values into the live plugin
                // before processing, the same way inserts do below. Without this, the values
                // loaded from a session (or set via `mix.insert_state`) never reach the plugin
                // instance and the rendered audio matches the plugin's defaults — e.g. a saved
                // `level = -12 dB` sounds the same as `level = -6 dB`.
                if let Some(ins) = &t.instrument {
                    apply_params(inst, ins, t, None, pos);
                }
                inst.plugin.set_transport(&transport);
                // A musician or module input has no clip controllers to chase (and while only live
                // MIDI is heard, stopped, the clips wait for the transport).
                if !live.monitor_only && musician.is_none() && !module_in && std::mem::take(&mut strip.chase) {
                    chase_controllers(s, t, pos, inst);
                }
                match musician {
                    Some(k) => {
                        if let Some(slot) = live.band.get(k) {
                            band::deliver(slot, inst);
                        }
                    }
                    None if module_in => deliver_module_midi(&strip.midi_in, inst, &mut strip.midi_held),
                    None => {
                        // An armed MIDI track plays its clips and what is played in.
                        if !live.monitor_only {
                            schedule_notes(s, t, pos, frames, inst, &mut strip.held);
                        }
                        if let Some(f) = live_filter {
                            deliver_live_midi(live.midi, f, inst, &mut strip.live_held);
                        }
                    }
                }
                if strip.buf.len() >= 2 {
                    inst.plugin.process(&mut strip.buf, frames);
                } else if let Some(c0) = strip.buf.first_mut() {
                    // Mono MIDI track: render the instrument in stereo and fold.
                    let mut st = [std::mem::take(c0), std::mem::take(&mut strip.scratch)];
                    inst.plugin.process(&mut st, frames);
                    let [a, b] = st;
                    *c0 = a;
                    strip.scratch = b;
                    for (x, y) in c0.iter_mut().zip(strip.scratch.iter()).take(frames) {
                        *x = 0.5 * (*x + *y);
                    }
                }
                if contain_failure(inst, &mut strip.buf, frames) {
                    strip.failures.record(SlotKey::Instrument);
                }
                // A let-go release runs until it has been silent a while (a quiet gap in it is not
                // its end); one that outlasts its time is faded out over this block and cut
                // (nothing is left frozen in the instrument). What the inserts, sends and auxes
                // make of it rings on through the mix's own tail (`MixEngine::track_tail`).
                if strip.live_tail > 0 {
                    tail_ran = true;
                    let silent = chain_peak(&strip.buf, frames) < TAIL_SILENCE;
                    strip.tail_quiet = if silent { strip.tail_quiet.saturating_add(frames) } else { 0 };
                    if strip.tail_quiet >= (s.sample_rate.as_f64() * TAIL_QUIET_SECS) as usize {
                        strip.live_tail = 0;
                    } else {
                        strip.live_tail = strip.live_tail.saturating_sub(frames.max(1));
                        if strip.live_tail == 0 {
                            for c in strip.buf.iter_mut() {
                                fade_out(c, frames);
                            }
                            inst.plugin.all_notes_off();
                            inst.plugin.reset();
                        }
                    }
                }
            }
        }
        _ => {}
    }
    if !tail_ran {
        strip.live_tail = 0;
    }
    let trim = db_to_gain(t.mixer.trim_db) * if t.mixer.phase_invert { -1.0 } else { 1.0 };
    if (trim - 1.0).abs() > f32::EPSILON && frozen.is_none() {
        for c in strip.buf.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x *= trim);
        }
    }
    // Scout PRE: after the source, trim and phase, immediately before the first insert. Like POST
    // it is ahead of delay compensation (`align`), so not PDC-aligned across strips.
    if let (Some(win), Some(taps)) = (live.scout, strip.scout.as_deref_mut()) {
        taps.feed_pre(&strip.buf, win);
    }
    let mut gr = 0.0f32;
    let mut latency = 0usize;
    let mut level = chain_peak(&strip.buf, frames);
    let mut bits = 0u64;
    let mut skipped = 0usize;
    let on_llm = strip.llm;
    for (i, ins) in t.mixer.inserts.iter().enumerate() {
        if i < frozen_mods {
            if let Some(m) = strip.mods.get_mut(i) {
                *m = ModuleMeter { module: ins.id, ..ModuleMeter::default() };
            }
            continue;
        }
        let mut meter = ModuleMeter { module: ins.id, in_peak: level, out_peak: level, ..ModuleMeter::default() };
        if let Some(Some(slot)) = strip.plugins.get_mut(i)
            && slot.module == ins.id
        {
            meter.failed = slot.failed;
            // Last block's MIDI was read before this strip ran again; a module that does not run
            // this block (bypassed, failed) emits nothing.
            slot.midi.clear();
            // Low-latency monitoring takes a latent module on the live path out (see `llm_step`).
            let run = if ins.active && !ins.bypass && !slot.failed {
                apply_params(slot, ins, t, Some(ins.id), pos);
                let run = llm_step(slot, strip.llm && slot.plugin.latency() > 0, live.xf);
                if slot.llm_off {
                    skipped = skipped.saturating_add(slot.plugin.latency());
                    if i < 64 {
                        bits |= 1 << i;
                    }
                }
                run
            } else {
                LlmRun::Skip
            };
            if run != LlmRun::Skip {
                if run == LlmRun::Fade {
                    copy_dry(&strip.buf, &mut strip.llm_dry, frames);
                }
                slot.plugin.set_transport(&transport);
                match strip.keys.iter_mut().find(|k| k.module == ins.id).filter(|_| ins.sidechain.is_some()) {
                    Some(key) => {
                        // Line the key up with this module's main input: the earlier one waits.
                        let main_at = strip.base.saturating_add(latency);
                        // On the low-latency path the main input never waits for a key.
                        let (wait_key, wait_main) =
                            if live.pdc { (main_at.saturating_sub(key.at), if on_llm { 0 } else { key.at.saturating_sub(main_at) }) } else { (0, 0) };
                        let ch = strip.buf.len();
                        strip.key_regrow |= !key.main_delay.set(wait_main, ch, live.xf);
                        key.main_delay.process(&mut strip.buf, frames);
                        latency = latency.saturating_add(key.main_delay.delay());
                        strip.key_regrow |= !key.delay.set(wait_key, ch, live.xf);
                        key.delay.process(&mut key.buf, frames);
                        slot.plugin.process_with_sidechain(&mut strip.buf, frames, Some(&key.buf));
                    }
                    None => slot.plugin.process(&mut strip.buf, frames),
                }
                if run == LlmRun::Fade {
                    llm_blend(slot, &mut strip.buf, &strip.llm_dry, frames, live.xf);
                }
                if slot.emits {
                    slot.plugin.midi_out(&mut slot.midi);
                }
                collect_released(slot, &mut strip.released);
                if contain_failure(slot, &mut strip.buf, frames) {
                    strip.failures.record(SlotKey::Module(ins.id));
                    meter.failed = true;
                } else {
                    // A module low-latency monitoring takes out adds no latency, even while it
                    // fades out.
                    if !slot.llm_off {
                        latency = latency.saturating_add(slot.plugin.latency());
                    }
                    meter.gr_db = slot.plugin.gain_reduction_db();
                    if gr == 0.0 {
                        gr = meter.gr_db;
                    }
                    read_readouts(&*slot.plugin, &mut meter.readouts);
                }
                level = chain_peak(&strip.buf, frames);
                meter.out_peak = level;
            }
        }
        // `sync` sized `mods` to the chain; never grow it here.
        if let Some(m) = strip.mods.get_mut(i) {
            *m = meter;
        }
    }
    // Scout POST: after the last insert, before the pre-fader copy, the fader and the pan.
    if let (Some(win), Some(taps)) = (live.scout, strip.scout.as_deref_mut()) {
        taps.feed_post(&strip.buf, win);
    }
    strip.gr = gr;
    strip.llm_bits = bits;
    strip.llm_latency = skipped.min(MAX_LATENCY);
    strip.latency = latency.min(MAX_LATENCY);
    strip.post_at = strip.base.saturating_add(strip.latency);
    strip.pre_at = strip.post_at;
    let auto_mute = t.mixer.automation_mode.reads() && t.lane(&AutoParam::Mute).is_some_and(|l| l.value_at(pos, 0.0) >= 0.5);
    strip.muted = t.mixer.mute || auto_mute || solo_muted;
    if strip.pre_key || t.mixer.sends.iter().flatten().any(|x| x.pre_fader) {
        strip.pre.resize_with(strip.buf.len(), Vec::new);
        for (d, src) in strip.pre.iter_mut().zip(strip.buf.iter()) {
            d.clear();
            d.extend_from_slice(src.get(..frames).unwrap_or(&[]));
        }
    }
    // Endpoints are the first and last sample of this block. The next block starts on its own
    // first sample, so a breakpoint there does not ramp the block before it.
    let last = pos.saturating_add(frames.saturating_sub(1) as i64);
    let mut v0 = volume_db_at(t, pos);
    let mut v1 = volume_db_at(t, last);
    // Trim automation (an offset on top of the volume curve).
    if let Some(l) = t.automation.iter().find(|l| l.param == AutoParam::Trim).filter(|l| !l.points.is_empty()) {
        v0 += l.value_at(pos, 0.0);
        v1 += l.value_at(last, 0.0);
    }
    // VCA master: its fader offsets the member's, and its mute mutes the member.
    if let Some(vid) = t.mixer.vca
        && let Some(vca) = s.tracks.iter().find(|x| x.id.0 == vid && x.kind == TrackKind::Vca)
    {
        v0 += volume_db_at(vca, pos);
        v1 += volume_db_at(vca, last);
        if vca.mixer.mute {
            strip.muted = true;
        }
    }
    let (g0, g1) = if strip.muted { (-144.0, -144.0) } else { (v0.min(12.0), v1.min(12.0)) };
    for c in strip.buf.iter_mut() {
        for i in 0..frames {
            if let Some(x) = c.get_mut(i) {
                *x *= lerp_gain(g0, g1, i, frames.saturating_sub(1));
            }
        }
    }
}

/// Whether a frozen track plays its render: audio tracks, and MIDI and instrument tracks with an
/// instrument (Track › Freeze makes no other).
pub fn plays_frozen(t: &Track) -> bool {
    match t.kind {
        TrackKind::Audio => true,
        TrackKind::Instrument | TrackKind::Midi => t.instrument.is_some(),
        _ => false,
    }
}

/// Adds the frozen render `fz` under `[pos, pos + frames)` into `buf` (channel `k` plays source
/// channel `k` modulo the source's channels). A render missing from the pool is silence.
fn play_frozen(s: &Session, fz: &FrozenAudio, pos: Samples, frames: usize, buf: &mut [Vec<f32>]) {
    let Some(audio) = s.pool.get(fz.source) else { return };
    let nsrc = audio.num_channels();
    if nsrc == 0 {
        return;
    }
    let rt = on_audio_thread();
    let first = pos.saturating_sub(fz.start);
    for (k, dst) in buf.iter_mut().enumerate() {
        let mut src = audio.cursor(k % nsrc, rt);
        for (i, d) in dst.iter_mut().take(frames).enumerate() {
            let at = first.saturating_add(i as i64);
            if let Some(x) = usize::try_from(at).ok().and_then(|a| src.get(a)) {
                *d += x;
            }
        }
    }
}

/// Silences a block the plugin filled with NaN or infinity and takes the plugin out until the
/// next [`MixEngine::reset`]. True then.
fn contain_failure(slot: &mut PluginSlot, buf: &mut [Vec<f32>], frames: usize) -> bool {
    // A branch-free fold vectorizes; `all` runs about 8 times slower here.
    if buf.iter().all(|c| c.get(..frames).unwrap_or(c).iter().fold(true, |ok, x| ok & x.is_finite())) {
        return false;
    }
    for c in buf.iter_mut() {
        c.iter_mut().take(frames).for_each(|x| *x = 0.0);
    }
    slot.failed = true;
    true
}

/// Clip-effect parameters of a clip, or None when it has none or they are bypassed.
fn clip_fx_params(s: &Session, clip: u64) -> Option<impl Iterator<Item = (&str, f32)> + Clone> {
    if s.edit.values.is_empty() || Key::new(format_args!("clip_fx.bypass.{clip}")).is_none_or(|k| s.edit.flag(k.as_str())) {
        return None;
    }
    let pre = Key::new(format_args!("clip_fx.{clip}."))?;
    let mut params = s
        .edit
        .values
        .range::<str, _>((Bound::Included(pre.as_str()), Bound::Unbounded))
        .take_while(move |(k, _)| k.starts_with(pre.as_str()))
        .map(move |(k, v)| (k.get(pre.len..).unwrap_or(""), *v as f32))
        .peekable();
    params.peek()?;
    Some(params)
}

/// A short key formatted on the stack: lookups made on the audio thread must not allocate.
#[derive(Clone, Copy)]
struct Key {
    buf: [u8; 64],
    len: usize,
}

impl Key {
    /// None when the key doesn't fit.
    fn new(args: std::fmt::Arguments<'_>) -> Option<Key> {
        let mut k = Key { buf: [0; 64], len: 0 };
        std::fmt::Write::write_fmt(&mut k, args).ok()?;
        Some(k)
    }

    fn as_str(&self) -> &str {
        self.buf.get(..self.len).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl std::fmt::Write for Key {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(std::fmt::Error)?;
        self.buf.get_mut(self.len..end).ok_or(std::fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl ClipFx {
    fn new(sr: f32, ch: usize) -> ClipFx {
        ClipFx { eq: None, comp: None, gain_db: 0.0, sr, ch }
    }
    /// Clear filter, detector and lookahead history (transport stop / locate).
    fn reset(&mut self) {
        for p in self.eq.iter_mut().chain(self.comp.iter_mut()) {
            p.reset();
        }
    }
    fn make(&self, id: &str) -> Option<Box<dyn Plugin>> {
        soundcraft_dsp::create(id).map(|mut p| {
            p.prepare(self.sr, 16_384, self.ch.max(1));
            p
        })
    }
    /// Modules exist only when one of their parameters is set.
    fn configure<'a>(&mut self, params: impl Iterator<Item = (&'a str, f32)> + Clone) {
        self.gain_db = 0.0;
        let has_eq = params.clone().any(|(k, _)| k.starts_with("eq."));
        let has_comp = params.clone().any(|(k, _)| k.starts_with("comp."));
        if has_eq && self.eq.is_none() {
            self.eq = self.make("eq_7band");
        } else if !has_eq {
            self.eq = None;
        }
        if has_comp && self.comp.is_none() {
            self.comp = self.make("compressor");
        } else if !has_comp {
            self.comp = None;
        }
        for (k, v) in params {
            if let Some(p) = k.strip_prefix("eq.") {
                if let Some(eq) = &mut self.eq
                    && eq.param(p) != Some(v)
                {
                    eq.set_param(p, v);
                }
            } else if let Some(p) = k.strip_prefix("comp.") {
                if let Some(c) = &mut self.comp
                    && c.param(p) != Some(v)
                {
                    c.set_param(p, v);
                }
            } else if k == "gain" {
                self.gain_db = v;
            }
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if let Some(eq) = &mut self.eq {
            eq.process(io, frames);
        }
        if let Some(c) = &mut self.comp {
            c.process(io, frames);
        }
        if self.gain_db.abs() > 1e-6 {
            let g = db_to_gain(self.gain_db);
            for ch in io.iter_mut() {
                ch.iter_mut().take(frames).for_each(|x| *x *= g);
            }
        }
    }
}

/// Hash of everything `sync`/`processing_order` depend on (tracks, kinds, routing, plugins, sends).
fn structure_fingerprint(s: &Session) -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for t in &s.tracks {
        t.id.hash(&mut h);
        t.kind.hash(&mut h);
        t.format.hash(&mut h);
        t.inactive.hash(&mut h);
        t.mixer.inserts.len().hash(&mut h);
        t.mixer.input.hash(&mut h);
        t.mixer.output.hash(&mut h);
        for i in &t.mixer.inserts {
            (i.id, i.plugin.as_str(), i.sidechain, i.link_group).hash(&mut h);
            // A linked module's role (from its params) sets the rendering order.
            if i.link_group.is_some() {
                link::role_of(i).hash(&mut h);
            }
            // which assets it works from, and which decoded data (made anew: re-sync)
            for (k, r) in &i.assets {
                (k, r).hash(&mut h);
                s.assets.get_for(k, r, s.sample_rate.hz()).map(|a| Arc::as_ptr(&a.0) as *const () as usize).hash(&mut h);
            }
        }
        for snd in t.mixer.sends.iter().flatten() {
            snd.target.hash(&mut h);
            // A pre-fader send needs the pre-fader delay line, which `sync` reserves.
            snd.pre_fader.hash(&mut h);
        }
        t.instrument.as_ref().map(|x| x.plugin.as_str()).hash(&mut h);
        let sample = t.instrument.as_ref().and_then(|x| x.sample);
        sample.hash(&mut h);
        // Which decoded audio it is, so new audio under the same id (another session) re-syncs.
        sample.and_then(|id| s.pool.get(id)).map(|a| Arc::as_ptr(a) as usize).hash(&mut h);
    }
    for b in &s.busses {
        b.id.hash(&mut h);
        b.format.hash(&mut h);
    }
    // Hardware output buffers are sized by `sync`, which also resolves the route names.
    for o in &s.outputs {
        (o.name.as_str(), o.first_channel, o.format).hash(&mut h);
    }
    for i in &s.inputs {
        (i.name.as_str(), i.first_channel, i.format).hash(&mut h);
    }
    s.main_format().hash(&mut h);
    h.finish() as usize
}

std::thread_local! {
    static AUDIO_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Mark the calling thread as the realtime audio thread (`soundcraft_playback`'s cpal callbacks
/// do, every call: one thread-local store). Strips [`MixEngine::render`] hands to worker threads
/// count as the audio thread while they render for it. Code that might run there asks
/// [`on_audio_thread`] before doing anything that blocks; the desktop app's logger does, so a
/// `log::` record from the audio thread never takes a lock or touches a file.
pub fn mark_audio_thread() {
    AUDIO_THREAD.set(true);
}

/// True on the thread [`mark_audio_thread`] marked, and on a worker while it renders strips for
/// it. Lock-free and allocation-free.
pub fn on_audio_thread() -> bool {
    AUDIO_THREAD.get()
}

/// Run `f` on this thread as (or as not) the audio thread, then restore the thread's own mark
/// (also when `f` unwinds): a pool worker renders for realtime playback and offline bounces alike.
#[cfg(not(target_arch = "wasm32"))]
fn as_audio_thread(audio: bool, f: impl FnOnce()) {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            AUDIO_THREAD.set(self.0);
        }
    }
    let _restore = Restore(AUDIO_THREAD.replace(audio));
    f();
}

/// Process strips, in parallel on native targets when there are enough of them.
#[cfg(not(target_arch = "wasm32"))]
fn run_strips<T: Send>(work: &mut [T], f: impl Fn(&mut T) + Sync + Send) {
    use rayon::prelude::*;
    if work.len() >= 4 {
        // The workers render on behalf of this thread: on the audio thread, they are it too.
        let audio = on_audio_thread();
        work.par_iter_mut().for_each(|w| as_audio_thread(audio, || f(w)));
    } else {
        work.iter_mut().for_each(f);
    }
}

#[cfg(target_arch = "wasm32")]
fn run_strips<T: Send>(work: &mut [T], f: impl Fn(&mut T) + Sync + Send) {
    work.iter_mut().for_each(f);
}

fn strip_channels(t: &Track, main_ch: usize) -> usize {
    match t.kind {
        TrackKind::Master => main_ch,
        TrackKind::Instrument => t.channels().clamp(2, MAX_CHANNELS),
        _ => t.channels().clamp(1, MAX_CHANNELS),
    }
}

/// The format a strip's buffer actually carries (a mono instrument renders in stereo).
fn strip_format(t: &Track, n: usize) -> ChannelFormat {
    if t.format.channels().min(MAX_CHANNELS) == n { t.format } else { ChannelFormat::for_channels(n) }
}

/// A count-off before a take: `bars` bars of `beats_per_bar` clicks, `beat` samples apart, at the
/// tempo and meter of the bar the take starts in. The take starts right after its last frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CountOff {
    pub bars: u32,
    pub beats_per_bar: u32,
    /// Samples per beat (fractional: each click lands on its rounded position).
    pub beat: f64,
}

impl CountOff {
    /// The count-off for a take starting at `at`: `edit.countoff_bars` bars at the tempo and
    /// meter in effect there (a beat is the meter's, so 6/8 counts six eighths a bar, as the
    /// metronome clicks). `None` when the tempo map gives no usable tempo.
    pub fn plan(s: &Session, at: Samples) -> Option<CountOff> {
        let sr = s.sample_rate;
        // The tick at or before `at` (a take a sample before a tempo change counts at the old one).
        let tick = (s.tempo.samples_to_ticks_f(at, sr) + 1e-6).floor() as i64;
        let bpm = s.tempo.tempo_at_tick(tick);
        let m = s.tempo.meter_at_tick(tick);
        let beat = sr.as_f64() * 60.0 / bpm * m.ticks_per_beat() as f64 / TICKS_PER_QUARTER as f64;
        (bpm.is_finite() && bpm > 0.0 && beat.is_finite() && beat >= 1.0).then_some(CountOff {
            bars: s.edit.countoff_bars.clamp(1, 16),
            beats_per_bar: m.numerator.clamp(1, 64),
            beat,
        })
    }

    pub fn beats(&self) -> u32 {
        self.bars.saturating_mul(self.beats_per_bar)
    }

    /// Where click `k` (from 0) starts, in frames from the count-off's start.
    pub fn beat_at(&self, k: u32) -> i64 {
        (self.beat * f64::from(k)).round() as i64
    }

    /// Its length in frames: the take starts this long after the count-off does.
    pub fn len(&self) -> i64 {
        self.beat_at(self.beats())
    }

    pub fn is_empty(&self) -> bool {
        self.len() <= 0
    }

    /// The bar and beat (both from 1) sounding `elapsed` frames into the count-off.
    pub fn bar_beat(&self, elapsed: i64) -> (u32, u32) {
        let k = ((elapsed.max(0) as f64 / self.beat.max(1.0)).floor() as u32).min(self.beats().saturating_sub(1));
        let bpb = self.beats_per_bar.max(1);
        (k / bpb + 1, k % bpb + 1)
    }
}

/// Metronome: a short synthesized blip on every beat of the tempo map, accented on beat 1 of
/// each bar. Settings come from `setup.click_countoff` (`click.*` values).
mod click {
    use super::*;

    /// Blip length in seconds.
    const LEN_S: f64 = 0.03;

    /// The click's gain, and its accented and normal (pitch, velocity).
    fn voice(s: &Session) -> (f64, (f64, f64), (f64, f64)) {
        let gain = f64::from(db_to_gain(s.edit.value("click.volume_db", -6.0).clamp(-60.0, 12.0) as f32));
        let accent = (note_hz(s.edit.value("click.accent_note", 84.0)), s.edit.value("click.accent_velocity", 127.0));
        let normal = (note_hz(s.edit.value("click.normal_note", 72.0)), s.edit.value("click.normal_velocity", 100.0));
        (gain, accent, normal)
    }

    /// The count-off's clicks over the `frames` from `elapsed` frames into it (beat 1 of each of
    /// its bars accented), with the metronome's sound. No allocation.
    pub fn render_countoff(s: &Session, c: &CountOff, elapsed: i64, frames: usize, main: &mut [Vec<f32>]) {
        let rate = s.sample_rate.as_f64();
        let len = (LEN_S * rate) as i64;
        let end = elapsed.saturating_add(frames as i64);
        let (gain, accent, normal) = voice(s);
        // From the click that may still ring into this block.
        let first = ((elapsed - len).max(0) as f64 / c.beat.max(1.0)).floor() as u32;
        for k in first..c.beats() {
            let at = c.beat_at(k);
            if at >= end {
                break;
            }
            if at.saturating_add(len) > elapsed {
                let (hz, vel) = if k % c.beats_per_bar.max(1) == 0 { accent } else { normal };
                let amp = gain * (vel.clamp(1.0, 127.0) / 127.0);
                blip(main, at - elapsed, frames, hz, amp as f32, rate);
            }
        }
    }

    pub fn render(s: &Session, pos: Samples, frames: usize, main: &mut [Vec<f32>]) {
        let sr = s.sample_rate;
        let rate = sr.as_f64();
        let len = (LEN_S * rate) as i64;
        let end = pos.saturating_add(frames as i64);
        let (gain, accent, normal) = voice(s);
        // Start one blip length early so a click that began in an earlier block still rings into this one.
        let t0 = s.tempo.samples_to_ticks(pos.saturating_sub(len), sr).max(0).saturating_sub(1);
        let mut tick = t0.saturating_sub(s.tempo.bar_beat_at_tick(t0).tick);
        // Blocks hold a few beats at most; the guard only protects against a malformed tempo map.
        for _ in 0..4096 {
            let at = s.tempo.tick_to_samples(tick, sr);
            if at >= end {
                break;
            }
            if at.saturating_add(len) > pos {
                let (hz, vel) = if s.tempo.bar_beat_at_tick(tick).beat == 1 { accent } else { normal };
                let amp = gain * (vel.clamp(1.0, 127.0) / 127.0);
                blip(main, at - pos, frames, hz, amp as f32, rate);
            }
            tick = tick.saturating_add(s.tempo.meter_at_tick(tick).ticks_per_beat());
        }
    }

    /// Adds one decaying sine blip whose first sample sits `rel` frames into the block (negative
    /// when it began before the block) to every channel.
    fn blip(main: &mut [Vec<f32>], rel: i64, frames: usize, hz: f64, amp: f32, rate: f64) {
        let len = (LEN_S * rate) as usize;
        let w = std::f64::consts::TAU * hz / rate;
        let first = usize::try_from(-rel).unwrap_or(0);
        for i in first..len {
            let Ok(at) = usize::try_from(rel + i as i64) else { continue };
            if at >= frames {
                break;
            }
            let env = (-6.0 * i as f64 / len as f64).exp() as f32;
            let v = ((w * i as f64).sin() as f32) * env * amp;
            for ch in main.iter_mut() {
                if let Some(d) = ch.get_mut(at) {
                    *d += v;
                }
            }
        }
    }

    /// Click pitch for a MIDI note setting (clamped to the MIDI range).
    fn note_hz(note: f64) -> f64 {
        soundcraft_dsp::midi_to_hz(note.clamp(0.0, 127.0))
    }
}

fn render_clip(s: &Session, clip: &Clip, pos: Samples, frames: usize, buf: &mut [Vec<f32>], _scratch: &mut [f32]) {
    let ClipContent::Audio { source, offset } = clip.content else { return };
    let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
    let Some(isect) = clip.range().intersect(&block) else { return };
    let Some(audio) = s.pool.get(source) else { return };
    let nsrc = audio.num_channels();
    if nsrc == 0 {
        return;
    }
    // On the audio thread a block not loaded from disk yet reads as silence; offline reads wait.
    let rt = on_audio_thread();
    let stretch = if clip.stretch.is_finite() && clip.stretch > 0.0 { clip.stretch } else { 1.0 };
    let i0 = usize::try_from(isect.start - pos).unwrap_or(0);
    let i1 = usize::try_from(isect.end - pos).unwrap_or(0).min(frames);
    // Gain is evaluated every 32 samples and interpolated (fades and envelopes stay smooth).
    const STEP: usize = 32;
    // Clean Vocal's cleaned alternate, mixed in sample for sample before gain and fades.
    let alt = clip.clean_alt().filter(|k| k.wet() > 0.0).and_then(|k| s.pool.get(k.source).map(|a| (k.wet(), k.origin, a)));
    let edges = alt.map(|(_, origin, a)| CleanEdges::new(s, clip, origin, a.frames()));
    // Vocal tuning's render replaces the heard audio inside its spans (looked up once per block).
    let unstretched = (stretch - 1.0).abs() < 1e-9;
    let tuned = tuned_spans(s, clip);
    for (ch, dst) in buf.iter_mut().enumerate() {
        let mut src = audio.cursor(ch % nsrc, rt);
        let mut cleaned = alt.and_then(|(w, origin, a)| {
            let n = a.num_channels();
            (n > 0).then(|| (w, origin, a.cursor(ch % n, rt)))
        });
        // The cleaned alternate covers source positions `[origin, origin + frames)`.
        let alt_span = alt.map(|(_, origin, a)| (origin, origin.saturating_add(i64::try_from(a.frames()).unwrap_or(i64::MAX))));
        // The source (and the cleaned audio) read a run of `RUN` frames at a time.
        const RUN: usize = 8 * STEP;
        let mut sv = [0.0f32; RUN];
        let mut cv = [0.0f32; RUN];
        let mut run_at = usize::MAX;
        let mut i = i0;
        while i < i1 {
            let seg_end = (i + STEP).min(i1);
            let rel0 = pos.saturating_add(i as i64).saturating_sub(clip.start);
            // The last sample of this step, not the first sample of the next one, so a breakpoint
            // on the boundary does not ramp the audio before it.
            let rel1 = pos.saturating_add(seg_end.saturating_sub(1) as i64).saturating_sub(clip.start);
            let g0 = clip.gain_at(rel0);
            let g1 = clip.gain_at(rel1.min(clip.length));
            let n = (seg_end - i).saturating_sub(1).max(1) as f32;
            if unstretched && (i < run_at || seg_end > run_at.saturating_add(RUN)) {
                run_at = i;
                let len = (i1 - i).min(RUN);
                let at0 = offset.saturating_add(rel0);
                src.fill(at0, sv.get_mut(..len).unwrap_or_default());
                if let Some((_, origin, c)) = cleaned.as_mut() {
                    c.fill(at0.saturating_sub(*origin), cv.get_mut(..len).unwrap_or_default());
                }
            }
            for k in i..seg_end {
                let rel = pos.saturating_add(k as i64).saturating_sub(clip.start);
                let v = if unstretched {
                    let at = offset.saturating_add(rel);
                    match tuned.and_then(|t| t.sample_at(ch, at)) {
                        Some(tv) => tv,
                        None => {
                            let v = sv.get(k - run_at).copied().unwrap_or(0.0);
                            match (cleaned.as_ref(), alt_span) {
                                (Some((w, _, _)), Some((lo, hi))) if at >= lo && at < hi => {
                                    blend(v, cv.get(k - run_at).copied().unwrap_or(0.0), edges.map_or(*w, |e| e.weight(*w, at as f64)))
                                }
                                _ => v,
                            }
                        }
                    }
                } else {
                    let f = offset as f64 + rel as f64 / stretch;
                    let v = lerp_at(&mut src, f).unwrap_or(0.0);
                    match cleaned.as_mut().and_then(|(w, origin, c)| lerp_at(c, f - *origin as f64).map(|cv| (*w, cv))) {
                        Some((w, cv)) => blend(v, cv, edges.map_or(w, |e| e.weight(w, f))),
                        None => v,
                    }
                };
                let t = (k - i) as f32 / n;
                if let Some(d) = dst.get_mut(k) {
                    *d += v * (g0 + (g1 - g0) * t);
                }
            }
            i = seg_end;
        }
    }
}

/// The audio `clip` plays from its source over source positions `[start, start + n)` (each of the
/// source's channels), with Clean Vocal's cleaned alternate mixed in at its current amount, before
/// gain, fades and stretch, and with its vocal tuning's render in place (when current: see
/// [`tuned_spans`]); positions outside the source read as silence. `None` for MIDI and video
/// clips or missing audio. Used by edits that render a clip's material into a new source.
pub fn clip_source_window(s: &Session, clip: &Clip, start: Samples, n: usize) -> Option<Vec<Vec<f32>>> {
    let mut out = clip_heard_window(s, clip, start, n)?;
    if let Some(t) = tuned_spans(s, clip) {
        for (ch, dst) in out.iter_mut().enumerate() {
            for (k, d) in dst.iter_mut().enumerate() {
                if let Some(v) = start.checked_add(k as i64).and_then(|at| t.sample_at(ch, at)) {
                    *d = v;
                }
            }
        }
    }
    Some(out)
}

/// The clip's current tuned render, when it should play: a live tuning switched to Tuned whose
/// fingerprint (kept current by the engine) is the one the render was made from. Reads only
/// (no allocation, lock or `Arc` drop): safe on the audio thread. A stretched clip plays untuned
/// (Elastic clips are rendered by processing anyway).
pub fn tuned_spans<'a>(s: &'a Session, clip: &Clip) -> Option<&'a soundcraft_model::TunedSpans> {
    if !clip.stretch.is_finite() || (clip.stretch - 1.0).abs() >= 1e-9 {
        return None;
    }
    let tune = clip.tune_live().filter(|t| !t.original && t.fingerprint != 0)?;
    s.vocal.renders.get(&clip.id).map(|r| &**r).filter(|r| r.inputs == tune.fingerprint && !r.spans.is_empty())
}

/// [`clip_source_window`] without the vocal tuning: what the clip sounds like before tuning (the
/// source with Clean Vocal blended in), which the tuner analyses and renders from.
pub fn clip_heard_window(s: &Session, clip: &Clip, start: Samples, n: usize) -> Option<Vec<Vec<f32>>> {
    let ClipContent::Audio { source, .. } = clip.content else { return None };
    let audio = s.pool.get(source)?;
    let alt = clip.clean_alt().filter(|k| k.wet() > 0.0).and_then(|k| s.pool.get(k.source).map(|a| (k.wet(), k.origin, a)));
    let nalt = alt.map_or(0, |(_, _, a)| a.num_channels());
    let edges = alt.map(|(_, origin, a)| CleanEdges::new(s, clip, origin, a.frames()));
    Some(
        (0..audio.num_channels())
            .map(|ch| {
                let mut src = audio.cursor(ch, false);
                let mut cleaned = alt.filter(|_| nalt > 0).map(|(w, origin, a)| (w, origin, a.cursor(ch % nalt, false)));
                (0..n)
                    .map(|k| {
                        let at = start.saturating_add(k as i64);
                        let v = usize::try_from(at).ok().and_then(|x| src.get(x)).unwrap_or(0.0);
                        match cleaned
                            .as_mut()
                            .and_then(|(w, origin, c)| usize::try_from(at.saturating_sub(*origin)).ok().and_then(|x| c.get(x)).map(|cv| (*w, cv)))
                        {
                            Some((w, cv)) => blend(v, cv, edges.map_or(w, |e| e.weight(w, at as f64))),
                            None => v,
                        }
                    })
                    .collect()
            })
            .collect(),
    )
}

/// How long the crossfade is where the cleaned audio ends inside a clip, seconds.
pub const CLEAN_EDGE_SECS: f64 = 0.005;

/// Where Clean Vocal's cleaned audio starts or ends inside the clip's window (a trim opened past
/// it), the cleaned share fades in or out over [`CLEAN_EDGE_SECS`] inside the cleaned part, so the
/// switch to the original doesn't click. Ends at the window's own edges don't fade.
#[derive(Debug, Clone, Copy)]
struct CleanEdges {
    origin: f64,
    end: f64,
    ramp_in: bool,
    ramp_out: bool,
    len: f64,
}

impl CleanEdges {
    fn new(s: &Session, clip: &Clip, origin: Samples, frames: usize) -> CleanEdges {
        let stretch = if clip.stretch.is_finite() && clip.stretch > 0.0 { clip.stretch } else { 1.0 };
        let (w0, w1) = (clip.source_offset() as f64, clip.source_offset() as f64 + clip.length as f64 / stretch);
        let (origin, end) = (origin as f64, origin as f64 + frames as f64);
        CleanEdges { origin, end, ramp_in: w0 < origin, ramp_out: w1 > end, len: (s.sample_rate.as_f64() * CLEAN_EDGE_SECS).max(1.0) }
    }

    /// The cleaned share `w` at source position `pos`.
    fn weight(&self, w: f32, pos: f64) -> f32 {
        let mut k = 1.0f64;
        if self.ramp_in {
            k = k.min((pos - self.origin + 1.0) / self.len);
        }
        if self.ramp_out {
            k = k.min((self.end - pos) / self.len);
        }
        w * k.clamp(0.0, 1.0) as f32
    }
}

/// `(1 − w) · dry + w · wet`, exactly `wet` at `w = 1`.
fn blend(dry: f32, wet: f32, w: f32) -> f32 {
    if w >= 1.0 { wet } else { (1.0 - w) * dry + w * wet }
}

/// `src` at the fractional position `f` (linear interpolation), `None` outside it (the sample after
/// the last counts as silence, as the plain read does).
fn lerp_at(src: &mut soundcraft_model::SampleCursor<'_>, f: f64) -> Option<f32> {
    if !f.is_finite() || f < 0.0 {
        return None;
    }
    let x0 = f.floor();
    let fr = (f - x0) as f32;
    let i = x0 as usize;
    let a = src.get(i)?;
    let b = src.get(i.saturating_add(1)).unwrap_or(0.0);
    Some(a + (b - a) * fr)
}

/// The track's MIDI Track Offset (`midi.offset.<id>` in the edit values), in samples. Looked up
/// without formatting the key, since this runs on the audio thread every block.
fn midi_offset(s: &Session, t: &Track) -> i64 {
    const PREFIX: &str = "midi.offset.";
    s.edit
        .values
        .range::<str, _>((std::ops::Bound::Included(PREFIX), std::ops::Bound::Unbounded))
        .take_while(|(k, _)| k.starts_with(PREFIX))
        .find(|(k, _)| k.get(PREFIX.len()..).and_then(|id| id.parse::<u64>().ok()) == Some(t.id.0))
        .map(|(_, v)| *v)
        .filter(|v| v.is_finite())
        .map_or(0, |v| v as i64)
}

/// How much later than its clips' end the track's notes can sound: a positive MIDI Track Offset
/// (`midi.offset.<id>`), in samples; 0 otherwise. What a freeze render runs past the last clip
/// before its tail starts. A MIDI Real-Time Properties delay (`rtp.<id>.delay`) adds nothing: it
/// moves notes within their clip, and [`schedule_notes`] ends every note at its clip's end (a
/// note delayed past it does not play), so the clip's end still bounds them.
pub fn midi_lag(s: &Session, t: &Track) -> Samples {
    midi_offset(s, t).max(0)
}

/// Send note on/offs and controllers for MIDI clips overlapping the block.
///
/// A held pitch that no note of the playing sequences spans any more (a note edit, a cleanup
/// slider, the Cleaned/Original switch, a muted or removed clip moved its note-off behind the
/// playhead or dropped the note) gets its note-off at the block's start: otherwise it would ring
/// until the next stop or seek. Found in the same pass over the notes, with a table on the stack.
fn schedule_notes(s: &Session, t: &Track, pos: Samples, frames: usize, inst: &mut PluginSlot, held: &mut Vec<u8>) {
    let sr = s.sample_rate;
    // MIDI Track Offsets: the track's notes sound `offset` samples later (earlier if negative), so
    // the block looks that far back into the clips.
    let offset = midi_offset(s, t);
    let pos = pos.saturating_sub(offset);
    let end = pos.saturating_add(frames as i64);
    // Pitches some note sounds on in this block (started before its end, not ended before it).
    let mut alive = [false; 128];
    // A printed band clip (muted or not, in this block or not) may have left a note on any channel.
    let printed = t.clips().iter().any(|c| c.midi_channels);
    for clip in t.clips() {
        if clip.muted {
            continue;
        }
        let Some(sequence) = clip.midi_playing() else { continue };
        // Through the clip's end: a note-off clamped to it lands there, and when that is a block
        // boundary it plays in the block that starts on it.
        if clip.start >= end || clip.end() < pos {
            continue;
        }
        let base = s.tempo.samples_to_ticks(clip.start, sr);
        // A printed band clip plays each event on its own channel (`Clip::midi_channels`).
        let chans = clip.midi_channels;
        // MIDI Real-Time Properties (non-destructive velocity/transpose/duration/delay).
        let rtp = |name: &str, d: f64| Key::new(format_args!("rtp.{}.{name}", t.id.0)).map_or(d, |k| s.edit.value(k.as_str(), d));
        let (dv, dp, dur, delay) = if s.edit.values.is_empty() {
            (0i64, 0i64, 1.0f64, 0i64)
        } else {
            (
                rtp("velocity", 0.0).clamp(-127.0, 127.0) as i64,
                rtp("transpose", 0.0).clamp(-127.0, 127.0) as i64,
                rtp("duration", 100.0).clamp(1.0, 1000.0) / 100.0,
                rtp("delay", 0.0).clamp(-1e7, 1e7) as i64,
            )
        };
        let near = s.tempo.samples_to_ticks(pos, sr).saturating_sub(base).saturating_sub(1)
            ..=s.tempo.samples_to_ticks(end, sr).saturating_sub(base).saturating_add(1);
        let ctrls = |inst: &mut PluginSlot| {
            // Events before tick 0 are what a start trim hid: they don't play.
            for c in sequence.ctrls.iter().filter(|c| c.tick >= 0) {
                let tick = c.tick.saturating_add(delay).max(0);
                if !near.contains(&tick) {
                    continue;
                }
                let at = s.tempo.tick_to_samples(base.saturating_add(tick), sr);
                if at >= pos && at < end && at < clip.end() {
                    let offset = usize::try_from(at - pos).unwrap_or(0);
                    let mut m = ctrl_message(c.kind, c.value, dp);
                    if chans {
                        m[0] |= c.channel & 0x0F;
                        inst.plugin.midi_raw(offset, m);
                    } else {
                        inst.plugin.midi(offset, m);
                    }
                }
            }
        };
        // A printed clip's controllers go first, so one that sets up a note on the same tick
        // (an articulation, a sustain) arrives before it; the instrument keeps equal times in order.
        if chans {
            ctrls(inst);
        }
        for n0 in &sequence.notes {
            let mut n = *n0;
            n.velocity = (i64::from(n.velocity) + dv).clamp(1, 127) as u8;
            n.pitch = (i64::from(n.pitch) + dp).clamp(0, 127) as u8;
            n.length = ((n.length as f64 * dur) as i64).max(1);
            n.start = n.start.saturating_add(delay).max(0);
            let on = s.tempo.tick_to_samples(base.saturating_add(n.start), sr);
            let off = s.tempo.tick_to_samples(base.saturating_add(n.start).saturating_add(n.length), sr).min(clip.end());
            if on >= clip.end() {
                continue;
            }
            let ch = n.channel & 0x0F;
            if on < end
                && off >= pos
                && let Some(a) = alive.get_mut(usize::from(n.pitch & 0x7F))
            {
                *a = true;
            }
            if on >= pos && on < end {
                let offset = usize::try_from(on - pos).unwrap_or(0);
                if chans {
                    inst.plugin.midi_raw(offset, [0x90 | ch, n.pitch & 0x7F, n.velocity]);
                } else {
                    inst.plugin.note_on(offset, n.pitch, n.velocity);
                }
                if !held.contains(&n.pitch) && held.len() < 128 {
                    held.push(n.pitch);
                }
            }
            if off >= pos && off < end {
                let offset = usize::try_from(off - pos).unwrap_or(0);
                if chans {
                    inst.plugin.midi_raw(offset, [0x80 | ch, n.pitch & 0x7F, n.release_velocity & 0x7F]);
                } else {
                    inst.plugin.note_off(offset, n.pitch);
                }
                held.retain(|p| *p != n.pitch);
            }
        }
        if !chans {
            ctrls(inst);
        }
    }
    let sounds = |p: u8| alive.get(usize::from(p & 0x7F)).copied().unwrap_or(false);
    if held.iter().any(|p| !sounds(*p)) {
        for &p in held.iter().filter(|p| !sounds(**p)) {
            if printed {
                // Which channel it sounded on is not kept: close it on each.
                for ch in 0..16u8 {
                    inst.plugin.midi_raw(0, [0x80 | ch, p & 0x7F, 0]);
                }
            } else {
                inst.plugin.note_off(0, p);
            }
        }
        held.retain(|p| sounds(*p));
    }
}

/// The modules' merged events of this block into the instrument, channel kept (`midi_raw`),
/// keeping the strip's list of sounding notes by source so a source taken out of the set closes
/// its own (a seek, a stop or another input closes them all). Audio thread: no allocation
/// (`held` has its capacity from `Strip::new`; past it, a note is not tracked).
/// Whether `t` plays its live MIDI input now: input monitoring On always, Off never, Auto when
/// record-armed and MIDI Thru is on.
pub fn hears_live_midi(t: &Track, thru: bool) -> bool {
    matches!(t.mixer.input, Route::Midi(_))
        && match t.mixer.input_monitor {
            soundcraft_model::InputMonitor::On => true,
            soundcraft_model::InputMonitor::Off => false,
            soundcraft_model::InputMonitor::Auto => t.mixer.record_arm && thru,
        }
}

/// Note-offs for the notes `st`'s clips (and modules) hold on its instrument, which leave the live
/// input's notes sounding (unlike `all_notes_off`). A printed band clip's note is closed on every
/// channel (which one it sounded on is not kept), as `schedule_notes` does. No allocation.
fn release_clip_notes(s: &Session, id: TrackId, st: &mut Strip) {
    if let Some(inst) = st.instrument.as_mut() {
        if !st.held.is_empty() {
            let printed = s.tracks.iter().find(|t| t.id == id).is_some_and(|t| t.clips().iter().any(|c| c.midi_channels));
            for &p in &st.held {
                if printed {
                    for ch in 0..16u8 {
                        inst.plugin.midi_raw(0, [0x80 | ch, p & 0x7F, 0]);
                    }
                } else {
                    inst.plugin.note_off(0, p);
                }
            }
        }
        for &(_, ch, p) in &st.midi_held {
            inst.plugin.midi_raw(0, [0x80 | (ch & 0x0F), p & 0x7F, 0]);
        }
    }
    st.held.clear();
    st.midi_held.clear();
}

/// Plays the block's live MIDI that `f` lets through into the instrument, raw (channels kept),
/// noting what it holds.
fn deliver_live_midi(midi: &LiveMidi, f: MidiFilter, inst: &mut PluginSlot, held: &mut HeldNotes) {
    for e in midi.events() {
        if f.matches(midi, e) {
            held.track(e.port, e.msg);
            inst.plugin.midi_raw(e.offset as usize, e.msg);
        }
    }
}

fn deliver_module_midi(events: &StripMidi, inst: &mut PluginSlot, held: &mut Vec<(u8, u8, u8)>) {
    for e in events.events() {
        inst.plugin.midi_raw(e.offset as usize, e.msg);
        let key = (e.src, e.msg[0] & 0x0F, e.msg[1] & 0x7F);
        match e.msg[0] & 0xF0 {
            0x90 if e.msg[2] > 0 => {
                if !held.contains(&key) && held.len() < held.capacity() {
                    held.push(key);
                }
            }
            0x80 | 0x90 => held.retain(|k| *k != key),
            _ => {}
        }
    }
}

/// A controller event as a MIDI 1.0 message on channel 1 (where the notes go), with `transpose`
/// applied to a poly-pressure key.
fn ctrl_message(kind: CtrlKind, value: i32, transpose: i64) -> [u8; 3] {
    let v7 = value.clamp(0, 127) as u8;
    match kind {
        CtrlKind::Cc(n) => [0xB0, n & 0x7F, v7],
        CtrlKind::PitchBend => {
            let v = value.saturating_add(8192).clamp(0, 16383);
            [0xE0, (v & 0x7F) as u8, (v >> 7) as u8]
        }
        CtrlKind::ChannelPressure => [0xD0, v7, 0],
        CtrlKind::PolyPressure(k) => [0xA0, (i64::from(k) + transpose).clamp(0, 127) as u8, v7],
        CtrlKind::Program => [0xC0, v7, 0],
    }
}

/// Before an instrument plays from `pos`: reset its controllers, then send each one the last value
/// it had on this track before `pos`. Clips play on channel 1 and are chased there; a printed band
/// clip (`Clip::midi_channels`) plays each event on its own channel, so its controllers are chased
/// per (channel, controller): CC121 and the chased values go out on every channel such a clip uses
/// (an organ's lower manual too). Audio thread: no allocation (one 131-entry table per pass).
fn chase_controllers(s: &Session, t: &Track, pos: Samples, inst: &mut PluginSlot) {
    inst.plugin.midi(0, [0xB0, 121, 0]);
    let pos = pos.saturating_sub(midi_offset(s, t));
    chase_channel(s, t, pos, None, inst);
    let mut used = 0u16;
    for clip in t.clips().iter().filter(|c| c.midi_channels && !c.muted) {
        let Some(sequence) = clip.midi_playing() else { continue };
        for n in &sequence.notes {
            used |= 1 << (n.channel & 0x0F);
        }
        for c in &sequence.ctrls {
            used |= 1 << (c.channel & 0x0F);
        }
    }
    for ch in (0..16u8).filter(|ch| used & (1 << ch) != 0) {
        inst.plugin.midi_raw(0, [0xB0 | ch, 121, 0]);
        chase_channel(s, t, pos, Some(ch), inst);
    }
}

/// [`chase_controllers`] for one channel: `None` = the clips that play on channel 1 (every clip but
/// a printed one), `Some(ch)` = the printed clips' controllers on channel `ch`.
fn chase_channel(s: &Session, t: &Track, pos: Samples, ch: Option<u8>, inst: &mut PluginSlot) {
    const BEND: usize = 128;
    const PRESSURE: usize = 129;
    const PROGRAM: usize = 130;
    let mut last = [(i64::MIN, 0i32); 131];
    let sr = s.sample_rate;
    for clip in t.clips() {
        let Some(sequence) = clip.midi_playing() else { continue };
        if clip.muted || clip.start >= pos || clip.midi_channels != ch.is_some() {
            continue;
        }
        let base = s.tempo.samples_to_ticks(clip.start, sr);
        let until = pos.min(clip.end());
        for c in sequence.ctrls.iter().filter(|c| c.tick >= 0 && ch.is_none_or(|ch| c.channel & 0x0F == ch)) {
            let slot = match c.kind {
                CtrlKind::Cc(n) => usize::from(n & 0x7F),
                CtrlKind::PitchBend => BEND,
                CtrlKind::ChannelPressure => PRESSURE,
                CtrlKind::Program => PROGRAM,
                CtrlKind::PolyPressure(_) => continue,
            };
            let at = s.tempo.tick_to_samples(base.saturating_add(c.tick), sr);
            if at < until
                && let Some(l) = last.get_mut(slot)
                && at >= l.0
            {
                *l = (at, c.value);
            }
        }
    }
    for (slot, &(at, value)) in last.iter().enumerate() {
        let kind = match slot {
            BEND => CtrlKind::PitchBend,
            PRESSURE => CtrlKind::ChannelPressure,
            PROGRAM => CtrlKind::Program,
            n => CtrlKind::Cc(u8::try_from(n).unwrap_or(0)),
        };
        if at != i64::MIN {
            let mut m = ctrl_message(kind, value, 0);
            match ch {
                None => inst.plugin.midi(0, m),
                Some(ch) => {
                    m[0] |= ch;
                    inst.plugin.midi_raw(0, m);
                }
            }
        }
    }
}

/// Session audio shared with a sample-playing instrument.
/// The engine keeps sampler sources in memory: one played from disk plays nothing here.
struct PoolSample(Arc<SourceAudio>);

impl SampleSource for PoolSample {
    fn sample_rate(&self) -> f32 {
        self.0.sample_rate() as f32
    }
    fn num_channels(&self) -> usize {
        if self.0.in_memory().is_some() { self.0.num_channels() } else { 0 }
    }
    fn channel(&self, ch: usize) -> Option<&[f32]> {
        self.0.channel(ch)
    }
}

/// Capacity reserved per strip for assets its plugins let go of between two syncs.
const RELEASED_RESERVE: usize = 16;

/// The asset a session has ready for `reference` at `rate` (`None`: not made, or not a
/// `SampleSource`).
fn session_asset(s: &Session, key: &str, reference: &str, rate: u32) -> Option<Arc<dyn SampleSource>> {
    s.assets.get_for(key, reference, rate).and_then(|d| d.0.downcast_ref::<Arc<dyn SampleSource>>()).cloned()
}

/// Hands a module's plugin its link group's slab (`Insert::link_group`) and member index when
/// they changed: a new group, none, a grown slab, or a new instance. The slot's previous
/// reference goes to `retired` (the plugin may have held the last other one). Part of `sync`.
fn sync_link(slot: &mut PluginSlot, track: TrackId, ins: &soundcraft_model::Insert, links: &[link::LinkGroup], retired: &mut Vec<Arc<LinkSlab>>) {
    let want =
        ins.link_group.and_then(|g| links.iter().find(|x| x.group == g)).and_then(|g| Some((Arc::clone(&g.slab), g.member_of(track, ins.id)?)));
    let same = match (&slot.link, &want) {
        (Some((a, i)), Some((b, j))) => Arc::ptr_eq(a, b) && i == j,
        (None, None) => true,
        _ => false,
    };
    if !same {
        slot.plugin.set_link(want.as_ref().map(|(a, _)| Arc::clone(a)), want.as_ref().map_or(0, |(_, i)| *i));
        if let Some((old, _)) = std::mem::replace(&mut slot.link, want) {
            retired.push(old);
        }
    }
}

/// Hands a module's plugin its assets (`Insert::assets`) where they changed: a new one, a cleared
/// one, or a new instance. The engine keeps its own reference to each; the ones it replaces go to
/// `retired` (dropped off the audio thread).
fn sync_assets(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, s: &Session, rate: u32, retired: &mut Vec<Arc<dyn SampleSource>>) {
    let same = |a: &Arc<dyn SampleSource>, b: &Arc<dyn SampleSource>| std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b));
    for (key, reference) in &ins.assets {
        let want = session_asset(s, key, reference, rate);
        let at = slot.assets.iter().position(|(k, _)| k == key);
        match (want, at) {
            (Some(w), Some(i)) if same(&w, &slot.assets[i].1) => {}
            (Some(w), at) => {
                slot.plugin.set_asset(key, Some(Arc::clone(&w)));
                match at {
                    Some(i) => retired.push(std::mem::replace(&mut slot.assets[i].1, w)),
                    None => slot.assets.push((key.clone(), w)),
                }
            }
            (None, Some(i)) => {
                slot.plugin.set_asset(key, None);
                retired.push(slot.assets.remove(i).1);
            }
            (None, None) => {}
        }
    }
    // keys the module no longer has
    let mut i = 0;
    while i < slot.assets.len() {
        if ins.assets.contains_key(&slot.assets[i].0) {
            i += 1;
        } else {
            let (key, a) = slot.assets.remove(i);
            slot.plugin.set_asset(&key, None);
            retired.push(a);
        }
    }
    while let Some(a) = slot.plugin.take_released_asset() {
        retired.push(a);
    }
}

/// Collects what a plugin let go of while rendering, into the capacity `sync` reserved (past it,
/// the plugin keeps holding them until the next sync takes them).
fn collect_released(slot: &mut PluginSlot, out: &mut Vec<Arc<dyn SampleSource>>) {
    while out.len() < out.capacity() {
        match slot.plugin.take_released_asset() {
            Some(a) => out.push(a),
            None => break,
        }
    }
}

/// Hands a strip's instrument the audio of its insert's sample source when it changed (a new
/// sample, a cleared one, or a new instance). Missing media plays nothing.
fn sync_sample(strip: &mut Strip, want: Option<&soundcraft_model::Insert>, s: &Session) {
    let audio = want.and_then(|i| i.sample).and_then(|id| s.pool.get(id));
    let same = match (strip.sample.as_ref(), audio) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    if let Some(inst) = strip.instrument.as_mut() {
        let shared = audio.map(|a| Arc::new(PoolSample(Arc::clone(a))) as Arc<dyn SampleSource>);
        inst.plugin.set_sample(shared);
    }
    strip.sample = audio.cloned();
}

/// Record insert latency without processing audio, so this block can compensate for it.
fn note_insert_latency(t: &Track, strip: &mut Strip, pos: Samples) {
    let mut latency = 0usize;
    for (i, ins) in t.mixer.inserts.iter().enumerate() {
        let Some(Some(slot)) = strip.plugins.get_mut(i) else { continue };
        if slot.module != ins.id || !ins.active || ins.bypass || slot.failed {
            continue;
        }
        apply_params(slot, ins, t, Some(ins.id), pos);
        latency = latency.saturating_add(slot.plugin.latency());
        // The wait a sidechain key imposed on the main input last block.
        if ins.sidechain.is_some()
            && let Some(k) = strip.keys.iter().find(|k| k.module == ins.id)
        {
            latency = latency.saturating_add(k.main_delay.delay());
        }
    }
    strip.latency = latency.min(MAX_LATENCY);
}

fn apply_params(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, t: &Track, module: Option<soundcraft_model::ModuleId>, pos: Samples) {
    apply_prepared(slot, ins);
    // Automation lanes are keyed by module id; the instrument has none, so it picks up no lanes.
    let Some(module) = module else {
        return apply_values(slot, ins, |_, v| (v, false));
    };
    let automated = t.mixer.automation_mode.reads()
        && t.automation.iter().any(|l| matches!(&l.param, AutoParam::Plugin { module: m, .. } if *m == module) && !l.points.is_empty());
    apply_values(slot, ins, |k, val| {
        if automated && let Some(l) = plugin_lane(t, module, k) {
            return (l.value_at(pos, val), true);
        }
        (val, false)
    });
}

/// The lane that drives param `param` of chain module `module` as the render reads it: automation
/// is read on the track and the lane has points. Shared by `apply_params` and
/// [`scout::automated_params`].
fn plugin_lane<'a>(t: &'a Track, module: ModuleId, param: &str) -> Option<&'a soundcraft_model::AutomationLane> {
    if !t.mixer.automation_mode.reads() {
        return None;
    }
    t.automation
        .iter()
        .find(|l| matches!(&l.param, AutoParam::Plugin { module: m, param: p } if *m == module && p == param))
        .filter(|l| !l.points.is_empty())
}

/// Hands the plugin what its stored state was made ready as ([`Insert::prepared`], made off the
/// audio thread) when that changed since last time: a new preparation, a cleared one, or a new
/// instance. A copy into the plugin's own storage, so nothing is allocated or freed here; the
/// numbers stay owned by the session snapshot, which the host drops off the audio thread.
///
/// [`Insert::prepared`]: soundcraft_model::Insert::prepared
fn apply_prepared(slot: &mut PluginSlot, ins: &soundcraft_model::Insert) {
    let rev = ins.prepared.as_ref().map_or(0, |p| p.rev);
    if rev != slot.prepared {
        let data: &[f64] = ins.prepared.as_ref().map_or(&[], |p| &p.data);
        slot.plugin.set_prepared_state(data);
        slot.prepared = rev;
    }
}

/// Sends each stored param (as mapped by `value`, which also says whether a lane drives it) to
/// the plugin when it changed since last time, telling the plugin first when a param's lane comes
/// or goes.
fn apply_values(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, value: impl Fn(&str, f32) -> (f32, bool)) {
    let keys = keys_hash(ins);
    if keys != slot.keys || slot.applied.len() != ins.params.len() || slot.laned.len() != ins.params.len() {
        // The stored params changed their keys (added, removed or swapped, whatever the length),
        // so the old per-position bookkeeping no longer lines up: resend every value, and tell the
        // plugin nothing is laned (every one of its params, a removed key included) before the
        // loop below tells it again what is laned now. Without this a flag the plugin holds
        // (Dist's laned Level) could go stale with nothing ever clearing it. Filled in place: no
        // allocation unless the params grew past the room made when the slot was built.
        for p in slot.plugin.info().params {
            slot.plugin.set_param_automated(p.id, false);
        }
        slot.applied.clear();
        slot.applied.resize(ins.params.len(), f32::NAN);
        slot.laned.clear();
        slot.laned.resize(ins.params.len(), false);
        slot.keys = keys;
    }
    for (((k, v), last), was_laned) in ins.params.iter().zip(slot.applied.iter_mut()).zip(slot.laned.iter_mut()) {
        let (val, laned) = value(k, *v);
        if laned != *was_laned {
            slot.plugin.set_param_automated(k, laned);
            *was_laned = laned;
        }
        if val.to_bits() != last.to_bits() {
            slot.plugin.set_param(k, val);
            *last = val;
        }
    }
}

/// FNV-1a over the stored params' keys, in their (sorted) order, with a separator: tells one key
/// set from another without allocating.
fn keys_hash(ins: &soundcraft_model::Insert) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for k in ins.params.keys() {
        for b in k.bytes().chain(std::iter::once(0xff)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn transport_at(s: &Session, pos: Samples, playing: bool, recording: bool) -> Transport {
    let sr = s.sample_rate;
    let ticks = Some(s.tempo.samples_to_ticks_f(pos, sr)).filter(|t| t.is_finite()).unwrap_or(0.0);
    let tick = ticks.floor() as i64;
    let meter = s.tempo.meter_at_tick(tick);
    let bar = s.tempo.bar_beat_at_tick(tick).bar;
    let q = TICKS_PER_QUARTER as f64;
    Transport {
        playing,
        recording,
        tempo: s.tempo.tempo_at_tick(tick),
        numerator: meter.numerator,
        denominator: meter.denominator,
        samples: pos,
        seconds: pos as f64 / sr.as_f64().max(1.0),
        quarters: ticks / q,
        bar_start: s.tempo.bar_start_tick(bar) as f64 / q,
        bar: bar - 1,
    }
}

fn volume_db_at(t: &Track, pos: Samples) -> f32 {
    if t.mixer.automation_mode.reads()
        && t.mixer.automation_mode != soundcraft_model::AutomationMode::Write
        && let Some(l) = t.lane(&AutoParam::Volume)
        && !l.points.is_empty()
    {
        return l.value_at(pos, t.mixer.volume_db);
    }
    t.mixer.volume_db
}

fn pan_at(t: &Track, idx: usize, pos: Samples) -> f32 {
    let base = t.mixer.pan.get(idx).copied().unwrap_or(if idx == 0 { 0.0 } else { 1.0 });
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.lane(&AutoParam::Pan(u8::try_from(idx).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return l.value_at(pos, base);
    }
    base
}

/// Pan automation is read on a grid of [`PAN_STEP`] timeline samples rather than once per block,
/// so playback (callback-sized blocks) and bounces (1024-frame blocks) pan identically.
const PAN_STEP: Samples = 64;

/// The position to read the pan at for the frames from `start` of the block at `pos`, and the end
/// of the frames that share it. Without pan automation the whole block shares one pan.
fn pan_segment(t: &Track, pos: Samples, start: usize, frames: usize) -> (Samples, usize) {
    let automated = t.mixer.automation_mode.reads() && t.automation.iter().any(|l| matches!(l.param, AutoParam::Pan(_)) && !l.points.is_empty());
    if !automated {
        return (pos, frames);
    }
    let here = pos.saturating_add(start as i64);
    let at = here.div_euclid(PAN_STEP).saturating_mul(PAN_STEP);
    let left = usize::try_from(at.saturating_add(PAN_STEP).saturating_sub(here)).unwrap_or(1).max(1);
    (at, start.saturating_add(left).min(frames))
}

fn send_level(t: &Track, i: usize, base: f32, pos: Samples) -> f32 {
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.lane(&AutoParam::SendLevel(u8::try_from(i).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return l.value_at(pos, base);
    }
    base
}

/// A send's pan as its automation plays it (read like [`send_level`]).
fn send_pan(t: &Track, i: usize, base: f32, pos: Samples) -> f32 {
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.lane(&AutoParam::SendPan(u8::try_from(i).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return l.value_at(pos, base);
    }
    base
}

/// Whether a send is muted: its key, or its mute automation.
fn send_muted(t: &Track, i: usize, base: bool, pos: Samples) -> bool {
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.lane(&AutoParam::SendMute(u8::try_from(i).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return base || l.value_at(pos, 0.0) >= 0.5;
    }
    base
}

fn lerp_gain(db0: f32, db1: f32, i: usize, n: usize) -> f32 {
    let g0 = db_to_gain(db0);
    let g1 = db_to_gain(db1);
    if n == 0 { g0 } else { g0 + (g1 - g0) * (i as f32 / n as f32) }
}

/// A linear fade to silence over the first `frames` samples of `c` (the last one silent).
fn fade_out(c: &mut [f32], frames: usize) {
    let n = frames.min(c.len());
    let last = n.saturating_sub(1).max(1) as f32;
    for (i, x) in c.iter_mut().take(n).enumerate() {
        *x *= 1.0 - i as f32 / last;
    }
}

/// Linear maximum over the first `frames` samples of every channel.
fn chain_peak(bufs: &[Vec<f32>], frames: usize) -> f32 {
    bufs.iter().fold(0.0f32, |m, b| m.max(peak(b.get(..frames).unwrap_or(b))))
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { m })
}

/// Per-block routing gains: `m[src][dst]`.
type Matrix = [[f32; MAX_CHANNELS]; MAX_CHANNELS];

/// How a source is panned: the stereo pan pair (mono uses `[0]`) and the optional surround pan.
struct PanSpec<'a> {
    stereo: [f32; 2],
    surround: Option<&'a SurroundPan>,
}

fn identity_matrix(n: usize) -> Matrix {
    let mut m = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
    for (i, row) in m.iter_mut().enumerate().take(n) {
        if let Some(g) = row.get_mut(i) {
            *g = 1.0;
        }
    }
    m
}

/// ITU-R BS.775 stereo fold-down gains (to L, to R) for one speaker: C and surrounds at -3 dB,
/// the centre surround at -6 dB per side, LFE dropped.
pub fn itu_stereo_fold(sp: Speaker) -> (f32, f32) {
    use Speaker::*;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    match sp {
        L | Lc | Lw => (1.0, 0.0),
        R | Rc | Rw => (0.0, 1.0),
        C => (h, h),
        Lfe => (0.0, 0.0),
        Cs => (0.5, 0.5),
        Ls | Lss | Lrs | Ltf | Ltm | Ltr => (h, 0.0),
        Rs | Rss | Rrs | Rtf | Rtm | Rtr => (0.0, h),
    }
}

/// Speaker directions of a format (for the DSP panner). Returns the count written.
fn layout_of(fmt: ChannelFormat, out: &mut [SpeakerPos; MAX_CHANNELS]) -> usize {
    let sp = fmt.speakers();
    for (o, s) in out.iter_mut().zip(sp.iter()) {
        *o = SpeakerPos { az: s.azimuth(), el: s.elevation(), lfe: s.is_lfe() };
    }
    sp.len().min(MAX_CHANNELS)
}

fn surround_params(sp: &SurroundPan, dx: f32) -> SurroundParams {
    SurroundParams { x: (sp.x + dx).clamp(-1.0, 1.0), y: sp.y, z: sp.z, divergence: sp.divergence, center: sp.center / 100.0 }
}

/// Routing gains from a source (`ns` channels of `src_fmt`) into a destination (`nd` channels of
/// `dst_fmt`). See the crate docs for the rules.
fn build_matrix(src_fmt: ChannelFormat, ns: usize, dst_fmt: ChannelFormat, nd: usize, pan: &PanSpec<'_>, law: PanLaw) -> Matrix {
    let mut m = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
    let (ns, nd) = (ns.min(MAX_CHANNELS), nd.min(MAX_CHANNELS));
    if ns == 0 || nd == 0 {
        return m;
    }
    let set = |m: &mut Matrix, s: usize, d: usize, g: f32| {
        if let Some(x) = m.get_mut(s).and_then(|r| r.get_mut(d)) {
            *x += g;
        }
    };
    if nd == 1 {
        // Mono destination: the stereo routing summed at -6 dB.
        let st = build_matrix(src_fmt, ns, ChannelFormat::Stereo, 2, pan, law);
        for (row, srow) in m.iter_mut().zip(st.iter()).take(ns) {
            row[0] = 0.5 * (srow[0] + srow[1]);
        }
        return m;
    }
    let src_sp = src_fmt.speakers();
    let src_amb = src_fmt.is_ambisonic() && ns >= 4;
    if nd == 2 && !dst_fmt.is_ambisonic() {
        match ns {
            1 => {
                let (gl, gr) = gains(pan.stereo[0], law);
                m[0][0] = gl;
                m[0][1] = gr;
            }
            2 if !src_amb => {
                let (a_l, a_r) = gains(pan.stereo[0], PanLaw::Zero);
                let (b_l, b_r) = gains(pan.stereo[1], PanLaw::Zero);
                m[0] = [0.0; MAX_CHANNELS];
                m[0][0] = a_l;
                m[0][1] = a_r;
                m[1][0] = b_l;
                m[1][1] = b_r;
            }
            _ if src_amb => {
                for (d, az) in [(0usize, -30.0f32), (1, 30.0)] {
                    let g = soundcraft_dsp::pan::ambisonic_decode(az, 0.0, 2);
                    for (c, gc) in g.iter().enumerate() {
                        set(&mut m, c, d, *gc);
                    }
                }
            }
            _ if src_sp.len() == ns => {
                for (s, sp) in src_sp.iter().enumerate() {
                    let (l, r) = itu_stereo_fold(*sp);
                    set(&mut m, s, 0, l);
                    set(&mut m, s, 1, r);
                }
            }
            _ => {
                // Unknown layout: first two channels to L/R, the rest to the centre.
                let h = std::f32::consts::FRAC_1_SQRT_2;
                m[0][0] = 1.0;
                m[1][1] = 1.0;
                for row in m.iter_mut().take(ns).skip(2) {
                    row[0] = h;
                    row[1] = h;
                }
            }
        }
        return m;
    }
    if ns > 2 && src_fmt == dst_fmt && ns == nd {
        return identity_matrix(ns);
    }
    // Multichannel destination.
    let mut layout = [SpeakerPos::default(); MAX_CHANNELS];
    let nl = layout_of(dst_fmt, &mut layout);
    let layout = layout.get(..nl).unwrap_or(&[]);
    let dst_sp = dst_fmt.speakers();
    let dst_amb = dst_fmt.is_ambisonic();
    let find = |sp: Speaker| dst_sp.iter().position(|x| *x == sp).filter(|i| *i < nd);
    let mut g = [0.0f32; MAX_CHANNELS];
    if dst_amb {
        if src_amb || (src_fmt.is_ambisonic() && ns > 2) {
            return identity_matrix(ns.min(nd));
        }
        let mut enc = |m: &mut Matrix, s: usize, az: f32, el: f32, spread: f32| {
            soundcraft_dsp::pan::ambisonic_encode(az, el, spread, &mut g);
            for (d, gd) in g.iter().enumerate().take(nd) {
                set(m, s, d, *gd);
            }
        };
        match (ns, pan.surround) {
            (1 | 2, Some(sp)) => {
                for s in 0..ns {
                    let dx = if ns == 2 { if s == 0 { -1.0 } else { 1.0 } } else { 0.0 };
                    let p = surround_params(sp, dx);
                    enc(&mut m, s, soundcraft_dsp::pan::puck_azimuth(&[], p.x, p.y), p.z * 90.0, p.divergence);
                }
            }
            (1, None) => enc(&mut m, 0, pan.stereo[0].clamp(-1.0, 1.0) * 30.0, 0.0, 0.0),
            (2, None) => {
                enc(&mut m, 0, -30.0, 0.0, 0.0);
                enc(&mut m, 1, 30.0, 0.0, 0.0);
            }
            _ => {
                for (s, sp) in src_sp.iter().enumerate().take(ns) {
                    if !sp.is_lfe() {
                        enc(&mut m, s, sp.azimuth(), sp.elevation(), 0.0);
                    }
                }
            }
        }
        return m;
    }
    if dst_sp.len() != nd {
        // No layout to pan on: channel for channel.
        return identity_matrix(ns.min(nd));
    }
    let (li, ri) = (find(Speaker::L), find(Speaker::R));
    match (ns, pan.surround) {
        (1 | 2, Some(sp)) if !src_amb => {
            for s in 0..ns {
                let dx = if ns == 2 { if s == 0 { -1.0 } else { 1.0 } } else { 0.0 };
                soundcraft_dsp::pan::surround_gains(layout, &surround_params(sp, dx), &mut g);
                for (d, gd) in g.iter().enumerate().take(nd) {
                    set(&mut m, s, d, *gd);
                }
                if let Some(lfe) = find(Speaker::Lfe)
                    && sp.lfe_db > -143.9
                {
                    set(&mut m, s, lfe, db_to_gain(sp.lfe_db.min(12.0)) / ns as f32);
                }
            }
        }
        (1, None) => {
            let (gl, gr) = gains(pan.stereo[0], law);
            if let (Some(l), Some(r)) = (li, ri) {
                set(&mut m, 0, l, gl);
                set(&mut m, 0, r, gr);
            }
        }
        (2, None) if !src_amb => {
            let (a_l, a_r) = gains(pan.stereo[0], PanLaw::Zero);
            let (b_l, b_r) = gains(pan.stereo[1], PanLaw::Zero);
            if let (Some(l), Some(r)) = (li, ri) {
                set(&mut m, 0, l, a_l);
                set(&mut m, 0, r, a_r);
                set(&mut m, 1, l, b_l);
                set(&mut m, 1, r, b_r);
            }
        }
        _ if src_amb => {
            let n_spk = dst_sp.iter().filter(|s| !s.is_lfe()).count();
            for (d, sp) in dst_sp.iter().enumerate().take(nd) {
                if sp.is_lfe() {
                    continue;
                }
                let dec = soundcraft_dsp::pan::ambisonic_decode(sp.azimuth(), sp.elevation(), n_spk);
                for (c, gc) in dec.iter().enumerate() {
                    set(&mut m, c, d, *gc);
                }
            }
        }
        _ if src_sp.len() == ns => {
            for (s, sp) in src_sp.iter().enumerate() {
                if let Some(d) = find(*sp) {
                    set(&mut m, s, d, 1.0);
                } else if sp.is_lfe() {
                    // No LFE in the destination: dropped.
                } else {
                    soundcraft_dsp::pan::direction_gains(layout, sp.azimuth(), sp.elevation(), &mut g);
                    for (d, gd) in g.iter().enumerate().take(nd) {
                        set(&mut m, s, d, *gd);
                    }
                }
            }
        }
        _ => return identity_matrix(ns.min(nd)),
    }
    m
}

/// `dst[d] += Σ_s src[s] × gain × m[s][d]` for every destination channel (no allocation).
fn apply_matrix(src: &[Vec<f32>], frames: usize, dst: &mut [Vec<f32>], m: &Matrix, gain: f32) {
    apply_matrix_span(src, 0..frames, dst, m, gain);
}

/// [`apply_matrix`] over the frames in `span` only.
fn apply_matrix_span(src: &[Vec<f32>], span: std::ops::Range<usize>, dst: &mut [Vec<f32>], m: &Matrix, gain: f32) {
    let ns = src.len().min(MAX_CHANNELS);
    for (d, out) in dst.iter_mut().enumerate().take(MAX_CHANNELS) {
        let mut idx = [(0usize, 0.0f32); MAX_CHANNELS];
        let mut k = 0;
        for (s, row) in m.iter().enumerate().take(ns) {
            let g = row.get(d).copied().unwrap_or(0.0);
            if g != 0.0
                && let Some(slot) = idx.get_mut(k)
            {
                *slot = (s, g);
                k += 1;
            }
        }
        let taps = idx.get(..k).unwrap_or(&[]);
        if taps.is_empty() {
            continue;
        }
        for (i, o) in out.iter_mut().enumerate().take(span.end).skip(span.start) {
            let mut acc = 0.0f32;
            for &(s, g) in taps {
                acc += src.get(s).and_then(|c| c.get(i)).copied().unwrap_or(0.0) * gain * g;
            }
            *o += acc;
        }
    }
}

/// Ceiling on one in-memory offline render (samples summed over channels): 2^29 f32 = 2 GiB, about
/// 93 minutes of stereo at 48 kHz. Longer requests are truncated rather than exhausting memory;
/// [`RangeRender`] streams any length.
pub const MAX_RENDER_SAMPLES: usize = 1 << 29;

/// Offline render of the main mix over `range` (planar, [`main_channels`] channels in SMPTE/WAV
/// order). The mix's delay-compensation latency is removed, so the result lines up with the
/// timeline.
pub fn render_range(s: &Session, range: Range, block: usize) -> Vec<Vec<f32>> {
    render_in_memory(s, None, range, block)
}

/// Offline render of one bus over `range` (planar, the bus format's channel count).
/// The compensation delay written onto the bus is removed (track inserts, and the aux returns
/// when an aux returns onto it), so the bus lines up with the timeline. An unknown bus is silent.
pub fn render_bus(s: &Session, bus: BusId, range: Range, block: usize) -> Vec<Vec<f32>> {
    render_in_memory(s, Some(bus), range, block)
}

fn render_in_memory(s: &Session, bus: Option<BusId>, range: Range, block: usize) -> Vec<Vec<f32>> {
    let nch = source_channels(s, bus);
    let len = usize::try_from(range.len().max(0)).unwrap_or(0).min(MAX_RENDER_SAMPLES / nch.max(1));
    let mut out: Vec<Vec<f32>> = (0..nch).map(|_| Vec::with_capacity(len)).collect();
    let mut render = RangeRender::new(s, bus, Range::new(range.start, range.start + len as i64), block);
    while let Some((chunk, n)) = render.next_block(s) {
        for (o, c) in out.iter_mut().zip(chunk) {
            o.extend_from_slice(c.get(..n).unwrap_or_default());
        }
    }
    out
}

/// Channels of the main mix, or of `bus` (none for an unknown bus).
pub fn source_channels(s: &Session, bus: Option<BusId>) -> usize {
    match bus {
        Some(id) => s.bus(id).map_or(0, |b| b.format.channels()),
        None => main_channels(s),
    }
}

/// The realtime host's own engine, with every plugin instance that plays, lent to an offline
/// render (Bounce, Freeze, Commit): hosted plugins render through the very instances the person
/// hears, Pro Tools offline-bounce style, instead of through copies made from their stored
/// state. The host stops its transport and hands the engine over (outputting silence meanwhile);
/// [`RangeRender::with_instances`] renders through it at the live sample rate and block size;
/// dropping this (after the render, or when it is cancelled) syncs the engine back to the
/// host's session, resets it and gives it back through `back`, on the thread that drops it.
///
/// While lent, `sync` never retires a third-party instance: one the render's session has no
/// place for (a module after a freeze point, a strip of another width) is parked and seated
/// again on the way back (see `MixEngine::parked`).
pub struct LiveInstances {
    eng: Option<Box<MixEngine>>,
    lender: Option<Lender>,
}

/// The host's session and settings the lent engine goes back to.
struct Lender {
    session: Arc<Session>,
    back: Box<dyn FnOnce(Box<MixEngine>) + Send>,
    offline: bool,
    metronome: bool,
    monitor_only: bool,
    recording: bool,
    /// The host's live-input buffers, kept out of the render (a monitored input would print).
    input: Vec<Vec<f32>>,
}

impl LiveInstances {
    /// Takes `eng` (synced to `session`, the host's) into render mode: offline (every hosted
    /// instance re-prepared in offline mode, see [`MixEngine::set_hosted_offline`]), no
    /// metronome, no input. `back` receives it once the render is over, with the instances back
    /// in realtime mode. Main thread only (the instances' mode change wants it). Built-in
    /// modules are rendered through as they are (no snapshot: they are `reset` on the way back,
    /// as a Stop resets them, so nothing audible is lost).
    pub fn new(mut eng: Box<MixEngine>, session: Arc<Session>, back: impl FnOnce(Box<MixEngine>) + Send + 'static) -> Self {
        eng.set_hosted_offline(true);
        let lender = Lender {
            session,
            back: Box::new(back),
            offline: eng.offline,
            metronome: eng.metronome,
            monitor_only: eng.monitor_only,
            recording: eng.recording,
            input: std::mem::take(&mut eng.input),
        };
        eng.offline = true;
        eng.metronome = false;
        eng.monitor_only = false;
        eng.recording = false;
        eng.lent = true;
        LiveInstances { eng: Some(eng), lender: Some(lender) }
    }

    /// Sample rate the instances run at.
    pub fn sample_rate(&self) -> f32 {
        self.eng.as_ref().map_or(0.0, |e| e.sample_rate)
    }

    /// The block size the instances are prepared for (the render's).
    pub fn max_block(&self) -> usize {
        self.eng.as_ref().map_or(0, |e| e.max_block)
    }

    /// How many third-party instances the engine holds (seated or parked).
    pub fn hosted_instances(&self) -> usize {
        self.eng.as_ref().map_or(0, |e| e.hosted_instances())
    }
}

impl Drop for LiveInstances {
    fn drop(&mut self) {
        let (Some(mut eng), Some(l)) = (self.eng.take(), self.lender.take()) else { return };
        eng.offline = l.offline;
        eng.metronome = l.metronome;
        eng.monitor_only = l.monitor_only;
        eng.recording = l.recording;
        eng.input = l.input;
        // Back in step with the host's session (the parked instances seated again), silent and
        // at rest: the host resets it at its position when it takes it back.
        eng.ensure_synced(&l.session);
        eng.lent = false;
        // Whatever the host's session has no place for either is retired after all.
        let stray: Vec<Box<dyn Plugin>> = eng.parked.drain().map(|(_, p)| p.plugin).collect();
        eng.retired.extend(stray);
        // Realtime again for the player (here, on the main thread, before the audio thread
        // gets the engine back).
        eng.set_hosted_offline(false);
        eng.reset();
        (l.back)(eng);
    }
}

impl MixEngine {
    /// Tells every third-party instance this engine holds (seated, waiting to be adopted or
    /// parked) that it renders offline from now on (or in realtime again) and prepares it again
    /// so the mode takes: VST3 deactivates, runs `setupProcessing` with `kOffline`/`kRealtime`
    /// and activates again; CLAP deactivates, sets `clap.render` and activates. The instance
    /// keeps its state. Main thread only (plugins want their activation there); built-in plugins
    /// ignore the mode and are left alone. A plugin that refuses the mode logs and renders on
    /// in realtime mode.
    fn set_hosted_offline(&mut self, offline: bool) {
        let (sr, mb) = (self.sample_rate, self.max_block);
        let again = |id: &str, ch: usize, plugin: &mut Box<dyn Plugin>| {
            if is_third_party(id) {
                plugin.set_offline(offline);
                plugin.prepare(sr, mb, ch);
            }
        };
        for st in self.strips.values_mut() {
            for slot in st.plugins.iter_mut().flatten().chain(st.instrument.iter_mut()) {
                again(&slot.id, slot.ch, &mut slot.plugin);
            }
        }
        for p in self.adopt.values_mut() {
            again(&p.id, p.ch, &mut p.plugin);
        }
        for p in self.parked.values_mut() {
            again(&p.id, p.ch, &mut p.plugin);
        }
    }

    /// Third-party instances held: seated in strips, waiting to be adopted or parked.
    #[doc(hidden)]
    pub fn hosted_instances(&self) -> usize {
        let seated = self
            .strips
            .values()
            .map(|st| st.plugins.iter().flatten().chain(st.instrument.iter()).filter(|p| is_third_party(&p.id)).count())
            .sum::<usize>();
        seated + self.adopt.len() + self.parked.len()
    }
}

/// [`render_range`] or [`render_bus`] of any length, a block at a time.
pub struct RangeRender {
    eng: Box<MixEngine>,
    /// The engine is the host's, lent (see [`LiveInstances`]): it goes back when the render is
    /// dropped or [`RangeRender::into_live`] takes it.
    live: Option<LiveInstances>,
    bus: Option<BusId>,
    tmp: Vec<Vec<f32>>,
    start: Samples,
    done: usize,
    total: usize,
    latency: usize,
    /// The block `render` last ran: its timeline position and length.
    last: (Samples, usize),
}

impl RangeRender {
    /// The timeline position and length of the block [`RangeRender::next_block`] last rendered
    /// (the mix's latency head included: these are engine blocks, not range samples).
    pub fn last_block(&self) -> (Samples, usize) {
        self.last
    }

    /// The module MIDI track `t` played in that block (see [`MixEngine::module_midi_in`]).
    pub fn module_midi_in(&self, t: TrackId) -> impl Iterator<Item = (u32, [u8; 3])> + '_ {
        self.eng.module_midi_in(t)
    }

    /// Renders the main mix over `range`, or `bus` when one is named, through instances of its
    /// own (third-party ones made from their stored state).
    pub fn new(s: &Session, bus: Option<BusId>, range: Range, block: usize) -> Self {
        let mut eng = Box::new(MixEngine::new(s.sample_rate.as_f64() as f32, block));
        eng.offline = true;
        RangeRender::over(eng, None, s, bus, range)
    }

    /// Renders through the host's live instances (see [`LiveInstances`]), at their sample rate
    /// and block size. A session at another rate is refused: the render then uses instances of
    /// its own, as [`RangeRender::new`] does, and the engine goes straight back.
    pub fn with_instances(s: &Session, bus: Option<BusId>, range: Range, mut live: LiveInstances) -> Self {
        let rate = s.sample_rate.as_f64() as f32;
        if (live.sample_rate() - rate).abs() > 0.5 {
            log::warn!("render through the live instances refused: the session runs at {rate} Hz, the instances at {} Hz", live.sample_rate());
            return RangeRender::new(s, bus, range, 1024);
        }
        match live.eng.take() {
            Some(eng) => RangeRender::over(eng, Some(live), s, bus, range),
            None => RangeRender::new(s, bus, range, 1024),
        }
    }

    fn over(mut eng: Box<MixEngine>, live: Option<LiveInstances>, s: &Session, bus: Option<BusId>, range: Range) -> Self {
        let nch = source_channels(s, bus);
        // Seat every module at the automation of the start, as a Locate does (offline bounces
        // start with a reset): no glide from the plugins' defaults on the first block.
        eng.ensure_synced(s);
        eng.reset_at(s, range.start);
        let tmp = vec![vec![0.0f32; eng.max_block()]; nch];
        let len = if nch == 0 { 0 } else { usize::try_from(range.len().max(0)).unwrap_or(0) };
        RangeRender { eng, live, bus, tmp, start: range.start, done: 0, total: len, latency: 0, last: (0, 0) }
    }

    /// Whether the render runs through the host's live instances.
    pub fn is_live(&self) -> bool {
        self.live.is_some()
    }

    /// Takes the lent instances back out (to render again with them, as a normalizing bounce's
    /// second pass does); `None` when the render has instances of its own.
    pub fn into_live(mut self) -> Option<LiveInstances> {
        let mut live = self.live.take()?;
        live.eng = Some(std::mem::replace(&mut self.eng, Box::new(MixEngine::new(48_000.0, 16))));
        Some(live)
    }

    /// The band plays into this render (an offline bounce): its update from `s` and `lib`, with
    /// each "auto" target picked from this render's own instruments. False when no track's input
    /// is a musician (nothing to adopt). The host's lent engine already has its band: nothing is
    /// adopted then.
    pub fn adopt_band_library(&mut self, s: &Session, lib: &Arc<soundcraft_band::library::LibraryData>) -> bool {
        if self.live.is_some() {
            return self.eng.has_band();
        }
        let eng = &self.eng;
        let Some(u) = prepare_band(s, lib, &|t| eng.instrument_name(t)) else { return false };
        self.eng.adopt_band(u);
        true
    }

    /// The engine has a band (see [`RangeRender::adopt_band_library`]).
    pub fn has_band(&self) -> bool {
        self.eng.has_band()
    }

    fn render(&mut self, s: &Session, pos: Samples, n: usize) {
        self.last = (pos, n);
        match self.bus {
            Some(id) => {
                self.eng.render(s, pos, n, &mut []);
                self.eng.copy_bus(id, n, &mut self.tmp);
            }
            None => self.eng.render(s, pos, n, &mut self.tmp),
        }
    }

    /// Renders one more block of the mix of `s` (the session given to [`RangeRender::new`]) and
    /// returns it as `(channels, n)`: the next `n` samples of the range lead each channel (none
    /// while the mix's latency is still being skipped). `None` once the range is done.
    pub fn next_block(&mut self, s: &Session) -> Option<(&[Vec<f32>], usize)> {
        if self.done >= self.total {
            return None;
        }
        let n = (self.total - self.done).min(self.eng.max_block());
        self.render(s, self.start + self.done as i64, n);
        if self.done == 0 {
            // The first block tells the mix's latency: render that much more and drop the head.
            self.latency = match self.bus {
                Some(b) => self.eng.bus_latency(b),
                None => self.eng.latency,
            };
            self.total = self.total.saturating_add(self.latency);
        }
        let skip = self.latency.saturating_sub(self.done).min(n);
        self.done += n;
        if skip > 0 && skip < n {
            for c in &mut self.tmp {
                c.copy_within(skip..n, 0);
            }
        }
        Some((&self.tmp, n - skip))
    }
}

impl Drop for RangeRender {
    /// A lent engine goes back to its host (see [`LiveInstances`]).
    fn drop(&mut self) {
        if let Some(mut live) = self.live.take() {
            live.eng = Some(std::mem::replace(&mut self.eng, Box::new(MixEngine::new(48_000.0, 16))));
        }
    }
}

/// ITU-R BS.775 stereo fold-down of planar audio in `fmt` (C and surrounds at -3 dB, LFE dropped;
/// Ambisonics play their W channel). Mono/stereo input is returned unchanged.
pub fn fold_down_stereo(fmt: ChannelFormat, ch: &[Vec<f32>]) -> Vec<Vec<f32>> {
    if ch.len() <= 2 {
        return ch.to_vec();
    }
    let len = ch.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = vec![vec![0.0f32; len]; 2];
    let sp = fmt.speakers();
    let h = std::f32::consts::FRAC_1_SQRT_2;
    for (c, src) in ch.iter().enumerate() {
        let (gl, gr) = match sp.get(c) {
            Some(x) if sp.len() == ch.len() => itu_stereo_fold(*x),
            _ if c == 0 => (h, h),
            _ => (0.0, 0.0),
        };
        for (g, dst) in [(gl, 0usize), (gr, 1)] {
            if g == 0.0 {
                continue;
            }
            if let Some(d) = out.get_mut(dst) {
                for (o, x) in d.iter_mut().zip(src.iter()) {
                    *o += x * g;
                }
            }
        }
    }
    out
}

/// Render only one track's clips (no mixer processing) — Consolidate.
pub fn render_clips(s: &Session, track: TrackId, range: Range) -> Vec<Vec<f32>> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let len = usize::try_from(range.len().max(0)).unwrap_or(0).min(MAX_RENDER_SAMPLES / t.channels().max(1));
    let mut out = vec![vec![0.0f32; len]; t.channels().max(1)];
    let mut scratch = Vec::new();
    for c in t.clips().iter().filter(|c| !c.muted && c.range().overlaps(&range)) {
        match clip_fx_params(s, c.id.0) {
            None => render_clip(s, c, range.start, len, &mut out, &mut scratch),
            Some(params) => {
                let mut tmp = vec![vec![0.0f32; len]; out.len()];
                render_clip(s, c, range.start, len, &mut tmp, &mut scratch);
                let mut fx = ClipFx::new(s.sample_rate.as_f64() as f32, tmp.len());
                fx.configure(params);
                let mut done = 0;
                while done < len {
                    let n = (len - done).min(4096);
                    let mut block: Vec<Vec<f32>> = tmp.iter().map(|ch| ch.get(done..done + n).map(<[f32]>::to_vec).unwrap_or_default()).collect();
                    fx.process(&mut block, n);
                    for (o, b) in out.iter_mut().zip(block.iter()) {
                        if let Some(dst) = o.get_mut(done..done + n) {
                            for (x, y) in dst.iter_mut().zip(b.iter()) {
                                *x += *y;
                            }
                        }
                    }
                    done += n;
                }
            }
        }
    }
    out
}

/// Render one track through its inserts (pre-fader) — Commit. Through instances of its own; a
/// host renders through its live ones with [`TrackRender::pre_fader`].
pub fn render_track_pre_fader(s: &Session, track: TrackId, range: Range) -> Vec<Vec<f32>> {
    TrackRender::pre_fader(s, track, range, None).run().0
}

/// The shortest and longest tail a freeze renders past the track's last clip.
pub const FREEZE_TAIL_MIN_SECS: f64 = 2.0;
pub const FREEZE_TAIL_MAX_SECS: f64 = 30.0;

/// Track › Freeze: one track's clips through its module chain (trim and phase included), pre-fader
/// (no fader, pan, mute, sends or fader-section automation), from the session start to `end` plus
/// the chain's tail (the modules' reported tails, at least [`FREEZE_TAIL_MIN_SECS`] and at most
/// [`FREEZE_TAIL_MAX_SECS`]). Planar, the track's channel count. Runs off the audio thread.
/// `upto` is the freeze point: the chain runs down to and including that module, and the modules
/// after it are left out (they stay live on the frozen track); `None` (or a module not on the
/// chain) renders the whole chain. The frozen modules' latency is removed, as the mix's is.
/// A MIDI or instrument track with an instrument renders the instrument playing its MIDI clips, and
/// its tail runs until the output dies away (see [`TrackRender::frozen`]) instead.
/// Through instances of its own; a host renders through its live ones with [`TrackRender::frozen`].
pub fn render_track_frozen(s: &Session, track: TrackId, end: Samples, upto: Option<soundcraft_model::ModuleId>) -> Vec<Vec<f32>> {
    TrackRender::frozen(s, track, end, upto, None).run().0
}

/// The longest tail an instrument track's freeze renders past its last clip (so past its last
/// note-off), and the level below which the tail counts as over: the render stops once the output
/// stays under it for [`INSTRUMENT_TAIL_HOLD_SECS`].
pub const INSTRUMENT_TAIL_MAX_SECS: f64 = 10.0;
pub const INSTRUMENT_TAIL_FLOOR_DBFS: f32 = -90.0;
pub const INSTRUMENT_TAIL_HOLD_SECS: f64 = 0.1;

/// A Freeze or Commit render of one track, a block at a time ([`TrackRender::step`]) so a host
/// can keep drawing, through its own instances or the host's live ones ([`LiveInstances`]).
pub struct TrackRender {
    /// The session with the track alone (`pre_fader_session`).
    solo: Session,
    /// The track is mono: its stereo render folds back to one channel (`fold_mono`).
    mono: bool,
    render: RangeRender,
    out: Vec<Vec<f32>>,
    /// An instrument's freeze: render to `end`, then on until the output has died away (one past
    /// the last sample above the floor is `loud`), at most to the render's total.
    tail: Option<Tail>,
    done: bool,
}

struct Tail {
    end: usize,
    hold: usize,
    floor: f32,
    loud: usize,
}

impl TrackRender {
    /// [`render_track_pre_fader`], through `live` when given.
    pub fn pre_fader(s: &Session, track: TrackId, range: Range, live: Option<LiveInstances>) -> TrackRender {
        let solo = pre_fader_session(s, track, true);
        let nch = source_channels(&solo, None);
        let render = match live {
            Some(l) => RangeRender::with_instances(&solo, None, range, l),
            None => RangeRender::new(&solo, None, range, 1024),
        };
        let mono = s.track(track).is_some_and(|t| t.channels() == 1);
        let cap = usize::try_from(range.len().max(0)).unwrap_or(0).min(MAX_RENDER_SAMPLES / nch.max(1));
        TrackRender { solo, mono, render, out: (0..nch).map(|_| Vec::with_capacity(cap)).collect(), tail: None, done: false }
    }

    /// [`render_track_frozen`], through `live` when given. The modules after the freeze point
    /// are made inactive in the render's session rather than removed, so a lent engine keeps
    /// their instances in place.
    pub fn frozen(s: &Session, track: TrackId, end: Samples, upto: Option<soundcraft_model::ModuleId>, live: Option<LiveInstances>) -> TrackRender {
        let mut solo = pre_fader_session(s, track, false);
        if let Some(t) = solo.track_mut(track)
            && let Some(at) = upto.and_then(|m| t.mixer.module_index(m))
        {
            let after: Vec<_> = t.mixer.inserts.iter().skip(at.saturating_add(1)).map(|i| i.id).collect();
            for i in t.mixer.inserts.iter_mut().skip(at.saturating_add(1)) {
                i.active = false;
            }
            t.automation.retain(|l| !matches!(&l.param, AutoParam::Plugin { module, .. } if after.contains(module)));
        }
        let mono = s.track(track).is_some_and(|t| t.channels() == 1);
        let nch = source_channels(&solo, None);
        let most = i64::try_from(MAX_RENDER_SAMPLES / nch.max(1)).unwrap_or(i64::MAX);
        let make = |solo: &Session, range: Range| match live {
            Some(l) => RangeRender::with_instances(solo, None, range, l),
            None => RangeRender::new(solo, None, range, 1024),
        };
        if let Some(t) = s.track(track).filter(|t| t.kind.is_midi() && t.instrument.is_some()) {
            // Notes delayed past the last clip's end (a MIDI Track Offset) are still inside the
            // render: the tail starts after them.
            let end = end.saturating_add(midi_lag(s, t));
            let end = usize::try_from(end.max(0).min(most)).unwrap_or(0);
            let max_tail = usize::try_from(s.sample_rate.samples(INSTRUMENT_TAIL_MAX_SECS).max(0)).unwrap_or(0);
            let total = end.saturating_add(max_tail).min(usize::try_from(most).unwrap_or(usize::MAX));
            let hold = usize::try_from(solo.sample_rate.samples(INSTRUMENT_TAIL_HOLD_SECS)).unwrap_or(0).max(1);
            let floor = 10f32.powf(INSTRUMENT_TAIL_FLOOR_DBFS / 20.0);
            let render = make(&solo, Range::new(0, i64::try_from(total).unwrap_or(i64::MAX)));
            let out = (0..nch).map(|_| Vec::with_capacity(end.saturating_add(hold))).collect();
            return TrackRender { solo, mono, render, out, tail: Some(Tail { end, hold, floor, loud: 0 }), done: false };
        }
        let (lo, hi) = (s.sample_rate.samples(FREEZE_TAIL_MIN_SECS), s.sample_rate.samples(FREEZE_TAIL_MAX_SECS));
        let mut render = make(&solo, Range::new(0, end.max(0).saturating_add(hi).min(most)));
        let tail = i64::try_from(render.eng.chain_tail(track)).unwrap_or(i64::MAX).clamp(lo, hi);
        render.total = usize::try_from(end.max(0).saturating_add(tail).min(most)).unwrap_or(0);
        let out = (0..nch).map(|_| Vec::with_capacity(render.total)).collect();
        TrackRender { solo, mono, render, out, tail: None, done: false }
    }

    /// Whether the render runs through the host's live instances.
    pub fn is_live(&self) -> bool {
        self.render.is_live()
    }

    /// How far along, from 0 to 1.
    pub fn progress(&self) -> f32 {
        if self.done {
            return 1.0;
        }
        let total = match &self.tail {
            Some(t) => t.end.max(1),
            None => self.render.total.max(1),
        };
        (self.out.first().map_or(0, Vec::len) as f64 / total as f64).min(1.0) as f32
    }

    /// Renders one more block. True once the render is complete.
    pub fn step(&mut self) -> bool {
        if self.done {
            return true;
        }
        let Some((chunk, n)) = self.render.next_block(&self.solo) else {
            self.done = true;
            return true;
        };
        let at = self.out.first().map_or(0, Vec::len);
        for (o, c) in self.out.iter_mut().zip(chunk) {
            let c = c.get(..n).unwrap_or_default();
            if let Some(t) = &mut self.tail
                && let Some(k) = c.iter().rposition(|x| x.abs() >= t.floor)
            {
                t.loud = t.loud.max(at.saturating_add(k).saturating_add(1));
            }
            o.extend_from_slice(c);
        }
        if let Some(t) = &self.tail {
            let len = at.saturating_add(n);
            if len >= t.end && len.saturating_sub(t.loud.max(t.end)) >= t.hold {
                self.done = true;
            }
        }
        if self.done {
            return true;
        }
        false
    }

    /// Renders what is left and returns the audio (planar, the track's channel count) and the
    /// lent instances, when the render had them (they go back to the host when dropped).
    pub fn run(mut self) -> (Vec<Vec<f32>>, Option<LiveInstances>) {
        while !self.step() {}
        self.finish()
    }

    /// The audio rendered so far (the whole render once [`TrackRender::step`] returned true),
    /// folded to mono for a mono track, and the lent instances when the render had them.
    pub fn finish(mut self) -> (Vec<Vec<f32>>, Option<LiveInstances>) {
        if let Some(t) = &self.tail {
            let keep = t.loud.max(t.end);
            for o in &mut self.out {
                o.truncate(keep);
            }
        }
        let out = std::mem::take(&mut self.out);
        let out = fold_mono(self.mono, out);
        let render = std::mem::replace(&mut self.render, RangeRender::new(&Session::default(), None, Range::new(0, 0), 16));
        (out, render.into_live())
    }
}

/// Mono tracks render through a stereo main mix: fold back to one channel (undoing the centre
/// pan law the render mixed with: exactly -3 dB, so not quite 1/sqrt(2)).
fn fold_mono(mono: bool, out: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    if mono && let Some(l) = out.first() {
        // The law a new engine (so every offline render) mixes with: `MixEngine::new`.
        let centre = gains(0.0, PanLaw::Minus3).0;
        let g = if centre > 0.0 { centre.recip() } else { 1.0 };
        return vec![l.iter().map(|x| x * g).collect()];
    }
    out
}

/// `s` with only `track` playing, straight into a main mix of its own width, at unity and centred,
/// without sends; `trim_automation` keeps its Trim lane (Commit), else only module automation.
fn pre_fader_session(s: &Session, track: TrackId, trim_automation: bool) -> Session {
    let mut solo = s.clone();
    // Multichannel tracks render into a main mix of their own format (straight through).
    let fmt = s.track(track).map_or(ChannelFormat::Stereo, |t| if t.channels() > 2 { t.format } else { ChannelFormat::Stereo });
    solo.outputs = vec![soundcraft_model::OutputPath { name: "Out".into(), first_channel: 0, format: fmt }];
    for t in &mut solo.tracks {
        if t.id == track {
            t.mixer.volume_db = 0.0;
            t.mixer.mute = false;
            t.mixer.solo = false;
            t.mixer.output = Route::Main;
            t.mixer.pan = match t.channels() {
                1 => vec![0.0],
                _ => vec![-1.0, 1.0],
            };
            t.automation.retain(|l| matches!(l.param, AutoParam::Plugin { .. }) || (trim_automation && l.param == AutoParam::Trim));
            t.mixer.sends.iter_mut().for_each(|x| *x = None);
            t.mixer.surround = None;
            t.mixer.vca = None;
            // The render is what the frozen track would play live.
            t.frozen = false;
            t.freeze = None;
        } else {
            t.inactive = true;
        }
    }
    solo
}

/// A built-in plugin, else a hosted CLAP plugin (`clap:<id>`).
/// A factory tests install ahead of the built-in and hosted ones ([`set_test_plugin_factory`]).
type TestFactory = fn(&str) -> Option<Box<dyn Plugin>>;
static TEST_FACTORY: std::sync::RwLock<Option<TestFactory>> = std::sync::RwLock::new(None);

/// Installs (or, with `None`, removes) a factory asked first for every plugin id, so tests in
/// other crates can stand in a "hosted" plugin (an id under a host prefix, see
/// [`is_third_party`]) without a real one installed. Not for use outside tests.
#[doc(hidden)]
pub fn set_test_plugin_factory(f: Option<TestFactory>) {
    *TEST_FACTORY.write().unwrap_or_else(std::sync::PoisonError::into_inner) = f;
}

fn create_plugin(id: &str) -> Option<Box<dyn Plugin>> {
    if let Some(f) = *TEST_FACTORY.read().unwrap_or_else(std::sync::PoisonError::into_inner)
        && let Some(p) = f(id)
    {
        return Some(p);
    }
    soundcraft_dsp::create(id)
        .or_else(|| soundcraft_clap_host::create(id))
        .or_else(|| soundcraft_vst3_host::create(id))
        .or_else(|| soundcraft_au_host::create(id))
}

#[cfg(test)]
mod audio_thread_tests {
    use super::*;

    #[test]
    fn only_a_marked_thread_is_the_audio_thread() {
        assert!(!on_audio_thread());
        let marked = std::thread::spawn(|| {
            let before = on_audio_thread();
            mark_audio_thread();
            (before, on_audio_thread())
        })
        .join()
        .unwrap();
        assert_eq!(marked, (false, true));
        assert!(!on_audio_thread(), "marking another thread leaves this one alone");
    }

    /// What `on_audio_thread` says inside each strip job, for strips run from a thread that is
    /// (or is not) the audio thread.
    fn seen_by_strips(audio: bool) -> Vec<bool> {
        std::thread::spawn(move || {
            if audio {
                mark_audio_thread();
            }
            let mut work = vec![None; 64];
            run_strips(&mut work, |w| {
                // Long enough that the pool's workers take part.
                std::thread::sleep(std::time::Duration::from_millis(1));
                *w = Some(on_audio_thread());
            });
            work.into_iter().map(|w| w.unwrap()).collect()
        })
        .join()
        .unwrap()
    }

    #[test]
    fn strips_rendered_for_the_audio_thread_count_as_the_audio_thread_and_only_then() {
        assert!(seen_by_strips(true).iter().all(|&rt| rt), "a worker rendering for the audio thread is on it");
        assert!(seen_by_strips(false).iter().all(|&rt| !rt), "the workers don't stay marked afterwards");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{Insert, SourceAudio, SourceId};
    use std::sync::Arc;

    fn synth_session(level_db: f32) -> Session {
        use soundcraft_midi::{Note, Sequence};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let mut seq = Sequence::default();
        seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 });
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 24_000, seq));
        let mut ins = Insert::new("subtractive_synth");
        ins.params.insert("level".into(), level_db);
        tr.instrument = Some(ins);
        s
    }

    #[test]
    fn stored_instrument_params_reach_the_plugin() {
        // #152: a saved instrument level of -12 dB must render quieter than -6 dB.
        // RMS over the held part of the note, past any parameter smoothing.
        let rms = |db: f32| {
            let out = render_range(&synth_session(db), Range::new(0, 12_000), 512);
            let tail = out[0].get(6_000..).unwrap_or_default();
            (tail.iter().map(|x| x * x).sum::<f32>() / tail.len().max(1) as f32).sqrt()
        };
        let (loud, quiet) = (rms(-6.0), rms(-12.0));
        assert!(loud > 0.0, "the synth should sound");
        assert!((quiet / loud - 0.5).abs() < 0.05, "-12 dB vs -6 dB should halve the level: {quiet} / {loud}");
    }

    /// A prepared `fs_dist` slot with Link on, and a track whose module 1 is it.
    fn dist_slot(lane: &str, value: f32) -> (PluginSlot, soundcraft_model::Insert, Track) {
        let mut ins = soundcraft_model::Insert::new("fs_dist");
        ins.id = soundcraft_model::ModuleId(1);
        for p in soundcraft_dsp::plugin_info("fs_dist").unwrap().params {
            ins.params.insert(p.id.to_string(), p.default);
        }
        ins.params.insert("link".into(), 1.0);
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let mut tr = s.track(t).unwrap().clone();
        let mut l = soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: soundcraft_model::ModuleId(1), param: lane.into() });
        l.set_point(0, value);
        tr.automation.push(l);
        let plugin = soundcraft_dsp::create("fs_dist").unwrap();
        let slot = PluginSlot {
            module: ins.id,
            serial: 0,
            id: "fs_dist".into(),
            ch: 2,
            state_load: 0,
            plugin,
            applied: Vec::new(),
            laned: Vec::new(),
            keys: 0,
            failed: false,
            prepared: 0,
            assets: Vec::new(),
            link: None,
            llm_off: false,
            llm_pos: LLM_SETTLED,
            llm_fresh: true,
            emits: false,
            midi: MidiOut::new(),
        };
        (slot, ins, tr)
    }

    fn run_blocks(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, tr: &Track) {
        let mut io = vec![vec![0.1f32; 64]; 2];
        for _ in 0..3 {
            apply_params(slot, ins, tr, Some(ins.id), 0);
            slot.plugin.process(&mut io, 64);
        }
    }

    #[test]
    fn a_camel_case_lane_drives_its_parameter() {
        // FireStation ids are camelCase; the lane is matched by the module's own id (the engine
        // restores ids saved lowercased when a session loads).
        let mut ins = soundcraft_model::Insert::new("fs_cleanq");
        ins.id = soundcraft_model::ModuleId(1);
        ins.params.insert("outGain".into(), 0.5);
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let mut tr = s.track(t).unwrap().clone();
        let mut l = soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: ins.id, param: "outGain".into() });
        l.set_point(0, 0.25);
        tr.automation.push(l);
        let plugin = soundcraft_dsp::create("fs_cleanq").unwrap();
        let mut slot = PluginSlot {
            module: ins.id,
            serial: 0,
            id: "fs_cleanq".into(),
            ch: 2,
            state_load: 0,
            plugin,
            applied: Vec::new(),
            laned: Vec::new(),
            keys: 0,
            failed: false,
            prepared: 0,
            assets: Vec::new(),
            link: None,
            llm_off: false,
            llm_pos: LLM_SETTLED,
            llm_fresh: true,
            emits: false,
            midi: MidiOut::new(),
        };
        apply_params(&mut slot, &ins, &tr, Some(ins.id), 0);
        assert_eq!(slot.plugin.param("outGain"), Some(0.25));
    }

    #[test]
    fn link_follows_an_automated_drive() {
        let (mut slot, mut ins, tr) = dist_slot("drive", 0.9);
        // The stored Level is what the command layer set when Link went on (at Drive 0.5).
        let start = soundcraft_dsp::plugins::fs::dist::wrapper::link_target(|id| if id == "level" { -1.0 } else { ins.params[id] });
        ins.params.insert("level".into(), start.unwrap());
        run_blocks(&mut slot, &ins, &tr);
        let mut at_09 = ins.params.clone();
        at_09.insert("drive".into(), 0.9);
        let want = soundcraft_dsp::plugins::fs::dist::wrapper::link_target(|id| at_09[id]).unwrap();
        assert_eq!(slot.plugin.param("drive"), Some(0.9));
        assert_eq!(slot.plugin.param("level"), Some(want), "Level followed the automated Drive");
    }

    #[test]
    fn lane_flags_are_retold_when_the_stored_params_change_shape() {
        // A Level lane, then the module's stored params change shape while the lane goes away:
        // the plugin must hear that Level is no longer laned, so Link follows the laned Drive
        // again when it moves.
        let (mut slot, mut ins, mut tr) = dist_slot("level", 0.2);
        let mut d = soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: soundcraft_model::ModuleId(1), param: "drive".into() });
        d.set_point(0, 0.95);
        tr.automation.push(d);
        run_blocks(&mut slot, &ins, &tr);
        assert_eq!(slot.plugin.param("level"), Some(0.2), "the lane owns Level");
        tr.automation.retain(|l| !matches!(&l.param, AutoParam::Plugin { param, .. } if param == "level"));
        ins.params.remove("trim");
        ins.params.insert("level".into(), 0.2);
        run_blocks(&mut slot, &ins, &tr);
        // The Drive lane moves.
        for l in &mut tr.automation {
            l.set_point(0, 0.6);
        }
        run_blocks(&mut slot, &ins, &tr);
        let mut at = ins.params.clone();
        at.insert("drive".into(), 0.6);
        at.insert("trim".into(), 0.3333);
        at.insert("level".into(), -1.0);
        let want = soundcraft_dsp::plugins::fs::dist::wrapper::link_target(|id| at[id]).unwrap();
        assert_eq!(slot.plugin.param("level"), Some(want), "Level is free again and follows the laned Drive");
    }

    #[test]
    fn mod_link_follows_an_automated_depth() {
        let mut ins = soundcraft_model::Insert::new("fs_mod");
        ins.id = soundcraft_model::ModuleId(1);
        for p in soundcraft_dsp::plugin_info("fs_mod").unwrap().params {
            ins.params.insert(p.id.to_string(), p.default);
        }
        for (k, v) in [("link", 1.0), ("linkLo", 0.9), ("linkHi", 0.4)] {
            ins.params.insert(k.into(), v);
        }
        let get = |p: &std::collections::BTreeMap<String, f32>, depth: f32| {
            let mut at = p.clone();
            at.insert("depth".into(), depth);
            at.insert("level".into(), -1.0);
            soundcraft_dsp::plugins::fs::mod_engine::link_target(|id| at[id]).unwrap()
        };
        ins.params.insert("level".into(), get(&ins.params, 0.5));
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let mut tr = s.track(t).unwrap().clone();
        let mut l = soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: ins.id, param: "depth".into() });
        l.set_point(0, 0.9);
        tr.automation.push(l);
        let plugin = soundcraft_dsp::create("fs_mod").unwrap();
        let mut slot = PluginSlot {
            module: ins.id,
            serial: 0,
            id: "fs_mod".into(),
            ch: 2,
            state_load: 0,
            plugin,
            applied: Vec::new(),
            laned: Vec::new(),
            keys: 0,
            failed: false,
            prepared: 0,
            assets: Vec::new(),
            link: None,
            llm_off: false,
            llm_pos: LLM_SETTLED,
            llm_fresh: true,
            emits: false,
            midi: MidiOut::new(),
        };
        run_blocks(&mut slot, &ins, &tr);
        assert_eq!(slot.plugin.param("depth"), Some(0.9));
        assert_eq!(slot.plugin.param("level"), Some(get(&ins.params, 0.9)), "Level followed the automated Depth");
        // A seek keeps it there (the laned Depth re-runs Link on the full push).
        slot.plugin.reset();
        run_blocks(&mut slot, &ins, &tr);
        assert_eq!(slot.plugin.param("level"), Some(get(&ins.params, 0.9)));
    }

    #[test]
    fn lane_flags_are_retold_when_a_key_is_swapped_at_the_same_length() {
        // Same length, different keys: the per-position flags no longer line up, so the plugin
        // hears Level is unlaned (its lane went with the swap) and Link follows Drive again.
        let (mut slot, mut ins, mut tr) = dist_slot("level", 0.2);
        let mut d = soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: soundcraft_model::ModuleId(1), param: "drive".into() });
        d.set_point(0, 0.95);
        tr.automation.push(d);
        run_blocks(&mut slot, &ins, &tr);
        assert_eq!(slot.plugin.param("level"), Some(0.2), "the lane owns Level");
        tr.automation.retain(|l| !matches!(&l.param, AutoParam::Plugin { param, .. } if param == "level"));
        let n = ins.params.len();
        ins.params.remove("bbGain"); // a key sorted before "level": Level moves down a position
        ins.params.insert("zzz".into(), 0.0); // a key the plugin does not know, after every other
        assert_eq!(ins.params.len(), n);
        run_blocks(&mut slot, &ins, &tr);
        for l in &mut tr.automation {
            l.set_point(0, 0.6);
        }
        run_blocks(&mut slot, &ins, &tr);
        let mut at = ins.params.clone();
        at.insert("drive".into(), 0.6);
        at.insert("bbGain".into(), 0.0);
        at.insert("level".into(), -1.0);
        let want = soundcraft_dsp::plugins::fs::dist::wrapper::link_target(|id| at[id]).unwrap();
        assert_eq!(slot.plugin.param("level"), Some(want), "Level is free again and follows the laned Drive");
    }

    #[test]
    fn apply_values_fills_its_bookkeeping_in_place() {
        // With the room made when the slot is built, a change of keys rebuilds the bookkeeping
        // without moving it (no allocation on the audio thread).
        let (mut slot, mut ins, tr) = dist_slot("drive", 0.5);
        let n = ins.params.len() + 4;
        slot.applied = Vec::with_capacity(n);
        slot.laned = Vec::with_capacity(n);
        run_blocks(&mut slot, &ins, &tr);
        let ptrs = (slot.applied.as_ptr(), slot.laned.as_ptr());
        ins.params.remove("trim");
        run_blocks(&mut slot, &ins, &tr);
        ins.params.insert("trim".into(), 0.5);
        ins.params.insert("zzz".into(), 0.0);
        run_blocks(&mut slot, &ins, &tr);
        assert_eq!(ptrs, (slot.applied.as_ptr(), slot.laned.as_ptr()));
    }

    #[test]
    fn a_level_lane_wins_over_link() {
        let (mut slot, ins, mut tr) = dist_slot("level", 0.2);
        let mut d = soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: soundcraft_model::ModuleId(1), param: "drive".into() });
        d.set_point(0, 0.95);
        tr.automation.push(d);
        run_blocks(&mut slot, &ins, &tr);
        assert_eq!(slot.plugin.param("drive"), Some(0.95));
        assert_eq!(slot.plugin.param("level"), Some(0.2), "the lane owns Level");
    }

    #[test]
    fn insert_slot_a_automation_does_not_drive_the_instrument() {
        let mut s = synth_session(-6.0);
        let reference = render_range(&s, Range::new(0, 12_000), 512);
        let tr = &mut s.tracks[0];
        let mut lane = soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: soundcraft_model::ModuleId(1), param: "level".into() });
        lane.set_point(0, -48.0);
        tr.automation.push(lane);
        assert_eq!(render_range(&s, Range::new(0, 12_000), 512), reference);
    }

    fn session_with_dc(level: f32, frames: usize) -> (Session, TrackId) {
        let mut s = Session::default();
        let t = add_dc_track(&mut s, level, frames);
        (s, t)
    }

    fn add_dc_track(s: &mut Session, level: f32, frames: usize) -> TrackId {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; frames]] };
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(buf)));
        let id = s.new_clip_id();
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.push(Clip::audio(id, "dc", SourceId(900), 0, 0, frames as i64));
        }
        t
    }

    #[test]
    fn mono_center_pan_is_minus_3db() {
        let (s, _) = session_with_dc(0.5, 4800);
        let out = render_range(&s, Range::new(0, 4800), 512);
        let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert!((out[0][1000] - expect).abs() < 1e-3, "{}", out[0][1000]);
        assert!((out[1][1000] - expect).abs() < 1e-3);
    }

    #[test]
    fn fader_mute_and_solo() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.track_mut(t).unwrap().mixer.volume_db = -6.0;
        let out = render_range(&s, Range::new(0, 4800), 512);
        let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
        assert!((out[0][2000] - expect).abs() < 1e-3);
        s.track_mut(t).unwrap().mixer.mute = true;
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][2000].abs() < 1e-6);
        // Soloing another track mutes this one.
        s.track_mut(t).unwrap().mixer.mute = false;
        let other = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(other).unwrap().mixer.solo = true;
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][2000].abs() < 1e-6);
    }

    /// Kick In and Kick Out → Kick Bus → Drum Bus → main; Snare → Drum Bus; Snare also sends to
    /// a Verb aux. Each source plays its own DC level.
    struct DrumKit {
        s: Session,
        kick_in: TrackId,
        kick_out: TrackId,
        snare: TrackId,
        kick_bus: TrackId,
        drum_bus: TrackId,
        verb: TrackId,
    }

    fn drum_kit() -> DrumKit {
        let mut s = Session::default();
        let dc = |s: &mut Session, name: &str, level: f32, src: u64| {
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
            let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; 4800]] };
            s.pool.insert(SourceId(src), Arc::new(SourceAudio::new(buf)));
            let id = s.new_clip_id();
            if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
                pl.clips.push(Clip::audio(id, name, SourceId(src), 0, 0, 4800));
            }
            t
        };
        let kick_in = dc(&mut s, "Kick In", 0.1, 901);
        let kick_out = dc(&mut s, "Kick Out", 0.02, 902);
        let snare = dc(&mut s, "Snare", 0.004, 903);
        let kb = s.add_bus("Kick Bus", ChannelFormat::Stereo);
        let db = s.add_bus("Drum Bus", ChannelFormat::Stereo);
        let vb = s.add_bus("Verb", ChannelFormat::Stereo);
        let kick_bus = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Kick Bus"));
        let drum_bus = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Drum Bus"));
        let verb = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
        s.track_mut(kick_bus).unwrap().mixer.input = Route::Bus(kb);
        s.track_mut(kick_bus).unwrap().mixer.output = Route::Bus(db);
        s.track_mut(drum_bus).unwrap().mixer.input = Route::Bus(db);
        s.track_mut(verb).unwrap().mixer.input = Route::Bus(vb);
        s.track_mut(kick_in).unwrap().mixer.output = Route::Bus(kb);
        s.track_mut(kick_out).unwrap().mixer.output = Route::Bus(kb);
        let sn = s.track_mut(snare).unwrap();
        sn.mixer.output = Route::Bus(db);
        let mut send = soundcraft_model::SendSlot::new(Route::Bus(vb));
        send.level_db = 0.0;
        sn.mixer.sends[0] = Some(send);
        DrumKit { s, kick_in, kick_out, snare, kick_bus, drum_bus, verb }
    }

    /// The main mix at frame 2000 (left channel).
    fn level(s: &Session) -> f32 {
        render_range(s, Range::new(0, 4800), 512)[0][2000]
    }

    /// The same session with `muted` muted and no solos.
    fn muting(k: &DrumKit, muted: &[TrackId]) -> f32 {
        let mut s = k.s.clone();
        for t in muted {
            s.track_mut(*t).unwrap().mixer.mute = true;
        }
        level(&s)
    }

    #[test]
    fn soloing_a_bus_solos_what_feeds_it_and_where_it_goes() {
        let mut k = drum_kit();
        let expect = muting(&k, &[k.snare]);
        assert!(expect > 0.05, "the kick tracks reach the main mix: {expect}");
        k.s.track_mut(k.kick_bus).unwrap().mixer.solo = true;
        let got = level(&k.s);
        assert!((got - expect).abs() < 1e-5, "Kick In, Kick Out, Kick Bus and Drum Bus play, the Snare and the Verb do not: {got} vs {expect}");
    }

    #[test]
    fn soloing_a_source_keeps_its_busses_but_not_its_siblings() {
        let mut k = drum_kit();
        let expect = muting(&k, &[k.kick_out, k.snare]);
        k.s.track_mut(k.kick_in).unwrap().mixer.solo = true;
        let got = level(&k.s);
        assert!(got > 0.05, "Kick In reaches the main mix through both busses: {got}");
        assert!((got - expect).abs() < 1e-5, "Kick Out and the Snare stay muted: {got} vs {expect}");
    }

    #[test]
    fn soloing_a_reverb_return_does_not_unmute_its_senders() {
        let mut k = drum_kit();
        k.s.track_mut(k.verb).unwrap().mixer.solo = true;
        let got = level(&k.s);
        assert!(got.abs() < 1e-6, "the Snare only sends to the Verb, so it stays muted: {got}");
        // Soloing the Snare keeps its reverb return (and the Drum Bus) audible.
        k.s.track_mut(k.verb).unwrap().mixer.solo = false;
        k.s.track_mut(k.snare).unwrap().mixer.solo = true;
        let expect = muting(&k, &[k.kick_in, k.kick_out]);
        let got = level(&k.s);
        assert!(got > 0.004, "the Snare plays through the Drum Bus and the Verb: {got}");
        assert!((got - expect).abs() < 1e-5, "{got} vs {expect}");
    }

    #[test]
    fn a_routing_cycle_under_solo_neither_hangs_nor_panics() {
        let mut k = drum_kit();
        // Drum Bus back into the Kick Bus: Kick Bus → Drum Bus → Kick Bus.
        let kb = k.s.track(k.kick_bus).unwrap().mixer.input.clone();
        k.s.track_mut(k.drum_bus).unwrap().mixer.output = kb;
        k.s.track_mut(k.kick_bus).unwrap().mixer.solo = true;
        let out = render_range(&k.s, Range::new(0, 4800), 512);
        assert!(out.iter().flatten().all(|x| x.is_finite()));
    }

    #[test]
    fn hard_pans() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
        let out = render_range(&s, Range::new(0, 4800), 256);
        assert!(out[0][100] > 0.49 && out[1][100].abs() < 1e-4);
    }

    #[test]
    fn fade_curvature_reaches_the_rendered_audio() {
        for fade_in in [true, false] {
            for curvature in [-0.5, 0.0, 0.5] {
                let (mut s, t) = session_with_dc(1.0, 8192);
                s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
                let fade = soundcraft_model::Fade { len: 8192, shape: soundcraft_model::FadeShape::SCurve, curvature };
                let c = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
                if fade_in {
                    c.fade_in = fade;
                } else {
                    c.fade_out = fade;
                }
                let out = render_range(&s, Range::new(0, 8192), 512);
                let expected = if curvature < 0.0 {
                    0.2
                } else if curvature > 0.0 {
                    0.8
                } else {
                    0.5
                };
                assert!((out[0][4096] - expected).abs() < 1e-5, "{fade_in}, {curvature}: {} vs {expected}", out[0][4096]);
                assert!(out[1][4096].abs() < 1e-6);
                assert!(out[0][if fade_in { 0 } else { 8191 }].abs() < 1e-4);
            }
        }
    }

    #[test]
    fn pan_automation_does_not_depend_on_the_block_size() {
        // Bounces render in 1024-frame blocks, playback in whatever its callback asks for (#244).
        let (mut s, t) = session_with_dc(0.5, 4800);
        {
            let l = s.track_mut(t).unwrap().lane_mut(&AutoParam::Pan(0));
            l.set_point(0, -1.0);
            l.set_point(480, 1.0);
        }
        let bounce = render_range(&s, Range::new(0, 4800), 1024);
        assert!(bounce[0][600].abs() < 1e-4 && bounce[1][600] > 0.49, "{} {}", bounce[0][600], bounce[1][600]);
        for block in [512, 100, 37] {
            assert_eq!(render_range(&s, Range::new(0, 4800), block), bounce, "block {block}");
        }
    }

    #[test]
    fn fades_and_clip_gain_apply() {
        let (mut s, t) = session_with_dc(1.0, 48_000);
        {
            let c = &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0];
            c.fade_in = soundcraft_model::Fade { len: 1000, shape: soundcraft_model::FadeShape::Linear, curvature: 0.0 };
            c.gain_db = -6.0;
        }
        let out = render_range(&s, Range::new(0, 2000), 512);
        let g = std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
        assert!(out[0][0].abs() < 1e-3);
        assert!((out[0][500] - 0.5 * g).abs() < 0.02, "{}", out[0][500]);
        assert!((out[0][1500] - g).abs() < 1e-3);
    }

    #[test]
    fn sends_feed_aux_via_bus() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        let bus = s.add_bus("Verb", ChannelFormat::Stereo);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        {
            let tr = s.track_mut(t).unwrap();
            tr.mixer.output = Route::None;
            let mut snd = soundcraft_model::SendSlot::new(Route::Bus(bus));
            snd.level_db = 0.0;
            tr.mixer.sends[0] = Some(snd);
        }
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][1000] > 0.1, "aux should carry the send: {}", out[0][1000]);
    }

    #[test]
    fn render_bus_is_the_bus_not_the_main_mix() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        let stem = s.add_bus("Stem", ChannelFormat::Stereo);
        let empty = s.add_bus("Empty", ChannelFormat::Stereo);
        s.track_mut(t).unwrap().mixer.output = Route::Bus(stem);
        let main = render_range(&s, Range::new(0, 4800), 512);
        let bus = render_bus(&s, stem, Range::new(0, 4800), 512);
        let silent = render_bus(&s, empty, Range::new(0, 4800), 512);
        assert!(main[0][1000].abs() < 1e-4, "main should not carry a track routed to the bus: {}", main[0][1000]);
        assert!(bus[0][1000].abs() > 0.1, "bus should carry the track: {}", bus[0][1000]);
        assert!(silent.iter().all(|c| c.iter().all(|x| x.abs() < 1e-6)));
    }

    #[test]
    fn volume_automation_ramps() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        {
            let tr = s.track_mut(t).unwrap();
            let l = tr.lane_mut(&AutoParam::Volume);
            l.set_point(0, 0.0);
            l.set_point(24_000, -144.0);
        }
        let out = render_range(&s, Range::new(0, 48_000), 512);
        assert!(out[0][100] > 0.3);
        assert!(out[0][30_000].abs() < 1e-4);
    }

    #[test]
    fn master_volume_breakpoint_does_not_ramp_the_previous_block() {
        for block in [1, 2, 37, 512, 1024] {
            let start = 37;
            let boundary = start + block as i64;
            let end = boundary + block as i64;
            let (mut s, t) = session_with_dc(0.5, end as usize);
            s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
            let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
            let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
            lane.set_point(start, 0.0);
            lane.set_point(boundary - 1, 0.0);
            lane.set_point(boundary, -60.0);
            lane.set_point(end - 1, -60.0);

            let mut mixer = MixEngine::new(48_000.0, block);
            let mut out = vec![vec![0.0; block]; 2];
            for (pos, expected) in [(start, 0.5), (boundary, 0.0005)] {
                mixer.render(&s, pos, block, &mut out);
                for (i, &sample) in out[0].iter().enumerate() {
                    assert!((sample - expected).abs() < 1e-6, "block {block}, position {pos}, sample {i}: {sample} vs {expected}");
                }
            }
        }
    }

    #[test]
    fn master_volume_ramp_reaches_the_last_sample_of_the_block() {
        for block in [2, 37, 512, 1024] {
            let start = 37;
            let end = start + block as i64;
            let (mut s, t) = session_with_dc(0.5, end as usize);
            s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
            let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
            let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
            lane.set_point(start, 0.0);
            lane.set_point(end - 1, -12.0);

            let out = render_range(&s, Range::new(start, end), block);
            assert_eq!(out[0].len(), block);
            assert!((out[0][0] - 0.5).abs() < 1e-6);
            let expected = 0.5 * db_to_gain(-12.0);
            assert!((out[0][block - 1] - expected).abs() < 1e-6, "block {block}: {} vs {expected}", out[0][block - 1]);
        }
    }

    #[test]
    fn inserts_process_and_survive_unknown_plugins() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.track_mut(t).unwrap().mixer.add_module(Insert::new("does-not-exist"), None).unwrap();
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][100] > 0.3);
        let id = soundcraft_dsp::plugins().iter().find(|p| p.id.contains("gain")).map(|p| p.id).unwrap_or("gain");
        let mut ins = Insert::new(id);
        ins.params.insert("gain".into(), -6.0);
        s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap();
        let _ = render_range(&s, Range::new(0, 4800), 512);
    }

    /// A built-in gain behind a third-party id, with a log of parameter sets.
    struct Stateful {
        inner: Box<dyn Plugin>,
        blob: Vec<u8>,
        sets: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Plugin for Stateful {
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
            self.sets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.inner.process(io, frames);
        }
        fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
            let blob = self.blob.clone();
            Some(Box::new(move || Some(blob.clone())))
        }
    }

    struct Clock {
        inner: Box<dyn Plugin>,
        seen: Arc<std::sync::Mutex<Vec<Transport>>>,
    }

    impl Plugin for Clock {
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
        fn set_transport(&mut self, t: &Transport) {
            self.seen.lock().unwrap().push(*t);
        }
    }

    #[test]
    fn plugins_get_the_session_transport() {
        let (mut s, t) = session_with_dc(0.5, 48_000 * 4);
        s.tempo.set_tempo(0, 90.0).unwrap();
        s.tempo.set_meter(0, 3, 4).unwrap();
        s.track_mut(t).unwrap().mixer.add_module(Insert::new("clap:test.clock"), None).unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let clock = Clock { inner: soundcraft_dsp::create("gain").unwrap(), seen: Arc::clone(&seen) };
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(vec![PreparedInstance {
            track: t,
            slot: SlotKey::Module(ModuleId(1)),
            id: "clap:test.clock".into(),
            ch: 1,
            plugin: Box::new(clock),
        }]);
        render_block(&mut eng, &s, 96_000);
        eng.monitor_only = true;
        render_block(&mut eng, &s, 96_000);
        let seen = seen.lock().unwrap();
        let t0 = seen[0];
        assert!(t0.playing && !t0.recording);
        assert_eq!((t0.tempo, t0.numerator, t0.denominator, t0.samples, t0.bar), (90.0, 3, 4, 96_000, 1));
        assert!((t0.seconds - 2.0).abs() < 1e-9 && (t0.quarters - 3.0).abs() < 1e-9 && (t0.bar_start - 3.0).abs() < 1e-9);
        assert!(!seen[1].playing);
    }

    struct Ears {
        inner: Box<dyn Plugin>,
        heard: Arc<std::sync::Mutex<Vec<(usize, [u8; 3])>>>,
    }

    impl Plugin for Ears {
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
        fn midi(&mut self, offset: usize, msg: [u8; 3]) {
            self.heard.lock().unwrap().push((offset, msg));
        }
    }

    #[test]
    fn controllers_play_at_their_offsets_and_are_chased_after_a_seek() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.ears"));
        let cc = |tick, kind, value| soundcraft_midi::CtrlEvent { tick, channel: 0, kind, value };
        let ctrls = vec![cc(4, CtrlKind::Cc(64), 127), cc(960, CtrlKind::PitchBend, -8192), cc(1920, CtrlKind::Cc(64), 0)];
        let seq = soundcraft_midi::Sequence { ctrls, ..Default::default() };
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000 * 4, seq));
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ears = Ears { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.ears".into(), ch: 2, plugin: Box::new(ears) }]);
        render_block(&mut eng, &s, 0);
        assert_eq!(std::mem::take(&mut *heard.lock().unwrap()), vec![(0, [0xB0, 121, 0]), (100, [0xB0, 64, 127])]);
        render_block(&mut eng, &s, 36_000);
        assert_eq!(std::mem::take(&mut *heard.lock().unwrap()), vec![(0, [0xB0, 121, 0]), (0, [0xB0, 64, 127]), (0, [0xE0, 0, 0])]);
        render_block(&mut eng, &s, 36_512);
        assert!(heard.lock().unwrap().is_empty());
    }

    struct Keys {
        inner: Box<dyn Plugin>,
        heard: Arc<std::sync::Mutex<Vec<(u8, u8)>>>,
    }

    impl Plugin for Keys {
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
        fn note_on(&mut self, _offset: usize, note: u8, velocity: u8) {
            self.heard.lock().unwrap().push((note, velocity));
        }
    }

    /// Records every note and MIDI call as `(offset, bytes)`; `midi_raw` calls are marked with a
    /// leading `true` (clip playback never makes them).
    struct Tape {
        inner: Box<dyn Plugin>,
        heard: Arc<std::sync::Mutex<Vec<(bool, usize, [u8; 3])>>>,
    }

    impl Plugin for Tape {
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
        fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
            self.heard.lock().unwrap().push((false, offset, [0x90, note, velocity]));
        }
        fn note_off(&mut self, offset: usize, note: u8) {
            self.heard.lock().unwrap().push((false, offset, [0x80, note, 0]));
        }
        fn midi(&mut self, offset: usize, msg: [u8; 3]) {
            self.heard.lock().unwrap().push((false, offset, msg));
        }
        fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
            self.heard.lock().unwrap().push((true, offset, msg));
        }
        fn all_notes_off(&mut self) {
            self.heard.lock().unwrap().push(ALL_OFF);
        }
    }

    /// What [`Tape`] records for an `all_notes_off` call.
    const ALL_OFF: (bool, usize, [u8; 3]) = (true, usize::MAX, [0xFF; 3]);

    #[test]
    fn channel_1_clips_play_exactly_as_before() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.tape"));
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 4, length: 16 });
        let cc = |tick, kind, value| soundcraft_midi::CtrlEvent { tick, channel: 0, kind, value };
        seq.ctrls = vec![cc(2, CtrlKind::Program, 7), cc(8, CtrlKind::Cc(1), 50), cc(12, CtrlKind::PitchBend, 0)];
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000, seq));
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let tape = Tape { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.tape".into(), ch: 2, plugin: Box::new(tape) }]);
        render_block(&mut eng, &s, 0);
        // 25 samples per tick; everything on channel 1, through note_on/note_off/midi as before.
        assert_eq!(
            *heard.lock().unwrap(),
            vec![
                (false, 0, [0xB0, 121, 0]),
                (false, 100, [0x90, 60, 100]),
                (false, 500, [0x80, 60, 0]),
                (false, 50, [0xC0, 7, 0]),
                (false, 200, [0xB0, 1, 50]),
                (false, 300, [0xE0, 0, 64]),
            ]
        );
    }

    /// A tape on a new instrument track holding `clip`, and its engine.
    fn taped_clip(clip: Clip) -> (Session, MixEngine, Arc<std::sync::Mutex<Vec<(bool, usize, [u8; 3])>>>) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.tape"));
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip);
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let tape = Tape { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.tape".into(), ch: 2, plugin: Box::new(tape) }]);
        (s, eng, heard)
    }

    /// Live MIDI for the next block: `(offset, port slot, message)`, slots 0 "Keys", 1 "DIN".
    fn play_live(eng: &mut MixEngine, thru: bool, events: &[(u32, u8, [u8; 3])]) {
        eng.live_midi.ports[0] = port_id("Keys");
        eng.live_midi.ports[1] = port_id("DIN");
        eng.live_midi.thru = thru;
        for &(offset, port, msg) in events {
            eng.live_midi.push(LiveEvent { offset, port, msg, stamp: 0 });
        }
        eng.live_midi.sort();
    }

    #[test]
    fn armed_instrument_tracks_play_live_midi_through_their_instrument() {
        let (mut s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 1_000_000, 10, soundcraft_midi::Sequence::default()));
        let t = s.tracks[0].id;
        let set = |s: &mut Session, input: soundcraft_model::MidiIn, arm: bool, mon: soundcraft_model::InputMonitor| {
            let m = &mut s.track_mut(t).unwrap().mixer;
            (m.input, m.record_arm, m.input_monitor) = (Route::Midi(input), arm, mon);
        };
        let take = |h: &Arc<std::sync::Mutex<Vec<(bool, usize, [u8; 3])>>>| std::mem::take(&mut *h.lock().unwrap());
        let keys = [(10, 0, [0x90, 60, 100]), (20, 1, [0x91, 62, 90]), (30, 0, [0xB0, 64, 127]), (40, 0, [0xE0, 0, 80])];
        let all = soundcraft_model::MidiIn::all();
        let auto = soundcraft_model::InputMonitor::Auto;

        // Not armed, monitoring Auto: unheard (after the first block's controller reset).
        set(&mut s, all.clone(), false, auto);
        render_block(&mut eng, &s, -512);
        take(&heard);
        play_live(&mut eng, true, &keys);
        render_block(&mut eng, &s, 0);
        assert!(take(&heard).is_empty());
        assert!(eng.live_midi.is_empty(), "heard once, then gone");

        // Armed (Thru on): every port and channel, raw, at their offsets; sustain and bend too.
        set(&mut s, all.clone(), true, auto);
        play_live(&mut eng, true, &keys);
        render_block(&mut eng, &s, 512);
        assert_eq!(take(&heard), keys.iter().map(|&(o, _, m)| (true, o as usize, m)).collect::<Vec<_>>());

        // One port; one channel.
        set(&mut s, soundcraft_model::MidiIn { device: Some("DIN".into()), channel: 0 }, true, auto);
        play_live(&mut eng, true, &keys);
        render_block(&mut eng, &s, 1024);
        let h = take(&heard);
        assert!(h.contains(&(true, 20, [0x91, 62, 90])) && !h.iter().any(|x| x.2 == [0x90, 60, 100]), "{h:?}");
        set(&mut s, soundcraft_model::MidiIn { device: None, channel: 1 }, true, auto);
        play_live(&mut eng, true, &keys);
        render_block(&mut eng, &s, 1536);
        let h = take(&heard);
        assert!(h.contains(&(true, 10, [0x90, 60, 100])) && !h.iter().any(|x| x.2 == [0x91, 62, 90]), "{h:?}");

        // MIDI Thru off: an armed track is silent (the notes it held are let go), an
        // input-monitored one still plays.
        set(&mut s, all.clone(), true, auto);
        play_live(&mut eng, false, &keys[..1]);
        render_block(&mut eng, &s, 2048);
        // (62 was let go when the input changed to channel 1, which took the sustain pedal.)
        assert_eq!(take(&heard), vec![(true, 0, [0x80, 60, 0]), (true, 0, [0xB0, 64, 0])]);
        set(&mut s, all.clone(), false, soundcraft_model::InputMonitor::On);
        play_live(&mut eng, false, &keys[..1]);
        render_block(&mut eng, &s, 2560);
        assert_eq!(take(&heard), vec![(true, 10, [0x90, 60, 100])]);
    }

    #[test]
    fn live_midi_plays_while_stopped_and_lets_go_when_disarmed() {
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 40, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 400 });
        let (mut s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq));
        let t = s.tracks[0].id;
        {
            let m = &mut s.track_mut(t).unwrap().mixer;
            (m.input, m.record_arm) = (Route::Midi(soundcraft_model::MidiIn::all()), true);
        }
        // Stopped (monitor only): the keyboard is heard, the clip's note is not played.
        eng.monitor_only = true;
        play_live(&mut eng, true, &[(5, 0, [0x90, 60, 100]), (6, 0, [0x93, 64, 100])]);
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 0);
        let h = std::mem::take(&mut *heard.lock().unwrap());
        assert_eq!(h, vec![(true, 5, [0x90, 60, 100]), (true, 6, [0x93, 64, 100])]);
        // Disarmed with the keys held: their note-offs, once, at the block's start.
        s.track_mut(t).unwrap().mixer.record_arm = false;
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 0);
        let h = std::mem::take(&mut *heard.lock().unwrap());
        assert_eq!(h, vec![(true, 0, [0x80, 60, 0]), (true, 0, [0x83, 64, 0])]);
        // Playing: the clip and the keyboard together.
        s.track_mut(t).unwrap().mixer.record_arm = true;
        eng.monitor_only = false;
        play_live(&mut eng, true, &[(7, 0, [0x90, 72, 100])]);
        render_block(&mut eng, &s, 0);
        let h = std::mem::take(&mut *heard.lock().unwrap());
        assert!(h.contains(&(true, 7, [0x90, 72, 100])) && h.contains(&(false, 0, [0x90, 40, 100])), "{h:?}");
    }

    /// What a [`Ring`] outputs (a DC level per `process`: the queued ones, then `then`), and how
    /// often it was told to let go.
    #[derive(Default)]
    struct RingCtl {
        levels: std::collections::VecDeque<f32>,
        then: f32,
        offs: usize,
        resets: usize,
    }

    /// A scripted instrument or insert: it writes its level over whatever it is given.
    struct Ring {
        inner: Box<dyn Plugin>,
        ctl: Arc<std::sync::Mutex<RingCtl>>,
    }

    impl Plugin for Ring {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.ctl.lock().unwrap().resets += 1;
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            let mut c = self.ctl.lock().unwrap();
            let level = c.levels.pop_front().unwrap_or(c.then);
            for ch in io.iter_mut() {
                ch.iter_mut().take(frames).for_each(|x| *x = level);
            }
        }
        fn all_notes_off(&mut self) {
            self.ctl.lock().unwrap().offs += 1;
        }
    }

    type RingHandle = Arc<std::sync::Mutex<RingCtl>>;

    /// An armed instrument track (a [`Ring`]) hearing every live input, with a [`Ring`] insert
    /// when `insert`; its engine monitors only (the transport stopped).
    fn ringing(insert: bool) -> (Session, MixEngine, TrackId, RingHandle, RingHandle) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let ins = Insert::new("clap:test.ring");
        let module = ins.id;
        {
            let tr = s.track_mut(t).unwrap();
            tr.instrument = Some(Insert::new("clap:test.ring"));
            (tr.mixer.input, tr.mixer.record_arm) = (Route::Midi(soundcraft_model::MidiIn::all()), true);
            if insert {
                tr.mixer.inserts.push(ins);
            }
        }
        let (inst, fx) = (RingHandle::default(), RingHandle::default());
        let ring = |ctl: &RingHandle| Box::new(Ring { inner: soundcraft_dsp::create("gain").unwrap(), ctl: Arc::clone(ctl) });
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        let mut adopt = vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.ring".into(), ch: 2, plugin: ring(&inst) }];
        if insert {
            adopt.push(PreparedInstance { track: t, slot: SlotKey::Module(module), id: "clap:test.ring".into(), ch: 2, plugin: ring(&fx) });
        }
        eng.adopt(adopt);
        eng.monitor_only = true;
        (s, eng, t, inst, fx)
    }

    /// One stopped block (the position never moves), all of it.
    fn stopped_block(eng: &mut MixEngine, s: &Session) -> Vec<Vec<f32>> {
        let mut out = vec![vec![0.0f32; 512]; 2];
        eng.render(s, 0, 512, &mut out);
        out
    }

    /// A key played and let go on the armed track, then the track disarmed: its release begins.
    fn let_go(eng: &mut MixEngine, s: &mut Session, t: TrackId) {
        play_live(eng, true, &[(5, 0, [0x90, 60, 100])]);
        stopped_block(eng, s);
        play_live(eng, true, &[(9, 0, [0x80, 60, 0])]);
        stopped_block(eng, s);
        s.track_mut(t).unwrap().mixer.record_arm = false;
    }

    /// A key held on the armed track as it is disarmed: let go there, its release begins.
    fn disarm_held(eng: &mut MixEngine, s: &mut Session, t: TrackId) {
        play_live(eng, true, &[(5, 0, [0x90, 60, 100])]);
        stopped_block(eng, s);
        s.track_mut(t).unwrap().mixer.record_arm = false;
    }

    fn loud(out: &[Vec<f32>]) -> f32 {
        chain_peak(out, 512)
    }

    #[test]
    fn a_release_still_sounding_when_the_track_is_disarmed_is_heard_out() {
        // The keys came up before the disarm: no note is held, but the release still sounds.
        let (mut s, mut eng, t, inst, _) = ringing(false);
        inst.lock().unwrap().then = 0.5;
        let_go(&mut eng, &mut s, t);
        for b in 0..4 {
            assert!(loud(&stopped_block(&mut eng, &s)) > 0.1, "block {b}: the release is heard, not cut");
            assert!(eng.live_tail(), "block {b}");
        }
        // It dies away: the mix stops rendering soon after.
        inst.lock().unwrap().then = 0.0;
        for _ in 0..10 {
            stopped_block(&mut eng, &s);
        }
        assert!(!eng.live_tail());
    }

    #[test]
    fn a_release_ends_when_its_track_stops_rendering_or_loses_its_instrument() {
        for how in ["inactive", "no instrument", "reset"] {
            let (mut s, mut eng, t, inst, _) = ringing(false);
            inst.lock().unwrap().then = 0.5;
            disarm_held(&mut eng, &mut s, t);
            stopped_block(&mut eng, &s);
            assert!(eng.live_tail(), "{how}: releasing");
            match how {
                "inactive" => s.track_mut(t).unwrap().inactive = true,
                "no instrument" => s.track_mut(t).unwrap().instrument = None,
                _ => eng.reset(),
            }
            // Well within the 4 s a release may run (375 blocks): the quiet mix lets go.
            for _ in 0..10 {
                stopped_block(&mut eng, &s);
            }
            assert!(!eng.live_tail(), "{how}: the release must not keep the mix rendering");
        }
    }

    #[test]
    fn a_quiet_gap_in_a_release_does_not_end_it() {
        let (mut s, mut eng, t, inst, _) = ringing(false);
        inst.lock().unwrap().then = 0.5;
        disarm_held(&mut eng, &mut s, t);
        {
            // A gated release: two silent blocks (21 ms), then it sounds again.
            let mut c = inst.lock().unwrap();
            c.levels.extend([0.5, 0.0, 0.0, 0.5, 0.5]);
            c.then = 0.0;
        }
        let blocks: Vec<f32> = (0..5).map(|_| loud(&stopped_block(&mut eng, &s))).collect();
        assert!(blocks[3] > 0.1 && blocks[4] > 0.1, "the release goes on after its gap: {blocks:?}");
        for _ in 0..10 {
            stopped_block(&mut eng, &s);
        }
        assert!(!eng.live_tail(), "silent for long enough: over");
    }

    #[test]
    fn a_release_that_outlasts_its_time_is_faded_out_not_cut() {
        let (mut s, mut eng, t, inst, _) = ringing(false);
        inst.lock().unwrap().then = 0.5;
        disarm_held(&mut eng, &mut s, t);
        let offs = inst.lock().unwrap().offs;
        let mut last = Vec::new();
        for _ in 0..400 {
            let out = stopped_block(&mut eng, &s);
            if loud(&out) == 0.0 {
                break;
            }
            last = out;
        }
        assert!(last[0][0] > 0.1, "the last block starts at the release's level: {}", last[0][0]);
        assert_eq!(last[0][511], 0.0, "and fades to silence");
        assert!(last[0][256] < last[0][0] && last[0][256] > 0.0);
        assert!(inst.lock().unwrap().offs > offs, "the instrument let go of its voices");
        for _ in 0..10 {
            stopped_block(&mut eng, &s);
        }
        assert!(!eng.live_tail());
    }

    #[test]
    fn a_tail_out_of_time_never_cuts_live_input_still_heard() {
        // Track A is disarmed with a reverb that never dies away; track B is still armed and
        // played. At A's 4 s limit nothing is faded or reset: B goes on as it was.
        let (mut s, mut eng, t, inst, fx) = ringing(true);
        inst.lock().unwrap().then = 0.0;
        fx.lock().unwrap().then = 0.3;
        let b = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        {
            let tr = s.track_mut(b).unwrap();
            tr.instrument = Some(Insert::new("clap:test.ring"));
            (tr.mixer.input, tr.mixer.record_arm) = (Route::Midi(soundcraft_model::MidiIn::all()), true);
        }
        let played = RingHandle::default();
        played.lock().unwrap().then = 0.2;
        let ring = Box::new(Ring { inner: soundcraft_dsp::create("gain").unwrap(), ctl: Arc::clone(&played) });
        eng.adopt(vec![PreparedInstance { track: b, slot: SlotKey::Instrument, id: "clap:test.ring".into(), ch: 2, plugin: ring }]);
        disarm_held(&mut eng, &mut s, t);
        let (offs, resets) = {
            let c = played.lock().unwrap();
            (c.offs, c.resets)
        };
        for n in 0..400 {
            let out = stopped_block(&mut eng, &s);
            assert!(out[0][511] > 0.1, "block {n}: B is never faded out");
        }
        let c = played.lock().unwrap();
        assert_eq!((c.offs, c.resets), (offs, resets), "B's notes and plugin are left alone");
        assert_eq!(fx.lock().unwrap().resets, 0, "nor is anything else reset under live playing");
        assert!(!eng.tail_on, "A's tail gave up at its limit");
    }

    #[test]
    fn an_insert_ringing_after_the_instrument_is_heard_out() {
        // The instrument's release is over at once; its insert (a reverb, say) rings on.
        let (mut s, mut eng, t, inst, fx) = ringing(true);
        inst.lock().unwrap().then = 0.0;
        fx.lock().unwrap().then = 0.3;
        disarm_held(&mut eng, &mut s, t);
        for b in 0..20 {
            assert!(loud(&stopped_block(&mut eng, &s)) > 0.05, "block {b}: the insert's tail is heard");
            assert!(eng.live_tail(), "block {b}: the mix keeps rendering while it rings");
        }
        fx.lock().unwrap().then = 0.0;
        for _ in 0..10 {
            stopped_block(&mut eng, &s);
        }
        assert!(!eng.live_tail(), "quiet: the mix stops rendering");

        // One that never dies away is faded out at the 4 s mark and let go.
        let (mut s, mut eng, t, _, fx) = ringing(true);
        fx.lock().unwrap().then = 0.3;
        disarm_held(&mut eng, &mut s, t);
        let resets = fx.lock().unwrap().resets;
        let mut blocks = 0;
        let mut last = Vec::new();
        while eng.live_tail() || blocks == 0 {
            last = stopped_block(&mut eng, &s);
            blocks += 1;
            assert!(blocks < 400, "bounded by the 4 s cap");
        }
        assert!((370..=380).contains(&blocks), "{blocks} blocks");
        assert!(last[0][0] > 0.05 && last[0][511] == 0.0, "faded over the last block");
        assert!(fx.lock().unwrap().resets > resets, "the insert was let go");
    }

    /// An armed instrument track whose clip holds pitch 40 over its first 48000 samples (25
    /// samples per tick), hearing every live input.
    fn armed_with_clip_note() -> (Session, MixEngine, Arc<std::sync::Mutex<Vec<(bool, usize, [u8; 3])>>>) {
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 40, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 1900 });
        let (mut s, eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq));
        let m = &mut s.tracks[0].mixer;
        (m.input, m.record_arm) = (Route::Midi(soundcraft_model::MidiIn::all()), true);
        (s, eng, heard)
    }

    fn take_heard(h: &Arc<std::sync::Mutex<Vec<(bool, usize, [u8; 3])>>>) -> Vec<(bool, usize, [u8; 3])> {
        std::mem::take(&mut *h.lock().unwrap())
    }

    #[test]
    fn a_key_held_while_stopped_keeps_sounding_until_it_comes_up() {
        let (s, mut eng, heard) = armed_with_clip_note();
        eng.monitor_only = true;
        play_live(&mut eng, true, &[(5, 0, [0x90, 60, 100])]);
        // The stopped transport renders the same position block after block: no seek.
        for _ in 0..50 {
            render_block(&mut eng, &s, 4096);
        }
        assert_eq!(take_heard(&heard), vec![(true, 5, [0x90, 60, 100])], "no all-notes-off, no note-off");
        play_live(&mut eng, true, &[(9, 0, [0x80, 60, 0])]);
        render_block(&mut eng, &s, 4096);
        assert_eq!(take_heard(&heard), vec![(true, 9, [0x80, 60, 0])]);
    }

    #[test]
    fn a_loop_wrap_closes_clip_notes_but_keeps_held_live_notes() {
        let (s, mut eng, heard) = armed_with_clip_note();
        eng.monitor_only = false;
        play_live(&mut eng, true, &[(5, 0, [0x90, 60, 100])]);
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 512);
        let h = take_heard(&heard);
        assert!(h.contains(&(false, 0, [0x90, 40, 100])) && h.contains(&(true, 5, [0x90, 60, 100])), "{h:?}");
        // The loop wraps back to 0: the clip's note is closed (and plays again), the key still
        // down keeps sounding.
        render_block(&mut eng, &s, 0);
        let h = take_heard(&heard);
        assert!(!h.contains(&ALL_OFF), "{h:?}");
        assert!(!h.iter().any(|e| e.2[0] & 0xF0 == 0x80 && e.2[1] == 60), "{h:?}");
        let off = h.iter().position(|e| *e == (false, 0, [0x80, 40, 0]));
        let on = h.iter().position(|e| *e == (false, 0, [0x90, 40, 100]));
        assert!(off.is_some() && off < on, "{h:?}");
        // With no key down, a wrap is a plain seek: all notes off, as before.
        play_live(&mut eng, true, &[(1, 0, [0x80, 60, 0])]);
        render_block(&mut eng, &s, 512);
        take_heard(&heard);
        render_block(&mut eng, &s, 0);
        assert!(take_heard(&heard).contains(&ALL_OFF));
    }

    #[test]
    fn stopping_by_itself_closes_clip_notes_once_and_keeps_held_live_notes() {
        let (s, mut eng, heard) = armed_with_clip_note();
        eng.monitor_only = false;
        play_live(&mut eng, true, &[(5, 0, [0x90, 60, 100])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        // The end of play: the next blocks render stopped at the position it reached.
        eng.monitor_only = true;
        for _ in 0..10 {
            render_block(&mut eng, &s, 512);
        }
        assert_eq!(take_heard(&heard), vec![(false, 0, [0x80, 40, 0])]);
    }

    #[test]
    fn changing_the_input_lets_go_of_the_notes_the_old_one_held() {
        let (mut s, mut eng, heard) = armed_with_clip_note();
        eng.monitor_only = true;
        s.tracks[0].mixer.input = Route::Midi(soundcraft_model::MidiIn { device: Some("Keys".into()), channel: 0 });
        play_live(&mut eng, true, &[(5, 0, [0x90, 60, 100])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        // Another port: the key held on Keys would never get its note-off through the new input.
        s.tracks[0].mixer.input = Route::Midi(soundcraft_model::MidiIn { device: Some("DIN".into()), channel: 0 });
        render_block(&mut eng, &s, 0);
        assert_eq!(take_heard(&heard), vec![(true, 0, [0x80, 60, 0])]);
        // Another channel, likewise.
        play_live(&mut eng, true, &[(5, 1, [0x90, 62, 100])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        s.tracks[0].mixer.input = Route::Midi(soundcraft_model::MidiIn { device: Some("DIN".into()), channel: 2 });
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 0);
        assert_eq!(take_heard(&heard), vec![(true, 0, [0x80, 62, 0])]);
    }

    #[test]
    fn letting_go_lifts_the_sustain_pedal_too() {
        let (mut s, mut eng, heard) = armed_with_clip_note();
        eng.monitor_only = true;
        play_live(&mut eng, true, &[(1, 0, [0x91, 60, 100]), (2, 0, [0xB1, 64, 127]), (3, 0, [0x81, 60, 0]), (4, 0, [0xB2, 66, 100])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        // Disarmed with the pedals down: the voices they sustain would ring on forever.
        s.tracks[0].mixer.record_arm = false;
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 0);
        assert_eq!(take_heard(&heard), vec![(true, 0, [0xB1, 64, 0]), (true, 0, [0xB2, 66, 0])]);
        // A pedal lifted before is not lifted again.
        s.tracks[0].mixer.record_arm = true;
        play_live(&mut eng, true, &[(1, 0, [0xB0, 64, 127]), (2, 0, [0xB0, 64, 0])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        s.tracks[0].mixer.record_arm = false;
        render_block(&mut eng, &s, 0);
        assert!(take_heard(&heard).is_empty());
    }

    #[test]
    fn a_port_that_closes_lets_go_of_its_notes() {
        let (mut s, mut eng, heard) = armed_with_clip_note();
        eng.monitor_only = true;
        // A second track listens to DIN only.
        let t2 = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        {
            let tr = s.track_mut(t2).unwrap();
            tr.instrument = Some(Insert::new("clap:test.tape2"));
            (tr.mixer.input, tr.mixer.record_arm) = (Route::Midi(soundcraft_model::MidiIn { device: Some("DIN".into()), channel: 0 }), true);
        }
        let heard2 = Arc::new(std::sync::Mutex::new(Vec::new()));
        let tape = Tape { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard2) };
        eng.adopt(vec![PreparedInstance { track: t2, slot: SlotKey::Instrument, id: "clap:test.tape2".into(), ch: 2, plugin: Box::new(tape) }]);
        play_live(&mut eng, true, &[(5, 0, [0x90, 60, 100]), (6, 1, [0x90, 50, 100])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        take_heard(&heard2);
        // Keys is unplugged with the key down: its note-off never comes.
        eng.live_midi.ports[0] = 0;
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 0);
        assert_eq!(take_heard(&heard), vec![(true, 0, [0x80, 60, 0])], "the all-inputs track lets go of Keys' note");
        assert!(take_heard(&heard2).is_empty(), "DIN's listener keeps its note");
    }

    #[test]
    fn live_midi_storage_is_never_reallocated_while_rendering() {
        let (mut s, mut eng, _) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 10, soundcraft_midi::Sequence::default()));
        s.tracks[0].mixer.input = Route::Midi(soundcraft_model::MidiIn::all());
        s.tracks[0].mixer.record_arm = true;
        let before = eng.live_midi.storage();
        for b in 0..8 {
            play_live(&mut eng, true, &[(1, 0, [0x90, 60, 100]), (2, 0, [0x80, 60, 0])]);
            render_block(&mut eng, &s, b * 512);
        }
        assert_eq!(eng.live_midi.storage(), before);
    }

    #[test]
    fn a_note_off_on_the_clip_end_plays_on_a_block_boundary() {
        // 25 samples per tick: the note runs past the clip, whose end (1024) is a block boundary.
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 4, length: 400 });
        let (s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 1024, seq));
        for b in 0..4 {
            render_block(&mut eng, &s, b * 512);
            if b == 1 {
                heard.lock().unwrap().push((false, usize::MAX, [0; 3])); // block 2 starts
            }
        }
        let h = heard.lock().unwrap().clone();
        let mark = h.iter().position(|x| x.1 == usize::MAX).unwrap();
        assert_eq!(h.iter().filter(|x| x.2 == [0x80, 60, 0]).count(), 1, "{h:?}");
        assert_eq!(h[mark + 1..].first(), Some(&(false, 0, [0x80, 60, 0])), "at the clip's end: {h:?}");
    }

    #[test]
    fn a_sounding_note_that_leaves_the_playing_sequence_gets_its_note_off() {
        // 25 samples per tick: the note sounds from 100 to 10100. After the first block the playing
        // sequence changes so that the note no longer spans the playhead (512).
        let note = |length| soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 4, length };
        let take = |len| {
            let mut seq = soundcraft_midi::Sequence::default();
            seq.notes.push(note(len));
            seq
        };
        let cleaned = |seq: soundcraft_midi::Sequence| {
            Some(soundcraft_model::MidiCleanup {
                preview: Some(Arc::new(soundcraft_model::CleanupPreview {
                    inputs: 1,
                    out: soundcraft_midi::cleanup::CleanupOutput { sequence: seq, edits: Vec::new(), report: Default::default() },
                })),
                ..soundcraft_model::MidiCleanup::default()
            })
        };
        let changes: [(&str, Box<dyn Fn(&mut Clip)>); 5] = [
            ("shortened behind the playhead", Box::new(move |c: &mut Clip| c.content = ClipContent::Midi { sequence: take(8) })),
            ("removed", Box::new(move |c: &mut Clip| c.content = ClipContent::Midi { sequence: soundcraft_midi::Sequence::default() })),
            ("cleaned shorter", Box::new(move |c: &mut Clip| c.midi_cleanup = cleaned(take(8)))),
            ("cleanup removed it", Box::new(move |c: &mut Clip| c.midi_cleanup = cleaned(soundcraft_midi::Sequence::default()))),
            ("muted", Box::new(|c: &mut Clip| c.muted = true)),
        ];
        for (what, change) in changes {
            let (mut s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, take(400)));
            render_block(&mut eng, &s, 0);
            // Unchanged, the next block sends nothing for the note.
            render_block(&mut eng, &s, 512);
            let ons = |h: &[(bool, usize, [u8; 3])]| h.iter().filter(|x| x.2[0] & 0xF0 == 0x90 || x.2[0] & 0xF0 == 0x80).copied().collect::<Vec<_>>();
            assert_eq!(ons(&heard.lock().unwrap()), vec![(false, 100, [0x90, 60, 100])], "{what}: steady");
            let t = s.tracks[0].id;
            let clip = s.track_mut(t).unwrap().playlist_mut().unwrap().clips.first_mut().unwrap();
            change(clip);
            heard.lock().unwrap().clear();
            render_block(&mut eng, &s, 1024);
            assert_eq!(ons(&heard.lock().unwrap()), vec![(false, 0, [0x80, 60, 0])], "{what}");
            heard.lock().unwrap().clear();
            render_block(&mut eng, &s, 1536);
            assert_eq!(ons(&heard.lock().unwrap()), vec![], "{what}: only once");
        }
    }

    #[test]
    fn printed_clips_play_their_channels_controllers_first() {
        let mut seq = soundcraft_midi::Sequence::default();
        // A keyswitch and a note with the controller that sets them up, all on tick 4.
        seq.notes.push(soundcraft_midi::Note { pitch: 24, velocity: 1, release_velocity: 0, channel: 0, start: 4, length: 2 });
        seq.notes.push(soundcraft_midi::Note { pitch: 48, velocity: 90, release_velocity: 64, channel: 2, start: 4, length: 16 });
        seq.ctrls = vec![soundcraft_midi::CtrlEvent { tick: 4, channel: 0, kind: CtrlKind::Cc(11), value: 70 }];
        let mut clip = Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq.clone());
        clip.midi_channels = true;
        let (s, mut eng, heard) = taped_clip(clip);
        render_block(&mut eng, &s, 0);
        // Without the chase's CC121 (on channel 1, and on each channel the printed clip uses).
        let h: Vec<_> = heard.lock().unwrap().iter().filter(|x| !(x.2[0] & 0xF0 == 0xB0 && x.2[1] == 121)).copied().collect();
        assert_eq!(
            h,
            vec![
                (true, 100, [0xB0, 11, 70]),
                (true, 100, [0x90, 24, 1]),
                (true, 150, [0x80, 24, 0]),
                (true, 100, [0x92, 48, 90]),
                (true, 500, [0x82, 48, 64])
            ]
        );
        // Unflagged, the same clip plays as every clip does: channel 1, notes first.
        let (s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq));
        render_block(&mut eng, &s, 0);
        let h: Vec<_> = heard.lock().unwrap().iter().filter(|x| x.2 != [0xB0, 121, 0]).copied().collect();
        assert_eq!(
            h,
            vec![
                (false, 100, [0x90, 24, 1]),
                (false, 150, [0x80, 24, 0]),
                (false, 100, [0x90, 48, 90]),
                (false, 500, [0x80, 48, 0]),
                (false, 100, [0xB0, 11, 70])
            ]
        );
    }

    #[test]
    fn printed_clips_chase_each_channel_they_use() {
        // A printed clip: CC11 on channel 1 (twice: the later value wins), CC1 on channel 3, a lower-manual note on
        // channel 9 (no controllers), all before the seek; an unflagged clip's CC7 still goes to channel 1 as before.
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 48, velocity: 90, release_velocity: 64, channel: 8, start: 2, length: 2 });
        seq.ctrls = vec![
            soundcraft_midi::CtrlEvent { tick: 2, channel: 0, kind: CtrlKind::Cc(11), value: 40 },
            soundcraft_midi::CtrlEvent { tick: 4, channel: 0, kind: CtrlKind::Cc(11), value: 70 },
            soundcraft_midi::CtrlEvent { tick: 8, channel: 2, kind: CtrlKind::Cc(1), value: 30 },
            soundcraft_midi::CtrlEvent { tick: 100, channel: 2, kind: CtrlKind::Cc(1), value: 99 }, // after this block
        ];
        let mut clip = Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq);
        clip.midi_channels = true;
        let (mut s, mut eng, heard) = taped_clip(clip);
        let plain = soundcraft_midi::Sequence {
            ctrls: vec![soundcraft_midi::CtrlEvent { tick: 1, channel: 5, kind: CtrlKind::Cc(7), value: 80 }],
            ..Default::default()
        };
        let t = s.tracks[0].id;
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(soundcraft_model::ClipId(2), "p", 0, 48_000, plain));
        render_block(&mut eng, &s, 512); // the first block chases from 512 (tick 20)
        let h = heard.lock().unwrap().clone();
        assert_eq!(
            h,
            vec![
                (false, 0, [0xB0, 121, 0]),
                (false, 0, [0xB0, 7, 80]),
                (true, 0, [0xB0, 121, 0]),
                (true, 0, [0xB0, 11, 70]),
                (true, 0, [0xB2, 121, 0]),
                (true, 0, [0xB2, 1, 30]),
                (true, 0, [0xB8, 121, 0]),
            ]
        );
    }

    #[test]
    fn real_time_properties_reach_the_instrument() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.keys"));
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 });
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000, seq));
        s.edit.values.insert(format!("rtp.{}.transpose", t.0), 12.0);
        s.edit.values.insert(format!("rtp.{}.velocity", t.0), -20.0);
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let keys = Keys { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.keys".into(), ch: 2, plugin: Box::new(keys) }]);
        render_block(&mut eng, &s, 0);
        assert_eq!(*heard.lock().unwrap(), vec![(72, 80)]);
    }

    fn render_block(eng: &mut MixEngine, s: &Session, pos: i64) -> f32 {
        let mut out = vec![vec![0.0f32; 512]; 2];
        eng.render(s, pos, 512, &mut out);
        out[0][500]
    }

    #[test]
    fn external_instances_are_adopted_and_retired() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let mut ins = Insert::new("clap:test.fake");
        ins.params.insert("gain".into(), -6.0);
        s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap();
        let specs = instance_specs(&s);
        assert_eq!(
            specs,
            vec![InstanceSpec { track: t, slot: SlotKey::Module(ModuleId(1)), id: "clap:test.fake".into(), ch: 1, state: None, state_load: 0 }]
        );
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        // Nothing to adopt yet: the slot passes audio through (and sync never tried to load a
        // CLAP plugin on this thread).
        let dry = render_block(&mut eng, &s, 0);
        assert!(dry > 0.3, "{dry}");
        let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut gain = soundcraft_dsp::create("gain").unwrap();
        gain.prepare(48_000.0, 512, 1);
        let fake = Stateful { inner: gain, blob: Vec::new(), sets: Arc::clone(&sets) };
        // An instance for the wrong channel count is retired, not used.
        let wrong = Stateful { inner: soundcraft_dsp::create("gain").unwrap(), blob: Vec::new(), sets: Arc::clone(&sets) };
        eng.adopt(vec![PreparedInstance {
            track: t,
            slot: SlotKey::Module(ModuleId(1)),
            id: "clap:test.fake".into(),
            ch: 2,
            plugin: Box::new(wrong),
        }]);
        assert!(render_block(&mut eng, &s, 512) > 0.3);
        assert_eq!(eng.take_retired().len(), 1);
        eng.adopt(vec![PreparedInstance {
            track: t,
            slot: SlotKey::Module(ModuleId(1)),
            id: "clap:test.fake".into(),
            ch: 1,
            plugin: Box::new(fake),
        }]);
        let mut wet = 0.0;
        for b in 2..20 {
            wet = render_block(&mut eng, &s, b * 512);
        }
        assert!((wet / dry - 0.501).abs() < 0.01, "{wet} vs {dry}");
        assert!(sets.load(std::sync::atomic::Ordering::Relaxed) > 0, "no stored state: session params are pushed");
        assert!(eng.take_retired().is_empty());
        // Removing the insert retires the instance instead of dropping it here.
        s.track_mut(t).unwrap().mixer.inserts.clear();
        render_block(&mut eng, &s, 20 * 512);
        assert_eq!(eng.take_retired().len(), 1);
        // Instances nobody claims are retired too; drain_plugins empties the engine.
        eng.adopt(vec![PreparedInstance {
            track: t,
            slot: SlotKey::Module(ModuleId(3)),
            id: "clap:x".into(),
            ch: 1,
            plugin: soundcraft_dsp::create("gain").unwrap(),
        }]);
        render_block(&mut eng, &s, 21 * 512);
        assert_eq!(eng.take_retired().len(), 1);
        assert!(eng.drain_plugins().is_empty());
    }

    struct Nan(Box<dyn Plugin>);

    impl Plugin for Nan {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            self.0.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.0.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.0.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.0.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.0.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            io.iter_mut().for_each(|c| c.iter_mut().take(frames).for_each(|x| *x = f32::NAN));
        }
        fn latency(&self) -> usize {
            self.0.latency()
        }
    }

    fn nan(track: TrackId, slot: SlotKey, ch: usize) -> Vec<PreparedInstance> {
        vec![PreparedInstance { track, slot, id: "clap:test.nan".into(), ch, plugin: Box::new(Nan(soundcraft_dsp::create("gain").unwrap())) }]
    }

    #[test]
    fn an_insert_that_puts_out_nan_is_taken_out_until_the_next_reset() {
        let (mut s, a) = session_with_dc(0.5, 48_000);
        let b = add_dc_track(&mut s, 0.5, 48_000);
        s.track_mut(a).unwrap().mixer.add_module(Insert::new("clap:test.nan"), None).unwrap();
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        let both = render_block(&mut eng, &s, 0);
        eng.adopt(nan(a, SlotKey::Module(ModuleId(1)), 1));
        let only_b = render_block(&mut eng, &s, 512);
        assert!(only_b > 0.3 && (only_b - both / 2.0).abs() < 1e-6, "{only_b} vs {both}");
        assert_eq!(eng.meters[&a].plugin_failures, PluginFailures { count: 1, last: Some(SlotKey::Module(ModuleId(1))) });
        assert_eq!(eng.meters[&b].plugin_failures, PluginFailures::default());
        assert_eq!(render_block(&mut eng, &s, 1024), both);
        eng.reset();
        assert_eq!(render_block(&mut eng, &s, 1536), only_b);
        assert_eq!(eng.meters[&a].plugin_failures.count, 2);
    }

    #[test]
    fn nan_from_an_instrument_or_a_master_insert_is_contained() {
        let mut s = synth_session(-6.0);
        let synth = s.tracks[0].id;
        s.track_mut(synth).unwrap().instrument = Some(Insert::new("clap:test.nan"));
        let b = add_dc_track(&mut s, 0.5, 48_000);
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(nan(synth, SlotKey::Instrument, 2));
        let only_b = render_block(&mut eng, &s, 0);
        assert!(only_b > 0.3, "{only_b}");
        assert_eq!(eng.meters[&synth].plugin_failures, PluginFailures { count: 1, last: Some(SlotKey::Instrument) });
        assert_eq!(eng.meters[&b].plugin_failures, PluginFailures::default());
        let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        s.track_mut(m).unwrap().mixer.add_module(Insert::new("clap:test.nan"), None).unwrap();
        eng.adopt(nan(m, SlotKey::Module(ModuleId(1)), 2));
        assert_eq!(render_block(&mut eng, &s, 512), 0.0);
        assert_eq!(eng.meters[&m].plugin_failures, PluginFailures { count: 1, last: Some(SlotKey::Module(ModuleId(1))) });
        assert_eq!(render_block(&mut eng, &s, 1024), only_b);
    }

    #[test]
    fn failed_inserts_stop_contributing_latency_until_reset() {
        for kind in [TrackKind::Audio, TrackKind::Aux, TrackKind::Folder, TrackKind::Master] {
            for block in [64, 512] {
                let mut s = Session::default();
                s.edit.delay_compensation = true;
                let source = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
                let mut impulse = vec![0.0; block * 6];
                for at in [block, block * 2, block * 4] {
                    impulse[at] = 0.25;
                }
                s.pool
                    .insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![impulse.clone(), impulse] })));
                let cid = s.new_clip_id();
                s.track_mut(source).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(
                    cid,
                    "impulses",
                    SourceId(900),
                    0,
                    0,
                    (block * 6) as i64,
                ));
                let target = if kind == TrackKind::Audio { source } else { s.add_track(kind, ChannelFormat::Stereo, None) };
                if matches!(kind, TrackKind::Aux | TrackKind::Folder) {
                    let bus = s.add_bus("Return", ChannelFormat::Stereo);
                    s.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
                    s.track_mut(target).unwrap().mixer.input = Route::Bus(bus);
                }
                let mut failed = Insert::new("clap:test.nan");
                failed.params.insert("lookahead".into(), 5.0);
                let mut healthy = Insert::new("compressor");
                healthy.params.insert("lookahead".into(), 1.0);
                healthy.params.insert("ratio".into(), 1.0);
                let tr = s.track_mut(target).unwrap();
                tr.mixer.add_module(failed, None).unwrap();
                tr.mixer.add_module(healthy, None).unwrap();
                let mut plugin = soundcraft_dsp::create("compressor").unwrap();
                plugin.prepare(48_000.0, block, 2);
                let mut eng = MixEngine::new(48_000.0, block);
                eng.set_external_instances(true);
                eng.adopt(vec![PreparedInstance {
                    track: target,
                    slot: SlotKey::Module(ModuleId(1)),
                    id: "clap:test.nan".into(),
                    ch: 2,
                    plugin: Box::new(Nan(plugin)),
                }]);
                let mut out = vec![vec![0.0; block]; 2];
                eng.render(&s, 0, block, &mut out);
                assert!(out.iter().flatten().all(|x| *x == 0.0));
                assert_eq!(eng.meters[&target].plugin_failures.count, 1);

                for b in [1, 2, 4] {
                    if b == 4 {
                        // Reset retries the failed plugin once; the next block must drop its
                        // latency again, while keeping the healthy insert's 48-sample delay.
                        eng.reset();
                        eng.render(&s, (block * 3) as i64, block, &mut out);
                        assert_eq!(eng.meters[&target].plugin_failures.count, 2);
                    }
                    eng.render(&s, (block * b) as i64, block, &mut out);
                    assert_eq!(eng.latency, 48, "{kind:?}, block {block}, pass {b}: only the healthy insert delays audio");
                    for channel in &out {
                        assert_eq!(impulse_at(channel), vec![48], "{kind:?}, block {block}, pass {b}: no compensation for the failed insert");
                        assert!((channel[48] - 0.25).abs() < 1e-5);
                    }
                }
            }
        }
    }

    #[test]
    fn a_new_instance_replaces_a_matching_one_in_its_slot() {
        // The host removed and re-added the plugin, and both session updates reached the audio
        // thread in one go: the slot still holds the old instance (same id, same channels), but
        // the host made a new one and drives its editor. The new one must win, or the editor
        // talks to an instance that was retired ("the plugin is gone").
        let (mut s, t) = session_with_dc(0.5, 48_000);
        s.track_mut(t).unwrap().mixer.add_module(Insert::new("clap:test.fake"), None).unwrap();
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let make = |blob: &[u8]| {
            let mut gain = soundcraft_dsp::create("gain").unwrap();
            gain.prepare(48_000.0, 512, 1);
            Box::new(Stateful { inner: gain, blob: blob.to_vec(), sets: Arc::clone(&sets) })
        };
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Module(ModuleId(1)), id: "clap:test.fake".into(), ch: 1, plugin: make(b"old") }]);
        render_block(&mut eng, &s, 0);
        assert!(eng.take_retired().is_empty());
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Module(ModuleId(1)), id: "clap:test.fake".into(), ch: 1, plugin: make(b"new") }]);
        render_block(&mut eng, &s, 512);
        let mut retired = eng.take_retired();
        assert_eq!(retired.len(), 1, "the old instance is retired");
        assert_eq!(retired[0].save_state(), Some(b"old".to_vec()), "the new instance stays");
    }

    #[test]
    fn stored_state_comes_first_then_session_params() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let mut ins = Insert::new("vst3:00000000000000000000000000000001");
        ins.params.insert("gain".into(), -6.0);
        ins.set_state_bytes(b"stored");
        s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap();
        assert_eq!(instance_specs(&s)[0].state.as_deref(), Some(&b"stored"[..]));
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        let sets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fake = Stateful { inner: soundcraft_dsp::create("gain").unwrap(), blob: Vec::new(), sets: Arc::clone(&sets) };
        eng.adopt(vec![PreparedInstance {
            track: t,
            slot: SlotKey::Module(ModuleId(1)),
            id: "vst3:00000000000000000000000000000001".into(),
            ch: 1,
            plugin: Box::new(fake),
        }]);
        render_block(&mut eng, &s, 0);
        assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 1, "session params apply once");
        render_block(&mut eng, &s, 512);
        assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 1);
        s.track_mut(t).unwrap().mixer.inserts[0].params.insert("gain".into(), -12.0);
        render_block(&mut eng, &s, 1024);
        assert_eq!(sets.load(std::sync::atomic::Ordering::Relaxed), 2, "later session edits apply");
    }

    // Offline, the mix creates the plugin itself: an unknown `vst3:` id is looked up in the scan
    // of the system VST3 folders.
    #[test]
    #[ignore = "loads every installed VST3 plugin (slow; some abort the test process); run with --ignored"]
    fn offline_unknown_vst3_plugins_are_simply_absent() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let mut ins = Insert::new("vst3:00000000000000000000000000000001");
        ins.params.insert("gain".into(), -6.0);
        ins.set_state_bytes(b"stored");
        s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap();
        let out = render_range(&s, Range::new(0, 1024), 512);
        assert!(out[0][100] > 0.3);
    }

    #[test]
    fn clip_effects_gain_and_trim_automation_apply() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        let cid = s.track(t).unwrap().clips()[0].id.0;
        s.edit.values.insert(format!("clip_fx.{cid}.gain"), -6.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
        assert!((out[0][2000] - expect).abs() < 0.01, "{}", out[0][2000]);
        s.edit.set_flag(&format!("clip_fx.bypass.{cid}"), true);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[0][2000] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        s.track_mut(t).unwrap().lane_mut(&AutoParam::Trim).set_point(0, -12.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][2000] < 0.2, "{}", out[0][2000]);
    }

    #[test]
    fn delay_compensation_aligns_latent_tracks() {
        let mut s = Session::default();
        let mut imp = vec![0.0f32; 48_000];
        imp[1000] = 1.0;
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![imp] })));
        let mut ids = Vec::new();
        for (pan, fx) in [(-1.0f32, true), (1.0, false)] {
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            let cid = s.new_clip_id();
            let tr = s.track_mut(t).unwrap();
            tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "i", SourceId(900), 0, 0, 48_000));
            tr.mixer.pan = vec![pan];
            if fx {
                tr.mixer.add_module(Insert::new("maximizer"), None).unwrap();
            }
            ids.push(t);
        }
        let argmax = |c: &[f32]| c.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).unwrap();
        let lat = soundcraft_dsp::create("maximizer").unwrap().latency();
        assert!(lat > 0, "test needs a latent plugin");
        let out = render_range(&s, Range::new(0, 8000), 512);
        assert_eq!(argmax(&out[0]), argmax(&out[1]), "PDC should align L and R");
        s.edit.delay_compensation = false;
        let out = render_range(&s, Range::new(0, 8000), 512);
        assert_ne!(argmax(&out[0]), argmax(&out[1]));
    }

    /// A reset (stop, locate) must not leave the previous position's audio in the alignment delay.
    #[test]
    fn reset_clears_delay_compensation_history() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.edit.click = false;
        s.track_mut(t).unwrap().mixer.pan = vec![1.0];
        let latent = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut comp = Insert::new("compressor");
        comp.params.insert("lookahead".into(), 5.0);
        s.track_mut(latent).unwrap().mixer.add_module(comp, None).unwrap();
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut out = vec![vec![0.0f32; 512]; 2];
        for b in 0..9 {
            eng.render(&s, b * 512, 512, &mut out);
        }
        assert!(eng.latency > 0 && out[1][500] > 0.3, "the fixture plays through the alignment delay");
        eng.reset();
        eng.render(&s, 1_000_000, 512, &mut out);
        assert!(out.iter().flatten().all(|x| *x == 0.0), "old aligned audio after a reset");
    }

    /// A reset (stop, locate) must clear the cached clip EQ/compressor history too.
    #[test]
    fn reset_clears_clip_effect_history() {
        // Compressor lookahead delay, then EQ high-pass filter state, each primed with DC.
        for (param, value) in [("comp.lookahead", 5.0), ("eq.hpf_on", 1.0)] {
            let (mut s, t) = session_with_dc(0.5, 9600);
            s.edit.click = false;
            let cid = s.track(t).unwrap().clips()[0].id.0;
            s.edit.values.insert(format!("clip_fx.{cid}.{param}"), value);
            let mut out = vec![vec![0.0f32; 512]; 2];
            let mut eng = MixEngine::new(48_000.0, 512);
            for b in 0..4 {
                eng.render(&s, b * 512, 512, &mut out);
            }
            eng.reset();
            eng.render(&s, 4096, 512, &mut out);
            let mut fresh = MixEngine::new(48_000.0, 512);
            let mut expect = vec![vec![0.0f32; 512]; 2];
            fresh.render(&s, 4096, 512, &mut expect);
            assert_eq!(out, expect, "{param}: clip effect state survived the reset");
        }
    }

    fn impulse_at(ch: &[f32]) -> Vec<usize> {
        ch.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4).map(|(i, _)| i).collect()
    }

    /// An aux insert's lookahead is part of the mix latency on the first block, not the block after.
    #[test]
    fn delay_compensation_aligns_an_aux_insert() {
        let mut s = Session::default();
        let n = 8192;
        let mut left = vec![0.0f32; n];
        for at in [0usize, 2048, 8000] {
            left[at] = 0.25;
        }
        s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![left.clone(), left] })));
        let bus = s.add_bus("Test Bus", ChannelFormat::Stereo);
        let source = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Source"));
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Return"));
        let cid = s.new_clip_id();
        s.track_mut(source).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "impulses", SourceId(901), 0, 0, n as i64));
        s.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        let render = |s: &Session, start: i64, end: i64| render_range(s, Range::new(start, end), 1024);
        let control = render(&s, 0, n as i64);
        assert_eq!(impulse_at(&control[0]), vec![0, 2048, 8000]);
        assert_eq!(impulse_at(&control[1]), impulse_at(&control[0]));

        let mut comp = Insert::new("compressor");
        comp.params.insert("lookahead".into(), 5.0);
        comp.params.insert("ratio".into(), 1.0);
        s.track_mut(source).unwrap().mixer.add_module(comp.clone(), None).unwrap();
        let on_source = render(&s, 0, n as i64);
        assert_eq!(impulse_at(&on_source[0]), impulse_at(&control[0]));

        s.track_mut(source).unwrap().mixer.inserts.clear();
        s.track_mut(aux).unwrap().mixer.add_module(comp, None).unwrap();
        let on_aux = render(&s, 0, n as i64);
        assert_eq!(impulse_at(&on_aux[0]), vec![0, 2048, 8000], "aux lookahead must stay inside the bounce");
        assert_eq!(impulse_at(&on_aux[1]), impulse_at(&on_aux[0]));
        assert!((on_aux[0][8000] - 0.25).abs() < 1e-3, "{}", on_aux[0][8000]);
        let partial = render(&s, 2048, 3072);
        assert_eq!(impulse_at(&partial[0]), vec![0]);

        s.track_mut(aux).unwrap().mixer.inserts[0].params.insert("lookahead".into(), 0.0);
        let zero = render(&s, 0, n as i64);
        assert_eq!(impulse_at(&zero[0]), impulse_at(&control[0]));
    }

    #[test]
    fn offline_renders_line_up_with_the_timeline_in_any_block_size() {
        let mut s = Session::default();
        let mut imp = vec![0.0f32; 48_000];
        imp[1000] = 1.0;
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![imp] })));
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let bus = s.add_bus("Stem", ChannelFormat::Stereo);
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "i", SourceId(900), 0, 0, 48_000));
        tr.mixer.add_module(Insert::new("maximizer"), None).unwrap();
        let mut send = soundcraft_model::SendSlot::new(Route::Bus(bus));
        send.level_db = 0.0;
        tr.mixer.sends[0] = Some(send);
        let latency = soundcraft_dsp::create("maximizer").unwrap().latency();
        assert!(latency > 16);
        for source in [None, Some(bus)] {
            for block in [16, 100, 512] {
                let mut got = Vec::new();
                let mut skipped = 0;
                let mut render = RangeRender::new(&s, source, Range::new(900, 2900), block);
                while let Some((chunk, n)) = render.next_block(&s) {
                    assert!(n <= block);
                    skipped += usize::from(n == 0);
                    got.extend_from_slice(&chunk[0][..n]);
                }
                assert_eq!(got.len(), 2000, "{source:?} block {block}");
                assert_eq!(skipped, latency / block, "{source:?} block {block}");
                let peak = got.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i);
                assert_eq!(peak, Some(100), "{source:?} block {block}");
            }
        }
    }

    #[test]
    fn a_plugins_bogus_latency_is_capped() {
        struct Latent(Box<dyn Plugin>);
        impl Plugin for Latent {
            fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
                self.0.info()
            }
            fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
                self.0.prepare(sr, mb, ch);
            }
            fn reset(&mut self) {
                self.0.reset();
            }
            fn set_param(&mut self, id: &str, v: f32) -> bool {
                self.0.set_param(id, v)
            }
            fn param(&self, id: &str) -> Option<f32> {
                self.0.param(id)
            }
            fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
                self.0.process(io, frames);
            }
            fn latency(&self) -> usize {
                u32::MAX as usize
            }
        }
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, None);
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        for track in [t, aux] {
            s.track_mut(track).unwrap().mixer.add_module(Insert::new("clap:test.latent"), None).unwrap();
            let mut gain = soundcraft_dsp::create("gain").unwrap();
            gain.prepare(48_000.0, 512, 1);
            eng.adopt(vec![PreparedInstance {
                track,
                slot: SlotKey::Module(ModuleId(1)),
                id: "clap:test.latent".into(),
                ch: 1,
                plugin: Box::new(Latent(gain)),
            }]);
        }
        render_block(&mut eng, &s, 0);
        assert_eq!((eng.track_latency, eng.latency), (MAX_LATENCY, 2 * MAX_LATENCY));
    }

    #[test]
    fn input_monitoring_replaces_clips() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.input = vec![vec![0.25; 512]];
        let mut out = vec![vec![0.0; 512]; 2];
        // Not monitored: the clip plays.
        eng.render(&s, 0, 512, &mut out);
        assert!((out[0][100] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        // Monitored: the input replaces it, and it still sounds when stopped.
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::On;
        eng.render(&s, 512, 512, &mut out);
        assert!((out[0][100] - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{}", out[0][100]);
        eng.monitor_only = true;
        eng.render(&s, 1024, 512, &mut out);
        assert!(out[0][100] > 0.1);
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::Auto;
        eng.render(&s, 1024, 512, &mut out);
        assert!(out[0][100].abs() < 1e-6, "stopped, unmonitored: silence");
    }

    #[test]
    fn empty_session_is_silent() {
        let s = Session::default();
        let out = render_range(&s, Range::new(0, 1000), 128);
        assert!(out.iter().all(|c| c.iter().all(|x| *x == 0.0)));
    }

    #[test]
    fn strip_workspace_keeps_its_allocation_between_blocks() {
        let mut s = Session::default();
        for _ in 0..6 {
            s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        }
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut out = vec![vec![0.0; 512]; 2];
        eng.render(&s, 0, 512, &mut out);
        let capacity = eng.work.capacity();
        assert!(capacity >= 6);
        eng.render(&s, 512, 512, &mut out);
        assert_eq!(eng.work.capacity(), capacity);
    }

    #[test]
    fn stopped_monitoring_does_not_click() {
        let mut s = Session::default();
        s.edit.click = true;
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.metronome = true;
        eng.monitor_only = true;
        let mut out = vec![vec![0.0f32; 512]; 2];
        for _ in 0..4 {
            eng.render(&s, 0, 512, &mut out);
            assert!(out.iter().all(|c| c.iter().all(|x| *x == 0.0)));
        }
    }

    /// Renders `len` frames from zero the way playback does (metronome on), in blocks of `block`.
    fn render_playback(s: &Session, len: usize, block: usize, recording: bool) -> Vec<Vec<f32>> {
        let mut eng = MixEngine::new(s.sample_rate.as_f64() as f32, block);
        eng.metronome = true;
        eng.recording = recording;
        let mut out = vec![vec![0.0f32; len]; 2];
        let mut tmp = vec![vec![0.0f32; block]; 2];
        let mut done = 0usize;
        while done < len {
            let n = (len - done).min(block);
            eng.render(s, done as i64, n, &mut tmp);
            for (o, t) in out.iter_mut().zip(tmp.iter()) {
                if let (Some(d), Some(src)) = (o.get_mut(done..done + n), t.get(..n)) {
                    d.copy_from_slice(src);
                }
            }
            done += n;
        }
        out
    }

    #[test]
    fn click_sounds_on_every_beat_only_in_playback() {
        let mut s = Session::default();
        let beat = s.sample_rate.as_f64() as usize * 60 / 120; // 120 BPM quarter notes
        let len = beat * 8;
        s.edit.click = true;
        // Offline renders (bounces, stems) never print the metronome.
        let offline = render_range(&s, Range::new(0, len as i64), 512);
        assert!(offline.iter().all(|c| c.iter().all(|x| *x == 0.0)));

        let out = render_playback(&s, len, 512, false);
        for b in 0..8usize {
            let at = b * beat;
            let on: f32 = out[0][at..at + 200].iter().map(|x| x.abs()).fold(0.0, f32::max);
            let off: f32 = out[0][at + beat / 2..at + beat / 2 + 200].iter().map(|x| x.abs()).fold(0.0, f32::max);
            assert!(on > 0.1, "beat {b} should click, peak {on}");
            assert_eq!(off, 0.0, "between beats must stay silent");
            assert_eq!(out[0][at], out[1][at], "both channels get the click");
        }
        // Blocks that start mid-beat render the same clicks as a single pass.
        let split = render_playback(&s, len, 97, false);
        assert_eq!(out, split);
    }

    #[test]
    fn click_only_during_record_follows_recording() {
        let mut s = Session::default();
        s.edit.click = true;
        s.edit.set_flag("click.only_during_record", true);
        let idle = render_playback(&s, 48_000, 512, false);
        assert!(idle.iter().all(|c| c.iter().all(|x| *x == 0.0)));
        let rec = render_playback(&s, 48_000, 512, true);
        assert!(rec.iter().any(|c| c.iter().any(|x| x.abs() > 0.1)));
    }

    #[test]
    fn a_two_bar_count_off_at_120_lasts_four_seconds_in_4_4_and_three_in_6_8() {
        let mut s = Session::default();
        s.edit.countoff_bars = 2;
        let c = CountOff::plan(&s, 0).unwrap();
        assert_eq!((c.bars, c.beats_per_bar, c.beats()), (2, 4, 8));
        assert_eq!(c.len(), 192_000, "2 bars of 4/4 at 120 BPM, 48 kHz");
        // 6/8: six eighth-note clicks a bar, as the metronome counts it; a bar is 1.5 s.
        s.tempo.set_meter(0, 6, 8).unwrap();
        let c = CountOff::plan(&s, 0).unwrap();
        assert_eq!((c.beats_per_bar, c.beats()), (6, 12));
        assert_eq!(c.beat_at(1), 12_000);
        assert_eq!(c.len(), 144_000);
        assert_eq!(c.bar_beat(0), (1, 1));
        assert_eq!(c.bar_beat(12_000 * 7 + 5), (2, 2));
        assert_eq!(c.bar_beat(c.len() + 99), (2, 6), "past the end: still the last beat");
    }

    #[test]
    fn a_count_off_takes_the_tempo_and_meter_of_the_record_point() {
        let mut s = Session::default();
        s.edit.countoff_bars = 1;
        // 120 BPM 4/4 for two bars (4 s), then 60 BPM in 3/4 from bar 3.
        let bar3 = 2 * 4 * TICKS_PER_QUARTER;
        s.tempo.set_tempo(bar3, 60.0).unwrap();
        s.tempo.set_meter(bar3, 3, 4).unwrap();
        let at = s.tempo.tick_to_samples(bar3, s.sample_rate);
        assert_eq!(at, 192_000);
        let c = CountOff::plan(&s, at).unwrap();
        assert_eq!((c.beats_per_bar, c.beat_at(1), c.len()), (3, 48_000, 144_000), "one bar of 3/4 at 60");
        // A take starting just before the change counts off at the old tempo and meter.
        let c = CountOff::plan(&s, at - 1).unwrap();
        assert_eq!((c.beats_per_bar, c.len()), (4, 96_000));
    }

    #[test]
    fn the_count_off_clicks_each_beat_accenting_each_bar_whatever_the_click_setting() {
        let mut s = Session::default();
        s.edit.click = false;
        let c = CountOff { bars: 2, beats_per_bar: 3, beat: 3000.5 };
        let len = c.len() as usize + 2000;
        let render = |block: usize| {
            let mut eng = MixEngine::new(48_000.0, block);
            eng.metronome = true;
            eng.monitor_only = true;
            let mut out = vec![vec![0.0f32; len]; 2];
            let mut tmp = vec![vec![0.0f32; block]; 2];
            let mut done = 0usize;
            while done < len {
                let n = (len - done).min(block);
                eng.countoff = Some((c, done as i64));
                eng.render(&s, 5_000, n, &mut tmp);
                for (o, t) in out.iter_mut().zip(&tmp) {
                    o[done..done + n].copy_from_slice(&t[..n]);
                }
                done += n;
            }
            out
        };
        let out = render(256);
        let peak = |a: usize, b: usize| out[0][a..b].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        for k in 0..c.beats() {
            let at = c.beat_at(k) as usize;
            assert!(peak(at, at + 200) > 0.05, "beat {k} clicks");
            assert_eq!(peak(at + 2000, at + 2500), 0.0, "silent between beats");
        }
        assert_eq!(peak(c.len() as usize, len), 0.0, "nothing after the count-off");
        // The accent is the higher click: more zero crossings in its first 400 frames.
        let crossings = |at: usize| out[0][at..at + 400].windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!(crossings(c.beat_at(3) as usize) > crossings(c.beat_at(4) as usize), "beat 1 of bar 2 is accented");
        assert!(crossings(0) > crossings(c.beat_at(1) as usize));
        // Blocks that cut a click render the same count-off.
        assert_eq!(out, render(97));
        // Without the playback engine's metronome (an offline render) it is never printed.
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.countoff = Some((c, 0));
        let mut tmp = vec![vec![0.0f32; 512]; 2];
        eng.render(&s, 0, 512, &mut tmp);
        assert!(tmp.iter().all(|ch| ch.iter().all(|x| *x == 0.0)));
    }

    /// A session with one track of `fmt` playing a distinct DC level per channel (0.1, 0.2, …).
    fn multichannel_session(fmt: ChannelFormat, main: ChannelFormat, frames: usize) -> (Session, TrackId) {
        let mut s = Session::default();
        s.outputs[0].format = main;
        let t = s.add_track(TrackKind::Audio, fmt, None);
        let chans: Vec<Vec<f32>> = (0..fmt.channels()).map(|c| (0..frames).map(|i| 0.1 * (c + 1) as f32 + (i % 7) as f32 * 1e-3).collect()).collect();
        s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: chans })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "mc", SourceId(901), 0, 0, frames as i64));
        (s, t)
    }

    #[test]
    fn surround_track_passes_through_bit_exact() {
        let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround51, 4800);
        let src = s.pool.get(SourceId(901)).unwrap().buffer().channels.clone();
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert_eq!(out.len(), 6);
        for (o, x) in out.iter().zip(src.iter()) {
            assert_eq!(o, x, "5.1 → 5.1 must be bit-exact");
        }
    }

    #[test]
    fn surround_to_stereo_folds_down_itu() {
        let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Stereo, 4800);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert_eq!(out.len(), 2);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let x = |c: usize| 0.1 * (c + 1) as f32 + (1000 % 7) as f32 * 1e-3;
        // L R C LFE Ls Rs.
        let l = x(0) + h * x(2) + h * x(4);
        let r = x(1) + h * x(2) + h * x(5);
        assert!((out[0][1000] - l).abs() < 1e-5, "{} vs {l}", out[0][1000]);
        assert!((out[1][1000] - r).abs() < 1e-5, "{} vs {r}", out[1][1000]);
    }

    #[test]
    fn mono_track_on_surround_main_pans() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.outputs[0].format = ChannelFormat::Surround51;
        // No surround pan: stereo behaviour on L/R.
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert_eq!(out.len(), 6);
        let h = 0.5 * gains(0.0, PanLaw::Minus3).0;
        assert!((out[0][100] - h).abs() < 1e-6 && (out[1][100] - h).abs() < 1e-6);
        assert!(out[2..].iter().all(|c| c[100].abs() < 1e-7));
        // Front centre: the centre speaker only.
        s.track_mut(t).unwrap().mixer.surround = Some(SurroundPan::default());
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[2][100] - 0.5).abs() < 1e-4, "{}", out[2][100]);
        assert!(out.iter().enumerate().all(|(c, x)| c == 2 || x[100].abs() < 1e-4));
        // Rear right with LFE: Rs plus the LFE send.
        s.track_mut(t).unwrap().mixer.surround = Some(SurroundPan { x: 1.0, y: -1.0, lfe_db: -6.0, ..SurroundPan::default() });
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut buf = vec![vec![0.0; 512]; 6];
        eng.render(&s, 0, 512, &mut buf);
        assert!((buf[5][100] - 0.5).abs() < 1e-4 && (buf[3][100] - 0.5 * db_to_gain(-6.0)).abs() < 1e-4);
        assert_eq!(eng.main_meter.peaks.len(), 6);
        assert_eq!(eng.meters.get(&t).map(|m| m.peaks.len()), Some(1));
        // A stereo device buffer gets the first two channels.
        let mut two = vec![vec![1.0; 512]; 2];
        eng.render(&s, 512, 512, &mut two);
        assert!(two[0][10].abs() < 1e-6);
    }

    #[test]
    fn surround_busses_and_master_fader() {
        let (mut s, t) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround51, 4800);
        let bus = s.add_bus("Stem", ChannelFormat::Surround51);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Surround51, Some("Stem"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        s.track_mut(t).unwrap().mixer.output = Route::Bus(bus);
        let m = s.add_track(TrackKind::Master, ChannelFormat::Surround51, None);
        s.track_mut(m).unwrap().mixer.volume_db = -6.0;
        let out = render_range(&s, Range::new(0, 4800), 512);
        let g = db_to_gain(-6.0);
        for (c, ch) in out.iter().enumerate() {
            let x = 0.1 * (c + 1) as f32 + (1000 % 7) as f32 * 1e-3;
            assert!((ch[1000] - x * g).abs() < 1e-5, "channel {c}: {} vs {}", ch[1000], x * g);
        }
        // A mono bus sums a stereo source like the old stereo bus did.
        let (mut s, t) = session_with_dc(0.5, 4800);
        let bus = s.add_bus("Mono", ChannelFormat::Mono);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("M"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        s.track_mut(aux).unwrap().mixer.pan = vec![-1.0];
        s.track_mut(t).unwrap().mixer.output = Route::Bus(bus);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[0][1000] - 0.5 * gains(0.0, PanLaw::Minus3).0).abs() < 1e-6, "{}", out[0][1000]);
    }

    #[test]
    fn surround_upmix_and_downmix_between_layouts() {
        // 5.1 → 7.1: shared speakers map 1:1; 5.x surrounds pan between the 7.1 sides and rears.
        let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround71, 2000);
        let out = render_range(&s, Range::new(0, 2000), 512);
        assert_eq!(out.len(), 8);
        let x = |c: usize| 0.1 * (c + 1) as f32 + (100 % 7) as f32 * 1e-3;
        for (c, ch) in out.iter().enumerate().take(4) {
            assert!((ch[100] - x(c)).abs() < 1e-6);
        }
        let e_ls = out[4][100].powi(2) + out[6][100].powi(2);
        assert!((e_ls - x(4).powi(2)).abs() < 1e-5, "Ls energy is preserved");
    }

    /// A hosted instrument stands in for a VST3/CLAP piano: a signal generator behind a
    /// third-party id, with the level as its own state (the preset) and a "preset change" the
    /// user makes in its editor (`choose`), which the session's stored params never see. It
    /// counts its `prepare` calls (a host's `initialize`) and its `process` calls.
    struct HostedKeys {
        inner: Box<dyn Plugin>,
        choose: Arc<std::sync::Mutex<Option<f32>>>,
        /// The level as the plugin has it now (what its state reader saves).
        level: Arc<std::sync::Mutex<f32>>,
        prepares: Arc<std::sync::atomic::AtomicUsize>,
        processed: Arc<std::sync::atomic::AtomicUsize>,
        /// Every `set_offline` call, and the mode the plugin processes in now.
        modes: Arc<std::sync::Mutex<Vec<bool>>>,
        offline: bool,
        /// `process` calls made in offline mode.
        processed_offline: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl HostedKeys {
        fn new(choose: &Arc<std::sync::Mutex<Option<f32>>>) -> Self {
            let inner = soundcraft_dsp::create("signal_generator").unwrap();
            let level = Arc::new(std::sync::Mutex::new(inner.param("level").unwrap_or(0.0)));
            HostedKeys {
                inner,
                choose: Arc::clone(choose),
                level,
                prepares: Default::default(),
                processed: Default::default(),
                modes: Default::default(),
                offline: false,
                processed_offline: Default::default(),
            }
        }
        fn set_level(&mut self, v: f32) {
            self.inner.set_param("level", v);
            *self.level.lock().unwrap() = v;
        }
    }

    impl Plugin for HostedKeys {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.prepares.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            if id == "level" {
                self.set_level(v);
                return true;
            }
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self.offline {
                self.processed_offline.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let chosen = self.choose.lock().unwrap().take();
            if let Some(level) = chosen {
                self.set_level(level);
            }
            self.inner.process(io, frames);
        }
        fn set_offline(&mut self, offline: bool) {
            self.modes.lock().unwrap().push(offline);
            self.offline = offline;
        }
        fn load_state(&mut self, data: &[u8]) -> bool {
            let Ok(bytes) = <[u8; 4]>::try_from(data) else { return false };
            self.set_level(f32::from_le_bytes(bytes));
            true
        }
        fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
            let level = Arc::clone(&self.level);
            Some(Box::new(move || Some(level.lock().unwrap().to_le_bytes().to_vec())))
        }
    }

    /// RMS (dBFS) of the main mix after `blocks` blocks of 512 from `eng`.
    fn settled_db(eng: &mut MixEngine, s: &Session, blocks: usize) -> f32 {
        let mut out = vec![vec![0.0f32; 512]; 2];
        for b in 0..blocks {
            eng.render(s, (b * 512) as i64, 512, &mut out);
        }
        20.0 * block_rms(&out[0]).max(1e-9).log10()
    }

    fn block_rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// A session whose instrument track plays the hosted keys, with the stored params as the
    /// scan's defaults (`mix.instrument` fills every param with its default; nothing updates
    /// them for a hosted instrument).
    fn hosted_keys_session() -> (Session, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let mut ins = Insert::new("clap:test.keys");
        for p in soundcraft_dsp::plugin_info("signal_generator").unwrap().params {
            ins.params.insert(p.id.to_string(), p.default);
        }
        s.track_mut(t).unwrap().instrument = Some(ins);
        (s, t)
    }

    /// The realtime host's engine with the hosted keys (`ch` channels wide) adopted and a preset
    /// chosen in the plugin's editor (a louder piano at -6 dB instead of the default -20 dB),
    /// playing: the main mix's level with the defaults and with the preset, dBFS.
    fn live_keys(s: &Session, t: TrackId, ch: usize) -> (Box<MixEngine>, HostedKeysHandles, (f32, f32)) {
        let choose = Arc::new(std::sync::Mutex::new(None));
        let mut keys = HostedKeys::new(&choose);
        let reader = keys.state_reader().unwrap();
        let handles = HostedKeysHandles {
            reader,
            prepares: Arc::clone(&keys.prepares),
            processed: Arc::clone(&keys.processed),
            modes: Arc::clone(&keys.modes),
            processed_offline: Arc::clone(&keys.processed_offline),
        };
        let mut live = Box::new(MixEngine::new(48_000.0, 512));
        live.set_external_instances(true);
        live.metronome = true;
        live.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.keys".into(), ch, plugin: Box::new(keys) }]);
        let before = settled_db(&mut live, s, 12);
        *choose.lock().unwrap() = Some(-6.0);
        let heard = settled_db(&mut live, s, 12);
        (live, handles, (before, heard))
    }

    struct HostedKeysHandles {
        reader: Box<dyn soundcraft_dsp::PluginStateReader>,
        prepares: Arc<std::sync::atomic::AtomicUsize>,
        processed: Arc<std::sync::atomic::AtomicUsize>,
        modes: Arc<std::sync::Mutex<Vec<bool>>>,
        processed_offline: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[test]
    fn a_hosted_instruments_preset_bounces_at_its_playback_level() {
        let (s, t) = hosted_keys_session();
        let (live, mut h, (before, heard)) = live_keys(&s, t, 2);
        // A sine's RMS is 3 dB under its level.
        assert!((before + 23.0).abs() < 0.5, "defaults first: {before}");
        assert!((heard + 9.0).abs() < 0.5, "the preset is what plays: {heard}");
        let state_before = h.reader.save_state().unwrap();
        let prepares = h.prepares.load(std::sync::atomic::Ordering::Relaxed);

        // Bounce: the host lends its engine, the render goes through the live instance (no copy
        // made from the stored state, so the stale stored params are never pushed over the
        // preset) and the engine comes back.
        let back = Arc::new(std::sync::Mutex::new(None));
        let returned = Arc::clone(&back);
        let lent = LiveInstances::new(live, Arc::new(s.clone()), move |e| *returned.lock().unwrap() = Some(e));
        assert_eq!(lent.hosted_instances(), 1);
        // The live instance is re-prepared in offline mode for the render (a disk-streaming
        // instrument driven faster than realtime waits for its data instead of dropping it).
        assert_eq!(*h.modes.lock().unwrap(), vec![true], "told it renders offline, on lend");
        assert_eq!(h.prepares.load(std::sync::atomic::Ordering::Relaxed), prepares + 1, "and prepared again so the mode takes");
        let mut render = RangeRender::with_instances(&s, None, Range::new(0, 512 * 12), lent);
        assert!(render.is_live());
        let mut last = Vec::new();
        while let Some((chunk, n)) = render.next_block(&s) {
            if n > 0 {
                last = chunk[0][..n].to_vec();
            }
        }
        let bounced = 20.0 * block_rms(&last).max(1e-9).log10();
        assert!((heard - bounced).abs() < 0.5, "the bounce must play the preset the player heard: live {heard:.1} dBFS, bounce {bounced:.1} dBFS");
        // No new instance was made: the one instance (its state reader is the same) was only
        // prepared again for the mode change, once on lend (and once more on return, below).
        assert_eq!(h.prepares.load(std::sync::atomic::Ordering::Relaxed), prepares + 1, "no new instance was made");
        assert!(h.processed.load(std::sync::atomic::Ordering::Relaxed) > 24, "the live instance rendered the bounce");
        assert!(h.processed_offline.load(std::sync::atomic::Ordering::Relaxed) >= 12, "in offline mode");
        assert!(back.lock().unwrap().is_none(), "the engine is out while the render lives");
        drop(render);
        let mut live = back.lock().unwrap().take().expect("the engine came back");
        assert_eq!(h.reader.save_state().unwrap(), state_before, "the live instance's state is as it was");
        assert!(!live.offline && live.metronome, "the host's settings are back");
        assert_eq!(*h.modes.lock().unwrap(), vec![true, false], "realtime again for the player");
        assert_eq!(h.prepares.load(std::sync::atomic::Ordering::Relaxed), prepares + 2);
        assert_eq!(live.hosted_instances(), 1);
        // And it plays the preset, as before.
        let again = settled_db(&mut live, &s, 12);
        assert!((again - heard).abs() < 0.5, "after the bounce the live engine still plays the preset: {again}");
    }

    #[test]
    fn a_fresh_instance_made_from_the_stored_state_plays_the_preset_too() {
        // Headless renders (CLI, MCP) still make a copy: `create_instance` (`load_state`, then
        // `prepare`) with the insert's stale stored params applied on top. `SlotCtx::make` seeds
        // the bookkeeping from those params, so the restored state stands.
        let (mut s, t) = hosted_keys_session();
        let (_live, mut h, (_, heard)) = live_keys(&s, t, 2);
        let state = h.reader.save_state().unwrap();
        s.track_mut(t).unwrap().instrument.as_mut().unwrap().state = Some(soundcraft_model::b64::encode(&state));
        let mut fresh = HostedKeys::new(&Arc::new(std::sync::Mutex::new(None)));
        assert!(fresh.load_state(&state));
        fresh.prepare(48_000.0, 1024, 2);
        let mut bounce = MixEngine::new(48_000.0, 1024);
        bounce.set_external_instances(true);
        bounce.metronome = true;
        bounce.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.keys".into(), ch: 2, plugin: Box::new(fresh) }]);
        bounce.ensure_synced(&s);
        bounce.reset_at(&s, 0);
        let bounced = settled_db(&mut bounce, &s, 12);
        assert!(
            (heard - bounced).abs() < 0.5,
            "a copy from the stored state must play the preset the player heard: live {heard:.1} dBFS, copy {bounced:.1} dBFS"
        );
    }

    #[test]
    fn a_freeze_up_to_a_point_through_the_live_instances_keeps_the_modules_after_it() {
        // The keys through a hosted module and a built-in gain; the freeze point is the hosted
        // module, so the gain after it is left out of the render and stays live.
        let (mut s, t) = hosted_keys_session();
        let m = s.track_mut(t).unwrap().mixer.add_module(Insert::new("clap:test.fake"), None).unwrap();
        let g = s.track_mut(t).unwrap().mixer.add_module(Insert::new("gain"), None).unwrap();
        s.track_mut(t).unwrap().mixer.module_mut(g).unwrap().params.insert("gain".into(), -12.0);
        let (mut live, h, (_, heard)) = live_keys(&s, t, 2);
        assert!((heard + 21.0).abs() < 0.5, "live, the gain after the point plays: {heard}");
        // (a gain stands in for the hosted module: a passthrough)
        let module: Box<dyn Plugin> = soundcraft_dsp::create("gain").unwrap();
        live.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Module(m), id: "clap:test.fake".into(), ch: 2, plugin: module }]);
        live.ensure_synced(&s);
        assert_eq!(live.hosted_instances(), 2);
        let prepares = h.prepares.load(std::sync::atomic::Ordering::Relaxed);
        let back = Arc::new(std::sync::Mutex::new(None));
        let returned = Arc::clone(&back);
        let lent = LiveInstances::new(live, Arc::new(s.clone()), move |e| *returned.lock().unwrap() = Some(e));
        let render = TrackRender::frozen(&s, t, 512 * 12, Some(m), Some(lent));
        assert!(render.is_live());
        let (audio, live) = render.run();
        drop(live);
        let level = 20.0 * block_rms(&audio[0][512 * 6..512 * 7]).max(1e-9).log10();
        assert!((level + 9.0).abs() < 0.5, "the frozen render stops before the -12 dB gain: {level:.1} dBFS");
        let live = back.lock().unwrap().take().expect("the engine came back");
        assert_eq!(live.hosted_instances(), 2, "both hosted instances are still the host's");
        // (prepared again twice, offline for the freeze and realtime after it: the same instance)
        assert_eq!(h.prepares.load(std::sync::atomic::Ordering::Relaxed), prepares + 2, "no instance was made for the freeze");
        assert_eq!(*h.modes.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn a_render_through_a_lent_engine_parks_what_its_session_has_no_place_for() {
        // The host's session has a hosted module on the master; the freeze of a 5.1 track
        // renders into a 5.1 main mix, so the master strip changes width: its instance is parked
        // and seated again when the engine goes back.
        let (mut s, t) = hosted_keys_session();
        s.track_mut(t).unwrap().format = ChannelFormat::Surround51;
        let aux = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
        let m = s.track_mut(aux).unwrap().mixer.add_module(Insert::new("clap:test.fake"), None).unwrap();
        let (mut live, _h, _) = live_keys(&s, t, 6);
        let module: Box<dyn Plugin> = soundcraft_dsp::create("gain").unwrap();
        live.adopt(vec![PreparedInstance { track: aux, slot: SlotKey::Module(m), id: "clap:test.fake".into(), ch: 2, plugin: module }]);
        live.ensure_synced(&s);
        assert_eq!(live.hosted_instances(), 2);
        let back = Arc::new(std::sync::Mutex::new(None));
        let returned = Arc::clone(&back);
        let lent = LiveInstances::new(live, Arc::new(s.clone()), move |e| *returned.lock().unwrap() = Some(e));
        let render = TrackRender::frozen(&s, t, 512 * 4, None, Some(lent));
        let (_audio, live) = render.run();
        drop(live);
        let live = back.lock().unwrap().take().expect("the engine came back");
        assert_eq!(live.hosted_instances(), 2, "the aux's instance was parked, not retired");
        assert!(live.instance_serial(aux, m).is_some(), "and is seated again");
    }

    #[test]
    fn a_render_at_another_sample_rate_falls_back_to_copies_and_gives_the_engine_straight_back() {
        // The player runs at 48 kHz, the session at 44.1 kHz (a device the session's rate could
        // not open): the live instances cannot render it, so the render uses instances of its
        // own and the engine goes back at once, untouched (no plugin retired, realtime again).
        let (mut s, t) = hosted_keys_session();
        let (live, h, (_, heard)) = live_keys(&s, t, 2);
        s.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
        let prepares = h.prepares.load(std::sync::atomic::Ordering::Relaxed);
        let back = Arc::new(std::sync::Mutex::new(None));
        let returned = Arc::clone(&back);
        let lent = LiveInstances::new(live, Arc::new(s.clone()), move |e| *returned.lock().unwrap() = Some(e));
        let mut render = RangeRender::with_instances(&s, None, Range::new(0, 512 * 4), lent);
        assert!(!render.is_live(), "the live instances are refused");
        assert!(render.next_block(&s).is_some(), "the render goes on with instances of its own");
        let mut live = back.lock().unwrap().take().expect("the engine came straight back");
        assert_eq!(live.hosted_instances(), 1, "with its instance");
        assert_eq!(*h.modes.lock().unwrap(), vec![true, false], "realtime again");
        assert_eq!(h.prepares.load(std::sync::atomic::Ordering::Relaxed), prepares + 2);
        assert!(!live.offline && live.metronome);
        assert!(live.take_retired().is_empty(), "nothing was retired");
        s.sample_rate = soundcraft_time::SampleRate::new(48_000).unwrap();
        let again = settled_db(&mut live, &s, 12);
        assert!((again - heard).abs() < 0.5, "the live engine still plays the preset: {again}");
    }

    #[test]
    fn send_pan_and_mute_lanes_play_while_the_track_reads() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let tr = s.track_mut(t).unwrap();
        tr.lane_mut(&AutoParam::SendPan(1)).set_point(0, -0.5);
        tr.lane_mut(&AutoParam::SendPan(1)).set_point(100, 0.5);
        tr.lane_mut(&AutoParam::SendMute(1)).set_point(0, 0.0);
        tr.lane_mut(&AutoParam::SendMute(1)).set_point(100, 1.0);
        let tr = s.track(t).unwrap();
        assert_eq!(send_pan(tr, 1, 0.0, 50), 0.0);
        assert_eq!(send_pan(tr, 1, 0.2, 100), 0.5);
        assert_eq!(send_pan(tr, 0, 0.2, 100), 0.2, "another send has no lane");
        assert!(!send_muted(tr, 1, false, 99));
        assert!(send_muted(tr, 1, false, 100));
        assert!(send_muted(tr, 1, true, 0), "the key mutes whatever the lane says");
        let mut off = tr.clone();
        off.mixer.automation_mode = soundcraft_model::AutomationMode::Off;
        assert!(!send_muted(&off, 1, false, 100));
        assert_eq!(send_pan(&off, 1, 0.2, 100), 0.2);
    }
}
