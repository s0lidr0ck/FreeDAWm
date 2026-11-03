use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
}

/// A plucked, decaying chord-ish signal so the envelope moves.
fn plucks(len: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let t = (i % 12_000) as f32 / 48_000.0;
            let env = (-t * 12.0).exp();
            let x = i as f32 / 48_000.0;
            amp * env * ((std::f32::consts::TAU * 110.0 * x).sin() + 0.5 * (std::f32::consts::TAU * 330.0 * x).sin())
        })
        .collect()
}

fn prepared() -> Filter {
    let mut s = Filter::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut Filter, io: &mut [Vec<f32>], block: usize) {
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

fn transport(playing: bool, tempo: f64, quarters: f64) -> Transport {
    Transport { playing, tempo, quarters, numerator: 4, denominator: 4, ..Transport::default() }
}

const HOSTILE: [f32; 9] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0];
const CHOICES: [&str; 20] = [
    "circuit",
    "modSrc",
    "lfoShape",
    "lfoSync",
    "note",
    "seqSteps",
    "tempoSrc",
    "qtRange",
    "qtMode",
    "muRange",
    "muMode",
    "cbInductor",
    "cbVoice",
    "voxVoice",
    "moogSlope",
    "moogComp",
    "msMode",
    "semMode",
    "tbMod",
    "waspMode",
];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = FILTER_PARAMS.iter().map(|p| p.id).collect();
    let mut want = vec![
        "cutoff",
        "reso",
        "drive",
        "mix",
        "level",
        "circuit",
        "bypass",
        "trim",
        "modSrc",
        "depth",
        "envSens",
        "envAtk",
        "envRel",
        "lfoShape",
        "lfoSync",
        "lfoRate",
        "note",
        "lfoStereo",
        "seqSteps",
        "seqGlide",
        "tempoSrc",
        "bpm",
    ];
    let steps: Vec<String> = (1..=16).map(|i| format!("step{i}")).collect();
    want.extend(steps.iter().map(String::as_str));
    want.extend([
        "qtRange",
        "qtMode",
        "muRange",
        "muMode",
        "cbInductor",
        "cbVoice",
        "voxVoice",
        "moogSlope",
        "moogComp",
        "msMode",
        "semMode",
        "tbMod",
        "waspMode",
        "envSrc",
    ]);
    assert_eq!(ids, want);
    assert_eq!(FILTER_PARAMS[d::P_STEP1].id, "step1");
    assert_eq!(FILTER_PARAMS[d::P_STEP16].id, "step16");
    assert_eq!(FILTER_PARAMS[d::P_ENV_SRC].id, "envSrc");
    let defaults: Vec<f32> = FILTER_PARAMS.iter().take(22).map(|p| p.default).collect();
    assert_eq!(defaults, [0.5, 0.5, 0.0, 1.0, 0.6667, 0.0, 0.0, 0.3333, 1.0, 0.85, 0.4, 0.2, 0.4, 0.0, 1.0, 0.4, 9.0, 0.0, 6.0, 0.1, 0.0, 0.4]);
    let step_defaults: Vec<f32> = FILTER_PARAMS[d::P_STEP1..=d::P_STEP16].iter().map(|p| p.default).collect();
    assert_eq!(step_defaults, [0.20, 0.80, 0.45, 0.65, 0.10, 0.90, 0.35, 0.55, 0.20, 0.80, 0.45, 0.65, 0.10, 0.90, 0.35, 0.55]);
    let switch_defaults: Vec<f32> = FILTER_PARAMS[d::P_QT_RANGE..].iter().map(|p| p.default).collect();
    assert_eq!(switch_defaults, [1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(FILTER_PARAMS[d::P_CIRCUIT].choices.len(), 9);
    assert_eq!(FILTER_PARAMS[d::P_NOTE].choices.len(), 18);
    assert_eq!(FILTER_PARAMS[d::P_SEQ_STEPS].choices.len(), 15);
    assert_eq!(FILTER_PARAMS[d::P_BYPASS].group, "", "the engine's bypass is hidden");
    // the circuit names line up with the config table
    for (i, c) in filter_config::CIRCUITS.iter().enumerate() {
        assert_eq!(FILTER_PARAMS[d::P_CIRCUIT].choices[i], c.name);
        for sw in c.switches {
            assert_eq!(FILTER_PARAMS[sw.param].choices.len(), sw.descriptions.len(), "{} {}", c.name, sw.label);
        }
    }
    // formatValue, through the engine's own maps
    assert_eq!(FILTER_PARAMS[d::P_LEVEL].format(0.6667), "+0.0 dB");
    assert_eq!(FILTER_PARAMS[d::P_DEPTH].format(0.85), "Up 70 %");
    assert_eq!(FILTER_PARAMS[d::P_MIX].format(1.0), "Wet only");
    assert_eq!(FILTER_PARAMS[d::P_SEQ_STEPS].format(6.0), "8");
}

#[test]
fn engine_maps_match_the_header() {
    assert_eq!(d::knob_to_step_count(0.0), 2);
    assert_eq!(d::knob_to_step_count(6.0), 8);
    assert_eq!(d::knob_to_step_count(14.0), 16);
    assert_eq!(d::note_to_beats(9), 1.0);
    assert_eq!(d::note_to_beats(0), 16.0);
    assert_eq!(d::note_to_beats(99), 0.125);
    assert_eq!(d::knob_to_bpm(0.4), 120.0);
    assert_eq!(d::knob_to_depth_bipolar(0.85), 0.70000005);
}

#[test]
fn registered_with_no_latency_and_a_one_second_tail() {
    let mut p = crate::create("fs_filter").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    for circuit in 0..9 {
        p.set_param("circuit", circuit as f32);
        p.prepare(48_000.0, 512, 2);
        let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
        p.process(&mut io, 512);
        assert_eq!(p.latency(), 0);
        assert_eq!(p.tail_samples(), 48_000);
    }
    p.prepare(96_000.0, 512, 2);
    assert_eq!(p.tail_samples(), 96_000);
}

#[test]
fn readouts_report_the_live_cutoff_mod_tempo_and_step() {
    let mut s = prepared();
    s.set_param("circuit", 4.0); // Moog: fc = 30 * 600^p
    s.set_param("modSrc", 0.0); // Manual: the cutoff knob alone
    s.set_param("cutoff", 0.5);
    let mut io = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut s, &mut io, 512);
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert!((r[0] - gain_to_db(0.5)).abs() < 0.1, "{r:?}");
    assert!(r[1].is_finite(), "{r:?}");
    let want = 30.0 * 600.0f64.powf(0.5);
    assert!((f64::from(r[2]) - want).abs() < 1.0, "{r:?} vs {want}");
    assert_eq!(r[3], 0.0, "Manual: no modulation");
    assert_eq!(r[4], -120.0, "no host tempo: the BPM knob runs it");
    assert_eq!(r[5], -1.0, "Steps is not the source");
    // Steps: the step index, and a host tempo in use.
    s.set_param("modSrc", 3.0);
    s.set_transport(&transport(true, 90.0, 2.5));
    run(&mut s, &mut io, 512);
    s.readouts(&mut r);
    assert_eq!(r[4], 90.0);
    assert!((0.0..8.0).contains(&r[5]), "{r:?}");
}

