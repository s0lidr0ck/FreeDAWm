#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! SoundCraft DSP.
//!
//! Realtime plugins (EQ, dynamics, reverb, delay, modulation, harmonic, pitch, utilities and three
//! instruments), metering (peak, RMS, ITU-R BS.1770 loudness), spectrum analysis and offline
//! Offline (Process menu) whole-buffer processing.
//!
//! Every algorithm here is an original implementation of textbook DSP (RBJ cookbook biquads,
//! feed-forward log-domain dynamics, feedback-delay-network reverbs, WSOLA, windowed-sinc
//! resampling). Plugins never panic on any parameter value: NaN falls back to the default and
//! infinities clamp to the range. `Plugin::process` never allocates; buffers are sized in
//! `Plugin::prepare`.
//!
//! Audio is planar `f32`: `io[channel][frame]`.

pub mod analysis;
pub mod beat;
pub mod biquad;
pub mod derive;
pub mod link;
pub mod meter;
pub mod offline;
mod osc;
pub mod pan;
mod params;
pub mod pitch_detect;
pub mod plugins;
pub mod scout;
pub mod spectrum;
mod util;

pub use derive::{Derive, DeriveFn, ParamEdit, param_deriver};
pub use link::{LinkRole, LinkSlab, LinkSpec, MAX_LINK_WORDS, link_role};
pub use offline::FadeShape;
pub use osc::Waveform;
pub use plugins::eq::{eq1_response, eq7_response};

use std::sync::Arc;

/// Plugin category, used to group the insert menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Category {
    Eq,
    Dynamics,
    PitchShift,
    Reverb,
    Delay,
    Modulation,
    Harmonic,
    Dither,
    Instrument,
    /// The owner's FireStation engines.
    FireStation,
    Other,
}

/// Display unit of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum Unit {
    None,
    Db,
    Hz,
    Ms,
    Percent,
    Ratio,
    Semitones,
    Cents,
    Seconds,
    Toggle,
    Choice,
}

/// How a control maps its 0..1 travel onto the parameter range.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum Taper {
    Linear,
    Log,
}

/// Static description of one plugin parameter.
// `display` is compared by address; two equal tables share the same fn items, which is all `==` is used for.
#[allow(unpredictable_function_pointer_comparisons)]
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ParamInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: Unit,
    pub taper: Taper,
    /// Labels for `Unit::Choice` parameters (value is the index).
    pub choices: &'static [&'static str],
    /// Optional custom value formatter (takes precedence over `unit` in `format`).
    #[serde(skip)]
    pub display: Option<fn(f32) -> String>,
    /// UI group the parameter belongs to (`""` = ungrouped).
    pub group: &'static str,
}

impl ParamInfo {
    /// Clamps any value (including NaN/inf) into this parameter's legal range.
    /// NaN becomes the default; toggles and choices round to whole numbers.
    pub fn clamp(&self, value: f32) -> f32 {
        let v = if value.is_nan() { self.default } else { value };
        let v = v.clamp(self.min, self.max);
        match self.unit {
            Unit::Toggle | Unit::Choice => v.round().clamp(self.min, self.max),
            _ => v,
        }
    }

    /// Maps a value to 0..1 control travel honoring the taper.
    pub fn to_normalized(&self, value: f32) -> f32 {
        let v = self.clamp(value);
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        match self.taper {
            Taper::Log if self.min > 0.0 => ((v / self.min).ln() / (self.max / self.min).ln()).clamp(0.0, 1.0),
            _ => ((v - self.min) / span).clamp(0.0, 1.0),
        }
    }

    /// Maps 0..1 control travel back to a value honoring the taper.
    pub fn from_normalized(&self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let v = match self.taper {
            Taper::Log if self.min > 0.0 => self.min * (self.max / self.min).powf(t),
            _ => self.min + (self.max - self.min) * t,
        };
        self.clamp(v)
    }

