// The C++ literals (6.2831853f, 0.7071f) are kept as written, not swapped for the constants.
#![allow(clippy::approx_constant)]
use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
}

fn prepared() -> ModEngine {
    let mut s = ModEngine::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut ModEngine, io: &mut [Vec<f32>], block: usize) {
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

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// A setting away from every default, so a reset that left anything behind would show.
const BUSY: &[(&str, f32)] = &[
    ("circuit", 7.0),
    ("variant", 0.0),
    ("depth", 0.9),
    ("rate", 0.8),
    ("level", 0.6),
    ("trim", 0.6),
    ("mix", 0.8),
    ("width", 0.2),
    ("regen", 0.9),
    ("drive", 0.5),
    ("noise", 0.9),
    ("shape", 3.0),
];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = MOD_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            "depth", "rate", "level", "circuit", "variant", "bypass", "trim", "link", "linkLo", "linkHi", "mix", "width", "regen", "drive", "noise",
            "bias", "tone", "stereo", "shape", "sync", "division", "tempo"
        ]
    );
    let defaults: Vec<f32> = MOD_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(defaults, [0.5, 0.5, 0.7071, 0.0, 1.0, 0.0, 0.3333, 0.0, 0.7071, 0.7071, 0.5, 0.5, 0.0, 0.0, 0.35, 0.5, 0.5, 1.0, 0.0, 0.0, 5.0, 0.4]);
    assert_eq!(MOD_PARAMS[d::P_TRIM].format(0.3333), "+0.0 dB");
    assert_eq!(MOD_PARAMS[d::P_LEVEL].format(0.7071), "+0.0 dB");
    assert_eq!(MOD_PARAMS[d::P_TEMPO].format(0.4), "120 bpm");
    assert_eq!(MOD_PARAMS[d::P_DEPTH].format(0.5), "5");
    assert_eq!(MOD_PARAMS[d::P_CIRCUIT].choices.len(), 14);
}

#[test]
fn registered_with_no_latency_and_a_quarter_second_tail() {
    let mut p = crate::create("fs_mod").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    p.prepare(48_000.0, 512, 2);
    assert_eq!(p.latency(), 0);
    assert_eq!(p.tail_samples(), 12_000);
}

#[test]
fn rate_maps_as_the_editor_prints_it() {
    // Rate 5 is the circuit's stock rate; the ends are the table's.
    assert!((d::lfo_rate_hz(0, 1, 0.5, 0, 5, 0.4) - 0.78).abs() < 1e-6);
    assert!((d::lfo_rate_hz(11, 0, 0.0, 0, 5, 0.4) - 0.5).abs() < 1e-6);
    assert!((d::lfo_rate_hz(11, 0, 1.0, 0, 5, 0.4) - 12.0).abs() < 1e-4);
    // Synced: 120 bpm quarter notes = 2 Hz, a bar at 40 bpm = 1/12 Hz.
    assert!((d::lfo_rate_hz(3, 1, 0.1, 1, 5, 0.4) - 2.0).abs() < 1e-6);
    assert!((d::lfo_rate_hz(3, 1, 0.1, 1, 0, 0.0) - 40.0 / 60.0 / 8.0).abs() < 1e-6);
    // The engine runs at what the map says.
    let mut s = prepared();
    s.set_param("circuit", 12.0);
    s.set_param("rate", 0.3);
    let mut io = vec![sine(512, 0.3), sine(512, 0.3)];
    // The Rate glide (0.005 a sample) settles within a few blocks.
    for _ in 0..8 {
        s.process(&mut io, 512);
    }
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    let want = d::lfo_rate_hz(12, 1, 0.3, 0, 5, 0.4);
    assert!((r[1] - want).abs() < want * 1e-3, "{} vs {want}", r[1]);
}

