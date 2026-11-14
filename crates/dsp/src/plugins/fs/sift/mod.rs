//! `fs_sift`: Sift, a de-esser for vocals that also catches plosives. It sorts a voice into the
//! voice itself, its sibilance and its plosives, and treats each on its own. Rust-native: like
//! Manners it has no C++ original, so there is no fidelity test; the engine is [`sift_dsp`].
//!
//! - Parameters are in their own units (percent, dB, Hz).
//! - Latency: the STFT frame, about 8 ms (384 samples at 48 kHz), always.
//! - Stereo is linked (one gain for both sides); mono runs the same engine on one channel. Extra
//!   channels are left untouched (the host contract).
//! - `bypass` is the engine's own (the input a frame late, so the latency holds), kept hidden
//!   (group "") like every engine's: the UI uses the module's bypass.
//! - No tail beyond the latency.

pub mod sift_dsp;
#[cfg(test)]
mod tests;

pub use sift_dsp::{Listen, Meters, Settings, SiftDsp, frame_len};

use crate::analysis::Analysis;
use crate::params::Params;
use crate::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use std::sync::Arc;

const fn knob(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit, taper: Taper, group: &'static str) -> ParamInfo {
    ParamInfo { id, name, min, max, default, unit, taper, choices: &[], display: None, group }
}

const fn pick(id: &'static str, name: &'static str, choices: &'static [&'static str], group: &'static str) -> ParamInfo {
    ParamInfo {
        id,
        name,
        min: 0.0,
        max: (choices.len() - 1) as f32,
        default: 0.0,
        unit: Unit::Choice,
        taper: Taper::Linear,
        choices,
        display: None,
        group,
    }
}

pub const MODE_NAMES: [&str; 2] = ["Split", "Wide"];
pub const LISTEN_NAMES: [&str; 4] = ["Off", "Sibilance", "Plosives", "Removed"];

pub const NUM_PARAMS: usize = 10;
const P_SIB_AMOUNT: usize = 0;
const P_SIB_RANGE: usize = 1;
const P_SIB_LO: usize = 2;
const P_SIB_HI: usize = 3;
const P_MODE: usize = 4;
const P_PLO_AMOUNT: usize = 5;
const P_PLO_RANGE: usize = 6;
const P_PLO_HI: usize = 7;
const P_LISTEN: usize = 8;
const P_BYPASS: usize = 9;

pub static SIFT_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob("sibAmount", "Sibilance", 0.0, 100.0, 50.0, Unit::Percent, Taper::Linear, "Sibilance"),
    knob("sibRange", "Sibilance Range", 0.0, 30.0, 12.0, Unit::Db, Taper::Linear, "Sibilance"),
    knob("sibLow", "Sibilance Low", 1_000.0, 20_000.0, 4_500.0, Unit::Hz, Taper::Log, "Sibilance"),
    knob("sibHigh", "Sibilance High", 1_000.0, 20_000.0, 14_000.0, Unit::Hz, Taper::Log, "Sibilance"),
    pick("mode", "Mode", &MODE_NAMES, "Sibilance"),
    knob("ploAmount", "Plosives", 0.0, 100.0, 50.0, Unit::Percent, Taper::Linear, "Plosives"),
    knob("ploRange", "Plosive Range", 0.0, 36.0, 18.0, Unit::Db, Taper::Linear, "Plosives"),
    knob("ploHigh", "Plosive Ceiling", 40.0, 500.0, 180.0, Unit::Hz, Taper::Log, "Plosives"),
    pick("listen", "Listen", &LISTEN_NAMES, "Listen"),
    // The engine's own bypass, hidden; the UI uses the module's bypass instead.
    knob("bypass", "Bypass", 0.0, 1.0, 0.0, Unit::Toggle, Taper::Linear, ""),
];