    /// Human-readable label for a value (choice label, On/Off, or number with unit).
    pub fn format(&self, value: f32) -> String {
        let v = self.clamp(value);
        if let Some(display) = self.display {
            return display(v);
        }
        match self.unit {
            Unit::Choice => self.choices.get(v as usize).map(|s| (*s).to_string()).unwrap_or_else(|| format!("{v}")),
            Unit::Toggle => if v >= 0.5 { "On" } else { "Off" }.to_string(),
            Unit::Db => {
                if v <= -95.9 {
                    "-inf dB".to_string()
                } else {
                    format!("{v:.1} dB")
                }
            }
            Unit::Hz => {
                if v >= 1000.0 {
                    format!("{:.2} kHz", v / 1000.0)
                } else {
                    format!("{v:.1} Hz")
                }
            }
            Unit::Ms => format!("{v:.1} ms"),
            Unit::Percent => format!("{v:.0}%"),
            Unit::Ratio => format!("{v:.1}:1"),
            Unit::Semitones => format!("{v:.0} st"),
            Unit::Cents => format!("{v:.0} ct"),
            Unit::Seconds => format!("{v:.2} s"),
            Unit::None => format!("{v:.2}"),
        }
    }
}

/// Static description of a plugin.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PluginInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// At most 8 characters, for insert slots.
    pub short_name: &'static str,
    pub category: Category,
    pub params: &'static [ParamInfo],
    pub is_instrument: bool,
    /// Usable as an offline process (the Process menu).
    pub audiosuite: bool,
    /// Takes a sidechain key from another track (`Plugin::process_with_sidechain`); the host
    /// offers a sidechain source for it.
    pub sidechain: bool,
    /// Takes part in session-level instance links (`Insert::link_group`) and how: the host
    /// offers a link group for it and hands it the group's [`LinkSlab`] (`Plugin::set_link`).
    /// `None`: no links.
    pub link: Option<LinkSpec>,
    /// Reference audio slots (0..=4) the host offers, handed over as assets keyed
    /// [`REFERENCE_KEYS`]`[slot]` (`Plugin::set_asset`): a file decoded and resampled to the
    /// session rate off the audio thread, read by the plugin at the transport position
    /// (`Transport::samples`).
    pub references: u8,
    /// Emits MIDI while processing ([`Plugin::midi_out`]): the host offers the module as a MIDI
    /// source for instrument and MIDI tracks (`Route::Modules`). False for plugins that never do.
    pub midi_out: bool,
}

/// Asset keys of the reference audio slots ([`PluginInfo::references`]). The session model uses
/// the same names (`soundcraft_model::REFERENCE_KEYS`).
pub const REFERENCE_KEYS: [&str; 4] = ["ref0", "ref1", "ref2", "ref3"];

/// The stock effect categories kept out of every plugin list (the picker, its search, the
/// plugin browser): the FireStation engines cover them. Their plugins still load from sessions,
/// open from the chain and can be added by id (`mix.module_add`).
pub const HIDDEN_STOCK: &[Category] = &[
    Category::Eq,
    Category::Dynamics,
    Category::PitchShift,
    Category::Reverb,
    Category::Delay,
    Category::Modulation,
    Category::Harmonic,
    Category::Dither,
];

impl PluginInfo {
    /// Whether the plugin lists show this plugin (not one of the [`HIDDEN_STOCK`] categories).
    pub fn listed(&self) -> bool {
        !HIDDEN_STOCK.contains(&self.category)
    }

    /// Looks up a parameter description by id.
    pub fn param(&self, id: &str) -> Option<&'static ParamInfo> {
        self.params.iter().find(|p| p.id == id)
    }
}

/// The host's transport at the start of a block, for tempo-synced plugins.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transport {
    pub playing: bool,
    pub recording: bool,
    /// Quarter notes per minute.
    pub tempo: f64,
    pub numerator: u32,
    pub denominator: u32,
    /// Timeline position.
    pub samples: i64,
    pub seconds: f64,
    pub quarters: f64,
    /// Start of the current bar, in quarter notes, and its number (0 for the first bar).
    pub bar_start: f64,
    pub bar: i64,
}