#[test]
fn readouts_follow_the_lfo() {
    let mut s = prepared();
    s.set_param("circuit", 11.0);
    let mut r = [f32::NAN; 8];
    let mut io = vec![sine(512, 0.3), sine(512, 0.3)];
    s.process(&mut io, 512);
    s.readouts(&mut r);
    let p0 = r[0];
    // Opto Trem's stock 4 Hz: 512 samples move the phase 512 * 4 / 48000.
    s.process(&mut io, 512);
    s.readouts(&mut r);
    let step = (r[0] - p0).rem_euclid(1.0);
    assert!((step - 512.0 * 4.0 / 48_000.0).abs() < 1e-3, "{step}");
    assert!((0.0..1.0).contains(&r[0]));
    assert_eq!(r[2], 0.0, "Opto's stock LFO is a sine");
    s.set_param("shape", 4.0);
    s.process(&mut io, 512);
    s.readouts(&mut r);
    assert_eq!(r[2], 8.0);
    assert!(r[3].is_nan(), "unused slots untouched");
}

#[test]
fn lfo_helpers_match_the_engine_table() {
    for circuit in 0..14 {
        for variant in 0..6 {
            for shape in 0..5 {
                let mut e = ModEngineDsp::new();
                e.set_parameter(d::P_CIRCUIT, circuit as f32);
                e.set_parameter(d::P_VARIANT, variant as f32);
                e.set_parameter(d::P_SHAPE, shape as f32);
                assert_eq!(e.lfo_shape(), d::lfo_shape_for(circuit, variant, shape), "{circuit} {variant} {shape}");
            }
        }
    }
    assert!((d::lfo_value(0.25, 0.0) - 1.0).abs() < 1e-6);
    assert_eq!(d::lfo_value(0.0, 1.0), 1.0);
    assert_eq!(d::lfo_value(0.5, 1.0), -1.0);
    assert_eq!(d::lfo_value(0.4, 8.0), -1.0);
}

#[test]
fn tempo_sync_follows_transport() {
    // Stopped, playing, looping back, seeking and a tempo change between blocks: Sync runs at the
    // host tempo (the Tempo knob is the fallback when the host gives none), the output stays
    // finite, and nothing is re-made.
    let mut s = prepared();
    s.set_param("circuit", 11.0);
    s.set_param("sync", 1.0);
    s.set_param("division", 5.0); // 1/4: one cycle per beat
    s.set_param("tempo", 0.0); // 40 bpm if the host tempo were ignored
    let engine_ptr: *const ModEngineDsp = s.engine();
    let mut io = vec![sine(256, 0.5), sine(256, 0.5)];
    let mut r = [f32::NAN; 8];
    let mut t = Transport { tempo: 90.0, ..Transport::default() };
    let rate = |s: &mut ModEngine, t: &Transport, io: &mut Vec<Vec<f32>>, r: &mut [f32; 8]| {
        s.set_transport(t);
        s.process(io, 256);
        s.readouts(r);
        assert!(all_finite(io));
        r[1]
    };
    // Stopped: the session tempo still sets the rate.
    assert!((rate(&mut s, &t, &mut io, &mut r) - 1.5).abs() < 1e-3);
    t.playing = true;
    for blk in 0..40 {
        t.samples = blk * 256;
        t.quarters = t.samples as f64 / 48_000.0 * 1.5;
        assert!((rate(&mut s, &t, &mut io, &mut r) - 1.5).abs() < 1e-3);
    }
    // Looping back and seeking: the position jumps, the rate does not.
    t.samples = 0;
    t.quarters = 0.0;
    assert!((rate(&mut s, &t, &mut io, &mut r) - 1.5).abs() < 1e-3);
    s.reset();
    t.samples = 480_000;
    assert!((rate(&mut s, &t, &mut io, &mut r) - 1.5).abs() < 1e-3);
    // A tempo change lands within the block it arrives in (the engine re-reads every 32 samples).
    t.tempo = 150.0;
    assert!((rate(&mut s, &t, &mut io, &mut r) - 2.5).abs() < 1e-3);
    // No usable host tempo: the Tempo knob, as the C++.
    t.tempo = f64::NAN;
    assert!((rate(&mut s, &t, &mut io, &mut r) - 40.0 / 60.0).abs() < 1e-3);
    t.tempo = 5.0;
    assert!((rate(&mut s, &t, &mut io, &mut r) - 40.0 / 60.0).abs() < 1e-3);
    // Free-running ignores the host tempo.
    t.tempo = 150.0;
    s.set_param("sync", 0.0);
    assert!((rate(&mut s, &t, &mut io, &mut r) - 4.0).abs() < 1e-3);
    assert_eq!(engine_ptr, s.engine() as *const ModEngineDsp);
}

