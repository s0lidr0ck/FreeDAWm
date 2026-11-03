//! `fs_delay`: the owner's DelayEngine (FireStation Engine 6), a faithful translation of
//! `Engine 6 - Delay/Source/DelayEngineDSP.h`: one delay core with six voicings (Digital, Rack,
//! BBD, Tape, Drum, Oil Can), four routings (Mono, Stereo, Ping-Pong, Dual), host-synced note
//! values, tap tempo, freeze, reverse, a pitch shifter inside the loop, multi-head and rhythm
//! pattern taps.
//!
//! Wrapper (`PluginProcessor.cpp`, `DelayConfig.h`, `Presets.h`, the editor) audit, everything
//! that affects the sound:
//! - The constructor makes a `DelayEngineDSP`, which prepares itself at 48 kHz with its default
//!   knob values; `prepareToPlay` calls `dsp.prepare(sampleRate)` (still the engine's own values:
//!   nothing has been pushed yet) and only then flags a full push. The first `processBlock` pushes
//!   every parameter in parameter order; every later block pushes only the values that moved, in
//!   parameter order ("a style change re-derives the loop and re-seats the delay pointers, so
//!   pushing it every block would stop the delay ever settling"). Here `set_param` stores the
//!   value and `process` pushes exactly that way ([`Delay::push_params`]). A `prepare` before any
//!   audio has run is that fresh instance (defaults, the constructor's 48 kHz prepare, the session-rate prepare,
//!   a full push at the next block): what the render tool and a host do once. Every later
//!   `prepare`, and `reset` (Play, Stop, Locate, a render's start), is `prepareToPlay` on the live
//!   instance: the values that moved are pushed, the audio state is cleared and the engine is
//!   prepared on its own settings ([`DelayEngineDsp::reset_live`]), so the first repeat after a
//!   locate is already in place (no swoop, crossfade or fade-in) and a tapped tempo is kept (there
//!   is no forced push to clear it). No allocation: the lines are allocated once, in `new`, at the
//!   C++'s fixed size.
//! - Host transport: `processBlock` calls `dsp.setHostTransport (playing, bpm)` at the top of every
//!   block, before the parameter push. [`Plugin::set_transport`] does the same with the session's
//!   `playing` and `tempo` (SoundCraft calls it before every `process`); the engine keeps only a
//!   tempo inside 20..999 BPM (else "none", 0). With Tempo Src on Host (the default) synced note
//!   values, Mod Sync and the R Note follow the session tempo; with no tempo the Manual BPM knob
//!   stands in, as in the C++. The C++ never reads the song position (ppq), so neither does this.
//! - APVTS ranges: floats 0..1, choices whole indices, bools 0/1. `ParamInfo::clamp` does the same
//!   (NaN becomes the default), so the engine never sees a non-finite value.
//! - Bypass (the engine's own) is a smoothed crossfade inside the DSP with Trails; kept, hidden in
//!   the UI (the module's bypass is used).
//! - Unused output channels are cleared by the C++ host contract; here the Rust contract leaves
//!   extra channels alone and the engine processes at most 2; one channel runs the left path only
//!   (`chR == nullptr`), as the C++ does on a mono bus.
//! - No latency. Tail: `getTailLengthSeconds` is 8 s.
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent; it only changes values below
//!   ~1e-38. `silence_tail_is_cheap` guards the CPU side.
//! - Tap: the editor holds Tap at 1 for 60 ms (a parameter is only sampled per block). After an
//!   accepted tap it sets Tempo Src to Manual, the Manual BPM to the tapped tempo and Sync on. The
//!   strip's drawer does both (`module_drawer`); a DAW automation lane on `tap` works as the
//!   C++'s footswitch does.
//! - The diagnostics log, preset dirty-tracking and A/B are UI-only.
//!
//! Rust additions:
//! - `readouts`: `[0]`/`[1]` the echo times (ms) the left/right lines run at, `[2]` the tempo in
//!   force (BPM), `[3]` the Threshold gate lamp, `[4]` the tap sequence, `[5]` the tapped BPM,
//!   `[6]` 1 while the tempo is the engine's own rather than the session's (Tempo Src Manual, or a
//!   tap forcing it), `[7]` the loop's own delay (`loopComp`, ms: diffusion and the shifter).
//! - Never-crash: a non-finite output sample is written as 0 (the C++ would pass it on).

