//! `fs_tape`: the owner's TapeEngine (FireStation Engine 9), a faithful translation of
//! `Engine 9 - Tape/Source/TapeEngineDSP.h` with `TapeHalfband.h` and `TapeGainTable.h`.
//!
//! Wrapper (`PluginProcessor.cpp`) audit, everything that affects the sound:
//! - `prepareToPlay` pushes every parameter, then `prepare(sampleRate)` (oversampling built for the
//!   selected quality and voicing, glides start at their targets, transport phases and hiss RNGs
//!   seeded): here `prepare` hands the stored values to the engine before preparing it.
//! - `processBlock` pushes only changed values, in parameter order, before processing: here
//!   `set_param` passes the value straight to `setParameter` (re-sending an unchanged machine,
//!   formula, speed or quality is a no-op in the engine, so pushing on every set is equivalent).
//! - `setLatencySamples(dsp.getLatencySamples())` after prepare: the transport's constant 2 ms
//!   (`lround(0.002 * sr)`), whatever the knobs say. Here `latency()`. The dry and bypass paths are
//!   delayed by the same amount inside the engine.
//! - `getTailLengthSeconds()` is 0.05 s (transport delay plus the head bump's ring): `tail_samples()`.
//! - APVTS ranges: floats are 0..1, choices are whole indices, bools 0/1. `ParamInfo::clamp`
//!   does the same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Unused output channels are cleared; the engine processes at most 2 channels and leaves the
//!   rest untouched (the Rust host contract for extra channels). Mono: channel 1 reads as silence.
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. It only changes values below
//!   ~1e-38, far under the fidelity bar; `silence_tail_is_cheap` guards the CPU side.
//! - Meters (`takeInputPeak`/`takeOutputPeak`) feed `readouts`. The preset dirty-tracking,
//!   `TapeScope` and the editor are UI-only.

mod presets;
pub mod tape_engine_dsp;
// Generated from the C++ verbatim: its number text (an f32 table with more digits than an f32
// holds, a measured 1.57070 that is not meant as pi / 2) is kept as written, rounding exactly as the C++ does.
#[allow(clippy::excessive_precision, clippy::approx_constant)]
pub mod tape_gain_table;
pub mod tape_halfband;

pub use presets::PRESETS;

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, gain_to_db};
use tape_engine_dsp::{NUM_PARAMS, TapeEngineDsp, flutter_percent, knob_to_bias, knob_to_drive_db, knob_to_hiss_db, knob_to_out_db};

fn drive_text(v: f32) -> String {
    format!("{:+.1} dB", knob_to_drive_db(v))
}
fn bias_text(v: f32) -> String {
    let b = knob_to_bias(v);
    if b.abs() < 0.005 {
        "Aligned".to_string()
    } else if b > 0.0 {
        format!("Over {:.0}%", b * 100.0)
    } else {
        format!("Under {:.0}%", -b * 100.0)
    }
}
fn hiss_text(v: f32) -> String {
    // The engine treats a knob below 0.001 as off.
    if v < 0.001 { "Off".to_string() } else { format!("{:+.1} dB", knob_to_hiss_db(v)) }
}
fn pct_text(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}
fn out_text(v: f32) -> String {
    format!("{:+.1} dB", knob_to_out_db(v))
}

/// Parameters in the C++ order (`Parameters.h`), ids and defaults verbatim.
pub static TAPE_PARAMS: [ParamInfo; NUM_PARAMS] = [
    fs_choice("machine", "Machine", 1.0, "Machine", &["Two-Track", "Multitrack", "Valve", "Cassette"]),
    // One tape slot shared by every machine: A/B/C are Standard / High Output / Vintage on the
    // reels and Type I / II / IV on Cassette (TapeConfig.h).
    fs_choice("formula", "Tape", 0.0, "Machine", &["A", "B", "C"]),
    fs_choice("speed", "Speed", 1.0, "Machine", &["7.5 ips", "15 ips", "30 ips"]),
    knob01("drive", "Drive", 0.3, "Tape", drive_text),
    knob01("bias", "Bias", 0.5, "Tape", bias_text),
    knob01("flutter", "Wow & Flutter", 0.0, "Tape", pct_text),
    knob01("hiss", "Hiss", 0.0, "Tape", hiss_text),
    knob01("mix", "Mix", 1.0, "Output", pct_text),
    knob01("output", "Output", 0.5, "Output", out_text),
    fs_toggle("autoGain", "Auto Gain", 1.0, "Output"),
    fs_choice("quality", "Quality", 1.0, "Options", &["Eco", "Normal", "Insane"]),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
];