#[test]
fn no_transport_is_the_cpp_path() {
    // The render tool gives no transport: a module that never sees one renders bit-identically to
    // one told "no tempo" (0), so the fidelity cases cover the C++ path.
    let mut a = prepared();
    let mut b = prepared();
    for m in [&mut a, &mut b] {
        m.set_param("sync", 1.0);
        m.set_param("circuit", 3.0);
    }
    b.set_transport(&Transport { tempo: 0.0, ..Transport::default() });
    let src = vec![sine(8192, 0.5), sine(8192, -0.4)];
    let (mut x, mut y) = (src.clone(), src.clone());
    run(&mut a, &mut x, 512);
    run(&mut b, &mut y, 512);
    assert_eq!(x, y);
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_mod");
    assert_eq!(presets.len(), 48);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &MOD_INFO).unwrap();
        assert_eq!(&p.name, name);
        for key in
            ["circuit", "variant", "depth", "rate", "mix", "width", "regen", "drive", "noise", "bias", "tone", "stereo", "shape", "sync", "division"]
        {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
        assert!(!p.values.contains_key("bypass"), "{name}");
    }
}

#[test]
fn link_level_is_apply_link() {
    assert_eq!(link_level(0.0, 0.5, 0.2, 0.8, 0.5), None, "Link off");
    assert_eq!(link_level(1.0, 0.5, 0.0, 0.0, 0.5), None, "no calibration");
    assert_eq!(link_level(1.0, 0.0, 0.3, 0.8, 0.5), Some(0.3));
    assert_eq!(link_level(1.0, 1.0, 0.3, 0.8, 0.5), Some(0.8));
    let mid = link_level(1.0, 0.5, 0.3, 0.8, 0.0).unwrap();
    assert!((mid - (0.3 + 0.5 * (1.0 - 0.5f32.powf(2.2)))).abs() < 1e-6);
    assert_eq!(link_level(1.0, 0.0, 0.3, 0.8, 0.3002), None, "already there");
}

#[test]
fn hostile_params_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
        for circuit in 0..14 {
            let mut s = prepared();
            s.set_param("circuit", circuit as f32);
            // Bypass stays off: a hostile value that turned it on would hide the wet path.
            for info in MOD_PARAMS.iter().filter(|p| p.id != "circuit" && p.id != "bypass") {
                assert!(s.set_param(info.id, bad));
            }
            assert_eq!(s.param("bypass"), Some(0.0));
            s.set_transport(&Transport { tempo: f64::from(bad), ..Transport::default() });
            let mut io = vec![sine(4096, 0.9), sine(4096, -0.9)];
            run(&mut s, &mut io, 256);
            assert!(all_finite(&io), "circuit {circuit}, value {bad}");
        }
    }
}