#[test]
fn params_are_pushed_only_when_they_move() {
    // Circuit's setter clears every solver state: re-sending the same value must not.
    let mut a = prepared();
    let mut b = prepared();
    for s in [&mut a, &mut b] {
        s.set_param("circuit", 4.0);
        s.set_param("reso", 0.8);
        s.set_param("modSrc", 2.0);
    }
    let src = plucks(48_000, 0.6);
    let mut ia = vec![src.clone(), src.clone()];
    let mut ib = ia.clone();
    let block = 480;
    for (ca, cb) in ia[0].chunks_mut(block).zip(ib[0].chunks_mut(block)) {
        let mut x = vec![ca.to_vec(), ca.to_vec()];
        let mut y = vec![cb.to_vec(), cb.to_vec()];
        b.set_param("circuit", 4.0); // unchanged: nothing to push
        a.process(&mut x, block);
        b.process(&mut y, block);
        ca.copy_from_slice(&x[0]);
        cb.copy_from_slice(&y[0]);
    }
    assert_eq!(ia[0], ib[0]);
    // A forced re-push every block would clear the ladder every block: the output differs.
    let mut c = prepared();
    c.set_param("circuit", 4.0);
    c.set_param("reso", 0.8);
    c.set_param("modSrc", 2.0);
    let mut ic = [src.clone(), src.clone()];
    for chunk in ic[0].chunks_mut(block) {
        let mut x = vec![chunk.to_vec(), chunk.to_vec()];
        c.force_push = true;
        c.process(&mut x, block);
        chunk.copy_from_slice(&x[0]);
    }
    assert_ne!(ic[0], ia[0]);
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_filter");
    assert_eq!(presets.len(), 51);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &FILTER_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert!(!p.values.contains_key("bypass"), "{name}");
        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        for key in raw["values"].as_object().unwrap().keys() {
            assert!(p.values.contains_key(key), "{name}: {key} did not resolve");
        }
        for key in ["circuit", "cutoff", "reso", "modSrc", "seqSteps", "note", "step16", "envSrc"] {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
    }
    let acid = presets.iter().find(|(n, _)| *n == "Acid Line").unwrap();
    let p = FsPreset::parse(acid.1, &FILTER_INFO).unwrap();
    assert_eq!(p.values["circuit"], 7.0);
    assert_eq!(p.values["modSrc"], 1.0);
    assert_eq!(p.values["seqSteps"], 6.0, "the label \"8\" is the eighth count");
    assert_eq!(p.values["note"], 9.0);
}

