//! `fs_mod`: the owner's ModEngine (FireStation Engine 4), a faithful translation of
//! `Engine 4 - Mod/Source/ModEngineDSP.h`: fourteen component-level modulation circuits (BBD
//! choruses and flangers, phasers, tremolos and vibratos), each with its own variants.
//!
//! Wrapper (`PluginProcessor.cpp`, `ModConfig.h`, `Presets.h`, the editor) audit, everything that
//! affects the sound:
//! - `prepareToPlay` calls `dsp.prepare(sampleRate)` with the engine's OWN current knob values
//!   (its glides start there) and only then flags a full push: the first `processBlock` pushes
//!   every parameter, in parameter order, before processing; every later block pushes only the
//!   values that moved, again in parameter order. Here `set_param` stores the value and `process`
//!   pushes exactly the same way ([`ModEngine::push_params`]), so the order of `set_param` calls
//!   does not matter, as it does not in the C++. Until audio has flowed, `prepare` and `reset`
//!   put the engine back as a fresh instance makes it, in place, before preparing (what the
//!   render tool measures). After that they are `prepareToPlay` on the LIVE instance (the host
//!   resets on every Play / Stop / Locate and at bounce start): what moved is pushed first, as
//!   the C++ engine's `prepare` builds from its own knobs, then `dsp.prepare` clears the audio,
//!   zeroes the LFO and the tone counter, reseeds the noise, and seats every glide at its knob,
//!   so nothing glides from a default and two histories render the same. The host tempo is kept.
//! - APVTS ranges: floats 0..1, choices whole indices, bools 0/1. `ParamInfo::clamp` does the same
//!   (NaN becomes the default), so the engine never sees a non-finite value.
//! - Unused output channels are cleared (the Rust host contract leaves extra channels alone; the
//!   engine processes at most 2; mono runs the left path only, as the C++ does with one channel).
//! - No latency (`setLatencySamples` is never called). Tail: `getTailLengthSeconds` is 0.25 s.
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent; it only changes values below
//!   ~1e-38. `silence_tail_is_cheap` guards the CPU side.
//! - Link (`applyLink`, an AsyncUpdater) moves Level from Depth on the message thread: a derived
//!   parameter, as Dist's. [`link_level`] is its translated maths and [`derive`] runs it in the
//!   host's command layer after each edit that touches a parameter the C++ processor listens to
//!   (Depth, Link, the two set points, and every preset parameter: [`listened`]), so it is part of
//!   the same undoable edit, and a Level automation lane wins. Link also follows host automation,
//!   as in a C++ host: when a listened parameter reaches the plugin without a Level of its own (an
//!   automated Depth), the plugin applies Link to its own Level before the push (`follow_link`),
//!   unless Level has a lane (`set_param_automated`). A full push alone (after a prepare or a
//!   transport reset) re-evaluates Link only when a lane drives a listened parameter, so a seek
//!   never moves Level away from what the session stores. ModEngine has one set-point pair
//!   (`linkLo` / `linkHi`, Level at Depth 0 and 10), not one per circuit; the editor's
//!   calibration wizard (which writes them) is not ported, so the drawer shows them as knobs.
//! - The calibration wizard, preset dirty-tracking, A/B and the editor's undo are UI-only. The
//!   editor's rate readout is [`mod_engine_dsp::lfo_rate_hz`], which the graph uses too.
//!
//! Rust additions:
//! - The host transport (plan A3): `set_transport` hands the session tempo to the engine as the
//!   FireStation Delay engine takes its host's (`setHostTransport`; 20..999 BPM, else none), and
//!   while Sync is on a valid host tempo stands in for the Tempo knob. With no transport (the
//!   render tool has none) the C++ path is unchanged. The LFO free-runs as in the C++ (no phase
//!   lock to the song position); a seek resets it with the rest of the engine.
//! - Never-crash: a non-finite output sample is written as 0 (the C++ would pass it on).