/// Most events a module's MIDI output holds per block ([`MidiOut`]).
pub const MIDI_OUT_CAPACITY: usize = 64;

/// One MIDI 1.0 channel message a module emitted, at a sample offset within the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MidiOutEvent {
    pub offset: u32,
    pub msg: [u8; 3],
}

/// A module's MIDI output for one block ([`Plugin::midi_out`]): a fixed-capacity ring of
/// [`MIDI_OUT_CAPACITY`] events, made once per slot by the host and cleared per block. Full, it
/// drops the event and counts it. Never allocates.
#[derive(Debug, Clone)]
pub struct MidiOut {
    events: [MidiOutEvent; MIDI_OUT_CAPACITY],
    len: usize,
    dropped: u32,
}

impl Default for MidiOut {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiOut {
    pub const fn new() -> Self {
        MidiOut { events: [MidiOutEvent { offset: 0, msg: [0; 3] }; MIDI_OUT_CAPACITY], len: 0, dropped: 0 }
    }

    /// Forgets the events (and the drop count).
    pub fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }

    /// Adds an event; `false` (and counted) when the ring is full.
    pub fn push(&mut self, offset: u32, msg: [u8; 3]) -> bool {
        match self.events.get_mut(self.len) {
            Some(slot) => {
                *slot = MidiOutEvent { offset, msg };
                self.len += 1;
                true
            }
            None => {
                self.dropped = self.dropped.saturating_add(1);
                false
            }
        }
    }

    /// The events of this block, in the order they were pushed.
    pub fn events(&self) -> &[MidiOutEvent] {
        self.events.get(..self.len).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Events that did not fit since the last `clear`.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Copies `other`'s events here (replacing this ring's).
    pub fn copy_from(&mut self, other: &MidiOut) {
        self.events = other.events;
        self.len = other.len;
        self.dropped = other.dropped;
    }

    /// Moves every event of `other` here (those that fit) and clears `other`.
    pub fn take_from(&mut self, other: &mut MidiOut) {
        for e in other.events() {
            self.push(e.offset, e.msg);
        }
        other.clear();
    }
}