pub static TAPE_INFO: PluginInfo = PluginInfo {
    id: "fs_tape",
    name: "Tape",
    short_name: "Tape",
    category: Category::FireStation,
    params: &TAPE_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &TAPE_INFO,
    make: || Box::new(Tape::new()),
    presets: PRESETS,
    order: 30,
    fidelity: Some(super::Fidelity {
        name: "tape",
        source_dir: "Engine 9 - Tape",
        render_tool: "TapeEngine",
        tables: &[("Source/TapeGainTable.h", "crates/dsp/src/plugins/fs/tape/tape_gain_table.rs")],
    }),
    prepare_state: None,
    actions: &[],
};

/// Highest sample rate handed to the engine: its delay lines scale with the rate, so this bounds
/// the one allocation in `prepare` (any real host rate is far below it).
const MAX_SAMPLE_RATE: f64 = 1_536_000.0;

/// The TapeEngine port.
///
/// `readouts`: `[0]` input peak dBFS and `[1]` output peak dBFS over the last block, `[2]` drive
/// in dB (the engine's knob map), `[3]` the Wow & Flutter depth the engine reports for the knob
/// and machine (`flutterPercent`: peak speed deviation, in %).
pub struct Tape {
    p: Params,
    dsp: Box<TapeEngineDsp>,
    channels: usize,
    sample_rate: f64,
    in_peak_db: f32,
    out_peak_db: f32,
}

impl Default for Tape {
    fn default() -> Self {
        Self::new()
    }
}

impl Tape {
    pub fn new() -> Self {
        let mut s = Tape {
            p: Params::new(&TAPE_PARAMS),
            dsp: Box::new(TapeEngineDsp::new()),
            channels: 2,
            sample_rate: 48_000.0,
            in_peak_db: f32::NAN,
            out_peak_db: f32::NAN,
        };
        s.push_all();
        // Rust safety: the engine's transport divides by the sample rate, which is 0 until a
        // prepare. Preparing here (off the audio thread) means a process before `prepare` is sane.
        s.dsp.prepare(s.sample_rate);
        s
    }

    /// Hands every stored value to the engine, in parameter order.
    fn push_all(&mut self) {
        for i in 0..NUM_PARAMS {
            self.dsp.set_parameter(i, self.p.v(i));
        }
    }
}