// The C++ literals (6.2831853f, 0.7071f) are kept as written, not swapped for the constants.
#![allow(clippy::approx_constant)]

pub mod mod_config;
pub mod mod_engine_dsp;
mod presets;

#[cfg(test)]
mod tests;

pub use mod_engine_dsp::{ModEngineDsp, NUM_PARAMS};
pub use presets::PRESETS;

use crate::derive::Derive;
use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Transport};
use mod_engine_dsp as d;

/// The editor's dials read 0..10 (`formatTenths`: whole numbers).
fn tenths_text(v: f32) -> String {
    format!("{}", (v * 10.0).round() as i32)
}

/// `formatInputDb`: -12..+24 dB, and the 0.3333 default prints as 0.0 rather than -0.0.
fn input_text(v: f32) -> String {
    let mut db = d::knob_to_trim_db(v);
    if db.abs() < 0.05 {
        db = 0.0;
    }
    format!("{}{db:.1} dB", if db >= 0.0 { "+" } else { "" })
}

/// `formatLevelDb`; the 0.7071 default (2 v^2 = 0.99998) prints as +0.0 rather than -0.0, as
/// `formatInputDb` does for its default.
fn level_text(v: f32) -> String {
    let g = d::knob_to_level_gain(v);
    let mut db = if g > 0.0 { 20.0 * g.log10() } else { -60.0 };
    if db.abs() < 0.05 {
        db = 0.0;
    }
    format!("{}{db:.1} dB", if db >= 0.0 { "+" } else { "" })
}

/// `formatBpm`.
fn bpm_text(v: f32) -> String {
    format!("{} bpm", d::knob_to_bpm(v).round() as i32)
}

/// The drawer's tabs: the performance dials and the circuit; the circuit's trims; the LFO.
const PEDAL: &str = "Pedal";
const VOICING: &str = "Voicing";
const LFO: &str = "LFO";

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static MOD_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob01("depth", "Depth", 0.5, PEDAL, tenths_text),
    knob01("rate", "Rate", 0.5, PEDAL, tenths_text),
    knob01("level", "Level", 0.7071, PEDAL, level_text),
    fs_choice(
        "circuit",
        "Circuit",
        0.0,
        PEDAL,
        &[
            "Dim D",
            "CE-1",
            "CE-2",
            "Small Clone",
            "JC-120",
            "Brigade",
            "Mistress",
            "A/DA",
            "Phase 90",
            "Small Stone",
            "Uni-Vibe",
            "Opto Trem",
            "Harmonic",
            "Magnatone",
        ],
    ),
    // Relabelled per circuit by the editor (and the drawer).
    fs_choice("variant", "Variant", 1.0, PEDAL, &["A", "B", "C", "D", "E", "F"]),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    knob01("trim", "Input Trim", 0.3333, PEDAL, input_text),
    fs_toggle("link", "Link", 0.0, PEDAL),
    knob01("linkLo", "Level@0", 0.7071, PEDAL, tenths_text),
    knob01("linkHi", "Level@10", 0.7071, PEDAL, tenths_text),
    knob01("mix", "Mix", 0.5, PEDAL, tenths_text),
    knob01("width", "Width", 0.5, PEDAL, tenths_text),
    knob01("regen", "Regen", 0.0, VOICING, tenths_text),
    knob01("drive", "Drive", 0.0, VOICING, tenths_text),
    knob01("noise", "Noise", 0.35, VOICING, tenths_text),
    knob01("bias", "Bias", 0.5, VOICING, tenths_text),
    knob01("tone", "Tone", 0.5, VOICING, tenths_text),
    fs_choice("stereo", "Input Mode", 1.0, VOICING, &["Mono", "Stereo"]),
    fs_choice("shape", "Shape", 0.0, LFO, &["Stock", "Sine", "Triangle", "Trapezoid", "Square"]),
    fs_choice("sync", "Sync", 0.0, LFO, &["Free", "Sync"]),
    fs_choice("division", "Division", 5.0, LFO, &["1 bar", "1/1", "1/2d", "1/2", "1/4d", "1/4", "1/8", "1/16"]),
    knob01("tempo", "Tempo", 0.4, LFO, bpm_text),
];