/// A realtime audio processor or instrument.
///
/// Contract: `process` never allocates or panics. It processes `min(frames, io[c].len())` frames
/// of at most the channel count given to `prepare`; extra channels are left untouched.
/// Instruments replace the buffer contents with their output.
pub trait Plugin: Send {
    fn info(&self) -> &'static PluginInfo;
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);
    fn reset(&mut self);
    /// Sets a parameter (clamped into range; NaN becomes the default). `false` if `id` is unknown.
    fn set_param(&mut self, id: &str, value: f32) -> bool;
    fn param(&self, id: &str) -> Option<f32>;
    /// In-place processing of `channels` planar buffers each `frames` long.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize);
    /// [`Plugin::process`] with a sidechain key, for plugins whose [`PluginInfo::sidechain`] is
    /// true. `key` is `None` when no source is chosen (the plugin keys from its own input), else
    /// one buffer per prepared channel (each at least `frames` long; read the first `frames`),
    /// already aligned in time with `io`. A source that is muted, deleted, inactive or would
    /// form a routing cycle gives a silent key, not `None`. Same contract as `process`: never
    /// allocates or panics. The default ignores the key.
    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        let _ = key;
        self.process(io, frames);
    }
    /// Processing latency in samples (for delay compensation).
    fn latency(&self) -> usize {
        0
    }
    /// How long the output keeps ringing after the input stops, in samples.
    fn tail_samples(&self) -> usize {
        0
    }
    /// Gain reduction in dB for metering (a non-negative amount); 0 for non-dynamics plugins.
    fn gain_reduction_db(&self) -> f32 {
        0.0
    }
    /// Engine-specific live values for metering, written after `process`. What each index means
    /// is per plugin and documented on its impl; unused slots are left untouched (the caller
    /// fills them with NaN, "no reading"). Must not allocate.
    fn readouts(&self, _out: &mut [f32; 8]) {}
    /// Sample streams for the UI (spectrum, scope): by default the plugin pushes the mono sum of
    /// its input to `pre` and of its output to `post` in `process`. Created once (in
    /// `new`/`prepare`) and the same `Arc` every call. `None`: no streams.
    ///
    /// A port may feed them as its C++ editor does instead, documented on its impl: `fs_twine`
    /// carries its *input* `(L+R)/2` on `post` (the C++ scope's source) and leaves `pre` unfed.
    fn analysis(&self) -> Option<Arc<analysis::Analysis>> {
        None
    }
    /// The transport for the next block, given before `process`.
    fn set_transport(&mut self, _transport: &Transport) {}
    /// Whether the next `prepare` is for an offline render (a bounce, freeze or print, run as
    /// fast as the thread goes) rather than realtime playback. Hosted plugins pass it on (VST3
    /// `kOffline`, CLAP `clap.render`): disk-streaming instruments wait for their data instead
    /// of dropping it. Built-in plugins ignore it. Off the audio thread, before `prepare`.
    fn set_offline(&mut self, _offline: bool) {}
    /// Whether the host now drives `id` from an automation lane (called when that changes, before
    /// `set_param`). A plugin that derives parameters itself (FireStation Dist's Link) leaves a
    /// laned one alone: the lane wins. Must not allocate.
    fn set_param_automated(&mut self, _id: &str, _automated: bool) {}
    /// Instruments: note events at sample offsets within the next block.
    fn note_on(&mut self, _offset: usize, _note: u8, _velocity: u8) {}
    fn note_off(&mut self, _offset: usize, _note: u8) {}
    /// Instruments: any other MIDI 1.0 channel message (control change, pitch bend, pressure,
    /// program change) at a sample offset within the next block.
    fn midi(&mut self, _offset: usize, _msg: [u8; 3]) {}
    /// Instruments: a raw MIDI 1.0 channel message on any channel (the band's events: notes,
    /// controllers, pitch bend, program change). The default routes `0x9n` with velocity > 0 to
    /// [`Plugin::note_on`], `0x8n` and `0x9n` with velocity 0 to [`Plugin::note_off`] and anything
    /// else to [`Plugin::midi`], dropping the channel; plugin hosts override it to keep the
    /// channel. Clip playback keeps using `note_on`/`note_off`/`midi` (channel 1). Audio thread:
    /// must not allocate.
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        let [status, d1, d2] = msg;
        match status & 0xF0 {
            0x90 if d2 > 0 => self.note_on(offset, d1, d2),
            0x80 | 0x90 => self.note_off(offset, d1),
            _ => self.midi(offset, msg),
        }
    }
    fn all_notes_off(&mut self) {}
    /// The MIDI the module emitted while processing the block just run (a drum gate's note per
    /// hit): called by the host right after `process` for every chain module whose
    /// [`PluginInfo::midi_out`] is true; `out` is the host's ring for this slot, already cleared.
    /// Offsets are within that block; the channel is kept. Audio thread: must not allocate. The
    /// default emits nothing.
    fn midi_out(&mut self, _out: &mut MidiOut) {}
    /// Sample-playing instruments (the Sampler): the recorded audio to play, `None` to clear it.
    /// `false` when the plugin does not play samples. Does not allocate; sounding voices stop.
    /// The previous sample is dropped here, so hosts keep their own reference to it (the
    /// session's source pool does) to keep deallocation off the audio thread.
    fn set_sample(&mut self, _sample: Option<Arc<dyn SampleSource>>) -> bool {
        false
    }
    /// A keyed asset the plugin works from (FireStation Amp's cabinet impulse responses, one per
    /// amp: `irPathTwin`, `irPathJtm`, …), `None` to clear it; `false` when the plugin has no such
    /// key. Made and prepared off the audio thread by the host; the plugin only swaps it in (no
    /// allocation, no I/O). The plugin never drops the last reference of an asset: what it lets
    /// go of (the one replaced here, one a crossfade has finished with) comes back out of
    /// [`Plugin::take_released_asset`] for the host to drop off the audio thread.
    fn set_asset(&mut self, _key: &str, _asset: Option<Arc<dyn SampleSource>>) -> bool {
        false
    }
    /// The value bus of the link group this module is in (`Insert::link_group`), `None` when it
    /// is in none, and this module's member index in the group (stable while it stays; see
    /// [`link`]); given by the host when it syncs the session (never while rendering), and again
    /// whenever the group, the slab (grown) or the index changes. A plugin keeps the `Arc` and
    /// reads and writes the slab while processing (lock-free). The host keeps its own reference to
    /// every slab it hands over until the plugin has let go, so dropping the old one here never
    /// frees it. Must not block. The default ignores it.
    fn set_link(&mut self, _link: Option<Arc<LinkSlab>>, _member: usize) {}
    /// A one-off action on the live instance (FireStation LevelUp's `"reset_delivery"`, its
    /// Deliver tab's Reset), sent by the host outside the session (`mix.module_action`): not a
    /// parameter, not undoable. `false` when the plugin has no such action. Must not allocate or
    /// block. The default has none.
    fn action(&mut self, _name: &str) -> bool {
        false
    }
    /// One asset the plugin no longer uses (see [`Plugin::set_asset`]); the host drops it off the
    /// audio thread. Call until `None`. Does not allocate.
    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        None
    }
    /// The plugin's complete internal state as an opaque blob (third-party plugins), as its
    /// [`Plugin::state_reader`] reads it. `None` when the plugin has no state beyond its
    /// parameters (built-ins) or the plugin refused. May allocate: never call it in steady-state
    /// audio processing.
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.state_reader()?.save_state()
    }
    /// Takes numbers the host worked out off the audio thread from the module's stored state
    /// ([`prepare_state`]): CleanQ's ceiling on its grid (empty: no ceiling). Copies them into the
    /// plugin's own storage: never allocates, blocks or panics, so the host may call it between
    /// blocks on the audio thread. `false` when the plugin has no such state.
    fn set_prepared_state(&mut self, _data: &[f64]) -> bool {
        false
    }
    /// Restores a blob from [`Plugin::save_state`]. `false` if unsupported or the plugin rejected
    /// it (the plugin keeps working with its previous state). The data is hostile input.
    fn load_state(&mut self, _data: &[u8]) -> bool {
        false
    }
    /// Opens the plugin's own editor window (GUI). Main (UI) thread only.
    fn open_editor(&mut self) -> Result<(), String> {
        Err("no editor".into())
    }
    /// Closes the editor opened with [`Plugin::open_editor`] (no-op when none is open).
    fn close_editor(&mut self) {}
    /// A handle to this instance's editor that can be used on the UI thread while the instance
    /// itself is processing on the audio thread (`None` when the plugin has no editor). The
    /// handle stays valid after the instance is dropped (its calls then do nothing).
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        None
    }
    /// Reads this instance's state from the UI thread (`None` when it has no state of its own).
    fn state_reader(&mut self) -> Option<Box<dyn PluginStateReader>> {
        None
    }
}

