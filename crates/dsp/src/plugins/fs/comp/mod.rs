//! `fs_comp`: the owner's CompEngine (FireStation Engine 1), a faithful translation of
//! `Engine 1 - Comp/Source/CompEngineDSP.h`: ten compressor styles, three lanes of them
//! (parallel, multiband or serial), plus a dry fader.
//!
//! Wrapper (`PluginProcessor.cpp`) audit, everything that affects the sound:
//! - `prepareToPlay` calls `dsp.prepare (sampleRate)` and then raises `forcePush`, so the first
//!   `processBlock` pushes every parameter before processing: here `prepare` prepares the engine
//!   and then hands it every stored value, in parameter order.
//! - `processBlock` pushes only changed values, in parameter order, before processing: here
//!   `set_param` passes the value straight to `setParameter`. Re-sending an unchanged value only
//!   marks a lane dirty (its `updateParams` re-derives the same model from the same knobs) or
//!   recomputes the same crossover coefficients, so pushing on every set is equivalent.
//! - APVTS ranges: floats are 0..1, choices are whole indices, bools 0/1. `ParamInfo::clamp`
//!   does the same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Bus layout: mono or stereo only; `dsp.process` gets the buffer's channel count, and with one
//!   channel the engine uses it for both sides. The engine processes at most 2 channels and
//!   leaves the rest untouched (the Rust host contract for extra channels). Unused output
//!   channels are cleared by the C++ wrapper (there are none here: in and out match).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. It only changes values below
//!   ~1e-38, far under the fidelity bar; the engine adds its own `DN` offsets, and
//!   `silence_tail_is_cheap` guards the CPU side.
//! - No `setLatencySamples`: the engine is zero-latency (the colour residual's half-band
//!   allpasses are IIR and add no reported delay), so `latency()` is 0.
//! - No tail (`getTailLengthSeconds` is 0). Bypass is the engine's own 8 ms crossfade, kept.
//! - Meters (`getGainReductionDb`, `getInputPeakDb`, `getOutputPeakDb`, per-lane GR) and the
//!   input scope are editor-only: they reach the editor through `analysis()` (the engine's own
//!   30 Hz meters as live values, [`LIVE_GR`] .. [`LIVE_LANE_GR`], and `(L+R)/2` of the input on
//!   `pre`, as `copyScope` reads it). The preset dirty-tracking, state save and editor are
//!   UI-only.

pub mod comp_engine_dsp;
mod presets;

pub use comp_engine_dsp::{CompEngineDsp, NUM_PARAMS, knob_to_lane_db, knob_to_xover_hi, knob_to_xover_lo};
pub use presets::PRESETS;

use crate::analysis::Analysis;
use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, gain_to_db};
use std::sync::Arc;

/// [`Analysis::live`] indices: the engine's 30 Hz meters as the C++ editor polls them
/// (`getGainReductionDb`, `getInputPeakDb`, `getOutputPeakDb`, `getLaneGainReductionDb (0..2)`).
pub const LIVE_GR: usize = 0;
pub const LIVE_IN: usize = 1;
pub const LIVE_OUT: usize = 2;
/// Lane A, B, C at `LIVE_LANE_GR + lane`.
pub const LIVE_LANE_GR: usize = 3;
pub const LIVE_LEN: usize = 6;

const STYLES: &[&str] = &["Clean", "1176", "LA-2A", "160", "G Bus", "Vari-Mu", "Ross", "EL8", "2500", "33609"];
const VARIANTS: &[&str] = &["A", "B", "C"];
const DETECTORS: &[&str] = &["0", "1", "2", "3"];

