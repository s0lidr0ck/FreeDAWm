//! `fs_twine`: the owner's Twine (a clipper and a true-peak limiter, married, for mastering), a
//! faithful translation of `Twine/Source/TwineDSP.h`.
//!
//! Wrapper (`PluginProcessor.{h,cpp}`) audit, everything that affects the sound:
//! - `prepareToPlay` calls `dsp.prepare (sampleRate, samplesPerBlock)` and raises `forcePush`, so
//!   the first `processBlock` pushes every parameter before processing: here `prepare` prepares
//!   the engine and then hands it every stored value, in parameter order.
//! - `processBlock` pushes only changed values, in parameter order, before processing: here
//!   `set_param` passes the value straight to `setParameter`. Re-sending an unchanged value is a
//!   no-op for Character and Multiband (both act only on a change), a plain store for the knobs
//!   and switches, and recomputes the same crossover coefficients (filter state untouched) for
//!   the crossovers, so pushing on every set is equivalent.
//! - Latency: `prepareToPlay` reports `dsp.latencySamples()`; after each block's push,
//!   `processBlock` compares it with `latencyWanted` and, when Multiband changed it, stores the
//!   new value and has an `AsyncUpdater` call `setLatencySamples` off the audio thread. Here the
//!   same check runs at the start of `process` (after any `set_param`, i.e. after the push) and
//!   `latency()` returns `latencyWanted`: the host sees the new latency after the block in which
//!   the engine switched, which the mix engine's re-alignment (Task 3) picks up. The engine
//!   itself switches its internal dry/bypass delay at once, as the C++.
//! - Bypass: the engine's own `bypass` parameter is a 0.005-per-sample crossfade to the dry
//!   signal delayed by the same latency (translated in `process`). It is kept in order but hidden
//!   (group ""): the UI uses the module's bypass.
//! - APVTS ranges: floats are 0..1, choices are whole indices, bools 0/1. `ParamInfo::clamp`
//!   does the same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Bus layout: mono or stereo, in = out. `dsp.process` gets the channel count (clamped to 1..2
//!   by the engine); with one channel the engine copies it to the second side internally. Extra
//!   channels are left untouched (the Rust host contract). Unused output channels are cleared by
//!   the C++ wrapper (there are none: in and out match).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. It only changes values below
//!   ~1e-38, far under the fidelity bar; `silence_tail_is_cheap` guards the CPU side.
//! - No tail (`getTailLengthSeconds` is 0). The meters (`inPeak`, `outTruePeak`, `clipGr`,
//!   `limGr`, `bandGr`) are running maxima the editor resets with `exchange (0)`: here they are
//!   taken after every block, so `readouts` are block maxima. The input scope feeds the analysis
//!   ring. The preset dirty-tracking (`parameterChanged`), state save and editor are UI-only.
//!
//! Station (`ModuleRegistry.cpp`): key knobs drive, clip, ceiling, release; accent 0xFFD8B25C;
//! Twine is marked `fixedOutput` (last in the chain). Recorded here, not enforced by the module.

mod presets;
pub mod twine_dsp;

pub use presets::PRESETS;
pub use twine_dsp::{NUM_PARAMS, TwineDsp, knob_to_ceiling_db, knob_to_drive_db, knob_to_release_ms, knob_to_xover_hi, knob_to_xover_lo};

use crate::analysis::Analysis;
use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, gain_to_db};
use std::sync::Arc;

fn drive_text(v: f32) -> String {
    format!("+{:.1} dB", knob_to_drive_db(v))
}
fn clip_text(v: f32) -> String {
    if v <= 0.0005 { "Off".to_string() } else { format!("{:.0} %", (v * 100.0).round()) }
}
fn ceiling_text(v: f32) -> String {
    format!("{:.1} dBTP", knob_to_ceiling_db(v))
}
fn release_text(v: f32) -> String {
    let ms = knob_to_release_ms(v);
    if ms >= 100.0 { format!("{:.0} ms", ms.round()) } else { format!("{ms:.1} ms") }
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
pub static TWINE_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob01("drive", "Drive", 0.0, "Clipper", drive_text),
    knob01("clip", "Clip", 0.5, "Clipper", clip_text),
    fs_choice("shape", "Clip Shape", 1.0, "Clipper", &["Hard", "Soft", "Tape"]),
    fs_choice("character", "Character", 0.0, "Limiter", &["Transparent", "Punchy", "Loud"]),
    knob01("ceiling", "Ceiling", 0.916667, "Limiter", ceiling_text), // -1 dBTP (11/12)
    knob01("release", "Release", 0.5654, "Limiter", release_text),   // 100 ms
    fs_toggle("match", "Match", 0.0, "Listen"),
    fs_toggle("chaff", "Chaff", 0.0, "Listen"),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    // appended: multiband
    fs_toggle("multi", "Multiband", 0.0, "Multiband"),
    knob01("xoverLo", "Low / Mid", 0.4106, "Multiband", xover_lo_text),  // 150 Hz
    knob01("xoverHi", "Mid / High", 0.4881, "Multiband", xover_hi_text), // 3 kHz
];