#[test]
fn hostile_choice_values_stay_finite() {
    // Every choice (circuit, variant, input mode, shape, sync, division) at and past its ends.
    let choices: Vec<&ParamInfo> = MOD_PARAMS.iter().filter(|p| p.unit == crate::Unit::Choice).collect();
    assert_eq!(choices.len(), 6);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.49, 0.5, 5.5, 13.6, 99.0, 1e9] {
        let mut s = prepared();
        s.set_param("depth", 1.0);
        s.set_param("regen", 1.0);
        for c in &choices {
            assert!(s.set_param(c.id, bad));
        }
        let mut io = vec![sine(4096, 0.9), sine(4096, -0.9)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "choices at {bad}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        for circuit in 0..14 {
            let mut s = prepared();
            s.set_param("circuit", circuit as f32);
            s.set_param("depth", 1.0);
            let src = [sine(4096, 0.9), sine(4096, -0.9)];
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
fn loud_input_with_every_feedback_path_open_stays_finite() {
    // The worst case the knobs allow: +24 dB in, full drive, regen and depth, on every circuit and
    // variant, fed a full-scale square.
    for circuit in 0..14 {
        for variant in 0..2 {
            let mut s = prepared();
            for (k, v) in [
                ("circuit", circuit as f32),
                ("variant", variant as f32),
                ("trim", 1.0),
                ("drive", 1.0),
                ("regen", 1.0),
                ("depth", 1.0),
                ("bias", 1.0),
            ] {
                s.set_param(k, v);
            }
            let sq: Vec<f32> = (0..24_000).map(|i| if (i / 50) % 2 == 0 { 1.0 } else { -1.0 }).collect();
            let mut io = vec![sq.clone(), sq];
            run(&mut s, &mut io, 512);
            assert!(all_finite(&io), "circuit {circuit} variant {variant}");
            assert!(io[0][12_000..].iter().any(|v| *v != 0.0), "circuit {circuit} variant {variant}: still sounding");
        }
    }
}

#[test]
fn switching_circuits_and_variants_mid_stream_is_safe() {
    // This engine has no quality setting; its model switches are the circuit, variant and shape.
    let mut s = prepared();
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = sine(128 * 400, 0.8);
    let engine_ptr: *const ModEngineDsp = s.engine();
    for blk in 0..400 {
        s.set_param("circuit", (blk % 14) as f32);
        s.set_param("variant", (blk % 6) as f32);
        s.set_param("shape", (blk % 5) as f32);
        s.set_param("sync", (blk % 2) as f32);
        s.set_param("stereo", ((blk / 3) % 2) as f32);
        for b in &mut buf {
            b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
        }
        s.process(&mut buf, 128);
        assert!(all_finite(&buf), "block {blk}");
    }
    assert_eq!(engine_ptr, s.engine() as *const ModEngineDsp);
}

#[test]
fn mono_and_wide_buffers() {
    // Mono: one channel processed (the left path, as the C++ with one channel).
    let mut s = ModEngine::new();
    s.prepare(48_000.0, 512, 1);
    let mut mono = vec![sine(4096, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    // Six channels: the first two processed, the rest untouched.
    let mut s = ModEngine::new();
    s.prepare(48_000.0, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(4096, 0.1 * (c + 1) as f32)).collect();
    let orig = wide.clone();
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
    let mut ragged = vec![vec![0.2f32; 64], vec![0.2f32; 10]];
    s.process(&mut ragged, 64);
    let mut empty = vec![Vec::new(), Vec::new()];
    s.process(&mut empty, 64);
}

#[test]
fn silence_tail_is_cheap() {
    let sr = 48_000usize;
    let mut cases = Vec::new();
    for circuit in 0..14 {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            s.set_param("circuit", circuit as f32);
            s.set_param("regen", 0.7);
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
    // No counting allocator in this crate: the engine holds no heap memory at all (both BBD lines
    // are fixed arrays inside one boxed struct), so its address is the whole story; the host
    // buffers keep their pointers and capacities across 1000 blocks of switching.
    let mut s = prepared();
    let engine_ptr: *const ModEngineDsp = s.engine();
    let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        s.set_param("circuit", ((blk / 3) % 14) as f32);
        s.set_param("variant", (blk % 6) as f32);
        s.set_transport(&Transport { tempo: 60.0 + blk as f64, playing: true, ..Transport::default() });
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, s.engine() as *const ModEngineDsp);
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is put back
    // in place, never re-made (its address is unchanged and it holds no other heap memory).
    let mut s = prepared();
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const ModEngineDsp = s.engine();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const ModEngineDsp);
        s.process(&mut io, 512);
    }
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(engine_ptr, s.engine() as *const ModEngineDsp);
    assert!(all_finite(&io));
}

/// Every parameter to its default, then `BUSY`, then `circuit`.
fn set_all(m: &mut ModEngine, circuit: usize) {
    set_all_v(m, circuit, 1);
}

/// `set_all` with the circuit's `variant`.
fn set_all_v(m: &mut ModEngine, circuit: usize, variant: usize) {
    for p in MOD_PARAMS.iter() {
        m.set_param(p.id, p.default);
    }
    for (k, v) in BUSY {
        m.set_param(k, *v);
    }
    m.set_param("circuit", circuit as f32);
    m.set_param("variant", variant as f32);
}

#[test]
fn a_reset_is_independent_of_what_played_before() {
    // Two modules with different histories (input, knob moves, circuit and sync switches, a host
    // tempo), brought to the same values and reset, render bit for bit the same over 60000
    // samples (several LFO cycles at Rate 8), every circuit.
    let src = vec![sine(60_000, 0.7), sine(60_000, -0.5)];
    for circuit in 0..14 {
        for variant in 0..mod_config::circuit(circuit as i32).variants.len().max(1) {
            let mut a = ModEngine::new();
            a.prepare(44_100.0, 512, 2);
            a.set_param("circuit", ((circuit + 5) % 14) as f32);
            a.set_param("sync", 1.0);
            a.set_param("depth", 0.1);
            a.set_transport(&Transport { tempo: 133.0, ..Transport::default() });
            let mut warm = vec![sine(7000, 0.9), sine(7000, -0.3)];
            run(&mut a, &mut warm, 333);
            set_all_v(&mut a, circuit, variant);
            let mut warm = vec![sine(301, 0.2), sine(301, 0.2)];
            run(&mut a, &mut warm, 64);

            let mut b = ModEngine::new();
            b.prepare(44_100.0, 512, 2);
            b.set_param("level", 0.1);
            b.set_param("mix", 1.0);
            b.set_param("regen", 1.0);
            b.set_transport(&Transport { tempo: 133.0, ..Transport::default() });
            let mut warm = vec![sine(3000, -0.6), sine(3000, 0.8)];
            run(&mut b, &mut warm, 128);
            set_all_v(&mut b, circuit, variant);

            a.reset();
            b.reset();
            let (mut x, mut y) = (src.clone(), src.clone());
            run(&mut a, &mut x, 512);
            run(&mut b, &mut y, 300);
            assert!(x == y, "circuit {circuit} variant {variant}");
            assert!(all_finite(&x));
        }
    }
}

#[test]
fn a_reset_does_not_glide() {
    // Every smoother sits at its knob right after a reset (the C++ `prepare` seeds them from the
    // engine's own knobs; the wrapper pushes what moved first): a non-default Level or Mix is
    // right from the first sample, and nothing glides from a default.
    let mut s = prepared();
    let mut warm = vec![sine(4800, 0.5), sine(4800, 0.5)];
    run(&mut s, &mut warm, 512);
    for (k, v) in BUSY {
        s.set_param(k, *v);
    }
    s.reset();
    let (got, want) = s.engine().smoothers();
    assert_eq!(got, want);
    // And Level is the only gain on a fully dry Mix: the output is the input times Level x trim
    // compensation from sample 0, as after a long settle.
    let mut a = prepared();
    a.set_param("mix", 0.0);
    a.set_param("level", 0.3);
    let mut warm = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut a, &mut warm, 512);
    let settled = warm[0][47_000..48_000].to_vec();
    a.reset();
    let mut io = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut a, &mut io, 512);
    let first = rms(&io[0][0..1000]);
    assert!((first / rms(&settled) - 1.0).abs() < 0.02, "{first} vs {}", rms(&settled));
}