/// A plugin editor window `[left, top, right, bottom]` (screen pixels) moved, and when larger
/// shrunk, so all of it is inside the monitor work area `work`: one opened near a screen edge
/// slides in; one larger than the monitor keeps its top-left (title bar) in view at the largest
/// size that fits. A degenerate `work` leaves the window as it is (spec 2026-11-02 item 6).
pub fn fit_window(win: [i32; 4], work: [i32; 4]) -> [i32; 4] {
    let [l, t, r, b] = win;
    let [wl, wt, wr, wb] = work;
    if wr <= wl || wb <= wt {
        return win;
    }
    let w = r.saturating_sub(l).clamp(0, wr.saturating_sub(wl));
    let h = b.saturating_sub(t).clamp(0, wb.saturating_sub(wt));
    let x = l.min(wr.saturating_sub(w)).max(wl);
    let y = t.min(wb.saturating_sub(h)).max(wt);
    [x, y, x.saturating_add(w), y.saturating_add(h)]
}

/// A plugin's editor (GUI), driven from the main (UI) thread. Obtained from
/// [`Plugin::editor`]; the plugin instance may meanwhile run on the audio thread (plugin formats
/// allow GUI calls on the main thread concurrently with processing on the audio thread).
pub trait PluginEditor: Send {
    /// Opens (or raises) the editor window. Main thread only; an error says why it can't.
    fn open(&mut self) -> Result<(), String>;
    /// Closes the editor window.
    fn close(&mut self);
    fn is_open(&self) -> bool;
    /// Call every UI frame while open: services the plugin's GUI requests, notices a window the
    /// user closed, and returns parameter edits made in the editor as `(param id, value)` in
    /// SoundCraft units (ids and ranges of [`PluginInfo::params`]).
    fn idle(&mut self) -> Vec<(String, f32)>;
    /// Tells the editor that a parameter changed elsewhere (generic editor, automation), so the
    /// GUI shows it. Values are in SoundCraft units.
    fn set_param(&mut self, _id: &str, _value: f32) {}
    /// The plugin's own text for a parameter value (SoundCraft units) when SoundCraft's
    /// [`ParamInfo::format`] would be wrong for it. Main thread only; `None` means "format it
    /// yourself".
    fn value_text(&mut self, _id: &str, _value: f32) -> Option<String> {
        None
    }
    /// A picture of the open editor as `(width, height, RGBA pixels)`, row by row from the top.
    /// Main thread only; never touches the audio thread. `None` when it is closed, or when this
    /// editor (or platform) cannot be captured.
    fn capture(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        None
    }
}

