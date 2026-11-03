//! `fs_sat`: the owner's SatEngine (FireStation Engine 8), a faithful translation of
//! `Engine 8 - Sat/Source/SatEngineDSP.h` with `SatHalfband.h` and `SatGainTable.h`.
//!
//! Wrapper (`PluginProcessor.cpp`) audit, everything that affects the sound:
//! - `prepareToPlay` pushes every parameter, then `prepare(sampleRate)` (glides start at their
//!   targets): here `prepare` hands the stored values to the engine before preparing it.
//! - `processBlock` pushes only changed values, in parameter order, before processing: here
//!   `set_param` passes the value straight to `setParameter` (re-sending an unchanged circuit,
//!   variant or quality is a no-op in the engine, so pushing on every set is equivalent).
//! - APVTS ranges: floats are 0..1, choices are whole indices, bools 0/1. `ParamInfo::clamp`
//!   does the same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Unused output channels are cleared; the engine processes at most 2 channels and leaves the
//!   rest untouched (the Rust host contract for extra channels).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. It only changes values below
//!   ~1e-38, far under the fidelity bar; `silence_tail_is_cheap` guards the CPU side.
//! - No `setLatencySamples` anywhere: the half-band IIR oversamplers report no latency, so
//!   `latency()` is 0, and the C++ render tool's output is not shifted either.
//! - No tail (`getTailLengthSeconds` is 0). Meters (`takeInputPeak`/`takeOutputPeak`) feed
//!   `readouts`. The preset dirty-tracking and editor are UI-only.

mod presets;
pub mod sat_engine_dsp;
pub mod sat_gain_table;
pub mod sat_halfband;

pub use presets::PRESETS;

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, gain_to_db};
use sat_engine_dsp::{NUM_PARAMS, SatEngineDsp, knob_to_bias, knob_to_drive_db, knob_to_out_db, knob_to_tilt_db};

fn drive_text(v: f32) -> String {
    format!("{:+.1} dB", knob_to_drive_db(v))
}
fn bias_text(v: f32) -> String {
    let b = knob_to_bias(v);
    if b.abs() < 0.005 { "Neutral".to_string() } else { format!("{:+.0}%", b * 100.0) }
}
fn focus_text(v: f32) -> String {
    let t = knob_to_tilt_db(v);
    if t.abs() < 0.05 {
        "Flat".to_string()
    } else if t > 0.0 {
        format!("Lows +{t:.1} dB")
    } else {
        format!("Highs +{:.1} dB", -t)
    }
}
fn pct_text(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}
fn out_text(v: f32) -> String {
    format!("{:+.1} dB", knob_to_out_db(v))
}

/// Parameters in the C++ order (`Parameters.h`), ids and defaults verbatim.
pub static SAT_PARAMS: [ParamInfo; NUM_PARAMS] = [
    fs_choice("circuit", "Circuit", 1.0, "Circuit", &["Iron", "Valve", "Desk", "Bridge", "FET"]),
    fs_choice("variant", "Variant", 0.0, "Circuit", &["A", "B", "C"]),
    knob01("drive", "Drive", 0.4, "Drive", drive_text),
    knob01("bias", "Bias", 0.5, "Drive", bias_text),
    knob01("focus", "Focus", 0.5, "Drive", focus_text),
    knob01("air", "Air", 0.0, "Drive", pct_text),
    knob01("mix", "Mix", 1.0, "Output", pct_text),
    knob01("output", "Output", 0.5, "Output", out_text),
    fs_toggle("autoGain", "Auto Gain", 1.0, "Output"),
    fs_choice("chanMode", "Channels", 0.0, "Options", &["Stereo", "M/S"]),
    fs_choice("quality", "Quality", 1.0, "Options", &["Eco", "Normal", "Insane"]),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
];

pub static SAT_INFO: PluginInfo = PluginInfo {
    id: "fs_sat",
    name: "Sat",
    short_name: "Sat",
    category: Category::FireStation,
    params: &SAT_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &SAT_INFO,
    make: || Box::new(Sat::new()),
    presets: PRESETS,
    order: 10,
    fidelity: Some(super::Fidelity {
        name: "sat",
        source_dir: "Engine 8 - Sat",
        render_tool: "SatEngine",
        tables: &[("Source/SatGainTable.h", "crates/dsp/src/plugins/fs/sat/sat_gain_table.rs")],
    }),
    prepare_state: None,
    actions: &[],
};

