//! `fs_reverb`: the owner's ReverbEngine (FireStation Engine 7), a faithful translation of
//! `Engine 7 - Reverb/Source/ReverbEngineDSP.h`: one frame, six machines (Room and Hall FDNs, an EMT
//! 140 Plate, a three-spring tank, Cloud and Shimmer), a decay of up to 30 s, a free or note-synced
//! pre-delay, Freeze, and the 1970s / 1980s / Now colours.
//!
//! Wrapper (`PluginProcessor.cpp`, `Parameters.h`, `ReverbConfig.h`, the editor) audit, everything that
//! affects the sound:
//! - `prepareToPlay` calls `dsp.prepare (sampleRate)` on the engine's OWN current state, which for a
//!   fresh instance (the constructor already ran `prepare (48000.0)` on the defaults) is the defaults,
//!   and only then flags a full push: the first `processBlock` pushes every parameter, in parameter
//!   order, before processing; later blocks push only the values that moved, again in order. "Only
//!   what moved" matters: a Machine push restarts the 0.33 s flush. Here `set_param` stores the value
//!   and `process` pushes exactly the same way ([`ReverbEngine::push_params`]). `prepare` puts the
//!   engine back as a fresh instance makes it, in place ([`ReverbEngineDsp::restore_and_prepare`]:
//!   the constructor's `prepare (48000.0)`, then `prepare (sr)`), and flags the full push: exactly the
//!   wrapper's `prepareToPlay` on a new instance (what the render tool does). `reset` (every Play,
//!   Stop, Locate and the start of a bounce) is `prepareToPlay` on the LIVE instance: the values are
//!   pushed, then [`ReverbEngineDsp::reset_live`] runs the C++ `prepare` on the engine's own state:
//!   its parameter state is kept (its machine is already current, so there is no 0.33 s flush), and
//!   so are the filter designs `prepare` never touches (the output EQ and the colour converters, with
//!   their `lastLc` / `lastHc` / `coefColor`); the audio state (buffers, filter states, envelopes,
//!   positions, the RNG) is cleared and the glides start at their targets (a Rust choice: the C++
//!   leaves them mid-glide), so a reset renders the same whatever came before it. This also reproduces the C++ keeping the
//!   constructor's 48 kHz output-EQ and colour coefficients at another rate until those knobs move
//!   (`lastLc` / `lastHc` / `coefColor` already match the defaults), through `prepare` and `reset`.
//! - The host transport: `processBlock` hands the play head's `(playing, bpm)` to `setHostTransport`
//!   when the host gives a position; [`ReverbEngine::set_transport`] does the same with the session's
//!   tempo (20..999 BPM, else "none"). With Tempo Src on Host (the default) a synced pre-delay follows
//!   it; the render tool has no play head, so the fidelity cases run on the Manual BPM fallback.
//! - APVTS ranges: floats 0..1, choices whole indices, bools 0/1. `ParamInfo::clamp` does the same (NaN
//!   becomes the default), so the engine never sees a non-finite value.
//! - Channels: stereo or mono (`isBusesLayoutSupported`); mono runs the left path with x1 = x0. The
//!   engine processes at most 2 channels; more are left untouched.
//! - No latency (the dry path is untouched; `setLatencySamples` is never called).
//! - Tail: `getTailLengthSeconds` is a fixed 30 s. [`ReverbEngine::tail_samples`] reports that, or the
//!   tail the settings make when it is longer (see there).
//! - `ScopedNoDenormals` (flush-to-zero, denormals-are-zero): it changes the sound in one place, the
//!   pitch shifter's splice search, whose scores during near-silence are built from subnormal
//!   products; the engine reproduces it there (`daz` in the DSP). Elsewhere the engine's own `DN` bias
//!   keeps its loops out of the denormal range, and `silence_tail_is_cheap` guards the CPU side.
//! - The editor, preset dirty-tracking, A/B, undo and the diagnostics log are UI-only.
//!
//! Rust addition: a non-finite output sample is written as 0 (the render tool zeroes them too).

// The C++ literals (0.7071f) are kept as written, not swapped for the constants.
#![allow(clippy::approx_constant)]

mod presets;
pub mod reverb_config;
pub mod reverb_engine_dsp;

#[cfg(test)]
mod tests;

pub use presets::PRESETS;
pub use reverb_engine_dsp::{NUM_PARAMS, ReverbEngineDsp};

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Transport};
use reverb_config as rc;
use reverb_engine_dsp as d;