/// Knobs whose meaning depends on the lane's style (the C++ host shows them as 0..1 too).
fn pct_text(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}
/// Makeup: -12 .. +24 dB (`updateParams`, with its +/-0.05 dB snap to 0).
fn makeup_text(v: f32) -> String {
    let mut db = -12.0 + 36.0 * v;
    if db.abs() < 0.05 {
        db = 0.0;
    }
    format!("{db:+.1} dB")
}
/// Colour: 0 clean, 5 the stock circuit, 10 every non-linear element ~3.4x harder.
fn color_text(v: f32) -> String {
    format!("{:.1}", v * 10.0)
}
/// Sidechain high-pass: off below 0.02, else 20 .. 500 Hz.
fn hpf_text(v: f32) -> String {
    if v > 0.02 { format!("{:.0} Hz", 20.0 * 25.0f32.powf(v)) } else { "Off".to_string() }
}
/// Lane and dry faders: 0.75 = unity, (v - 0.75) x 40 dB, 0 = silent.
fn level_text(v: f32) -> String {
    if v <= 0.0 { "Off".to_string() } else { format!("{:+.1} dB", knob_to_lane_db(v)) }
}
fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 { format!("{:.2} kHz", hz / 1000.0) } else { format!("{:.0} Hz", hz.round()) }
}
fn xover_lo_text(v: f32) -> String {
    hz_text(knob_to_xover_lo(v))
}
fn xover_hi_text(v: f32) -> String {
    hz_text(knob_to_xover_hi(v))
}

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static COMP_PARAMS: [ParamInfo; NUM_PARAMS] = [
    fs_choice("style", "A Style", 0.0, "Lane A", STYLES),
    knob01("amount", "A Amount", 0.4, "Lane A", pct_text),
    knob01("ratio", "A Ratio", 0.4, "Lane A", pct_text),
    knob01("attack", "A Attack", 0.5, "Lane A", pct_text),
    knob01("release", "A Release", 0.5, "Lane A", pct_text),
    knob01("makeup", "A Makeup", 0.3333, "Lane A", makeup_text),
    knob01("dry", "Dry", 0.0, "Global", level_text),
    knob01("knee", "A Knee", 0.25, "Lane A", pct_text),
    knob01("scHpf", "A SC HPF", 0.0, "Lane A", hpf_text),
    knob01("color", "A Color", 0.5, "Lane A", color_text),
    knob01("link", "A Link", 1.0, "Lane A", pct_text),
    fs_toggle("autoGain", "A Auto Gain", 0.0, "Lane A"),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    fs_choice("variant", "A Variant", 0.0, "Lane A", VARIANTS),
    fs_choice("detMode", "A Detector", 0.0, "Lane A", DETECTORS),
    fs_choice("chanMode", "Channels", 0.0, "Global", &["Stereo", "Mid-Side", "Mid only", "Side only"]),
    knob01("aLevel", "A Level", 0.75, "Lane A", level_text),
    fs_toggle("bOn", "B On", 0.0, "Lane B"),
    knob01("bLevel", "B Level", 0.75, "Lane B", level_text),
    fs_toggle("cOn", "C On", 0.0, "Lane C"),
    knob01("cLevel", "C Level", 0.75, "Lane C", level_text),
    fs_choice("bStyle", "B Style", 1.0, "Lane B", STYLES),
    knob01("bAmount", "B Amount", 0.4, "Lane B", pct_text),
    knob01("bRatio", "B Ratio", 0.4, "Lane B", pct_text),
    knob01("bAttack", "B Attack", 0.5, "Lane B", pct_text),
    knob01("bRelease", "B Release", 0.5, "Lane B", pct_text),
    knob01("bMakeup", "B Makeup", 0.3333, "Lane B", makeup_text),
    knob01("bKnee", "B Knee", 0.25, "Lane B", pct_text),
    knob01("bScHpf", "B SC HPF", 0.0, "Lane B", hpf_text),
    knob01("bColor", "B Color", 0.5, "Lane B", color_text),
    knob01("bLink", "B Link", 1.0, "Lane B", pct_text),
    fs_toggle("bAutoGain", "B Auto Gain", 0.0, "Lane B"),
    fs_choice("bVariant", "B Variant", 0.0, "Lane B", VARIANTS),
    fs_choice("bDetMode", "B Detector", 0.0, "Lane B", DETECTORS),
    fs_choice("cStyle", "C Style", 2.0, "Lane C", STYLES),
    knob01("cAmount", "C Amount", 0.4, "Lane C", pct_text),
    knob01("cRatio", "C Ratio", 0.4, "Lane C", pct_text),
    knob01("cAttack", "C Attack", 0.5, "Lane C", pct_text),
    knob01("cRelease", "C Release", 0.5, "Lane C", pct_text),
    knob01("cMakeup", "C Makeup", 0.3333, "Lane C", makeup_text),
    knob01("cKnee", "C Knee", 0.25, "Lane C", pct_text),
    knob01("cScHpf", "C SC HPF", 0.0, "Lane C", hpf_text),
    knob01("cColor", "C Color", 0.5, "Lane C", color_text),
    knob01("cLink", "C Link", 1.0, "Lane C", pct_text),
    fs_toggle("cAutoGain", "C Auto Gain", 0.0, "Lane C"),
    fs_choice("cVariant", "C Variant", 0.0, "Lane C", VARIANTS),
    fs_choice("cDetMode", "C Detector", 0.0, "Lane C", DETECTORS),
    fs_toggle("aOn", "A On", 1.0, "Lane A"),
    fs_toggle("dryOn", "Dry On", 1.0, "Global"),
    fs_choice("split", "Split", 0.0, "Crossover", &["Parallel", "Multiband", "Serial"]),
    knob01("xoverLo", "Low / Mid", 0.467, "Crossover", xover_lo_text),
    knob01("xoverHi", "Mid / High", 0.42, "Crossover", xover_hi_text),
];