/// Reads a plugin's state on the main (UI) thread while the instance processes on the audio
/// thread, as plugin formats allow. Obtained from [`Plugin::state_reader`]; closures are readers.
pub trait PluginStateReader: Send {
    /// What [`Plugin::save_state`] would return; `None` once the instance is gone.
    fn save_state(&mut self) -> Option<Vec<u8>>;
    /// Restores a [`Plugin::save_state`] blob into the running instance (a preset loaded in
    /// place: the instance, its editor and its audio keep going). `false` when unsupported, the
    /// instance is gone or the plugin refused it. Main thread. The data is hostile input.
    fn load_state(&mut self, _data: &[u8]) -> bool {
        false
    }
    /// Loads one of the plugin's own presets into the running instance, as its preset-discovery
    /// provider names it (CLAP `preset-load`: location kind 0 a file, 1 the plugin; the file and
    /// the key within it). `false` when unsupported, gone or refused. Main thread.
    fn load_preset(&mut self, _kind: u32, _location: Option<&str>, _load_key: Option<&str>) -> bool {
        false
    }
}

impl<F: FnMut() -> Option<Vec<u8>> + Send> PluginStateReader for F {
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self()
    }
}

/// Recorded audio a sample-playing instrument reads: planar `f32` at its own sample rate (the
/// instrument converts rates as it plays). Implementations must be cheap to read from the audio
/// thread: no locks, no I/O.
pub trait SampleSource: Send + Sync {
    /// Sample rate of the audio in Hz.
    fn sample_rate(&self) -> f32;
    fn num_channels(&self) -> usize;
    /// One channel's frames (`None` past the last channel).
    fn channel(&self, ch: usize) -> Option<&[f32]>;
    /// Frames per channel (the shortest channel, so every index below it is readable).
    fn frames(&self) -> usize {
        (0..self.num_channels()).filter_map(|c| self.channel(c)).map(<[f32]>::len).min().unwrap_or(0)
    }
    /// The concrete source, for a plugin that takes a richer, prepared asset (FireStation Amp's
    /// `ImpulseAsset` carries its partition spectra); `None` by default.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
}