/// The editor's main row and its tabs (`tabsSeg`: Tone, Time, Motion, Dynamics, Setup).
const MAIN: &str = "Main";
const TONE: &str = "Tone";
const TIME: &str = "Time";
const MOTION: &str = "Motion";
const DYNAMICS: &str = "Dynamics";
const SETUP: &str = "Setup";

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static REVERB_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob01("decay", "Decay", 0.5041, MAIN, rc::decay_text),
    knob01("mix", "Mix", 0.3, MAIN, rc::mix_text),
    fs_choice("machine", "Machine", 1.0, MAIN, &rc::MACHINES),
    knob01("size", "Size", 0.5, MAIN, rc::percent_text),
    knob01("preDelay", "Pre-Delay", 0.2828, TIME, rc::pre_delay_text),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    knob01("level", "Level", 0.7071, SETUP, rc::level_text),
    knob01("lowCut", "Low Cut", 0.3544, TONE, rc::low_cut_text),
    knob01("highCut", "High Cut", 0.8295, TONE, rc::high_cut_text),
    knob01("damping", "Damping", 0.5981, TONE, rc::damping_text),
    knob01("bass", "Low Decay", 0.5, TONE, rc::bass_text),
    // Renamed per machine: Early / Zing / Dwell / Bloom / Shimmer.
    knob01("character", "Character", 0.5, MAIN, rc::tenths_text),
    knob01("diffusion", "Diffusion", 0.7, TONE, rc::percent_text),
    knob01("modRate", "Mod Rate", 0.4537, MOTION, rc::mod_rate_text),
    knob01("modDepth", "Mod Depth", 0.3, MOTION, rc::percent_text),
    knob01("width", "Width", 0.5, SETUP, rc::width_text),
    knob01("duck", "Duck", 0.0, DYNAMICS, rc::duck_text),
    knob01("swell", "Swell", 0.0, DYNAMICS, rc::swell_text),
    fs_toggle("freeze", "Freeze", 0.0, MAIN),
    fs_toggle("trails", "Trails", 1.0, SETUP),
    fs_choice("interval", "Interval", 0.0, MOTION, &["+Oct", "+5th", "-Oct"]),
    fs_toggle("sync", "Pre Sync", 0.0, TIME),
    fs_choice("division", "Pre Note", 3.0, TIME, &rc::DIVISIONS),
    knob01("tempo", "Manual BPM", 0.4, SETUP, rc::tempo_text),
    fs_choice("tempoSrc", "Tempo Src", 0.0, SETUP, &["Host", "Manual"]),
    fs_choice("color", "Color", 2.0, MAIN, &["1970s", "1980s", "Now"]),
    knob01("attack", "Attack", 0.0, TIME, rc::attack_text),
];