pub static TWINE_INFO: PluginInfo = PluginInfo {
    id: "fs_twine",
    name: "Twine",
    short_name: "Twine",
    category: Category::FireStation,
    params: &TWINE_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &TWINE_INFO,
    make: || Box::new(Twine::new()),
    presets: PRESETS,
    order: 40,
    fidelity: Some(super::Fidelity { name: "twine", source_dir: "Twine", render_tool: "Twine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// [`Analysis::live`] indices: `inPeak` and `outTruePeak` (linear), `clipGr`, `limGr` and
/// `bandGr[0..3]` (dB), running maxima as the C++ atomics.
pub const LIVE_IN_PEAK: usize = 0;
pub const LIVE_OUT_TP: usize = 1;
pub const LIVE_CLIP_GR: usize = 2;
pub const LIVE_LIM_GR: usize = 3;
/// Low, mid, high at `LIVE_BAND_GR + band`.
pub const LIVE_BAND_GR: usize = 4;
pub const LIVE_LEN: usize = 7;

/// The Twine port.
///
/// `readouts` (maxima over the last block): `[0]` input peak dBFS, `[1]` output true peak dBFS,
/// `[2]` clipper gain reduction dB, `[3]` limiter gain reduction dB (in Multiband the full-band
/// stage plus the hardest-working band, as the C++), `[4]`/`[5]`/`[6]` the low/mid/high band
/// limiters' gain reduction dB in Multiband (0 when Multiband is off, as the C++ meters).
/// `gain_reduction_db` is clip GR + limiter GR. `analysis().post` carries the C++ scope's
/// source, the mono input `(L+R)/2`; `pre` is not fed. Its live values are the C++ meters the
/// editor polls with `exchange (0)` ([`LIVE_IN_PEAK`] ..): raised to each block's maxima here and
/// taken (reset) by the editor at 30 Hz.
pub struct Twine {
    p: Params,
    dsp: Box<TwineDsp>,
    analysis: Arc<Analysis>,
    channels: usize,
    max_block: i32,
    /// `latencyWanted`: what the processor last reported to the host.
    latency_wanted: i32,
    in_peak_db: f32,
    out_tp_db: f32,
    clip_gr_db: f32,
    lim_gr_db: f32,
    band_gr_db: [f32; 3],
}

impl Default for Twine {
    fn default() -> Self {
        Self::new()
    }
}

impl Twine {
    pub fn new() -> Self {
        let dsp = Box::new(TwineDsp::new());
        let latency_wanted = dsp.latency_samples();
        let mut s = Twine {
            p: Params::new(&TWINE_PARAMS),
            dsp,
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
            channels: 2,
            max_block: 512,
            latency_wanted,
            in_peak_db: f32::NAN,
            out_tp_db: f32::NAN,
            clip_gr_db: 0.0,
            lim_gr_db: 0.0,
            band_gr_db: [0.0; 3],
        };
        s.push_all();
        s.latency_wanted = s.dsp.latency_samples();
        s
    }

    /// Hands every stored value to the engine, in parameter order (the wrapper's `forcePush`).
    fn push_all(&mut self) {
        for i in 0..NUM_PARAMS {
            self.dsp.set_parameter(i, self.p.v(i));
        }
    }
}

impl Plugin for Twine {
    fn info(&self) -> &'static PluginInfo {
        &TWINE_INFO
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

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        // As `prepareToPlay` + the forced push of the first block: prepare (every buffer sized
        // for this rate and for Multiband, the longer mode), then every value; the latency is
        // reported straight after, as the C++ does.
        self.channels = channels.min(2);
        self.max_block = i32::try_from(max_block).unwrap_or(i32::MAX);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.dsp.prepare(sr, self.max_block);
        self.push_all();
        self.latency_wanted = self.dsp.latency_samples();
        self.dsp.take_meters();
    }

    fn reset(&mut self) {
        // The C++ has no reset; back to rest is a re-prepare at the same rate, which re-fills
        // every buffer in place (no allocation) and keeps the parameters.
        let sr = self.dsp.sample_rate();
        self.dsp.prepare(sr, self.max_block);
        self.latency_wanted = self.dsp.latency_samples();
        self.dsp.take_meters();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        // Multiband on or off changes the latency (and only that does).
        if self.dsp.latency_samples() != self.latency_wanted {
            self.latency_wanted = self.dsp.latency_samples();
        }
        let nc = self.channels.min(io.len());
        if nc > 0 {
            self.dsp.process(io, nc, frames, Some(&self.analysis.post));
        }
        let m = self.dsp.take_meters();
        self.in_peak_db = gain_to_db(m.in_peak);
        self.out_tp_db = gain_to_db(m.out_true_peak);
        self.clip_gr_db = m.clip_gr;
        self.lim_gr_db = m.lim_gr;
        self.band_gr_db = m.band_gr;
        // `raiseMax` into what the editor takes.
        let live = &self.analysis.live;
        live.raise_max(LIVE_IN_PEAK, m.in_peak);
        live.raise_max(LIVE_OUT_TP, m.out_true_peak);
        live.raise_max(LIVE_CLIP_GR, m.clip_gr);
        live.raise_max(LIVE_LIM_GR, m.lim_gr);
        for (i, g) in m.band_gr.iter().enumerate() {
            live.raise_max(LIVE_BAND_GR + i, *g);
        }
    }

    fn latency(&self) -> usize {
        usize::try_from(self.latency_wanted).unwrap_or(0)
    }

    fn gain_reduction_db(&self) -> f32 {
        self.clip_gr_db + self.lim_gr_db
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.in_peak_db;
        out[1] = self.out_tp_db;
        out[2] = self.clip_gr_db;
        out[3] = self.lim_gr_db;
        out[4] = self.band_gr_db[0];
        out[5] = self.band_gr_db[1];
        out[6] = self.band_gr_db[2];
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
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

    fn prepared() -> Twine {
        let mut s = Twine::new();
        s.prepare(48_000.0, 512, 2);
        s
    }

    fn run(s: &mut Twine, io: &mut [Vec<f32>], block: usize) {
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

    const HOSTILE: [f32; 9] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0];

    #[test]
    fn table_matches_the_cpp_order_and_defaults() {
        let ids: Vec<&str> = TWINE_PARAMS.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["drive", "clip", "shape", "character", "ceiling", "release", "match", "chaff", "bypass", "multi", "xoverLo", "xoverHi"]);
        let defaults: Vec<f32> = TWINE_PARAMS.iter().map(|p| p.default).collect();
        assert_eq!(defaults, [0.0, 0.5, 1.0, 0.0, 0.916667, 0.5654, 0.0, 0.0, 0.0, 0.0, 0.4106, 0.4881]);
        assert_eq!(TWINE_PARAMS[2].choices, ["Hard", "Soft", "Tape"]);
        assert_eq!(TWINE_PARAMS[3].choices, ["Transparent", "Punchy", "Loud"]);
        // formatParam, through the engine's own maps
        assert_eq!(TWINE_PARAMS[0].format(0.5), "+12.0 dB");
        assert_eq!(TWINE_PARAMS[1].format(0.0), "Off");
        assert_eq!(TWINE_PARAMS[1].format(0.5), "50 %");
        assert_eq!(TWINE_PARAMS[4].format(0.916667), "-1.0 dBTP");
        // 0.5654 maps to 99.99.. ms, under the C++'s 100 ms switch to whole milliseconds
        assert_eq!(TWINE_PARAMS[5].format(0.5654), "100.0 ms");
        assert_eq!(TWINE_PARAMS[5].format(1.0), "1000 ms");
        assert_eq!(TWINE_PARAMS[5].format(0.0), "5.0 ms");
        assert_eq!(TWINE_PARAMS[10].format(0.4106), "150 Hz");
        assert_eq!(TWINE_PARAMS[10].format(1.0), "1.00 kHz");
        assert_eq!(TWINE_PARAMS[11].format(0.4881), "3.00 kHz");
        assert_eq!(TWINE_PARAMS[8].group, "", "the engine's bypass is hidden");
    }

    #[test]
    fn registered_and_reports_the_cpp_latency() {
        let mut p = crate::create("fs_twine").unwrap();
        assert_eq!(p.info().category, Category::FireStation);
        // osDelay 64 + (tpDelay 16 + lookMax 240 - 1) at 48 kHz; Multiband adds one more stage.
        p.prepare(48_000.0, 512, 2);
        assert_eq!(p.latency(), 319);
        p.set_param("multi", 1.0);
        p.prepare(48_000.0, 512, 2);
        assert_eq!(p.latency(), 574);
        p.set_param("multi", 0.0);
        p.prepare(96_000.0, 512, 2);
        assert_eq!(p.latency(), 64 + 16 + 480 - 1);
    }

    #[test]
    fn latency_switch_follows_the_processor() {
        let mut s = prepared();
        let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
        s.process(&mut io, 512);
        assert_eq!(s.latency(), 319);
        // The push changes the engine at once; the processor notices at the next block and
        // reports it (the C++ AsyncUpdater's `setLatencySamples`).
        s.set_param("multi", 1.0);
        assert_eq!(s.dsp.latency_samples(), 574);
        assert_eq!(s.latency(), 319, "not reported before the block that applies it");
        s.process(&mut io, 512);
        assert_eq!(s.latency(), 574);
        s.set_param("multi", 0.0);
        s.process(&mut io, 512);
        assert_eq!(s.latency(), 319);
        // Character never changes it.
        for c in 0..3 {
            s.set_param("character", c as f32);
            s.process(&mut io, 512);
            assert_eq!(s.latency(), 319);
        }
    }

    #[test]
    fn live_values_are_running_maxima_the_editor_takes() {
        let mut s = prepared();
        s.set_param("drive", 0.5);
        let mut io = vec![sine(9600, 0.5), sine(9600, 0.5)];
        run(&mut s, &mut io, 512);
        let l = &s.analysis().unwrap().live;
        assert_eq!(l.len(), LIVE_LEN);
        assert!((gain_to_db(l.get(LIVE_IN_PEAK)) - gain_to_db(0.5)).abs() < 0.1);
        assert!(l.get(LIVE_CLIP_GR) > 1.0 && l.get(LIVE_LIM_GR) > 1.0);
        assert!(l.take(LIVE_CLIP_GR) > 1.0);
        assert_eq!(l.get(LIVE_CLIP_GR), 0.0, "taken");
    }

    #[test]
    fn readouts_are_block_maxima() {
        let mut s = prepared();
        s.set_param("drive", 0.5);
        let mut io = vec![sine(9600, 0.5), sine(9600, 0.5)];
        run(&mut s, &mut io, 512);
        let mut r = [f32::NAN; 8];
        s.readouts(&mut r);
        assert!((r[0] - gain_to_db(0.5)).abs() < 0.1, "{r:?}");
        assert!(r[1] <= -0.9 && r[1] > -3.0, "out true peak under the -1 dBTP ceiling: {r:?}");
        assert!(r[2] > 1.0, "the clipper works: {r:?}");
        assert!(r[3] > 1.0, "the limiter works: {r:?}");
        assert_eq!(&r[4..7], &[0.0, 0.0, 0.0], "no bands without Multiband");
        assert!(r[7].is_nan());
        assert_eq!(s.gain_reduction_db(), r[2] + r[3]);

        // Later blocks are silent: their readouts are their own (the C++ `exchange (0)`).
        s.set_param("drive", 0.0);
        for _ in 0..8 {
            let mut quiet = vec![vec![0.0f32; 512], vec![0.0f32; 512]];
            s.process(&mut quiet, 512);
        }
        s.readouts(&mut r);
        assert_eq!(r[0], gain_to_db(0.0));
        assert_eq!(r[2], 0.0);

        // Multiband: the band limiters report.
        let mut s = prepared();
        s.set_param("drive", 0.6);
        s.set_param("multi", 1.0);
        let mut io = vec![sine(19_200, 0.5), sine(19_200, 0.5)];
        run(&mut s, &mut io, 512);
        s.readouts(&mut r);
        assert!(r[4..7].iter().any(|&g| g > 0.5), "{r:?}");
        assert!(r[4..7].iter().all(|g| g.is_finite() && *g >= 0.0), "{r:?}");
    }

    #[test]
    fn analysis_post_is_the_cpp_scope_source() {
        let mut s = prepared();
        let a = s.analysis().unwrap();
        assert!(Arc::ptr_eq(&a, &s.analysis().unwrap()), "the same streams every call");
        let l = sine(1000, 0.4);
        let r = sine(1000, -0.2);
        let mut io = vec![l.clone(), r.clone()];
        s.process(&mut io, 1000);
        let mut got = vec![0.0f32; 1000];
        assert_eq!(a.post.latest(&mut got), 1000);
        for i in 0..1000 {
            assert_eq!(got[i], (0.5 * f64::from(l[i] + r[i])) as f32, "sample {i}");
        }
        assert_eq!(a.pre.written(), 0);
        let mut scope = vec![0.0f32; 1000];
        s.dsp.copy_scope(&mut scope);
        assert_eq!(scope, got);
    }

    #[test]
    fn every_factory_preset_parses() {
        let presets = factory_presets("fs_twine");
        assert_eq!(presets.len(), 10);
        for (name, json) in presets {
            let p = FsPreset::parse(json, &TWINE_INFO).unwrap();
            assert_eq!(&p.name, name);
            let raw: serde_json::Value = serde_json::from_str(json).unwrap();
            for key in raw["values"].as_object().unwrap().keys() {
                assert!(p.values.contains_key(key), "{name}: {key} did not resolve");
            }
            for key in ["drive", "clip", "shape", "character", "ceiling", "release"] {
                assert!(p.values.contains_key(key), "{name}: {key}");
            }
        }
    }

    #[test]
    fn hostile_params_stay_finite() {
        // Bypass stays off (it would hide the wet path); the choices are set per case.
        let skip = ["shape", "character", "multi", "bypass"];
        for bad in HOSTILE {
            for shape in 0..3 {
                for character in 0..3 {
                    for multi in 0..2 {
                        let mut s = prepared();
                        s.set_param("shape", shape as f32);
                        s.set_param("character", character as f32);
                        s.set_param("multi", multi as f32);
                        for info in TWINE_PARAMS.iter().filter(|p| !skip.contains(&p.id)) {
                            assert!(s.set_param(info.id, bad));
                        }
                        assert_eq!(s.param("bypass"), Some(0.0));
                        let mut io = vec![sine(2048, 0.9), sine(2048, -0.9)];
                        run(&mut s, &mut io, 256);
                        assert!(all_finite(&io), "shape {shape}, character {character}, multi {multi}, value {bad}");
                    }
                }
            }
        }
    }

    #[test]
    fn hostile_choice_params_stay_finite() {
        for bad in HOSTILE {
            let mut s = prepared();
            s.set_param("drive", 1.0);
            for id in ["shape", "character", "multi", "match", "chaff"] {
                assert!(s.set_param(id, bad));
            }
            assert_eq!(s.param("bypass"), Some(0.0));
            let mut io = vec![sine(4096, 0.9), sine(4096, -0.9)];
            run(&mut s, &mut io, 256);
            assert!(all_finite(&io), "value {bad}");
            assert!(s.latency() == 319 || s.latency() == 574);
        }
    }

    #[test]
    fn hostile_bypass_values_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
            for multi in 0..2 {
                let mut s = prepared();
                s.set_param("drive", 1.0);
                s.set_param("multi", multi as f32);
                let src = [sine(4096, 0.9), sine(4096, -0.9)];
                // Half the run with the wet path engaged, then the hostile bypass value mid-stream.
                let mut a = vec![src[0][..2048].to_vec(), src[1][..2048].to_vec()];
                run(&mut s, &mut a, 256);
                assert!(s.set_param("bypass", bad));
                let mut b = vec![src[0][2048..].to_vec(), src[1][2048..].to_vec()];
                run(&mut s, &mut b, 256);
                assert!(all_finite(&a) && all_finite(&b), "multi {multi}, bypass {bad}");
            }
        }
    }

    #[test]
    fn switching_quality_and_model_mid_stream_is_safe() {
        // No quality setting in this engine: shape, character and Multiband are what switch.
        let mut s = prepared();
        s.set_param("drive", 0.7);
        let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
        let src = sine(128 * 400, 0.8);
        for blk in 0..400 {
            s.set_param("shape", (blk % 3) as f32);
            s.set_param("character", ((blk / 2) % 3) as f32);
            s.set_param("multi", ((blk / 5) % 2) as f32);
            s.set_param("xoverLo", (blk % 7) as f32 / 7.0);
            for b in &mut buf {
                b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
            }
            s.process(&mut buf, 128);
            assert!(all_finite(&buf), "block {blk}");
            assert!(buf.iter().flatten().all(|v| v.abs() <= 1.0), "block {blk}: over the ceiling");
        }
    }

    #[test]
    fn mono_and_wide_buffers() {
        // Mono: one channel processed.
        let mut s = Twine::new();
        s.prepare(48_000.0, 512, 1);
        s.set_param("drive", 0.5);
        let mut mono = vec![sine(4096, 0.5)];
        let orig = mono.clone();
        run(&mut s, &mut mono, 512);
        assert!(all_finite(&mono));
        assert_ne!(mono, orig);

        // Six channels: the first two processed, the rest untouched.
        let mut s = Twine::new();
        s.prepare(48_000.0, 512, 6);
        let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(4096, 0.1 * (c + 1) as f32)).collect();
        let orig = wide.clone();
        s.set_param("drive", 1.0);
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
        for multi in 0..2 {
            for character in 0..3 {
                let time = |signal_s: usize, silence_s: usize| {
                    let mut s = prepared();
                    s.set_param("drive", 0.6);
                    s.set_param("character", character as f32);
                    s.set_param("multi", multi as f32);
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
                cases.push((multi, character, tail, full));
                assert!(tail < 4.0 * full, "multi {multi} character {character}: silence tail {tail:.3}s vs signal {full:.3}s");
            }
        }
        println!("{cases:?}");
    }

    #[test]
    fn process_does_not_allocate() {
        // No counting allocator in this crate: every heap buffer is made in `prepare`, so their
        // addresses and sizes across 1000 blocks of switching (Character and Multiband re-fill
        // the limiters' buffers) and a reset are the whole story; the host buffers keep theirs.
        let mut s = prepared();
        s.set_param("drive", 0.6);
        let engine_ptr: *const TwineDsp = &*s.dsp;
        let before = s.dsp.buffer_fingerprint();
        let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
        let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
        for blk in 0..1000 {
            s.set_param("multi", ((blk / 4) % 2) as f32);
            s.set_param("character", ((blk / 3) % 3) as f32);
            s.set_param("xoverHi", (blk % 5) as f32 / 5.0);
            s.process(&mut io, 256);
        }
        s.reset();
        assert_eq!(engine_ptr, &*s.dsp as *const TwineDsp);
        assert_eq!(before, s.dsp.buffer_fingerprint());
        assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
        assert!(all_finite(&io));
    }

    #[test]
    fn bypass_lands_on_the_input_at_the_same_latency() {
        for multi in 0..2 {
            let mut s = prepared();
            s.set_param("drive", 1.0);
            s.set_param("multi", multi as f32);
            s.set_param("bypass", 1.0);
            let lat = s.dsp.latency_samples() as usize;
            let src = sine(48_000, 0.6);
            let mut io = vec![src.clone(), src.clone()];
            run(&mut s, &mut io, 512);
            // The 0.005-per-sample crossfade, then the input delayed by the latency.
            for i in 24_000..48_000 {
                assert!((io[0][i] - src[i - lat]).abs() < 1e-6, "multi {multi}, sample {i}");
            }
        }
    }

    #[test]
    fn reset_returns_to_rest() {
        let mut s = prepared();
        s.set_param("drive", 0.8);
        s.set_param("multi", 1.0);
        let src = vec![sine(4096, 0.7), sine(4096, 0.7)];
        let mut a = src.clone();
        run(&mut s, &mut a, 512);
        s.reset();
        let mut b = src.clone();
        run(&mut s, &mut b, 512);
        assert_eq!(a, b, "a reset engine renders the same as a freshly prepared one");
    }
}