// The C++ literals (0.7071, 0.6215) are kept as written.
#![allow(clippy::approx_constant)]

pub mod delay_config;
pub mod delay_engine_dsp;
mod presets;

#[cfg(test)]
mod tests;

pub use delay_engine_dsp::{DelayEngineDsp, NUM_PARAMS};
pub use presets::PRESETS;

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Transport};
use delay_config as dc;

/// The drawer's tabs: the pedal face, then the editor's own drawer tabs.
const DELAY: &str = "Delay";
const TONE: &str = "Tone";
const DYNAMICS: &str = "Dynamics";
const MOTION: &str = "Motion";
const RHYTHM: &str = "Rhythm";
const STEREO: &str = "Stereo";
const SETUP: &str = "Setup";

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static DELAY_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob01("time", "Time", 0.6215, DELAY, dc::time_text),
    knob01("feedback", "Feedback", 0.35, DELAY, dc::feedback_text),
    knob01("mix", "Mix", 0.3, DELAY, dc::percent_text),
    fs_choice("style", "Style", 0.0, DELAY, &dc::STYLES),
    fs_choice("routing", "Routing", 1.0, STEREO, &["Mono", "Stereo", "Ping-Pong", "Dual"]),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    knob01("trim", "Input", 0.3333, SETUP, dc::trim_text),
    knob01("level", "Level", 0.7071, SETUP, dc::level_text),
    fs_toggle("sync", "Sync", 0.0, DELAY),
    fs_choice("division", "Division", 6.0, DELAY, &dc::DIVISIONS),
    knob01("lowCut", "Low Cut", 0.3544, TONE, dc::low_cut_text),
    knob01("highCut", "High Cut", 0.8295, TONE, dc::high_cut_text),
    knob01("character", "Character", 0.3, DELAY, dc::tenths_text),
    knob01("modRate", "Mod Rate", 0.4537, MOTION, dc::mod_rate_text),
    knob01("modDepth", "Mod Depth", 0.0, MOTION, dc::tenths_text),
    knob01("offset", "R Offset", 0.5, STEREO, dc::offset_text),
    fs_choice("heads", "Heads", 0.0, DELAY, &["1", "2", "3", "4", "1+2", "1+3", "2+4", "All"]),
    knob01("diffusion", "Diffusion", 0.0, MOTION, dc::tenths_text),
    knob01("duck", "Duck", 0.0, DYNAMICS, dc::tenths_text),
    knob01("swing", "Swing", 0.5, STEREO, dc::swing_text),
    fs_choice("pitch", "Pitch", 0.0, MOTION, &["Off", "-Oct", "+4th", "+5th", "+Oct"]),
    fs_toggle("freeze", "Freeze", 0.0, DELAY),
    knob01("width", "Width", 0.5, STEREO, dc::width_text),
    fs_toggle("trails", "Trails", 1.0, SETUP),
    knob01("tempo", "Manual BPM", 0.4, SETUP, dc::tempo_text),
    fs_choice("tempoSrc", "Tempo Src", 0.0, SETUP, &["Host", "Manual"]),
    knob01("threshold", "Threshold", 0.0, DYNAMICS, dc::threshold_text),
    knob01("focus", "Attack Focus", 0.0, DYNAMICS, dc::focus_text),
    fs_toggle("tap", "Tap", 0.0, DELAY),
    fs_toggle("reverse", "Reverse", 0.0, DELAY),
    knob01("noise", "Noise", 0.0, TONE, dc::noise_text),
    knob01("filtMod", "Filter Sweep", 0.0, TONE, dc::filt_mod_text),
    fs_choice("pattern", "Pattern", 0.0, RHYTHM, &["Off", "Gallop", "Tresillo", "Triplet", "Clave", "Cascade"]),
    fs_toggle("modSync", "Mod Sync", 0.0, MOTION),
    fs_choice("divisionR", "R Note", 0.0, STEREO, &dc::DIVISIONS_R),
];

