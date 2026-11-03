//! `fs_dist`: the owner's DistEngine (FireStation Engine 2), a faithful translation of
//! `Engine 2 - Dist/Source/DistEngineDSP.h`: twelve component-level pedal circuits.
//!
//! Wrapper (`PluginProcessor.cpp`, `PedalConfig.h`, the editor's preset load) audit, everything
//! that affects the sound:
//! - `prepareToPlay` calls `dsp.prepare(sampleRate)` with the engine's OWN current knob values
//!   (its glides start there) and only then flags a full push: the first `processBlock` pushes
//!   every parameter, in parameter order, before processing. Every later block pushes only the
//!   values that moved, again in parameter order. Here `set_param` stores the value and
//!   `process` pushes exactly the same way (`push_changed`), so the order of `set_param` calls
//!   does not matter, as it does not in the C++. Until audio has flowed, `prepare` and `reset`
//!   build a fresh engine (the C++ `prepare` is not idempotent and SoundCraft prepares more than
//!   once): a single prepare of a fresh C++ instance, which is what the render tool measures.
//!   After that they are `prepareToPlay` on the LIVE instance (the host resets on every Play /
//!   Stop / Locate and at bounce start): what moved is pushed first (the engine's `prepare`
//!   seats its glides at its own knobs), then [`DistEngineDsp::reset_live`] clears every audio
//!   state and prepares, and the full push follows at the next block. Nothing glides from a
//!   default and two histories render the same.
//! - APVTS ranges: floats 0..1, choices whole indices, bools 0/1. `ParamInfo::clamp` does the
//!   same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Unused output channels are cleared (the Rust host contract leaves extra channels alone; the
//!   engine processes at most 2). No latency (`setLatencySamples` is never called), no tail.
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent; it only changes values below
//!   ~1e-38. `silence_tail_is_cheap` guards the CPU side.
//! - Link (`applyLink`), the per-circuit knob memory (`applyPendingKnobSwap`) and a preset's
//!   matched Level (`applyMatchedLevel`) move parameters on the message thread: they are the
//!   plugin's derived parameters, [`derive`] (see `wrapper.rs`), run by the host's command layer.
//!   Link also follows host automation, as in a C++ host: when a listened parameter reaches the
//!   plugin without a Level of its own (an automated Drive), the plugin applies Link to its own
//!   Level before the push (`follow_link`), unless Level has an automation lane (the lane wins,
//!   `set_param_automated`). A full push alone (after a prepare or a transport reset) evaluates
//!   Link only when a lane drives one of its listened parameters: the stored values are already
//!   on Link's curve (the command layer keeps them there), so a seek never moves Level away from
//!   what the session stores; with a lane, Level follows the lane's value at the new position. Known limit: an automated Circuit does not trigger the knob swap
//!   (that needs the per-module memory, which lives with the host's document).
//! - The calibration wizard, preset dirty-tracking, A/B and the editor's undo are UI-only.
//! - The field diagnostics (a log file when `DIAGNOSE.txt` exists) do not touch the audio.
//!
//! Rust additions (never-crash): a non-finite output sample is written as 0 (the C++ would pass
//! it on; the render tool zeroes and reports them, and no reference has any). Tail: none, as
//! `getTailLengthSeconds` (0): nothing in the circuits rings for more than a few samples.

mod dist_engine_dsp;
mod presets;
pub mod wrapper;

pub use dist_engine_dsp::{DistEngineDsp, NUM_PARAMS};
pub use presets::PRESETS;
pub use wrapper::derive;

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, gain_to_db};

/// The editor's dials read 0..10 (`formatTenths`).
fn tenths_text(v: f32) -> String {
    format!("{:.1}", v * 10.0)
}

/// `formatInputDb`: -12..+24 dB, and the 0.3333 default prints as 0.0 rather than -0.0.
fn input_text(v: f32) -> String {
    let mut db = -12.0 + 36.0 * v;
    if db.abs() < 0.05 {
        db = 0.0;
    }
    format!("{}{db:.1} dB", if db >= 0.0 { "+" } else { "" })
}

