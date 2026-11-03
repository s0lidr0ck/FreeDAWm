#![allow(clippy::approx_constant)]

use super::chamber_geometry as geo;
use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

const SR: usize = 48_000;

/// A pluck-ish test signal: a decaying 220 Hz tone with a click on top, every 300 ms.
fn hits(len: usize, amp: f32) -> Vec<f32> {
    let mut seed = 0x2468_ace1u32;
    (0..len)
        .map(|i| {
            let t = (i % (SR * 3 / 10)) as f32 / SR as f32;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            let body = (std::f32::consts::TAU * 220.0 * t).sin() * (-t * 9.0).exp();
            let click = noise * (-t * 500.0).exp();
            amp * (0.7 * body + 0.5 * click)
        })
        .collect()
}

fn prepared() -> ChamberMade {
    let mut s = ChamberMade::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut ChamberMade, io: &mut [Vec<f32>], block: usize) {
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

/// A busy room: a rectangle (flutter), barrels and pots (rings and notches), a panel, the tape
/// machine and the amp drive all on.
fn busy(s: &mut ChamberMade) {
    for (id, v) in [
        ("mix", 0.7),
        ("shape", 0.0),
        ("drive", 0.6),
        ("tapeAmt", 0.5),
        ("tapeFb", 0.8),
        ("o1t", 6.0),
        ("o1x", 0.2),
        ("o2t", 2.0),
        ("o2y", 0.3),
        ("o3t", 3.0),
        ("o3r", 0.3),
        ("micPat", 1.0),
        ("micType", 2.0),
        ("predelay", 0.3),
    ] {
        assert!(s.set_param(id, v), "{id}");
    }
}

const HOSTILE: [f32; 9] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0];

fn choice_ids() -> Vec<&'static str> {
    CHAMBER_PARAMS.iter().filter(|p| p.unit == crate::Unit::Choice).map(|p| p.id).collect()
}

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    assert_eq!(CHAMBER_PARAMS.len(), 79);
    let ids: Vec<&str> = CHAMBER_PARAMS.iter().map(|p| p.id).collect();
    let mut want = vec![
        "mix", "decay", "predelay", "early", "tail", "lowCut", "highCut", "width", "level", "bypass", "drive", "drift", "shape", "roomW", "roomL",
        "roomH", "taper", "wallMat", "floorMat", "ceilMat", "spkX", "spkY", "spkZ", "spkAim", "spkType", "micAX", "micAY", "micBX", "micBY", "micZ",
        "micAAim", "micBAim", "micPat", "micType",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    for o in 1..=8 {
        for k in ["t", "x", "y", "s"] {
            want.push(format!("o{o}{k}"));
        }
    }
    for id in ["ceilSlope", "tapeAmt", "tapeTime", "tapeFb", "tapeTone"] {
        want.push(id.to_string());
    }
    for o in 1..=8 {
        want.push(format!("o{o}r"));
    }
    assert_eq!(ids, want);
    // The parameter defaults are the DSP's own (`defaultKnobs`), as the C++ requires.
    let defaults: Vec<f32> = CHAMBER_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(defaults, d::default_knobs().to_vec());
    assert_eq!(CHAMBER_PARAMS[d::P_BYPASS].group, "", "the engine's bypass is hidden");
    assert_eq!(CHAMBER_PARAMS[d::P_SHAPE].choices, SHAPE_NAMES);
    assert_eq!(CHAMBER_PARAMS[d::P_OBJ1_TYPE + 4 * 7].choices, OBJECT_NAMES);
    assert_eq!(CHAMBER_PARAMS[d::P_OBJ1_ROT + 7].id, "o8r");
    for (i, m) in geo::MATERIALS.iter().enumerate() {
        assert_eq!(m.name, MATERIAL_NAMES[i], "Parameters.h names the materials() table");
    }
    for (i, o) in geo::OBJECT_SPECS.iter().enumerate() {
        assert_eq!(o.name, OBJECT_NAMES[i]);
    }
    // The editor's formatters, through the engine's own maps.
    let f = |i: usize, v: f32| CHAMBER_PARAMS[i].format(v);
    assert_eq!(f(d::P_MIX, 0.35), "35 %");
    assert_eq!(f(d::P_EARLY, 0.7071), "0.0 dB");
    assert_eq!(f(d::P_EARLY, 1.0), "+12.0 dB");
    assert_eq!(f(d::P_LEVEL, 0.0), "off");
    assert_eq!(f(d::P_DECAY, 0.5), "\u{d7}1.00");
    assert_eq!(f(d::P_PRE_DELAY, 1.0), "200 ms");
    assert_eq!(f(d::P_LOW_CUT, 0.0), "20 Hz");
    assert_eq!(f(d::P_HIGH_CUT, 1.0), "20.0 k");
    assert_eq!(f(d::P_HIGH_CUT, 0.0), "1.00 k");
    assert_eq!(f(d::P_WIDTH, 0.5), "100 %");
    assert_eq!(f(d::P_ROOM_W, 1.0), "98.4 ft");
    assert_eq!(f(d::P_ROOM_H, 0.0), "4.9 ft");
    assert_eq!(f(d::P_SPK_AIM, 0.625), "225\u{b0}");
    assert_eq!(f(d::P_CEIL_SLOPE, 0.0), "flat");
    assert_eq!(f(d::P_CEIL_SLOPE, 0.4), "\u{b1}10 %");
    assert_eq!(f(d::P_TAPE_REPEATS, 1.0), "90 %");
    assert_eq!(f(d::P_OBJ1_ROT, 0.5), "90\u{b0}");
}