/// The SatEngine port.
///
/// `readouts`: `[0]` input peak dBFS and `[1]` output peak dBFS over the last block, `[2]` drive
/// in dB (the engine's knob map).
pub struct Sat {
    p: Params,
    dsp: Box<SatEngineDsp>,
    channels: usize,
    in_peak_db: f32,
    out_peak_db: f32,
}

impl Default for Sat {
    fn default() -> Self {
        Self::new()
    }
}

impl Sat {
    pub fn new() -> Self {
        let mut s = Sat { p: Params::new(&SAT_PARAMS), dsp: Box::new(SatEngineDsp::new()), channels: 2, in_peak_db: f32::NAN, out_peak_db: f32::NAN };
        s.push_all();
        s
    }

    /// Hands every stored value to the engine, in parameter order.
    fn push_all(&mut self) {
        for i in 0..NUM_PARAMS {
            self.dsp.set_parameter(i, self.p.v(i));
        }
    }
}

impl Plugin for Sat {
    fn info(&self) -> &'static PluginInfo {
        &SAT_INFO
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
        // As `prepareToPlay`: parameters first, so the oversampling is built for the circuit and
        // quality actually selected, then prepare. Every buffer is fixed-size (sized for Insane,
        // 8x), so nothing is allocated here or later.
        self.channels = channels.min(2);
        self.push_all();
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.dsp.prepare(sr);
    }

    fn reset(&mut self) {
        self.dsp.reset();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let nc = self.channels.min(io.len());
        self.dsp.process(io, nc, frames);
        self.in_peak_db = gain_to_db(self.dsp.take_input_peak());
        self.out_peak_db = gain_to_db(self.dsp.take_output_peak());
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.in_peak_db;
        out[1] = self.out_peak_db;
        out[2] = knob_to_drive_db(self.dsp.drive_knob());
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

    fn prepared() -> Sat {
        let mut s = Sat::new();
        s.prepare(48_000.0, 512, 2);
        s
    }

    fn run(s: &mut Sat, io: &mut [Vec<f32>], block: usize) {
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

    #[test]
    fn table_matches_the_cpp_order_and_defaults() {
        let ids: Vec<&str> = SAT_PARAMS.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["circuit", "variant", "drive", "bias", "focus", "air", "mix", "output", "autoGain", "chanMode", "quality", "bypass"]);
        let defaults: Vec<f32> = SAT_PARAMS.iter().map(|p| p.default).collect();
        assert_eq!(defaults, [1.0, 0.0, 0.4, 0.5, 0.5, 0.0, 1.0, 0.5, 1.0, 0.0, 1.0, 0.0]);
        assert_eq!(SAT_PARAMS[2].format(0.4), "+6.0 dB");
        assert_eq!(SAT_PARAMS[4].format(0.5), "Flat");
        assert_eq!(SAT_PARAMS[4].format(0.0), "Lows +12.0 dB");
        assert_eq!(SAT_PARAMS[7].format(0.5), "+0.0 dB");
    }

    #[test]
    fn registered_and_reports_no_latency() {
        let p = crate::create("fs_sat").unwrap();
        assert_eq!(p.info().category, Category::FireStation);
        assert_eq!(p.latency(), 0, "the C++ reports no latency (IIR half-bands)");
    }

    #[test]
    fn readouts_meter_and_drive() {
        let mut s = prepared();
        let mut io = vec![sine(4800, 0.5), sine(4800, 0.5)];
        run(&mut s, &mut io, 512);
        let mut r = [f32::NAN; 8];
        s.readouts(&mut r);
        assert!((r[0] - gain_to_db(0.5)).abs() < 0.1, "{r:?}");
        assert!(r[1].is_finite() && r[1] > -20.0, "{r:?}");
        assert_eq!(r[2], 6.0);
        assert!(r[3].is_nan());
    }

    #[test]
    fn every_factory_preset_parses() {
        let presets = factory_presets("fs_sat");
        assert_eq!(presets.len(), 45);
        for (name, json) in presets {
            let p = FsPreset::parse(json, &SAT_INFO).unwrap();
            assert_eq!(&p.name, name);
            for key in ["circuit", "variant", "drive", "bias", "focus", "air", "mix", "chanMode"] {
                assert!(p.values.contains_key(key), "{name}: {key}");
            }
        }
    }

    #[test]
    fn hostile_params_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
            for circuit in 0..5 {
                let mut s = prepared();
                s.set_param("circuit", circuit as f32);
                // Bypass stays off: a hostile value that turned it on would hide the wet path.
                for info in SAT_PARAMS.iter().filter(|p| p.id != "circuit" && p.id != "bypass") {
                    assert!(s.set_param(info.id, bad));
                }
                assert_eq!(s.param("bypass"), Some(0.0));
                let mut io = vec![sine(2048, 0.9), sine(2048, -0.9)];
                run(&mut s, &mut io, 256);
                assert!(all_finite(&io), "circuit {circuit}, value {bad}");
            }
        }
    }

    #[test]
    fn hostile_bypass_values_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
            for circuit in 0..5 {
                let mut s = prepared();
                s.set_param("circuit", circuit as f32);
                s.set_param("drive", 1.0);
                let src = [sine(4096, 0.9), sine(4096, -0.9)];
                // Half the run with the wet path engaged, then the hostile bypass value mid-stream.
                let mut a = vec![src[0][..2048].to_vec(), src[1][..2048].to_vec()];
                run(&mut s, &mut a, 256);
                assert!(s.set_param("bypass", bad));
                let mut b = vec![src[0][2048..].to_vec(), src[1][2048..].to_vec()];
                run(&mut s, &mut b, 256);
                assert!(all_finite(&a) && all_finite(&b), "circuit {circuit}, bypass {bad}");
            }
        }
    }

    #[test]
    fn switching_quality_and_model_mid_stream_is_safe() {
        let mut s = prepared();
        let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
        let src = sine(128 * 300, 0.8);
        for blk in 0..300 {
            s.set_param("circuit", (blk % 5) as f32);
            s.set_param("variant", (blk % 3) as f32);
            s.set_param("quality", (blk % 3) as f32);
            s.set_param("chanMode", (blk % 2) as f32);
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
        let mut s = Sat::new();
        s.prepare(48_000.0, 512, 1);
        let mut mono = vec![sine(4096, 0.5)];
        let orig = mono.clone();
        run(&mut s, &mut mono, 512);
        assert!(all_finite(&mono));
        assert_ne!(mono, orig);

        // Six channels: the first two processed, the rest untouched.
        let mut s = Sat::new();
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
    }

    #[test]
    fn silence_tail_is_cheap() {
        let sr = 48_000usize;
        let mut cases = Vec::new();
        for circuit in 0..5 {
            let time = |signal_s: usize, silence_s: usize| {
                let mut s = prepared();
                s.set_param("circuit", circuit as f32);
                s.set_param("drive", 0.8);
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
            cases.push((circuit, tail, full));
            assert!(tail < 4.0 * full, "circuit {circuit}: silence tail {tail:.3}s vs signal {full:.3}s");
        }
        println!("{cases:?}");
    }

    #[test]
    fn process_does_not_allocate() {
        // No counting allocator in this crate: the engine holds no heap memory at all (every
        // buffer is a fixed-size array inside one boxed struct), so its address and size are the
        // whole story; the host buffers keep their pointers and capacities across 1000 blocks.
        let mut s = prepared();
        let engine_ptr: *const SatEngineDsp = &*s.dsp;
        let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
        let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
        for blk in 0..1000 {
            s.set_param("quality", (blk % 3) as f32);
            s.set_param("circuit", ((blk / 3) % 5) as f32);
            s.process(&mut io, 256);
        }
        assert_eq!(engine_ptr, &*s.dsp as *const SatEngineDsp);
        assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
        assert!(all_finite(&io));
    }

    #[test]
    fn bypass_lands_exactly_on_the_input() {
        let mut s = prepared();
        s.set_param("drive", 1.0);
        s.set_param("bypass", 1.0);
        let src = sine(48_000, 0.6);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        // 15 ms glide, then exactly the input.
        assert_eq!(io[0][24_000..], src[24_000..]);
    }
}