/// A sample held in memory (tests, generated audio).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SampleBuffer {
    pub sample_rate: f32,
    pub channels: Vec<Vec<f32>>,
}

impl SampleSource for SampleBuffer {
    fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
    fn num_channels(&self) -> usize {
        self.channels.len()
    }
    fn channel(&self, ch: usize) -> Option<&[f32]> {
        self.channels.get(ch).map(Vec::as_slice)
    }
}

/// The plugin registry.
pub fn plugins() -> &'static [&'static PluginInfo] {
    &plugins::REGISTRY
}

/// The plugins the plugin lists show ([`PluginInfo::listed`]), in registry order.
pub fn listed_plugins() -> impl Iterator<Item = &'static PluginInfo> {
    plugins().iter().copied().filter(|p| p.listed())
}

/// Creates a plugin by id, prepared for 48 kHz stereo with 1024-frame blocks.
/// Hosts should call `prepare` again with their real settings.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    let mut p = plugins::instantiate(id)?;
    p.prepare(48_000.0, 1024, 2);
    Some(p)
}

/// What a built-in plugin's stored state (`Insert::state`, as [`Plugin::load_state`] reads it)
/// works out to for [`Plugin::set_prepared_state`]: made off the audio thread (it allocates).
/// `None` when the plugin keeps no such state. The state is hostile input: a corrupt one gives
/// the plugin's own fallback (CleanQ: no ceiling), never an error the audio could trip on.
pub fn prepare_state(id: &str, state: &[u8]) -> Option<Vec<f64>> {
    plugins::fs::engine(id).and_then(|e| e.prepare_state).map(|f| f(state))
}

/// The actions a built-in plugin takes through [`Plugin::action`], by plugin id (empty for every
/// other plugin): the names the host may send it.
pub fn plugin_actions(id: &str) -> &'static [&'static str] {
    plugins::fs::engine(id).map_or(&[], |e| e.actions)
}

/// Looks up a plugin description by id.
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    plugins::REGISTRY.iter().chain(plugins::TEST_REGISTRY).copied().find(|i| i.id == id)
}

/// Whether a plugin makes sound instead of processing its input (it ignores the input).
pub fn is_generator(id: &str) -> bool {
    id == "signal_generator"
}

/// Silence floor used by the dB helpers.
pub const MIN_DB: f32 = -144.0;

/// Converts decibels to linear gain. Anything at or below -144 dB (or NaN) is silence.
pub fn db_to_gain(db: f32) -> f32 {
    if db.is_nan() || db <= MIN_DB { 0.0 } else { 10f32.powf(db.min(200.0) / 20.0) }
}

/// Converts linear gain (sign ignored) to decibels, floored at -144 dB.
pub fn gain_to_db(g: f32) -> f32 {
    let a = g.abs();
    if a.is_nan() {
        return MIN_DB;
    }
    if a.is_infinite() {
        return f32::MAX;
    }
    (20.0 * a.log10()).max(MIN_DB)
}

/// MIDI note number to frequency in Hz (equal temperament, A4 = note 69 = 440 Hz).
pub fn midi_to_hz(note: f64) -> f64 {
    440.0 * 2f64.powf((note - 69.0) / 12.0)
}

#[cfg(test)]
mod trait_default_tests {
    use super::*;

    #[test]
    fn stock_effects_are_hidden_but_still_load() {
        let listed = |id: &str| plugin_info(id).map(PluginInfo::listed);
        for id in ["eq_7band", "compressor", "room_reverb", "mod_delay", "chorus", "saturator", "pitch_shifter", "channel_strip", "dither"] {
            assert_eq!(listed(id), Some(false), "{id}");
            assert!(create(id).is_some(), "{id} still runs");
        }
        for id in ["gain", "trim", "invert", "dc_offset_removal", "time_shift", "signal_generator"] {
            assert_eq!(listed(id), Some(true), "{id}");
        }
        assert!(plugins().iter().filter(|p| p.category == Category::FireStation || p.is_instrument).all(|p| p.listed()));
        assert!(listed_plugins().all(|p| !HIDDEN_STOCK.contains(&p.category)));
        assert!(listed_plugins().any(|p| p.category == Category::FireStation));
    }

