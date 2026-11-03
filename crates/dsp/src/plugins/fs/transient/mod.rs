//! `fs_transient`: the owner's Transient engine (attack and sustain, on anything), a faithful
//! translation of `Engine 10 - Transient/Source/TransientEngineDSP.h`, with the editor's text from
//! `TransientConfig.h` in [`transient_config`].
//!
//! Wrapper (`PluginProcessor.{h,cpp}`) audit, everything that affects the sound:
//! - `prepareToPlay` pushes every parameter (in parameter order) to the engine *first*, so
//!   `prepare` builds the detector for the selected source and starts every glide at its target,
//!   then calls `dsp.prepare (sampleRate)` and raises `forcePush`. [`Plugin::prepare`] does the
//!   same: push, prepare, force.
//! - `processBlock` pushes only what moved (`v != lastPushed[i]`, everything after a prepare), in
//!   parameter order, at the start of the block, then processes. A change to Source or a Focus
//!   recomputes filters, the Time knobs recompute the envelope coefficients. Here `set_param` only
//!   stores the value; `process` does the wrapper's push before handing the block to the engine.
//! - Latency: v2's 3 ms lookahead, `round(0.003 * sr)` samples (144 at 48 kHz), which
//!   `prepareToPlay` reports with `setLatencySamples (dsp.getLatencySamples())` right after the
//!   engine's prepare; [`Plugin::latency`] reports the engine's figure. It is reported always:
//!   bypassed, and with both amounts at zero (the output is then the input delayed, sample for
//!   sample). No oversampling. No tail beyond the delay (gain only).
//! - Bypass: the engine's own `bypass` parameter is a 15 ms glide to the (delayed) dry input that
//!   lands on it exactly (translated in the engine). It is kept in order but hidden (group ""): the
//!   UI uses the module's bypass.
//! - APVTS ranges: floats are 0..1, choices are whole indices, the bool 0/1. `ParamInfo::clamp`
//!   does the same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Bus layout: mono or stereo, in = out. `dsp.process` gets the channel count (at most 2); with
//!   one channel the engine detects and shapes that one. Extra channels are left untouched (the
//!   Rust host contract). The wrapper's clearing of unused outputs never applies (in = out).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. It only changes values below
//!   ~1e-38, far under the fidelity bar; `silence_tail_is_cheap` guards the CPU side.
//! - The preset dirty-tracking (`parameterChanged`), state save and the editor are UI-only. A
//!   preset carries the sound (source, attack, sustain, both times, both focuses, mix), not Output
//!   or Bypass (`getPresetParams`); the factory files hold exactly those keys.
//! - The C++ has no reset: here `reset` rebuilds the engine in place exactly as `new` made it
//!   ([`TransientEngineDsp::restart`]) and runs `prepareToPlay` at the same rate (the current
//!   parameters pushed first, then prepare, then the forced push; no allocation: the scope ring
//!   is re-filled and the delay line re-sized to the same length in place). After a seek the module renders bit for bit as a fresh one made with
//!   the same parameters and then prepared, the C++ wrapper's path: every glide starts at its
//!   target, none runs from the defaults. This is also `prepareToPlay` on the LIVE instance, as
//!   the host's transport reset (Play / Stop / Locate / bounce start) needs: everything the
//!   engine keeps besides its audio state is derived from the pushed parameters (detector,
//!   times, focus filters), so the in-place rebuild loses nothing, has no change-driven
//!   crossfade, and two histories render the same.
//!
//! Station (`ModuleRegistry.cpp`): `fixed ("transient", "Transient", "source", 0xFFC4E86A, ..,
//! list ("attack,sustain,mix,output"))`: model param Source, key knobs Attack, Sustain, Mix,
//! Output.

mod presets;
pub mod transient_config;
pub mod transient_engine_dsp;

pub use presets::PRESETS;
pub use transient_engine_dsp::{NUM_PARAMS, TransientEngineDsp};

use crate::analysis::Analysis;
use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, gain_to_db};
use std::sync::Arc;
use transient_engine_dsp as d;

/// The Source choices (`Parameters.h`), as `TransientConfig.h` names them.
pub const SOURCE_NAMES: [&str; 7] = ["Drums", "Percussion", "Guitar", "Piano", "Bass", "Vocal", "Full Mix"];
/// The Focus choices.
pub const FOCUS_NAMES: [&str; 4] = ["Full", "Low", "Mid", "High"];

/// The editor's `signedDb`: one decimal, a sign only when positive, never "-0.0 dB".
pub fn signed_db(db: f32) -> String {
    let db = if db.abs() < 0.05 { 0.0 } else { db };
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}

fn attack_text(v: f32) -> String {
    let db = d::knob_to_attack_db(v);
    if db.abs() < 0.05 { "0 dB".to_string() } else { signed_db(db) }
}

fn sustain_text(v: f32) -> String {
    let db = d::knob_to_sustain_db(v);
    if db.abs() < 0.05 { "0 dB".to_string() } else { signed_db(db) }
}

/// The editor's `formatScale`: "Source" at x1, else the multiplier.
fn scale_text(v: f32) -> String {
    let k = d::knob_to_time_scale(v);
    if (k - 1.0).abs() < 0.02 {
        "Source".to_string()
    } else if k > 1.0 {
        format!("x{k:.1}")
    } else {
        format!("x{k:.2}")
    }
}

fn percent_text(v: f32) -> String {
    format!("{} %", (v * 100.0).round() as i32)
}

fn output_text(v: f32) -> String {
    signed_db(d::knob_to_out_db(v))
}