#[test]
fn the_room_is_arithmetic() {
    // A plain box: 1.5 * 20^v by 1.5 * 20^v by 1.5 * 10^v, flat ceiling, nothing in it.
    let mut k = d::default_knobs();
    k[d::P_SHAPE] = 0.0;
    k[d::P_ROOM_W] = 0.5;
    k[d::P_ROOM_L] = 0.5;
    k[d::P_ROOM_H] = 0.0;
    k[d::P_CEIL_SLOPE] = 0.0;
    let c = build_chamber_for(&k);
    let w = 1.5 * 20f32.powf(0.5);
    assert!((c.room.width_m - w).abs() < 1e-5);
    assert!((c.room.floor_area - w * w).abs() < 1e-3);
    assert!((c.room.height_m - 1.5).abs() < 1e-6);
    assert_eq!(c.room.num_walls, 4);
    // Inward normals.
    assert_eq!((c.room.nx[0], c.room.ny[0]), (0.0, 1.0));
    // A rectangle keeps every first-order reflection (all six surfaces) for both mics, and
    // the direct sound's distance is the straight line between speaker and mic.
    assert!(c.taps[geo::TAP_DIRECT].live);
    let dist = geo::len3(c.placement.mic_a - c.placement.speaker);
    assert_eq!(c.taps[geo::TAP_DIRECT].distance, dist);
    let first = (geo::TAP_FIRST..geo::TAP_SECOND).filter(|&i| c.taps[i].live).count();
    assert_eq!(first, 6, "four walls, floor and ceiling");
    assert!(c.live_taps(0) > 6 && c.live_taps(1) > 6);
    // Eyring: a concrete box rings for seconds; carpet and drapes kill it.
    assert!(c.acoustics.rt60.mid > 1.0, "{:?}", c.acoustics.rt60);
    k[d::P_WALL_MAT] = 12.0;
    k[d::P_FLOOR_MAT] = 11.0;
    let dead = build_chamber_for(&k);
    assert!(dead.acoustics.rt60.hi < c.acoustics.rt60.hi * 0.5);
    // `confine` keeps everything a wall margin inside: a corner position lands inside.
    let (x, y) = c.room.confine(0.0, 0.0);
    assert!(c.room.min_wall_distance(x, y) >= geo::WALL_MARGIN - 1e-6);
    // Irregular rooms drop most second-order paths that a box keeps.
    let irregular = build_chamber_for(&d::default_knobs());
    assert!(irregular.live_taps(0) < c.live_taps(0));
    // A pot is a pot: placed, and its size sets its pitch.
    k[d::P_OBJ1_TYPE] = 2.0;
    let pots = build_chamber_for(&k);
    assert!(pots.objects[0].on && pots.taps[geo::TAP_OBJECT].live);
}