    #[test]
    fn built_ins_have_no_state_but_cleanqs_ceiling_and_no_editor() {
        for info in plugins() {
            let mut p = create(info.id).unwrap();
            if info.id == "fs_cleanq" {
                // CleanQ keeps its ceiling (`uiState/ceiling`) as its state; text without one
                // reads as no ceiling, as the C++ reads it.
                assert!(p.save_state().is_some_and(|s| s.starts_with(b"<ceiling")));
                assert!(p.load_state(b"anything") && !p.load_state(&[0xff]));
            } else {
                assert!(p.save_state().is_none(), "{}", info.id);
                assert!(!p.load_state(b"anything"), "{}", info.id);
            }
            assert!(p.open_editor().is_err(), "{}", info.id);
            p.close_editor();
            assert!(p.editor().is_none(), "{}", info.id);
        }
    }

    #[test]
    fn only_the_sampler_takes_samples() {
        for info in plugins() {
            let mut p = create(info.id).unwrap();
            let s: Arc<dyn SampleSource> = Arc::new(SampleBuffer { sample_rate: 48_000.0, channels: vec![vec![0.0; 16]] });
            assert_eq!(p.set_sample(Some(s)), info.id == "sampler", "{}", info.id);
            assert_eq!(p.set_sample(None), info.id == "sampler", "{}", info.id);
        }
    }

    #[test]
    fn sample_buffer_frames_is_the_shortest_channel() {
        let s = SampleBuffer { sample_rate: 44_100.0, channels: vec![vec![0.0; 10], vec![0.0; 7]] };
        assert_eq!(s.frames(), 7);
        assert_eq!(SampleBuffer::default().frames(), 0);
    }

    #[test]
    fn midi_out_ring_drops_when_full_and_keeps_order() {
        let mut r = MidiOut::new();
        assert!(r.is_empty());
        for i in 0..100u32 {
            let ok = r.push(i, [0x99, 36, 100]);
            assert_eq!(ok, (i as usize) < MIDI_OUT_CAPACITY);
        }
        assert_eq!(r.len(), MIDI_OUT_CAPACITY);
        assert_eq!(r.dropped(), 100 - MIDI_OUT_CAPACITY as u32);
        assert_eq!(r.events()[5].offset, 5);
        let mut other = MidiOut::new();
        other.take_from(&mut r);
        assert!(r.is_empty());
        assert_eq!(r.dropped(), 0);
        assert_eq!(other.len(), MIDI_OUT_CAPACITY);
        r.clear();
        assert_eq!(r.dropped(), 0);
    }
}

#[cfg(test)]
mod fit_window_tests {
    use super::fit_window;

    #[test]
    fn editor_windows_slide_onto_the_monitor_and_shrink_when_too_big() {
        let work = [0, 0, 1920, 1040];
        assert_eq!(fit_window([1800, 900, 2400, 1300], work), [1320, 640, 1920, 1040]);
        assert_eq!(fit_window([-3000, -20, -2400, 380], work), [0, 0, 600, 400]);
        assert_eq!(fit_window([100, 100, 2500, 1500], work), work);
        // A second monitor left of the first.
        assert_eq!(fit_window([-100, 50, 500, 450], [-1920, 0, 0, 1080]), [-600, 50, 0, 450]);
        assert_eq!(fit_window([10, 10, 20, 20], [0, 0, 0, 0]), [10, 10, 20, 20]);
        assert_eq!(fit_window([i32::MIN, i32::MIN, i32::MAX, i32::MAX], work), work);
    }
}