/// The group every shown parameter is in: the drawer shows one panel, as the editor does.
const G: &str = "Pedal";

/// Parameters in the C++ order (`Parameters.h`), ids and defaults verbatim.
pub static DIST_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob01("drive", "Drive", 0.5, G, tenths_text),
    knob01("tone", "Tone", 0.5, G, tenths_text),
    knob01("level", "Level", 0.5, G, tenths_text),
    fs_choice(
        "circuit",
        "Circuit",
        0.0,
        G,
        &["Tube Screamer", "RAT", "Big Muff", "Fuzz Face", "DS-1", "Klon", "Blues Breaker", "Guv'nor", "Tube Driver", "Blues Driver", "Dist+", "OCD"],
    ),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    knob01("trim", "Input", 0.3333, G, input_text),
    fs_toggle("link", "Link", 0.0, G),
    // per-circuit Level@low / Level@high anchors (Link's set points)
    knob01("tsLo", "TS Lo", 0.7, G, tenths_text),
    knob01("tsHi", "TS Hi", 0.5, G, tenths_text),
    knob01("ratLo", "RAT Lo", 0.7, G, tenths_text),
    knob01("ratHi", "RAT Hi", 0.35, G, tenths_text),
    knob01("muffLo", "Muff Lo", 0.6, G, tenths_text),
    knob01("muffHi", "Muff Hi", 0.4, G, tenths_text),
    knob01("ffLo", "FF Lo", 0.6, G, tenths_text),
    knob01("ffHi", "FF Hi", 0.4, G, tenths_text),
    knob01("ds1Lo", "DS1 Lo", 0.6, G, tenths_text),
    knob01("ds1Hi", "DS1 Hi", 0.4, G, tenths_text),
    knob01("klonLo", "Klon Lo", 0.6, G, tenths_text),
    knob01("klonHi", "Klon Hi", 0.45, G, tenths_text),
    knob01("bbLo", "BB Lo", 0.6, G, tenths_text),
    knob01("bbHi", "BB Hi", 0.45, G, tenths_text),
    knob01("govLo", "Gov Lo", 0.6, G, tenths_text),
    knob01("govHi", "Gov Hi", 0.4, G, tenths_text),
    knob01("tdLo", "TD Lo", 0.6, G, tenths_text),
    knob01("tdHi", "TD Hi", 0.45, G, tenths_text),
    knob01("bdLo", "BD Lo", 0.6, G, tenths_text),
    knob01("bdHi", "BD Hi", 0.45, G, tenths_text),
    knob01("dpLo", "D+ Lo", 0.6, G, tenths_text),
    knob01("dpHi", "D+ Hi", 0.4, G, tenths_text),
    knob01("ocdLo", "OCD Lo", 0.6, G, tenths_text),
    knob01("ocdHi", "OCD Hi", 0.45, G, tenths_text),
    // per-circuit switches
    fs_choice("tsModel", "TS Model", 0.0, G, &["TS808", "TS9", "TS9 Late", "TS10", "DX Hot", "Mod"]),
    fs_choice("tsDiode", "TS Diode", 0.0, G, &["Stock", "1N914", "1N4148", "1S1588", "Asym", "LED", "Ge"]),
    fs_choice("ratModel", "RAT Model", 0.0, G, &["Vintage", "RAT2", "Turbo", "Dirty", "Fat"]),
    fs_choice("ratDiode", "RAT Diode", 0.0, G, &["Stock", "1N914", "LED", "Ge", "MOSFET"]),
    fs_choice("ratChip", "RAT Chip", 0.0, G, &["Stock", "LM308", "OP07", "LM741", "TL071"]),
    fs_choice("ratFat", "RAT Fat", 0.0, G, &["Off", "On"]),
    fs_choice("muffVer", "Muff Version", 1.0, G, &["Triangle", "Ram's Head", "Russian", "NYC"]),
    fs_choice("muffDiode", "Muff Diode", 0.0, G, &["Stock", "1N914", "LED", "Ge"]),
    fs_choice("muffMids", "Muff Mids", 0.0, G, &["Stock", "Mids"]),
    fs_choice("ffTrans", "FF Transistor", 0.0, G, &["Germanium", "Silicon"]),
    fs_choice("ffBias", "FF Bias", 1.0, G, &["Cold", "Stock", "Hot"]),
    fs_choice("ds1Diode", "DS1 Diode", 0.0, G, &["Stock", "LED", "Asym"]),
    fs_choice("ds1Voice", "DS1 Voice", 0.0, G, &["Stock", "Modded"]),
    fs_choice("klonDiode", "Klon Diode", 0.0, G, &["Germanium", "Silicon", "LED"]),
    fs_choice("bbGain", "BB Gain", 0.0, G, &["Stock", "Hi"]),
    fs_choice("govClip", "Gov Clip", 0.0, G, &["LED", "Si+LED"]),
    fs_choice("govBass", "Gov Bass", 1.0, G, &["Cut", "Flat", "Boost"]),
    fs_choice("govMid", "Gov Mid", 1.0, G, &["Cut", "Flat", "Boost"]),
    fs_choice("tdPlate", "TD Plate", 1.0, G, &["9 V", "12 V", "250 V"]),
    fs_choice("tdBias", "TD Bias", 1.0, G, &["Cold", "Stock", "Hot"]),
    fs_choice("bdMode", "BD Mode", 0.0, G, &["Stock", "Phat"]),
    fs_choice("dpDiode", "D+ Diode", 0.0, G, &["Germanium", "Silicon", "LED"]),
    fs_choice("ocdMode", "OCD Mode", 0.0, G, &["HP", "LP"]),
];