#[test]
fn a_reset_before_any_audio_is_the_fresh_path() {
    // The first prepare and any reset before audio flows are a freshly made module, prepared,
    // with every value pushed at the first block: what the fidelity cases measure.
    let src = vec![sine(8192, 0.7), sine(8192, -0.5)];
    for circuit in [0, 7, 9, 12] {
        let mut fresh = ModEngine::new();
        set_all(&mut fresh, circuit);
        fresh.prepare(44_100.0, 512, 2);
        let mut want = src.clone();
        run(&mut fresh, &mut want, 512);

        let mut m = ModEngine::new();
        m.prepare(96_000.0, 512, 2);
        set_all(&mut m, circuit);
        m.reset();
        m.prepare(44_100.0, 512, 2);
        m.reset();
        let mut got = src.clone();
        run(&mut m, &mut got, 512);
        assert!(got == want, "circuit {circuit}");
    }
}

#[test]
fn restore_initial_is_a_new_engine() {
    let mut e = ModEngineDsp::new();
    for (i, v) in [(d::P_CIRCUIT, 9.0), (d::P_DEPTH, 0.1), (d::P_TRIM, 0.9), (d::P_SYNC, 1.0), (d::P_STEREO, 0.0)] {
        e.set_parameter(i, v);
    }
    e.prepare(96_000.0);
    let mut io = vec![sine(1000, 0.5), sine(1000, 0.5)];
    e.process(&mut io, 2, 1000);
    e.restore_initial();
    assert_eq!(e, ModEngineDsp::new());
}

