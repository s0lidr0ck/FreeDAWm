// 0.7071 is the C++ default knob value (unity on the audio taper).
#![allow(clippy::approx_constant)]

use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
}

fn prepared() -> Pitch {
    let mut s = Pitch::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut Pitch, io: &mut [Vec<f32>], block: usize) {
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

/// Every voice up, so every shifter (and the divider) runs.
fn all_voices(s: &mut Pitch) {
    for id in ["sub", "octDown", "octUp", "oct2Up", "fifth", "voice"] {
        s.set_param(id, 0.8);
    }
    s.set_param("pedal", 1.0);
}

const HOSTILE: [f32; 9] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0];
const CHOICES: [&str; 3] = ["style", "fifthMode", "interval"];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = PITCH_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            "style",
            "dry",
            "sub",
            "octDown",
            "octUp",
            "oct2Up",
            "fifth",
            "fifthMode",
            "detune",
            "attack",
            "lpf",
            "reso",
            "level",
            "bypass",
            "trim",
            "voice",
            "interval",
            "pedal",
            "glide",
            "width",
            "grain",
            "track"
        ]
    );
    let defaults: Vec<f32> = PITCH_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(defaults, [0.0, 0.7071, 0.0, 0.6, 0.6, 0.0, 0.0, 1.0, 0.25, 0.0, 1.0, 0.0, 0.7071, 0.0, 0.3333, 0.7, 1.0, 0.0, 0.2, 0.35, 0.5, 0.5]);
    let names: Vec<&str> = PITCH_PARAMS.iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        [
            "Style", "Dry", "Sub", "-1 Oct", "+1 Oct", "+2 Oct", "5th", "5th Mode", "Detune", "Attack", "Filter", "Q", "Level", "Bypass", "Input",
            "Voice", "Interval", "Pedal", "Glide", "Width", "Grain", "Track"
        ]
    );
    assert_eq!(PITCH_PARAMS[d::P_STYLE].choices, pitch_config::STYLES);
    assert_eq!(PITCH_PARAMS[d::P_FIFTH_MODE].choices, ["+5th", "+Oct 5th", "-5th"]);
    assert_eq!(PITCH_PARAMS[d::P_INTERVAL].choices.len(), 10);
    assert_eq!(PITCH_PARAMS[d::P_BYPASS].group, "", "the engine's bypass is hidden");
    // formatValue, through the engine's own maps
    assert_eq!(PITCH_PARAMS[d::P_DRY].format(0.7071), "0.0 dB");
    assert_eq!(PITCH_PARAMS[d::P_SUB].format(0.0), "Off");
    assert_eq!(PITCH_PARAMS[d::P_TRIM].format(0.3333), "+0.0 dB");
    assert_eq!(PITCH_PARAMS[d::P_LPF].format(1.0), "Open");
    assert_eq!(PITCH_PARAMS[d::P_DETUNE].format(0.25), "12.5 \u{a2}");
    assert_eq!(PITCH_PARAMS[d::P_GLIDE].format(0.2), "20 ms");
}

#[test]
fn registered_with_no_latency() {
    let mut p = crate::create("fs_pitch").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    // The dry path is untouched; the C++ reports no latency (the voice lag is a readout).
    for style in 0..5 {
        p.set_param("style", style as f32);
        p.prepare(48_000.0, 512, 2);
        let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
        p.process(&mut io, 512);
        assert_eq!(p.latency(), 0);
        let mut r = [f32::NAN; 8];
        p.readouts(&mut r);
        assert_eq!(r[3], d::latency_ms(style, 0.5));
    }
}

#[test]
fn readouts_meter_the_block_and_the_oc2_lamp() {
    let mut s = prepared();
    s.set_param("style", 2.0);
    let mut io = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut s, &mut io, 512);
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert!((r[0] - gain_to_db(0.5)).abs() < 0.1, "{r:?}");
    assert!(r[1].is_finite() && r[1] > -20.0, "{r:?}");
    assert_eq!(r[2], 1.0, "OC-2 locked on a steady note: {r:?}");
    assert_eq!(r[3], 0.0, "OC-2 does not shift");
    // Silence: the lamp goes out, the meters read silence.
    let mut quiet = vec![vec![0.0f32; 48_000], vec![0.0f32; 48_000]];
    run(&mut s, &mut quiet, 512);
    s.readouts(&mut r);
    assert_eq!(r[0], gain_to_db(0.0));
    assert_eq!(r[2], 0.0);
    // A shifting style never lights it.
    let mut s = prepared();
    run(&mut s, &mut io.clone(), 512);
    s.readouts(&mut r);
    assert_eq!(r[2], 0.0);
}