#[test]
fn hostile_params_stay_finite() {
    // Bypass stays off (it would hide the wet path); the choices are set per case.
    for bad in HOSTILE {
        for circuit in 0..9 {
            for src in 0..4 {
                let mut s = prepared();
                s.set_param("circuit", circuit as f32);
                s.set_param("modSrc", src as f32);
                for info in FILTER_PARAMS.iter().filter(|p| !CHOICES.contains(&p.id) && p.id != "bypass") {
                    assert!(s.set_param(info.id, bad));
                }
                assert_eq!(s.param("bypass"), Some(0.0));
                let mut io = vec![sine(4096, 0.9), sine(4096, -0.9)];
                run(&mut s, &mut io, 256);
                assert!(all_finite(&io), "circuit {circuit}, source {src}, value {bad}");
                assert!(io.iter().flatten().all(|v| v.abs() <= 1.5), "circuit {circuit}: past the output knee");
            }
        }
    }
}

#[test]
fn hostile_choice_params_stay_finite() {
    for bad in HOSTILE {
        let mut s = prepared();
        s.set_param("reso", 1.0);
        s.set_param("drive", 1.0);
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
        for circuit in 0..9 {
            let mut s = prepared();
            s.set_param("circuit", circuit as f32);
            s.set_param("reso", 0.9);
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
fn hostile_transport_stays_finite() {
    for (tempo, quarters) in
        [(f64::NAN, 0.0), (f64::INFINITY, 1.0), (-120.0, f64::NAN), (1e12, 1e12), (0.0, -5.0), (19.9, 3.0), (999.0, f64::NEG_INFINITY)]
    {
        for src in 2..4 {
            let mut s = prepared();
            s.set_param("modSrc", src as f32);
            let io = [sine(4096, 0.7), sine(4096, 0.7)];
            for (i, chunk) in io[0].clone().chunks(256).enumerate() {
                s.set_transport(&transport(i % 3 != 0, tempo, quarters + i as f64));
                let mut x = vec![chunk.to_vec(), chunk.to_vec()];
                s.process(&mut x, chunk.len());
                assert!(all_finite(&x), "tempo {tempo}, quarters {quarters}");
            }
            let mut r = [f32::NAN; 8];
            s.readouts(&mut r);
            assert!(r.iter().take(6).all(|v| v.is_finite()), "{r:?}");
        }
    }
}

/// `a - b` folded into -0.5..0.5 (phases are cyclic).
fn wrap_diff(a: f64, b: f64) -> f64 {
    let d = a - b;
    d - d.round()
}

#[test]
fn tempo_sync_follows_transport() {
    // LFO synced to quarter notes, Steps one step per quarter: everything follows the host.
    let sr = 48_000.0;
    let block = 512usize;
    let mut s = prepared();
    s.set_param("modSrc", 2.0);
    s.set_param("lfoSync", 1.0);
    s.set_param("note", 9.0); // 1/4: one beat
    let silence = vec![vec![0.0f32; block], vec![0.0f32; block]];
    let step = |s: &mut Filter, t: Transport| {
        s.set_transport(&t);
        let mut io = silence.clone();
        s.process(&mut io, block);
        assert!(all_finite(&io));
    };
    let mut r = [0.0f32; 8];

    // Stopped, host tempo 150: the LFO free-runs at the host's rate (no grid lock).
    step(&mut s, transport(false, 150.0, 7.3));
    let p0 = s.engine().lfo_phase();
    step(&mut s, transport(false, 150.0, 7.3));
    let per_block = 150.0 / 60.0 * block as f64 / sr;
    assert!(wrap_diff(s.engine().lfo_phase(), p0 + per_block).abs() < 1e-9, "stopped: free-runs at 150 BPM");
    for _ in 0..4 {
        step(&mut s, transport(false, 150.0, 7.3));
    }
    s.readouts(&mut r);
    assert_eq!(r[4], 150.0, "the host tempo is in use while stopped");

    // Playing: the phase is pulled onto the grid (ppq / beats, fractional part) at the block start.
    let mut q = 10.25;
    s.set_transport(&transport(true, 120.0, q));
    assert!((s.engine().lfo_phase() - 0.25).abs() < 1e-9, "locked to the bar position");
    let mut io = silence.clone();
    s.process(&mut io, block);
    // ... and runs at the host tempo, so the next block start agrees with the grid (no re-snap).
    q += 120.0 / 60.0 * block as f64 / sr;
    let before = s.engine().lfo_phase();
    assert!(wrap_diff(before, q).abs() < 1e-9, "{before} vs {q}");
    s.set_transport(&transport(true, 120.0, q));
    assert_eq!(s.engine().lfo_phase(), before, "inside the 0.02 tolerance: no jump");

    // Seek forward, then a loop back: the next block snaps to the new position.
    s.set_transport(&transport(true, 120.0, 33.6));
    assert!((s.engine().lfo_phase() - 0.6).abs() < 1e-9, "sought to beat 33.6");
    step(&mut s, transport(true, 120.0, 33.6));
    s.set_transport(&transport(true, 120.0, 4.9));
    assert!((s.engine().lfo_phase() - 0.9).abs() < 1e-9, "looped back to beat 4.9");

    // A tempo change: the rate follows from the next block on.
    s.set_transport(&transport(true, 60.0, 4.9));
    let a = s.engine().lfo_phase();
    let mut io = silence.clone();
    s.process(&mut io, block);
    let got = wrap_diff(s.engine().lfo_phase(), a);
    assert!((got - block as f64 / sr).abs() < 1e-9, "60 BPM rate: {got}");
    for i in 1..5 {
        step(&mut s, transport(true, 60.0, 4.9 + i as f64 * block as f64 / sr));
    }
    s.readouts(&mut r);
    assert_eq!(r[4], 60.0, "the tempo in use: {r:?}");

    // Steps: the sequencer is on the grid too (step = quarter, 8 steps).
    let mut s = prepared();
    s.set_param("modSrc", 3.0);
    s.set_param("note", 9.0);
    step(&mut s, transport(true, 120.0, 0.0));
    s.set_transport(&transport(true, 120.0, 13.5));
    assert!((s.engine().seq_phase() - 5.5).abs() < 1e-9, "13.5 beats into an 8-step loop");
    // Manual tempo source (the C++ path, hidden in the DAW): the host grid is ignored.
    s.set_param("tempoSrc", 1.0);
    step(&mut s, transport(true, 120.0, 13.5));
    s.set_transport(&transport(true, 120.0, 2.0));
    let p = s.engine().seq_phase();
    assert!((p - 2.0).abs() > 0.05, "Manual: not pulled onto the host grid ({p})");
    // No transport at all (the render tool's case): the BPM knob, reported negative.
    let mut s = prepared();
    s.set_param("modSrc", 3.0);
    let mut io = vec![vec![0.0f32; 48_000], vec![0.0f32; 48_000]];
    run(&mut s, &mut io, 512);
    s.readouts(&mut r);
    assert_eq!(r[4], -120.0);
}

#[test]
fn switching_quality_and_model_mid_stream_is_safe() {
    // No quality setting (the engine always runs 4x): circuit, mode switches and the mod source
    // are what switch.
    let mut s = prepared();
    s.set_param("reso", 0.95);
    s.set_param("drive", 0.7);
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = plucks(128 * 700, 0.9);
    for blk in 0..700 {
        s.set_param("circuit", (blk % 9) as f32);
        s.set_param("modSrc", ((blk / 3) % 4) as f32);
        s.set_param("msMode", ((blk / 2) % 3) as f32);
        s.set_param("qtMode", ((blk / 5) % 4) as f32);
        s.set_param("cbInductor", ((blk / 7) % 4) as f32);
        s.set_param("moogSlope", ((blk / 4) % 4) as f32);
        s.set_param("tbMod", ((blk / 6) % 2) as f32);
        s.set_param("seqSteps", (blk % 15) as f32);
        s.set_transport(&transport(blk % 2 == 0, 80.0 + blk as f64, blk as f64 * 0.37));
        for b in &mut buf {
            b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
        }
        s.process(&mut buf, 128);
        assert!(all_finite(&buf), "block {blk}");
        assert!(buf.iter().flatten().all(|v| v.abs() <= 1.5), "block {blk}: past the output knee");
    }
}

#[test]
fn mono_and_wide_buffers() {
    // Mono: one channel processed.
    let mut s = Filter::new();
    s.prepare(48_000.0, 512, 1);
    s.set_param("cutoff", 0.2);
    let mut mono = vec![sine(9600, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    // Six channels: the first two processed, the rest untouched.
    let mut s = Filter::new();
    s.prepare(48_000.0, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(9600, 0.1 * (c + 1) as f32)).collect();
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
    for circuit in 0..9 {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            s.set_param("circuit", circuit as f32);
            s.set_param("reso", 0.7);
            s.set_param("drive", 0.4);
            let mut io = vec![plucks(signal_s * sr, 0.9), plucks(signal_s * sr, 0.9)];
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
    // No counting allocator in this crate (`unsafe_code` is forbidden, so no `GlobalAlloc`): the
    // engine is plain data (fixed arrays, no `Vec`) in the one box made in `new`, so the box and
    // the host buffers keeping their addresses across 1000 blocks of switching is the whole story.
    let mut s = prepared();
    let engine_ptr: *const FilterEngineDsp = s.engine();
    let src = plucks(256, 0.7);
    let mut io = vec![src.clone(), src.clone()];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        for c in &mut io {
            c.copy_from_slice(&src);
        }
        s.set_param("circuit", ((blk / 4) % 9) as f32);
        s.set_param("modSrc", (blk % 4) as f32);
        s.set_transport(&transport(true, 128.0, blk as f64 * 0.1));
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, s.engine() as *const FilterEngineDsp);
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is re-prepared
    // in its own box, never re-made. The box pointer is the whole check because `FilterEngineDsp`
    // has no heap fields (fixed arrays only, see `filter_engine_dsp.rs`): both the fresh path
    // (`*self = FilterEngineDsp::new()`) and the live one write in place. If a `Vec` or `Box` field is ever added, this test
    // no longer proves anything and must also compare that buffer's pointer and capacity.
    let mut s = prepared();
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const FilterEngineDsp = s.engine();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const FilterEngineDsp);
        s.process(&mut io, 512);
    }
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(engine_ptr, s.engine() as *const FilterEngineDsp);
    assert!(all_finite(&io));
}

/// Every parameter at its default, then a busy setting on `circuit` (modulation, sequencer,
/// switches away from their defaults).
fn busy(s: &mut Filter, circuit: usize) {
    for p in FILTER_PARAMS.iter() {
        s.set_param(p.id, p.default);
    }
    for (id, v) in [
        ("circuit", circuit as f32),
        ("cutoff", 0.3),
        ("reso", 0.85),
        ("drive", 0.6),
        ("mix", 0.7),
        ("level", 0.4),
        ("trim", 0.8),
        ("modSrc", (circuit % 4) as f32),
        ("depth", 0.2),
        ("lfoShape", if circuit.is_multiple_of(2) { 6.0 } else { 5.0 }),
        ("lfoRate", 0.6),
        ("lfoStereo", 0.5),
        ("seqSteps", 3.0),
        ("seqGlide", 0.6),
        ("step2", 0.0),
        ("envSrc", 1.0),
        ("envAtk", 0.7),
        ("moogComp", 1.0),
        ("tbMod", 1.0),
        ("msMode", 2.0),
    ] {
        s.set_param(id, v);
    }
}

#[test]
fn a_reset_is_independent_of_what_played_before() {
    // Two modules with different histories (input, knob moves, circuit and source switches, a
    // playing host on another grid position), brought to the same values and the same host
    // transport and reset, render bit for bit the same over 64000 samples: more than two cycles
    // of the 1.8 Hz LFO, and the S&H / Drift random source reseeded.
    for circuit in 0..9 {
        let mut a = Filter::new();
        a.prepare(44_100.0, 512, 2);
        a.set_param("circuit", ((circuit + 3) % 9) as f32);
        a.set_param("modSrc", 3.0);
        a.set_transport(&transport(true, 133.0, 17.25));
        let mut warm = vec![plucks(7000, 0.9), sine(7000, -0.3)];
        run(&mut a, &mut warm, 333);
        busy(&mut a, circuit);
        let mut warm = vec![plucks(301, 0.9), plucks(301, 0.9)];
        run(&mut a, &mut warm, 64);

        let mut b = Filter::new();
        b.prepare(44_100.0, 512, 2);
        b.set_param("cutoff", 0.9);
        b.set_param("modSrc", 2.0);
        b.set_param("lfoShape", 5.0);
        let mut warm = vec![sine(5000, 0.7), plucks(5000, 0.5)];
        run(&mut b, &mut warm, 128);
        busy(&mut b, circuit);

        for m in [&mut a, &mut b] {
            m.set_transport(&transport(false, 120.0, 0.0));
            m.reset();
        }
        let src = vec![plucks(64_000, 0.7), sine(64_000, -0.4)];
        let (mut x, mut y) = (src.clone(), src.clone());
        run(&mut a, &mut x, 512);
        run(&mut b, &mut y, 300);
        assert!(x == y, "circuit {circuit}");
        assert!(all_finite(&x));
    }
}

#[test]
fn a_reset_does_not_glide_or_sweep() {
    // Cutoff, Reso, Drive, Mix, Level and Input sit at their knobs right after a reset, and the
    // pedal position and the slow opto cell (Q-Tron, Mu-Tron) sit at the cutoff: no sweep from
    // the C++ `prepare`'s parked 0.5, which the opto would take about a second to leave.
    for circuit in [0usize, 1, 4] {
        let mut s = prepared();
        let mut warm = vec![plucks(9600, 0.7), plucks(9600, 0.7)];
        run(&mut s, &mut warm, 512);
        s.set_param("circuit", circuit as f32);
        s.set_param("modSrc", 0.0);
        s.set_param("cutoff", 0.15);
        s.set_param("level", 0.2);
        s.set_param("mix", 0.4);
        s.reset();
        let (got, want) = s.engine().smoothers();
        assert_eq!(got, want, "circuit {circuit}");
        let (pos, opto) = s.engine().positions();
        assert_eq!(pos, [0.15, 0.15], "circuit {circuit}");
        assert_eq!(opto, [0.15, 0.15], "circuit {circuit}");

        // And the output right after a reset matches the settled output on the same input.
        let src = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
        let mut settled = src.clone();
        run(&mut s, &mut settled, 512);
        s.reset();
        let mut io = src.clone();
        run(&mut s, &mut io, 512);
        let e: f32 = io[0][4800..9600].iter().zip(&settled[0][4800..9600]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(e < 1e-4, "circuit {circuit}: {e}");
    }
}

#[test]
fn a_reset_keeps_the_host_transport_and_relocks() {
    // The host tempo and play state are the host's: a reset keeps them (Tempo Source Host keeps
    // the session tempo), and the next transport puts a synced LFO back on the host's grid.
    let mut s = prepared();
    s.set_param("modSrc", 2.0);
    s.set_param("lfoSync", 1.0);
    s.set_param("note", 9.0); // 1/4
    s.set_transport(&transport(true, 150.0, 3.0));
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    s.reset();
    assert_eq!(s.engine().host_transport(), (true, 150.0));
    let mut r = [0.0f32; 8];
    for _ in 0..4 {
        s.process(&mut io, 512);
    }
    s.readouts(&mut r);
    assert_eq!(r[4], 150.0, "still on the session tempo after the reset");
    s.reset();
    s.set_transport(&transport(true, 150.0, 41.4));
    assert!((s.engine().lfo_phase() - 0.4).abs() < 1e-9, "relocked to beat 41.4");
    // The fresh path (no audio yet) keeps it too.
    let mut f = prepared();
    f.set_transport(&transport(true, 90.0, 0.0));
    f.reset();
    f.prepare(44_100.0, 512, 2);
    assert_eq!(f.engine().host_transport(), (true, 90.0));
}

#[test]
fn a_reset_before_any_audio_is_the_fresh_path() {
    for circuit in [0usize, 3, 5, 8] {
        let src = vec![plucks(8192, 0.7), sine(8192, -0.4)];
        let mut fresh = Filter::new();
        fresh.prepare(44_100.0, 512, 2);
        busy(&mut fresh, circuit);
        let mut want = src.clone();
        run(&mut fresh, &mut want, 512);

        let mut s = Filter::new();
        s.prepare(96_000.0, 512, 2);
        busy(&mut s, circuit);
        s.reset();
        s.prepare(44_100.0, 512, 2);
        s.reset();
        let mut got = src.clone();
        run(&mut s, &mut got, 512);
        assert!(got == want, "circuit {circuit}");
    }
}

#[test]
fn bypass_passes_the_input() {
    for circuit in 0..9 {
        let mut s = prepared();
        s.set_param("circuit", circuit as f32);
        s.set_param("bypass", 1.0);
        let src = sine(4800, 0.6);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        assert_eq!(io[0], src, "circuit {circuit}");
    }
}

#[test]
fn every_circuit_filters() {
    // Manual source, cutoff low: each circuit takes the top off a bright signal (the highpass
    // modes aside), at the default (lowpass) switch settings.
    let noise: Vec<f32> = {
        let mut r = 1u32;
        (0..48_000)
            .map(|_| {
                r = r.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (r >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    };
    let hf = |x: &[f32]| x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>() / x.iter().map(|v| v * v).sum::<f32>().max(1e-12);
    for circuit in 0..9 {
        let mut s = prepared();
        s.set_param("circuit", circuit as f32);
        s.set_param("modSrc", 0.0);
        s.set_param("cutoff", 0.15);
        s.set_param("reso", 0.2);
        let mut io = vec![noise.clone(), noise.clone()];
        run(&mut s, &mut io, 512);
        assert!(all_finite(&io));
        assert!(hf(&io[0][24_000..]) < 0.5 * hf(&noise[24_000..]), "circuit {circuit}: the top end stays");
    }
}

#[test]
fn position_to_hz_is_update_channel() {
    // Moog: fc = 30 * 600^p; the Q-Tron's opto network; everything clamped to 10..22000 Hz.
    let mut e = FilterEngineDsp::new();
    e.set_parameter(d::P_CIRCUIT, 4.0);
    assert!((e.position_to_hz(0.5) - 30.0 * 600.0f64.sqrt()).abs() < 1e-9);
    e.set_parameter(d::P_CIRCUIT, 0.0);
    let lo = e.position_to_hz(0.0);
    let hi = e.position_to_hz(1.0);
    assert!(lo < hi && (10.0..=22_000.0).contains(&lo) && (10.0..=22_000.0).contains(&hi), "{lo} {hi}");
}

#[test]
fn linear_coefs_are_update_shared() {
    let mut e = FilterEngineDsp::new();
    // Q-Tron at full Peak: Q = 14, damping 1/14, input comp 1/sqrt(14), gain inV * outV = 1.4.
    let c = e.linear_coefs(1.0, 0.0);
    assert!((c.svf_damp - 1.0 / 14.0).abs() < 1e-9 && (c.svf_in_comp - 1.0 / 14.0f64.sqrt()).abs() < 1e-9);
    assert!((c.gain - 1.4).abs() < 1e-9);
    e.set_parameter(d::P_CIRCUIT, 4.0);
    e.set_parameter(d::P_MOOG_COMP, 1.0);
    let c = e.linear_coefs(0.5, 0.0);
    assert!((c.lad_k - 2.15).abs() < 1e-9 && (c.lad_in_comp - (1.0 + 0.6 * 2.15)).abs() < 1e-9);
}

#[test]
fn a_reset_while_the_host_plays_seats_at_the_relocked_phase() {
    // Steps with Glide up on a Q-Tron (circuit 0) and its Hi range: after a reset and the
    // playing host's next transport, the sequencer value and the opto cell are already where the
    // relocked step puts them, so neither glides nor sweeps from the phase-0 seat.
    for qt_range in [0.0, 1.0] {
        let mut s = prepared();
        for (id, v) in [("circuit", 0.0), ("qtRange", qt_range), ("modSrc", 3.0), ("note", 9.0), ("seqGlide", 0.9), ("depth", 0.9), ("cutoff", 0.4)] {
            s.set_param(id, v);
        }
        s.set_transport(&transport(true, 120.0, 2.0));
        let mut io = vec![plucks(4096, 0.5), plucks(4096, 0.5)];
        run(&mut s, &mut io, 512);
        s.reset();
        // 13.5 beats: step 6 of 8 (step 1 holds 0.20, step 6 0.90).
        s.set_transport(&transport(true, 120.0, 13.5));
        let mut io = vec![plucks(256, 0.5), plucks(256, 0.5)];
        s.process(&mut io, 256);
        let (val, target) = s.engine().seq_value();
        assert_eq!(val, target, "qtRange {qt_range}: the sequencer sits at its step");
        let want = (0.4f32 + 0.8 * (2.0 * target - 1.0)).clamp(0.0, 1.0);
        let (pos, opto) = s.engine().positions();
        for c in 0..2 {
            assert!((pos[c] - want).abs() < 1e-6, "qtRange {qt_range}: position {pos:?} vs {want}");
            assert!((opto[c] - want).abs() < 1e-6, "qtRange {qt_range}: opto {opto:?} vs {want}");
        }
        assert!(all_finite(&io));
    }
}

#[test]
fn a_reset_while_the_host_plays_is_independent_of_what_played_before() {
    // As the two-histories test, with the host playing (synced LFO and Steps, on the grid): after a
    // reset and the same transport stream, two histories render bit for bit the same.
    for circuit in 0..9 {
        let setup = |s: &mut Filter| {
            busy(s, circuit);
            s.set_param("lfoSync", 1.0);
            s.set_param("note", 9.0);
            s.set_param("modSrc", [2.0, 3.0][circuit % 2]);
        };
        let mut a = prepared();
        a.set_transport(&transport(true, 133.0, 17.25));
        let mut warm = vec![plucks(7000, 0.9), sine(7000, -0.3)];
        run(&mut a, &mut warm, 333);
        setup(&mut a);
        let mut warm = vec![plucks(500, 0.9), plucks(500, 0.9)];
        run(&mut a, &mut warm, 100);

        let mut b = prepared();
        b.set_param("modSrc", 2.0);
        b.set_transport(&transport(true, 90.0, 3.1));
        let mut warm = vec![sine(5000, 0.7), plucks(5000, 0.5)];
        run(&mut b, &mut warm, 128);
        setup(&mut b);

        a.reset();
        b.reset();
        let block = 512usize;
        let src = vec![plucks(64_000, 0.7), sine(64_000, -0.4)];
        let (mut x, mut y) = (src.clone(), src.clone());
        for (m, out) in [(&mut a, &mut x), (&mut b, &mut y)] {
            let mut q = 41.4;
            let mut pos = 0;
            while pos < 64_000 {
                let n = block.min(64_000 - pos);
                m.set_transport(&transport(true, 120.0, q));
                let mut buf: Vec<Vec<f32>> = out.iter().map(|c| c[pos..pos + n].to_vec()).collect();
                m.process(&mut buf, n);
                for (c, b) in out.iter_mut().zip(&buf) {
                    c[pos..pos + n].copy_from_slice(b);
                }
                q += 120.0 / 60.0 * n as f64 / 48_000.0;
                pos += n;
            }
        }
        assert!(x == y, "circuit {circuit}");
        assert!(all_finite(&x));
    }
}