pub static COMP_INFO: PluginInfo = PluginInfo {
    id: "fs_comp",
    name: "Comp",
    short_name: "Comp",
    category: Category::FireStation,
    params: &COMP_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &COMP_INFO,
    make: || Box::new(Comp::new()),
    presets: PRESETS,
    order: 20,
    fidelity: Some(super::Fidelity { name: "comp", source_dir: "Engine 1 - Comp", render_tool: "CompEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// The CompEngine port.
///
/// `readouts` (over the last block): `[0]` detector input level, the peak of the plugin's input,
/// in dBFS (every lane's detector is fed from it); `[1]`/`[2]`/`[3]` lane A/B/C gain reduction in
/// dB (a non-negative amount; 0 for a lane that is off, or when bypassed, as the C++ meters);
/// `[4]` output peak dBFS. `gain_reduction_db` is the largest lane GR.
pub struct Comp {
    p: Params,
    dsp: Box<CompEngineDsp>,
    channels: usize,
    in_peak_db: f32,
    out_peak_db: f32,
    lane_gr_db: [f32; 3],
    /// The editor's streams: the input scope on `pre` and the 30 Hz meters as live values.
    analysis: Arc<Analysis>,
}

impl Default for Comp {
    fn default() -> Self {
        Self::new()
    }
}

impl Comp {
    pub fn new() -> Self {
        let mut s = Comp {
            p: Params::new(&COMP_PARAMS),
            dsp: Box::new(CompEngineDsp::new()),
            channels: 2,
            in_peak_db: f32::NAN,
            out_peak_db: f32::NAN,
            lane_gr_db: [0.0; 3],
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
        };
        s.push_all();
        s.publish();
        s
    }

    /// The engine's 30 Hz meters for the editor (atomic stores only).
    fn publish(&self) {
        let live = &self.analysis.live;
        live.set(LIVE_GR, self.dsp.gain_reduction_db());
        live.set(LIVE_IN, self.dsp.input_peak_db());
        live.set(LIVE_OUT, self.dsp.output_peak_db());
        for lane in 0..3 {
            live.set(LIVE_LANE_GR + lane, self.dsp.lane_gain_reduction_db(lane as i32));
        }
    }

    /// Hands every stored value to the engine, in parameter order (the wrapper's `forcePush`).
    fn push_all(&mut self) {
        for i in 0..NUM_PARAMS {
            self.dsp.set_parameter(i, self.p.v(i));
        }
    }
}

impl Plugin for Comp {
    fn info(&self) -> &'static PluginInfo {
        &COMP_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match self.p.set(id, value) {
            Some(idx) => {
                self.dsp.set_parameter(idx, self.p.v(idx));
                true
            }
            None => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // As `prepareToPlay` + the forced push of the first block: prepare, then every value.
        // The engine is fixed-size state in one box: nothing is allocated here or later.
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.dsp.prepare(sr);
        self.push_all();
    }

    fn reset(&mut self) {
        // The C++ has no reset, and its `prepare` keeps the lanes' smoothed threshold, gains and
        // auto-gain average. Back to rest means a freshly constructed engine (as the C++ render
        // tool starts from), written into the existing box: no allocation.
        let sr = self.dsp_sample_rate();
        *self.dsp = CompEngineDsp::new();
        self.dsp.prepare(sr);
        self.push_all();
        self.publish();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let nc = self.channels.min(io.len());
        let frames = io[..nc].iter().map(Vec::len).fold(frames, usize::min);
        // The input scope (`scope[...] = 0.5f * (x0 + x1)`, x1 = x0 on a mono bus), before the
        // engine overwrites the buffers in place.
        // Pushed in chunks through a small stack buffer: one ring publish per chunk, not per sample.
        if let Some(c0) = io.first() {
            let c1 = if nc >= 2 { io.get(1) } else { None };
            let mut chunk = [0.0f32; 256];
            let mut start = 0;
            while start < frames {
                let len = (frames - start).min(chunk.len());
                for (k, slot) in chunk[..len].iter_mut().enumerate() {
                    let n = start + k;
                    let x0 = c0.get(n).copied().unwrap_or(0.0);
                    let x1 = c1.and_then(|c| c.get(n).copied()).unwrap_or(x0);
                    *slot = 0.5 * (x0 + x1);
                }
                self.analysis.pre.push_slice(&chunk[..len]);
                start += len;
            }
        }
        self.dsp.process(io, nc, frames);
        let m = self.dsp.take_block_meters();
        self.in_peak_db = gain_to_db(m.in_peak);
        self.out_peak_db = gain_to_db(m.out_peak);
        for (o, g) in self.lane_gr_db.iter_mut().zip(m.lane_gr_db) {
            *o = -g;
        }
        self.publish();
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }

    fn gain_reduction_db(&self) -> f32 {
        self.lane_gr_db.iter().copied().fold(0.0, f32::max)
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.in_peak_db;
        out[1] = self.lane_gr_db[0];
        out[2] = self.lane_gr_db[1];
        out[3] = self.lane_gr_db[2];
        out[4] = self.out_peak_db;
    }
}

impl Comp {
    fn dsp_sample_rate(&self) -> f64 {
        f64::from(self.dsp.sample_rate())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::fs::{FsPreset, factory_presets};
    use std::time::Instant;

    fn sine(len: usize, amp: f32) -> Vec<f32> {
        (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
    }

    fn prepared() -> Comp {
        let mut s = Comp::new();
        s.prepare(48_000.0, 512, 2);
        s
    }

    fn run(s: &mut Comp, io: &mut [Vec<f32>], block: usize) {
        let len = io.iter().map(Vec::len).min().unwrap_or(0);
        let mut buf: Vec<Vec<f32>> = io.iter().map(|_| vec![0.0; block]).collect();
        let mut pos = 0;
        while pos < len {
            let n = block.min(len - pos);
            for (b, c) in buf.iter_mut().zip(io.iter()) {
                b[..n].copy_from_slice(&c[pos..pos + n]);
            }
            s.process(&mut buf, n);
            for (b, c) in buf.iter().zip(io.iter_mut()) {
                c[pos..pos + n].copy_from_slice(&b[..n]);
            }
            pos += n;
        }
    }

    fn all_finite(io: &[Vec<f32>]) -> bool {
        io.iter().flatten().all(|v| v.is_finite())
    }

    /// Every lane on and set to `style`, so the hostile loops reach all three lanes' maths.
    fn all_lanes(s: &mut Comp, style: usize) {
        for id in ["style", "bStyle", "cStyle"] {
            s.set_param(id, style as f32);
        }
        s.set_param("bOn", 1.0);
        s.set_param("cOn", 1.0);
    }

    #[test]
    fn table_matches_the_cpp_order_and_defaults() {
        let ids: Vec<&str> = COMP_PARAMS.iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            [
                "style",
                "amount",
                "ratio",
                "attack",
                "release",
                "makeup",
                "dry",
                "knee",
                "scHpf",
                "color",
                "link",
                "autoGain",
                "bypass",
                "variant",
                "detMode",
                "chanMode",
                "aLevel",
                "bOn",
                "bLevel",
                "cOn",
                "cLevel",
                "bStyle",
                "bAmount",
                "bRatio",
                "bAttack",
                "bRelease",
                "bMakeup",
                "bKnee",
                "bScHpf",
                "bColor",
                "bLink",
                "bAutoGain",
                "bVariant",
                "bDetMode",
                "cStyle",
                "cAmount",
                "cRatio",
                "cAttack",
                "cRelease",
                "cMakeup",
                "cKnee",
                "cScHpf",
                "cColor",
                "cLink",
                "cAutoGain",
                "cVariant",
                "cDetMode",
                "aOn",
                "dryOn",
                "split",
                "xoverLo",
                "xoverHi"
            ]
        );
        let defaults: Vec<f32> = COMP_PARAMS.iter().map(|p| p.default).collect();
        let lane = [0.4, 0.4, 0.5, 0.5, 0.3333, 0.25, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0];
        let mut want = vec![0.0, 0.4, 0.4, 0.5, 0.5, 0.3333, 0.0, 0.25, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.75, 0.0, 0.75, 0.0, 0.75, 1.0];
        want.extend(lane);
        want.push(2.0);
        want.extend(lane);
        want.extend([1.0, 1.0, 0.0, 0.467, 0.42]);
        assert_eq!(defaults, want);
        for p in &COMP_PARAMS {
            let g = p.group;
            let lane_id = if p.id.starts_with('b') && p.id != "bypass" {
                "Lane B"
            } else if p.id.starts_with('c') && p.id != "chanMode" && p.id != "color" {
                "Lane C"
            } else {
                ""
            };
            if !lane_id.is_empty() {
                assert_eq!(g, lane_id, "{}", p.id);
            }
        }
        assert_eq!(COMP_PARAMS[5].format(0.3333), "+0.0 dB");
        assert_eq!(COMP_PARAMS[16].format(0.75), "+0.0 dB");
        assert_eq!(COMP_PARAMS[16].format(1.0), "+10.0 dB");
        assert_eq!(COMP_PARAMS[6].format(0.0), "Off");
        assert_eq!(COMP_PARAMS[8].format(0.0), "Off");
        assert_eq!(COMP_PARAMS[8].format(1.0), "500 Hz");
        assert_eq!(COMP_PARAMS[50].format(0.0), "40 Hz");
        assert_eq!(COMP_PARAMS[50].format(1.0), "1.00 kHz");
        assert_eq!(COMP_PARAMS[51].format(1.0), "12.00 kHz");
        assert_eq!(COMP_PARAMS[50].group, "Crossover");
        assert_eq!(COMP_PARAMS[12].group, "");
    }

    #[test]
    fn registered_and_reports_no_latency() {
        let p = crate::create("fs_comp").unwrap();
        assert_eq!(p.info().category, Category::FireStation);
        assert_eq!(p.latency(), 0, "the C++ is zero-latency");
    }

    #[test]
    fn readouts_meter_lanes_and_output() {
        let mut s = prepared();
        s.set_param("amount", 0.8);
        s.set_param("bOn", 1.0);
        let mut io = vec![sine(9600, 0.5), sine(9600, 0.5)];
        run(&mut s, &mut io, 512);
        let mut r = [f32::NAN; 8];
        s.readouts(&mut r);
        assert!((r[0] - gain_to_db(0.5)).abs() < 0.1, "{r:?}");
        assert!(r[1] > 1.0 && r[2] > 1.0, "lanes A and B compress: {r:?}");
        assert_eq!(r[3], 0.0, "lane C is off");
        assert!(r[4].is_finite(), "{r:?}");
        assert!(r[5].is_nan());
        assert_eq!(s.gain_reduction_db(), r[1].max(r[2]));
    }

    #[test]
    fn every_factory_preset_parses() {
        let presets = factory_presets("fs_comp");
        assert_eq!(presets.len(), 86);
        for (name, json) in presets {
            let p = FsPreset::parse(json, &COMP_INFO).unwrap();
            assert_eq!(&p.name, name);
            let raw: serde_json::Value = serde_json::from_str(json).unwrap();
            let keys = raw["values"].as_object().unwrap();
            for key in keys.keys() {
                assert!(p.values.contains_key(key), "{name}: {key} did not resolve");
            }
            assert!(p.values.contains_key("style"), "{name}");
        }
    }

    #[test]
    fn hostile_params_stay_finite() {
        // Bypass stays off (it would hide the wet path); the styles are set per case.
        let skip = ["style", "bStyle", "cStyle", "bypass"];
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
            for style in 0..10 {
                let mut s = prepared();
                all_lanes(&mut s, style);
                for info in COMP_PARAMS.iter().filter(|p| !skip.contains(&p.id)) {
                    assert!(s.set_param(info.id, bad));
                }
                assert_eq!(s.param("bypass"), Some(0.0));
                let mut io = vec![sine(2048, 0.9), sine(2048, -0.9)];
                run(&mut s, &mut io, 256);
                assert!(all_finite(&io), "style {style}, value {bad}");
            }
        }
    }

    #[test]
    fn hostile_bypass_values_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
            for style in 0..10 {
                let mut s = prepared();
                all_lanes(&mut s, style);
                s.set_param("amount", 1.0);
                let src = [sine(4096, 0.9), sine(4096, -0.9)];
                // Half the run with the wet path engaged, then the hostile bypass value mid-stream.
                let mut a = vec![src[0][..2048].to_vec(), src[1][..2048].to_vec()];
                run(&mut s, &mut a, 256);
                assert!(s.set_param("bypass", bad));
                let mut b = vec![src[0][2048..].to_vec(), src[1][2048..].to_vec()];
                run(&mut s, &mut b, 256);
                assert!(all_finite(&a) && all_finite(&b), "style {style}, bypass {bad}");
            }
        }
    }

    #[test]
    fn switching_quality_and_model_mid_stream_is_safe() {
        // No quality setting in this engine: the style, variant, detector, split and channel
        // mode are what switch mid-stream.
        let mut s = prepared();
        s.set_param("bOn", 1.0);
        s.set_param("cOn", 1.0);
        let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
        let src = sine(128 * 400, 0.8);
        for blk in 0..400 {
            s.set_param("style", (blk % 10) as f32);
            s.set_param("bStyle", ((blk + 3) % 10) as f32);
            s.set_param("cStyle", ((blk + 7) % 10) as f32);
            s.set_param("variant", (blk % 3) as f32);
            s.set_param("detMode", (blk % 4) as f32);
            s.set_param("split", (blk % 3) as f32);
            s.set_param("chanMode", ((blk / 3) % 4) as f32);
            for b in &mut buf {
                b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
            }
            s.process(&mut buf, 128);
            assert!(all_finite(&buf), "block {blk}");
        }
    }

    #[test]
    fn mono_and_wide_buffers() {
        // Mono: one channel processed.
        let mut s = Comp::new();
        s.prepare(48_000.0, 512, 1);
        s.set_param("amount", 0.8);
        let mut mono = vec![sine(4096, 0.5)];
        let orig = mono.clone();
        run(&mut s, &mut mono, 512);
        assert!(all_finite(&mono));
        assert_ne!(mono, orig);

        // Six channels: the first two processed, the rest untouched.
        let mut s = Comp::new();
        s.prepare(48_000.0, 512, 6);
        let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(4096, 0.1 * (c + 1) as f32)).collect();
        let orig = wide.clone();
        s.set_param("amount", 1.0);
        run(&mut s, &mut wide, 512);
        assert!(all_finite(&wide));
        assert_ne!(wide[0], orig[0]);
        assert_ne!(wide[1], orig[1]);
        assert_eq!(wide[2..], orig[2..]);

        // Fewer buffers than prepared, short buffers, zero frames: no panic.
        let mut s = prepared();
        let mut one = vec![vec![0.3f32; 10]];
        s.process(&mut one, 512);
        s.process(&mut [], 64);
        let mut empty = vec![Vec::new(), Vec::new()];
        s.process(&mut empty, 64);
        let mut ragged = vec![vec![0.3f32; 64], vec![0.3f32; 10]];
        s.process(&mut ragged, 64);
    }

    #[test]
    fn silence_tail_is_cheap() {
        let sr = 48_000usize;
        let mut cases = Vec::new();
        for style in 0..10 {
            let time = |signal_s: usize, silence_s: usize| {
                let mut s = prepared();
                all_lanes(&mut s, style);
                s.set_param("amount", 0.8);
                let mut io = vec![sine(signal_s * sr, 0.9), sine(signal_s * sr, 0.9)];
                for c in &mut io {
                    c.resize((signal_s + silence_s) * sr, 0.0);
                }
                let t = Instant::now();
                run(&mut s, &mut io, 512);
                assert!(all_finite(&io));
                t.elapsed().as_secs_f64()
            };
            let tail = time(2, 10);
            let full = time(12, 0);
            cases.push((style, tail, full));
            assert!(tail < 4.0 * full, "style {style}: silence tail {tail:.3}s vs signal {full:.3}s");
        }
        println!("{cases:?}");
    }

    #[test]
    fn process_does_not_allocate() {
        // No counting allocator in this crate: the engine holds no heap memory at all (every
        // state is a fixed-size field of one boxed struct), so its address is the whole story;
        // the host buffers keep their pointers and capacities across 1000 blocks.
        let mut s = prepared();
        s.set_param("bOn", 1.0);
        s.set_param("cOn", 1.0);
        let engine_ptr: *const CompEngineDsp = &*s.dsp;
        let live_at = s.analysis.live.buffer_address();
        let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
        let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
        for blk in 0..1000 {
            s.set_param("split", (blk % 3) as f32);
            s.set_param("style", ((blk / 3) % 10) as f32);
            s.set_param("xoverLo", (blk % 7) as f32 / 7.0);
            s.process(&mut io, 256);
        }
        assert_eq!(engine_ptr, &*s.dsp as *const CompEngineDsp);
        assert_eq!(live_at, s.analysis.live.buffer_address(), "the live values never move");
        assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
        assert!(all_finite(&io));
    }

    #[test]
    fn editor_streams_carry_the_meters_and_the_input_scope() {
        let mut s = prepared();
        s.set_param("amount", 0.9);
        let a = s.analysis().unwrap();
        assert_eq!(a.live.len(), LIVE_LEN);
        let mut io = vec![sine(48_000, 0.5), vec![0.0; 48_000]];
        let input: Vec<f32> = io[0].iter().zip(&io[1]).map(|(l, r)| 0.5 * (l + r)).collect();
        run(&mut s, &mut io, 512);
        // the 30 Hz meters, as the engine holds them
        assert_eq!(a.live.get(LIVE_GR), s.dsp.gain_reduction_db());
        assert!(a.live.get(LIVE_GR) > 1.0, "lane A compresses: {}", a.live.get(LIVE_GR));
        assert_eq!(a.live.get(LIVE_IN), s.dsp.input_peak_db());
        assert_eq!(a.live.get(LIVE_OUT), s.dsp.output_peak_db());
        assert_eq!(a.live.get(LIVE_LANE_GR), s.dsp.lane_gain_reduction_db(0));
        assert_eq!(a.live.get(LIVE_LANE_GR + 1), 0.0, "lane B is off");
        // the scope is the input's (L+R)/2, not the output
        let mut last = vec![0.0; 1024];
        a.pre.latest(&mut last);
        assert_eq!(&last[..], &input[input.len() - 1024..]);
        assert_eq!(a.post.written(), 0);
    }

    #[test]
    fn bypass_lands_on_the_input() {
        let mut s = prepared();
        s.set_param("amount", 1.0);
        s.set_param("bypass", 1.0);
        let src = sine(48_000, 0.6);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        // Bypassed from the first sample (the smoother snaps), so the output is the input.
        for (a, b) in io[0][24_000..].iter().zip(&src[24_000..]) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn lane_fader_unity_is_exact_and_zero_is_silent() {
        assert_eq!(knob_to_lane_db(0.75), 0.0);
        assert_eq!(knob_to_lane_db(1.0), 10.0);
        assert_eq!(knob_to_lane_db(0.0), -60.0);
        // aLevel 0 with the dry off: silence.
        let mut s = prepared();
        s.set_param("aLevel", 0.0);
        let mut io = vec![sine(4096, 0.5), sine(4096, 0.5)];
        run(&mut s, &mut io, 512);
        assert!(io.iter().flatten().all(|v| *v == 0.0));
    }

    #[test]
    fn reset_returns_to_rest() {
        let mut s = prepared();
        s.set_param("amount", 1.0);
        let src = vec![sine(4096, 0.7), sine(4096, 0.7)];
        let mut a = src.clone();
        run(&mut s, &mut a, 512);
        s.reset();
        let mut b = src.clone();
        run(&mut s, &mut b, 512);
        assert_eq!(a, b, "a reset engine renders the same as a fresh one");
    }
}