#[test]
fn bypass_passes_the_input() {
    let mut s = prepared();
    s.set_param("depth", 1.0);
    s.set_param("bypass", 1.0);
    let src = sine(48_000, 0.6);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    assert_eq!(io[0], src, "the C++ bypass is a hard pass-through");
}

#[test]
fn every_circuit_modulates() {
    // Each circuit moves the sound away from the dry path and keeps the level near unity.
    for circuit in 0..14 {
        let mut s = prepared();
        s.set_param("circuit", circuit as f32);
        s.set_param("depth", 0.8);
        let src = sine(48_000, 0.5);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        let out = rms(&io[0][24_000..]);
        assert!(out > 0.05 && out < 1.5, "circuit {circuit}: rms {out}");
        assert_ne!(io[0], io[1], "circuit {circuit}: Width puts the sides apart");
    }
}

#[test]
fn the_engine_itself_stays_finite_when_driven_hard() {
    // The output guard in `process` is belt and braces: the translated engine on its own (no
    // guard) stays finite with every feedback path open and a hot square in.
    let sq: Vec<f32> = (0..48_000).map(|i| if (i / 37) % 2 == 0 { 4.0 } else { -4.0 }).collect();
    for circuit in 0..14 {
        for variant in 0..6 {
            let mut e = ModEngineDsp::new();
            for (i, v) in [
                (d::P_CIRCUIT, circuit as f32),
                (d::P_VARIANT, variant as f32),
                (d::P_TRIM, 1.0),
                (d::P_DRIVE, 1.0),
                (d::P_REGEN, 1.0),
                (d::P_DEPTH, 1.0),
            ] {
                e.set_parameter(i, v);
            }
            for bias in [0.0, 1.0] {
                e.set_parameter(d::P_BIAS, bias);
                let mut io = vec![sq.clone(), sq.clone()];
                for k in 0..(48_000 / 512) {
                    let mut blk: Vec<Vec<f32>> = io.iter().map(|c| c[k * 512..(k + 1) * 512].to_vec()).collect();
                    e.process(&mut blk, 2, 512);
                    for (c, b) in io.iter_mut().zip(&blk) {
                        c[k * 512..(k + 1) * 512].copy_from_slice(b);
                    }
                }
                assert!(all_finite(&io), "circuit {circuit} variant {variant} bias {bias}");
            }
        }
    }
}

// ---------------------------------------------------------------- Link (derived Level)