#[test]
fn registered_with_no_latency_and_an_honest_tail() {
    let mut p = crate::create("fs_chamber").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    p.prepare(48_000.0, 512, 2);
    let mut io = vec![hits(512, 0.5), hits(512, 0.5)];
    p.process(&mut io, 512);
    assert_eq!(p.latency(), 0);
    let mut r = [f32::NAN; 8];
    p.readouts(&mut r);
    // The default room: Hollywood Bunker, a few seconds of concrete.
    assert!(r[1] > 1.0 && r[1] < 25.0, "{r:?}");
    let tail = p.tail_samples() as f32 / 48_000.0;
    let rt = r[0].max(r[1]).max(r[2]);
    // getTailLengthSeconds: longest band + pre-delay (0) + four tape times.
    assert!((tail - (rt + 4.0 * 0.001 * d::knob_to_tape_ms(0.4927))).abs() < 1e-3, "{tail} vs {rt}");
    assert!(r[3] > 10.0 && r[6] > 0.0 && r[7] > 0.0, "{r:?}");
    assert!(r[5].is_finite());
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_chamber");
    assert_eq!(presets.len(), 36);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &CHAMBER_INFO).unwrap();
        assert_eq!(&p.name, name);
        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        for key in raw["values"].as_object().unwrap().keys() {
            assert!(p.values.contains_key(key), "{name}: {key} did not resolve");
        }
        assert_eq!(p.values.len(), 76, "{name}: the room and its voicing");
        for skipped in ["mix", "level", "bypass"] {
            assert!(!p.values.contains_key(skipped), "{name}: {skipped}");
        }
    }
    // The plugin's defaults equal Hollywood Bunker.
    let bunker = presets.iter().find(|(n, _)| *n == "Hollywood Bunker").unwrap();
    let p = FsPreset::parse(bunker.1, &CHAMBER_INFO).unwrap();
    for (k, v) in &p.values {
        let def = CHAMBER_INFO.param(k).unwrap().default;
        assert!((def - v).abs() < 2e-4, "{k}: {def} vs {v}");
    }
}

#[test]
fn it_reverberates() {
    let mut s = prepared();
    s.set_param("mix", 1.0);
    let mut io = vec![hits(SR / 4, 0.5), hits(SR / 4, 0.5)];
    for c in &mut io {
        c.resize(SR * 2, 0.0);
    }
    run(&mut s, &mut io, 512);
    assert!(all_finite(&io));
    // A second after the input stops, the room is still ringing.
    let late = rms(&io[0][SR + SR / 2..]);
    assert!(late > 1e-4, "{late}");
    // Stereo: the two microphones hear different things.
    assert!(io[0][SR..] != io[1][SR..]);
}

#[test]
fn hostile_params_stay_finite() {
    let choices = choice_ids();
    for bad in HOSTILE {
        for shape in 0..6 {
            let mut s = prepared();
            busy(&mut s);
            s.set_param("shape", shape as f32);
            for info in CHAMBER_PARAMS.iter().filter(|p| p.id != "bypass" && !choices.contains(&p.id)) {
                assert!(s.set_param(info.id, bad));
            }
            assert_eq!(s.param("bypass"), Some(0.0));
            let mut io = vec![hits(8192, 0.9), hits(8192, -0.9)];
            run(&mut s, &mut io, 256);
            assert!(all_finite(&io), "shape {shape}, value {bad}");
        }
    }
}