/// The drawer shows the editor's one page: no tabs.
const MAIN: &str = "Transient";

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static TRANSIENT_PARAMS: [ParamInfo; NUM_PARAMS] = [
    fs_choice("source", "Source", 0.0, MAIN, &SOURCE_NAMES),
    knob01("attack", "Attack", 0.5, MAIN, attack_text),
    knob01("sustain", "Sustain", 0.5, MAIN, sustain_text),
    knob01("attackTime", "Attack Time", 0.5, MAIN, scale_text),
    knob01("sustainTime", "Sustain Time", 0.5, MAIN, scale_text),
    fs_choice("attackFocus", "Attack Focus", 0.0, MAIN, &FOCUS_NAMES),
    fs_choice("sustainFocus", "Sustain Focus", 0.0, MAIN, &FOCUS_NAMES),
    knob01("mix", "Mix", 1.0, MAIN, percent_text),
    knob01("output", "Output", 0.5, MAIN, output_text),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
];

pub static TRANSIENT_INFO: PluginInfo = PluginInfo {
    id: "fs_transient",
    name: "Transient",
    short_name: "Transnt",
    category: Category::FireStation,
    params: &TRANSIENT_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &TRANSIENT_INFO,
    make: || Box::new(Transient::new()),
    presets: PRESETS,
    order: 100,
    fidelity: Some(super::Fidelity { name: "transient", source_dir: "Engine 10 - Transient", render_tool: "TransientEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// The Transient port.
///
/// `readouts`: `[0]` input peak dBFS and `[1]` output peak dBFS over the last block (Rust-side
/// meters), `[2]` the attack gain and `[3]` the sustain gain applied to the last sample in dB
/// (`lastAttackDb`, `lastSustainDb`), `[4]` and `[5]` the most extreme attack and sustain gain of
/// the last block in dB (the scope bins' rule), `[6]` the attack detector's level (the attack
/// band's 1 ms mean square) and `[7]` the sustain level `L`, in dBFS.
///
/// `analysis().post` carries the C++ editor's scope (`TransientScope`): one entry per 4 ms bin as
/// four values, input peak, output peak (linear), attack gain and sustain gain (dB), pushed as the
/// engine finishes each bin. `pre` is not fed.
pub struct Transient {
    p: Params,
    dsp: Box<TransientEngineDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    analysis: Arc<Analysis>,
    in_peak_db: f32,
    out_peak_db: f32,
}

impl Default for Transient {
    fn default() -> Self {
        Self::new()
    }
}

impl Transient {
    pub fn new() -> Self {
        Transient {
            p: Params::new(&TRANSIENT_PARAMS),
            dsp: Box::new(TransientEngineDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            analysis: Analysis::new(),
            in_peak_db: f32::NAN,
            out_peak_db: f32::NAN,
        }
    }

    /// The top of `processBlock`: push only what moved (everything after a prepare), in
    /// parameter order.
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

    /// `prepareToPlay`: every parameter pushed first (so `prepare` builds the selected source's
    /// detector and starts every glide at its target), then the engine's prepare, then the next
    /// block's push forced.
    fn prepare_to_play(&mut self, sr: f64) {
        for i in 0..NUM_PARAMS {
            self.last_pushed[i] = self.p.v(i);
            self.dsp.set_parameter(i, self.last_pushed[i]);
        }
        self.dsp.prepare(sr);
        self.force_push = true;
    }

    /// The engine (tests).
    #[cfg(test)]
    pub(crate) fn engine(&self) -> &TransientEngineDsp {
        &self.dsp
    }
}

fn peak(io: &[Vec<f32>], nc: usize, frames: usize) -> f32 {
    io.iter().take(nc).flat_map(|c| c.iter().take(frames)).fold(0.0f32, |m, &x| if x.abs() > m { x.abs() } else { m })
}

fn lin_to_db(x: f64) -> f32 {
    gain_to_db(x as f32)
}

impl Plugin for Transient {
    fn info(&self) -> &'static PluginInfo {
        &TRANSIENT_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.p.set(id, value).is_some()
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // `prepareToPlay`: every parameter first, then the engine's prepare (it sizes the
        // lookahead's delay line; the scope ring is made in `new`), then force the next block's
        // push. The latency it reports is `latency()`.
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.prepare_to_play(sr);
        self.in_peak_db = f32::NAN;
        self.out_peak_db = f32::NAN;
    }

    fn reset(&mut self) {
        // A fresh engine (no allocation), then `prepareToPlay` at the same rate: the current
        // parameters first, so every glide starts at its target, as after the C++ prepare.
        let sr = self.dsp.sample_rate();
        self.dsp.restart();
        self.prepare_to_play(sr);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.push_params();
        let nc = self.channels.min(io.len());
        self.in_peak_db = gain_to_db(peak(io, nc, frames));
        self.dsp.process(io, nc, frames, Some(&self.analysis.post));
        self.out_peak_db = gain_to_db(peak(io, nc, frames));
    }

    fn latency(&self) -> usize {
        // `setLatencySamples (dsp.getLatencySamples())`
        usize::try_from(self.dsp.get_latency_samples()).unwrap_or(0)
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let (att, sus) = self.dsp.block_gains_db();
        let (e1, e1s) = self.dsp.envelopes();
        out[0] = self.in_peak_db;
        out[1] = self.out_peak_db;
        out[2] = self.dsp.last_attack_db();
        out[3] = self.dsp.last_sustain_db();
        out[4] = att;
        out[5] = sus;
        out[6] = lin_to_db(e1);
        out[7] = lin_to_db(e1s);
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}

#[cfg(test)]
mod tests;