impl Plugin for Tape {
    fn info(&self) -> &'static PluginInfo {
        &TAPE_INFO
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
        // As `prepareToPlay`: parameters first, so the oversampling is built for the machine and
        // quality actually selected, then prepare. The delay lines are allocated here, sized for
        // Insane (8x); nothing is allocated later.
        self.channels = channels.min(2);
        self.push_all();
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate).min(MAX_SAMPLE_RATE) } else { 48_000.0 };
        self.sample_rate = sr;
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

    fn latency(&self) -> usize {
        usize::try_from(self.dsp.get_latency_samples()).unwrap_or(0)
    }

    fn tail_samples(&self) -> usize {
        // `getTailLengthSeconds()`: 0.05 s.
        (0.05 * self.sample_rate).round() as usize
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.in_peak_db;
        out[1] = self.out_peak_db;
        out[2] = knob_to_drive_db(self.dsp.drive_knob());
        out[3] = flutter_percent(self.dsp.machine(), self.dsp.flutter_knob()) as f32;
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

    fn prepared() -> Tape {
        let mut s = Tape::new();
        s.prepare(48_000.0, 512, 2);
        s
    }

    fn run(s: &mut Tape, io: &mut [Vec<f32>], block: usize) {
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
        let ids: Vec<&str> = TAPE_PARAMS.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["machine", "formula", "speed", "drive", "bias", "flutter", "hiss", "mix", "output", "autoGain", "quality", "bypass"]);
        let defaults: Vec<f32> = TAPE_PARAMS.iter().map(|p| p.default).collect();
        assert_eq!(defaults, [1.0, 0.0, 1.0, 0.3, 0.5, 0.0, 0.0, 1.0, 0.5, 1.0, 1.0, 0.0]);
        assert_eq!(TAPE_PARAMS[3].format(0.3), "+3.0 dB");
        assert_eq!(TAPE_PARAMS[4].format(0.5), "Aligned");
        assert_eq!(TAPE_PARAMS[4].format(1.0), "Over 100%");
        assert_eq!(TAPE_PARAMS[4].format(0.25), "Under 50%");
        assert_eq!(TAPE_PARAMS[6].format(0.0), "Off");
        assert_eq!(TAPE_PARAMS[6].format(4.0 / 7.0), "+0.0 dB");
        assert_eq!(TAPE_PARAMS[8].format(0.5), "+0.0 dB");
        assert_eq!(TAPE_PARAMS[11].group, "");
    }

    #[test]
    fn registered_and_reports_two_ms_latency_and_tail() {
        let mut p = crate::create("fs_tape").unwrap();
        assert_eq!(p.info().category, Category::FireStation);
        p.prepare(48_000.0, 512, 2);
        assert_eq!(p.latency(), 96, "lround(0.002 * 48000)");
        assert_eq!(p.tail_samples(), 2400);
        p.prepare(44_100.0, 512, 2);
        assert_eq!(p.latency(), 88, "lround(88.2)");
        p.prepare(96_000.0, 512, 2);
        assert_eq!(p.latency(), 192);
        // Turning Wow & Flutter or any other knob never moves it.
        for id in ["flutter", "quality", "machine", "speed"] {
            p.set_param(id, 1.0);
            assert_eq!(p.latency(), 192, "{id}");
        }
    }

    #[test]
    fn readouts_meter_drive_and_flutter() {
        let mut s = prepared();
        s.set_param("machine", 3.0);
        s.set_param("flutter", 0.5);
        let mut io = vec![sine(4800, 0.5), sine(4800, 0.5)];
        run(&mut s, &mut io, 512);
        let mut r = [f32::NAN; 8];
        s.readouts(&mut r);
        assert!((r[0] - gain_to_db(0.5)).abs() < 0.1, "{r:?}");
        assert!(r[1].is_finite() && r[1] > -20.0, "{r:?}");
        assert_eq!(r[2], 3.0);
        assert!((f64::from(r[3]) - flutter_percent(3, 0.5)).abs() < 1e-6, "{r:?}");
        assert!(r[3] > 0.0);
        assert!(r[4].is_nan());
    }

    #[test]
    fn every_factory_preset_parses() {
        let presets = factory_presets("fs_tape");
        assert_eq!(presets.len(), 46);
        for (name, json) in presets {
            let p = FsPreset::parse(json, &TAPE_INFO).unwrap();
            assert_eq!(&p.name, name);
            for key in ["machine", "formula", "speed", "drive", "bias", "flutter", "hiss", "mix"] {
                assert!(p.values.contains_key(key), "{name}: {key}");
            }
        }
        let hiss_bed = presets.iter().find(|p| p.0 == "Hiss Bed").unwrap();
        let p = FsPreset::parse(hiss_bed.1, &TAPE_INFO).unwrap();
        assert_eq!((p.values["machine"], p.values["formula"], p.values["speed"]), (0.0, 2.0, 1.0));
    }

    #[test]
    fn hostile_params_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
            for machine in 0..4 {
                let mut s = prepared();
                s.set_param("machine", machine as f32);
                // Bypass stays off: a hostile value that turned it on would hide the wet path.
                for info in TAPE_PARAMS.iter().filter(|p| p.id != "machine" && p.id != "bypass") {
                    assert!(s.set_param(info.id, bad));
                }
                assert_eq!(s.param("bypass"), Some(0.0));
                let mut io = vec![sine(2048, 0.9), sine(2048, -0.9)];
                run(&mut s, &mut io, 256);
                assert!(all_finite(&io), "machine {machine}, value {bad}");
            }
        }
    }

    #[test]
    fn hostile_choice_values_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.49, 3.6] {
            for id in ["machine", "formula", "speed", "quality", "autoGain"] {
                let mut s = prepared();
                s.set_param("flutter", 1.0);
                s.set_param("hiss", 0.8);
                let mut a = vec![sine(1024, 0.9), sine(1024, -0.9)];
                run(&mut s, &mut a, 256);
                // Mid-stream, as an automation lane would deliver it.
                assert!(s.set_param(id, bad));
                let mut b = vec![sine(2048, 0.9), sine(2048, -0.9)];
                run(&mut s, &mut b, 256);
                assert!(all_finite(&a) && all_finite(&b), "{id} = {bad}");
            }
        }
    }

    #[test]
    fn hostile_bypass_values_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
            for machine in 0..4 {
                let mut s = prepared();
                s.set_param("machine", machine as f32);
                s.set_param("drive", 1.0);
                s.set_param("flutter", 1.0);
                s.set_param("hiss", 1.0);
                let src = [sine(4096, 0.9), sine(4096, -0.9)];
                // Half the run with the wet path engaged, then the hostile bypass value mid-stream.
                let mut a = vec![src[0][..2048].to_vec(), src[1][..2048].to_vec()];
                run(&mut s, &mut a, 256);
                assert!(s.set_param("bypass", bad));
                let mut b = vec![src[0][2048..].to_vec(), src[1][2048..].to_vec()];
                run(&mut s, &mut b, 256);
                assert!(all_finite(&a) && all_finite(&b), "machine {machine}, bypass {bad}");
            }
        }
    }

    #[test]
    fn switching_quality_and_model_mid_stream_is_safe() {
        let mut s = prepared();
        s.set_param("flutter", 1.0);
        s.set_param("hiss", 0.8);
        let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
        let src = sine(128 * 300, 0.8);
        for blk in 0..300 {
            s.set_param("machine", (blk % 4) as f32);
            s.set_param("formula", (blk % 3) as f32);
            s.set_param("speed", ((blk / 2) % 3) as f32);
            s.set_param("quality", ((blk / 3) % 3) as f32);
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
        let mut s = Tape::new();
        s.prepare(48_000.0, 512, 1);
        let mut mono = vec![sine(4096, 0.5)];
        let orig = mono.clone();
        run(&mut s, &mut mono, 512);
        assert!(all_finite(&mono));
        assert_ne!(mono, orig);

        // Six channels: the first two processed, the rest untouched.
        let mut s = Tape::new();
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

        // Processing before any prepare, and with absurd sample rates: no panic, finite.
        let mut s = Tape::new();
        s.set_param("flutter", 1.0);
        let mut io = vec![sine(1024, 0.5), sine(1024, 0.5)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io));
        for sr in [f32::NAN, 0.0, -1.0, 1.0, 1e12] {
            let mut s = Tape::new();
            s.prepare(sr, 512, 2);
            s.set_param("flutter", 1.0);
            let mut io = vec![sine(1024, 0.5), sine(1024, 0.5)];
            run(&mut s, &mut io, 256);
            assert!(all_finite(&io), "sr {sr}");
        }
    }

    #[test]
    fn silence_tail_is_cheap() {
        let sr = 48_000usize;
        for machine in 0..4 {
            let time = |signal_s: usize, silence_s: usize| {
                let mut s = prepared();
                s.set_param("machine", machine as f32);
                s.set_param("drive", 0.8);
                s.set_param("flutter", 0.5);
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
            assert!(tail < 4.0 * full, "machine {machine}: silence tail {tail:.3}s vs signal {full:.3}s");
        }
    }

    #[test]
    fn process_does_not_allocate() {
        // No counting allocator in this crate. The engine's only heap memory is its delay lines,
        // sized in `prepare`; their pointers and capacities, the engine's own address and the host
        // buffers' must all be unchanged across 1000 blocks of quality and voicing switches.
        let mut s = prepared();
        s.set_param("flutter", 1.0);
        s.set_param("hiss", 0.7);
        let engine_ptr: *const TapeEngineDsp = &*s.dsp;
        let lines = s.dsp.buffer_ptrs();
        let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
        let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
        for blk in 0..1000 {
            s.set_param("quality", (blk % 3) as f32);
            s.set_param("machine", ((blk / 3) % 4) as f32);
            s.set_param("speed", ((blk / 5) % 3) as f32);
            s.process(&mut io, 256);
        }
        assert_eq!(engine_ptr, &*s.dsp as *const TapeEngineDsp);
        assert_eq!(lines, s.dsp.buffer_ptrs());
        assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
        assert!(all_finite(&io));
    }

    #[test]
    fn bypass_lands_exactly_on_the_delayed_input() {
        let mut s = prepared();
        s.set_param("drive", 1.0);
        s.set_param("flutter", 1.0);
        s.set_param("bypass", 1.0);
        let lat = s.latency();
        let src = sine(48_000, 0.6);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        // 15 ms glide, then exactly the input, `latency` samples late.
        assert_eq!(io[0][24_000..], src[24_000 - lat..48_000 - lat]);
    }

    #[test]
    fn hiss_is_seeded_so_renders_repeat() {
        let render = || {
            let mut s = prepared();
            s.set_param("hiss", 1.0);
            s.set_param("flutter", 0.7);
            let mut io = vec![vec![0.0f32; 9600], vec![0.0f32; 9600]];
            run(&mut s, &mut io, 512);
            io
        };
        let a = render();
        assert!(a[0].iter().any(|v| *v != 0.0), "hiss is audible on silence");
        assert_ne!(a[0], a[1], "each channel has its own noise");
        assert_eq!(a, render());
    }
}