pub static DELAY_INFO: PluginInfo = PluginInfo {
    id: "fs_delay",
    name: "Delay",
    short_name: "Delay",
    category: Category::FireStation,
    params: &DELAY_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &DELAY_INFO,
    make: || Box::new(Delay::new()),
    presets: PRESETS,
    order: 120,
    fidelity: Some(super::Fidelity { name: "delay", source_dir: "Engine 6 - Delay", render_tool: "DelayEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// Station's key knobs for Delay (`ModuleRegistry.cpp`).
pub const KEY_KNOBS: [&str; 4] = ["time", "feedback", "mix", "level"];

/// The DelayEngine port. See the module docs for the wrapper audit and the readouts.
pub struct Delay {
    p: Params,
    dsp: Box<DelayEngineDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    sr: f64,
    /// Whether the engine has run since it was made fresh: until then a `prepare` or `reset` is
    /// still the fresh instance's first `prepareToPlay` (`create` prepares at 48 kHz, then the
    /// host prepares at its rate, before any audio), after it they act on the live instance.
    processed: bool,
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

impl Delay {
    pub fn new() -> Self {
        Delay {
            p: Params::new(&DELAY_PARAMS),
            dsp: Box::new(DelayEngineDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            sr: 48_000.0,
            processed: false,
        }
    }

    /// The top of `processBlock`: push only what moved (everything after a prepare), in parameter
    /// order.
    fn push_params(&mut self) {
        let force = std::mem::take(&mut self.force_push);
        for i in 0..NUM_PARAMS {
            let v = self.p.v(i);
            if force || v != self.last_pushed[i] {
                self.last_pushed[i] = v;
                self.dsp.set_parameter(i, v);
            }
        }
    }

    /// A fresh instance prepared at `sr`, in place: the constructor's engine (defaults, prepared
    /// at 48 kHz), `prepareToPlay`'s `prepare(sr)`, and a full push at the next block.
    fn fresh(&mut self, sr: f64) {
        self.sr = sr;
        self.dsp.restore_initial();
        self.dsp.prepare(sr);
        self.last_pushed = [0.0; NUM_PARAMS];
        self.force_push = true;
    }

    /// `prepareToPlay` on the live instance at `sr`: what moved is pushed first (as the next block
    /// would), then the engine clears its audio and prepares on its own settings.
    fn live_prepare(&mut self, sr: f64) {
        self.sr = sr;
        self.push_params();
        self.dsp.reset_live(sr);
    }

    /// The engine (tests).
    #[cfg(test)]
    pub(crate) fn engine(&self) -> &DelayEngineDsp {
        &self.dsp
    }
}

impl Plugin for Delay {
    fn info(&self) -> &'static PluginInfo {
        &DELAY_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.p.set(id, value).is_some()
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // Nothing to size: the lines are the C++'s fixed 524288 samples each, made in `new`.
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        if self.processed {
            self.live_prepare(sr);
        } else {
            self.fresh(sr);
        }
    }

    fn reset(&mut self) {
        let sr = self.sr;
        if self.processed {
            self.live_prepare(sr);
        } else {
            self.fresh(sr);
        }
    }

    fn tail_samples(&self) -> usize {
        // `getTailLengthSeconds`: 8 s.
        (8.0 * self.sr) as usize
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.dsp.set_host_transport(transport.playing, transport.tempo as f32);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.processed = true;
        self.push_params();
        let nc = self.channels.min(io.len());
        if nc == 0 {
            return;
        }
        self.dsp.process(io, nc, frames);
        for c in io.iter_mut().take(nc) {
            for s in c.iter_mut().take(frames) {
                if !s.is_finite() {
                    *s = 0.0;
                }
            }
        }
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let (l, r) = self.dsp.echo_ms();
        out[0] = l;
        out[1] = r;
        out[2] = self.dsp.bpm_now();
        out[3] = self.dsp.gate_out();
        out[4] = self.dsp.tap_seq() as f32;
        out[5] = self.dsp.tap_bpm_out();
        out[6] = if self.dsp.tempo_off_session() { 1.0 } else { 0.0 };
        out[7] = self.dsp.loop_comp_ms();
    }
}
