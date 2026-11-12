//! `fs_tarn`: Tarn, an automatic balance equaliser (Tame what covers, Recover what is covered).
//! Rust-native: unlike the other engines here it has no C++ original, so there is nothing to be
//! faithful to and no fidelity test; the engine is [`tarn_dsp`], its constants [`Tuning`].
//! `examples/tarn_tune.rs` renders files through it and compares it with a reference render.
//!
//! - Parameters are in their own units (percent, Hz), not FireStation's 0..1 knobs.
//! - Latency: the STFT frame (2048 samples at 44.1/48 kHz, 4096 up to 96 kHz, 8192 above),
//!   always, whatever the settings.
//! - Stereo is linked (one gain curve for both sides); mono runs the same engine on one channel.
//!   Extra channels are left untouched (the host contract).
//! - `bypass` is the engine's own (the input a frame late, so the latency holds), kept hidden
//!   (group "") like every engine's: the UI uses the module's bypass.
//! - No tail beyond the latency: the gain has no memory of the audio.

pub mod tarn_dsp;
#[cfg(test)]
mod tests;

pub use tarn_dsp::{MAX_BANDS, Settings, TarnDsp, Tuning};

use crate::analysis::Analysis;
use crate::params::Params;
use crate::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use std::sync::Arc;

const fn knob(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit, taper: Taper, group: &'static str) -> ParamInfo {
    ParamInfo { id, name, min, max, default, unit, taper, choices: &[], display: None, group }
}

pub const NUM_PARAMS: usize = 9;
const P_TAME: usize = 0;
const P_RECOVER: usize = 1;
const P_BIAS: usize = 2;
const P_BRIGHTEN: usize = 3;
const P_BOOST: usize = 4;
const P_LO: usize = 5;
const P_HI: usize = 6;
const P_DELTA: usize = 7;
const P_BYPASS: usize = 8;

pub static TARN_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob("tame", "Tame", 0.0, 100.0, 25.0, Unit::Percent, Taper::Linear, "Balance"),
    knob("recover", "Recover", 0.0, 100.0, 25.0, Unit::Percent, Taper::Linear, "Balance"),
    knob("bias", "Bias", -100.0, 100.0, 0.0, Unit::Percent, Taper::Linear, "Balance"),
    knob("brighten", "Brighten", -100.0, 100.0, 0.0, Unit::Percent, Taper::Linear, "Tone"),
    knob("boost", "Boost", -100.0, 100.0, 0.0, Unit::Percent, Taper::Linear, "Tone"),
    knob("rangeLo", "Range Low", 20.0, 20_000.0, 20.0, Unit::Hz, Taper::Log, "Range"),
    knob("rangeHi", "Range High", 20.0, 20_000.0, 20_000.0, Unit::Hz, Taper::Log, "Range"),
    knob("delta", "Delta", 0.0, 1.0, 0.0, Unit::Toggle, Taper::Linear, "Listen"),
    // The engine's own bypass, hidden; the UI uses the module's bypass instead.
    knob("bypass", "Bypass", 0.0, 1.0, 0.0, Unit::Toggle, Taper::Linear, ""),
];

pub static TARN_INFO: PluginInfo = PluginInfo {
    id: "fs_tarn",
    name: "Tarn",
    short_name: "Tarn",
    category: Category::FireStation,
    params: &TARN_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg =
    super::EngineReg { info: &TARN_INFO, make: || Box::new(Tarn::new()), presets: &[], order: 45, fidelity: None, prepare_state: None, actions: &[] };

/// [`Analysis::live`]: the number of bands at `LIVE_BANDS`, then each band's centre in Hz from
/// `LIVE_HZ` and its gain in dB from `LIVE_GAIN` (the whole curve: Tame, Recover, the tilts and
/// the level given back), as of the last block.
pub const LIVE_BANDS: usize = 0;
pub const LIVE_HZ: usize = 1;
pub const LIVE_GAIN: usize = 1 + MAX_BANDS;
pub const LIVE_LEN: usize = 1 + 2 * MAX_BANDS;

/// Tarn.
///
/// `readouts` (as of the last block): `[0]` the largest cut Tame made, dB; `[1]` the largest
/// lift Recover made, dB; `[2]` the level given back, dB. `gain_reduction_db` is `[0]`.
/// `analysis()`: `pre` the mono input, `post` the mono output, and the gain curve in the live
/// values ([`LIVE_BANDS`] ..).
pub struct Tarn {
    p: Params,
    dsp: Box<TarnDsp>,
    analysis: Arc<Analysis>,
    channels: usize,
}

impl Default for Tarn {
    fn default() -> Self {
        Self::new()
    }
}

impl Tarn {
    pub fn new() -> Self {
        let mut s =
            Tarn { p: Params::new(&TARN_PARAMS), dsp: Box::new(TarnDsp::new()), analysis: Arc::new(Analysis::with_live(LIVE_LEN)), channels: 2 };
        s.update();
        s
    }

    /// Hands the knobs to the engine.
    fn update(&mut self) {
        self.dsp.set(Settings {
            tame: self.p.v(P_TAME) / 100.0,
            recover: self.p.v(P_RECOVER) / 100.0,
            bias: self.p.v(P_BIAS) / 100.0,
            brighten: self.p.v(P_BRIGHTEN) / 100.0,
            boost: self.p.v(P_BOOST) / 100.0,
            lo_hz: self.p.v(P_LO),
            hi_hz: self.p.v(P_HI),
            delta: self.p.on(P_DELTA),
            bypass: self.p.on(P_BYPASS),
        });
    }

    fn publish(&self) {
        let live = &self.analysis.live;
        let hz = self.dsp.band_hz();
        let gain = self.dsp.band_gain_db();
        live.set(LIVE_BANDS, hz.len() as f32);
        for (i, (h, g)) in hz.iter().zip(gain).enumerate() {
            live.set(LIVE_HZ + i, *h);
            live.set(LIVE_GAIN + i, *g);
        }
    }
}

impl Plugin for Tarn {
    crate::params::param_plumbing!(TARN_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, 2);
        self.dsp.prepare(f64::from(sample_rate), self.channels);
        self.update();
        self.publish();
    }

    fn reset(&mut self) {
        self.dsp.reset();
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
        self.publish();
    }

    fn latency(&self) -> usize {
        self.dsp.latency_samples()
    }

    fn gain_reduction_db(&self) -> f32 {
        self.dsp.extremes_db().0
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let (cut, lift) = self.dsp.extremes_db();
        out[0] = cut;
        out[1] = lift;
        out[2] = self.dsp.level_comp_db();
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}