pub static REVERB_INFO: PluginInfo = PluginInfo {
    id: "fs_reverb",
    name: "Reverb",
    short_name: "Reverb",
    category: Category::FireStation,
    params: &REVERB_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &REVERB_INFO,
    make: || Box::new(ReverbEngine::new()),
    presets: PRESETS,
    order: 130,
    fidelity: Some(super::Fidelity { name: "reverb", source_dir: "Engine 7 - Reverb", render_tool: "ReverbEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// Station's key knobs for Reverb (`ModuleRegistry.cpp`).
pub const KEY_KNOBS: [&str; 4] = ["decay", "size", "preDelay", "mix"];

/// `getTailLengthSeconds`: the C++ reports a fixed 30 s, the floor of [`ReverbEngine::tail_samples`].
pub const CPP_TAIL_S: f32 = 30.0;

/// The longest tail [`ReverbEngine::tail_samples`] reports, seconds: what a frozen tail (which never
/// ends) reports. Callers cap it further (the offline renderer at 60 s).
pub const FROZEN_TAIL_S: f32 = 600.0;

/// The ReverbEngine port.
///
/// `readouts`: `[0]` input peak dBFS and `[1]` output peak dBFS over the last block, `[2]` the
/// pre-delay in force in ms (`preTarget`: follows the host tempo when synced), `[3]` the tempo the
/// sync uses (BPM), `[4]` the machine running (it changes at the flush's swap point), `[5]` the
/// smoothed freeze amount (0..1).
pub struct ReverbEngine {
    p: Params,
    dsp: Box<ReverbEngineDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    in_peak: f32,
    out_peak: f32,
}

impl Default for ReverbEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Linear peak to dBFS (silence reads -inf).
fn peak_db(pk: f32) -> f32 {
    if pk > 0.0 { 20.0 * pk.log10() } else { f32::NEG_INFINITY }
}

impl ReverbEngine {
    pub fn new() -> Self {
        ReverbEngine {
            p: Params::new(&REVERB_PARAMS),
            dsp: Box::new(ReverbEngineDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            in_peak: 0.0,
            out_peak: 0.0,
        }
    }

    /// The top of `processBlock`: push only what moved (everything after a prepare), in order.
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

    /// `prepareToPlay` on a new instance, in place: the engine as the constructor and
    /// `prepare (sr)` leave it, with every value pushed again at the next block.
    fn fresh(&mut self, sr: f64) {
        self.dsp.restore_and_prepare(sr);
        self.last_pushed = [0.0; NUM_PARAMS];
        self.force_push = true;
        self.in_peak = 0.0;
        self.out_peak = 0.0;
    }

    /// The engine (tests).
    #[cfg(test)]
    pub(crate) fn engine(&self) -> &ReverbEngineDsp {
        &self.dsp
    }
}

impl Plugin for ReverbEngine {
    fn info(&self) -> &'static PluginInfo {
        &REVERB_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.p.set(id, value).is_some()
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // Nothing to size: every buffer was made at its full (192 kHz) size in `new`.
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.fresh(sr);
    }

    fn reset(&mut self) {
        // The values the engine would get at the next block, first: a reset right after `prepare`
        // (an offline bounce) then keeps the session's machine instead of flushing to it.
        self.push_params();
        self.dsp.reset_live();
        self.force_push = true;
        self.in_peak = 0.0;
        self.out_peak = 0.0;
    }

    /// The C++'s fixed 30 s (`getTailLengthSeconds`), or, when longer, the tail the settings make:
    /// the slowest band's T60 (the Low Decay band can ring 2.5x the Decay) plus the pre-delay (the
    /// note at the tempo in force when synced) and the attack ramp. The Swell envelope only gates
    /// the send. The Shimmer's shifted path re-enters the loop and rings somewhat past the set
    /// RT60; the 30 s floor covers it at every setting short of the longest decays. A frozen tail
    /// never ends: [`FROZEN_TAIL_S`].
    fn tail_samples(&self) -> usize {
        let sr = self.dsp.sample_rate() as f32;
        let v = |i: usize| self.p.v(i);
        if v(d::P_FREEZE) > 0.5 {
            return (FROZEN_TAIL_S * sr) as usize;
        }
        let shape =
            rc::decay_shape(v(d::P_MACHINE).round() as i32, v(d::P_DECAY), v(d::P_BASS), v(d::P_DAMPING), v(d::P_COLOR).round() as i32, false, 0.0);
        let ring = shape.t60_mid.max(shape.t60_low);
        // The pre-delay the parameters ask for, as `updateControl` works it out.
        let pre_s = if v(d::P_SYNC) > 0.5 {
            let host = self.dsp.get_host_bpm();
            let bpm = if v(d::P_TEMPO_SRC).round() as i32 == 0 && host >= 20.0 { host } else { d::knob_to_bpm(v(d::P_TEMPO)) };
            d::division_beats(v(d::P_DIVISION).round() as i32) * 60.0 / bpm
        } else {
            0.001 * d::knob_to_pre_ms(v(d::P_PRE_DELAY))
        };
        let attack_s = 0.001 * d::knob_to_attack_ms(v(d::P_ATTACK));
        let s = (ring + pre_s + attack_s).max(CPP_TAIL_S) * sr;
        if s.is_finite() && s > 0.0 { s.min(FROZEN_TAIL_S * sr) as usize } else { (CPP_TAIL_S * sr) as usize }
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.dsp.set_host_transport(transport.playing, transport.tempo as f32);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.push_params();
        let nc = self.channels.min(io.len());
        if nc == 0 {
            return;
        }
        let n = io.iter().take(nc).map(Vec::len).min().unwrap_or(0).min(frames);
        let mut pk = 0.0f32;
        for c in io.iter().take(nc) {
            for s in c.iter().take(n) {
                pk = pk.max(s.abs());
            }
        }
        self.in_peak = pk;
        self.dsp.process(io, nc, n);
        let mut pk = 0.0f32;
        for c in io.iter_mut().take(nc) {
            for s in c.iter_mut().take(n) {
                if !s.is_finite() {
                    *s = 0.0;
                }
                pk = pk.max(s.abs());
            }
        }
        self.out_peak = pk;
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let sr = self.dsp.sample_rate() as f32;
        out[0] = peak_db(self.in_peak);
        out[1] = peak_db(self.out_peak);
        out[2] = (self.dsp.pre_target_samples() + 0.5).floor() / sr * 1000.0;
        out[3] = self.dsp.effective_bpm();
        out[4] = self.dsp.machine() as f32;
        out[5] = self.dsp.freeze_amount();
    }
}
