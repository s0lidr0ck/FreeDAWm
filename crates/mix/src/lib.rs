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
//! [`MixEngine::sync`] brings plugin instances in line with the session. Built-in plugins and
//! Clip Effects modules are created there, on whichever thread renders, unless the host makes
//! them on its control thread with a [`BuiltinPrep`] ([`MixEngine::set_prepared_builtins`], see
//! `prepare`): realtime playback does, so `sync` on the audio thread only seats them. Hosted
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
use std::sync::Arc;

pub mod band;
pub use band::{
    BAND_SLOTS, BandSongCache, BandUpdate, SLOT_CAPACITY, SlotBuf, auto_target, band_transport, band_transport_within, new_band_runtime,
    prepare_band, prepare_band_cached,
};
mod band_lanes;
pub use band_lanes::{band_song_end, half_ticks, lane_song, lanes_from_song};
mod delay_line;
pub mod external_midi;
mod inst_outs;
pub mod track_midi;
use delay_line::DelayLine;
mod link;
pub mod live_midi;
pub mod midi_fx;
pub mod midi_sync;
mod modulate;
mod monitor;
mod prepare;
mod rpn;
pub use live_midi::{HeldNotes, LIVE_MIDI_CAPACITY, LiveEvent, LiveMidi, MAX_MIDI_PORTS, MidiFilter, port_id};
pub use prepare::{BuiltinPrep, Prepared, PreparedClipFx, PreparedStrip, RetiredStrips, StripRoom};
pub mod scout;
pub use link::{LinkProblem, LinkRoute, describe_link, link_refusal, link_regressions, link_routes, link_routes_with, short_link_problem};
mod sidechain;
pub use sidechain::{
    INSTRUMENT_OUT, ModuleInputRoute, SidechainProblem, SidechainRoute, describe_route, module_input_refusal, module_input_routes, short_problem,
    sidechain_refusal, sidechain_regressions, sidechain_routes, sidechain_routes_with,
};
use sidechain::{KeySlot, KeySource, fill_key};

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
    /// Sum of squares of the first two channels (as `peak`) over `frames` frames: the blocks
    /// since the reader last took the peaks. Their mean is what an RMS meter averages.
    pub energy: [f32; 2],
    pub frames: u32,
    /// The main mix's loudness (master meter only, while the playback engine measures it: see
    /// [`MixEngine::set_loudness_metering`]); `None` elsewhere.
    pub loudness: Option<LoudnessReading>,
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
    /// The instrument's readouts of the last block (`Plugin::readouts`; what each means is the
    /// plugin's, e.g. Juke's pad levels); NaN where it gives none, and on non-instrument strips.
    pub instrument: [f32; 8],
    /// Feedback protection silenced this strip's monitored input (see [`FEEDBACK_SECONDS`]).
    pub feedback: bool,
    /// The loudest note-on velocity its instrument was given (clips, live input, routed MIDI
    /// tracks, the Band), 0 when none: the track's MIDI activity meter. Held like the peaks.
    pub midi_velocity: u8,
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
    /// The share of the block's own duration the module took to process it (0.02: 2 % of one
    /// processor core); 0 when it did not run, and always on the web build (no clock there).
    pub load: f32,
}

impl Default for ModuleMeter {
    fn default() -> Self {
        ModuleMeter { module: ModuleId::default(), in_peak: 0.0, out_peak: 0.0, gr_db: 0.0, readouts: [f32::NAN; 8], failed: false, load: 0.0 }
    }
}

/// The clock [`ModuleMeter::load`] is measured with (none on the web build).
#[cfg(not(target_arch = "wasm32"))]
type LoadClock = std::time::Instant;
#[cfg(target_arch = "wasm32")]
type LoadClock = ();

#[cfg(not(target_arch = "wasm32"))]
fn load_clock() -> Option<LoadClock> {
    Some(std::time::Instant::now())
}
#[cfg(target_arch = "wasm32")]
fn load_clock() -> Option<LoadClock> {
    None
}

/// [`ModuleMeter::load`] of a module that started at `t0` and processed `frames` at `rate`.
fn load_since(t0: Option<LoadClock>, frames: usize, rate: f64) -> f32 {
    #[cfg(not(target_arch = "wasm32"))]
    let spent = t0.map_or(0.0, |t| t.elapsed().as_secs_f64());
    #[cfg(target_arch = "wasm32")]
    let spent = {
        let _ = t0;
        0.0
    };
    let block = frames as f64 / rate.max(1.0);
    if block > 0.0 { (spent / block) as f32 } else { 0.0 }
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
        self.energy = o.energy;
        self.frames = o.frames;
        self.loudness = o.loudness;
        self.gain_reduction_db = o.gain_reduction_db;
        self.plugin_failures = o.plugin_failures;
        self.modules.clone_from(&o.modules);
        self.analysis.clone_from(&o.analysis);
        self.llm_bypassed = o.llm_bypassed;
        self.instrument = o.instrument;
        self.feedback = o.feedback;
        self.midi_velocity = o.midi_velocity;
    }

    /// The block's MIDI activity: with `hold` the loudest since the reader last took it.
    fn hear_midi(&mut self, velocity: u8, hold: bool) {
        self.midi_velocity = if hold { self.midi_velocity.max(velocity) } else { velocity };
    }

    /// Measures one block. With `hold`, the peaks and gain reduction are the maxima of this block
    /// and the ones before it (the earlier blocks were not taken yet: see
    /// [`MixEngine::meters_held`]); otherwise they are this block's.
    fn measure(&mut self, bufs: &[Vec<f32>], frames: usize, gr: f32, hold: bool) {
        let hold = hold && self.peaks.len() == bufs.len();
        if self.peaks.len() != bufs.len() {
            self.peaks.resize(bufs.len(), 0.0);
        }
        for (p, b) in self.peaks.iter_mut().zip(bufs.iter()) {
            let now = peak(b.get(..frames).unwrap_or(&[]));
            *p = if hold { p.max(now) } else { now };
        }
        let last = self.peaks.len().saturating_sub(1);
        self.peak = [self.peaks.first().copied().unwrap_or(0.0), self.peaks.get(1.min(last)).copied().unwrap_or(0.0)];
        self.gain_reduction_db = if hold { self.gain_reduction_db.max(gr) } else { gr };
        let n = u32::try_from(frames).unwrap_or(u32::MAX);
        if !hold {
            self.energy = [0.0; 2];
            self.frames = 0;
        }
        for (c, e) in self.energy.iter_mut().enumerate() {
            let b = bufs.get(c.min(last)).map_or(&[][..], |b| b.get(..frames).unwrap_or(b));
            let sum: f32 = b.iter().map(|x| x * x).sum();
            if sum.is_finite() {
                *e += sum;
            }
        }
        self.frames = self.frames.saturating_add(n);
    }

    /// Mean square of each of the first two channels over the measured frames (NaN when none):
    /// what an RMS meter averages.
    pub fn mean_square(&self) -> [f32; 2] {
        if self.frames == 0 {
            return [f32::NAN; 2];
        }
        let n = self.frames as f32;
        [self.energy[0] / n, self.energy[1] / n]
    }

    /// Folds `o`'s peaks and gain reduction into this one's (the maximum of each), and takes the
    /// rest of `o` as it is: a reader that polls less often than blocks are rendered still sees
    /// the loudest block in between. Allocates only when the channel count changes.
    pub fn merge_from(&mut self, o: &StripMeter) {
        if self.peaks.len() != o.peaks.len() {
            self.copy_from(o);
            return;
        }
        for (p, n) in self.peaks.iter_mut().zip(&o.peaks) {
            *p = p.max(*n);
        }
        self.peak = [self.peak[0].max(o.peak[0]), self.peak[1].max(o.peak[1])];
        self.energy = [self.energy[0] + o.energy[0], self.energy[1] + o.energy[1]];
        self.frames = self.frames.saturating_add(o.frames);
        self.loudness = o.loudness;
        self.gain_reduction_db = self.gain_reduction_db.max(o.gain_reduction_db);
        self.plugin_failures = o.plugin_failures;
        self.modules.clone_from(&o.modules);
        self.analysis.clone_from(&o.analysis);
        self.llm_bypassed = o.llm_bypassed;
        self.instrument = o.instrument;
        self.feedback = o.feedback;
        self.midi_velocity = self.midi_velocity.max(o.midi_velocity);
    }

    /// Forgets the peaks and gain reduction (after a reader took them), keeping the storage.
    pub fn clear_peaks(&mut self) {
        self.peaks.iter_mut().for_each(|p| *p = 0.0);
        self.peak = [0.0; 2];
        self.energy = [0.0; 2];
        self.frames = 0;
        self.gain_reduction_db = 0.0;
        self.midi_velocity = 0;
    }
}

/// The master's loudness as a loudness meter reads it (EBU R128 / BS.1770), LUFS and dBTP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessReading {
    /// 400 ms window.
    pub momentary: f32,
    /// 3 s window.
    pub short_term: f32,
    /// Gated, since the meter was last reset.
    pub integrated: f32,
    /// Highest momentary since the reset.
    pub max_momentary: f32,
    /// Highest true peak (4× oversampled) since the reset, dBTP.
    pub true_peak_db: f32,
    /// Loudness range (EBU Tech 3342) of the short-term values since the reset, LU.
    pub range_lu: f32,
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
    /// The plugin each of its hosted slots holds (`Plugin::set_child`: Juke's bus effects), by
    /// [`soundcraft_model::KitFx::index`].
    kit_fx: Vec<Option<String>>,
    /// Low-latency monitoring bypasses it (or is crossfading to that), see `llm_step`.
    llm_off: bool,
    /// Samples into that crossfade (negative: a resumed plugin is still priming its latency);
    /// done at the crossfade length (`LLM_SETTLED` when none ran).
    llm_pos: i64,
    /// Not run since it was made or reset: a low-latency monitoring change applies at once.
    llm_fresh: bool,
    /// The insert's input on its way past it while it is bypassed or crossfading to or from that:
    /// delayed by the plugin's latency (none on the low-latency monitoring path), so a bypass
    /// keeps the strip's delay compensation and crossfades without a comb (`insert_step`).
    dry: DelayLine,
    /// The plugin declares MIDI output (`PluginInfo::midi_out`): `midi` is filled after each
    /// `process` and read by the strips whose input is this module.
    emits: bool,
    /// The module's MIDI of the block just run (made with the slot: never grown while rendering).
    midi: MidiOut,
    /// The insert's wet/dry mix as last heard, and as wanted ([`Insert::mix`]; `wet_blend`).
    ///
    /// [`Insert::mix`]: soundcraft_model::Insert::mix
    wet: f32,
    wet_to: f32,
    /// Frames in a row both its input and its output were silent (Suspend Plug-ins on Silence).
    quiet: usize,
    /// The loudest note-on velocity handed to it since the strip last read it (the track's MIDI
    /// activity meter, `StripMeter::midi_velocity`).
    heard: u8,
}

impl PluginSlot {
    /// Notes a message on its way in for the MIDI activity meter.
    fn hear(&mut self, msg: [u8; 3]) {
        if msg[0] & 0xF0 == 0x90 {
            self.heard = self.heard.max(msg[2] & 0x7F);
        }
    }
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
    /// The track's modulators' state ([`soundcraft_model::Modulator`]).
    modrun: modulate::ModRun,
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
    /// Which MIDI tracks play this strip's instrument, as last heard: (source index, track). A
    /// change lets go of the notes only of the tracks that left; the others' held notes follow
    /// their new index. Capacity reserved in `sync`.
    routed_src: Vec<(u8, TrackId)>,
    /// The instrument's next block starts somewhere new: send it the controllers in effect there.
    chase: bool,
    /// Counts the passes the clips have played from somewhere new (each start, locate or loop
    /// turn): notes with a probability are rolled afresh each pass, the same way in every bounce.
    pass: u64,
    muted: bool,
    /// The mute's own gain (0 muted, 1 open), ramped over [`MUTE_RAMP`] samples so a mute that
    /// switches mid-play does not click; snapped to the mute when `mute_settled` is false.
    mute_gain: f32,
    mute_settled: bool,
    /// Per-sample fader and mute gains of the block (reserved in `sync`, filled by the fader).
    fader_gain: Vec<f32>,
    mute_gains: Vec<f32>,
    /// Muted, and the ramp to silence was over before this block: nothing of the strip (not
    /// even a send) is heard.
    silent: bool,
    gr: f32,
    /// Per-clip effect chains (Clip Effects) and a scratch buffer to render clips into.
    clip_fx: HashMap<u64, ClipFx>,
    clipbuf: Vec<Vec<f32>>,
    /// Total latency of the active inserts (samples).
    latency: usize,
    /// Of it, the post-fader modules' (`Insert::post_fader`): the pre-fader copy is that much
    /// earlier than the post-fader signal.
    post_lat: usize,
    /// Delay compensation: align this strip with the slowest one (post- and pre-fader paths),
    /// plus extra delay for direct-to-main outputs so they line up with aux returns.
    align: DelayLine,
    align_pre: DelayLine,
    out_extra: DelayLine,
    /// The track's own delay (`Mixer::delay`), before its inserts; sized in `sync`.
    track_delay: DelayLine,
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
    /// The instrument's extra outputs on mixer channels (`inst_outs`).
    inst_outs: inst_outs::InstOuts,
    /// The instrument's readouts of the last block it rendered (NaN without one).
    inst_readouts: [f32; 8],
    /// The loudest note-on velocity its instrument was given in the last block (0: none).
    midi_vel: u8,
    /// The MPE zone its instrument was last set up for (`rpn::mpe_setup`).
    mpe_sent: Option<soundcraft_model::MpeZone>,
    /// This strip's Scout PRE/POST taps while a capture runs (see [`scout`]).
    scout: Option<Box<scout::StripTaps>>,
    /// Where the strip's output goes, resolved in `sync` (no name lookups while rendering).
    dest: Dest,
    /// The device input channels (first, width) the strip hears when monitoring, resolved in
    /// `sync`; `None`: an unknown input (silence).
    in_span: Option<(usize, usize)>,
    /// No hardware input is chosen: track channel `k` hears
    /// [`default_input_channel`](soundcraft_model::default_input_channel), as its take records.
    in_default: bool,
    /// The strip heard live input in its last render (monitored audio input, or live MIDI into
    /// its instrument): the mix's tail must not cut what is still being played.
    hears_live: bool,
    /// Feedback protection: frames in a row the monitored input has been at or above
    /// [`FEEDBACK_LEVEL`], and whether it has been silenced for it (until it stops being heard).
    fb_hot: usize,
    fb_muted: bool,
    /// On the low-latency monitoring path this block (`llm_scope`, and the mode is on).
    llm: bool,
    /// The modules low-latency monitoring bypasses (`StripMeter::llm_bypassed`), last block.
    llm_bits: u64,
    /// Their latency (not in `latency`), last block.
    llm_latency: usize,
    /// A module's input while low-latency monitoring crossfades it (channels x `max_block`,
    /// sized in `sync` for strips with modules).
    llm_dry: Vec<Vec<f32>>,
    /// An audio track whose input is a bus: this block of that bus, at the track's width (sized
    /// in `sync`; empty otherwise). It is what the track monitors and what its take records
    /// ([`MixEngine::record_tap`]).
    bus_in: Vec<Vec<f32>>,
    /// Record with inserts (an audio track): this block after the trim, polarity and inserts,
    /// before the fader (sized in `sync`; empty otherwise). It is what the track's take records
    /// ([`MixEngine::record_tap`]).
    print: Vec<Vec<f32>>,
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

/// Options › More › Suspend Plug-ins on Silence: an insert whose input and output have both been
/// silent (below [`TAIL_SILENCE`]) for [`SUSPEND_AFTER_SECS`] is not run while its input stays
/// silent, so it costs nothing; the first sound in wakes it in the same block. Off by default
/// (a suspended module's state, a compressor's release, holds where it was).
pub const SUSPEND_ON_SILENCE: &str = "options.suspend_on_silence";
/// How long an insert's input and output are silent before it is suspended, in seconds.
pub const SUSPEND_AFTER_SECS: f64 = 1.0;

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

/// Clip Effects: an EQ and a compressor per clip, configured from [`Clip::effects`]
/// (`eq.<param>`, `comp.<param>`, `gain`).
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
            modrun: modulate::ModRun::default(),
            musician: None,
            live_held: HeldNotes::default(),
            live_filter: None,
            module_in: Vec::new(),
            midi_src: Vec::new(),
            midi_in: Box::new(StripMidi::new()),
            midi_held: Vec::with_capacity(MIDI_HELD),
            routed_src: Vec::new(),
            chase: true,
            pass: 0,
            muted: false,
            mute_gain: 1.0,
            mute_settled: false,
            fader_gain: Vec::new(),
            mute_gains: Vec::new(),
            silent: false,
            gr: 0.0,
            clip_fx: HashMap::new(),
            clipbuf: Vec::new(),
            latency: 0,
            post_lat: 0,
            align: DelayLine::default(),
            align_pre: DelayLine::default(),
            out_extra: DelayLine::default(),
            track_delay: DelayLine::default(),
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
            inst_outs: inst_outs::InstOuts::default(),
            inst_readouts: [f32::NAN; 8],
            midi_vel: 0,
            mpe_sent: None,
            scout: None,
            dest: Dest::None,
            in_span: None,
            in_default: false,
            hears_live: false,
            fb_hot: 0,
            fb_muted: false,
            llm: false,
            llm_bits: 0,
            llm_latency: 0,
            llm_dry: Vec::new(),
            bus_in: Vec::new(),
            print: Vec::new(),
            frozen_mods: 0,
            frozen_inst: false,
            live_tail: 0,
            tail_quiet: 0,
        }
    }

    /// Address and capacity of the strip's main buffers and its delay lines (to check for
    /// reallocation).
    fn buffer_ids(&self) -> Vec<(usize, usize)> {
        let lines = [&self.align, &self.align_pre, &self.out_extra, &self.track_delay].into_iter().flat_map(|l| l.buffer_ids());
        let bufs = [&self.buf, &self.pre, &self.outbuf, &self.llm_dry].into_iter().flatten().map(|b| (b.as_ptr() as usize, b.capacity()));
        let keys = self
            .keys
            .iter()
            .flat_map(|k| k.buf.iter().map(|b| (b.as_ptr() as usize, b.capacity())).chain(k.delay.buffer_ids()).chain(k.main_delay.buffer_ids()));
        bufs.chain(lines)
            .chain(keys)
            .chain([(self.fader_gain.as_ptr() as usize, self.fader_gain.capacity()), (self.midi_held.as_ptr() as usize, self.midi_held.capacity())])
            .collect()
    }

    /// The merged module MIDI this strip played in the block it last rendered.
    fn module_midi_in(&self) -> impl Iterator<Item = (u32, [u8; 3])> + '_ {
        self.midi_in.events().iter().map(|e| (e.offset, e.msg))
    }

    /// The MIDI module `m` of this strip emitted in the block it last rendered.
    fn module_midi(&self, m: ModuleId) -> Option<&MidiOut> {
        if m == sidechain::INSTRUMENT_OUT {
            // A MIDI track playing another track's instrument: what its MIDI to Track played.
            return self.instrument.as_ref().filter(|p| p.emits).map(|p| &p.midi);
        }
        self.plugins.iter().flatten().find(|p| p.module == m && p.emits).map(|p| &p.midi)
    }

    /// What this strip offers a sidechain key now.
    fn key_source(&self) -> KeySource<'_> {
        KeySource { buf: &self.buf, pre: &self.pre, post_at: self.post_at, pre_at: self.pre_at, muted: self.muted, stamp: self.stamp }
    }
}