#[test]
fn whammy_slews_to_the_interval() {
    let mut s = prepared();
    s.set_param("style", 4.0);
    s.set_param("interval", 0.0);
    s.set_param("pedal", 1.0);
    s.set_param("glide", 0.0);
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    // No glide: the control-rate target reaches the voice at the first update (every 32 samples).
    assert!((r[4] - 24.0).abs() < 1e-3, "{r:?}");
}

#[test]
fn params_are_pushed_only_when_they_move() {
    // Style's setter re-seats every head: re-sending the same value must not restart the voices.
    let mut a = prepared();
    let mut b = prepared();
    all_voices(&mut a);
    all_voices(&mut b);
    let src = sine(48_000, 0.6);
    let mut ia = vec![src.clone(), src.clone()];
    let mut ib = ia.clone();
    let block = 480;
    for (ca, cb) in ia[0].chunks_mut(block).zip(ib[0].chunks_mut(block)) {
        let mut x = vec![ca.to_vec(), ca.to_vec()];
        let mut y = vec![cb.to_vec(), cb.to_vec()];
        b.set_param("style", 0.0); // unchanged: nothing to push
        a.process(&mut x, block);
        b.process(&mut y, block);
        ca.copy_from_slice(&x[0]);
        cb.copy_from_slice(&y[0]);
    }
    assert_eq!(ia[0], ib[0]);
    // A real change is pushed (at the next block).
    let mut c = prepared();
    all_voices(&mut c);
    c.set_param("style", 3.0);
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    c.process(&mut io, 512);
    let mut r = [0.0; 8];
    c.readouts(&mut r);
    assert_eq!(r[3], 20.0, "H910's 40 ms window");
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_pitch");
    assert_eq!(presets.len(), 32);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &PITCH_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert!(!p.values.contains_key("bypass"), "{name}");
        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        for key in raw["values"].as_object().unwrap().keys() {
            assert!(p.values.contains_key(key), "{name}: {key} did not resolve");
        }
        for key in ["style", "dry", "voice", "interval", "fifthMode"] {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
    }
    let whammy = presets.iter().find(|(n, _)| *n == "Whammy +2 Oct").unwrap();
    let p = FsPreset::parse(whammy.1, &PITCH_INFO).unwrap();
    assert_eq!(p.values["style"], 4.0);
    assert_eq!(p.values["interval"], 0.0);
}

#[test]
fn hostile_params_stay_finite() {
    // Bypass stays off (it would hide the wet path); the choices are set per case.
    let skip = ["style", "fifthMode", "interval", "bypass"];
    for bad in HOSTILE {
        for style in 0..5 {
            for fifth_mode in 0..3 {
                let mut s = prepared();
                s.set_param("style", style as f32);
                s.set_param("fifthMode", fifth_mode as f32);
                s.set_param("interval", ((style * 3 + fifth_mode) % 10) as f32);
                for info in PITCH_PARAMS.iter().filter(|p| !skip.contains(&p.id)) {
                    assert!(s.set_param(info.id, bad));
                }
                assert_eq!(s.param("bypass"), Some(0.0));
                let mut io = vec![sine(4096, 0.9), sine(4096, -0.9)];
                run(&mut s, &mut io, 256);
                assert!(all_finite(&io), "style {style}, fifth mode {fifth_mode}, value {bad}");
            }
        }
    }
}