pub static DIST_INFO: PluginInfo = PluginInfo {
    id: "fs_dist",
    name: "Dist",
    short_name: "Dist",
    category: Category::FireStation,
    params: &DIST_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &DIST_INFO,
    make: || Box::new(Dist::new()),
    presets: PRESETS,
    order: 90,
    fidelity: Some(super::Fidelity { name: "dist", source_dir: "Engine 2 - Dist", render_tool: "DistEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// The DistEngine port.
///
/// `readouts`: `[0]` input peak dBFS and `[1]` output peak dBFS over the last block, `[2]` the
/// smoothed Drive knob (0..1), `[3]` the smoothed Level gain (`level²`), `[4]` the circuit
/// index the engine runs.
pub struct Dist {
    p: Params,
    dsp: Box<DistEngineDsp>,
    /// `lastPushed` and `forcePush` of the C++ processor.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    /// The host set Level since the last block (its own value wins over Link this block).
    level_from_host: bool,
    /// Which parameters an automation lane drives (`set_param_automated`): Link leaves a laned
    /// Level alone, and a full push re-evaluates Link only when a listened one is laned.
    automated: [bool; NUM_PARAMS],
    channels: usize,
    in_peak_db: f32,
    out_peak_db: f32,
    /// Audio has gone through since `new`: `prepare` / `reset` take the live path.
    processed: bool,
}

impl Default for Dist {
    fn default() -> Self {
        Self::new()
    }
}

impl Dist {
    pub fn new() -> Self {
        Dist {
            p: Params::new(&DIST_PARAMS),
            dsp: Box::new(DistEngineDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            level_from_host: false,
            automated: [false; NUM_PARAMS],
            channels: 2,
            in_peak_db: f32::NAN,
            out_peak_db: f32::NAN,
            processed: false,
        }
    }

    /// `prepareToPlay` on the live instance at `sr`: the top of the next block (Link following
    /// automation, then the push of what moved), the engine prepared on its own state, and the
    /// C++'s full push at the next block.
    fn live(&mut self, sr: f64) {
        self.follow_link();
        self.push_changed();
        self.dsp.reset_live(sr);
        self.force_push = true;
    }

    /// Link following host automation: when a listened parameter moved since the last push and
    /// the host sent no Level of its own, Level follows Drive as `applyLink` would (the C++
    /// listener runs on every host change). On a full push (after a prepare or a transport
    /// reset) it is evaluated only when a lane drives a listened parameter: otherwise the
    /// stored values are already on Link's curve (the command layer keeps them there) and the
    /// playing Level must stay the stored one after a seek. Never over a Level lane.
    /// Allocation-free.
    fn follow_link(&mut self) {
        let from_host = std::mem::replace(&mut self.level_from_host, false);
        if self.automated[dist_engine_dsp::P_LEVEL] {
            return;
        }
        let evaluate = if self.force_push { self.listened_laned() } else { !from_host && self.moved_listened() };
        if !evaluate {
            return;
        }
        let p = &self.p;
        if let Some(target) = wrapper::link_target(|id| p.get(id).unwrap_or(f32::NAN)) {
            self.p.set("level", target);
        }
    }

    /// A listened parameter is driven by an automation lane.
    fn listened_laned(&self) -> bool {
        DIST_PARAMS.iter().zip(self.automated.iter()).any(|(p, laned)| *laned && wrapper::listened(p.id))
    }

    /// A listened parameter (all but Level, Input trim, Bypass) differs from its last push.
    fn moved_listened(&self) -> bool {
        DIST_PARAMS.iter().zip(self.last_pushed.iter()).enumerate().any(|(i, (p, last))| wrapper::listened(p.id) && self.p.v(i) != *last)
    }

    /// `processBlock`'s push: every value that moved since it was last pushed (all of them after
    /// a prepare), in parameter order.
    fn push_changed(&mut self) {
        let force = std::mem::replace(&mut self.force_push, false);
        for (i, last) in self.last_pushed.iter_mut().enumerate() {
            let v = self.p.v(i);
            if force || v != *last {
                *last = v;
                self.dsp.set_parameter(i, v);
            }
        }
    }
}

impl Plugin for Dist {
    fn info(&self) -> &'static PluginInfo {
        &DIST_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "level" {
            self.level_from_host = true;
        }
        self.p.set(id, value).is_some()
    }

    fn set_param_automated(&mut self, id: &str, automated: bool) {
        if let Some(i) = DIST_PARAMS.iter().position(|p| p.id == id) {
            self.automated[i] = automated;
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        if self.processed {
            self.live(sr);
            return;
        }
        let mut dsp = Box::new(DistEngineDsp::new());
        dsp.prepare(sr);
        self.dsp = dsp;
        self.force_push = true; // re-push everything into the fresh DSP state
    }

    fn reset(&mut self) {
        if self.processed {
            let sr = self.dsp.sample_rate();
            self.live(sr);
            return;
        }
        // Not played yet: exactly a fresh instance, a new engine prepared at the same rate, every
        // parameter pushed again at the next block (`prepareToPlay` + `forcePush`), in place.
        self.dsp.reset();
        self.force_push = true;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.processed = true;
        self.follow_link();
        self.push_changed();
        let nc = self.channels.min(io.len());
        let frames = io.iter().take(nc).map(Vec::len).fold(frames, usize::min);
        let peak = |io: &[Vec<f32>]| io.iter().take(nc).flat_map(|c| c[..frames].iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        self.in_peak_db = gain_to_db(peak(io));
        self.dsp.process(io, nc, frames);
        for c in io.iter_mut().take(nc) {
            for s in &mut c[..frames] {
                if !s.is_finite() {
                    *s = 0.0;
                }
            }
        }
        self.out_peak_db = gain_to_db(peak(io));
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.in_peak_db;
        out[1] = self.out_peak_db;
        out[2] = self.dsp.drive_smoothed();
        out[3] = self.dsp.level_smoothed();
        out[4] = self.dsp.circuit() as f32;
    }
}

#[cfg(test)]
mod tests;