pub static MOD_INFO: PluginInfo = PluginInfo {
    id: "fs_mod",
    name: "Mod",
    short_name: "Mod",
    category: Category::FireStation,
    params: &MOD_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &MOD_INFO,
    make: || Box::new(ModEngine::new()),
    presets: PRESETS,
    order: 80,
    fidelity: Some(super::Fidelity { name: "mod", source_dir: "Engine 4 - Mod", render_tool: "ModEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// Station's key knobs for Mod (`ModuleRegistry.cpp`).
pub const KEY_KNOBS: [&str; 4] = ["rate", "depth", "mix", "level"];

/// `linkWeight`: the saturating curve the engines share, pinned at Depth 0 and Depth 10.
pub fn link_weight(depth: f32) -> f32 {
    let t = depth.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powf(2.2)
}

/// `applyLink` (calibration aside): the Level that Link derives from Depth between the two set
/// points, or `None` when it leaves Level alone (Link off, no usable calibration, or already
/// there).
pub fn link_level(link: f32, depth: f32, lo: f32, hi: f32, level: f32) -> Option<f32> {
    if link < 0.5 {
        return None;
    }
    // Both set points at zero: no usable calibration, and driving Level from them would mute it.
    if lo < 0.001 && hi < 0.001 {
        return None;
    }
    let target = lo + (hi - lo) * link_weight(depth);
    if (target - level).abs() < 0.0005 {
        return None;
    }
    Some(target)
}

/// The parameters whose changes the C++ processor listens to: Link's inputs (Depth, Link, the set
/// points) and everything a preset captures (`getPresetParams`). Any of them starts the async
/// update that runs `applyLink`; Level, Input trim, Bypass and Tempo do not.
pub fn listened(id: &str) -> bool {
    matches!(
        id,
        "depth"
            | "link"
            | "linkLo"
            | "linkHi"
            | "circuit"
            | "variant"
            | "rate"
            | "mix"
            | "width"
            | "regen"
            | "drive"
            | "noise"
            | "bias"
            | "tone"
            | "stereo"
            | "shape"
            | "sync"
            | "division"
    )
}

/// `applyLink`'s decision from parameter values (`get`): the Level Link sets, or `None`. Shared by
/// the command layer ([`derive`]) and the plugin itself (automation).
pub fn link_target(get: impl Fn(&str) -> f32) -> Option<f32> {
    link_level(get("link"), get("depth"), get("linkLo"), get("linkHi"), get("level"))
}

/// ModEngine's derived parameters, run after each edit (a control, a command, a preset): when a
/// listened parameter changed, Link (`handleAsyncUpdate` -> `applyLink`).
pub fn derive(d: &mut Derive<'_>) {
    if !d.changed().iter().any(|id| listened(id)) {
        return; // nothing the processor listens to moved: no async update
    }
    if let Some(target) = link_target(|id| d.get(id)) {
        d.set("level", target);
    }
}

/// The ModEngine port.
///
/// `readouts`: `[0]` the LFO phase (0..1, left side; the right runs Width x 0.5 cycles ahead),
/// `[1]` the LFO rate in Hz the engine is running at (`getRateHz`), `[2]` the LFO shape factor
/// in use (0 sine, 1 triangle, > 1 trapezoid towards square).
pub struct ModEngine {
    p: Params,
    dsp: Box<ModEngineDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    /// Audio has gone through since `new`: `prepare` / `reset` take the live path.
    processed: bool,
    /// The host set Level since the last block (its own value wins over Link this block).
    level_from_host: bool,
    /// Which parameters an automation lane drives (`set_param_automated`): Link leaves a laned
    /// Level alone, and a full push re-evaluates Link only when a listened one is laned.
    automated: [bool; NUM_PARAMS],
}

impl Default for ModEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ModEngine {
    pub fn new() -> Self {
        ModEngine {
            p: Params::new(&MOD_PARAMS),
            dsp: Box::new(ModEngineDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            processed: false,
            level_from_host: false,
            automated: [false; NUM_PARAMS],
        }
    }

    /// Link following host automation, as Dist's: when a listened parameter moved since the last
    /// push and the host sent no Level of its own, Level follows Depth as `applyLink` would (the
    /// C++ listener runs on every host change). On a full push it is evaluated only when a lane
    /// drives a listened parameter (the stored values are already on Link's curve). Never over
    /// a Level lane. Allocation-free.
    fn follow_link(&mut self) {
        let from_host = std::mem::replace(&mut self.level_from_host, false);
        if self.automated[d::P_LEVEL] {
            return;
        }
        let evaluate = if self.force_push { self.listened_laned() } else { !from_host && self.moved_listened() };
        if !evaluate {
            return;
        }
        let p = &self.p;
        if let Some(target) = link_target(|id| p.get(id).unwrap_or(f32::NAN)) {
            self.p.set("level", target);
        }
    }

    /// A listened parameter is driven by an automation lane.
    fn listened_laned(&self) -> bool {
        MOD_PARAMS.iter().zip(self.automated.iter()).any(|(p, laned)| *laned && listened(p.id))
    }

    /// A listened parameter differs from its last push.
    fn moved_listened(&self) -> bool {
        MOD_PARAMS.iter().zip(self.last_pushed.iter()).enumerate().any(|(i, (p, last))| listened(p.id) && self.p.v(i) != *last)
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

    /// A fresh engine prepared at `sr`, in place, with every value pushed again at the next block.
    fn fresh(&mut self, sr: f64) {
        self.dsp.restore_initial();
        self.dsp.prepare(sr);
        self.last_pushed = [0.0; NUM_PARAMS];
        self.force_push = true;
    }

    /// `prepareToPlay` on the live instance at `sr`: what moved is pushed first (as the next block
    /// would), then the engine's `prepare` on its own knobs, and the C++'s full push at the next
    /// block (idempotent on a just-prepared engine).
    fn live(&mut self, sr: f64) {
        self.follow_link();
        self.push_params();
        self.dsp.prepare(sr);
        self.force_push = true;
    }

    fn prepare_at(&mut self, sr: f64) {
        if self.processed {
            self.live(sr);
        } else {
            self.fresh(sr);
        }
    }

    /// The engine (tests).
    #[cfg(test)]
    pub(crate) fn engine(&self) -> &ModEngineDsp {
        &self.dsp
    }
}

impl Plugin for ModEngine {
    fn info(&self) -> &'static PluginInfo {
        &MOD_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "level" {
            self.level_from_host = true;
        }
        self.p.set(id, value).is_some()
    }

    fn set_param_automated(&mut self, id: &str, automated: bool) {
        if let Some(i) = MOD_PARAMS.iter().position(|p| p.id == id) {
            self.automated[i] = automated;
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // Nothing to size: the BBD lines are fixed arrays inside the boxed engine, made in `new`.
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.prepare_at(sr);
    }

    fn reset(&mut self) {
        let sr = self.dsp.sample_rate();
        self.prepare_at(sr);
    }

    fn tail_samples(&self) -> usize {
        // `getTailLengthSeconds`: 0.25 s.
        (0.25 * self.dsp.sample_rate()) as usize
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.dsp.set_host_transport(transport.playing, transport.tempo as f32);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.processed = true;
        self.follow_link();
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
        out[0] = self.dsp.lfo_phase();
        out[1] = self.dsp.get_rate_hz();
        out[2] = self.dsp.lfo_shape();
    }
}