/// One run of the derived parameters over `values` (as the command layer does).
fn derive_run(
    values: &mut std::collections::BTreeMap<String, f32>,
    set: &[(&str, f32)],
    kind: crate::derive::ParamEdit,
    locked: &dyn Fn(&str) -> bool,
) {
    let info = crate::plugin_info("fs_mod").unwrap();
    let before = values.clone();
    for (k, v) in set {
        values.insert((*k).to_string(), info.param(k).unwrap().clamp(*v));
    }
    let changed: Vec<String> = set.iter().map(|(k, _)| (*k).to_string()).collect();
    let mut memory = std::collections::BTreeMap::new();
    let mut d = crate::derive::Derive::new(info, &before, values, &mut memory, &changed, kind, locked);
    derive(&mut d);
}

fn level_of(values: &std::collections::BTreeMap<String, f32>) -> f32 {
    values.get("level").copied().unwrap_or(0.7071)
}

#[test]
fn link_drives_level_from_depth_and_is_a_no_op_when_off() {
    use crate::derive::ParamEdit::{Edit, Preset};
    let mut v = std::collections::BTreeMap::new();
    let free = |_: &str| false;
    derive_run(&mut v, &[("linkLo", 0.9), ("linkHi", 0.4)], Edit, &free);
    derive_run(&mut v, &[("depth", 0.8)], Edit, &free);
    assert_eq!(level_of(&v), 0.7071, "Link is off by default");
    derive_run(&mut v, &[("link", 1.0)], Edit, &free);
    assert_eq!(level_of(&v), 0.9 + (0.4 - 0.9) * link_weight(0.8), "turning Link on applies it");
    derive_run(&mut v, &[("depth", 0.2)], Edit, &free);
    assert_eq!(level_of(&v), 0.9 + (0.4 - 0.9) * link_weight(0.2));
    // Level itself is not listened to: a hand-set Level stays until the next listened change.
    derive_run(&mut v, &[("level", 0.33)], Edit, &free);
    assert_eq!(level_of(&v), 0.33);
    derive_run(&mut v, &[("trim", 0.9), ("tempo", 0.1)], Edit, &free);
    assert_eq!(level_of(&v), 0.33, "Input trim and Tempo are not listened to");
    derive_run(&mut v, &[("circuit", 4.0)], Edit, &free);
    assert_eq!(level_of(&v), 0.9 + (0.4 - 0.9) * link_weight(0.2), "a preset parameter is, so Link re-applies");
    // Moving a set point re-applies too.
    derive_run(&mut v, &[("linkHi", 0.6)], Edit, &free);
    assert_eq!(level_of(&v), 0.9 + (0.6 - 0.9) * link_weight(0.2));
    // A preset recall runs it (its parameters are listened to; presets carry no Level).
    derive_run(&mut v, &[("circuit", 1.0), ("depth", 1.0)], Preset, &free);
    assert_eq!(level_of(&v), 0.6);
    // Uncalibrated (both set points ~0): Level left alone.
    derive_run(&mut v, &[("linkLo", 0.0), ("linkHi", 0.0), ("level", 0.42)], Edit, &free);
    assert_eq!(level_of(&v), 0.42);
    // An empty edit (a preset rename) does nothing.
    let before = v.clone();
    derive_run(&mut v, &[], Preset, &free);
    assert_eq!(v, before);
}

#[test]
fn a_level_lane_wins_over_link_in_the_command_layer() {
    let mut v = std::collections::BTreeMap::new();
    derive_run(&mut v, &[("link", 1.0), ("depth", 0.9), ("linkLo", 0.2)], crate::derive::ParamEdit::Edit, &|id| id == "level");
    assert_eq!(level_of(&v), 0.7071, "locked by its lane");
}

fn link_block(s: &mut ModEngine) {
    let mut io = vec![sine(64, 0.3), sine(64, 0.3)];
    s.process(&mut io, 64);
}

/// The Level the command layer leaves for these values (Link on, set points 0.9 / 0.4).
fn link_want(depth: f32) -> f32 {
    0.9 + (0.4 - 0.9) * link_weight(depth)
}