#[test]
fn hostile_choice_params_stay_finite() {
    for bad in HOSTILE {
        let mut s = prepared();
        busy(&mut s);
        for id in choice_ids() {
            assert!(s.set_param(id, bad));
        }
        assert_eq!(s.param("bypass"), Some(0.0));
        let mut io = vec![hits(8192, 0.9), hits(8192, -0.9)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "value {bad}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        let mut s = prepared();
        busy(&mut s);
        let src = [hits(8192, 0.9), hits(8192, -0.9)];
        let mut a = vec![src[0][..4096].to_vec(), src[1][..4096].to_vec()];
        run(&mut s, &mut a, 256);
        assert!(s.set_param("bypass", bad));
        let mut b = vec![src[0][4096..].to_vec(), src[1][4096..].to_vec()];
        run(&mut s, &mut b, 256);
        assert!(all_finite(&a) && all_finite(&b), "bypass {bad}");
    }
}

#[test]
fn hostile_input_stays_finite() {
    let mut s = prepared();
    busy(&mut s);
    let mut io = vec![vec![1e30f32; 8192], vec![1e-40f32; 8192]];
    run(&mut s, &mut io, 256);
    assert!(io.iter().flatten().all(|v| !v.is_nan()), "the C++ tanh and filters saturate, never NaN");
}

#[test]
fn switching_quality_and_model_mid_stream_is_safe() {
    // No quality setting: the room is what switches (shape, materials, objects, speaker, mics).
    let mut s = prepared();
    busy(&mut s);
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = hits(128 * 600, 0.8);
    for blk in 0..600 {
        s.set_param("shape", (blk % 6) as f32);
        s.set_param("wallMat", ((blk / 2) % 15) as f32);
        s.set_param("spkType", ((blk / 3) % 5) as f32);
        s.set_param("micPat", ((blk / 5) % 4) as f32);
        s.set_param("micType", ((blk / 7) % 4) as f32);
        s.set_param(&format!("o{}t", blk % 8 + 1), ((blk / 4) % 9) as f32);
        s.set_param("roomW", (blk % 11) as f32 / 10.0);
        s.set_param("decay", (blk % 13) as f32 / 12.0);
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
    let mut s = ChamberMade::new();
    s.prepare(48_000.0, 512, 1);
    busy(&mut s);
    let mut mono = vec![hits(9600, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    let mut s = ChamberMade::new();
    s.prepare(48_000.0, 512, 6);
    busy(&mut s);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| hits(9600, 0.1 * (c + 1) as f32)).collect();
    let orig = wide.clone();
    run(&mut s, &mut wide, 512);
    assert!(all_finite(&wide));
    assert_ne!(wide[0], orig[0]);
    assert_ne!(wide[1], orig[1]);
    assert_eq!(wide[2..], orig[2..]);

    let mut s = prepared();
    let mut one = vec![vec![0.3f32; 10]];
    s.process(&mut one, 512);
    s.process(&mut [], 64);
    let mut empty = vec![Vec::new(), Vec::new()];
    s.process(&mut empty, 64);
    let mut ragged = vec![vec![0.3f32; 64], vec![0.3f32; 10]];
    s.process(&mut ragged, 64);
    assert!(all_finite(&ragged) && all_finite(&one));

    // Unprepared: no buffers yet, the input passes through untouched (and reset is a no-op).
    let mut s = ChamberMade::new();
    s.reset();
    let mut io = vec![hits(256, 0.5), hits(256, 0.5)];
    let orig = io.clone();
    s.process(&mut io, 256);
    assert_eq!(io, orig);
}

#[test]
fn silence_tail_is_cheap() {
    let time = |signal_s: usize, silence_s: usize| {
        let mut s = prepared();
        busy(&mut s);
        let mut io = vec![hits(signal_s * SR, 0.9), hits(signal_s * SR, 0.9)];
        for c in &mut io {
            c.resize((signal_s + silence_s) * SR, 0.0);
        }
        let t = Instant::now();
        run(&mut s, &mut io, 512);
        assert!(all_finite(&io));
        t.elapsed().as_secs_f64()
    };
    let tail = time(2, 10);
    let full = time(12, 0);
    println!("silence tail {tail:.3}s vs signal {full:.3}s");
    assert!(tail < 4.0 * full, "silence tail {tail:.3}s vs signal {full:.3}s");
}

#[test]
fn max_settings_stay_bounded() {
    // The biggest, hardest room at full decay, the tape machine at full send and repeats: the
    // RT60 is capped at 25 s, the tail stays finite and bounded, the report is honest.
    let mut s = prepared();
    for (id, v) in [
        ("mix", 1.0),
        ("decay", 1.0),
        ("roomW", 1.0),
        ("roomL", 1.0),
        ("roomH", 1.0),
        ("shape", 0.0),
        ("wallMat", 2.0),
        ("floorMat", 2.0),
        ("ceilMat", 2.0),
        ("tapeAmt", 1.0),
        ("tapeFb", 1.0),
        ("tapeTime", 1.0),
        ("predelay", 1.0),
        ("early", 1.0),
        ("tail", 1.0),
        ("level", 1.0),
        ("drive", 1.0),
    ] {
        s.set_param(id, v);
    }
    let blocks = s.engine().heap_blocks();
    let mut io = vec![hits(SR, 0.9), hits(SR, 0.9)];
    for c in &mut io {
        c.resize(SR * 20, 0.0);
    }
    run(&mut s, &mut io, 512);
    assert!(all_finite(&io));
    let peak = io.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak < 50.0, "peak {peak}");
    assert_eq!(blocks, s.engine().heap_blocks(), "nothing grows");
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert!(r[0] <= 25.0 && r[1] <= 25.0 && r[2] <= 25.0, "{r:?}");
    let tail = s.tail_samples();
    let rt = r[0].max(r[1]).max(r[2]);
    assert!(tail as f32 >= (rt + 0.2 + 2.0) * 48_000.0 - 1.0, "{tail}");
    assert!(tail <= (27.3 * 48_000.0) as usize, "{tail}");
    // And at the highest rate the delays stay in their fixed buffers.
    s.prepare(192_000.0, 512, 2);
    let mut io = vec![hits(SR, 0.9), hits(SR, 0.9)];
    run(&mut s, &mut io, 512);
    assert!(all_finite(&io));
    assert_eq!(blocks, s.engine().heap_blocks());
}

#[test]
fn process_does_not_allocate() {
    // No counting allocator in this crate: every heap block the engine owns (the fixed-size
    // rings) keeps its address and size across 1000 blocks of room changes, and the host
    // buffers keep theirs.
    let mut s = prepared();
    busy(&mut s);
    let before = s.engine().heap_blocks();
    let engine_ptr: *const ChamberMadeDsp = s.engine();
    let src = hits(256, 0.7);
    let mut io = vec![src.clone(), src.clone()];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        for c in &mut io {
            c.copy_from_slice(&src);
        }
        s.set_param("shape", ((blk / 4) % 6) as f32);
        s.set_param("o4t", (blk % 9) as f32);
        s.set_param("spkX", (blk % 10) as f32 / 9.0);
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, s.engine() as *const ChamberMadeDsp);
    assert_eq!(before, s.engine().heap_blocks());
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    let mut s = prepared();
    busy(&mut s);
    let mut io = vec![hits(512, 0.5), hits(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const ChamberMadeDsp = s.engine();
    let blocks = s.engine().heap_blocks();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const ChamberMadeDsp);
        assert_eq!(blocks, s.engine().heap_blocks());
        s.process(&mut io, 512);
    }
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(blocks, s.engine().heap_blocks());
    assert!(all_finite(&io));
}

/// Non-default parameters for the reset tests.
fn reset_setup(s: &mut ChamberMade) {
    busy(s);
    for (id, v) in [("level", 0.6), ("width", 0.8), ("early", 0.5), ("lowCut", 0.3), ("highCut", 0.7), ("mix", 0.4)] {
        s.set_param(id, v);
    }
}

#[test]
fn resets_after_different_histories_render_alike() {
    // Controller ruling: a reset is deterministic. Two instances with the same parameters but
    // different histories (other input, a Mix move, a bypass toggle, a pre-delay change, a tape
    // that was running) render bit for bit the same after a reset, pre-delay and tape on.
    let setup = |s: &mut ChamberMade| {
        reset_setup(s);
        s.set_param("predelay", 0.4);
        s.set_param("tapeAmt", 0.6);
        s.set_param("drift", 0.8);
    };
    let mut a = prepared();
    setup(&mut a);
    let mut warm = vec![hits(SR * 2, 0.9), hits(SR * 2, -0.4)];
    run(&mut a, &mut warm, 333);

    let mut b = prepared();
    setup(&mut b);
    b.set_param("mix", 1.0);
    b.set_param("predelay", 0.9);
    b.set_param("bypass", 1.0);
    let mut warm = vec![hits(SR, 0.3), hits(SR, 0.8)];
    run(&mut b, &mut warm, 512);
    b.set_param("bypass", 0.0);
    b.set_param("mix", 0.4);
    b.set_param("predelay", 0.4);
    let mut warm = vec![hits(SR / 2 + 77, -0.6), hits(SR / 2 + 77, 0.2)];
    run(&mut b, &mut warm, 128);

    let src = vec![hits(16_384, 0.7), hits(16_384, -0.4)];
    a.reset();
    let mut first = src.clone();
    run(&mut a, &mut first, 512);
    b.reset();
    let mut second = src.clone();
    run(&mut b, &mut second, 512);
    assert!(all_finite(&first));
    assert_eq!(first, second, "a reset renders the same whatever came before");
    // And again on the same instance after more playing.
    a.reset();
    let mut third = src.clone();
    run(&mut a, &mut third, 512);
    assert_eq!(first, third);
    for (i, p) in CHAMBER_PARAMS.iter().enumerate() {
        assert_eq!(a.engine().knobs()[i], a.param(p.id).unwrap(), "{}", p.id);
    }
}

#[test]
fn no_pre_delay_swoop_after_a_reset() {
    // An impulse after a reset first reaches the output at the set pre-delay (plus the direct
    // path), and that arrival does not move: the pre-delay starts at its target. Mix full wet so
    // only the room is heard; the pre-delay changed just before the reset, so a glide would show.
    let first_arrival = |s: &mut ChamberMade| {
        let mut io = vec![vec![0.0f32; SR / 2], vec![0.0f32; SR / 2]];
        io[0][0] = 1.0;
        io[1][0] = 1.0;
        run(s, &mut io, 512);
        io[0].iter().position(|v| v.abs() > 1e-4).unwrap_or(usize::MAX)
    };
    let mut s = prepared();
    s.set_param("mix", 1.0);
    let mut warm = vec![hits(SR, 0.5), hits(SR, 0.5)];
    run(&mut s, &mut warm, 512);
    s.set_param("predelay", 0.5); // 50 ms
    s.reset();
    let at = first_arrival(&mut s);
    let pre = (0.001 * d::knob_to_pre_delay_ms(0.5) * 48_000.0) as usize;
    let direct = (s.engine().chamber().taps[0].distance / 343.0 * 48_000.0) as usize;
    assert!(at >= pre + direct.saturating_sub(4) && at <= pre + direct + 8, "first arrival {at}, pre-delay {pre} + direct {direct}");
    // The same after a second reset: no glide carried in.
    s.reset();
    assert_eq!(first_arrival(&mut s), at);
}

#[test]
fn a_reset_builds_the_current_room_at_once() {
    // set_param then reset with no process in between: the room is the new one before the first
    // block (the tail length reports it at bounce start), and the first block does not glide.
    let mut s = prepared();
    let mut warm = vec![hits(SR / 2, 0.5), hits(SR / 2, 0.5)];
    run(&mut s, &mut warm, 512);
    let old_tail = s.tail_samples();
    let old_lines = s.engine().line_lengths();
    s.set_param("roomW", 1.0);
    s.set_param("roomL", 1.0);
    s.set_param("decay", 1.0);
    s.reset();
    let lines = s.engine().line_lengths();
    assert_ne!(lines, old_lines, "the room is rebuilt from the current knobs");
    assert!(s.tail_samples() > old_tail, "the tail reports the new, longer room");
    let mut k = d::default_knobs();
    k[d::P_ROOM_W] = 1.0;
    k[d::P_ROOM_L] = 1.0;
    k[d::P_DECAY] = 1.0;
    assert_eq!(s.engine().chamber(), &build_chamber_for(&k));
    // The first block: nothing to rebuild, nothing glides (same lines after it).
    let mut io = vec![hits(512, 0.5), hits(512, 0.5)];
    s.process(&mut io, 512);
    assert_eq!(lines, s.engine().line_lengths());
    // A prepare at a new rate after playing is the same path.
    s.set_param("roomH", 1.0);
    s.prepare(96_000.0, 512, 2);
    k[d::P_ROOM_H] = 1.0;
    assert_eq!(s.engine().chamber(), &build_chamber_for(&k));
    assert_eq!(s.engine().sample_rate(), 96_000.0);
}

#[test]
fn reset_clears_tails_without_a_glide() {
    let mut s = prepared();
    reset_setup(&mut s);
    let mut warm = vec![hits(SR * 2, 0.9), hits(SR * 2, 0.9)];
    run(&mut s, &mut warm, 512);
    let lines = s.engine().line_lengths();
    s.reset();
    // The tail is gone: silence in, silence out, exactly.
    let mut quiet = vec![vec![0.0f32; 8192], vec![0.0f32; 8192]];
    run(&mut s, &mut quiet, 512);
    assert!(quiet.iter().flatten().all(|&v| v == 0.0), "the reset cleared every buffer");
    // The room is the module's, not rebuilt from the defaults (no re-glide).
    assert_eq!(lines, s.engine().line_lengths());
    // No mute and no glide: the first sample after a reset is the dry path at its settled
    // gains (the wet path is still empty), level 0.6 and mix 0.4.
    s.reset();
    let x = 0.5f32;
    let mut io = vec![vec![x; 64], vec![x; 64]];
    s.process(&mut io, 64);
    let dry = d::knob_to_level_gain(0.6) * (2.0f32 * (1.0 - 0.4)).min(1.0);
    assert!((io[0][0] - x * dry).abs() < 1e-4, "{} vs {}", io[0][0], x * dry);
}

#[test]
fn prepare_starts_from_the_defaults_like_a_fresh_cpp_instance() {
    // The C++ wrapper prepares the engine with the knobs it holds and pushes the parameters with
    // the first block, so a fresh instance builds the default room first and moves to the set
    // one on the first control tick: the first 63 samples use the default room.
    let mut s = ChamberMade::new();
    s.set_param("roomW", 1.0);
    s.prepare(48_000.0, 512, 2);
    let default_lines = s.engine().line_lengths();
    let mut io = vec![hits(63, 0.5), hits(63, 0.5)];
    s.process(&mut io, 63);
    assert_eq!(default_lines, s.engine().line_lengths());
    let mut io = vec![hits(1, 0.5), hits(1, 0.5)];
    s.process(&mut io, 1);
    assert_ne!(default_lines, s.engine().line_lengths(), "rebuilt on the 64th sample");
}

#[test]
fn bypass_is_the_input_plus_the_ringing_tail() {
    let mut s = prepared();
    s.set_param("mix", 1.0);
    s.set_param("bypass", 1.0);
    let src = hits(SR, 0.6);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    // The send is gated within ~10 ms, so all that is left on top of the input is the little
    // that got in before the gate closed, ringing down.
    let err: Vec<f32> = io[0][SR / 2..].iter().zip(&src[SR / 2..]).map(|(a, b)| a - b).collect();
    // Not bypassed, the same room puts its whole wet return on top of the input.
    let mut s = prepared();
    s.set_param("mix", 1.0);
    let mut open = vec![src.clone(), src.clone()];
    run(&mut s, &mut open, 512);
    let open_err: Vec<f32> = open[0][SR / 2..].iter().zip(&src[SR / 2..]).map(|(a, b)| a - b).collect();
    let gap = 20.0 * (rms(&open_err) / rms(&err)).log10();
    assert!(gap > 10.0, "the bypassed residual is only {gap} dB under the open wet");
}