pub static SIFT_INFO: PluginInfo = PluginInfo {
    id: "fs_sift",
    name: "Sift",
    short_name: "Sift",
    category: Category::FireStation,
    params: &SIFT_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg =
    super::EngineReg { info: &SIFT_INFO, make: || Box::new(Sift::new()), presets: &[], order: 47, fidelity: None, prepare_state: None, actions: &[] };

/// [`Analysis::live`]: running maxima the editor takes (resets) as it draws: the sibilance cut
/// and the plosive cut in dB, and how sure the engine was of each, 0..1.
pub const LIVE_SIB_CUT: usize = 0;
pub const LIVE_PLO_CUT: usize = 1;
pub const LIVE_SIBILANT: usize = 2;
pub const LIVE_PLOSIVE: usize = 3;
pub const LIVE_LEN: usize = 4;

/// Sift.
///
/// `readouts` (the largest of the last block): `[0]` dB taken off the sibilance, `[1]` dB taken
/// off a plosive, `[2]` how sure it was of a sibilant and `[3]` of a plosive, 0..1.
/// `gain_reduction_db` is the larger cut. `analysis()`: `pre` the mono input, `post` the mono
/// output, and the same four values as running maxima in the live values ([`LIVE_SIB_CUT`] ..).
pub struct Sift {
    p: Params,
    dsp: Box<SiftDsp>,
    analysis: Arc<Analysis>,
    channels: usize,
    last: Meters,
}

impl Default for Sift {
    fn default() -> Self {
        Self::new()
    }
}

impl Sift {
    pub fn new() -> Self {
        let mut s = Sift {
            p: Params::new(&SIFT_PARAMS),
            dsp: Box::new(SiftDsp::new()),
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
            channels: 2,
            last: Meters::default(),
        };
        s.update();
        s
    }

    /// Hands the knobs to the engine.
    fn update(&mut self) {
        self.dsp.set(Settings {
            sib_amount: self.p.v(P_SIB_AMOUNT) / 100.0,
            sib_range_db: self.p.v(P_SIB_RANGE),
            sib_lo_hz: self.p.v(P_SIB_LO),
            sib_hi_hz: self.p.v(P_SIB_HI),
            wide: self.p.choice(P_MODE) == 1,
            plo_amount: self.p.v(P_PLO_AMOUNT) / 100.0,
            plo_range_db: self.p.v(P_PLO_RANGE),
            plo_hi_hz: self.p.v(P_PLO_HI),
            listen: match self.p.choice(P_LISTEN) {
                1 => Listen::Sibilance,
                2 => Listen::Plosives,
                3 => Listen::Removed,
                _ => Listen::Off,
            },
            bypass: self.p.on(P_BYPASS),
        });
    }
}

impl Plugin for Sift {
    crate::params::param_plumbing!(SIFT_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, 2);
        self.dsp.prepare(f64::from(sample_rate), self.channels);
        self.update();
        self.last = Meters::default();
    }

    fn reset(&mut self) {
        self.dsp.reset();
        self.last = Meters::default();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let nc = self.channels.min(io.len());
        if nc == 0 {
            return;
        }
        let frames = io.iter().take(nc).map(Vec::len).min().unwrap_or(0).min(frames);
        let mono = |io: &[Vec<f32>], i: usize| io.iter().take(nc).filter_map(|c| c.get(i)).sum::<f32>() / nc as f32;
        for i in 0..frames {
            self.analysis.pre.push(mono(io, i));
        }
        self.dsp.process(io, frames);
        for i in 0..frames {
            self.analysis.post.push(mono(io, i));
        }
        let m = self.dsp.take_meters();
        self.last = m;
        let live = &self.analysis.live;
        live.raise_max(LIVE_SIB_CUT, m.sib_cut_db);
        live.raise_max(LIVE_PLO_CUT, m.plo_cut_db);
        live.raise_max(LIVE_SIBILANT, m.sibilant);
        live.raise_max(LIVE_PLOSIVE, m.plosive);
    }

    fn latency(&self) -> usize {
        self.dsp.latency_samples()
    }

    fn gain_reduction_db(&self) -> f32 {
        self.last.sib_cut_db.max(self.last.plo_cut_db)
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.last.sib_cut_db;
        out[1] = self.last.plo_cut_db;
        out[2] = self.last.sibilant;
        out[3] = self.last.plosive;
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}