#[test]
fn hostile_choice_params_stay_finite() {
    for bad in HOSTILE {
        let mut s = prepared();
        all_voices(&mut s);
        for id in CHOICES {
            assert!(s.set_param(id, bad));
        }
        assert_eq!(s.param("bypass"), Some(0.0));
        let mut io = vec![sine(4096, 0.9), sine(4096, -0.9)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "value {bad}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        for style in 0..5 {
            let mut s = prepared();
            all_voices(&mut s);
            s.set_param("style", style as f32);
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
    // No quality setting in this engine: style, 5th mode, interval and grain are what switch.
    let mut s = prepared();
    all_voices(&mut s);
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = sine(128 * 500, 0.8);
    for blk in 0..500 {
        s.set_param("style", (blk % 5) as f32);
        s.set_param("fifthMode", ((blk / 2) % 3) as f32);
        s.set_param("interval", ((blk / 3) % 10) as f32);
        s.set_param("grain", (blk % 7) as f32 / 6.0);
        s.set_param("glide", (blk % 4) as f32 / 3.0);
        for b in &mut buf {
            b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
        }
        s.process(&mut buf, 128);
        assert!(all_finite(&buf), "block {blk}");
        assert!(buf.iter().flatten().all(|v| v.abs() < 50.0), "block {blk}: runaway level");
    }
}

#[test]
fn mono_and_wide_buffers() {
    // Mono: one channel processed.
    let mut s = Pitch::new();
    s.prepare(48_000.0, 512, 1);
    s.set_param("octUp", 0.9);
    let mut mono = vec![sine(9600, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    // Six channels: the first two processed, the rest untouched.
    let mut s = Pitch::new();
    s.prepare(48_000.0, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(9600, 0.1 * (c + 1) as f32)).collect();
    let orig = wide.clone();
    s.set_param("octUp", 0.9);
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
    assert!(all_finite(&ragged));
}

#[test]
fn silence_tail_is_cheap() {
    let sr = 48_000usize;
    let mut cases = Vec::new();
    for style in 0..5 {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            all_voices(&mut s);
            s.set_param("style", style as f32);
            s.set_param("attack", 0.4);
            s.set_param("lpf", 0.6);
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
    // No counting allocator in this crate: the engine's only heap blocks are its two channel
    // buffers, made in `new`; their addresses and sizes across 1000 blocks of switching (style
    // re-seats every head) are the whole story, and the host buffers keep theirs.
    let mut s = prepared();
    all_voices(&mut s);
    let engine_ptr: *const PitchEngineDsp = s.engine();
    let before = s.engine().heap_blocks();
    let src = sine(256, 0.7);
    let mut io = vec![src.clone(), src.clone()];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        for c in &mut io {
            c.copy_from_slice(&src);
        }
        s.set_param("style", ((blk / 4) % 5) as f32);
        s.set_param("grain", (blk % 5) as f32 / 4.0);
        s.set_param("lpf", (blk % 3) as f32 / 3.0);
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, s.engine() as *const PitchEngineDsp);
    assert_eq!(before, s.engine().heap_blocks());
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is
    // re-prepared in place, its buffers re-filled, never re-made.
    let mut s = prepared();
    all_voices(&mut s);
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const PitchEngineDsp = s.engine();
    let blocks = s.engine().heap_blocks();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const PitchEngineDsp);
        assert_eq!(blocks, s.engine().heap_blocks());
        s.process(&mut io, 512);
    }
    // And a prepare at another rate re-uses them too.
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(blocks, s.engine().heap_blocks());
    assert!(all_finite(&io));
}

#[test]
fn bypass_lands_exactly_on_the_input() {
    for style in 0..5 {
        let mut s = prepared();
        all_voices(&mut s);
        s.set_param("style", style as f32);
        s.set_param("bypass", 1.0);
        let src = sine(48_000, 0.6);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        // The 0.002-per-sample glide, then the input. In `f32` the glide stalls a few hundred
        // ulps short of 1 (each step rounds to nothing), as the C++'s does: what is left of the
        // wet path is under -50 dB.
        for i in 24_000..48_000 {
            assert!((io[0][i] - src[i]).abs() < 2e-3, "style {style}, sample {i}: {} vs {}", io[0][i], src[i]);
        }
    }
}

#[test]
fn a_reset_is_independent_of_what_played_before() {
    // Every style.
    for style in 0..5 {
        // Two modules with different histories (input, knob moves, style switches, the Whammy pedal
        // mid-glide), brought to the same parameters and reset, render bit for bit the same: no
        // state that the transport reset leaves behind depends on the past. Long enough to cover
        // the slowest glide and a whole buffer of voice delay.
        let last = |s: &mut Pitch| {
            for p in PITCH_PARAMS.iter() {
                s.set_param(p.id, p.default);
            }
            for (id, v) in [
                ("style", style as f32),
                ("sub", 0.7),
                ("octDown", 0.8),
                ("octUp", 0.8),
                ("oct2Up", 0.5),
                ("fifth", 0.6),
                ("dry", 0.2),
                ("level", 0.3),
                ("width", 1.0),
                ("trim", 0.9),
                ("voice", 1.0),
                ("pedal", 0.6),
                ("glide", 0.9),
                ("attack", 0.4),
                ("lpf", 0.6),
                ("reso", 0.5),
            ] {
                s.set_param(id, v);
            }
        };
        let mut a = prepared();
        all_voices(&mut a);
        let mut warm = vec![sine(20_000, 0.9), sine(20_000, -0.3)];
        run(&mut a, &mut warm, 333);
        a.set_param("style", 2.0);
        a.set_param("track", 0.9);
        let mut warm = vec![sine(5_000, 0.4), sine(5_000, 0.4)];
        run(&mut a, &mut warm, 128);
        last(&mut a);
        a.set_param("pedal", 0.0);
        let mut warm = vec![sine(700, 0.5), sine(700, 0.5)];
        run(&mut a, &mut warm, 64);
        last(&mut a);

        let mut b = prepared();
        b.set_param("style", 1.0);
        b.set_param("grain", 0.1);
        let mut warm = vec![sine(9_000, -0.6), sine(9_000, 0.8)];
        run(&mut b, &mut warm, 512);
        last(&mut b);
        b.set_param("pedal", 1.0);
        let mut warm = vec![sine(300, 0.6), sine(300, 0.6)];
        run(&mut b, &mut warm, 100);
        last(&mut b);

        a.reset();
        b.reset();
        let src = vec![sine(60_000, 0.7), sine(60_000, -0.4)];
        let mut got_a = src.clone();
        let mut got_b = src.clone();
        run(&mut a, &mut got_a, 512);
        run(&mut b, &mut got_b, 256);
        assert!(got_a == got_b, "style {style}: two histories, one render after a reset");
        assert!(all_finite(&got_a));
    }
}

#[test]
fn a_reset_does_not_glide() {
    // Dry, Level and the voices sit at their targets from the first sample after a reset: a
    // non-default Dry with every voice down is already the right gain, and a voice that was up
    // does not fade back in (the C++'s `prepare` would bring the voices up from 0).
    let mut s = prepared();
    for id in ["sub", "octDown", "octUp", "oct2Up", "fifth"] {
        s.set_param(id, 0.0);
    }
    s.set_param("dry", 0.3);
    let mut warm = vec![sine(4_800, 0.5), sine(4_800, 0.5)];
    run(&mut s, &mut warm, 512);
    s.reset();
    let src = sine(256, 0.5);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 256);
    let g = d::knob_to_gain(0.3);
    for i in 0..256 {
        assert!((io[0][i] - src[i] * g).abs() < 1e-6, "sample {i}: {} vs {}", io[0][i], src[i] * g);
    }

    // With a voice up, its smoothed gain sits at its target right after the reset.
    let mut a = prepared();
    a.set_param("octDown", 1.0);
    let mut warm = vec![sine(9_600, 0.5), sine(9_600, 0.5)];
    run(&mut a, &mut warm, 512);
    a.reset();
    assert_eq!(a.engine().voice_gains_smoothed(), a.engine().voice_gains_target());
}

#[test]
fn a_reset_before_any_audio_is_the_fresh_path() {
    // The first `prepare` and any reset before audio flows are the freshly-made module the
    // fidelity cases measure.
    let setup = |s: &mut Pitch| {
        for (id, v) in [("style", 4.0), ("dry", 0.0), ("level", 0.3), ("voice", 1.0), ("pedal", 0.6)] {
            s.set_param(id, v);
        }
    };
    let src = vec![sine(9600, 0.7), sine(9600, -0.4)];
    let mut fresh = prepared();
    setup(&mut fresh);
    let mut want = src.clone();
    run(&mut fresh, &mut want, 512);

    let mut s = Pitch::new();
    setup(&mut s);
    s.prepare(44_100.0, 512, 2);
    s.reset();
    s.prepare(48_000.0, 512, 2);
    s.reset();
    let mut got = src.clone();
    run(&mut s, &mut got, 512);
    assert!(got == want);
}

#[test]
fn dry_and_unity_level_pass_the_input_when_no_voice_is_up() {
    // Every voice down: the wet path is silent and Dry at 0.7071 is unity (2 v^2 = 1.0000).
    let mut s = prepared();
    for id in ["sub", "octDown", "octUp", "oct2Up", "fifth"] {
        s.set_param(id, 0.0);
    }
    let src = sine(9600, 0.5);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    let g = d::knob_to_gain(0.7071);
    for i in 4800..9600 {
        assert!((io[0][i] - src[i] * g).abs() < 1e-5, "sample {i}");
    }
}