/// Level is `link_want(depth)` to within a few ULPs: the engine's `powf` runs at run time, the
/// one in these tests may be constant-folded, and the two differ in the last bit on some
/// platforms (macOS).
fn assert_level(s: &ModEngine, depth: f32, what: &str) {
    let (got, want) = (s.param("level"), link_want(depth));
    assert!(got.is_some_and(|l| (l - want).abs() <= 1e-6), "{what}: level {got:?}, want {want}");
}

fn linked() -> ModEngine {
    let mut s = prepared();
    s.set_param("link", 1.0);
    s.set_param("linkLo", 0.9);
    s.set_param("linkHi", 0.4);
    s.set_param("level", link_want(0.5));
    link_block(&mut s);
    s
}

#[test]
fn automated_depth_moves_level_as_the_command_path_does() {
    let mut s = linked();
    // The host now sends only Depth (an automation lane), block after block.
    for depth in [0.9f32, 0.2, 0.65] {
        s.set_param("depth", depth);
        link_block(&mut s);
        assert_level(&s, depth, &format!("depth {depth}"));
        assert!(s.engine().smoothers().0.iter().all(|v| v.is_finite()));
    }
    // Link off: Level stays.
    s.set_param("link", 0.0);
    s.set_param("depth", 0.1);
    link_block(&mut s);
    assert_level(&s, 0.65, "Link off");
}

#[test]
fn a_level_lane_or_a_host_level_wins_over_link() {
    let mut s = linked();
    s.set_param_automated("level", true);
    s.set_param("level", 0.2);
    link_block(&mut s);
    s.set_param("depth", 0.95);
    link_block(&mut s);
    assert_eq!(s.param("level"), Some(0.2), "the Level lane wins");
    s.set_param_automated("level", false);
    s.set_param("depth", 0.9);
    s.set_param("level", 0.33);
    link_block(&mut s);
    assert_eq!(s.param("level"), Some(0.33), "a Level sent with the Depth wins that block");
    s.set_param("depth", 0.8);
    link_block(&mut s);
    assert_level(&s, 0.8, "then Link follows Depth again");
}

#[test]
fn a_reset_keeps_the_stored_level_unless_a_listened_lane_drives_link() {
    let mut s = linked();
    s.set_param("level", 0.42);
    link_block(&mut s);
    for _ in 0..2 {
        s.reset();
        link_block(&mut s);
        assert_eq!(s.param("level"), Some(0.42), "a seek keeps the stored Level");
    }
    s.prepare(48_000.0, 512, 2);
    link_block(&mut s);
    assert_eq!(s.param("level"), Some(0.42), "so does a re-prepare");
    // With a lane on Depth, Level follows the lane's value at the new position.
    s.set_param_automated("depth", true);
    s.set_param("depth", 0.85);
    s.reset();
    link_block(&mut s);
    assert_level(&s, 0.85, "level");
    // And the Level smoother starts at that Level: no glide after the reset.
    let (got, want) = s.engine().smoothers();
    assert_eq!(got[6], want[6]);
}

#[test]
fn link_following_does_not_allocate() {
    let mut s = linked();
    let engine_ptr: *const ModEngineDsp = s.engine();
    let mut io = vec![sine(128, 0.5), sine(128, 0.5)];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..500 {
        s.set_param("depth", (blk % 11) as f32 / 10.0);
        s.set_param_automated("level", blk % 7 == 0);
        s.set_param_automated("depth", blk % 5 == 0);
        if blk % 50 == 0 {
            s.reset();
        }
        s.process(&mut io, 128);
    }
    assert_eq!(engine_ptr, s.engine() as *const ModEngineDsp);
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn listened_is_the_cpp_listener_set() {
    let quiet: Vec<&str> = MOD_PARAMS.iter().map(|p| p.id).filter(|id| !listened(id)).collect();
    assert_eq!(quiet, ["level", "bypass", "trim", "tempo"]);
}