/// `ch` empty buffers with room for `mb` samples each. (`vec![Vec::with_capacity(mb); ch]` would
/// not do: a clone of an empty Vec has no capacity, so all but the last would grow while
/// rendering.)
fn reserved(ch: usize, mb: usize) -> Vec<Vec<f32>> {
    (0..ch).map(|_| Vec::with_capacity(mb)).collect()
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
    /// The control room: listen bus, monitor path, cues and talkback (see `monitor`).
    monitor: monitor::Monitor,
    /// The largest strip compensation with low-latency monitoring off, whatever it is now
    /// (`reserve_delays` sizes the lines for it, so a toggle never grows them).
    full_latency: usize,
    /// The master faders' insert latency with low-latency monitoring off.
    full_master_latency: usize,
    /// An offline render (bounce, stem, export): low-latency monitoring never applies.
    offline: bool,
    pub meters: HashMap<TrackId, StripMeter>,
    pub main_meter: StripMeter,
    /// The master loudness meter ([`MixEngine::set_loudness_metering`]): made in `sync` for the
    /// main mix's format, fed while the transport rolls, read into `main_meter.loudness`.
    loudness: Option<soundcraft_dsp::meter::LoudnessMeter>,
    /// The loudness range of the short-term values, one every 100 ms, and the frames since the
    /// last one: made with `loudness`.
    lra: Option<(soundcraft_dsp::meter::LoudnessRange, usize)>,
    loudness_on: bool,
    /// The main format the loudness meter was made for.
    loudness_fmt: Option<(ChannelFormat, u32)>,
    /// The meters of the blocks rendered so far were not taken (the realtime side could not
    /// publish them): the next block's peaks are folded into them instead of replacing them.
    pub meters_held: bool,
    /// The session's pan law ([`session_pan_law`]), taken at the start of every block.
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
    ports_gone: u64,
    /// Snapshot key of the session the strips/order were synced to.
    synced: (usize, usize, usize),
    order: Vec<usize>,
    /// Track index → a non-aux track that renders with the auxes (keyed from one, `Plan::late`).
    late: Vec<bool>,
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
    /// Recording into this range (a punch with pre/post-roll): record-armed tracks on auto input
    /// hear their input in the blocks that touch it and play their clips before and after.
    /// `None`: they hear their input for the whole recording.
    pub punch: Option<Range>,
    /// The loop the transport is playing, set by the audio host each block: clips running across
    /// its edges ramp out before its end and in after its start, so the wrap does not click.
    /// Offline renders leave it `None`.
    pub looping: Option<Range>,
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
    /// Built-in instances and Clip Effects modules come from the host too ([`MixEngine::set_prepared_builtins`]).
    builtins: bool,
    /// Clip Effects modules handed over by the host, waiting for `sync` (by track and clip).
    adopt_clip: HashMap<(TrackId, u64), PreparedClipFx>,
    /// Built-in instances and Clip Effects modules `sync` made itself although the host makes them
    /// ([`MixEngine::made_in_sync`]).
    made_in_sync: u64,
    /// New tracks' strips and new busses' buffers handed over by the host, waiting for `sync`.
    adopt_strips: Vec<PreparedStrip>,
    adopt_busses: Vec<(BusId, Vec<Vec<f32>>)>,
    /// Strips `sync` made itself although the host makes them ([`MixEngine::strips_made_in_sync`]).
    strips_made_in_sync: u64,
    /// Strips, tables and buffers taken out of service ([`MixEngine::take_retired_strips`]).
    retired_strips: RetiredStrips,
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
    /// Built-in instances come from the host too.
    builtins: bool,
    /// Built-in instances made here although the host makes them.
    made: &'a mut u64,
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
        (self.external || self.builtins) && self.adopt.contains_key(&(t, key))
    }

    /// Creates and prepares an instance for `ins` here (allocates).
    fn create(&self, ins: &soundcraft_model::Insert, ch: usize) -> Option<Box<dyn Plugin>> {
        // External MIDI keeps its port in its state too.
        let state = if is_third_party(&ins.plugin) || ins.plugin == external_midi::ID { ins.state_bytes() } else { None };
        create_instance_for(&ins.plugin, state.as_deref(), self.sr, self.mb, ch, self.offline).map(|n| n.plugin)
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
        let hosted = if third { self.external } else { self.builtins };
        let plugin = if hosted {
            let given = match self.adopt.remove(&(t, key)) {
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
            };
            // A built-in the host did not make (it never fails to, but the sound must not depend
            // on that) is made here after all, and counted.
            given.or_else(|| {
                let made = if third { None } else { self.create(ins, ch) };
                if made.is_some() && !self.lent {
                    *self.made = self.made.saturating_add(1);
                }
                made
            })
        } else {
            self.create(ins, ch)
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
            kit_fx: Vec::new(),
            llm_off: false,
            llm_pos: LLM_SETTLED,
            llm_fresh: true,
            dry: DelayLine::default(),
            emits,
            midi: MidiOut::new(),
            wet: 1.0,
            wet_to: 1.0,
            quiet: 0,
            heard: 0,
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
            monitor: monitor::Monitor::default(),
            full_latency: 0,
            full_master_latency: 0,
            offline: false,
            meters: HashMap::new(),
            main_meter: StripMeter::default(),
            loudness: None,
            lra: None,
            loudness_on: false,
            loudness_fmt: None,
            meters_held: false,
            pan_law: PanLaw::Minus3,
            last_end: 0,
            last_start: 0,
            last_stopped: false,
            last_ports: [0; MAX_MIDI_PORTS],
            ports_gone: 0,
            synced: (0, 0, 0),
            order: Vec::new(),
            late: Vec::new(),
            latency: 0,
            track_latency: 0,
            aux_fed: Vec::new(),
            aux_latency: 0,
            input: Vec::new(),
            live_midi: LiveMidi::new(),
            monitor_only: false,
            recording: false,
            punch: None,
            metronome: false,
            looping: None,
            countoff: None,
            external: false,
            adopt: HashMap::new(),
            builtins: false,
            adopt_clip: HashMap::new(),
            made_in_sync: 0,
            adopt_strips: Vec::new(),
            adopt_busses: Vec::new(),
            strips_made_in_sync: 0,
            retired_strips: RetiredStrips::default(),
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

    /// The transport stopped: what the inserts, sends and aux returns still hold (a reverb, a
    /// delay, audio in the delay-compensation lines) rings on, stopped, and the meters show it,
    /// as a live release's tail does (`track_tail`: until the mix has been quiet for
    /// [`TAIL_QUIET_SECS`], at most [`LIVE_TAIL_SECS`], then faded and reset), instead of a reset
    /// cutting it. Clip notes are let go by the first stopped block (`render`). No allocation.
    pub fn ring_out(&mut self) {
        if !self.tail_on {
            (self.tail_on, self.tail_age, self.tail_quiet) = (true, 0, 0);
        }
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

    /// Measures the main mix's loudness and true peak (EBU R128) into `main_meter.loudness`
    /// while the transport rolls: the playback engine's master meter. Off by default (offline
    /// renders measure their own way). The meter is made at the next `sync`.
    pub fn set_loudness_metering(&mut self, on: bool) {
        self.loudness_on = on;
        if !on {
            self.loudness = None;
            self.lra = None;
            self.loudness_fmt = None;
            self.main_meter.loudness = None;
        }
    }

    /// Starts the loudness meter's integrated value, maxima and true peak afresh. Never
    /// allocates.
    pub fn reset_loudness(&mut self) {
        if let Some(l) = &mut self.loudness {
            l.reset();
        }
        if let Some((r, n)) = &mut self.lra {
            r.reset();
            *n = 0;
        }
        // The next block reads the fresh meter.
        self.main_meter.loudness = None;
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

    /// [`MixEngine::adopt`] for everything the host made for the session about to be rendered
    /// (its third-party instances and a [`BuiltinPrep`]'s built-in ones and Clip Effects
    /// modules). Does not allocate unless the adopt tables must grow.
    pub fn adopt_prepared(&mut self, p: Prepared) {
        let Prepared { instances, clip_fx, strips, busses, room } = p;
        self.adopt(instances);
        if let Some(r) = room {
            self.take_room(r);
        }
        if self.adopt_strips.is_empty() {
            self.adopt_strips = strips;
        } else {
            self.adopt_strips.extend(strips);
        }
        if self.adopt_busses.is_empty() {
            self.adopt_busses = busses;
        } else {
            self.adopt_busses.extend(busses);
        }
        for c in clip_fx {
            if let Some(old) = self.adopt_clip.insert((c.track, c.clip), c) {
                self.retired.extend(old.eq.into_iter().chain(old.comp));
            }
        }
    }

    /// Moves the strips, meters and busses into the host's larger empty tables (where they are
    /// larger); the tables let go of are retired.
    fn take_room(&mut self, r: StripRoom) {
        let StripRoom { mut strips, mut meters, mut busses } = r;
        if strips.capacity() > self.strips.capacity() {
            strips.extend(self.strips.drain());
            strips = std::mem::replace(&mut self.strips, strips);
        }
        if meters.capacity() > self.meters.capacity() {
            meters.extend(self.meters.drain());
            meters = std::mem::replace(&mut self.meters, meters);
        }
        if busses.capacity() > self.busses.capacity() {
            busses.extend(self.busses.drain());
            busses = std::mem::replace(&mut self.busses, busses);
        }
        self.retired_strips.strip_maps.push(strips);
        self.retired_strips.meter_maps.push(meters);
        self.retired_strips.bus_maps.push(busses);
    }

    /// Strips `sync` made itself (a new track's buffers sized on the render path) while the host
    /// was to make them ([`MixEngine::set_prepared_builtins`]); a realtime host keeps this at 0.
    #[doc(hidden)]
    pub fn strips_made_in_sync(&self) -> u64 {
        self.strips_made_in_sync
    }

    /// Address and capacity of a track's strip buffers and delay lines (see
    /// [`PreparedStrip::buffer_ids`]).
    #[doc(hidden)]
    pub fn prepared_buffers(&self, t: TrackId) -> Vec<(usize, usize)> {
        self.strips.get(&t).map(Strip::buffer_ids).unwrap_or_default()
    }

    /// Strips, tables and buffers taken out of service since the last call (drop them off the
    /// audio thread). Does not allocate.
    pub fn take_retired_strips(&mut self) -> RetiredStrips {
        std::mem::take(&mut self.retired_strips)
    }

    /// Realtime mode for built-in plugins and Clip Effects too: the host makes them with a
    /// [`BuiltinPrep`] on its control thread and hands them over with
    /// [`MixEngine::adopt_prepared`], so `sync` (which a realtime render runs on the audio thread)
    /// only seats them. Off by default.
    pub fn set_prepared_builtins(&mut self, on: bool) {
        self.builtins = on;
    }

    /// How many built-in instances and Clip Effects modules `sync` has made itself while the host
    /// was to make them ([`MixEngine::set_prepared_builtins`]); a realtime host keeps this at 0.
    #[doc(hidden)]
    pub fn made_in_sync(&self) -> u64 {
        self.made_in_sync
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

    /// Plays `note` on `track`'s instrument now (a pad's audition button): a note-on at
    /// `velocity`, or a note-off when it is 0. `false` without a working instrument there.
    /// Allocation-free (the plugin queues the event for its next block): callable on the audio
    /// thread.
    ///
    /// With the transport stopped the instrument then runs as after live input is let go (see
    /// `Strip::live_tail`): until its sound has died away, at most [`LIVE_TAIL_SECS`], so the hit
    /// is heard without arming the track.
    pub fn audition(&mut self, track: TrackId, note: u8, velocity: u8) -> bool {
        let tail = (f64::from(self.sample_rate) * LIVE_TAIL_SECS) as usize;
        let Some(strip) = self.strips.get_mut(&track) else { return false };
        let Some(inst) = strip.instrument.as_mut().filter(|i| !i.failed) else { return false };
        if velocity > 0 {
            inst.plugin.note_on(0, note.min(127), velocity.min(127));
        } else {
            inst.plugin.note_off(0, note.min(127));
        }
        strip.live_tail = strip.live_tail.max(tail);
        strip.tail_quiet = 0;
        true
    }

    /// All MIDI Notes Off (panic): every instrument lets go of every note and pedal now, whatever
    /// played it (clips, live input, modules, auditions): the notes live input holds get their
    /// note-offs, every channel gets sustain up, All Notes Off and All Sound Off, and the plugin's
    /// own reset of its voices runs. Clips playing on chase their controllers again on the next
    /// block. Audio thread: does not allocate.
    pub fn all_notes_off(&mut self) {
        for strip in self.strips.values_mut() {
            if let Some(inst) = strip.instrument.as_mut() {
                strip.live_held.release_ports(u64::MAX, |m| inst.plugin.midi_raw(0, m));
                for ch in 0..16u8 {
                    for (cc, v) in [(64, 0), (66, 0), (123, 0), (120, 0)] {
                        inst.plugin.midi_raw(0, [0xB0 | ch, cc, v]);
                    }
                }
                inst.plugin.all_notes_off();
            }
            strip.live_held = HeldNotes::default();
            strip.held.clear();
            strip.midi_held.clear();
            strip.chase = true;
        }
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
        out.extend(self.adopt_clip.drain().flat_map(|(_, p)| p.eq.into_iter().chain(p.comp)));
        out.extend(self.parked.drain().map(|(_, p)| p.plugin));
        self.retired_links.extend(self.links.drain(..).map(|g| g.slab));
        self.synced = (usize::MAX, usize::MAX, usize::MAX);
        out
    }

    /// What the last `render` put on each hardware output, indexed like `Session::outputs`
    /// (index 0, the main, is empty). The player maps them onto device channels. Each channel
    /// holds `max_block` samples, of which the rendered frames lead.
    /// What audio track `t` recorded in the last block when its take is not the device input as
    /// it came in: its bus input (internal recording), or what its inserts made of its input
    /// (record with inserts). Its channels (the block's first frames) and how late they run
    /// behind the timeline besides any input round trip (a bus's delay compensation, the
    /// inserts' latency; frames). `None` when its take records the device input.
    /// Audio thread: no allocation.
    pub fn record_tap(&self, t: TrackId) -> Option<(&[Vec<f32>], usize)> {
        let st = self.strips.get(&t)?;
        // Record with inserts: after them, so later by their latency too (a bus input's delay
        // and theirs; a hardware input's is the round trip the take is placed by anyway).
        if !st.print.is_empty() {
            let bus = if st.bus_in.is_empty() { 0 } else { st.base };
            return Some((st.print.as_slice(), bus.saturating_add(st.latency)));
        }
        let st = Some(st).filter(|st| !st.bus_in.is_empty())?;
        Some((st.bus_in.as_slice(), st.base))
    }

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
                apply_params(slot, ins, t, Some(soundcraft_model::INSTRUMENT_MODULE), pos);
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
                p.dry.clear();
                p.quiet = 0;
            }
            if let Some(i) = &mut s.instrument {
                i.plugin.all_notes_off();
                i.plugin.reset();
                i.failed = false;
            }
            s.held.clear();
            s.midi_held.clear();
            s.chase = true;
            // A fresh start has nothing to ramp from.
            s.mute_settled = false;
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
        let want = (self.main_fmt, self.sample_rate.to_bits());
        if self.loudness_on && self.loudness_fmt != Some(want) {
            let mut m = soundcraft_dsp::meter::LoudnessMeter::new(self.sample_rate, main_ch);
            for (c, w) in self.main_fmt.loudness_weights().into_iter().enumerate() {
                m.set_channel_weight(c, w);
            }
            self.loudness = Some(m);
            if self.lra.is_none() {
                self.lra = Some((soundcraft_dsp::meter::LoudnessRange::new(), 0));
            }
            self.loudness_fmt = Some(want);
        }
        self.hw.resize_with(s.outputs.len(), Vec::new);
        self.hw_delay.resize_with(s.outputs.len(), DelayLine::default);
        self.llm.reserve(s);
        self.solo.reserve(s);
        self.monitor.reserve(s, main_ch, mb);
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
            builtins: self.builtins,
            made: &mut self.made_in_sync,
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
                // Its buffers are freed off the audio thread too.
                self.retired_strips.strips.push(st);
            }
            // The meter keeps its copies of the handles: hand those over too.
            if let Some(m) = self.meters.get_mut(&id) {
                cx.retired_analysis.extend(m.analysis.drain(..).map(|(_, a)| a));
            }
        }
        for (ti, t) in s.tracks.iter().enumerate() {
            let ch = strip_channels(t, main_ch);
            if !self.strips.contains_key(&t.id) {
                // A realtime host made the strip with its buffers sized (`BuiltinPrep`).
                let made = match self.adopt_strips.iter().position(|p| p.track == t.id) {
                    Some(at) => self.adopt_strips.swap_remove(at).strip,
                    None => {
                        if self.builtins {
                            self.strips_made_in_sync = self.strips_made_in_sync.saturating_add(1);
                        }
                        Strip::new()
                    }
                };
                self.strips.insert(t.id, made);
            }
            let Some(strip) = self.strips.get_mut(&t.id) else { continue };
            if strip.buf.len() != ch || strip.buf.first().map_or(0, Vec::len) != mb {
                strip.buf = vec![vec![0.0; mb]; ch];
            }
            for g in [&mut strip.fader_gain, &mut strip.mute_gains] {
                if g.capacity() < mb {
                    *g = Vec::with_capacity(mb);
                }
            }
            // Room for the pre-fader copy (pre-fader sends, keys and meters), so filling it never
            // allocates while rendering.
            if strip.pre.len() != ch || strip.pre.iter().any(|b| b.capacity() < mb) {
                strip.pre = reserved(ch, mb);
            }
            let host = self.builtins.then_some(&mut self.adopt_clip);
            sync_clip_fx(strip, t, cx.sr, mb, cx.retired, host, cx.made);
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
            // MIDI tracks playing this track's instrument (MIDI to Track), after the input's
            // modules: indices from `ROUTED_MIDI` on.
            strip.midi_src.extend(
                s.tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.instrument.as_ref().and_then(track_midi::target_of) == Some(t.id))
                    .filter(|(_, x)| plan.midi_ok(t.id, ModuleRef { track: x.id, module: sidechain::INSTRUMENT_OUT }))
                    .enumerate()
                    .filter_map(|(j, (si, _))| Some((u8::try_from(ROUTED_MIDI + j).ok()?, si, sidechain::INSTRUMENT_OUT))),
            );
            strip.routed_src.reserve(strip.midi_src.len());
            strip.inst_outs.sync(t, self.max_block);
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
            let lfe = lfe_mask(strip_format(t, ch));
            for (slot, ins) in chain.iter_mut().zip(&t.mixer.inserts) {
                if let Some(slot) = slot.as_mut() {
                    slot.plugin.set_lfe_channels(lfe);
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
            // (External MIDI is made here even for a realtime host: a new port is a new instance.)
            let load = |id: &str, l: u64| if cx.external && id != external_midi::ID { 0 } else { l };
            if strip.instrument.as_ref().map(|p| (p.id.as_str(), p.ch, load(&p.id, p.state_load)))
                != want.map(|i| (i.plugin.as_str(), ich, load(&i.plugin, i.state_load)))
                || (want.is_some() && cx.waiting(t.id, SlotKey::Instrument))
            {
                let old = strip.instrument.take();
                cx.retire(t.id, SlotKey::Instrument, old);
                strip.instrument = want.and_then(|i| cx.make(t.id, SlotKey::Instrument, i, ich));
                strip.mpe_sent = None;
                strip.sample = None;
                strip.inst_readouts = [f32::NAN; 8];
            }
            sync_sample(strip, want, s, cx.retired_assets);
            if let (Some(slot), Some(ins)) = (strip.instrument.as_mut(), want) {
                sync_kit_samples(slot, ins, s, cx.retired_assets);
                sync_kit_fx(slot, ins, cx.sr, cx.mb, cx.offline, cx.retired);
            }
            // MIDI effects ahead of the instrument (a host around it once the track has one).
            if let Some(slot) = strip.instrument.as_mut() {
                midi_fx::sync(&mut slot.plugin, &t.midi_fx, cx.sr);
            }
            let n = if t.mixer.inserts.is_empty() { 0 } else { ch };
            if strip.llm_dry.len() != n || strip.llm_dry.iter().any(|b| b.len() != mb) {
                strip.llm_dry = vec![vec![0.0; mb]; n];
            }
            let n = if sidechain::reads_bus(t) { ch } else { 0 };
            if strip.bus_in.len() != n || strip.bus_in.iter().any(|b| b.len() != mb) {
                strip.bus_in = vec![vec![0.0; mb]; n];
            }
            let n = if t.kind == TrackKind::Audio && t.mixer.record_inserts { ch } else { 0 };
            if strip.print.len() != n || strip.print.iter().any(|b| b.len() != mb) {
                strip.print = vec![vec![0.0; mb]; n];
            }
            strip.dest = out_dest(s, &t.mixer.output);
            // A named input (or a legacy "In N" / "In N-M") gives its device channels; any other
            // route (no input, and the Band's musicians and a module's MIDI, which have no audio
            // input) hears the device's first channels in order, as `plan_takes` records them.
            (strip.in_span, strip.in_default) = match &t.mixer.input {
                Route::Hardware(h) => (s.input_channels(h), false),
                _ => (Some((0, ch)), true),
            };
        }
        // Sources tapped before their fader keep their pre-fader copy, sized here.
        for r in plan.routes.iter().filter(|r| r.pre_fader && r.result.is_ok()) {
            if let Some(st) = self.strips.get_mut(&r.source) {
                st.pre_key = true;
                let ch = st.buf.len();
                if st.pre.len() != ch || st.pre.iter().any(|b| b.capacity() < mb) {
                    st.pre = reserved(ch, mb);
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
        if !cx.lent {
            for (_, p) in self.adopt_clip.drain() {
                cx.retired.extend(p.eq.into_iter().chain(p.comp));
            }
            self.retired_strips.strips.extend(self.adopt_strips.drain(..).map(|p| p.strip));
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
            if bus.buf.len() != n || bus.buf.iter().any(|c| c.len() != mb) {
                // A realtime host sized them (`BuiltinPrep`); the old ones are freed off the audio thread.
                let made = match self.adopt_busses.iter().position(|(id, buf)| *id == b.id && buf.len() == n && buf.iter().all(|c| c.len() == mb)) {
                    Some(at) => self.adopt_busses.swap_remove(at).1,
                    None => vec![vec![0.0; mb]; n],
                };
                let old = std::mem::replace(&mut bus.buf, made);
                if !old.is_empty() {
                    self.retired_strips.buffers.push(old);
                }
            }
        }
        let retired = &mut self.retired_strips;
        self.busses.retain(|id, b| {
            let keep = s.bus(*id).is_some();
            if !keep {
                retired.buffers.push(std::mem::take(&mut b.buf));
            }
            keep
        });
        if !self.lent {
            self.retired_strips.buffers.extend(self.adopt_busses.drain(..).map(|(_, b)| b));
        }
        if !self.external {
            self.retired_strips.clear();
        }
        self.order = plan.order;
        self.late = plan.late;
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
            let ch = st.buf.len();
            // A latent insert's bypass line: its latency plus the same headroom, so a latency
            // that changes while playing re-aligns without allocating.
            for p in st.plugins.iter_mut().flatten() {
                let l = p.plugin.latency().max(p.dry.need());
                if l > 0 || p.dry.active() {
                    p.dry.reserve(l.saturating_add(headroom).min(MAX_LATENCY), ch);
                }
            }
            // Strips `render` never delays (a reactivated track re-syncs).
            if t.inactive || matches!(t.kind, TrackKind::Master | TrackKind::Vca) {
                continue;
            }
            st.align.reserve(cap(&st.align), ch);
            if t.mixer.delay > 0 || st.track_delay.active() {
                st.track_delay.reserve((t.mixer.delay as usize).max(st.track_delay.need()), ch);
            }
            for k in &mut st.keys {
                k.delay.reserve(cap(&k.delay), ch);
                k.main_delay.reserve(cap(&k.main_delay), ch);
            }
            st.inst_outs.reserve(cap);
            if t.mixer.sends.iter().flatten().any(|x| x.pre_fader) || st.align_pre.active() {
                st.align_pre.reserve(cap(&st.align_pre), ch);
            }
            let aux = matches!(t.kind, TrackKind::Aux | TrackKind::Folder);
            if !aux && matches!(t.mixer.output, Route::Main | Route::Bus(_) | Route::Hardware(_)) {
                st.out_extra.reserve(cap(&st.out_extra), ch);
                if st.outbuf.len() != ch || st.outbuf.iter().any(|b| b.capacity() < mb) {
                    st.outbuf = reserved(ch, mb);
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

    /// Feedback protection's length in frames ([`FEEDBACK_SECONDS`]); 0 offline or when
    /// Options › Recording › Feedback Protection is off.
    fn feedback_frames(&self, s: &Session) -> usize {
        if self.offline || s.edit.flag(NO_FEEDBACK_PROTECTION) { 0 } else { (self.sample_rate * FEEDBACK_SECONDS).max(1.0) as usize }
    }

    /// Whether record-armed tracks on auto input hear their input in the block of `frames` at
    /// `pos`: recording, and (with a punch range) the block touches it.
    fn auto_input(&self, pos: Samples, frames: usize) -> bool {
        let block = Range { start: pos, end: pos.saturating_add(i64::try_from(frames).unwrap_or(i64::MAX)) };
        self.recording && self.punch.is_none_or(|p| p.overlaps(&block))
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
        // (A port unplugged and back within a rescan keeps its identity: the hub says so.)
        self.ports_gone = self.live_midi.reset;
        for (k, (old, now)) in self.last_ports.iter().zip(self.live_midi.ports).enumerate() {
            if *old != 0 && *old != now {
                self.ports_gone |= 1u64 << k;
            }
        }
        self.last_ports = self.live_midi.ports;
        self.ensure_synced(s);
        // The session's pan law, read every block (a setting, not a sync).
        self.pan_law = session_pan_law(s);
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
        // Solo follows the signal path; read every block (a solo toggle never re-syncs). In the
        // AFL and PFL modes a solo is heard on the listen bus instead and mutes nothing.
        self.monitor.begin(s, frames, self.offline);
        let listening = self.monitor.mode != monitor::Listen::Off;
        let mut solo = std::mem::take(&mut self.solo);
        solo.fill(s);
        let order = std::mem::take(&mut self.order);
        let late_set = std::mem::take(&mut self.late);
        let late = |i: usize| late_set.get(i).copied().unwrap_or(false);
        let live = |t: &Track| !(t.kind == TrackKind::Master || t.inactive || t.kind == TrackKind::Vca);
        let is_aux = sidechain::is_aux;
        // Phase 1: independent strips (audio, instrument, MIDI) in parallel.
        let mut work = std::mem::take(&mut self.work);
        work.clear();
        for &i in &order {
            let Some(t) = s.tracks.get(i).filter(|t| live(t) && !is_aux(t) && !late(i)) else { continue };
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
            auto_input: self.auto_input(pos, frames),
            pdc,
            xf,
            scout: scout_win,
            band: &self.band_out,
            midi: &self.live_midi,
            ports_gone: self.ports_gone,
            looping: self.looping,
            feedback_frames: self.feedback_frames(s),
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
                    process_strip(s, t, st, &busses, &live_in, pos, frames, solo.muted(*i) && !listening);
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
                // Before the post-fader modules: it waits for them too.
                let pre_lat = st.latency.saturating_sub(st.post_lat);
                fits &= st.align_pre.set(lmax.saturating_sub(pre_lat), ch, xf);
                st.align_pre.process(&mut st.pre, frames);
                st.pre_at = pre_lat.max(lmax);
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
            // The instrument's extra outputs wait for the inserts after it, and on a bus an aux
            // returns onto, for the aux returns too.
            let inst_lat = st.instrument.as_ref().map_or(0, |i| i.plugin.latency());
            let (fed, llm) = (&self.aux_fed, st.llm);
            fits &= st.inst_outs.align(|b| lmax.saturating_sub(inst_lat) + if !llm && fed.contains(&b) { lmax_a } else { 0 }, xf, frames);
        }
        for (i, st) in work.iter_mut() {
            if let Some(t) = s.tracks.get(*i) {
                self.route_strip(s, t, st, pos, frames);
                self.listen_strip(t, st, pos, frames);
            }
        }
        for (i, st) in work.drain(..) {
            if let Some(t) = s.tracks.get(i) {
                self.strips.insert(t.id, st);
            }
        }
        self.work = work;
        let is_aux = sidechain::is_aux;
        // Phase 2: auxes in dependency order (they read busses), and the tracks keyed from them.
        for (i, t) in order.iter().filter_map(|&i| Some((i, s.tracks.get(i)?))).filter(|(i, t)| live(t) && (is_aux(t) || late(*i))) {
            let Some(mut st) = self.strips.remove(&t.id) else { continue };
            st.llm = on_path(i);
            if st.scratch.len() < self.max_block {
                st.scratch.resize(self.max_block, 0.0);
            }
            let live_in = LiveInput {
                input: &self.input,
                monitor_only: self.monitor_only,
                recording: self.recording,
                auto_input: self.auto_input(pos, frames),
                pdc,
                xf,
                scout: scout_win,
                band: &self.band_out,
                midi: &self.live_midi,
                ports_gone: self.ports_gone,
                looping: self.looping,
                feedback_frames: self.feedback_frames(s),
            };
            // An aux reads busses that already carry the tracks' compensation delay. An aux
            // without a bus input hears nothing but its own modules; it is compensated as if its
            // input were the tracks' (it returns alongside the other auxes), so its main input
            // counts as `track_latency` too (test `an_aux_consumer_lines_its_key_up_with_the_mix`).
            st.base = match &t.mixer.input {
                _ if late(i) => 0,
                Route::Bus(b) if pdc => self.bus_latency(*b),
                _ if pdc => self.track_latency,
                _ => 0,
            };
            for k in &mut st.keys {
                let src = k.source.and_then(|(_, id)| self.strips.get(&id)).map(Strip::key_source);
                fill_key(k, src, block, frames);
            }
            if late(i) {
                late_midi_in(&mut st, s, &self.strips, block);
            }
            process_strip(s, t, &mut st, &self.busses, &live_in, pos, frames, solo.muted(i) && !listening);
            st.stamp = block;
            fits &= !std::mem::take(&mut st.key_regrow);
            if late(i) {
                // Keyed from an aux, it lines up where the main mix is (the tracks and the aux
                // returns). Its latency is left out of `track_latency`: the busses feeding the aux
                // would otherwise wait for the key that waits for them.
                let lmax = if pdc && !st.llm { lmax_t.saturating_add(lmax_a) } else { 0 };
                fits &= align_late(t, &mut st, lmax, xf, frames);
                let inst_lat = st.instrument.as_ref().map_or(0, |i| i.plugin.latency());
                fits &= st.inst_outs.align(|_| lmax.saturating_sub(inst_lat), xf, frames);
                self.route_strip(s, t, &mut st, pos, frames);
                self.listen_strip(t, &st, pos, frames);
                self.strips.insert(t.id, st);
                continue;
            }
            let ch = st.buf.len();
            let lmax = if pdc && !st.llm { lmax_a } else { 0 };
            fits &= st.align.set(lmax.saturating_sub(st.latency), ch, xf);
            st.align.process(&mut st.buf, frames);
            st.post_at = st.base.saturating_add(st.latency.max(lmax));
            fits &= st.out_extra.set(0, ch, xf);
            self.route_strip(s, t, &mut st, pos, frames);
            self.listen_strip(t, &st, pos, frames);
            self.strips.insert(t.id, st);
        }
        self.order = order;
        self.late = late_set;
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
                if !t.modulators.is_empty() {
                    strip.modrun.advance(t, &strip.buf, frames, s.sample_rate.as_f64(), &transport);
                }
                for (i, ins) in t.mixer.inserts.iter().enumerate() {
                    // Post-fader modules run on the main mix after the master fader (below).
                    if ins.post_fader {
                        continue;
                    }
                    let mut meter = ModuleMeter { module: ins.id, in_peak: level, out_peak: level, ..ModuleMeter::default() };
                    if let Some(Some(slot)) = strip.plugins.get_mut(i)
                        && slot.module == ins.id
                    {
                        meter.failed = slot.failed;
                        if ins.active && !slot.failed {
                            apply_params(slot, ins, t, Some(ins.id), pos);
                            apply_timed(slot, ins, t, ins.id, pos, frames);
                            strip.modrun.apply(t, ins, &mut *slot.plugin, &mut slot.applied, pos, frames);
                            full_m = full_m.saturating_add(slot.plugin.latency());
                            let byp = bypassed(t, ins, pos);
                            let (run, room) = insert_step(slot, byp, strip.llm, &mut strip.buf, &mut strip.llm_dry, frames, xf);
                            fits &= room;
                            if slot.llm_off && !byp && i < 64 {
                                bits |= 1 << i;
                            }
                            if run == LlmRun::Skip {
                                if let Some(m) = strip.mods.get_mut(i) {
                                    *m = meter;
                                }
                                continue;
                            }
                            slot.plugin.set_transport(&transport);
                            let t0 = load_clock();
                            slot.plugin.process(&mut strip.buf, frames);
                            meter.load = load_since(t0, frames, s.sample_rate.as_f64());
                            if run == LlmRun::Fade {
                                llm_blend(slot, &mut strip.buf, &strip.llm_dry, frames, xf);
                            }
                            wet_blend(slot, &mut strip.buf, &strip.llm_dry, frames);
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
                // Post-fader modules (`Insert::post_fader`, a dither last): on the main mix after
                // the master fader.
                if t.mixer.inserts.iter().any(|i| i.post_fader) {
                    for (d, src) in strip.buf.iter_mut().zip(self.main.iter()) {
                        if let (Some(d), Some(x)) = (d.get_mut(..frames), src.get(..frames)) {
                            d.copy_from_slice(x);
                        }
                    }
                    for (i, ins) in t.mixer.inserts.iter().enumerate().filter(|(_, i)| i.post_fader) {
                        let level = chain_peak(&strip.buf, frames);
                        let mut meter = ModuleMeter { module: ins.id, in_peak: level, out_peak: level, ..ModuleMeter::default() };
                        if let Some(Some(slot)) = strip.plugins.get_mut(i)
                            && slot.module == ins.id
                            && ins.active
                            && !slot.failed
                        {
                            apply_params(slot, ins, t, Some(ins.id), pos);
                            apply_timed(slot, ins, t, ins.id, pos, frames);
                            strip.modrun.apply(t, ins, &mut *slot.plugin, &mut slot.applied, pos, frames);
                            full_m = full_m.saturating_add(slot.plugin.latency());
                            let (run, room) = insert_step(slot, bypassed(t, ins, pos), strip.llm, &mut strip.buf, &mut strip.llm_dry, frames, xf);
                            fits &= room;
                            if run != LlmRun::Skip {
                                slot.plugin.set_transport(&transport);
                                let t0 = load_clock();
                                slot.plugin.process(&mut strip.buf, frames);
                                meter.load = load_since(t0, frames, s.sample_rate.as_f64());
                                if run == LlmRun::Fade {
                                    llm_blend(slot, &mut strip.buf, &strip.llm_dry, frames, xf);
                                }
                                wet_blend(slot, &mut strip.buf, &strip.llm_dry, frames);
                                collect_released(slot, &mut strip.released);
                                if contain_failure(slot, &mut strip.buf, frames) {
                                    strip.failures.record(SlotKey::Module(ins.id));
                                    meter.failed = true;
                                } else {
                                    meter.gr_db = slot.plugin.gain_reduction_db();
                                }
                                meter.out_peak = chain_peak(&strip.buf, frames);
                            }
                        }
                        if let Some(m) = strip.mods.get_mut(i) {
                            *m = meter;
                        }
                    }
                    for (d, src) in self.main.iter_mut().zip(strip.buf.iter()) {
                        if let (Some(d), Some(x)) = (d.get_mut(..frames), src.get(..frames)) {
                            d.copy_from_slice(x);
                        }
                    }
                }
                strip.llm_bits = bits;
                let m = self.meters.entry(t.id).or_default();
                m.measure(&self.main, frames, 0.0, self.meters_held);
                m.llm_bypassed = bits;
                m.plugin_failures = strip.failures;
                m.modules.clone_from(&strip.mods);
                m.analysis.clone_from(&strip.analysis);
                m.instrument = strip.inst_readouts;
                m.hear_midi(strip.midi_vel, self.meters_held);
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
        self.cue_stage(s, frames);
        self.regrow |= !fits;
        // Before the clicks: they are not the mix's tail, nor what the output meter shows.
        self.track_tail(s, frames);
        self.main_meter.measure(&self.main, frames, 0.0, self.meters_held);
        if let Some(l) = &mut self.loudness {
            if !self.monitor_only {
                l.process(&self.main, frames);
                // The short-term loudness every 100 ms feeds the loudness range.
                if let Some((r, n)) = &mut self.lra {
                    let step = ((self.sample_rate / 10.0) as usize).max(1);
                    *n += frames;
                    while *n >= step {
                        *n -= step;
                        r.add(l.short_term_lufs());
                    }
                }
            }
            self.main_meter.loudness = Some(LoudnessReading {
                momentary: l.momentary_lufs(),
                short_term: l.short_term_lufs(),
                integrated: l.integrated_lufs(),
                max_momentary: l.max_momentary_lufs(),
                true_peak_db: l.true_peak_db(),
                range_lu: self.lra.as_ref().map_or(0.0, |(r, _)| r.range_lu()),
            });
        }
        // Metronome: summed after the master fader, so it reaches the speakers without passing
        // through any track, send or insert. Only the playback engine sets `metronome`.
        // Not while monitoring only (transport stopped): the position does not advance there, so the
        // start of the same beat would repeat every block as a buzz.
        // With a click output chosen (Setup › Click/Countoff), the clicks go to that hardware
        // output alone instead of the main mix.
        let click_to = click::output(s).filter(|i| self.hw.get(*i).is_some_and(|h| !h.is_empty()));
        let clicks = match click_to.and_then(|i| self.hw.get_mut(i)) {
            Some(h) => h,
            None => &mut self.main,
        };
        if self.metronome && !self.monitor_only && s.edit.click && (!s.edit.flag("click.only_during_record") || self.recording) {
            click::render(s, pos, frames, clicks);
        }
        // The count-off before a take: its own clicks, counted from its start (the transport
        // stands at the record point meanwhile, so the tempo map cannot place them).
        if self.metronome
            && let Some((c, elapsed)) = self.countoff
        {
            click::render_countoff(s, &c, elapsed, frames, clicks);
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
        // The control room's monitor path (live only: after the meters, never in a bounce).
        self.monitor_stage(s, frames, out);
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
        // Options › Pre-Fader Metering: the strip's level before its fader (and mute).
        let metered = if s.edit.pre_fader_metering && strip.pre.len() == strip.buf.len() { &strip.pre } else { &strip.buf };
        m.measure(metered, frames, strip.gr, self.meters_held);
        m.llm_bypassed = strip.llm_bits;
        m.feedback = strip.fb_muted;
        m.plugin_failures = strip.failures;
        m.modules.clone_from(&strip.mods);
        m.analysis.clone_from(&strip.analysis);
        m.instrument = strip.inst_readouts;
        m.hear_midi(strip.midi_vel, self.meters_held);
        let src_fmt = strip_format(t, strip.buf.len());
        let law = self.pan_law;
        // Automation is read where the audio in the buffer is on the timeline (see `apos` in
        // `process_strip`): the post-fader buffer `post_at` samples back, the pre-fader copy
        // `pre_at`.
        let back = |d: usize| pos.saturating_sub(i64::try_from(d).unwrap_or(0));
        // Options › More › Pre-Fader Sends Ignore Mute: a muted (or solo-muted) track's pre-fader
        // sends still play, as cue mixes want.
        let pre_ignores_mute = s.edit.flag(PRE_SENDS_IGNORE_MUTE);
        for (i, snd) in t.mixer.sends.iter().enumerate() {
            let Some(snd) = snd else { continue };
            if strip.silent && !(snd.pre_fader && pre_ignores_mute) {
                continue;
            }
            // An additional output is the main output again: post-fader, unity, the main pan.
            let as_output = snd.output;
            let pre_fader = snd.pre_fader && !as_output;
            let src = if pre_fader { &strip.pre } else { &strip.buf };
            // A send reaches a bus, or a hardware output (or the main mix) directly.
            let dst: (&mut Vec<Vec<f32>>, ChannelFormat) = match out_dest(s, &snd.target) {
                Dest::Bus(b) => match self.busses.get_mut(&b) {
                    Some(d) => (&mut d.buf, d.fmt),
                    None => continue,
                },
                Dest::Main => (&mut self.main, self.main_fmt),
                Dest::Hw(h) => match (self.hw.get_mut(h), s.outputs.get(h)) {
                    (Some(d), Some(o)) if !d.is_empty() => (d, o.format),
                    _ => continue,
                },
                Dest::None => continue,
            };
            let (dst_buf, dst_fmt) = dst;
            let apos = back(if pre_fader { strip.pre_at } else { strip.post_at });
            let follows = as_output || (snd.surround.is_none() && snd.follow_main_pan);
            // An automated send changes on the timeline's PAN_STEP grid, as the pan does: any
            // block size (playback or bounce) sends the same.
            let mut start = 0;
            while start < frames {
                let (at, end) = if as_output { pan_segment(t, apos, start, frames) } else { send_segment(t, i, apos, start, frames) };
                let span = start..end;
                start = end;
                if !as_output && send_muted(t, i, snd.mute, at) {
                    continue;
                }
                let gain = if as_output { 1.0 } else { db_to_gain(send_level(t, i, snd.level_db, at)) };
                if gain <= 0.0 {
                    continue;
                }
                // Mono sends use the send's own pan; a stereo send's pan balances it (see
                // `stereo_send_pans`). A send whose pan follows the main pan takes the track's.
                let stereo = if snd.pan_follows_main || as_output {
                    [pan_at(t, 0, at), pan_at(t, 1, at)]
                } else {
                    let p = send_pan(t, i, snd.pan, at);
                    if src.len() == 1 { [p, 0.0] } else { stereo_send_pans(p) }
                };
                // A send following the main pan follows its surround lanes too.
                let main_sp = if follows { surround_at(t, at) } else { None };
                let surround = if as_output { main_sp.as_ref() } else { snd.surround.as_ref().or(main_sp.as_ref()) };
                let m = build_matrix(src_fmt, src.len(), dst_fmt, dst_buf.len(), &PanSpec { stereo, surround }, law);
                apply_matrix_span(src, span, dst_buf, &m, gain);
            }
        }
        // The instrument's extra outputs, each into its bus (a silent strip plays none).
        if strip.inst_outs.fresh && !strip.silent {
            for o in &strip.inst_outs.outs {
                if let Some(d) = self.busses.get_mut(&o.bus) {
                    inst_outs::add_into(&o.buf, &mut d.buf, frames);
                }
            }
        }
        let dest = strip.dest;
        let extra = if matches!(dest, Dest::Main | Dest::Bus(_) | Dest::Hw(_)) && strip.out_extra.active() { strip.out_extra.delay() } else { 0 };
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
        // The pan reads where its audio is on the timeline: behind by the strip's delay and the
        // extra wait of a direct output.
        let apos = back(strip.post_at.saturating_add(extra));
        let mut start = 0;
        while start < frames {
            let (at, end) = pan_segment(t, apos, start, frames);
            let sp = surround_at(t, at);
            let spec = PanSpec { stereo: [pan_at(t, 0, at), pan_at(t, 1, at)], surround: sp.as_ref() };
            let mut m = build_matrix(src_fmt, src.len(), dst_fmt, dst.len(), &spec, law);
            if src.len() == 2 {
                shape_stereo(&mut m, shape_at(t, AutoParam::Width, t.mixer.width, at), shape_at(t, AutoParam::Balance, t.mixer.balance, at));
            }
            apply_matrix_span(src, start..end, dst, &m, 1.0);
            start = end;
        }
    }
}

/// The `Session::edit` flag of Options › More › Pre-Fader Sends Ignore Mute.
pub const PRE_SENDS_IGNORE_MUTE: &str = "mix.pre_sends_ignore_mute";

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

/// One active insert's bypass and low-latency monitoring for this block: what it does
/// (`llm_step`, off when bypassed or a latent module on the low-latency path), with `dry` holding
/// its input on the way past it. A bypassed insert (and one crossfading to or from that) passes its
/// input delayed by the plugin's latency, so bypass neither clicks nor moves the strip's delay
/// compensation, and a plugin switched back on starts clean (`llm_step` resets it) and is heard
/// once its latency has passed. On the low-latency path the dry input is not delayed. A skipped
/// insert's dry input is copied into `buf`. False when the dry line needs more room than it has
/// (the next `sync` grows it). Does not allocate.
fn insert_step(
    slot: &mut PluginSlot,
    bypass: bool,
    llm: bool,
    buf: &mut [Vec<f32>],
    dry: &mut [Vec<f32>],
    frames: usize,
    xf: usize,
) -> (LlmRun, bool) {
    let lat = slot.plugin.latency();
    let run = llm_step(slot, bypass || (llm && lat > 0), xf);
    let delayed = lat > 0 || slot.dry.delay() > 0;
    let mixed = slot.wet < 1.0 || slot.wet_to < 1.0;
    if run == LlmRun::Run && !delayed && !mixed {
        return (run, true);
    }
    copy_dry(buf, dry, frames);
    let mut fits = true;
    if delayed {
        // Only a line the output uses asks for more room (a sync then grows the strip's lines).
        fits = slot.dry.set(if llm { 0 } else { lat }, dry.len(), xf) || run == LlmRun::Run;
        slot.dry.process(dry, frames);
        if run == LlmRun::Skip {
            copy_dry(dry, buf, frames);
        }
    }
    (run, fits)
}

/// The latency an active insert adds to its strip this block, after `insert_step` said `run`:
/// a skipped one its dry line's delay, one that runs (or crossfades) the plugin's, but none when
/// low-latency monitoring takes it out.
fn insert_latency(slot: &PluginSlot, run: LlmRun, llm: bool) -> usize {
    match run {
        LlmRun::Skip => slot.dry.delay(),
        _ if llm && slot.llm_off => 0,
        _ => slot.plugin.latency(),
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

/// An insert's wet/dry mix ([`Insert::mix`](soundcraft_model::Insert::mix)): its output `buf`
/// blended with its input `dry` (delayed by its latency in `insert_step`, so no comb), the amount
/// gliding across the block from the last one heard. At full wet nothing is touched.
fn wet_blend(slot: &mut PluginSlot, buf: &mut [Vec<f32>], dry: &[Vec<f32>], frames: usize) {
    let (from, to) = (slot.wet, slot.wet_to);
    if from >= 1.0 && to >= 1.0 {
        return;
    }
    let span = frames.max(1) as f32;
    for (b, d) in buf.iter_mut().zip(dry) {
        for (i, (x, y)) in b.iter_mut().zip(d.iter()).take(frames).enumerate() {
            let w = from + (to - from) * (i + 1) as f32 / span;
            *x = *y + (*x - *y) * w;
        }
    }
    slot.wet = to;
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

/// The first `midi_src` index of a MIDI track playing this strip's instrument (MIDI to Track);
/// the input's own modules come before it.
const ROUTED_MIDI: usize = 128;

/// Source → trim → inserts → (pre-fader copy) → mute/fader for one strip. Independent of other
/// strips except that aux inputs read `busses`, so non-aux strips can run in parallel.
/// Live input routed to monitored tracks.
struct LiveInput<'a> {
    input: &'a [Vec<f32>],
    monitor_only: bool,
    recording: bool,
    /// Recording, and inside the punch range if there is one: auto-input tracks hear their input.
    auto_input: bool,
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
    ports_gone: u64,
    /// The loop being played ([`MixEngine::looping`]).
    looping: Option<Range>,
    /// Feedback protection: frames of hot input that silence a monitored input; 0 when it is off.
    feedback_frames: usize,
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
    // What an instrument passed on last block (MIDI to Track) was read before this strip ran
    // again; one that does not run this block passes nothing.
    if let Some(inst) = strip.instrument.as_mut().filter(|i| i.emits) {
        inst.midi.clear();
    }
    // Other tracks playing this one's instrument (MIDI to Track): their events are in `midi_in`.
    let routed = strip.midi_src.iter().any(|x| usize::from(x.0) >= ROUTED_MIDI);
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
        Route::Midi(m) if hears_live_midi(t, live.midi.thru) => Some(MidiFilter::for_input(m)),
        _ => None,
    };
    // Notes (and pedals) the input held are let go when the track stops hearing it or hears
    // another port or channel (their note-offs would not come through), and those from a port
    // slot whose port closed (or changed) when it goes.
    let gone = if live_filter != strip.live_filter { u64::MAX } else { live.ports_gone };
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
    // An audio track on a bus input: this block of the bus, monitored or not (its take records it).
    if let Route::Bus(b) = &t.mixer.input
        && !strip.bus_in.is_empty()
    {
        for c in strip.bus_in.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        if let Some(src) = busses.get(b) {
            read_bus(src, t, &mut strip.bus_in, frames, session_pan_law(s));
        }
    }
    // Record with inserts: while it records, the input goes through the strip whether it is
    // heard or not (`silent_print`: not heard, so the strip's output is silence).
    let input_only = t.mixer.record_arm && t.mixer.input_monitor == soundcraft_model::InputMonitor::Auto && s.edit.flag(INPUT_ONLY_MONITORING);
    let hears = input_only || t.mixer.hears_input(live.monitor_only, live.auto_input);
    if !hears {
        // Feedback protection lets go once the input is no longer heard (its I key turned off).
        (strip.fb_hot, strip.fb_muted) = (0, false);
    }
    let prints = !strip.print.is_empty() && frozen.is_none() && t.mixer.record_arm && live.auto_input && !live.monitor_only;
    let silent_print = prints && !hears;
    let hears = hears || prints;
    // The instrument ran its release this block: a strip whose instrument did not run (frozen,
    // failed, gone, or no longer an instrument track) has no release to wait for.
    let mut tail_ran = false;
    match t.kind {
        _ if frozen.is_some() => {
            if let Some(fz) = frozen.filter(|_| !live.monitor_only) {
                play_frozen(s, fz, pos, frames, &mut strip.buf);
            }
        }
        TrackKind::Audio if !strip.bus_in.is_empty() && hears => {
            // Input monitoring of a bus: the track hears the bus instead of its clips.
            strip.hears_live = true;
            for (dst, src) in strip.buf.iter_mut().zip(&strip.bus_in) {
                for (d, x) in dst.iter_mut().zip(src.iter()).take(frames) {
                    *d = *x;
                }
            }
        }
        TrackKind::Audio if !live.input.is_empty() && !matches!(t.mixer.input, Route::Bus(_)) && hears => {
            // Input monitoring: the track hears its hardware input instead of its clips. A named
            // input (or a legacy "In N" / "In N-M") gives its device channels; with no hardware
            // input, `default_input_channel` (the channels its take records).
            // An unknown input, or a channel the device lacks, is silence: never wrapped onto
            // another channel.
            strip.hears_live = true;
            if let Some((first, width)) = strip.in_span {
                let width = if strip.in_default { width.min(live.input.len()).max(1) } else { width };
                for (k, dst) in strip.buf.iter_mut().enumerate() {
                    if let Some(src) = track_input_channel(k, first, width).and_then(|i| live.input.get(i)) {
                        for (d, x) in dst.iter_mut().zip(src.iter()).take(frames) {
                            *d = *x;
                        }
                    }
                }
            }
            feedback_guard(strip, frames, live.feedback_frames);
        }
        TrackKind::Audio if live.monitor_only => {}
        // Recording over them (input monitoring Off, or no input to hear): the clips under the
        // take are not heard, as the take will replace them.
        TrackKind::Audio if t.mixer.record_arm && live.auto_input => {}
        TrackKind::Instrument | TrackKind::Midi
            if live.monitor_only && musician.is_none() && !module_in && !routed && live_filter.is_none() && strip.live_tail == 0 => {}
        TrackKind::Audio => {
            let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
            // An auditioned take layer plays in place of the comp (`Track::heard_clips`).
            let clips = t.heard_clips();
            for clip in clips {
                if clip.muted || !clip.is_audio() || !clip.range().overlaps(&block) {
                    continue;
                }
                let edges = Edges::of(s, clips, clip, block, live.looping);
                let cover = Cover::of(clips, clip, block, auto_fade_frames(s));
                // The effects and the buffer were made by `sync` when the session changed: here
                // nothing is allocated. Effects not prepared yet (never the case after a sync)
                // are skipped for this block rather than made on the audio thread.
                let ready = strip.clipbuf.len() == strip.buf.len() && strip.clipbuf.iter().all(|c| c.len() >= frames);
                let fx = clip_fx_params(clip).filter(|_| ready).and_then(|params| {
                    let fx = strip.clip_fx.get_mut(&clip.id.0).filter(|fx| fx.ready(params.clone()))?;
                    fx.configure(params);
                    Some(fx)
                });
                match fx {
                    None => render_clip(s, clip, pos, frames, &mut strip.buf, edges, cover),
                    Some(fx) => {
                        for c in strip.clipbuf.iter_mut() {
                            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
                        }
                        render_clip(s, clip, pos, frames, &mut strip.clipbuf, edges, cover);
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
                read_bus(src, t, &mut strip.buf, frames, session_pan_law(s));
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
                    apply_params(inst, ins, t, Some(soundcraft_model::INSTRUMENT_MODULE), pos);
                }
                inst.plugin.set_transport(&transport);
                if inst.id == track_midi::ID {
                    // A muted source lets go of what it holds on the target and starts nothing.
                    inst.plugin.set_param("muted", if t.mixer.mute || solo_muted || strip.muted { 1.0 } else { 0.0 });
                }
                // Tracks that stopped playing into this instrument let go of the notes they held;
                // the others play on, their held notes following their (maybe new) index.
                let is_routed = |x: &&(u8, usize, ModuleId)| usize::from(x.0) >= ROUTED_MIDI;
                let now = strip.midi_src.iter().filter(is_routed).map(|x| (x.0, s.tracks.get(x.1).map(|t| t.id)));
                if !now.eq(strip.routed_src.iter().map(|x| (x.0, Some(x.1)))) {
                    let (src, was) = (&strip.midi_src, &strip.routed_src);
                    strip.midi_held.retain_mut(|(k, ch, pitch)| {
                        if usize::from(*k) < ROUTED_MIDI {
                            return true;
                        }
                        let track = was.iter().find(|x| x.0 == *k).map(|x| x.1);
                        match src.iter().filter(is_routed).find(|x| track.is_some() && s.tracks.get(x.1).map(|t| t.id) == track) {
                            Some(x) => {
                                *k = x.0;
                                true
                            }
                            None => {
                                inst.plugin.midi_raw(0, [0x80 | (*ch & 0x0F), *pitch & 0x7F, 0]);
                                false
                            }
                        }
                    });
                    strip.routed_src.clear();
                    strip.routed_src.extend(strip.midi_src.iter().filter(is_routed).filter_map(|x| Some((x.0, s.tracks.get(x.1)?.id))));
                }
                // A musician or module input has no clip controllers to chase (and while only live
                // MIDI is heard, stopped, the clips wait for the transport).
                // The track's MPE zone, sent to the instrument when it changes (or is new).
                if strip.mpe_sent != t.mpe {
                    let mut msgs = [[0u8; 3]; rpn::MPE_SETUP_MAX];
                    let n = rpn::mpe_setup(t.mpe, strip.mpe_sent, &mut msgs);
                    for m in msgs.iter().take(n) {
                        inst.plugin.midi_raw(0, *m);
                    }
                    strip.mpe_sent = t.mpe;
                }
                let chased = !live.monitor_only && musician.is_none() && !module_in && std::mem::take(&mut strip.chase);
                if chased {
                    strip.pass = strip.pass.wrapping_add(1);
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
                            schedule_notes(s, t, pos, frames, inst, &mut strip.held, chased && s.edit.flag("midi.note_chase"), strip.pass);
                        }
                        if let Some(f) = live_filter {
                            deliver_live_midi(live.midi, f, inst, &mut strip.live_held);
                        }
                        // And what the MIDI tracks routed to it play, each on its channel.
                        if routed {
                            deliver_module_midi(&strip.midi_in, inst, &mut strip.midi_held);
                        }
                    }
                }
                strip.inst_outs.before(&mut *inst.plugin);
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
                strip.inst_outs.after(&mut *inst.plugin, frames);
                if inst.emits {
                    inst.plugin.midi_out(&mut inst.midi);
                }
                if contain_failure(inst, &mut strip.buf, frames) {
                    strip.failures.record(SlotKey::Instrument);
                }
                read_readouts(&*inst.plugin, &mut strip.inst_readouts);
                strip.midi_vel = std::mem::take(&mut inst.heard);
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
    let trim = db_to_gain(t.mixer.trim_db);
    if ((trim - 1.0).abs() > f32::EPSILON || t.mixer.inverts_any()) && frozen.is_none() {
        for (ch, c) in strip.buf.iter_mut().enumerate() {
            let g = if t.mixer.inverts(ch) { -trim } else { trim };
            if (g - 1.0).abs() > f32::EPSILON {
                c.iter_mut().take(frames).for_each(|x| *x *= g);
            }
        }
    }
    // The track's own delay (Pro Tools' and Cubase's channel delay): before the inserts, left out
    // of the latency delay compensation lines up. A change crossfades.
    if t.mixer.delay > 0 || strip.track_delay.active() {
        let ch = strip.buf.len();
        strip.key_regrow |= !strip.track_delay.set(t.mixer.delay as usize, ch, live.xf);
        strip.track_delay.process(&mut strip.buf, frames);
    }
    // Scout PRE: after the source, trim and phase, immediately before the first insert. Like POST
    // it is ahead of delay compensation (`align`), so not PDC-aligned across strips.
    if let (Some(win), Some(taps)) = (live.scout, strip.scout.as_deref_mut()) {
        taps.feed_pre(&strip.buf, win);
    }
    let mut gr = 0.0f32;
    // The instrument's own latency (a hosted instrument with look-ahead) is compensated as an
    // insert's is: the strip's sound is that late. The built-in instruments report none; a frozen
    // track plays its render, without the instrument.
    let mut latency = if frozen.is_none() { strip.instrument.as_ref().filter(|i| !i.failed).map_or(0, |i| i.plugin.latency()) } else { 0 };
    let mut level = chain_peak(&strip.buf, frames);
    let mut bits = 0u64;
    let mut skipped = 0usize;
    let on_llm = strip.llm;
    if !t.modulators.is_empty() {
        strip.modrun.advance(t, &strip.buf, frames, s.sample_rate.as_f64(), &transport);
    }
    // The pre-fader modules, then the fader section, then the post-fader modules
    // (`Insert::post_fader`) on what the fader left. A post-fader module is never frozen.
    let mut pre_latency = 0usize;
    // Suspend Plug-ins on Silence: what counts as long enough, 0 when off.
    let suspend_after = if s.edit.flag(SUSPEND_ON_SILENCE) { (s.sample_rate.as_f64() * SUSPEND_AFTER_SECS).max(1.0) as usize } else { 0 };
    for post in [false, true] {
        if post {
            pre_latency = latency.min(MAX_LATENCY);
            let at = FaderAt { prints, silent_print, gr, bits, skipped, latency: pre_latency };
            fader_section(s, t, strip, live, pos, frames, solo_muted, at);
            if !t.mixer.inserts.iter().any(|i| i.post_fader) {
                break;
            }
        }
        for (i, ins) in t.mixer.inserts.iter().enumerate() {
            if ins.post_fader != post {
                continue;
            }
            if i < frozen_mods && !post {
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
                // Bypass crossfades and keeps the module's latency; low-latency monitoring takes a
                // latent module on the live path out (see `insert_step`).
                let live_mod = ins.active && !slot.failed;
                let run = if live_mod {
                    // Its automation is read where its input is on the timeline: behind by the
                    // strip's input delay and the modules before it (as the fader's is), so a
                    // move lines up with the audio it moves, in playback and bounce alike.
                    let apos = pos.saturating_sub(i64::try_from(strip.base.saturating_add(latency)).unwrap_or(0));
                    apply_params(slot, ins, t, Some(ins.id), apos);
                    apply_timed(slot, ins, t, ins.id, apos, frames);
                    strip.modrun.apply(t, ins, &mut *slot.plugin, &mut slot.applied, apos, frames);
                    let byp = bypassed(t, ins, apos);
                    let (run, fits) = insert_step(slot, byp, strip.llm, &mut strip.buf, &mut strip.llm_dry, frames, live.xf);
                    strip.key_regrow |= !fits;
                    if slot.llm_off && !byp {
                        skipped = skipped.saturating_add(slot.plugin.latency());
                        if i < 64 {
                            bits |= 1 << i;
                        }
                    }
                    run
                } else {
                    LlmRun::Skip
                };
                if live_mod && run == LlmRun::Skip {
                    latency = latency.saturating_add(insert_latency(slot, run, on_llm));
                }
                // Suspended: its input is silent and its output has been a while; a keyed module
                // or one that sends MIDI always runs. It keeps its latency (the sound is silence).
                let quiet_in = level < TAIL_SILENCE;
                let rest =
                    suspend_after > 0 && run == LlmRun::Run && quiet_in && slot.quiet >= suspend_after && ins.sidechain.is_none() && !slot.emits;
                if rest {
                    latency = latency.saturating_add(insert_latency(slot, run, on_llm));
                }
                if run != LlmRun::Skip && !rest {
                    slot.plugin.set_transport(&transport);
                    let t0 = load_clock();
                    match strip.keys.iter_mut().find(|k| k.module == ins.id).filter(|_| ins.sidechain.is_some()) {
                        Some(key) => {
                            // Line the key up with this module's main input: the earlier one waits.
                            let main_at = strip.base.saturating_add(latency);
                            // On the low-latency path the main input never waits for a key.
                            let (wait_key, wait_main) = if live.pdc {
                                (main_at.saturating_sub(key.at), if on_llm { 0 } else { key.at.saturating_sub(main_at) })
                            } else {
                                (0, 0)
                            };
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
                    meter.load = load_since(t0, frames, s.sample_rate.as_f64());
                    if run == LlmRun::Fade {
                        llm_blend(slot, &mut strip.buf, &strip.llm_dry, frames, live.xf);
                    }
                    wet_blend(slot, &mut strip.buf, &strip.llm_dry, frames);
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
                        latency = latency.saturating_add(insert_latency(slot, run, on_llm));
                        meter.gr_db = slot.plugin.gain_reduction_db();
                        if gr == 0.0 {
                            gr = meter.gr_db;
                        }
                        read_readouts(&*slot.plugin, &mut meter.readouts);
                    }
                    level = chain_peak(&strip.buf, frames);
                    meter.out_peak = level;
                    slot.quiet = if quiet_in && level < TAIL_SILENCE { slot.quiet.saturating_add(frames) } else { 0 };
                }
            }
            // `sync` sized `mods` to the chain; never grow it here.
            if let Some(m) = strip.mods.get_mut(i) {
                *m = meter;
            }
        }
    }
    // With the post-fader modules: the strip's whole latency (`post_at`); the pre-fader copy
    // stays at `pre_at`, before them.
    strip.gr = gr;
    strip.llm_bits = bits;
    strip.llm_latency = skipped.min(MAX_LATENCY);
    strip.latency = latency.min(MAX_LATENCY);
    strip.post_lat = strip.latency.saturating_sub(pre_latency);
    strip.post_at = strip.base.saturating_add(strip.latency);
}

/// Where the pre-fader modules left the chain (see [`fader_section`]).
struct FaderAt {
    prints: bool,
    silent_print: bool,
    gr: f32,
    bits: u64,
    skipped: usize,
    /// The pre-fader modules' latency.
    latency: usize,
}

/// The chain after its pre-fader modules: Scout POST, record with inserts, the pre-fader copy,
/// the fader and the mute.
#[allow(clippy::too_many_arguments)]
fn fader_section(s: &Session, t: &Track, strip: &mut Strip, live: &LiveInput<'_>, pos: Samples, frames: usize, solo_muted: bool, at: FaderAt) {
    let FaderAt { prints, silent_print, gr, bits, skipped, latency } = at;
    // Scout POST: after the last insert, before the pre-fader copy, the fader and the pan.
    if let (Some(win), Some(taps)) = (live.scout, strip.scout.as_deref_mut()) {
        taps.feed_post(&strip.buf, win);
    }
    // Record with inserts: the take is this, after the inserts and before the fader.
    if !strip.print.is_empty() {
        for (d, src) in strip.print.iter_mut().zip(strip.buf.iter()) {
            for (x, y) in d.iter_mut().zip(src.iter()).take(frames) {
                *x = if prints { *y } else { 0.0 };
            }
        }
        if silent_print {
            for c in strip.buf.iter_mut() {
                c.iter_mut().take(frames).for_each(|x| *x = 0.0);
            }
        }
    }
    strip.gr = gr;
    strip.llm_bits = bits;
    strip.llm_latency = skipped.min(MAX_LATENCY);
    strip.latency = latency;
    strip.post_lat = 0;
    strip.post_at = strip.base.saturating_add(strip.latency);
    strip.pre_at = strip.post_at;
    // The fader section reads its automation where the audio now in the buffer is on the
    // timeline: `post_at` samples back (the inserts' delay), so a move lines up with what it moves.
    let apos = pos.saturating_sub(i64::try_from(strip.post_at).unwrap_or(0));
    let mute_lane = t.played_lane(&AutoParam::Mute).filter(|l| t.mixer.automation_mode.reads() && !l.points.is_empty());
    let auto_mute = mute_lane.is_some_and(|l| l.value_at(apos, 0.0) >= 0.5);
    strip.muted = t.mixer.mute || auto_mute || solo_muted;
    let pfl = t.mixer.solo && monitor::listen_mode(s) == monitor::Listen::Pfl;
    if strip.pre_key || pfl || s.edit.pre_fader_metering || t.mixer.sends.iter().flatten().any(|x| x.pre_fader) {
        strip.pre.resize_with(strip.buf.len(), Vec::new);
        for (d, src) in strip.pre.iter_mut().zip(strip.buf.iter()) {
            d.clear();
            d.extend_from_slice(src.get(..frames).unwrap_or(&[]));
        }
    }
    let vcas = vca_chain(s, t);
    // VCA masters (a VCA can follow another): each one's mute mutes the member (their faders
    // offset the member's, below).
    let held_mute = t.mixer.mute || solo_muted || vcas.iter().flatten().any(|v| v.mixer.mute);
    strip.muted = strip.muted || held_mute;
    fader_gains(t, &vcas, apos, frames, &mut strip.fader_gain);
    // The mute: from where the last block left it, ramping (never switching) to what the mute,
    // the solo, the VCA and the mute lane ask, sample by sample.
    if !strip.mute_settled {
        strip.mute_gain = if strip.muted { 0.0 } else { 1.0 };
        strip.mute_settled = true;
    }
    let from = strip.mute_gain;
    let (end, open) = mute_gains(mute_lane.filter(|_| !held_mute), held_mute, apos, frames, from, &mut strip.mute_gains);
    // Muted all through: nothing of the strip (not even a send) is heard.
    strip.silent = !open;
    let (fg, mg) = (&strip.fader_gain, &strip.mute_gains);
    for c in strip.buf.iter_mut() {
        if !open {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
            continue;
        }
        for ((x, f), m) in c.iter_mut().take(frames).zip(fg).zip(mg) {
            *x *= f * m;
        }
    }
    // Pre-fader sends, keys and meters skip the fader, not the mute.
    if open && mg.iter().any(|m| *m < 1.0) {
        for c in strip.pre.iter_mut() {
            for (x, m) in c.iter_mut().take(frames).zip(mg) {
                *x *= m;
            }
        }
    }
    strip.mute_gain = end;
}

/// Fills `out` with the fader's linear gain for each of `frames` samples from timeline position
/// `at`: the volume lane (or the fader), the trim lane and the VCA master's fader, capped at
/// +12 dB. A block whose lanes bend or have a breakpoint inside it is read sample by sample, so a
/// breakpoint is honoured wherever it falls and any block size plays the same; otherwise it ramps
/// linearly between its first and last samples. `out` has room for `frames` (reserved in `sync`).
/// How deep VCA masters may follow one another (a VCA on a VCA on …).
pub const MAX_VCA_DEPTH: usize = 8;

/// The VCA masters over `t`: its own, then the one that VCA follows, and so on (at most
/// [`MAX_VCA_DEPTH`], stopping at a loop). Doesn't allocate.
fn vca_chain<'a>(s: &'a Session, t: &Track) -> [Option<&'a Track>; MAX_VCA_DEPTH] {
    let mut out: [Option<&'a Track>; MAX_VCA_DEPTH] = [None; MAX_VCA_DEPTH];
    let mut next = t.mixer.vca;
    for i in 0..MAX_VCA_DEPTH {
        let Some(v) = next.and_then(|vid| s.tracks.iter().find(|x| x.id.0 == vid && x.kind == TrackKind::Vca)) else { break };
        if v.id == t.id || out.iter().take(i).flatten().any(|o| o.id == v.id) {
            break;
        }
        if let Some(slot) = out.get_mut(i) {
            *slot = Some(v);
        }
        next = v.mixer.vca;
    }
    out
}

fn fader_gains(t: &Track, vcas: &[Option<&Track>], at: Samples, frames: usize, out: &mut Vec<f32>) {
    out.clear();
    let last = at.saturating_add(frames.saturating_sub(1) as i64);
    let reads = |tr: &Track| tr.mixer.automation_mode.reads();
    let trim = t.played_lane(&AutoParam::Trim).filter(|l| reads(t) && !l.points.is_empty());
    let vol = t
        .played_lane(&AutoParam::Volume)
        .filter(|l| reads(t) && t.mixer.automation_mode != soundcraft_model::AutomationMode::Write && !l.points.is_empty());
    fn vca_vol(v: &Track) -> Option<&soundcraft_model::AutomationLane> {
        let m = v.mixer.automation_mode;
        v.played_lane(&AutoParam::Volume).filter(|l| m.reads() && m != soundcraft_model::AutomationMode::Write && !l.points.is_empty())
    }
    let db = |p: Samples| {
        let mut d = volume_db_at(t, p);
        if let Some(l) = trim {
            d += l.value_at(p, 0.0);
        }
        for v in vcas.iter().flatten() {
            d += volume_db_at(v, p);
        }
        d.min(12.0)
    };
    let exact = [vol, trim].into_iter().flatten().chain(vcas.iter().flatten().filter_map(|v| vca_vol(v))).any(|l| bends_within(l, at, last));
    if exact {
        out.extend((0..frames).map(|i| db_to_gain(db(at.saturating_add(i as i64)))));
    } else {
        let (g0, g1) = (db_to_gain(db(at)), db_to_gain(db(last)));
        let n = frames.saturating_sub(1).max(1) as f32;
        out.extend((0..frames).map(|i| g0 + (g1 - g0) * (i as f32 / n)));
    }
}

/// Whether `l` changes course inside `[a, b]`: a breakpoint after `a` and before `b`, or a bent
/// segment under it (a straight line between the ends would not follow either).
fn bends_within(l: &soundcraft_model::AutomationLane, a: Samples, b: Samples) -> bool {
    let k = l.points.partition_point(|p| p.at <= a);
    let inside = l.points.get(k).is_some_and(|p| p.at < b);
    let bent = k > 0 && l.points.get(k - 1).is_some_and(|p| p.curve != 0.0) && l.points.get(k).is_some();
    inside || bent
}

/// Fills `out` with the mute's gain for each of `frames` samples from `at`, ramping from `from`
/// towards 0 while muted (`held`, or the mute `lane` at that sample) and 1 while not, over
/// [`MUTE_RAMP`] samples. Returns the gain after the block and whether any sample is open.
fn mute_gains(lane: Option<&soundcraft_model::AutomationLane>, held: bool, at: Samples, frames: usize, from: f32, out: &mut Vec<f32>) -> (f32, bool) {
    out.clear();
    let mut m = from;
    let mut open = false;
    // The lane is a step: walk its points as the samples reach them.
    let (mut on, mut k) = match lane {
        Some(l) => (l.value_at(at, 0.0) >= 0.5, l.points.partition_point(|p| p.at <= at)),
        None => (false, 0),
    };
    for i in 0..frames {
        let here = at.saturating_add(i as i64);
        if let Some(l) = lane {
            while let Some(p) = l.points.get(k).filter(|p| p.at <= here) {
                on = p.value >= 0.5;
                k += 1;
            }
        }
        m = mute_step(m, if held || on { 0.0 } else { 1.0 });
        open |= m > 0.0;
        out.push(m);
    }
    (m, open || (frames == 0 && m > 0.0))
}

/// Samples a mute takes to close or open (5 ms at 48 kHz): short enough to sound like a switch,
/// long enough not to click.
const MUTE_RAMP: f32 = 240.0;

/// The mute gain one sample on from `m`, towards `target` (0 or 1).
fn mute_step(m: f32, target: f32) -> f32 {
    if m < target { (m + 1.0 / MUTE_RAMP).min(target) } else { (m - 1.0 / MUTE_RAMP).max(target) }
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

/// Clip-effect parameters of a clip ([`Clip::effects`]), or None when it has none or they are
/// bypassed. Reads only: safe on the audio thread.
fn clip_fx_params(clip: &Clip) -> Option<impl Iterator<Item = (&str, f32)> + Clone> {
    clip.live_effects().map(|fx| fx.iter().map(|(k, v)| (k.as_str(), *v as f32)))
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
    const EQ: &'static str = "eq_7band";
    const COMP: &'static str = "compressor";
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
    /// Which modules `params` need: the EQ, the compressor.
    fn needs<'a>(params: impl Iterator<Item = (&'a str, f32)> + Clone) -> (bool, bool) {
        (params.clone().any(|(k, _)| k.starts_with("eq.")), params.clone().any(|(k, _)| k.starts_with("comp.")))
    }
    /// Make or retire the modules `params` need or no longer need (each exists only when one of
    /// its parameters is set). Allocates: `MixEngine::sync` calls it when the session changes, never
    /// `render`; a retired module goes to `retired`, not dropped here.
    /// Returns how many modules it made.
    fn prepare_modules<'a>(&mut self, params: impl Iterator<Item = (&'a str, f32)> + Clone, retired: &mut Vec<Box<dyn Plugin>>) -> u64 {
        let (has_eq, has_comp) = Self::needs(params);
        let mut made = 0;
        if has_eq && self.eq.is_none() {
            self.eq = self.make(Self::EQ);
            made += u64::from(self.eq.is_some());
        } else if !has_eq {
            retired.extend(self.eq.take());
        }
        if has_comp && self.comp.is_none() {
            self.comp = self.make(Self::COMP);
            made += u64::from(self.comp.is_some());
        } else if !has_comp {
            retired.extend(self.comp.take());
        }
        made
    }
    /// Seats modules made off the audio thread ([`PreparedClipFx`]) where this chain has none;
    /// the rest are retired. Allocates nothing.
    fn seat(&mut self, p: PreparedClipFx, retired: &mut Vec<Box<dyn Plugin>>) {
        for (have, new) in [(&mut self.eq, p.eq), (&mut self.comp, p.comp)] {
            match (have.is_none(), new) {
                (true, new) => *have = new,
                (false, Some(n)) => retired.push(n),
                (false, None) => {}
            }
        }
    }
    /// Whether the modules match what `params` need (`prepare_modules` has run since they changed).
    fn ready<'a>(&self, params: impl Iterator<Item = (&'a str, f32)> + Clone) -> bool {
        Self::needs(params) == (self.eq.is_some(), self.comp.is_some())
    }
    /// Set the modules' parameters and the gain. Allocates nothing: safe on the audio thread.
    fn configure<'a>(&mut self, params: impl Iterator<Item = (&'a str, f32)> + Clone) {
        self.gain_db = 0.0;
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

/// Clip Effects of a track's clips, made here (the session changed) rather than the first time a
/// clip plays: an effect chain for each clip with live effects and the modules its settings
/// need, and a buffer to render clips into. Chains of clips that lost their effects, and modules
/// no longer needed, go to `retired`.
///
/// With `host` (a realtime engine, [`MixEngine::set_prepared_builtins`]) the modules come made
/// from the control thread; one that did not is made here all the same and counted in `made`.
fn sync_clip_fx(
    strip: &mut Strip,
    t: &Track,
    sr: f32,
    mb: usize,
    retired: &mut Vec<Box<dyn Plugin>>,
    mut host: Option<&mut HashMap<(TrackId, u64), PreparedClipFx>>,
    made: &mut u64,
) {
    let ch = strip.buf.len();
    let live = || t.heard_clips().iter().filter(|c| c.is_audio()).filter_map(|c| clip_fx_params(c).map(|p| (c.id.0, p)));
    strip.clip_fx.retain(|id, fx| {
        let keep = fx.ch == ch && live().any(|(c, _)| c == *id);
        if !keep {
            retired.extend(fx.eq.take().into_iter().chain(fx.comp.take()));
        }
        keep
    });
    let mut any = false;
    for (id, params) in live() {
        any = true;
        let fx = strip.clip_fx.entry(id).or_insert_with(|| ClipFx::new(sr, ch));
        if let Some(p) = host.as_deref_mut().and_then(|a| a.remove(&(t.id, id))) {
            if p.ch == ch {
                fx.seat(p, retired);
            } else {
                retired.extend(p.eq.into_iter().chain(p.comp));
            }
        }
        let n = fx.prepare_modules(params, retired);
        if host.is_some() {
            *made = made.saturating_add(n);
        }
    }
    if any && (strip.clipbuf.len() != ch || strip.clipbuf.iter().any(|c| c.len() < mb)) {
        strip.clipbuf = vec![vec![0.0; mb]; ch];
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
        // Record with inserts sizes the strip's print buffer.
        t.mixer.record_inserts.hash(&mut h);
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
        // Which clips have Clip Effects and which modules they need (`sync` makes them).
        for c in t.heard_clips() {
            if let Some(p) = clip_fx_params(c) {
                (c.id.0, ClipFx::needs(p)).hash(&mut h);
            }
        }
        for snd in t.mixer.sends.iter().flatten() {
            snd.target.hash(&mut h);
            // A pre-fader send needs the pre-fader delay line, which `sync` reserves.
            snd.pre_fader.hash(&mut h);
        }
        t.instrument.as_ref().map(|x| x.plugin.as_str()).hash(&mut h);
        // MIDI effects ahead of it (`sync` configures them).
        t.midi_fx.hash(&mut h);
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
/// Adds `frames` of bus `src` into `dst` (the strip of `t`, `dst.len()` channels wide): as is
/// when the formats match, a mono bus on both sides of a stereo strip, else down- or up-mixed.
fn read_bus(src: &BusBuf, t: &Track, dst: &mut [Vec<f32>], frames: usize, law: PanLaw) {
    let (ns, nd) = (src.buf.len(), dst.len());
    let dst_fmt = strip_format(t, nd);
    let m = if src.fmt == dst_fmt && ns == nd {
        identity_matrix(ns)
    } else if ns == 1 && nd == 2 {
        // A mono bus feeds both sides of a stereo strip.
        let mut m = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
        m[0][0] = 1.0;
        m[0][1] = 1.0;
        m
    } else {
        let stereo = if ns == 1 { [0.0, 0.0] } else { [-1.0, 1.0] };
        build_matrix(src.fmt, ns, dst_fmt, nd, &PanSpec { stereo, surround: None }, law)
    };
    apply_matrix(&src.buf, frames, dst, &m, 1.0);
}

/// The LFE channels of `fmt` as [`Plugin::set_lfe_channels`] takes them (bit `n`: channel `n`).
fn lfe_mask(fmt: ChannelFormat) -> u64 {
    fmt.speakers().iter().take(64).enumerate().filter(|(_, sp)| sp.is_lfe()).fold(0, |m, (i, _)| m | (1u64 << i))
}

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

    /// The hardware output (index in `Session::outputs`, never the main) the click plays on
    /// instead of the main mix: the `click.output=<name>` setting, when it names one. Audio
    /// thread: no allocation.
    pub(crate) fn output(s: &Session) -> Option<usize> {
        let name = s.edit.flags.iter().find_map(|f| f.strip_prefix(OUTPUT_PREFIX))?;
        s.output_index(name).filter(|i| *i > 0)
    }

    /// The setting's prefix in the session's flags (`click.output=<output name>`).
    const OUTPUT_PREFIX: &str = "click.output=";

    /// Click pitch for a MIDI note setting (clamped to the MIDI range).
    fn note_hz(note: f64) -> f64 {
        soundcraft_dsp::midi_to_hz(note.clamp(0.0, 127.0))
    }
}

/// Feedback protection (as in Logic Pro): a monitored hardware input that clips (every block
/// peaks at or above [`FEEDBACK_LEVEL`]) for this long is taken for feedback through the
/// speakers and silenced on its track until its monitoring is turned off and on again. The take
/// records on. Only clipping trips it, so a loud sustained source (an organ, a distorted guitar
/// held at -1 dBFS) is never silenced.
pub const FEEDBACK_SECONDS: f32 = 1.0;
/// The block peak feedback protection counts as clipping: -0.1 dBFS (linear 0.98855).
pub const FEEDBACK_LEVEL: f32 = 0.988_55;
/// The flag that turns feedback protection off (it is on unless this is set).
pub const NO_FEEDBACK_PROTECTION: &str = "options.no_feedback_protection";

/// Feedback protection on a strip that has just taken its monitored input into `buf`: counts
/// the hot frames and, past `limit` (0: off), silences the input from then on. No allocation.
fn feedback_guard(strip: &mut Strip, frames: usize, limit: usize) {
    if limit == 0 {
        (strip.fb_hot, strip.fb_muted) = (0, false);
        return;
    }
    if !strip.fb_muted {
        let peak = strip.buf.iter().flat_map(|c| c.iter().take(frames)).fold(0.0f32, |m, x| m.max(x.abs()));
        strip.fb_hot = if peak >= FEEDBACK_LEVEL { strip.fb_hot.saturating_add(frames) } else { 0 };
        strip.fb_muted = strip.fb_hot >= limit;
    }
    if strip.fb_muted {
        for c in &mut strip.buf {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }
}

/// The flag of Options › Recording › Input Only Monitoring (Pro Tools' global mode): every
/// record-armed track on Auto hears its input during playback too, not only when stopped or
/// recording. A track set to On or Off keeps its own setting.
pub const INPUT_ONLY_MONITORING: &str = "options.input_only_monitoring";

/// The automatic edge fades' length when Options › Automatic Edge Fades has none set, ms.
pub const AUTO_FADE_MS: f64 = 1.0;

/// The automatic edge fades' length in frames at `s`'s rate; 0 when they are off
/// (`playback.auto_fade_ms` 0).
pub fn auto_fade_frames(s: &Session) -> i64 {
    let ms = s.edit.value("playback.auto_fade_ms", AUTO_FADE_MS).clamp(0.0, 100.0);
    (s.sample_rate.as_f64() * ms / 1000.0).round() as i64
}

/// The short ramps a clip's audio gets at edges that would otherwise click: an edge without a
/// fade of its own that faces silence (no clip butting against it: a split plays straight
/// through), and, while looping, the loop's end and start where the clip runs across them.
/// Each ramp is linear over `len` frames, silent on the edge's own frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Edges {
    len: i64,
    start: Option<Samples>,
    end: Option<Samples>,
    loop_in: Option<Samples>,
    loop_out: Option<Samples>,
}

impl Edges {
    /// The edges of `clip` (one of `clips`, its playlist) that `block` comes near. Reads only:
    /// no allocation (safe on the audio thread).
    fn of(s: &Session, clips: &[Clip], clip: &Clip, block: Range, looping: Option<Range>) -> Edges {
        let len = auto_fade_frames(s);
        if len <= 0 {
            return Edges::default();
        }
        let near = |at: Samples| block.start < at.saturating_add(len) && block.end > at.saturating_sub(len);
        let mut e = Edges { len, ..Edges::default() };
        if clip.fade_in.len <= 0 && near(clip.start) && !clips.iter().any(|p| continues(p, clip)) {
            e.start = Some(clip.start);
        }
        if clip.fade_out.len <= 0 && near(clip.end()) && !clips.iter().any(|n| continues(clip, n)) {
            e.end = Some(clip.end());
        }
        if let Some(l) = looping {
            if clip.start < l.start && l.start < clip.end() && near(l.start) {
                e.loop_in = Some(l.start);
            }
            if clip.start < l.end && l.end < clip.end() && near(l.end) {
                e.loop_out = Some(l.end);
            }
        }
        e
    }

    fn is_none(&self) -> bool {
        self.start.is_none() && self.end.is_none() && self.loop_in.is_none() && self.loop_out.is_none()
    }

    /// The ramp's gain at timeline position `p`.
    fn gain(&self, p: Samples) -> f32 {
        let len = self.len.max(1) as f32;
        let up = |from: Option<Samples>| from.filter(|f| p >= *f).map_or(1.0, |f| ((p - f) as f32 / len).min(1.0));
        let down = |to: Option<Samples>| to.filter(|t| p < *t).map_or(1.0, |t| ((t - 1 - p) as f32 / len).min(1.0));
        up(self.start) * up(self.loop_in) * down(self.end) * down(self.loop_out)
    }
}

/// Whether clip `b` starts right where clip `a` ends (a split, a comp edit, a file split in
/// pieces): what plays across that edit point is the user's to fade, and a split plays straight
/// through. No automatic fade goes between them.
fn continues(a: &Clip, b: &Clip) -> bool {
    a.id != b.id && !a.muted && !b.muted && a.is_audio() && b.is_audio() && a.end() == b.start
}

/// Where clips of a higher [`Clip::layer`] cover a clip (Options › Overlap › Layer): the lower
/// clip gives way under them, crossing over through the upper clip's own fades, or through the
/// automatic edge ramp (`len` frames) where an upper edge has no fade and nothing butts against
/// it, the same ramp the upper clip fades in on, so the two sum to one. Reads only (no
/// allocation): safe on the audio thread.
#[derive(Debug, Clone, Copy)]
struct Cover<'a> {
    clips: &'a [Clip],
    layer: i32,
    len: i64,
}

impl<'a> Cover<'a> {
    /// The cover over `clip` (one of `clips`) in `block`, or `None` where nothing above it
    /// overlaps there: every session whose clips all sit on one layer.
    fn of(clips: &'a [Clip], clip: &Clip, block: Range, len: i64) -> Option<Cover<'a>> {
        let above = |u: &Clip| u.layer > clip.layer && !u.muted && u.is_audio() && u.range().overlaps(&block) && u.range().overlaps(&clip.range());
        clips.iter().any(above).then_some(Cover { clips, layer: clip.layer, len: len.max(0) })
    }

    /// How much of the lower clip is heard at timeline position `p`, 0 to 1.
    fn gain(&self, p: Samples) -> f32 {
        let mut keep = 1.0f32;
        for u in self.clips.iter().filter(|u| u.layer > self.layer && !u.muted && u.is_audio() && p >= u.start && p < u.end()) {
            let rel = p - u.start;
            let to_end = u.end() - 1 - p;
            let ramp = |n: i64| if self.len <= 0 { 1.0 } else { (n as f32 / self.len as f32).min(1.0) };
            let mut c = 1.0f32;
            if u.fade_in.len > 0 {
                if rel < u.fade_in.len {
                    c = c.min(u.fade_in.gain(rel as f32 / u.fade_in.len as f32));
                }
            } else if !self.clips.iter().any(|x| continues(x, u)) {
                c = c.min(ramp(rel));
            }
            let from_end = u.length.saturating_sub(rel);
            if u.fade_out.len > 0 {
                if from_end <= u.fade_out.len {
                    c = c.min(u.fade_out.gain(from_end as f32 / u.fade_out.len as f32));
                }
            } else if !self.clips.iter().any(|x| continues(u, x)) {
                c = c.min(ramp(to_end));
            }
            keep = keep.min(1.0 - c.clamp(0.0, 1.0));
        }
        keep
    }
}

fn render_clip(s: &Session, clip: &Clip, pos: Samples, frames: usize, buf: &mut [Vec<f32>], auto: Edges, cover: Option<Cover<'_>>) {
    let ClipContent::Audio { source, offset } = clip.content else { return };
    let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
    let Some(isect) = clip.range().intersect(&block) else { return };
    // A stretch whose render is still being made: the original, through the real-time
    // stretcher ([`live_stretch_at`]).
    let live = live_stretch(s, source);
    let Some(audio) = s.pool.get(live.map_or(source, |l| l.source)) else { return };
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
    // A transposed clip's high-quality render, once made, plays in place of the live shifter.
    let hq = pitch_render(s, clip).filter(|_| live.is_none());
    // Clean Vocal's cleaned alternate, mixed in sample for sample before gain and fades (a
    // transpose render has it mixed in already).
    let alt = clip
        .clean_alt()
        .filter(|k| k.wet() > 0.0 && live.is_none() && hq.is_none())
        .and_then(|k| s.pool.get(k.source).map(|a| (k.wet(), k.origin, a)));
    let edges = alt.map(|(_, origin, a)| CleanEdges::new(s, clip, origin, a.frames()));
    // Vocal tuning's render replaces the heard audio inside its spans (looked up once per block).
    // A plain clip reads its window in runs; a stretched, reversed, looped or transposed one
    // sample by sample through `Clip::material_at`.
    let unstretched = (stretch - 1.0).abs() < 1e-9 && clip.plays_plain() && live.is_none();
    let pitch = clip.pitch_ratio();
    let grain = (s.sample_rate.as_f64() * TRANSPOSE_GRAIN_SECS).max(64.0);
    let live_grain = (s.sample_rate.as_f64() * soundcraft_dsp::live_stretch::GRAIN_SECS).max(64.0);
    let tuned = tuned_spans(s, clip);
    // A stereo source on a mono track folds to mono; a chosen source channel plays alone.
    let (taps, tap) = clip.channel_taps(nsrc, buf.len());
    for which in 0..taps {
        let (dch, ch, weight) = tap(which);
        let Some(dst) = buf.get_mut(dch) else { continue };
        let mut src = audio.cursor(ch, rt);
        // The render holds the chosen channel alone, or every channel.
        let mut hq_src = hq.map(|r| (r.start as f64, r.audio.cursor(if r.audio.num_channels() == 1 { 0 } else { ch }, rt)));
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
            let mut g0 = clip.gain_at(rel0);
            let mut g1 = clip.gain_at(rel1.min(clip.length));
            if !auto.is_none() {
                g0 *= auto.gain(clip.start.saturating_add(rel0));
                g1 *= auto.gain(clip.start.saturating_add(rel1));
            }
            if let Some(c) = cover {
                g0 *= c.gain(clip.start.saturating_add(rel0));
                g1 *= c.gain(clip.start.saturating_add(rel1));
            }
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
                    // A varispeed clip reads its file at `1 / stretch` file samples a sample,
                    // band-limited (the sampler's reader): a sped-up clip no longer aliases.
                    let f = clip.material_at(rel);
                    let v = match (live, hq_src.as_mut()) {
                        (Some(l), _) => live_stretch_at(&mut src, l, f, pitch, live_grain).unwrap_or(0.0),
                        (None, Some((start, r))) => band_limited_at(r, f - *start, 1.0 / stretch).unwrap_or(0.0),
                        (None, None) => shifted_at(&mut src, f, rel, stretch, pitch, grain).unwrap_or(0.0),
                    };
                    match cleaned.as_mut().and_then(|(w, origin, c)| shifted_at(c, f - *origin as f64, rel, stretch, pitch, grain).map(|cv| (*w, cv)))
                    {
                        Some((w, cv)) => blend(v, cv, edges.map_or(w, |e| e.weight(w, f))),
                        None => v,
                    }
                };
                let t = (k - i) as f32 / n;
                if let Some(d) = dst.get_mut(k) {
                    *d += v * weight * (g0 + (g1 - g0) * t);
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
/// source with Clean Vocal blended in), which the tuner analyses and renders from. A clip with a
/// [`Clip::source_channel`] gives that one channel.
pub fn clip_heard_window(s: &Session, clip: &Clip, start: Samples, n: usize) -> Option<Vec<Vec<f32>>> {
    let ClipContent::Audio { source, .. } = clip.content else { return None };
    if let Some(l) = live_stretch(s, source) {
        // A stretch still being rendered: the render positions asked, through the stretcher.
        let audio = s.pool.get(l.source)?;
        let nsrc = audio.num_channels();
        let grain = (s.sample_rate.as_f64() * soundcraft_dsp::live_stretch::GRAIN_SECS).max(64.0);
        let chans = match clip.source_channel.map(usize::from).filter(|c| *c < nsrc) {
            Some(c) => c..c + 1,
            None => 0..nsrc,
        };
        return Some(
            chans
                .map(|ch| {
                    let mut src = audio.cursor(ch, false);
                    (0..n).map(|k| live_stretch_at(&mut src, l, start.saturating_add(k as i64) as f64, 1.0, grain).unwrap_or(0.0)).collect()
                })
                .collect(),
        );
    }
    let audio = s.pool.get(source)?;
    let alt = clip.clean_alt().filter(|k| k.wet() > 0.0).and_then(|k| s.pool.get(k.source).map(|a| (k.wet(), k.origin, a)));
    let nalt = alt.map_or(0, |(_, _, a)| a.num_channels());
    let edges = alt.map(|(_, origin, a)| CleanEdges::new(s, clip, origin, a.frames()));
    // A clip playing one chosen source channel (Split into Mono) renders only that channel.
    let nsrc = audio.num_channels();
    let chans = match clip.source_channel.map(usize::from).filter(|c| *c < nsrc) {
        Some(c) => c..c + 1,
        None => 0..nsrc,
    };
    Some(
        chans
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

/// The high-quality render of `clip`'s transpose, when one serves it as it is now
/// ([`soundcraft_model::PitchRender::serves`]). Reads only: safe on the audio thread.
pub fn pitch_render<'a>(s: &'a Session, clip: &Clip) -> Option<&'a soundcraft_model::PitchRender> {
    if clip.transpose_cents == 0 || s.pitch_renders.0.is_empty() {
        return None;
    }
    s.pitch_renders.0.get(&clip.id).map(|r| &**r).filter(|r| r.serves(clip))
}

/// The live stretch standing in for `source` while its render is made: only while the pool has
/// no audio for it. Reads only: safe on the audio thread.
pub fn live_stretch(s: &Session, source: soundcraft_model::SourceId) -> Option<&soundcraft_model::LiveStretch> {
    if s.live_stretch.is_empty() || s.pool.get(source).is_some() {
        return None;
    }
    s.live_stretch.get(&source)
}

/// Render position `q` of a live stretch, read from its original through the real-time
/// stretcher ([`soundcraft_dsp::live_stretch`]): two grains of `grain` samples read at the
/// file's speed (times `pitch`, a clip transpose), placed by the stretch map. Never allocates.
fn live_stretch_at(src: &mut soundcraft_model::SampleCursor<'_>, l: &soundcraft_model::LiveStretch, q: f64, pitch: f64, grain: f64) -> Option<f32> {
    use soundcraft_dsp::live_stretch::{map_at, stretch_at};
    stretch_at(|at, step| band_limited_at(src, at, step), |x| map_at(&l.map, x), q, grain, pitch)
}

/// `src` at the fractional position `f`, read `step` file samples a sample: a cubic at or below
/// the file's own speed, a windowed sinc that filters what would fold back above it
/// ([`soundcraft_dsp::read_band_limited`]). `None` outside the audio (the sample after the last
/// counts as silence, as the plain read does). Never allocates.
fn band_limited_at(src: &mut soundcraft_model::SampleCursor<'_>, f: f64, step: f64) -> Option<f32> {
    soundcraft_dsp::read_band_limited(|i| src.get(i), f, step)
}

/// A transposed clip's grain, in seconds.
const TRANSPOSE_GRAIN_SECS: f64 = 0.03;

/// `src` read for timeline sample `rel` of a clip at file position `f` (stretch `stretch`),
/// transposed by `pitch` without moving in time: two grains half a grain apart read the file
/// `pitch` times as fast as the clip moves, each centred on `f` as it sweeps and faded in and
/// out (sin², summing to one), the delay-line pitch shifter with the file as its line. Every
/// sample depends only on `rel`, so a locate or loop wrap plays exactly what playing through
/// would. Not transposed, the band-limited read of `f`. Never allocates.
fn shifted_at(src: &mut soundcraft_model::SampleCursor<'_>, f: f64, rel: Samples, stretch: f64, pitch: f64, grain: f64) -> Option<f32> {
    let step = pitch / stretch;
    if (pitch - 1.0).abs() < 1e-9 {
        return band_limited_at(src, f, step);
    }
    // Each grain's offset from `f` moves by `pitch − 1` file samples a sample (in the clip's
    // time), from −grain/2 to +grain/2.
    let sweep = rel as f64 * (pitch - 1.0) / stretch / grain;
    let mut out = None;
    for k in [0.0, 0.5] {
        let phase = (sweep + k).rem_euclid(1.0);
        let g = (std::f64::consts::PI * phase).sin().powi(2) as f32;
        let at = f + (phase - 0.5) * grain;
        if let Some(v) = band_limited_at(src, at, step) {
            out = Some(out.unwrap_or(0.0) + g * v);
        }
    }
    out
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
/// The playback quantize `clip` on `t` plays with: its own, else its track's (the Real-Time
/// Properties `quantize` grid in ticks, `q_strength` and `q_swing` in percent). No allocation.
pub fn play_quantize<'a>(
    s: &Session,
    t: &Track,
    clip: &'a soundcraft_model::Clip,
) -> Option<std::borrow::Cow<'a, soundcraft_midi::ops::PlayQuantize>> {
    if let Some(q) = &clip.play_quantize {
        return Some(std::borrow::Cow::Borrowed(q));
    }
    if s.edit.values.is_empty() {
        return None;
    }
    let rtp = |name: &str, d: f64| Key::new(format_args!("rtp.{}.{name}", t.id.0)).map_or(d, |k| s.edit.value(k.as_str(), d));
    let grid = rtp("quantize", 0.0).clamp(0.0, 15_360.0) as i64;
    (grid > 0).then(|| {
        std::borrow::Cow::Owned(soundcraft_midi::ops::PlayQuantize {
            grid,
            strength: (rtp("q_strength", 100.0).clamp(0.0, 100.0) / 100.0) as f32,
            swing: (rtp("q_swing", 0.0).clamp(0.0, 100.0) / 100.0) as f32,
            groove: None,
        })
    })
}

/// Where a note `start` ticks into a clip starting on tick `base` plays under `q`: its grid
/// counted from each bar, never before the clip's start (a hidden note, before it, is left).
pub fn quantized_start(s: &Session, base: i64, q: &soundcraft_midi::ops::PlayQuantize, start: i64) -> i64 {
    if start < 0 {
        return start;
    }
    q.start(start, bar_around(s, base, start)).max(0)
}

/// The bar holding `start` ticks into a clip starting on tick `base`: its start and the next
/// bar's, in ticks from the clip's start.
fn bar_around(s: &Session, base: i64, start: i64) -> (i64, i64) {
    let bar = s.tempo.bar_beat_at_tick(base.saturating_add(start)).bar;
    (s.tempo.bar_start_tick(bar).saturating_sub(base), s.tempo.bar_start_tick(bar.saturating_add(1)).saturating_sub(base))
}

/// A note as `q` plays it ([`quantized_start`]), its velocity moved by the accent of its groove
/// point, if the groove has accents. No allocation.
pub fn quantize_note(s: &Session, base: i64, q: &soundcraft_midi::ops::PlayQuantize, n: &mut soundcraft_midi::Note) {
    if q.accents() && n.start >= 0 {
        n.velocity = q.velocity(n.start, bar_around(s, base, n.start), n.velocity);
    }
    n.start = quantized_start(s, base, q, n.start);
}

/// `chase`: playback starts here (a start or a locate) and notes already sounding at `pos`
/// start now (Note Chase).
#[allow(clippy::too_many_arguments)]
fn schedule_notes(s: &Session, t: &Track, pos: Samples, frames: usize, inst: &mut PluginSlot, held: &mut Vec<u8>, chase: bool, pass: u64) {
    let sr = s.sample_rate;
    // MIDI Track Offsets: the track's notes sound `offset` samples later (earlier if negative), so
    // the block looks that far back into the clips.
    let offset = midi_offset(s, t);
    let pos = pos.saturating_sub(offset);
    let end = pos.saturating_add(frames as i64);
    // Pitches some note sounds on in this block (started before its end, not ended before it).
    let mut alive = [false; 128];
    // A clip that plays on its channels (muted or not, in this block or not) may have left a note on any.
    let printed = || t.clips().iter().any(plays_channels);
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
    let any_printed = t.clips().iter().any(|c| c.midi_fx_printed);
    for clip in t.clips() {
        if clip.muted {
            continue;
        }
        let Some(sequence) = clip.midi_playing() else { continue };
        // A clip that holds its MIDI effects' output plays past them.
        if any_printed {
            midi_fx::set_direct(&mut *inst.plugin, clip.midi_fx_printed);
        }
        // The clip's own transpose and velocity offset, with the track's.
        let (dv, dp) = (dv + i64::from(clip.midi_velocity), dp + i64::from(clip.midi_transpose));
        // Through the clip's end: a note-off clamped to it lands there, and when that is a block
        // boundary it plays in the block that starts on it. A negative delay plays the clip's
        // first notes before its start.
        if (clip.start >= end && delay >= 0) || clip.end() < pos {
            continue;
        }
        let base = s.tempo.samples_to_ticks(clip.start, sr);
        if delay < 0 && s.tempo.tick_to_samples(base.saturating_add(delay), sr).min(clip.start) >= end {
            continue;
        }
        // A printed band or recorded clip, or one with events off channel 1, plays each event on its
        // own channel ([`plays_channels`]).
        let chans = plays_channels(clip);
        // Where an event `tick` ticks into the clip sounds. The clip's start need not fall on a
        // tick: an event in the clip is never placed before it (and skipped as playback starts
        // there); one a negative delay moves before the start plays before it.
        let at_tick = |tick: i64| {
            let at = s.tempo.tick_to_samples(base.saturating_add(tick), sr);
            if tick >= 0 { at.max(clip.start) } else { at }
        };
        let near = s.tempo.samples_to_ticks(pos, sr).saturating_sub(base).saturating_sub(1)
            ..=s.tempo.samples_to_ticks(end, sr).saturating_sub(base).saturating_add(1);
        // Events before tick 0 are what a start trim hid: they don't play in place, but the state
        // they leave (a program, the pedal) is sent as the clip starts, in their order, before
        // its first notes.
        if clip.start >= pos && clip.start < end {
            let offset = usize::try_from(clip.start - pos).unwrap_or(0);
            for c in sequence.ctrls.iter().take_while(|c| c.tick < 0) {
                let mut m = ctrl_message(c.kind, c.value, dp);
                if chans {
                    m[0] |= c.channel & 0x0F;
                }
                send_ctrl(inst, offset, m, chans, t.articulations.as_ref());
            }
        }
        // The controllers that can send something in this block: with the clip's index, those in
        // `near` (less the delay) and ramp points whose ramp can reach into it; every one without.
        let ctrl_range = soundcraft_midi::NoteIndex::ctrl_window(
            clip.midi_index.0.as_deref(),
            &sequence.ctrls,
            near.start().saturating_sub(delay).saturating_sub(2),
            near.end().saturating_sub(delay).saturating_add(2),
        );
        let ctrls = |inst: &mut PluginSlot| {
            let first = ctrl_range.start;
            for (k, c) in sequence.ctrls.get(ctrl_range.clone()).unwrap_or(&[]).iter().enumerate() {
                let i = first + k;
                let tick = c.tick.saturating_add(delay);
                let at = if c.tick >= 0 && near.contains(&tick) { at_tick(tick) } else { end };
                if at >= pos && at < end && at < clip.end() {
                    let offset = usize::try_from(at - pos).unwrap_or(0);
                    let mut m = ctrl_message(c.kind, c.value, dp);
                    if chans {
                        m[0] |= c.channel & 0x0F;
                    }
                    send_ctrl(inst, offset, m, chans, t.articulations.as_ref());
                }
                // A ramp to the next point of its lane plays on as a stream of values (not the
                // articulation lane: it switches).
                if !c.shape.is_step() && !is_articulation(t, c.kind) {
                    let span = (clip.start, clip.end(), pos, end);
                    send_ramp(inst, sequence, i, &|t: i64| at_tick(t.saturating_add(delay)), span, chans, dp);
                }
            }
        };
        // A printed clip's controllers go first, so one that sets up a note on the same tick
        // (an articulation, a sustain) arrives before it; the instrument keeps equal times in order.
        if chans {
            ctrls(inst);
        }
        // A note as it plays (playback quantize and Real-Time Properties applied), with where it
        // starts and ends.
        let pq = play_quantize(s, t, clip);
        let shaped = |n0: &soundcraft_midi::Note| {
            let mut n = *n0;
            if let Some(q) = &pq {
                quantize_note(s, base, q, &mut n);
            }
            n.velocity = (i64::from(n.velocity) + dv).clamp(1, 127) as u8;
            // The global transpose in force where the note is written.
            let global = if s.transpose_track.is_empty() { 0 } else { s.global_transpose(t, at_tick(n0.start)) };
            n.pitch = (i64::from(n.pitch) + dp + global).clamp(0, 127) as u8;
            n.length = ((n.length as f64 * dur) as i64).max(1);
            n.start = n.start.saturating_add(delay);
            let on = at_tick(n.start);
            let off = at_tick(n.start.saturating_add(n.length)).min(clip.end()).max(on);
            (n, on, off)
        };
        // The notes that can sound in `a..=b` (samples), by their written starts: playback quantize
        // moves a start by up to a grid step or a groove's period, the delay moves it, and a note
        // lasts at most the longest. The clip's index finds them by binary search; without one
        // (or for other notes than it was made from) every note is looked at, as before.
        let index = soundcraft_midi::NoteIndex::usable(clip.midi_index.0.as_deref(), &sequence.notes);
        let longest = index.map_or(0, |ix| ((ix.longest() as f64 * dur) as i64).max(1));
        let qmove = pq.as_ref().map_or(0, |q| q.groove.as_ref().map_or(q.grid, |g| g.period).max(0));
        let near_notes = |a: Samples, b: Samples| {
            let from = s.tempo.samples_to_ticks(a, sr).saturating_sub(base).saturating_sub(delay).saturating_sub(longest).saturating_sub(qmove);
            let to = s.tempo.samples_to_ticks(b, sr).saturating_sub(base).saturating_sub(delay).saturating_add(qmove);
            let r = soundcraft_midi::NoteIndex::window(index, &sequence.notes, from.saturating_sub(2), to.saturating_add(2));
            sequence.notes.get(r).unwrap_or(&[])
        };
        // Another note of this pitch (and channel, when channels are kept) of the clip sounding
        // across `at`: started before it, ending after it. Overlapping notes of one pitch share one
        // voice: a note starting over it retriggers it (its note-off first, so every note-on has
        // its own note-off), and only the last to end closes it.
        let sounding_across = |me: &soundcraft_midi::Note, pitch: u8, ch: u8, at: Samples| {
            near_notes(at, at).iter().filter(|m| m.start >= 0 && !m.muted && m.plays_on(clip.id.0, pass) && !std::ptr::eq(*m, me)).any(|m| {
                let (x, on, off) = shaped(m);
                x.pitch == pitch && (!chans || x.channel & 0x0F == ch) && on < at && off > at && on < clip.end()
            })
        };
        // Notes before tick 0 are what a start trim hid: they don't play; nor do muted notes.
        // A note with a probability plays on this pass or not, as its roll says.
        for n0 in near_notes(pos, end).iter().filter(|n| n.start >= 0 && !n.muted && n.plays_on(clip.id.0, pass)) {
            let (n, on, off) = shaped(n0);
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
            // A note sounding across where playback starts is started there with Note Chase.
            let chased = chase && on < pos && off > pos;
            if (on >= pos && on < end) || chased {
                let offset = usize::try_from(on - pos).unwrap_or(0);
                let retrigger = sounding_across(n0, n.pitch, ch, on);
                inst.hear([0x90, n.pitch, n.velocity]);
                if chans {
                    if retrigger {
                        inst.plugin.midi_raw(offset, [0x80 | ch, n.pitch & 0x7F, 0]);
                    }
                    inst.plugin.midi_raw(offset, [0x90 | ch, n.pitch & 0x7F, n.velocity]);
                } else {
                    if retrigger {
                        inst.plugin.note_off(offset, n.pitch);
                    }
                    inst.plugin.note_on(offset, n.pitch, n.velocity);
                }
                if !held.contains(&n.pitch) && held.len() < 128 {
                    held.push(n.pitch);
                }
            }
            if off >= pos && off < end && !sounding_across(n0, n.pitch, ch, off) {
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
    if any_printed {
        midi_fx::set_direct(&mut *inst.plugin, false);
    }
    let sounds = |p: u8| alive.get(usize::from(p & 0x7F)).copied().unwrap_or(false);
    if held.iter().any(|p| !sounds(*p)) {
        let printed = printed();
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
            let printed = s.tracks.iter().find(|t| t.id == id).is_some_and(|t| t.clips().iter().any(plays_channels));
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
        if f.matches(midi, e)
            && let Some(msg) = f.transform.apply(e.msg)
        {
            held.track(e.port, msg);
            inst.hear(msg);
            inst.plugin.midi_raw(e.offset as usize, msg);
        }
    }
}

fn deliver_module_midi(events: &StripMidi, inst: &mut PluginSlot, held: &mut Vec<(u8, u8, u8)>) {
    for e in events.events() {
        inst.hear(e.msg);
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

/// How often a controller ramp sends a value, in samples (under a millisecond at 44.1 kHz and up).
pub const RAMP_STEP: i64 = 32;

/// The values a ramp from `sequence.ctrls[i]` to the next event of its lane and channel sends in
/// the block `pos..end` (`span`: the clip's start and end, then the block's): one every
/// [`RAMP_STEP`] samples counted from the ramp's start, where the value changes, inside the clip.
/// The two points themselves go out as events. `at` places a tick. Audio thread: no allocation.
fn send_ramp(
    inst: &mut PluginSlot,
    sequence: &soundcraft_midi::Sequence,
    i: usize,
    at: &dyn Fn(i64) -> Samples,
    (clip_start, clip_end, pos, end): (Samples, Samples, Samples, Samples),
    chans: bool,
    transpose: i64,
) {
    let Some(a) = sequence.ctrls.get(i) else { return };
    let a_at = at(a.tick);
    if a_at >= end {
        return;
    }
    let Some(b) = sequence.next_in_lane(i) else { return };
    let b_at = at(b.tick);
    if b_at <= pos || b_at <= a_at {
        return;
    }
    let len = (b_at - a_at) as f64;
    let value = |m: i64| if m <= 0 { a.value } else { a.shape.value(a.value, b.value, (m * RAMP_STEP) as f64 / len) };
    let first = ((pos - a_at).max(0) + RAMP_STEP - 1) / RAMP_STEP;
    let mut m = first.max(1);
    loop {
        let x = a_at.saturating_add(m.saturating_mul(RAMP_STEP));
        if x >= end || x >= b_at || x >= clip_end {
            break;
        }
        let v = value(m);
        // The end value is the next point's own, sent at its time.
        if x >= clip_start && x >= pos && v != value(m - 1) && v != b.value {
            let offset = usize::try_from(x - pos).unwrap_or(0);
            let mut msg = ctrl_message(a.kind, v, transpose);
            if chans {
                msg[0] |= a.channel & 0x0F;
                inst.plugin.midi_raw(offset, msg);
            } else {
                inst.plugin.midi(offset, msg);
            }
        }
        m += 1;
    }
}

/// A controller event as a MIDI 1.0 message on channel 1 (where the notes go), with `transpose`
/// applied to a poly-pressure key.
/// A clip's controller to the instrument (raw: on its own channel), or, on the track's
/// articulation lane ([`soundcraft_model::Articulations`]), the keyswitch of the articulation its
/// value picks: a note-on and its note-off at the same offset (a value no articulation has sends
/// nothing).
fn send_ctrl(inst: &mut PluginSlot, offset: usize, m: [u8; 3], raw: bool, art: Option<&soundcraft_model::Articulations>) {
    if let Some(a) = art
        && m[0] & 0xF0 == 0xB0
        && m[1] == a.cc
    {
        if let Some(k) = a.items.get(usize::from(m[2])) {
            let ch = if raw { m[0] & 0x0F } else { 0 };
            inst.plugin.midi_raw(offset, [0x90 | ch, k.keyswitch & 0x7F, 100]);
            inst.plugin.midi_raw(offset, [0x80 | ch, k.keyswitch & 0x7F, 0]);
        }
        return;
    }
    if raw {
        inst.plugin.midi_raw(offset, m);
    } else {
        inst.plugin.midi(offset, m);
    }
}

/// Whether `kind` is `t`'s articulation lane.
fn is_articulation(t: &Track, kind: CtrlKind) -> bool {
    t.articulations.as_ref().is_some_and(|a| kind == CtrlKind::Cc(a.cc))
}

pub(crate) fn ctrl_message(kind: CtrlKind, value: i32, transpose: i64) -> [u8; 3] {
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

/// Whether a clip plays each event on its own channel: a printed band or recorded clip
/// (`Clip::midi_channels`), or one with a note or controller set off channel 1 (the editor's
/// Channel field, a multi-channel file). Every other clip plays on channel 1 through `note_on`,
/// `note_off` and `midi`, exactly as it always has. No allocation.
fn plays_channels(c: &Clip) -> bool {
    c.midi_channels
        || c.midi_playing().is_some_and(|q| q.notes.iter().any(|n| n.channel & 0x0F != 0) || q.ctrls.iter().any(|e| e.channel & 0x0F != 0))
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
    for clip in t.clips().iter().filter(|c| plays_channels(c) && !c.muted) {
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
    // RPN and NRPN: selected, then set (see `rpn`), read in the clips' order.
    let mut params = rpn::ParamChase::default();
    let sr = s.sample_rate;
    for clip in t.clips() {
        let Some(sequence) = clip.midi_playing() else { continue };
        if clip.muted || clip.start >= pos || plays_channels(clip) != ch.is_some() {
            continue;
        }
        let base = s.tempo.samples_to_ticks(clip.start, sr);
        let until = pos.min(clip.end());
        // Events a start trim hid (before tick 0) count as set when the clip starts.
        for (i, c) in sequence.ctrls.iter().enumerate().filter(|(_, c)| ch.is_none_or(|ch| c.channel & 0x0F == ch)) {
            let slot = match c.kind {
                CtrlKind::Cc(n) if rpn::is_param_cc(n & 0x7F) => {
                    if s.tempo.tick_to_samples(base.saturating_add(c.tick), sr).max(clip.start) < until {
                        params.cc(n & 0x7F, c.value.clamp(0, 127) as u8);
                    }
                    continue;
                }
                CtrlKind::Cc(n) => usize::from(n & 0x7F),
                CtrlKind::PitchBend => BEND,
                CtrlKind::ChannelPressure => PRESSURE,
                CtrlKind::Program => PROGRAM,
                CtrlKind::PolyPressure(_) => continue,
            };
            let at = s.tempo.tick_to_samples(base.saturating_add(c.tick), sr).max(clip.start);
            if at < until
                && let Some(l) = last.get_mut(slot)
                && at >= l.0
            {
                // Inside a ramp: the value it has reached (at its last step before `until`).
                let mut value = c.value;
                if !c.shape.is_step()
                    && let Some(b) = sequence.next_in_lane(i)
                {
                    let b_at = s.tempo.tick_to_samples(base.saturating_add(b.tick), sr);
                    if b_at > at {
                        let m = (until.saturating_sub(1).min(b_at) - at) / RAMP_STEP;
                        value = c.shape.value(c.value, b.value, (m * RAMP_STEP) as f64 / (b_at - at) as f64);
                    }
                }
                *l = (at, value);
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
            if let Some(ch) = ch {
                m[0] |= ch;
            }
            send_ctrl(inst, 0, m, ch.is_some(), t.articulations.as_ref());
        }
    }
    params.send(|mut m| match ch {
        None => inst.plugin.midi(0, m),
        Some(ch) => {
            m[0] |= ch;
            inst.plugin.midi_raw(0, m);
        }
    });
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
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// The pool audio a handed-over sample wraps (`None`: not a pool sample).
fn pool_audio(a: &Arc<dyn SampleSource>) -> Option<*const SourceAudio> {
    a.as_any().and_then(|x| x.downcast_ref::<PoolSample>()).map(|p| Arc::as_ptr(&p.0))
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
fn sync_sample(strip: &mut Strip, want: Option<&soundcraft_model::Insert>, s: &Session, retired: &mut Vec<Arc<dyn SampleSource>>) {
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
        // The old sound (and with it the old audio, if the session let go of it) is dropped off
        // the audio thread.
        while let Some(a) = inst.plugin.take_released_asset() {
            retired.push(a);
        }
    }
    strip.sample = audio.cloned();
}

/// Hands a strip's instrument its keyed sample sources (`Insert::samples`: Juke's pad samples)
/// where they changed: a new source, a cleared one, a removed key, or a new instance. The slot
/// keeps its own reference to each; the ones it replaces, and those the plugin let go of, go to
/// `retired` (dropped off the audio thread). Missing media plays nothing.
fn sync_kit_samples(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, s: &Session, retired: &mut Vec<Arc<dyn SampleSource>>) {
    for (key, sid) in &ins.samples {
        let want = s.pool.get(*sid);
        let at = slot.assets.iter().position(|(k, _)| k == key);
        match (want, at) {
            (Some(w), Some(i)) if slot.assets.get(i).and_then(|(_, a)| pool_audio(a)) == Some(Arc::as_ptr(w)) => {}
            (Some(w), at) => {
                let shared: Arc<dyn SampleSource> = Arc::new(PoolSample(Arc::clone(w)));
                slot.plugin.set_asset(key, Some(Arc::clone(&shared)));
                match at.and_then(|i| slot.assets.get_mut(i)) {
                    Some(entry) => retired.push(std::mem::replace(&mut entry.1, shared)),
                    None => slot.assets.push((key.clone(), shared)),
                }
            }
            (None, Some(i)) => {
                slot.plugin.set_asset(key, None);
                retired.push(slot.assets.remove(i).1);
            }
            (None, None) => {}
        }
    }
    let mut i = 0;
    while i < slot.assets.len() {
        if slot.assets.get(i).is_some_and(|(k, _)| ins.samples.contains_key(k)) {
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

/// Hands a kit instrument the effects its buses host (`Insert::kit_fx`): a new instance where a
/// slot's plugin changed, none where a slot emptied or its effect is bypassed, and every hosted
/// effect its parameters. What the plugin hands back goes to `retired` (dropped off the audio
/// thread).
fn sync_kit_fx(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, sr: f32, mb: usize, offline: bool, retired: &mut Vec<Box<dyn Plugin>>) {
    use soundcraft_model::{KitBus, KitFx};
    if ins.kit_fx.is_empty() && slot.kit_fx.iter().all(Option::is_none) {
        return;
    }
    let n = KitBus::ALL.len() * KitFx::SLOTS;
    slot.kit_fx.resize(n, None);
    for i in 0..n {
        let want = ins
            .kit_fx
            .iter()
            .find(|f| KitFx::index(f.bus, usize::from(f.slot)) == i)
            .filter(|f| f.insert.active && !f.insert.bypass && !is_third_party(&f.insert.plugin));
        let have = slot.kit_fx.get(i).cloned().flatten();
        match want {
            Some(f) if have.as_deref() == Some(f.insert.plugin.as_str()) => {}
            Some(f) => {
                let made = create_instance_for(&f.insert.plugin, None, sr, mb, 2, offline).map(|n| n.plugin);
                let accepted = made.is_some();
                if let Some(old) = slot.plugin.set_child(i, made) {
                    retired.push(old);
                }
                let hosted = accepted && slot.plugin.child_mut(i).is_some();
                if let Some(at) = slot.kit_fx.get_mut(i) {
                    *at = hosted.then(|| f.insert.plugin.clone());
                }
            }
            None if have.is_some() => {
                if let Some(old) = slot.plugin.set_child(i, None) {
                    retired.push(old);
                }
                if let Some(at) = slot.kit_fx.get_mut(i) {
                    *at = None;
                }
            }
            None => {}
        }
        if let (Some(f), Some(c)) = (want, slot.plugin.child_mut(i)) {
            for (k, v) in &f.insert.params {
                c.set_param(k, *v);
            }
        }
    }
}

/// Record insert latency without processing audio, so this block can compensate for it.
/// A late strip's module MIDI inputs (`Plan::late`): this block's events of each source, rendered
/// earlier in the block (nothing from one that did not render in `block`), merged.
fn late_midi_in(st: &mut Strip, s: &Session, strips: &HashMap<TrackId, Strip>, block: u64) {
    st.midi_in.clear();
    for &(k, si, m) in &st.midi_src {
        if let Some(src) = s.tracks.get(si).and_then(|x| strips.get(&x.id)).filter(|x| x.stamp == block)
            && let Some(out) = src.module_midi(m)
        {
            st.midi_in.add(k, out);
        }
    }
    st.midi_in.sort();
}

/// Aligns a late strip (`Plan::late`) to `lmax` (its pre-fader copy too) with no extra output
/// delay. False when a delay line needs more room than it has.
fn align_late(t: &Track, st: &mut Strip, lmax: usize, xf: usize, frames: usize) -> bool {
    let ch = st.buf.len();
    let mut fits = st.align.set(lmax.saturating_sub(st.latency), ch, xf);
    st.align.process(&mut st.buf, frames);
    st.post_at = st.latency.max(lmax);
    if t.mixer.sends.iter().flatten().any(|x| x.pre_fader) {
        let pre_lat = st.latency.saturating_sub(st.post_lat);
        fits &= st.align_pre.set(lmax.saturating_sub(pre_lat), ch, xf);
        st.align_pre.process(&mut st.pre, frames);
        st.pre_at = pre_lat.max(lmax);
    }
    fits & st.out_extra.set(0, ch, xf)
}

fn note_insert_latency(t: &Track, strip: &mut Strip, pos: Samples) {
    let mut latency = 0usize;
    for (i, ins) in t.mixer.inserts.iter().enumerate() {
        let Some(Some(slot)) = strip.plugins.get_mut(i) else { continue };
        // A bypassed module keeps its latency (`insert_step`).
        if slot.module != ins.id || !ins.active || slot.failed {
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
    let mix = if ins.mix.is_finite() { ins.mix } else { 1.0 };
    // Its lane, when one plays (read where the module's input is, as its parameters are).
    let mix = match module.and_then(|m| plugin_lane(t, m, soundcraft_model::MIX_PARAM)) {
        Some(l) => l.value_at(pos, mix),
        None => mix,
    };
    slot.wet_to = if mix.is_finite() { mix.clamp(0.0, 1.0) } else { 1.0 };
    // Automation lanes are keyed by module id (the instrument's is `INSTRUMENT_MODULE`); `None`
    // reads no lanes.
    let Some(module) = module else {
        return apply_values(slot, ins, |_, v| (v, false));
    };
    let automated = t.mixer.automation_mode.reads()
        && t.automation
            .iter()
            .any(|l| matches!(&l.param, AutoParam::Plugin { module: m, .. } if *m == module) && !l.points.is_empty() && !l.suspended);
    apply_values(slot, ins, |k, val| {
        if automated && let Some(l) = plugin_lane(t, module, k) {
            return (l.value_at(pos, val), true);
        }
        (val, false)
    });
}

/// How often inside a block a lane that changes there is read for a plugin that takes timed
/// changes ([`Plugin::set_param_at`]), samples: the sends' and pans' grid.
const TIMED_GRID: usize = 64;

/// Sample-accurate plugin automation: for each laned parameter whose lane changes inside the
/// block, its value every [`TIMED_GRID`] samples where it moves, as timed changes the plugin
/// takes or declines (the default declines: the value then changes once per block, as before).
/// A parameter sent so is resent at the next block's start.
fn apply_timed(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, t: &Track, module: ModuleId, pos: Samples, frames: usize) {
    let end = pos.saturating_add(i64::try_from(frames).unwrap_or(0));
    for ((k, v), applied) in ins.params.iter().zip(slot.applied.iter_mut()) {
        let Some(l) = plugin_lane(t, module, k) else { continue };
        // Only a lane with a point inside the block, or one ramping across it, moves here.
        let first = l.value_at(pos, *v);
        if l.points.iter().all(|p| p.at <= pos || p.at >= end) && (l.value_at(end, *v) - first).abs() <= f32::EPSILON {
            continue;
        }
        let mut last = first;
        let mut off = TIMED_GRID;
        let mut sent = false;
        while off < frames {
            let x = l.value_at(pos.saturating_add(i64::try_from(off).unwrap_or(0)), *v);
            if (x - last).abs() > f32::EPSILON {
                if !slot.plugin.set_param_at(off, k, x) {
                    break;
                }
                sent = true;
                last = x;
            }
            off += TIMED_GRID;
        }
        if sent {
            *applied = f32::NAN;
        }
    }
}

/// Whether module `ins` is bypassed at `pos`: its bypass lane when automation is read and the
/// lane has points (1 bypassed, 0 running; a crossfade as for a click), else its switch.
fn bypassed(t: &Track, ins: &soundcraft_model::Insert, pos: Samples) -> bool {
    match plugin_lane(t, ins.id, soundcraft_model::BYPASS_PARAM) {
        Some(l) => l.value_at(pos, if ins.bypass { 1.0 } else { 0.0 }) >= 0.5,
        None => ins.bypass,
    }
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
        .filter(|l| !l.points.is_empty() && !l.suspended)
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

pub(crate) fn transport_at(s: &Session, pos: Samples, playing: bool, recording: bool) -> Transport {
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
        && let Some(l) = t.played_lane(&AutoParam::Volume)
        && !l.points.is_empty()
    {
        return l.value_at(pos, t.mixer.volume_db);
    }
    t.mixer.volume_db
}

fn pan_at(t: &Track, idx: usize, pos: Samples) -> f32 {
    let base = t.mixer.pan.get(idx).copied().unwrap_or(if idx == 0 { 0.0 } else { 1.0 });
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.played_lane(&AutoParam::Pan(u8::try_from(idx).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return l.value_at(pos, base);
    }
    base
}

/// The track's surround panner at `pos`: its x, y, z and divergence lanes where automation is
/// read and they have points, else as set. `None` without a surround panner (lanes alone do not
/// make one). No allocation.
fn surround_at(t: &Track, pos: Samples) -> Option<SurroundPan> {
    let mut sp = t.mixer.surround?;
    if t.mixer.automation_mode.reads() {
        for l in &t.automation {
            if let AutoParam::Surround(i) = l.param
                && !l.points.is_empty()
                && !l.suspended
            {
                sp.set_axis(i, l.value_at(pos, sp.axis(i)));
            }
        }
    }
    Some(sp)
}

/// Pan automation is read on a grid of [`PAN_STEP`] timeline samples rather than once per block,
/// so playback (callback-sized blocks) and bounces (1024-frame blocks) pan identically.
const PAN_STEP: Samples = 64;

/// The position to read the pan at for the frames from `start` of the block at `pos`, and the end
/// of the frames that share it. Without pan automation the whole block shares one pan.
fn pan_segment(t: &Track, pos: Samples, start: usize, frames: usize) -> (Samples, usize) {
    let automated = t.mixer.automation_mode.reads()
        && t.automation.iter().any(|l| {
            matches!(l.param, AutoParam::Pan(_) | AutoParam::Width | AutoParam::Balance | AutoParam::Surround(_))
                && !l.points.is_empty()
                && !l.suspended
        });
    if !automated {
        return (pos, frames);
    }
    let here = pos.saturating_add(start as i64);
    let at = here.div_euclid(PAN_STEP).saturating_mul(PAN_STEP);
    let left = usize::try_from(at.saturating_add(PAN_STEP).saturating_sub(here)).unwrap_or(1).max(1);
    (at, start.saturating_add(left).min(frames))
}

/// A stereo track's width or balance (`p`) at `pos`: its lane when automation reads, else the
/// strip's setting `base`. On the pan's grid (`pan_segment`).
fn shape_at(t: &Track, p: AutoParam, base: f32, pos: Samples) -> f32 {
    match t.played_lane(&p).filter(|l| t.mixer.automation_mode.reads() && !l.points.is_empty()) {
        Some(l) => l.value_at(pos, base).clamp(-1.0, 1.0),
        None => base,
    }
}

/// The next stretch of a send's block from `start`: the timeline position its automation is read
/// at and where the stretch ends. An automated send (level, pan or mute lane) steps on the
/// [`PAN_STEP`] grid; one without lanes takes the whole block, read at its start.
fn send_segment(t: &Track, i: usize, pos: Samples, start: usize, frames: usize) -> (Samples, usize) {
    let k = u8::try_from(i).unwrap_or(0);
    let automated = t.mixer.automation_mode.reads()
        && t.automation.iter().any(|l| {
            matches!(l.param, AutoParam::SendLevel(x) | AutoParam::SendPan(x) | AutoParam::SendMute(x) if x == k)
                && !l.points.is_empty()
                && !l.suspended
        });
    if !automated {
        return (pos.saturating_add(start as i64), frames);
    }
    let here = pos.saturating_add(start as i64);
    let at = here.div_euclid(PAN_STEP).saturating_mul(PAN_STEP);
    let left = usize::try_from(at.saturating_add(PAN_STEP).saturating_sub(here)).unwrap_or(1).max(1);
    (at, start.saturating_add(left).min(frames))
}

fn send_level(t: &Track, i: usize, base: f32, pos: Samples) -> f32 {
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.played_lane(&AutoParam::SendLevel(u8::try_from(i).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return l.value_at(pos, base);
    }
    base
}

/// The left and right pan positions of a stereo send whose pan knob is at `p` (−1..1): centred,
/// the two sides go straight to left and right; turned left, the right side moves towards the
/// left until both are hard left (and the other way round). Centred is what it always was.
fn stereo_send_pans(p: f32) -> [f32; 2] {
    let p = if p.is_finite() { p.clamp(-1.0, 1.0) } else { 0.0 };
    if p < 0.0 { [-1.0, 1.0 + 2.0 * p] } else { [-1.0 + 2.0 * p, 1.0] }
}

/// A send's pan as its automation plays it (read like [`send_level`]).
fn send_pan(t: &Track, i: usize, base: f32, pos: Samples) -> f32 {
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.played_lane(&AutoParam::SendPan(u8::try_from(i).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return l.value_at(pos, base);
    }
    base
}

/// Whether a send is muted: its key, or its mute automation.
fn send_muted(t: &Track, i: usize, base: bool, pos: Samples) -> bool {
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.played_lane(&AutoParam::SendMute(u8::try_from(i).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return base || l.value_at(pos, 0.0) >= 0.5;
    }
    base
}

/// The `s.edit.values` key of the session's pan law: the centre level in dB (0, −3, −4.5 or −6).
pub const PAN_LAW_KEY: &str = "mix.pan_law_db";

/// The pan law the session asks for ([`PAN_LAW_KEY`]); −3 dB when unset or not one of the four.
/// Used by mono tracks and sends into stereo and by a mono bus read by a wider aux.
pub fn session_pan_law(s: &Session) -> PanLaw {
    pan_law_for_db(s.edit.value(PAN_LAW_KEY, -3.0))
}

/// The pan law whose centre level is `db` (the nearest of the four).
pub fn pan_law_for_db(db: f64) -> PanLaw {
    if !db.is_finite() {
        return PanLaw::Minus3;
    }
    [PanLaw::Zero, PanLaw::Minus3, PanLaw::Minus4_5, PanLaw::Minus6]
        .into_iter()
        .min_by(|a, b| (f64::from(a.center_db()) - db).abs().total_cmp(&(f64::from(b.center_db()) - db).abs()))
        .unwrap_or(PanLaw::Minus3)
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

/// A stereo strip's width and balance on its routing matrix (rows are the source's left and
/// right): the width mixes the sides before the pans (1 as it comes, 0 mono, -1 swapped), the
/// balance then turns down the side it leans away from. Neutral settings leave `m` as it is.
fn shape_stereo(m: &mut Matrix, width: f32, balance: f32) {
    let w = if width.is_finite() { width.clamp(-1.0, 1.0) } else { 1.0 };
    let b = if balance.is_finite() { balance.clamp(-1.0, 1.0) } else { 0.0 };
    if w == 1.0 && b == 0.0 {
        return;
    }
    let (a, x) = ((1.0 + w) * 0.5, (1.0 - w) * 0.5);
    let (gl, gr) = ((1.0 - b).min(1.0), (1.0 + b).min(1.0));
    let [l, r, ..] = m;
    for (pl, pr) in l.iter_mut().zip(r.iter_mut()) {
        let (ol, or) = (*pl, *pr);
        *pl = (a * ol + x * or) * gl;
        *pr = (x * ol + a * or) * gr;
    }
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
    /// ignore the mode and are left alone, but for Mend, which is only told (it waits for its
    /// model's answers offline; it needs no preparing again). A plugin that refuses the mode logs
    /// and renders on in realtime mode.
    fn set_hosted_offline(&mut self, offline: bool) {
        let (sr, mb) = (self.sample_rate, self.max_block);
        let again = |id: &str, ch: usize, plugin: &mut Box<dyn Plugin>| {
            if is_third_party(id) {
                plugin.set_offline(offline);
                plugin.prepare(sr, mb, ch);
            } else if id == soundcraft_dsp::plugins::mend::ID {
                plugin.set_offline(offline);
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
        seated + self.adopt.values().filter(|p| is_third_party(&p.id)).count() + self.parked.len()
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

/// Render only one track's clips (no mixer processing) — Consolidate. The clips' material as
/// edits take it: without the automatic edge fades playback and bounces add (`Edges`), which a
/// clip made from this render gets again where it plays.
pub fn render_clips(s: &Session, track: TrackId, range: Range) -> Vec<Vec<f32>> {
    render_clips_with(s, track, range, false)
}

/// [`render_clips`] as playback hears the clips: with the automatic edge fades
/// (`playback.auto_fade_ms`) where a clip starts or ends inside `range` (Consolidate).
pub fn render_clips_as_heard(s: &Session, track: TrackId, range: Range) -> Vec<Vec<f32>> {
    render_clips_with(s, track, range, true)
}

fn render_clips_with(s: &Session, track: TrackId, range: Range, heard: bool) -> Vec<Vec<f32>> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let len = usize::try_from(range.len().max(0)).unwrap_or(0).min(MAX_RENDER_SAMPLES / t.channels().max(1));
    let mut out = vec![vec![0.0f32; len]; t.channels().max(1)];
    for c in t.clips().iter().filter(|c| !c.muted && c.range().overlaps(&range)) {
        // Layered clips give way as they do in playback; the automatic edge ramps only as heard.
        let (edges, cover) = if heard {
            // An edge on the range's own boundary is the new clip's edge, which playback ramps
            // again: only edges inside the range are rendered faded.
            let mut edges = Edges::of(s, t.clips(), c, range, None);
            edges.start = edges.start.filter(|at| *at > range.start);
            edges.end = edges.end.filter(|at| *at < range.end);
            (edges, Cover::of(t.clips(), c, range, auto_fade_frames(s)))
        } else {
            (Edges::default(), Cover::of(t.clips(), c, range, 0))
        };
        match clip_fx_params(c) {
            None => render_clip(s, c, range.start, len, &mut out, edges, cover),
            Some(params) => {
                let mut tmp = vec![vec![0.0f32; len]; out.len()];
                render_clip(s, c, range.start, len, &mut tmp, edges, cover);
                let mut fx = ClipFx::new(s.sample_rate.as_f64() as f32, tmp.len());
                fx.prepare_modules(params.clone(), &mut Vec::new());
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
            // Width and balance belong to the main panner, after the render (the track delay is
            // before the inserts, so it is in it).
            t.mixer.width = 1.0;
            t.mixer.balance = 0.0;
            // The render is what the frozen track would play live.
            t.frozen = false;
            t.freeze = None;
            // Post-fader modules come after a fader the render does not have: they stay live
            // (never frozen; a commit moves them onto the new track).
            for i in t.mixer.inserts.iter_mut().filter(|i| i.post_fader) {
                i.active = false;
            }
        } else {
            t.inactive = true;
        }
    }
    solo
}

/// Plug-ins run in their own process that crashed or hung since the last call, said once each
/// (see `soundcraft_plugin_bridge`).
pub fn take_plugin_crash_reports() -> Vec<String> {
    soundcraft_plugin_bridge::take_crash_reports()
}

/// Hosted plug-ins in their own process: the app's setting (`plugin_bridge::set_enabled`), the
/// helper, Restart and the descriptions read there.
pub use soundcraft_plugin_bridge as plugin_bridge;

/// A CLAP or VST3 plug-in's description: read in a helper process while plug-ins run in their
/// own process (so a plug-in that crashes as it loads cannot take the app down; one that did is
/// described as nothing, never read here instead), else in this process as before.
pub fn hosted_plugin_info(id: &str) -> Option<&'static soundcraft_dsp::PluginInfo> {
    soundcraft_plugin_bridge::plugin_info(id)
        .unwrap_or_else(|| soundcraft_clap_host::plugin_info(id).or_else(|| soundcraft_vst3_host::plugin_info(id)))
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
    // A CLAP or VST3 plug-in in a process of its own when that is turned on (Preferences › Mixing): one
    // the helper could not make is not then made here, where its crash would take the app down.
    if let Some(bridged) = soundcraft_plugin_bridge::create(id) {
        return bridged;
    }
    external_midi::create(id)
        .or_else(|| track_midi::create(id))
        .or_else(|| soundcraft_dsp::create(id))
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
    use soundcraft_model::{ClipId, Insert, SourceAudio, SourceId};
    use std::sync::Arc;

    /// Options › Overlap › Layer: the lower clip gives way under a higher one exactly as the
    /// higher one comes in (its automatic ramp, or its own fade), so the two sum to one; clips on
    /// one layer (every session before layers) get no cover at all.
    #[test]
    fn a_layered_clip_crosses_over_to_the_one_under_it() {
        let lower = Clip::audio(ClipId(1), "low", SourceId(1), 0, 0, 10_000);
        let mut upper = Clip::audio(ClipId(2), "up", SourceId(1), 0, 4_000, 2_000);
        let clips = vec![lower.clone(), upper.clone()];
        assert!(Cover::of(&clips, &lower, Range::new(0, 10_000), 48).is_none(), "one layer: nothing covers");
        upper.layer = 1;
        let clips = vec![lower.clone(), upper.clone()];
        let c = Cover::of(&clips, &lower, Range::new(3_000, 3_500), 48);
        assert!(c.is_none(), "not in this block");
        let c = Cover::of(&clips, &lower, Range::new(3_990, 4_100), 48).unwrap();
        let up = Edges { len: 48, start: Some(4_000), end: Some(6_000), ..Edges::default() };
        for p in [3_999, 4_000, 4_010, 4_024, 4_047, 4_048, 5_000, 5_960, 5_999, 6_000] {
            let heard_up = if (4_000..6_000).contains(&p) { up.gain(p) } else { 0.0 };
            assert!((c.gain(p) + heard_up - 1.0).abs() < 1e-6, "at {p}: {} + {heard_up}", c.gain(p));
        }
        assert_eq!(c.gain(5_000), 0.0);
        // With a fade of its own, the cover follows the fade.
        upper.fade_in = soundcraft_model::Fade { len: 1_000, shape: soundcraft_model::FadeShape::Linear, curvature: 0.0 };
        let clips = vec![lower.clone(), upper];
        let c = Cover::of(&clips, &lower, Range::new(3_990, 4_100), 48).unwrap();
        assert!((c.gain(4_500) - 0.5).abs() < 1e-3, "{}", c.gain(4_500));
    }

    fn synth_session(level_db: f32) -> Session {
        use soundcraft_midi::{Note, Sequence};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let mut seq = Sequence::default();
        seq.notes.push(Note { probability: 100, pitch: 60, velocity: 100, muted: false, release_velocity: 64, channel: 0, start: 0, length: 960 });
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 24_000, seq));
        let mut ins = Insert::new("subtractive_synth");
        ins.params.insert("level".into(), level_db);
        tr.instrument = Some(ins);
        s
    }

    /// A mono audio track holding one clip of a two-channel source, L = 0.8 and R = 0.2.
    fn stereo_clip_on_mono(chosen: Option<u16>) -> (Session, TrackId) {
        let mut s = Session::default();
        // (about levels, not clip edges: no automatic edge fades)
        s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.pool.insert(SourceId(7), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.8; 4800], vec![0.2; 4800]] })));
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, "st", SourceId(7), 0, 0, 4800);
        c.source_channel = chosen;
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
        (s, t)
    }

    /// A mono track of constant 0.5 clips over `[start, start + len)` of one source, at `offset`.
    fn const_clips(clips: &[(i64, i64, i64)]) -> (Session, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.pool.insert(SourceId(8), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]] })));
        for &(start, len, offset) in clips {
            let id = s.new_clip_id();
            s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "c", SourceId(8), offset, start, len));
        }
        (s, t)
    }

    /// The left side of the main mix over `[0, 4000)` (playback's path, with its edge fades),
    /// scaled so a full-level 0.5 clip reads 0.5.
    fn heard(s: &Session) -> Vec<f32> {
        let out = render_range(s, Range::new(0, 4000), 256);
        let k = 1.0 / soundcraft_dsp::db_to_gain(-3.0);
        out[0].iter().map(|x| x * k).collect()
    }

    #[test]
    fn unfaded_clip_edges_ramp_so_a_cut_does_not_click() {
        let (s, t) = const_clips(&[(1000, 2000, 0)]);
        let l = auto_fade_frames(&s) as usize;
        assert_eq!(l, 48, "1 ms at 48 kHz");
        // Edits take the material as it is (Consolidate, Process).
        let material = &render_clips(&s, t, Range::new(0, 4000))[0];
        assert!((material[1000] - 0.5).abs() < 1e-6);
        let out = &heard(&s);
        assert_eq!(out[1000], 0.0, "silent on the first frame");
        assert_eq!(out[2999], 0.0, "and on the last");
        assert!((out[1000 + 2 * l] - 0.5).abs() < 1e-6 && (out[2999 - 2 * l] - 0.5).abs() < 1e-6, "full level inside");
        for w in out[990..3010].windows(2) {
            assert!((w[1] - w[0]).abs() < 0.5 / 16.0, "a step of {}", (w[1] - w[0]).abs());
        }
        // A fade of its own replaces the automatic one.
        let (mut s, _) = const_clips(&[(1000, 2000, 0)]);
        s.tracks[0].playlist_mut().unwrap().clips[0].fade_in.len = 500;
        let out = &heard(&s);
        assert!(out[1000 + 2 * l] < 0.2, "the clip's own 500-frame fade, not the short one");
        // Off: the cut is hard.
        let (mut s, _) = const_clips(&[(1000, 2000, 0)]);
        s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
        let out = &heard(&s);
        assert!((out[1000] - 0.5).abs() < 1e-6 && (out[2999] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_split_plays_straight_through_without_automatic_fades() {
        // Two halves of one stretch of material, end to end: no dip at the split.
        let (s, _) = const_clips(&[(1000, 1000, 0), (2000, 1000, 1000)]);
        let out = &heard(&s);
        assert!(out[1900..2100].iter().all(|x| (x - 0.5).abs() < 1e-6), "no dip at the split");
        assert_eq!(out[1000], 0.0, "the outer edges still ramp");
        assert_eq!(out[2999], 0.0);
        // Butted from elsewhere in the file (or another file: a file split in pieces) too.
        let (s, _) = const_clips(&[(1000, 1000, 0), (2000, 1000, 5000)]);
        assert!(heard(&s)[1900..2100].iter().all(|x| (x - 0.5).abs() < 1e-6));
        // With a gap between them, each edge faces silence and ramps.
        let (s, _) = const_clips(&[(1000, 900, 0), (2000, 1000, 1000)]);
        let out = &heard(&s);
        assert_eq!(out[1899], 0.0);
        assert_eq!(out[2000], 0.0);
    }

    #[test]
    fn a_clip_across_the_loop_ramps_at_its_edges_only_while_looping() {
        let (s, t) = const_clips(&[(0, 4000, 0)]);
        let clip = s.tracks[0].clips()[0].clone();
        let l = Range::new(1000, 3000);
        let e = Edges::of(&s, s.tracks[0].clips(), &clip, Range::new(2990, 3000), Some(l));
        assert_eq!(e.loop_out, Some(3000));
        assert_eq!(e.gain(2999), 0.0, "silent on the loop's last frame");
        assert!((e.gain(2999 - 48) - 1.0).abs() < 1e-6);
        let e = Edges::of(&s, s.tracks[0].clips(), &clip, Range::new(1000, 1010), Some(l));
        assert_eq!((e.loop_in, e.gain(1000)), (Some(1000), 0.0), "and on its first");
        // Not looping: no ramp inside the clip.
        assert!(Edges::of(&s, s.tracks[0].clips(), &clip, Range::new(2990, 3000), None).is_none());
        let _ = t;
    }

    #[test]
    fn a_stereo_clip_on_a_mono_track_folds_both_channels() {
        // Bug: only the left channel played. Folded at equal level, as a mono device folds the main.
        let (s, t) = stereo_clip_on_mono(None);
        let out = render_clips(&s, t, Range::new(0, 1000));
        assert_eq!(out.len(), 1);
        assert!(out[0].iter().all(|x| (x - 0.5).abs() < 1e-6), "0.5·L + 0.5·R, got {}", out[0][10]);
        // A chosen channel plays alone; one the source lacks plays the source as it is.
        let (s, t) = stereo_clip_on_mono(Some(1));
        assert!(render_clips(&s, t, Range::new(0, 1000))[0].iter().all(|x| (x - 0.2).abs() < 1e-6));
        let (s, t) = stereo_clip_on_mono(Some(5));
        assert!(render_clips(&s, t, Range::new(0, 1000))[0].iter().all(|x| (x - 0.5).abs() < 1e-6));
        // A stereo track still plays channel for channel; a chosen channel plays on both sides.
        let (mut s, t) = stereo_clip_on_mono(None);
        s.track_mut(t).unwrap().format = ChannelFormat::Stereo;
        let out = render_clips(&s, t, Range::new(0, 100));
        assert!((out[0][5] - 0.8).abs() < 1e-6 && (out[1][5] - 0.2).abs() < 1e-6);
        let (mut s, t) = stereo_clip_on_mono(Some(1));
        s.track_mut(t).unwrap().format = ChannelFormat::Stereo;
        let out = render_clips(&s, t, Range::new(0, 100));
        assert!((out[0][5] - 0.2).abs() < 1e-6 && (out[1][5] - 0.2).abs() < 1e-6);
        // And the heard window gives only the chosen channel.
        let (s, t) = stereo_clip_on_mono(Some(1));
        let c = s.track(t).unwrap().clips()[0].clone();
        let w = clip_heard_window(&s, &c, 0, 10).unwrap();
        assert_eq!(w.len(), 1);
        assert!((w[0][3] - 0.2).abs() < 1e-6);
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
            kit_fx: Vec::new(),
            llm_off: false,
            llm_pos: LLM_SETTLED,
            llm_fresh: true,
            dry: DelayLine::default(),
            emits: false,
            midi: MidiOut::new(),
            wet: 1.0,
            wet_to: 1.0,
            quiet: 0,
            heard: 0,
        };
        (slot, ins, tr)
    }

    type Sent = std::sync::Arc<std::sync::Mutex<Vec<(usize, String, f32)>>>;

    /// A plugin that takes timed changes when `2` is set, recording them in `1` (else as the
    /// plugin it wraps).
    struct Timed(Box<dyn Plugin>, Sent, bool);

    impl Plugin for Timed {
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
        fn set_param_at(&mut self, offset: usize, id: &str, v: f32) -> bool {
            if self.2 {
                self.1.lock().unwrap().push((offset, id.to_string(), v));
            }
            self.2
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.0.process(io, frames);
        }
    }

    /// Sample-accurate plugin automation: a lane ramping inside a block reaches a plugin that
    /// takes timed changes as points on the 64-sample grid; a plugin that declines (every
    /// built-in one) keeps its value once a block, as before; a flat block sends nothing timed.
    #[test]
    fn a_ramp_inside_a_block_reaches_a_plugin_as_timed_changes() {
        for takes in [true, false] {
            let (mut slot, ins, mut tr) = dist_slot("drive", 0.0);
            for l in &mut tr.automation {
                l.set_point(512, 1.0);
                l.set_point(2048, 1.0);
            }
            let sent: Sent = Default::default();
            let inner = std::mem::replace(&mut slot.plugin, soundcraft_dsp::create("fs_dist").unwrap());
            slot.plugin = Box::new(Timed(inner, sent.clone(), takes));
            apply_params(&mut slot, &ins, &tr, Some(ins.id), 0);
            apply_timed(&mut slot, &ins, &tr, ins.id, 0, 512);
            let got = sent.lock().unwrap().clone();
            if takes {
                let offs: Vec<usize> = got.iter().map(|g| g.0).collect();
                assert_eq!(offs, (1..8).map(|k| k * 64).collect::<Vec<_>>(), "every 64 samples of the ramp");
                assert!(got.iter().all(|g| g.1 == "drive"));
                let v = got.last().unwrap().2;
                assert!((v - 448.0 / 512.0).abs() < 1e-3, "{v}");
                assert!(slot.applied.iter().any(|a| a.is_nan()), "resent at the next block's start");
            } else {
                assert!(got.is_empty());
                assert!(slot.applied.iter().all(|a| !a.is_nan()), "a plugin that declines keeps its once-a-block value");
            }
            // A block where the lane stands still sends nothing timed.
            sent.lock().unwrap().clear();
            apply_params(&mut slot, &ins, &tr, Some(ins.id), 1024);
            apply_timed(&mut slot, &ins, &tr, ins.id, 1024, 512);
            assert!(sent.lock().unwrap().is_empty());
        }
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
            kit_fx: Vec::new(),
            llm_off: false,
            llm_pos: LLM_SETTLED,
            llm_fresh: true,
            dry: DelayLine::default(),
            emits: false,
            midi: MidiOut::new(),
            wet: 1.0,
            wet_to: 1.0,
            quiet: 0,
            heard: 0,
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
            kit_fx: Vec::new(),
            llm_off: false,
            llm_pos: LLM_SETTLED,
            llm_fresh: true,
            dry: DelayLine::default(),
            emits: false,
            midi: MidiOut::new(),
            wet: 1.0,
            wet_to: 1.0,
            quiet: 0,
            heard: 0,
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

    /// Parity section 7: a lane on the instrument (`INSTRUMENT_MODULE`) drives its parameter.
    #[test]
    fn an_instrument_lane_drives_the_instrument() {
        let mut s = synth_session(-6.0);
        let loud = render_range(&s, Range::new(0, 12_000), 512);
        let mut lane =
            soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: soundcraft_model::INSTRUMENT_MODULE, param: "level".into() });
        lane.set_point(0, -46.0);
        s.tracks[0].automation.push(lane);
        let quiet = render_range(&s, Range::new(0, 12_000), 512);
        let (a, b) = (peak(&loud[0]), peak(&quiet[0]));
        assert!(a > 0.01 && b < a * 0.05, "{a} against {b}");
    }

    fn session_with_dc(level: f32, frames: usize) -> (Session, TrackId) {
        let mut s = Session::default();
        // (about levels, not clip edges: no automatic edge fades)
        s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
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

    /// Mix bug 1: while the meters are held (not taken by the realtime side), a block's peaks are
    /// folded into the earlier ones instead of replacing them.
    #[test]
    fn held_meters_keep_the_loudest_block() {
        let (s, t) = session_with_dc(0.5, 512);
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut buf = vec![vec![0.0; 512]; 2];
        eng.render(&s, 0, 512, &mut buf);
        assert!((eng.meters[&t].peak[0] - 0.5).abs() < 1e-3);
        eng.meters_held = true;
        eng.render(&s, 512, 512, &mut buf);
        assert!((eng.meters[&t].peak[0] - 0.5).abs() < 1e-3, "the silent block does not hide the loud one");
        assert!(eng.main_meter.peak[0] > 0.3);
        eng.meters_held = false;
        eng.render(&s, 1024, 512, &mut buf);
        assert_eq!(eng.meters[&t].peak[0], 0.0, "taken: the next block starts afresh");
        assert_eq!(eng.main_meter.peak, [0.0; 2]);
    }

    /// Options › Pre-Fader Metering: a strip's meter reads its level before the fader.
    #[test]
    fn pre_fader_metering_reads_the_level_before_the_fader() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.track_mut(t).unwrap().mixer.volume_db = -20.0;
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut buf = vec![vec![0.0; 512]; 2];
        eng.render(&s, 0, 512, &mut buf);
        assert!((eng.meters[&t].peak[0] - 0.5 * db_to_gain(-20.0)).abs() < 1e-3, "post-fader by default");
        s.edit.pre_fader_metering = true;
        eng.render(&s, 512, 512, &mut buf);
        assert!((eng.meters[&t].peak[0] - 0.5).abs() < 1e-3, "pre-fader: {}", eng.meters[&t].peak[0]);
        assert!((buf[0][100] - 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-20.0)).abs() < 1e-3, "what plays is unchanged");
    }

    /// A meter carries the energy behind an RMS reading, summed over the blocks a reader has not
    /// taken, and the master's loudness meter reads the main mix while the transport rolls.
    #[test]
    fn meters_carry_rms_energy_and_the_master_its_loudness() {
        let (s, t) = session_with_dc(0.5, 48_000);
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_loudness_metering(true);
        let mut buf = vec![vec![0.0; 512]; 2];
        eng.render(&s, 0, 512, &mut buf);
        let ms = eng.meters[&t].mean_square();
        assert!((ms[0] - 0.25).abs() < 1e-4 && (ms[1] - 0.25).abs() < 1e-4, "{ms:?}");
        // Held: the next block adds its energy and frames, so the mean stays the block's mean.
        eng.meters_held = true;
        eng.render(&s, 512, 512, &mut buf);
        assert_eq!(eng.meters[&t].frames, 1024);
        let mut merged = eng.meters[&t].clone();
        merged.merge_from(&eng.meters[&t]);
        assert_eq!(merged.frames, 2048);
        assert!((merged.mean_square()[0] - 0.25).abs() < 1e-4);
        merged.clear_peaks();
        assert!(merged.mean_square()[0].is_nan());
        // Loudness: a second of DC at the master (0.354 on both sides after the pan law). DC is
        // filtered out by the K-weighting, so the integrated level is far below the sample level,
        // but the true peak reads the samples.
        for b in 2..90 {
            eng.render(&s, b * 512, 512, &mut buf);
        }
        let l = eng.main_meter.loudness.expect("the master is metered");
        let peak_db = 20.0 * (0.5 * std::f32::consts::FRAC_1_SQRT_2).log10();
        // (The step at the start overshoots between samples: a true peak a little above.)
        assert!(l.true_peak_db >= peak_db - 0.1 && l.true_peak_db < peak_db + 1.5, "{l:?}");
        assert!(l.momentary.is_finite());
        eng.reset_loudness();
        assert!(eng.main_meter.loudness.is_none());
        eng.render(&s, 90 * 512, 512, &mut buf);
        let fresh = eng.main_meter.loudness.unwrap();
        assert!(fresh.max_momentary < -100.0 && fresh.integrated < -100.0, "{fresh:?}");
        // Off by default (offline renders).
        let mut off = MixEngine::new(48_000.0, 512);
        off.render(&s, 0, 512, &mut buf);
        assert!(off.main_meter.loudness.is_none());
    }

    /// A stereo track's width narrows or swaps its sides before the pans, its balance turns one
    /// side down after them, and its delay moves its sound later; neutral, nothing changes.
    #[test]
    fn width_balance_and_track_delay_shape_the_strip() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        // A stereo source: left 0.5, right 0.
        let id = s.new_clip_id();
        s.pool
            .insert(SourceId(91), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800], vec![0.0; 4800]] })));
        let st = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.track_mut(st).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "lr", SourceId(91), 0, 0, 4800));
        s.track_mut(t).unwrap().mixer.mute = true;
        let at = |s: &Session| {
            let o = render_range(s, Range::new(0, 4800), 512);
            (o[0][1000], o[1][1000])
        };
        let (l, r) = at(&s);
        assert!((l - 0.5).abs() < 1e-4 && r.abs() < 1e-4, "as it comes: {l} {r}");
        s.track_mut(st).unwrap().mixer.width = 0.0;
        let (l, r) = at(&s);
        assert!((l - 0.25).abs() < 1e-4 && (r - 0.25).abs() < 1e-4, "mono: {l} {r}");
        s.track_mut(st).unwrap().mixer.width = -1.0;
        let (l, r) = at(&s);
        assert!(l.abs() < 1e-4 && (r - 0.5).abs() < 1e-4, "swapped: {l} {r}");
        s.track_mut(st).unwrap().mixer.width = 1.0;
        s.track_mut(st).unwrap().mixer.balance = 0.5;
        let (l, r) = at(&s);
        assert!((l - 0.25).abs() < 1e-4 && r.abs() < 1e-4, "balanced right: {l} {r}");
        s.track_mut(st).unwrap().mixer.balance = -0.5;
        assert!((at(&s).0 - 0.5).abs() < 1e-4, "the side it leans to stays at full level");
        s.track_mut(st).unwrap().mixer.balance = 0.0;
        // Delay: the clip's first sample comes 100 samples later.
        let first = |s: &Session| render_range(s, Range::new(0, 1024), 256)[0].iter().position(|x| x.abs() > 1e-3);
        assert_eq!(first(&s), Some(0));
        s.track_mut(st).unwrap().mixer.delay = 100;
        assert_eq!(first(&s), Some(100));
    }

    /// A send reaches a hardware output as well as a bus; its pan can follow the main pan; and
    /// with the option on, a muted track's pre-fader send still plays.
    #[test]
    fn sends_reach_outputs_follow_the_main_pan_and_can_ignore_mute() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.outputs.push(soundcraft_model::OutputPath { name: "Cue".into(), format: ChannelFormat::Stereo, first_channel: 2 });
        let mut snd = soundcraft_model::SendSlot::new(Route::Hardware("Cue".into()));
        snd.level_db = 0.0;
        s.track_mut(t).unwrap().mixer.sends[0] = Some(snd);
        s.track_mut(t).unwrap().mixer.output = Route::None;
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut buf = vec![vec![0.0; 512]; 4];
        let mut cue = |eng: &mut MixEngine, s: &Session, at: i64| {
            eng.render(s, at, 512, &mut buf);
            let h = &eng.hw[1];
            (h[0][300], h[1][300])
        };
        let c = std::f32::consts::FRAC_1_SQRT_2 * 0.5;
        let (l, r) = cue(&mut eng, &s, 0);
        assert!((l - c).abs() < 1e-3 && (r - c).abs() < 1e-3, "centred send into the cue output: {l} {r}");
        // Following the main pan (hard left), the send goes left.
        s.track_mut(t).unwrap().mixer.pan = vec![-1.0];
        s.track_mut(t).unwrap().mixer.sends[0].as_mut().unwrap().pan_follows_main = true;
        let (l, r) = cue(&mut eng, &s, 512);
        assert!((l - 0.5).abs() < 1e-3 && r.abs() < 1e-3, "follows the main pan: {l} {r}");
        // Muted: a pre-fader send is silent unless the option says otherwise.
        s.track_mut(t).unwrap().mixer.mute = true;
        s.track_mut(t).unwrap().mixer.sends[0].as_mut().unwrap().pre_fader = true;
        for b in 2..6 {
            cue(&mut eng, &s, b * 512);
        }
        assert!(cue(&mut eng, &s, 6 * 512).0.abs() < 1e-6, "muted");
        s.edit.set_flag(PRE_SENDS_IGNORE_MUTE, true);
        assert!((cue(&mut eng, &s, 7 * 512).0 - 0.5).abs() < 1e-3, "the pre-fader send ignores the mute");
    }

    /// A VCA can follow another VCA: the outer one's fader and mute reach the inner one's
    /// members too; a loop of VCAs stops where it closes.
    #[test]
    fn nested_vcas_add_their_faders() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        let inner = s.add_track(TrackKind::Vca, ChannelFormat::Stereo, None);
        let outer = s.add_track(TrackKind::Vca, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().mixer.vca = Some(inner.0);
        s.track_mut(inner).unwrap().mixer.vca = Some(outer.0);
        s.track_mut(inner).unwrap().mixer.volume_db = -6.0;
        s.track_mut(outer).unwrap().mixer.volume_db = -6.0;
        let level = |s: &Session| render_range(s, Range::new(0, 4800), 512)[0][2000];
        let c = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert!((level(&s) - c * db_to_gain(-12.0)).abs() < 5e-4, "{}", level(&s));
        // A loop (outer follows inner) counts each VCA once.
        s.track_mut(outer).unwrap().mixer.vca = Some(inner.0);
        assert!((level(&s) - c * db_to_gain(-12.0)).abs() < 5e-4);
        s.track_mut(outer).unwrap().mixer.vca = None;
        s.track_mut(outer).unwrap().mixer.mute = true;
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][4000].abs() < 1e-6, "the outer VCA's mute mutes the member");
    }

    /// The session's pan law sets a centred mono track's level (tier 2: a choice of pan law).
    #[test]
    fn the_session_pan_law_sets_the_centre_level() {
        let (mut s, _) = session_with_dc(0.5, 4800);
        for (db, want) in [(0.0, 0.5f32), (-3.0, 0.5 * std::f32::consts::FRAC_1_SQRT_2), (-4.5, 0.5 * db_to_gain(-4.5)), (-6.0, 0.25)] {
            s.edit.values.insert(PAN_LAW_KEY.into(), db);
            let out = render_range(&s, Range::new(0, 4800), 512);
            assert!((out[0][1000] - want).abs() < 2e-3 && (out[1][1000] - want).abs() < 2e-3, "{db} dB: {} vs {want}", out[0][1000]);
        }
        assert_eq!(pan_law_for_db(f64::NAN), PanLaw::Minus3);
        assert_eq!(pan_law_for_db(-5.0), PanLaw::Minus4_5);
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

    /// Mix bug 14 (the audit's claim was wrong): a mono track into a mono bus already takes its
    /// pan law there, so a stereo aux spreading that bus at unity plays it as loud as the track
    /// centred straight to the main mix. Pinned here.
    #[test]
    fn a_mono_bus_into_a_stereo_aux_is_as_loud_as_a_centred_mono_track() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        let direct = render_range(&s, Range::new(0, 4800), 512)[0][1000];
        let bus = s.add_bus("Mono", ChannelFormat::Mono);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Mono"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        s.track_mut(t).unwrap().mixer.output = Route::Bus(bus);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[0][1000] - direct).abs() < 1e-4 && (out[1][1000] - direct).abs() < 1e-4, "{} and {} vs {direct}", out[0][1000], out[1][1000]);
    }

    /// Mix bug 2: on a stereo track the send pan did nothing, though its knob is shown.
    #[test]
    fn a_stereo_send_follows_its_pan() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800], vec![0.25; 4800]] };
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(buf)));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "lr", SourceId(900), 0, 0, 4800));
        let bus = s.add_bus("Verb", ChannelFormat::Stereo);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        let set_pan = |s: &mut Session, pan: f32| {
            let tr = s.track_mut(t).unwrap();
            tr.mixer.output = Route::None;
            let mut snd = soundcraft_model::SendSlot::new(Route::Bus(bus));
            snd.level_db = 0.0;
            snd.pan = pan;
            tr.mixer.sends[0] = Some(snd);
        };
        set_pan(&mut s, 0.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[0][1000] - 0.5).abs() < 1e-3 && (out[1][1000] - 0.25).abs() < 1e-3, "centred: L to L, R to R");
        set_pan(&mut s, -1.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[1][1000].abs() < 1e-4, "hard left: nothing on the right: {}", out[1][1000]);
        assert!((out[0][1000] - 0.75).abs() < 1e-3, "both sides on the left: {}", out[0][1000]);
        set_pan(&mut s, 1.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][1000].abs() < 1e-4 && (out[1][1000] - 0.75).abs() < 1e-3, "hard right: {:?}", (out[0][1000], out[1][1000]));
        assert_eq!(stereo_send_pans(f32::NAN), [-1.0, 1.0]);
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

    /// A built-in gain that counts its `process` calls.
    struct Counting {
        inner: Box<dyn Plugin>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Plugin for Counting {
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
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inner.process(io, frames);
        }
    }

    /// Suspend Plug-ins on Silence: after a second of silence in and out an insert stops being
    /// run (off, it runs every block), and the first sound runs it again in the same block, which
    /// sounds as it would have.
    #[test]
    fn silent_inserts_are_suspended_and_wake_with_the_first_sound() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let blocks = 3 * 48_000 / 512;
        let mut heard = Vec::new();
        for on in [false, true] {
            let (mut s, t) = session_with_dc(0.5, 48_000);
            s.edit.set_flag(SUSPEND_ON_SILENCE, on);
            s.track_mut(t).unwrap().mixer.add_module(Insert::new("clap:test.count"), None).unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let counting = Counting { inner: soundcraft_dsp::create("gain").unwrap(), calls: Arc::clone(&calls) };
            let mut eng = MixEngine::new(48_000.0, 512);
            eng.set_external_instances(true);
            eng.adopt(vec![PreparedInstance {
                track: t,
                slot: SlotKey::Module(ModuleId(1)),
                id: "clap:test.count".into(),
                ch: 1,
                plugin: Box::new(counting),
            }]);
            // Three seconds of silence after the clip.
            for b in 0..blocks {
                render_block(&mut eng, &s, 48_000 + b as i64 * 512);
            }
            let n = calls.load(Ordering::Relaxed);
            if on {
                let after = (48_000.0 * SUSPEND_AFTER_SECS / 512.0).ceil() as usize;
                assert!((after..=after + 1).contains(&n), "ran {n} blocks, then rested");
            } else {
                assert_eq!(n, blocks, "off: every block runs");
            }
            heard.push(render_block(&mut eng, &s, 0));
            assert_eq!(calls.load(Ordering::Relaxed), n + 1, "sound runs it at once");
        }
        assert!(heard[0] > 0.1 && heard[0] == heard[1], "{heard:?}");
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
        let cc = |tick, kind, value| soundcraft_midi::CtrlEvent { tick, channel: 0, kind, value, shape: Default::default() };
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

    #[test]
    fn rpn_values_are_chased_after_their_select() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.ears"));
        let cc = |tick, n, value| soundcraft_midi::CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(n), value, shape: Default::default() };
        // Pitch-bend range 12 (RPN 0, 0) set at the start, the bank and a program after it.
        let ctrls = vec![cc(0, 101, 0), cc(1, 100, 0), cc(2, 6, 12), cc(3, 0, 1), cc(4, 32, 2)];
        let seq = soundcraft_midi::Sequence { ctrls, ..Default::default() };
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000 * 4, seq));
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ears = Ears { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.ears".into(), ch: 2, plugin: Box::new(ears) }]);
        render_block(&mut eng, &s, 0);
        heard.lock().unwrap().clear();
        render_block(&mut eng, &s, 36_000);
        let got: Vec<[u8; 3]> = heard.lock().unwrap().iter().map(|x| x.1).collect();
        assert_eq!(
            got,
            vec![[0xB0, 121, 0], [0xB0, 0, 1], [0xB0, 32, 2], [0xB0, 101, 0], [0xB0, 100, 0], [0xB0, 6, 12], [0xB0, 101, 0], [0xB0, 100, 0]],
            "the range reaches RPN 0, the bank before anything else"
        );
    }

    #[test]
    fn a_controller_ramp_plays_smoothly_and_is_chased_mid_ramp() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.ears"));
        // CC1 ramps from 0 to 127 over a beat (24 000 samples at 120 bpm), then holds; a step
        // point at the same time on CC7 holds as before.
        let cc = |tick, n, value, shape| soundcraft_midi::CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(n), value, shape };
        let ramp = soundcraft_midi::CtrlShape::Ramp(0.0);
        let step = soundcraft_midi::CtrlShape::Step;
        let ctrls = vec![cc(0, 1, 0, ramp), cc(0, 7, 10, step), cc(960, 1, 127, step), cc(960, 7, 90, step)];
        let seq = soundcraft_midi::Sequence { ctrls, ..Default::default() };
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000 * 4, seq));
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ears = Ears { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.ears".into(), ch: 2, plugin: Box::new(ears) }]);
        let mut cc1 = Vec::new();
        let mut cc7 = Vec::new();
        for b in 0..50 {
            render_block(&mut eng, &s, b * 512);
            for (off, m) in std::mem::take(&mut *heard.lock().unwrap()) {
                let at = b * 512 + off as i64;
                match m {
                    [0xB0, 1, v] => cc1.push((at, v)),
                    [0xB0, 7, v] => cc7.push((at, v)),
                    _ => {}
                }
            }
        }
        // CC7 steps: its two points only.
        assert_eq!(cc7, vec![(0, 10), (24_000, 90)]);
        // CC1 passes through every value on the way up, each on a 32-sample step, in order.
        assert_eq!(cc1.first(), Some(&(0, 0)));
        assert_eq!(cc1.last(), Some(&(24_000, 127)));
        assert_eq!(cc1.len(), 128, "every value once");
        assert!(cc1.windows(2).all(|w| w[1].1 == w[0].1 + 1 && w[1].0 > w[0].0), "{cc1:?}");
        assert!(cc1.iter().all(|(at, _)| at % RAMP_STEP == 0));
        // Halfway along it, a locate chases the value reached there, and the ramp carries on.
        render_block(&mut eng, &s, 12_000);
        let got = std::mem::take(&mut *heard.lock().unwrap());
        let chased = got.iter().find(|(o, m)| *o == 0 && m[0] == 0xB0 && m[1] == 1).map(|(_, m)| m[2]);
        assert!(chased.is_some_and(|v| (62..=64).contains(&v)), "{got:?}");
        assert!(got.iter().any(|(o, m)| *o > 0 && m[1] == 1 && m[2] > chased.unwrap_or(0)), "{got:?}");
    }

    /// Records the LFE layout the mixer hands it.
    struct Layout {
        inner: Box<dyn Plugin>,
        lfe: Arc<std::sync::atomic::AtomicU64>,
    }

    impl Plugin for Layout {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}
        fn set_lfe_channels(&mut self, mask: u64) {
            self.lfe.store(mask, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// The owner's rule for Comp's Link All Channels needs the plug-in to know which channel is the
    /// LFE: the mixer tells every insert from the track's channel format (5.1: channel 3; 7.1.4:
    /// channel 3; stereo and a 6-channel layout without one: none).
    #[test]
    fn inserts_are_told_which_channel_is_the_lfe() {
        for (fmt, want) in
            [(ChannelFormat::Surround51, 1u64 << 3), (ChannelFormat::Atmos714, 1 << 3), (ChannelFormat::Surround60, 0), (ChannelFormat::Stereo, 0)]
        {
            let mut s = Session::default();
            let t = s.add_track(TrackKind::Audio, fmt, None);
            let ins = Insert::new("clap:test.layout");
            let m = ins.id;
            s.track_mut(t).unwrap().mixer.inserts.push(ins);
            let lfe = Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
            let plugin = Layout { inner: soundcraft_dsp::create("gain").unwrap(), lfe: Arc::clone(&lfe) };
            let mut eng = MixEngine::new(48_000.0, 512);
            eng.set_external_instances(true);
            let ch = fmt.channels();
            eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Module(m), id: "clap:test.layout".into(), ch, plugin: Box::new(plugin) }]);
            render_block(&mut eng, &s, 0);
            assert_eq!(lfe.load(std::sync::atomic::Ordering::Relaxed), want, "{fmt:?}");
        }
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

    /// The note index changes nothing that plays: random dense clips (overlapping notes of one
    /// pitch, muted and hidden notes, controllers with ramps, channels, probabilities, playback quantize by grid, swing and
    /// groove, Real-Time delay and length, a start trim, locates with Note Chase) played block by
    /// block with and without their indexes give every message at the same offset.
    #[test]
    fn indexed_clips_play_event_for_event_as_before() {
        use soundcraft_midi::ops::{Groove, PlayQuantize};
        let mut rng = soundcraft_midi::XorShift::new(7);
        let mut pick = |n: u64| rng.next_u64() % n.max(1);
        for round in 0..12 {
            let mut s = Session::default();
            let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
            s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.tape"));
            if round % 3 == 1 {
                s.edit.values.insert(format!("rtp.{}.delay", t.0), pick(400) as f64 - 200.0);
                s.edit.values.insert(format!("rtp.{}.duration", t.0), 50.0 + pick(200) as f64);
            }
            if round % 2 == 0 {
                s.edit.set_flag("midi.note_chase", true);
            }
            let mut at = 0i64;
            for k in 0..3 {
                let mut seq = soundcraft_midi::Sequence::default();
                let channels = pick(3) == 0;
                for _ in 0..(200 + pick(400)) {
                    seq.notes.push(soundcraft_midi::Note {
                        probability: if pick(4) == 0 { 50 } else { 100 },
                        pitch: 48 + pick(12) as u8,
                        velocity: 1 + pick(127) as u8,
                        muted: pick(10) == 0,
                        release_velocity: 64,
                        channel: if channels { pick(4) as u8 } else { 0 },
                        start: pick(30_000) as i64 - 1_000,
                        length: 1 + if pick(20) == 0 { pick(20_000) } else { pick(600) } as i64,
                    });
                }
                // Controllers on a few lanes and channels, some ramps among them, and some hidden
                // before the start.
                for _ in 0..(100 + pick(300)) {
                    let kind = match pick(4) {
                        0 => CtrlKind::Cc(1),
                        1 => CtrlKind::Cc(7),
                        2 => CtrlKind::PitchBend,
                        _ => CtrlKind::PolyPressure(48 + pick(12) as u8),
                    };
                    seq.ctrls.push(soundcraft_midi::CtrlEvent {
                        tick: pick(30_000) as i64 - 1_000,
                        channel: if channels { pick(4) as u8 } else { 0 },
                        kind,
                        value: if kind == CtrlKind::PitchBend { pick(16_000) as i32 - 8_000 } else { pick(128) as i32 },
                        shape: if pick(3) == 0 { soundcraft_midi::CtrlShape::Ramp(pick(3) as f32 * 0.4 - 0.4) } else { Default::default() },
                    });
                }
                seq.sort();
                let len = 600_000 + pick(400_000) as i64;
                let cid = s.new_clip_id();
                let mut c = Clip::midi(cid, "dense", at, len, seq);
                c.play_quantize = match (round + k) % 4 {
                    0 => None,
                    1 => Some(PlayQuantize { grid: 240, strength: 0.8, swing: 0.0, groove: None }),
                    2 => Some(PlayQuantize { grid: 480, strength: 1.0, swing: 0.6, groove: None }),
                    _ => Some(PlayQuantize { grid: 240, strength: 1.0, swing: 0.0, groove: Groove::new("g", 960, &[0, 300, 620]) }),
                };
                s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
                at += len - pick(200_000) as i64;
            }
            let indexed = {
                let mut s = s.clone();
                for c in s.track_mut(t).unwrap().playlist_mut().unwrap().clips.iter_mut() {
                    c.midi_index.0 = c.midi_playing().map(|q| Arc::new(soundcraft_midi::NoteIndex::of_sequence(q)));
                    let notes = &c.midi_playing().unwrap().notes;
                    assert!(soundcraft_midi::NoteIndex::usable(c.midi_index.0.as_deref(), notes).is_some());
                }
                s
            };
            // Blocks in order, with a locate now and then.
            let mut blocks = Vec::new();
            let mut pos = 0i64;
            for b in 0..900 {
                if b % 150 == 149 {
                    pos = pick(2_000_000) as i64;
                }
                blocks.push(pos);
                pos += 512;
            }
            let play = |s: &Session| {
                let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
                let tape = Tape { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
                let mut eng = MixEngine::new(48_000.0, 512);
                eng.set_external_instances(true);
                eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.tape".into(), ch: 2, plugin: Box::new(tape) }]);
                for &p in &blocks {
                    render_block(&mut eng, s, p);
                    heard.lock().unwrap().push((true, usize::MAX - 1, [0; 3]));
                }
                heard.lock().unwrap().clone()
            };
            let (a, b) = (play(&s), play(&indexed));
            assert!(a.iter().filter(|e| e.2[0] & 0xF0 == 0x90).count() > 100, "round {round}: notes play");
            assert!(a.iter().filter(|e| e.2[0] & 0xF0 == 0xB0).count() > 50, "round {round}: controllers play");
            assert_eq!(a.len(), b.len(), "round {round}");
            for (k, (x, y)) in a.iter().zip(&b).enumerate() {
                assert_eq!(x, y, "round {round}, event {k}");
            }
        }
    }

    #[test]
    fn channel_1_clips_play_exactly_as_before() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.tape"));
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch: 60,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start: 4,
            length: 16,
        });
        let cc = |tick, kind, value| soundcraft_midi::CtrlEvent { tick, channel: 0, kind, value, shape: Default::default() };
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

    /// Parity section 7: the global transpose track shifts a note by the change in force where
    /// the note is written (its note-off too); a track set to ignore it plays as written.
    #[test]
    fn notes_follow_the_global_transpose_track() {
        let mut seq = soundcraft_midi::Sequence::default();
        for start in [0, 20] {
            seq.notes.push(soundcraft_midi::Note {
                probability: 100,
                pitch: 60,
                velocity: 100,
                muted: false,
                release_velocity: 64,
                channel: 0,
                start,
                length: 4,
            });
        }
        let (mut s, mut eng, heard) = taped_clip(Clip::midi(ClipId(1), "m", 0, 48_000, seq));
        // 25 samples a tick: the second note is written at sample 500.
        s.transpose_track = vec![(400, 3)];
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 512);
        let ons: Vec<u8> = heard.lock().unwrap().iter().filter(|e| e.2[0] & 0xF0 == 0x90).map(|e| e.2[1]).collect();
        let offs: Vec<u8> = heard.lock().unwrap().iter().filter(|e| e.2[0] & 0xF0 == 0x80).map(|e| e.2[1]).collect();
        assert_eq!((ons, offs), (vec![60, 63], vec![60, 63]));
        let t = s.tracks[0].id;
        s.track_mut(t).unwrap().transpose_independent = true;
        heard.lock().unwrap().clear();
        render_block(&mut eng, &s, 0);
        let ons: Vec<u8> = heard.lock().unwrap().iter().filter(|e| e.2[0] & 0xF0 == 0x90).map(|e| e.2[1]).collect();
        assert_eq!(ons, vec![60, 60]);
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

    /// An instrument that reports a look-ahead.
    struct LateKeys(Box<dyn Plugin>);

    impl Plugin for LateKeys {
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
            300
        }
    }

    /// Bug 23: an instrument's latency is compensated: the mix waits for it as for an insert's.
    #[test]
    fn an_instruments_latency_is_compensated() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.late"));
        s.edit.delay_compensation = true;
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        let late = LateKeys(soundcraft_dsp::create("gain").unwrap());
        eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.late".into(), ch: 2, plugin: Box::new(late) }]);
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 512);
        assert_eq!(eng.latency, 300);
    }

    /// Bug 19: two overlapping notes of one pitch: the second retriggers the voice and the sound
    /// lasts to the later end; the first note-off does not cut the second note short.
    #[test]
    fn overlapping_notes_of_one_pitch_each_get_their_own_end() {
        let mut seq = soundcraft_midi::Sequence::default();
        let note = |start, length| soundcraft_midi::Note {
            probability: 100,
            pitch: 60,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start,
            length,
        };
        seq.notes = vec![note(0, 16), note(8, 4)];
        let (s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq));
        render_block(&mut eng, &s, 0);
        let mut got: Vec<(usize, [u8; 3])> =
            std::mem::take(&mut *heard.lock().unwrap()).into_iter().filter(|(_, _, m)| m[0] & 0xE0 == 0x80).map(|(_, o, m)| (o, m)).collect();
        // In time order (equal times keep the order they were sent in).
        got.sort_by_key(|(o, _)| *o);
        assert_eq!(got, vec![(0, [0x90, 60, 100]), (200, [0x80, 60, 0]), (200, [0x90, 60, 100]), (400, [0x80, 60, 0])]);
    }

    /// Bug 9: a drawn note set to channel 3 plays on channel 3 (its controllers on theirs), and is
    /// closed on every channel when it is cut off.
    #[test]
    fn a_note_set_off_channel_1_plays_on_its_channel() {
        let seq = soundcraft_midi::Sequence {
            notes: vec![
                soundcraft_midi::Note {
                    probability: 100,
                    pitch: 60,
                    velocity: 100,
                    muted: false,
                    release_velocity: 64,
                    channel: 2,
                    start: 0,
                    length: 8,
                },
                soundcraft_midi::Note {
                    probability: 100,
                    pitch: 64,
                    velocity: 90,
                    muted: false,
                    release_velocity: 64,
                    channel: 0,
                    start: 0,
                    length: 8,
                },
            ],
            ctrls: vec![soundcraft_midi::CtrlEvent { tick: 4, channel: 2, kind: CtrlKind::Cc(1), value: 50, shape: Default::default() }],
        };
        let (s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq));
        render_block(&mut eng, &s, 0);
        let got = std::mem::take(&mut *heard.lock().unwrap());
        assert!(got.contains(&(true, 0, [0x92, 60, 100])), "{got:?}");
        assert!(got.contains(&(true, 0, [0x90, 64, 90])), "{got:?}");
        assert!(got.contains(&(true, 100, [0xB2, 1, 50])), "{got:?}");
        assert!(got.contains(&(true, 200, [0x82, 60, 64])), "{got:?}");
    }

    /// Note Chase: playback starting inside a note starts it there; without it the note waits.
    #[test]
    fn note_chase_starts_a_note_already_sounding() {
        let seq = soundcraft_midi::Sequence {
            notes: vec![soundcraft_midi::Note {
                probability: 100,
                pitch: 60,
                velocity: 90,
                muted: false,
                release_velocity: 64,
                channel: 0,
                start: 0,
                length: 400,
            }],
            ..Default::default()
        };
        let clip = || Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq.clone());
        let (s, mut eng, heard) = taped_clip(clip());
        render_block(&mut eng, &s, 1_000);
        assert!(!heard.lock().unwrap().iter().any(|e| e.2[0] & 0xF0 == 0x90), "off: the note does not start mid-way");
        let (mut s, mut eng, heard) = taped_clip(clip());
        s.edit.set_flag("midi.note_chase", true);
        render_block(&mut eng, &s, 1_000);
        assert!(heard.lock().unwrap().contains(&(false, 0, [0x90, 60, 90])), "{:?}", heard.lock().unwrap());
    }

    /// A muted note does not play; the notes around it do.
    #[test]
    fn a_muted_note_is_silent() {
        let note = |pitch, muted| soundcraft_midi::Note {
            probability: 100,
            pitch,
            velocity: 100,
            muted,
            release_velocity: 64,
            channel: 0,
            start: 0,
            length: 8,
        };
        let seq = soundcraft_midi::Sequence { notes: vec![note(60, true), note(64, false)], ..Default::default() };
        let (s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq));
        render_block(&mut eng, &s, 0);
        let got = std::mem::take(&mut *heard.lock().unwrap());
        assert!(got.contains(&(false, 0, [0x90, 64, 100])), "{got:?}");
        assert!(!got.iter().any(|e| e.2[1] == 60), "{got:?}");
    }

    /// Bug 8: a clip that starts between two ticks (25 samples a tick: 260 is tick 10.4) plays
    /// its first note at its start, so playback started there hears it.
    #[test]
    fn a_note_at_the_start_of_a_clip_off_the_tick_grid_plays_from_the_start() {
        let seq = soundcraft_midi::Sequence {
            notes: vec![soundcraft_midi::Note {
                probability: 100,
                pitch: 60,
                velocity: 100,
                muted: false,
                release_velocity: 64,
                channel: 0,
                start: 0,
                length: 8,
            }],
            ..Default::default()
        };
        let (s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 260, 48_000, seq));
        render_block(&mut eng, &s, 260);
        let got = std::mem::take(&mut *heard.lock().unwrap());
        assert!(got.contains(&(false, 0, [0x90, 60, 100])), "{got:?}");
    }

    /// Bug 10: a negative Real-Time Properties delay plays the early notes before the clip's
    /// start, each at its own time, rather than all together on the start.
    #[test]
    fn a_negative_delay_moves_early_notes_before_the_clip_rather_than_piling_them() {
        let mut seq = soundcraft_midi::Sequence::default();
        let note = |pitch, start| soundcraft_midi::Note {
            probability: 100,
            pitch,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start,
            length: 4,
        };
        seq.notes = vec![note(60, 0), note(62, 40)];
        // The clip starts on tick 100 (2 500 samples); 40 ticks early is 1 000 samples.
        let (mut s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 2_500, 48_000, seq));
        let t = s.tracks[0].id;
        s.edit.values.insert(format!("rtp.{}.delay", t.0), -40.0);
        render_block(&mut eng, &s, 1_024);
        render_block(&mut eng, &s, 1_536);
        render_block(&mut eng, &s, 2_048);
        let ons: Vec<(usize, u8)> =
            std::mem::take(&mut *heard.lock().unwrap()).into_iter().filter(|(_, _, m)| m[0] == 0x90).map(|(_, o, m)| (o, m[1])).collect();
        assert_eq!(ons, vec![(476, 60), (452, 62)], "60 at 1 500, 62 at 2 500");
    }

    /// Bug 5: what a start trim hid (negative ticks) does not play, but its controllers set the
    /// state the clip starts in, before its first note, and are chased after a seek into it.
    #[test]
    fn notes_hidden_by_a_start_trim_are_silent_and_their_controllers_set_the_start_state() {
        let mut seq = soundcraft_midi::Sequence::default();
        let note = |pitch, start| soundcraft_midi::Note {
            probability: 100,
            pitch,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start,
            length: 8,
        };
        seq.notes = vec![note(50, -100), note(60, 0)];
        seq.ctrls = vec![soundcraft_midi::CtrlEvent { tick: -50, channel: 0, kind: CtrlKind::Program, value: 9, shape: Default::default() }];
        // 25 samples a tick: the clip starts on tick 10.
        let (s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 250, 48_000, seq));
        render_block(&mut eng, &s, 0);
        assert_eq!(
            std::mem::take(&mut *heard.lock().unwrap()),
            vec![(false, 0, [0xB0, 121, 0]), (false, 250, [0xC0, 9, 0]), (false, 250, [0x90, 60, 100]), (false, 450, [0x80, 60, 0])]
        );
        render_block(&mut eng, &s, 10_000);
        let after: Vec<_> = std::mem::take(&mut *heard.lock().unwrap()).into_iter().filter(|x| *x != ALL_OFF).collect();
        assert_eq!(after, vec![(false, 0, [0xB0, 121, 0]), (false, 0, [0xC0, 9, 0])]);
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
        set(&mut s, soundcraft_model::MidiIn { device: Some("DIN".into()), channel: 0, ..Default::default() }, true, auto);
        play_live(&mut eng, true, &keys);
        render_block(&mut eng, &s, 1024);
        let h = take(&heard);
        assert!(h.contains(&(true, 20, [0x91, 62, 90])) && !h.iter().any(|x| x.2 == [0x90, 60, 100]), "{h:?}");
        set(&mut s, soundcraft_model::MidiIn { device: None, channel: 1, ..Default::default() }, true, auto);
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
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch: 40,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start: 0,
            length: 400,
        });
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
    fn a_pad_audition_is_heard_with_the_transport_stopped_and_the_track_unarmed() {
        // Juke's default kit on an unarmed instrument track, the transport stopped.
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let mut juke = Insert::new("juke");
        if let Some(info) = soundcraft_dsp::plugin_info("juke") {
            juke.params = info.params.iter().map(|p| (p.id.to_string(), p.default)).collect();
        }
        s.track_mut(t).unwrap().instrument = Some(juke);
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.monitor_only = true;
        stopped_block(&mut eng, &s);
        assert!(!eng.live_tail(), "nothing to render before the hit");
        // The kick pad's play button: note 36 on, held a few blocks, let go.
        assert!(eng.audition(t, 36, 110));
        assert!(eng.live_tail(), "the hit keeps the stopped mix rendering");
        let mut heard = 0.0f32;
        for _ in 0..4 {
            heard = heard.max(loud(&stopped_block(&mut eng, &s)));
        }
        assert!(eng.audition(t, 36, 0));
        assert!(heard > 0.05, "the pad is heard: {heard}");
        // The kick dies away and the mix stops rendering again.
        for _ in 0..(48_000 * 5 / 512) {
            stopped_block(&mut eng, &s);
        }
        assert!(!eng.live_tail(), "the hit's tail is over");
        // No track, or no instrument: nothing to play.
        assert!(!eng.audition(TrackId(9999), 36, 100));
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
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch: 40,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start: 0,
            length: 1900,
        });
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
        s.tracks[0].mixer.input = Route::Midi(soundcraft_model::MidiIn { device: Some("Keys".into()), channel: 0, ..Default::default() });
        play_live(&mut eng, true, &[(5, 0, [0x90, 60, 100])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        // Another port: the key held on Keys would never get its note-off through the new input.
        s.tracks[0].mixer.input = Route::Midi(soundcraft_model::MidiIn { device: Some("DIN".into()), channel: 0, ..Default::default() });
        render_block(&mut eng, &s, 0);
        assert_eq!(take_heard(&heard), vec![(true, 0, [0x80, 60, 0])]);
        // Another channel, likewise.
        play_live(&mut eng, true, &[(5, 1, [0x90, 62, 100])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        s.tracks[0].mixer.input = Route::Midi(soundcraft_model::MidiIn { device: Some("DIN".into()), channel: 2, ..Default::default() });
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 0);
        assert_eq!(take_heard(&heard), vec![(true, 0, [0x80, 62, 0])]);
    }

    #[test]
    fn all_notes_off_lets_go_of_every_note_and_pedal_now() {
        let (s, mut eng, heard) = armed_with_clip_note();
        eng.monitor_only = true;
        play_live(&mut eng, true, &[(1, 0, [0x91, 60, 100]), (2, 0, [0xB1, 64, 127])]);
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        eng.all_notes_off();
        let got = take_heard(&heard);
        assert_eq!(got.first(), Some(&(true, 0, [0x81, 60, 0])), "the held key's own note-off first");
        for ch in 0..16u8 {
            for cc in [64, 66, 123, 120] {
                assert!(got.contains(&(true, 0, [0xB0 | ch, cc, 0])), "channel {ch} CC {cc}");
            }
        }
        assert_eq!(got.last(), Some(&ALL_OFF));
        // Nothing is left held: disarmed, the track lets nothing go again.
        eng.live_midi.clear();
        let mut s = s;
        s.tracks[0].mixer.record_arm = false;
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 0);
        assert!(take_heard(&heard).iter().all(|h| h.2[0] & 0xF0 != 0x80), "no second note-off");
    }

    #[test]
    fn the_strip_meter_shows_the_velocity_its_instrument_was_given() {
        let (s, mut eng, _heard) = armed_with_clip_note();
        eng.monitor_only = true;
        play_live(&mut eng, true, &[(1, 0, [0x91, 60, 77])]);
        render_block(&mut eng, &s, 4096);
        let id = s.tracks[0].id;
        assert_eq!(eng.meters.get(&id).map(|m| m.midi_velocity), Some(77), "a played key");
        eng.live_midi.clear();
        render_block(&mut eng, &s, 4096);
        assert_eq!(eng.meters.get(&id).map(|m| m.midi_velocity), Some(0), "nothing new in the next block");
        // The clip's note (velocity 100) as playback starts on it; held for a slow reader.
        eng.monitor_only = false;
        eng.meters_held = true;
        render_block(&mut eng, &s, 0);
        render_block(&mut eng, &s, 512);
        assert_eq!(eng.meters.get(&id).map(|m| m.midi_velocity), Some(100), "a clip's note, held");
        let mut m = StripMeter::default();
        m.merge_from(eng.meters.get(&id).unwrap());
        m.clear_peaks();
        assert_eq!(m.midi_velocity, 0, "taken by the reader");
    }

    #[test]
    fn an_mpe_zone_is_sent_to_the_instrument_once_and_again_when_it_changes() {
        let (mut s, mut eng, heard) = armed_with_clip_note();
        eng.monitor_only = true;
        render_block(&mut eng, &s, 0);
        take_heard(&heard);
        s.tracks[0].mpe = Some(soundcraft_model::MpeZone { members: 3, ..Default::default() });
        render_block(&mut eng, &s, 0);
        let got = take_heard(&heard);
        assert!(got.contains(&(true, 0, [0xB0, 6, 3])), "the configuration message: {got:?}");
        assert!(got.contains(&(true, 0, [0xB3, 6, 48])), "the last member's bend range");
        render_block(&mut eng, &s, 0);
        assert!(take_heard(&heard).iter().all(|h| h.2 != [0xB0, 100, 6]), "sent once");
        s.tracks[0].mpe = None;
        render_block(&mut eng, &s, 0);
        assert!(take_heard(&heard).contains(&(true, 0, [0xB0, 6, 0])), "turned off: no members");
    }

    #[test]
    fn an_articulation_lane_sends_its_keyswitch_instead_of_the_controller() {
        let mut seq = soundcraft_midi::Sequence::default();
        let cc = |tick: i64, value: i32| soundcraft_midi::CtrlEvent {
            tick,
            channel: 0,
            kind: soundcraft_midi::CtrlKind::Cc(3),
            value,
            shape: Default::default(),
        };
        // Staccato from the start, then legato half way into the first block.
        seq.ctrls.push(cc(0, 1));
        seq.ctrls.push(cc(10, 0));
        let (mut s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq));
        let item = |name: &str, keyswitch: u8| soundcraft_model::Articulation { name: name.into(), keyswitch };
        s.tracks[0].articulations = Some(soundcraft_model::Articulations { cc: 3, items: vec![item("legato", 24), item("staccato", 25)] });
        render_block(&mut eng, &s, 0);
        let got: Vec<[u8; 3]> =
            take_heard(&heard).into_iter().map(|h| h.2).filter(|m| m[0] & 0xE0 == 0x80 || m[0] & 0xF0 == 0xB0 && m[1] == 3).collect();
        assert!(got.iter().all(|m| m[0] & 0xF0 != 0xB0), "the lane's controller never reaches the instrument: {got:?}");
        let ons: Vec<u8> = got.iter().filter(|m| m[0] & 0xF0 == 0x90).map(|m| m[1]).collect();
        assert_eq!(ons.last(), Some(&24), "legato last: {ons:?}");
        assert!(ons.contains(&25), "staccato first");
        // Chased from the middle: the articulation in force there.
        render_block(&mut eng, &s, 24_000);
        let chased: Vec<u8> = take_heard(&heard).into_iter().filter(|h| h.2[0] & 0xF0 == 0x90).map(|h| h.2[1]).collect();
        assert_eq!(chased, vec![24]);
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
            (tr.mixer.input, tr.mixer.record_arm) =
                (Route::Midi(soundcraft_model::MidiIn { device: Some("DIN".into()), channel: 0, ..Default::default() }), true);
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
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch: 60,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start: 4,
            length: 400,
        });
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
        let note = |length| soundcraft_midi::Note {
            probability: 100,
            pitch: 60,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start: 4,
            length,
        };
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
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch: 24,
            velocity: 1,
            muted: false,
            release_velocity: 0,
            channel: 0,
            start: 4,
            length: 2,
        });
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch: 48,
            velocity: 90,
            muted: false,
            release_velocity: 64,
            channel: 2,
            start: 4,
            length: 16,
        });
        seq.ctrls = vec![soundcraft_midi::CtrlEvent { tick: 4, channel: 0, kind: CtrlKind::Cc(11), value: 70, shape: Default::default() }];
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
        // Unflagged, a clip with a note off channel 1 plays its channels too (bug 9 of section 7;
        // it used to put everything on channel 1); one all on channel 1 plays as before.
        let (s, mut eng, heard) = taped_clip(Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq.clone()));
        render_block(&mut eng, &s, 0);
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
        if let Some(n) = seq.notes.get_mut(1) {
            n.channel = 0;
        }
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
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch: 48,
            velocity: 90,
            muted: false,
            release_velocity: 64,
            channel: 8,
            start: 2,
            length: 2,
        });
        seq.ctrls = vec![
            soundcraft_midi::CtrlEvent { tick: 2, channel: 0, kind: CtrlKind::Cc(11), value: 40, shape: Default::default() },
            soundcraft_midi::CtrlEvent { tick: 4, channel: 0, kind: CtrlKind::Cc(11), value: 70, shape: Default::default() },
            soundcraft_midi::CtrlEvent { tick: 8, channel: 2, kind: CtrlKind::Cc(1), value: 30, shape: Default::default() },
            soundcraft_midi::CtrlEvent { tick: 100, channel: 2, kind: CtrlKind::Cc(1), value: 99, shape: Default::default() }, // after this block
        ];
        let mut clip = Clip::midi(soundcraft_model::ClipId(1), "m", 0, 48_000, seq);
        clip.midi_channels = true;
        let (mut s, mut eng, heard) = taped_clip(clip);
        let plain = soundcraft_midi::Sequence {
            ctrls: vec![soundcraft_midi::CtrlEvent { tick: 1, channel: 5, kind: CtrlKind::Cc(7), value: 80, shape: Default::default() }],
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
                (true, 0, [0xB0, 121, 0]),
                (true, 0, [0xB0, 11, 70]),
                (true, 0, [0xB2, 121, 0]),
                (true, 0, [0xB2, 1, 30]),
                // The unflagged clip's CC7 is set on channel 6: it now plays (and is chased) there
                // (bug 9 of section 7; it used to go to channel 1).
                (true, 0, [0xB5, 121, 0]),
                (true, 0, [0xB5, 7, 80]),
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
        seq.notes.push(soundcraft_midi::Note {
            probability: 100,
            pitch: 60,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start: 0,
            length: 480,
        });
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

    /// Per-note probability: notes at 50 % play about half the time, a fresh render (a bounce)
    /// plays the same ones, a new pass rolls again, and notes at 100 % always play.
    #[test]
    fn notes_with_a_probability_roll_each_pass_and_bounces_repeat() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.keys"));
        let mut seq = soundcraft_midi::Sequence::default();
        // 200 sixteenths over the first 512 frames' worth of ticks would be too dense: one per
        // 10 ticks keeps them within one 48 000-frame second (1920 ticks).
        for i in 0..190i64 {
            let probability = if i % 10 == 0 { 100 } else { 50 };
            let n = soundcraft_midi::Note {
                pitch: 40 + (i % 40) as u8,
                velocity: 100,
                muted: false,
                release_velocity: 64,
                channel: 0,
                start: i * 10,
                length: 5,
                probability,
            };
            seq.notes.push(n);
        }
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 48_000, seq));
        let play = |passes: usize| -> Vec<Vec<u8>> {
            let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
            let keys = Keys { inner: soundcraft_dsp::create("gain").unwrap(), heard: Arc::clone(&heard) };
            let mut eng = MixEngine::new(48_000.0, 512);
            eng.set_external_instances(true);
            eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.keys".into(), ch: 2, plugin: Box::new(keys) }]);
            let mut out = Vec::new();
            for _ in 0..passes {
                for b in 0..94 {
                    render_block(&mut eng, &s, b * 512);
                }
                out.push(std::mem::take(&mut *heard.lock().unwrap()).into_iter().map(|(p, _)| p).collect());
            }
            out
        };
        let a = play(2);
        let b = play(2);
        assert_eq!(a, b, "a fresh render plays the same notes, pass for pass");
        let n = a[0].len();
        assert!((19 + 50..=19 + 120).contains(&n), "about half of the 171 notes at 50 % play, all 19 at 100 %: {n}");
        assert_ne!(a[0], a[1], "the next pass rolls again");
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
        let cid = s.track(t).unwrap().clips()[0].id;
        s.find_clip_mut(cid).unwrap().set_effect("gain", -6.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
        assert!((out[0][2000] - expect).abs() < 0.01, "{}", out[0][2000]);
        s.find_clip_mut(cid).unwrap().effects_bypass = true;
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[0][2000] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        s.track_mut(t).unwrap().lane_mut(&AutoParam::Trim).set_point(0, -12.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][2000] < 0.2, "{}", out[0][2000]);
    }

    /// Mix bug 4: the trim lane played with the track's automation Off.
    #[test]
    fn the_trim_lane_is_silent_with_automation_off() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.track_mut(t).unwrap().lane_mut(&AutoParam::Trim).set_point(0, -12.0);
        s.track_mut(t).unwrap().mixer.automation_mode = soundcraft_model::AutomationMode::Off;
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[0][2000] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{}", out[0][2000]);
    }

    /// Mix bug 5: a mute that switched mid-play jumped from full level to silence in one sample.
    #[test]
    fn a_mute_ramps_instead_of_clicking() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let full = 0.5 * db_to_gain(-3.0);
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut buf = vec![vec![0.0; 512]; 2];
        eng.render(&s, 0, 512, &mut buf);
        assert!((buf[0][511] - full).abs() < 1e-4);
        s.track_mut(t).unwrap().mixer.mute = true;
        eng.render(&s, 512, 512, &mut buf);
        let steps = buf[0].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        assert!(steps < full / 100.0, "no jump: {steps}");
        assert!(buf[0][0] > full * 0.9 && buf[0][300].abs() < 1e-6, "{} .. {}", buf[0][0], buf[0][300]);
        eng.render(&s, 1024, 512, &mut buf);
        assert!(buf[0].iter().all(|x| *x == 0.0), "then silent");
        // Opening ramps up the same way.
        s.track_mut(t).unwrap().mixer.mute = false;
        eng.render(&s, 1536, 512, &mut buf);
        assert!(buf[0][0] < full * 0.1 && (buf[0][400] - full).abs() < 1e-4);
        // A strip muted when the transport starts is silent from its first sample.
        s.track_mut(t).unwrap().mixer.mute = true;
        eng.reset();
        eng.render(&s, 0, 512, &mut buf);
        assert!(buf[0].iter().all(|x| *x == 0.0));
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

    /// Automation playback: a fader move on a latent track plays when its audio does, not the
    /// plug-in delay earlier.
    #[test]
    fn fader_automation_is_delay_compensated() {
        let lat = soundcraft_dsp::create("maximizer").unwrap().latency() as i64;
        assert!(lat > 0, "test needs a latent plugin");
        let mut s = Session::default();
        let mut imp = vec![0.0f32; 48_000];
        imp[2000] = 0.5;
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![imp] })));
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "i", SourceId(900), 0, 0, 48_000));
        tr.mixer.add_module(Insert::new("maximizer"), None).unwrap();
        // Full level until just after the impulse, then down: the impulse is heard.
        let step = 2000 + (lat / 2).max(1);
        let lane = tr.lane_mut(&AutoParam::Volume);
        lane.set_point(0, 0.0);
        lane.set_point(step - 1, 0.0);
        lane.set_point(step, -144.0);
        let out = render_range(&s, Range::new(0, 8000), 512);
        let peak = out[0].iter().fold(0f32, |a, x| a.max(x.abs()));
        assert!(peak > 0.1, "the impulse plays before the fade it comes before: {peak}");
    }

    /// Automation playback: a volume breakpoint inside a block is honoured where it is, and any
    /// block size plays the same.
    #[test]
    fn a_volume_breakpoint_inside_a_block_is_honoured() {
        let (mut s, t) = session_with_dc(0.5, 8000);
        let lane = s.track_mut(t).unwrap().lane_mut(&AutoParam::Volume);
        lane.set_point(0, 0.0);
        lane.set_point(1000, 0.0);
        lane.set_point(1001, -144.0);
        lane.set_point(3000, -144.0);
        lane.set_point(3001, 0.0);
        let big = render_range(&s, Range::new(0, 8000), 4096);
        let small = render_range(&s, Range::new(0, 8000), 97);
        let full = 0.5 * db_to_gain(-3.0);
        assert!(
            (big[0][1000] - full).abs() < 1e-4 && big[0][1002].abs() < 1e-4 && (big[0][3002] - full).abs() < 1e-4,
            "{} {} {}",
            big[0][1000],
            big[0][1002],
            big[0][3002]
        );
        for i in 0..8000 {
            assert!((big[0][i] - small[0][i]).abs() < 1e-5, "{i}: {} vs {}", big[0][i], small[0][i]);
        }
    }

    /// Automation playback: a mute lane switches at its point, not at the next block.
    #[test]
    fn a_mute_point_inside_a_block_mutes_there() {
        let (mut s, t) = session_with_dc(0.5, 8000);
        let lane = s.track_mut(t).unwrap().lane_mut(&AutoParam::Mute);
        lane.set_point(0, 0.0);
        lane.set_point(1000, 1.0);
        let out = render_range(&s, Range::new(0, 8000), 4096);
        let full = 0.5 * db_to_gain(-3.0);
        assert!((out[0][999] - full).abs() < 1e-4, "open before the point");
        assert!(out[0][1000] < full && out[0][1000 + MUTE_RAMP as usize] == 0.0, "closing from the point: {}", out[0][1000 + MUTE_RAMP as usize]);
        assert_eq!(out, render_range(&s, Range::new(0, 8000), 97), "any block size");
    }

    /// Automation playback: send automation plays the same whatever the block size.
    #[test]
    fn send_automation_does_not_depend_on_the_block_size() {
        let (mut s, t) = session_with_dc(0.5, 8000);
        let bus = s.add_bus("Verb", ChannelFormat::Stereo);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        let tr = s.track_mut(t).unwrap();
        tr.mixer.output = Route::None;
        let mut snd = soundcraft_model::SendSlot::new(Route::Bus(bus));
        snd.level_db = 0.0;
        tr.mixer.sends[0] = Some(snd);
        let lane = tr.lane_mut(&AutoParam::SendLevel(0));
        lane.set_point(0, 0.0);
        lane.set_point(4000, -40.0);
        let lane = tr.lane_mut(&AutoParam::SendMute(0));
        lane.set_point(0, 0.0);
        lane.set_point(6000, 1.0);
        let a = render_range(&s, Range::new(0, 8000), 4096);
        let b = render_range(&s, Range::new(0, 8000), 100);
        for i in 0..8000 {
            assert!((a[0][i] - b[0][i]).abs() < 1e-6, "{i}: {} vs {}", a[0][i], b[0][i]);
        }
        assert!(a[0][2000] > a[0][3900] && a[0][3900] > 0.0 && a[0][7000] == 0.0);
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
            let cid = s.track(t).unwrap().clips()[0].id;
            s.find_clip_mut(cid).unwrap().set_effect(param, value);
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

    /// Clip Effects are made when the session changes (`sync`), never by the first block a clip
    /// with effects plays: rendering then creates no module and grows no buffer.
    #[test]
    fn clip_effects_are_prepared_before_the_clip_plays() {
        let (mut s, t) = session_with_dc(0.5, 9600);
        s.edit.click = false;
        let cid = s.track(t).unwrap().clips()[0].id;
        s.find_clip_mut(cid).unwrap().set_effect("eq.hpf_on", 1.0);
        s.find_clip_mut(cid).unwrap().set_effect("comp.lookahead", 5.0);
        let mut eng = MixEngine::new(48_000.0, 512);
        // Realtime mode: what is retired is handed back for dropping elsewhere.
        eng.set_external_instances(true);
        eng.ensure_synced(&s);
        // What the audio thread will use, taken before anything plays.
        let seat = |eng: &MixEngine| {
            let st = &eng.strips[&t];
            let fx = st.clip_fx.get(&cid.0).expect("prepared by sync");
            let plug = |p: &Option<Box<dyn Plugin>>| p.as_ref().map(|b| &**b as *const dyn Plugin as *const () as usize);
            (plug(&fx.eq), plug(&fx.comp), st.clipbuf.iter().map(|c| (c.as_ptr() as usize, c.capacity())).collect::<Vec<_>>())
        };
        let before = seat(&eng);
        assert!(before.0.is_some() && before.1.is_some(), "both modules exist before the first block");
        assert!(before.2.iter().all(|(_, cap)| *cap >= 512), "the clip buffer is sized for a block");
        let mut out = vec![vec![0.0f32; 512]; 2];
        for b in 0..8 {
            eng.render(&s, b * 512, 512, &mut out);
        }
        assert_eq!(seat(&eng), before, "playing made or moved nothing");
        // Taking the EQ off is a session change: `sync` retires the module, not the audio thread.
        s.find_clip_mut(cid).unwrap().effects.retain(|(k, _)| !k.starts_with("eq."));
        eng.ensure_synced(&s);
        assert!(seat(&eng).0.is_none());
        assert!(!eng.retired.is_empty(), "the EQ was retired for dropping off the audio thread");
    }

    /// The realtime path: built-in inserts, instruments and Clip Effects modules are made by a
    /// `BuiltinPrep` on the control thread and only seated by `sync` on the render path, which
    /// creates (allocates) none itself; the sound is identical to an engine that makes them in
    /// `sync`.
    #[test]
    fn sync_on_the_render_path_makes_no_builtin_module() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        s.edit.click = false;
        let cid = s.track(t).unwrap().clips()[0].id;
        s.track_mut(t).unwrap().mixer.add_module(Insert::new("compressor"), None).unwrap();
        let synth = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(synth).unwrap().instrument = Some(Insert::new("subtractive_synth"));
        let realtime = || {
            let mut eng = MixEngine::new(48_000.0, 512);
            eng.set_external_instances(true);
            eng.set_prepared_builtins(true);
            eng
        };
        let mut eng = realtime();
        let mut prep = BuiltinPrep::default();
        let mut reference = MixEngine::new(48_000.0, 512);
        reference.set_external_instances(true);
        let mut out = vec![vec![0.0f32; 512]; 2];
        let mut want = vec![vec![0.0f32; 512]; 2];
        let mut block = 0i64;
        let mut play = |s: &Session, eng: &mut MixEngine, prep: &mut BuiltinPrep, reference: &mut MixEngine| {
            // The hand-over: made here (the control thread), seated by the render's sync.
            eng.adopt_prepared(prep.prepare(s, 512));
            for _ in 0..4 {
                eng.render(s, block * 512, 512, &mut out);
                reference.render(s, block * 512, 512, &mut want);
                assert_eq!(out, want, "block {block}: as loud and as shaped as an engine making its own modules");
                block += 1;
            }
            assert_eq!(eng.made_in_sync(), 0, "the render path created no module");
        };
        play(&s, &mut eng, &mut prep, &mut reference);
        assert!(eng.strips[&t].plugins[0].is_some() && eng.strips[&synth].instrument.is_some());
        // A new insert and new Clip Effects while playing: still nothing made on the render path.
        s.track_mut(t).unwrap().mixer.add_module(Insert::new("eq_7band"), None).unwrap();
        s.find_clip_mut(cid).unwrap().set_effect("eq.hpf_on", 1.0);
        play(&s, &mut eng, &mut prep, &mut reference);
        s.find_clip_mut(cid).unwrap().set_effect("comp.lookahead", 5.0);
        play(&s, &mut eng, &mut prep, &mut reference);
        let fx = &eng.strips[&t].clip_fx[&cid.0];
        assert!(fx.eq.is_some() && fx.comp.is_some());
        // A session already handed over makes nothing new.
        assert!(prep.prepare(&s, 512).is_empty());
        // The guard bites: an engine given nothing makes its modules in `sync` and says so.
        let mut bare = realtime();
        bare.render(&s, 0, 512, &mut out);
        assert_eq!(bare.made_in_sync(), 5, "two inserts, an instrument and two Clip Effects modules");
    }

    /// The realtime path: a track added while playing comes with its strip made and sized by the
    /// `BuiltinPrep` (buffers, delay and key lines, MIDI rings), as does a new bus's buffers; the
    /// render path seats them and renders with those very allocations, the same as an engine that
    /// sizes its own; a removed track's strip goes back to be freed off the audio thread.
    #[test]
    fn a_new_tracks_strip_comes_sized_from_the_control_thread() {
        let (mut s, t) = session_with_dc(0.5, 48_000);
        s.edit.click = false;
        let realtime = || {
            let mut eng = MixEngine::new(48_000.0, 512);
            eng.set_external_instances(true);
            eng.set_prepared_builtins(true);
            eng
        };
        let mut eng = realtime();
        let mut prep = BuiltinPrep::default();
        let mut reference = MixEngine::new(48_000.0, 512);
        reference.set_external_instances(true);
        let mut out = vec![vec![0.0f32; 512]; 2];
        let mut want = vec![vec![0.0f32; 512]; 2];
        let mut block = 0i64;
        let mut play = |s: &Session, eng: &mut MixEngine, prep: &mut BuiltinPrep, reference: &mut MixEngine| {
            let p = prep.prepare(s, 512);
            let ids: Vec<(TrackId, Vec<(usize, usize)>)> = p.strips.iter().map(|x| (x.track, x.buffer_ids())).collect();
            eng.adopt_prepared(p);
            for _ in 0..4 {
                eng.render(s, block * 512, 512, &mut out);
                reference.render(s, block * 512, 512, &mut want);
                assert_eq!(out, want, "block {block}: the same as an engine sizing its own strips");
                block += 1;
            }
            for (id, before) in &ids {
                assert_eq!(&eng.prepared_buffers(*id), before, "the strip renders with the buffers made off the audio thread");
            }
            assert_eq!(eng.strips_made_in_sync(), 0, "the render path made no strip");
            ids.len()
        };
        assert_eq!(play(&s, &mut eng, &mut prep, &mut reference), s.tracks.len());
        // While playing: a stereo track with a compressor keyed by the first, a pre-fader send to a
        // new bus and a track delay.
        let bus = s.add_bus("Verb", ChannelFormat::Stereo);
        let n = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("New"));
        {
            let tr = s.track_mut(n).unwrap();
            let mut comp = Insert::new("compressor");
            comp.sidechain = Some(soundcraft_model::SidechainSource { track: t, pre_fader: false });
            tr.mixer.add_module(comp, None).unwrap();
            let mut snd = soundcraft_model::SendSlot::new(Route::Bus(bus));
            snd.pre_fader = true;
            snd.level_db = 0.0;
            tr.mixer.sends[0] = Some(snd);
            tr.mixer.delay = 100;
        }
        assert_eq!(play(&s, &mut eng, &mut prep, &mut reference), 1, "only the new track's strip");
        assert!(!eng.prepared_buffers(n).is_empty());
        assert!(eng.busses[&bus].buf.len() == 2 && eng.busses[&bus].buf.iter().all(|c| c.len() == 512));
        let _ = eng.take_retired_strips();
        // Many tracks at once: the tables come larger, made off the audio thread.
        let more: Vec<TrackId> = (0..40).map(|_| s.add_track(TrackKind::Audio, ChannelFormat::Mono, None)).collect();
        assert_eq!(play(&s, &mut eng, &mut prep, &mut reference), 40);
        assert!(eng.strips.capacity() >= s.tracks.len());
        // Removed: the strip is retired, not dropped on the render path.
        s.tracks.retain(|x| x.id != more[0]);
        assert_eq!(play(&s, &mut eng, &mut prep, &mut reference), 0);
        let gone = eng.take_retired_strips();
        assert_eq!(gone.strips.len(), 1, "the removed track's strip waits to be freed elsewhere");
        // An engine given nothing sizes its strips itself and says so.
        let mut bare = realtime();
        bare.render(&s, 0, 512, &mut out);
        assert_eq!(bare.strips_made_in_sync(), s.tracks.len() as u64);
    }

    /// A suspended lane (automation Preview) is not played: the strip plays its own setting, as
    /// with no lane at all; sends' and plug-ins' lanes alike.
    #[test]
    fn a_suspended_lane_plays_the_control_as_set() {
        let (mut s, t) = session_with_dc(0.5, 8192);
        s.edit.click = false;
        let plain = render_range(&s, Range::new(0, 4096), 1024);
        let mut lane = soundcraft_model::AutomationLane::new(AutoParam::Volume);
        lane.set_point(0, -144.0);
        lane.set_point(8192, -144.0);
        s.track_mut(t).unwrap().automation.push(lane);
        let quiet = render_range(&s, Range::new(0, 4096), 1024);
        assert!(quiet[0].iter().all(|x| x.abs() < 1e-4), "the lane plays");
        s.track_mut(t).unwrap().automation[0].suspended = true;
        assert_eq!(render_range(&s, Range::new(0, 4096), 1024), plain, "suspended: the fader as set");
    }

    /// An additional output is the main output again: a track with no main output and an extra
    /// output to the main mix sounds as one with its main output there, fader, pan, their
    /// automation and mute included, whatever the slot's own send level, pan, mute and lanes say.
    #[test]
    fn an_additional_output_is_a_copy_of_the_main_output() {
        let (mut s, t) = session_with_dc(0.5, 8192);
        s.edit.click = false;
        {
            let tr = s.track_mut(t).unwrap();
            tr.mixer.volume_db = -6.0;
            tr.mixer.pan = vec![0.4];
            let mut pan = soundcraft_model::AutomationLane::new(AutoParam::Pan(0));
            pan.set_point(0, -0.8);
            pan.set_point(8192, 0.8);
            tr.automation.push(pan);
            // The slot's own controls and lanes, which an output ignores.
            let mut lvl = soundcraft_model::AutomationLane::new(AutoParam::SendLevel(0));
            lvl.set_point(0, -144.0);
            tr.automation.push(lvl);
        }
        let main = render_range(&s, Range::new(0, 4096), 1024);
        {
            let tr = s.track_mut(t).unwrap();
            tr.mixer.output = Route::None;
            let mut out = soundcraft_model::SendSlot::new(Route::Main);
            out.output = true;
            out.level_db = -144.0;
            out.mute = true;
            out.pan = -1.0;
            tr.mixer.sends[0] = Some(out);
        }
        assert_eq!(render_range(&s, Range::new(0, 4096), 1024), main);
        s.track_mut(t).unwrap().mixer.mute = true;
        assert!(render_range(&s, Range::new(0, 4096), 1024)[0].iter().all(|x| *x == 0.0), "the track's mute silences it");
    }

    fn impulse_at(ch: &[f32]) -> Vec<usize> {
        ch.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4).map(|(i, _)| i).collect()
    }

    /// An aux insert's lookahead is part of the mix latency on the first block, not the block after.
    #[test]
    fn delay_compensation_aligns_an_aux_insert() {
        let mut s = Session::default();
        // (impulses at the clip's first frame: no automatic edge fades)
        s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
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

    /// Feedback protection: a monitored input clipping for a second is silenced on its track
    /// (and reported) until its monitoring is turned off and on; a loud but varying input is
    /// not, nor is one held loud just under clipping, and with the protection off nothing is.
    #[test]
    fn feedback_protection_silences_a_howling_input() {
        let (mut s, t) = session_with_dc(0.0, 4800);
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::On;
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut out = vec![vec![0.0; 512]; 2];
        let run = |eng: &mut MixEngine, s: &Session, blocks: usize, level: f32, out: &mut Vec<Vec<f32>>| {
            for b in 0..blocks {
                // Every fifth block quieter when `level` is negative: loud, but not held.
                let l = if level < 0.0 && b % 5 == 0 { 0.2 } else { level.abs() };
                eng.input = vec![vec![l; 512]];
                eng.render(s, (b * 512) as i64, 512, out);
            }
        };
        run(&mut eng, &s, 200, -1.0, &mut out);
        assert!(out[0][100] > 0.1 && !eng.meters[&t].feedback, "a clipping but varying input plays");
        run(&mut eng, &s, 300, 0.95, &mut out);
        assert!(out[0][100] > 0.1 && !eng.meters[&t].feedback, "held at -0.45 dBFS, not clipping: plays");
        run(&mut eng, &s, 1, 0.1, &mut out);
        run(&mut eng, &s, 90, 1.0, &mut out);
        assert!(out[0][100] > 0.1, "under a second: still heard");
        run(&mut eng, &s, 10, 1.0, &mut out);
        assert!(out[0][100].abs() < 1e-6 && eng.meters[&t].feedback, "a second held: silenced");
        run(&mut eng, &s, 5, 0.1, &mut out);
        assert!(out[0][100].abs() < 1e-6, "stays silent once it is quiet");
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::Off;
        run(&mut eng, &s, 1, 0.1, &mut out);
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::On;
        run(&mut eng, &s, 1, 0.1, &mut out);
        assert!(out[0][100] > 0.01 && !eng.meters[&t].feedback, "monitoring off and on: heard again");
        s.edit.set_flag(NO_FEEDBACK_PROTECTION, true);
        run(&mut eng, &s, 200, 1.0, &mut out);
        assert!(out[0][100] > 0.1, "protection off");
    }

    /// Options › Recording › Input Only Monitoring: an armed track on Auto hears its input in
    /// playback too; unarmed, or set to Off, it plays its clips as before.
    #[test]
    fn input_only_monitoring_hears_armed_tracks_in_playback() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.input = vec![vec![0.25; 512]];
        let mut out = vec![vec![0.0; 512]; 2];
        let (clip, input) = (0.5 * std::f32::consts::FRAC_1_SQRT_2, 0.25 * std::f32::consts::FRAC_1_SQRT_2);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        eng.render(&s, 0, 512, &mut out);
        assert!((out[0][100] - clip).abs() < 1e-3, "auto input: the clip plays in playback");
        s.edit.set_flag(INPUT_ONLY_MONITORING, true);
        eng.render(&s, 512, 512, &mut out);
        assert!((out[0][100] - input).abs() < 1e-3, "input only: {}", out[0][100]);
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::Off;
        eng.render(&s, 1024, 512, &mut out);
        assert!((out[0][100] - clip).abs() < 1e-3, "Off keeps its own setting");
        let tr = s.track_mut(t).unwrap();
        (tr.mixer.input_monitor, tr.mixer.record_arm) = (soundcraft_model::InputMonitor::Auto, false);
        eng.render(&s, 1536, 512, &mut out);
        assert!((out[0][100] - clip).abs() < 1e-3, "unarmed: clips");
    }

    #[test]
    fn record_with_inserts_taps_the_input_after_the_strip_monitored_or_not() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let tr = s.track_mut(t).unwrap();
        (tr.mixer.record_arm, tr.mixer.record_inserts, tr.mixer.trim_db) = (true, true, -6.0);
        tr.mixer.input_monitor = soundcraft_model::InputMonitor::Off;
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.input = vec![vec![0.5; 512]];
        let mut out = vec![vec![0.0; 512]; 2];
        let printed = 0.5 * db_to_gain(-6.0);
        // Recording with monitoring Off: the input goes through the trim for the take, unheard.
        eng.recording = true;
        eng.render(&s, 0, 512, &mut out);
        let (tap, late) = eng.record_tap(t).unwrap();
        assert_eq!(late, 0);
        assert!((tap[0][100] - printed).abs() < 1e-4, "{}", tap[0][100]);
        assert!(out[0][100].abs() < 1e-6, "not heard: {}", out[0][100]);
        // Monitored: heard as printed.
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::On;
        eng.render(&s, 512, 512, &mut out);
        assert!((eng.record_tap(t).unwrap().0[0][100] - printed).abs() < 1e-4);
        assert!((out[0][100] - printed * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{}", out[0][100]);
        // Not recording: nothing to take.
        eng.recording = false;
        eng.render(&s, 1024, 512, &mut out);
        assert!(eng.record_tap(t).unwrap().0[0][100].abs() < 1e-6);
        // Off, the track has no tap: its take is the input as it came in.
        s.track_mut(t).unwrap().mixer.record_inserts = false;
        eng.render(&s, 1536, 512, &mut out);
        assert!(eng.record_tap(t).is_none());
    }

    #[test]
    fn an_audio_track_on_a_bus_input_hears_and_taps_that_bus() {
        // Internal recording: the "Print" track's input is the bus the DC track plays into. It is
        // first in the session but renders after what feeds the bus.
        let mut s = Session::default();
        let print = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Print"));
        let src = add_dc_track(&mut s, 0.5, 4800);
        let bus = s.add_bus("Print Bus", ChannelFormat::Mono);
        s.track_mut(src).unwrap().mixer.output = Route::Bus(bus);
        let p = s.track_mut(print).unwrap();
        p.mixer.input = Route::Bus(bus);
        p.mixer.record_arm = true;
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut out = vec![vec![0.0; 512]; 2];
        // Recording, armed on Auto: the track hears the bus (no audio device is needed).
        eng.recording = true;
        eng.render(&s, 0, 512, &mut out);
        let (tap, late) = eng.record_tap(print).unwrap();
        assert_eq!(late, 0);
        // The DC track centred into a mono bus: its level at the -3 dB pan law, or as is.
        let heard = tap[0][100];
        assert!(heard > 0.3 && heard <= 0.5 + 1e-6, "{heard}");
        assert!((out[0][100] - heard * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{}", out[0][100]);
        // Monitoring Off: silent, but the tap still holds the bus (the take records it).
        s.track_mut(print).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::Off;
        eng.render(&s, 512, 512, &mut out);
        assert!(out[0][100].abs() < 1e-6, "{}", out[0][100]);
        assert!((eng.record_tap(print).unwrap().0[0][100] - heard).abs() < 1e-6);
        // A track on a hardware input has no tap; neither does an aux.
        assert!(eng.record_tap(src).is_none());
    }

    #[test]
    fn an_armed_track_with_monitoring_off_does_not_play_its_clips_under_the_take() {
        // Bug: with monitoring Off, the old clips played under the take being recorded.
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::Off;
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.input = vec![vec![0.25; 512]];
        let mut out = vec![vec![0.0; 512]; 2];
        let pan = std::f32::consts::FRAC_1_SQRT_2;
        // Playing, not recording: the clips.
        eng.render(&s, 0, 512, &mut out);
        assert!((out[0][100] - 0.5 * pan).abs() < 1e-3, "{}", out[0][100]);
        // Recording: neither the input (monitoring is off) nor the clips.
        eng.recording = true;
        eng.render(&s, 512, 512, &mut out);
        assert!(out[0][100].abs() < 1e-6, "{}", out[0][100]);
        // A punch: the clips in the pre- and post-roll, silence inside.
        eng.punch = Some(Range::new(2048, 3072));
        for (pos, want) in [(1024, 0.5 * pan), (2048, 0.0), (3072, 0.5 * pan)] {
            eng.render(&s, pos, 512, &mut out);
            assert!((out[0][100] - want).abs() < 1e-3, "{pos}: {}", out[0][100]);
        }
        // A track that is not armed plays on.
        eng.punch = None;
        s.track_mut(t).unwrap().mixer.record_arm = false;
        eng.render(&s, 0, 512, &mut out);
        assert!((out[0][100] - 0.5 * pan).abs() < 1e-3, "{}", out[0][100]);
    }

    #[test]
    fn a_stereo_track_with_no_input_monitors_what_it_records() {
        // Bug: it heard In 1 on both sides but recorded In 1-2. Now both are
        // `default_input_channel`: In 1-2 on a stereo device, In 1 twice on a mono one.
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::On;
        let mut eng = MixEngine::new(48_000.0, 512);
        let mut out = vec![vec![0.0; 512]; 2];
        eng.input = vec![vec![0.1; 512], vec![0.3; 512]];
        eng.render(&s, 0, 512, &mut out);
        assert!(out[0][100] > 0.01 && (out[1][100] / out[0][100] - 3.0).abs() < 1e-3, "{} {}", out[0][100], out[1][100]);
        let heard: Vec<usize> = (0..2).filter_map(|k| soundcraft_model::default_input_channel(k, 2, 2)).collect();
        assert_eq!(heard, [0, 1]);
        eng.input = vec![vec![0.1; 512]];
        eng.render(&s, 512, 512, &mut out);
        assert!(out[0][100] > 0.01 && (out[1][100] - out[0][100]).abs() < 1e-6, "{} {}", out[0][100], out[1][100]);
    }

    #[test]
    fn a_punch_plays_the_clips_in_the_pre_and_post_roll() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.input = vec![vec![0.25; 512]];
        eng.recording = true;
        eng.punch = Some(Range::new(1024, 2048));
        let mut out = vec![vec![0.0; 512]; 2];
        let pan = std::f32::consts::FRAC_1_SQRT_2;
        for (pos, want, what) in [(0, 0.5, "pre-roll: the clip"), (1024, 0.25, "punched in: the input"), (2048, 0.5, "post-roll: the clip")] {
            eng.render(&s, pos, 512, &mut out);
            assert!((out[0][100] - want * pan).abs() < 1e-3, "{what}: {}", out[0][100]);
        }
        // No punch range: the input for the whole recording, as before.
        eng.punch = None;
        eng.render(&s, 0, 512, &mut out);
        assert!((out[0][100] - 0.25 * pan).abs() < 1e-3, "{}", out[0][100]);
        // Input monitoring On hears the input outside the range too.
        eng.punch = Some(Range::new(1024, 2048));
        s.track_mut(t).unwrap().mixer.input_monitor = soundcraft_model::InputMonitor::On;
        eng.render(&s, 0, 512, &mut out);
        assert!((out[0][100] - 0.25 * pan).abs() < 1e-3, "{}", out[0][100]);
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

    /// Setup › Click/Countoff › Output: the click plays on the chosen hardware output alone,
    /// not in the main mix; an output that is not there sends it back to the main mix.
    #[test]
    fn the_click_plays_on_its_own_output() {
        let mut s = Session::default();
        s.edit.click = true;
        s.outputs.push(soundcraft_model::OutputPath { name: "Drummer".into(), format: ChannelFormat::Stereo, first_channel: 2 });
        s.edit.flags.insert("click.output=drummer".into());
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.metronome = true;
        let mut out = vec![vec![0.0f32; 512]; 2];
        eng.render(&s, 0, 512, &mut out);
        assert!(out.iter().all(|c| c.iter().all(|x| *x == 0.0)), "not in the main mix");
        let peak = eng.hw[1][0].iter().take(200).fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.1, "on its output: {peak}");
        s.edit.flags.remove("click.output=drummer");
        s.edit.flags.insert("click.output=Gone".into());
        let mut eng = MixEngine::new(48_000.0, 512);
        eng.metronome = true;
        eng.render(&s, 0, 512, &mut out);
        assert!(out[0].iter().take(200).any(|x| x.abs() > 0.1), "an unknown output: the main mix");
    }

    /// Mix bug 12: the output meter measured the click with the mix.
    #[test]
    fn the_output_meter_leaves_the_click_out() {
        let mut s = Session::default();
        s.edit.click = true;
        let mut eng = MixEngine::new(s.sample_rate.as_f64() as f32, 512);
        eng.metronome = true;
        let mut out = vec![vec![0.0f32; 512]; 2];
        eng.render(&s, 0, 512, &mut out);
        assert!(out[0].iter().any(|x| x.abs() > 0.1), "the click plays");
        assert_eq!(eng.main_meter.peak, [0.0; 2], "but is not metered");
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
        // (about levels, not clip edges: no automatic edge fades)
        s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
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

    /// The surround panner's x and y lanes move the source as it plays (on the pan's grid), the
    /// stored position left alone; with automation Off the stored position plays.
    #[test]
    fn surround_lanes_move_the_source() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.outputs[0].format = ChannelFormat::Surround51;
        let tr = s.track_mut(t).unwrap();
        tr.mixer.surround = Some(SurroundPan::default());
        for (i, v) in [(0u8, 1.0f32), (1, -1.0)] {
            let mut lane = soundcraft_model::AutomationLane::new(AutoParam::Surround(i));
            lane.set_point(0, tr.mixer.surround.unwrap().axis(i));
            lane.set_point(2047, tr.mixer.surround.unwrap().axis(i));
            lane.set_point(2048, v);
            tr.automation.push(lane);
        }
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[2][100] - 0.5).abs() < 1e-4, "front centre first: {}", out[2][100]);
        assert!((out[5][3000] - 0.5).abs() < 1e-4 && out[2][3000].abs() < 1e-4, "then rear right: {}", out[5][3000]);
        assert_eq!(s.track(t).unwrap().mixer.surround.unwrap().x, 0.0, "the stored position is left alone");
        s.track_mut(t).unwrap().mixer.automation_mode = soundcraft_model::AutomationMode::Off;
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[2][3000] - 0.5).abs() < 1e-4, "automation off: as set");
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
