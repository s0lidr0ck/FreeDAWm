use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

const CIRCUITS: usize = 12;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
}

fn prepared() -> Dist {
    let mut s = Dist::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut Dist, io: &mut [Vec<f32>], block: usize) {
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

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    assert_eq!(DIST_PARAMS.len(), 54);
    let ids: Vec<&str> = DIST_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(ids[..7], ["drive", "tone", "level", "circuit", "bypass", "trim", "link"]);
    assert_eq!(ids[7..9], ["tsLo", "tsHi"]);
    assert_eq!(ids[29..31], ["ocdLo", "ocdHi"]);
    assert_eq!(ids[31], "tsModel");
    assert_eq!(ids[53], "ocdMode");
    assert_eq!(DIST_PARAMS[5].default, 0.3333);
    assert_eq!(DIST_PARAMS[6].default, 0.0, "Link is off by default");
    assert_eq!(DIST_PARAMS[5].format(0.3333), "+0.0 dB");
    assert_eq!(DIST_PARAMS[5].format(1.0), "+24.0 dB");
    assert_eq!(DIST_PARAMS[5].format(0.0), "-12.0 dB");
    assert_eq!(DIST_PARAMS[0].format(0.5), "5.0");
    let defaults: Vec<(&str, f32)> = DIST_PARAMS.iter().filter(|p| p.default != 0.0).map(|p| (p.id, p.default)).collect();
    assert!(defaults.contains(&("muffVer", 1.0)) && defaults.contains(&("ffBias", 1.0)) && defaults.contains(&("tdPlate", 1.0)));
    assert!(defaults.contains(&("govBass", 1.0)) && defaults.contains(&("govMid", 1.0)) && defaults.contains(&("tdBias", 1.0)));
}

#[test]
fn registered_and_reports_no_latency() {
    let p = crate::create("fs_dist").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    assert_eq!(p.latency(), 0, "the C++ reports no latency");
    assert!(crate::param_deriver("fs_dist").is_some(), "Link and the knob memory");
}

#[test]
fn every_circuit_makes_sound_and_differs() {
    let mut outs: Vec<Vec<f32>> = Vec::new();
    for circuit in 0..CIRCUITS {
        let mut s = prepared();
        s.set_param("circuit", circuit as f32);
        s.set_param("drive", 0.7);
        let mut io = vec![sine(9600, 0.5), sine(9600, 0.5)];
        run(&mut s, &mut io, 512);
        assert!(all_finite(&io), "circuit {circuit}");
        assert!(rms(&io[0][4800..]) > 1e-3, "circuit {circuit} is silent");
        outs.push(io[0].clone());
    }
    for (i, a) in outs.iter().enumerate() {
        for b in &outs[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn set_order_does_not_matter_as_in_the_cpp() {
    // The C++ pushes in parameter order at the start of a block, whatever order the host set them.
    let a_vals = [("tdPlate", 2.0), ("circuit", 8.0), ("drive", 0.8), ("tdBias", 0.0)];
    let render = |order: &[(&str, f32)]| {
        let mut s = prepared();
        for (k, v) in order {
            s.set_param(k, *v);
        }
        let mut io = vec![sine(4096, 0.5), sine(4096, 0.5)];
        run(&mut s, &mut io, 256);
        io
    };
    let mut rev = a_vals;
    rev.reverse();
    assert_eq!(render(&a_vals), render(&rev));
}

#[test]
fn readouts_meter_drive_level_and_circuit() {
    let mut s = prepared();
    s.set_param("circuit", 5.0);
    let mut io = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut s, &mut io, 512);
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert!((r[0] - gain_to_db(0.5)).abs() < 0.1, "{r:?}");
    assert!(r[1].is_finite() && r[1] > -40.0, "{r:?}");
    assert!((r[2] - 0.5).abs() < 1e-3, "drive glided to its knob: {r:?}");
    assert!((r[3] - 0.25).abs() < 1e-3, "level² : {r:?}");
    assert_eq!(r[4], 5.0);
    assert!(r[5].is_nan());
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_dist");
    assert_eq!(presets.len(), 50);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &DIST_INFO).unwrap();
        assert_eq!(&p.name, name);
        for key in ["circuit", "drive", "tone"] {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
        assert!(!p.values.contains_key("level"), "{name}: no C++ preset stores a Level");
    }
    let guv = FsPreset::parse(presets.iter().find(|p| p.0 == "Guv'nor Plexi").unwrap().1, &DIST_INFO).unwrap();
    assert_eq!(guv.values["circuit"], 7.0);
}

#[test]
fn hostile_params_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
        for circuit in 0..CIRCUITS {
            let mut s = prepared();
            s.set_param("circuit", circuit as f32);
            // Bypass stays off: a hostile value that turned it on would hide the wet path.
            for info in DIST_PARAMS.iter().filter(|p| p.id != "circuit" && p.id != "bypass") {
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
fn hostile_choice_values_stay_finite() {
    // The circuit selector and every switch get garbage mid-stream.
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 99.0, 1e9, 0.49, 0.51] {
        let mut s = prepared();
        let mut io = vec![sine(1024, 0.8), sine(1024, 0.8)];
        run(&mut s, &mut io, 256);
        for info in DIST_PARAMS.iter().filter(|p| p.unit == crate::Unit::Choice) {
            assert!(s.set_param(info.id, bad));
            let mut io = vec![sine(512, 0.8), sine(512, 0.8)];
            run(&mut s, &mut io, 256);
            assert!(all_finite(&io), "{} = {bad}", info.id);
        }
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        for circuit in 0..CIRCUITS {
            let mut s = prepared();
            s.set_param("circuit", circuit as f32);
            s.set_param("drive", 1.0);
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
fn hostile_input_stays_finite() {
    for circuit in 0..CIRCUITS {
        let mut s = prepared();
        s.set_param("circuit", circuit as f32);
        s.set_param("trim", 1.0);
        s.set_param("drive", 1.0);
        let mut io = vec![vec![1e30f32; 1024], vec![-1e30f32; 1024]];
        io[0][100] = f32::NAN;
        io[1][200] = f32::INFINITY;
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "circuit {circuit}");
        let mut io = vec![sine(1024, 0.5), sine(1024, 0.5)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "circuit {circuit} after");
    }
}

#[test]
fn switching_circuit_and_switches_mid_stream_is_safe() {
    let mut s = prepared();
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = sine(128 * 400, 0.8);
    let switches: Vec<&ParamInfo> = DIST_PARAMS.iter().filter(|p| p.unit == crate::Unit::Choice && p.id != "circuit").collect();
    for blk in 0..400 {
        s.set_param("circuit", (blk % CIRCUITS) as f32);
        let sw = switches[blk % switches.len()];
        s.set_param(sw.id, (blk % (sw.max as usize + 1)) as f32);
        s.set_param("drive", (blk % 11) as f32 / 10.0);
        for b in &mut buf {
            b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
        }
        s.process(&mut buf, 128);
        assert!(all_finite(&buf), "block {blk}");
    }
}

#[test]
fn mono_and_wide_buffers() {
    let mut s = Dist::new();
    s.prepare(48_000.0, 512, 1);
    let mut mono = vec![sine(4096, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    let mut s = Dist::new();
    s.prepare(48_000.0, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(4096, 0.1 * (c + 1) as f32)).collect();
    let orig = wide.clone();
    s.set_param("drive", 1.0);
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
    let mut ragged = vec![vec![0.3f32; 10], vec![0.3f32; 3]];
    s.process(&mut ragged, 512);
}

#[test]
fn silence_tail_is_cheap() {
    let sr = 48_000usize;
    let mut cases = Vec::new();
    for circuit in 0..CIRCUITS {
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
        let tail = time(1, 4);
        let full = time(5, 0);
        cases.push((circuit, tail, full));
        assert!(tail < 4.0 * full, "circuit {circuit}: silence tail {tail:.3}s vs signal {full:.3}s");
    }
    println!("{cases:?}");
}

#[test]
fn process_does_not_allocate() {
    // No counting allocator in this crate: the engine owns no heap memory at all (every state is
    // a fixed-size array inside one boxed struct), so its address is the whole story; the host
    // buffers keep their pointers and capacities across 1000 blocks of switching.
    let mut s = prepared();
    let engine_ptr: *const DistEngineDsp = &*s.dsp;
    let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        s.set_param("circuit", (blk % CIRCUITS) as f32);
        s.set_param("tdPlate", (blk % 3) as f32);
        s.set_param("link", (blk % 2) as f32);
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, &*s.dsp as *const DistEngineDsp);
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is cleared
    // in place (same box), nothing grows.
    let mut s = prepared();
    s.set_param("circuit", 2.0);
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const DistEngineDsp = &*s.dsp;
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, &*s.dsp as *const DistEngineDsp);
        s.process(&mut io, 512);
    }
    assert!(all_finite(&io));
}

/// Every parameter at its default, then `circuit` and a few knobs away from theirs.
fn settle(s: &mut Dist, circuit: usize) {
    for p in DIST_PARAMS.iter() {
        s.set_param(p.id, p.default);
    }
    for (id, v) in [("circuit", circuit as f32), ("drive", 0.9), ("tone", 0.2), ("level", 0.3), ("trim", 0.6), ("tdPlate", 0.0)] {
        s.set_param(id, v);
    }
}

#[test]
fn a_reset_is_independent_of_what_played_before() {
    // Two modules with different histories (input, knob moves, circuit and switch flips), brought
    // to the same values and reset, render bit for bit the same, every circuit.
    for circuit in 0..CIRCUITS {
        let mut a = prepared();
        a.set_param("circuit", ((circuit + 3) % CIRCUITS) as f32);
        a.set_param("drive", 0.1);
        a.set_param("muffVer", 3.0);
        let mut io = vec![sine(4800, 0.8), sine(4800, -0.6)];
        run(&mut a, &mut io, 333);
        settle(&mut a, circuit);
        let mut io = vec![sine(77, 0.9), sine(77, 0.9)];
        run(&mut a, &mut io, 77);
        settle(&mut a, circuit);

        let mut b = prepared();
        b.set_param("level", 1.0);
        b.set_param("trim", 1.0);
        let mut io = vec![sine(3000, -0.2), sine(3000, 0.9)];
        run(&mut b, &mut io, 128);
        settle(&mut b, circuit);

        a.reset();
        b.reset();
        let (mut x, mut y) = (vec![sine(9600, 0.5), sine(9600, 0.4)], vec![sine(9600, 0.5), sine(9600, 0.4)]);
        run(&mut a, &mut x, 512);
        run(&mut b, &mut y, 256);
        assert!(x == y, "circuit {circuit}");
        assert!(all_finite(&x));
    }
}

#[test]
fn a_reset_does_not_glide() {
    // Drive, Tone, Level and Input sit at their knobs right after a reset (nothing glides from a
    // default), including values moved since the last block.
    let mut s = prepared();
    let mut io = vec![sine(4800, 0.5), sine(4800, 0.5)];
    run(&mut s, &mut io, 512);
    settle(&mut s, 4);
    s.reset();
    let (got, want) = s.dsp.smoothers();
    assert_eq!(got, want);
    // And the output level right after the reset is the settled one.
    let mut io = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut s, &mut io, 512);
    let settled = rms(&io[0][40_000..48_000]);
    s.reset();
    let mut io = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut s, &mut io, 512);
    assert!((rms(&io[0][0..8000]) / settled - 1.0).abs() < 0.02);
}

#[test]
fn a_reset_before_any_audio_is_a_fresh_instance() {
    for circuit in [0usize, 2, 3, 8, 11] {
        let mut fresh = prepared();
        settle(&mut fresh, circuit);
        let mut used = Dist::new();
        used.prepare(96_000.0, 512, 2);
        settle(&mut used, circuit);
        used.reset();
        used.prepare(48_000.0, 512, 2);
        used.reset();
        let (mut a, mut b) = (vec![sine(4800, 0.5), sine(4800, 0.4)], vec![sine(4800, 0.5), sine(4800, 0.4)]);
        run(&mut used, &mut a, 512);
        run(&mut fresh, &mut b, 512);
        assert!(a == b, "circuit {circuit}");
    }
}

#[test]
fn bypass_is_exactly_the_input() {
    let mut s = prepared();
    s.set_param("drive", 1.0);
    s.set_param("bypass", 1.0);
    let src = sine(4800, 0.6);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    assert_eq!(io[0], src, "in-place: bypass is a no-op on the buffer");
}

#[test]
fn every_rate_is_finite() {
    for sr in [22_050.0f32, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        for circuit in 0..CIRCUITS {
            let mut s = Dist::new();
            s.prepare(sr, 512, 2);
            s.set_param("circuit", circuit as f32);
            let mut io = vec![sine(2048, 0.7), sine(2048, 0.7)];
            run(&mut s, &mut io, 512);
            assert!(all_finite(&io), "{sr} Hz circuit {circuit}");
        }
    }
}

// ---------------------------------------------------------------- fidelity cases and the wrapper

/// What the C++ render tool leaves in the processor's parameters: a fresh instance, the case's
/// `values` set and settled (the message loop pumped: knob swap, then Link), then its `after`
/// values set and settled.
fn rendered_state(values: &[(String, f32)], after: &[(String, f32)]) -> std::collections::BTreeMap<String, f32> {
    use crate::derive::{Derive, ParamEdit};
    let mut state: std::collections::BTreeMap<String, f32> = DIST_PARAMS.iter().map(|p| (p.id.to_string(), p.default)).collect();
    let mut memory = std::collections::BTreeMap::new();
    for stage in [values, after] {
        if stage.is_empty() {
            continue;
        }
        let before = state.clone();
        for (k, v) in stage {
            state.insert(k.clone(), DIST_INFO.param(k).unwrap().clamp(*v));
        }
        let changed: Vec<String> = stage.iter().map(|(k, _)| k.clone()).collect();
        let mut d = Derive::new(&DIST_INFO, &before, &mut state, &mut memory, &changed, ParamEdit::Edit, &|_| false);
        derive(&mut d);
    }
    state
}

fn case_values(case: &serde_json::Value, key: &str) -> Vec<(String, f32)> {
    case.get(key).and_then(|v| v.as_object()).map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap() as f32)).collect()).unwrap_or_default()
}

#[test]
fn fidelity_cases_are_what_the_wrapper_leaves() {
    // The fidelity test hands a case's values straight to the engine, while the C++ render tool
    // goes through the processor, whose knob memory and Link move Drive/Tone/Level. Each case
    // must therefore already be the state the wrapper leaves, or the two would render different
    // settings: checked here against the ported wrapper.
    let cases: serde_json::Value = serde_json::from_str(include_str!("../../../../tests/fidelity/dist/cases.json")).unwrap();
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 80, "{} cases", cases.len());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let (values, after) = (case_values(case, "values"), case_values(case, "after"));
        let got = rendered_state(&values, &after);
        let mut want: std::collections::BTreeMap<String, f32> = DIST_PARAMS.iter().map(|p| (p.id.to_string(), p.default)).collect();
        for (k, v) in values.iter().chain(&after) {
            want.insert(k.clone(), *v);
        }
        assert_eq!(got, want, "{name}");
    }
}

/// Prints `tests/fidelity/dist/cases.json` (run with `--ignored --nocapture`): each case's
/// intent, completed with the Drive/Tone/Level the wrapper settles on.
#[test]
#[ignore]
fn print_fidelity_cases() {
    use serde_json::{Map, Value, json};
    let num = |v: f32| json!(f64::from(v));
    let inputs = ["song.wav", "guitar.wav", "drums.wav", "rhodes.wav", "piano.wav", "synthetic.wav"];
    let mut cases: Vec<Value> = Vec::new();
    let defaults = |k: &str| DIST_INFO.param(k).unwrap().default;
    // `set` then `after`, completed with whatever the wrapper moved at each stage.
    let mut push = |name: String, input: &str, sr: u32, set: Vec<(&str, f32)>, after: Vec<(&str, f32)>| {
        let set: Vec<(String, f32)> = set.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        let after: Vec<(String, f32)> = after.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        let mid = rendered_state(&set, &[]);
        let fin = rendered_state(&set, &after);
        let in_after = |k: &str| after.iter().any(|(s, _)| s == k);
        let mut values = Map::new();
        for (k, v) in &set {
            values.insert(k.clone(), num(*v));
        }
        // the first stage's swap recall / Link
        for (k, v) in &mid {
            if *v != defaults(k) && !set.iter().any(|(s, _)| s == k) && !in_after(k) {
                values.insert(k.clone(), num(*v));
            }
        }
        let mut after_map = Map::new();
        for (k, v) in &after {
            after_map.insert(k.clone(), num(*v));
        }
        // the second stage's Link
        for (k, v) in &fin {
            if mid.get(k) != Some(v) && !in_after(k) {
                after_map.insert(k.clone(), num(*v));
            }
        }
        let mut c = json!({"name": name, "input": input, "sr": sr, "block": 512, "values": Value::Object(values)});
        if !after_map.is_empty() {
            c["after"] = Value::Object(after_map);
        }
        cases.push(c);
    };
    let mut n = 0usize;
    let mut next_input = move || {
        n += 1;
        inputs[n % inputs.len()]
    };

    push("default".into(), "song.wav", 48_000, vec![], vec![]);
    push("default_44k".into(), "guitar.wav", 44_100, vec![], vec![]);
    push("default_96k".into(), "drums.wav", 96_000, vec![], vec![]);
    let circuit_names = DIST_INFO.param("circuit").unwrap().choices;
    let slug = |s: &str| s.to_lowercase().replace(['\u{27}', '+'], "").replace([' ', '-'], "_").replace("__", "_");
    // Each circuit as selected (its start point), and driven hard.
    for (i, cname) in circuit_names.iter().enumerate() {
        push(slug(cname), next_input(), 48_000, vec![("circuit", i as f32)], vec![]);
        push(format!("{}_hot", slug(cname)), next_input(), 48_000, vec![("circuit", i as f32)], vec![("drive", 0.95), ("tone", 0.8)]);
    }
    // Every switch position other than the default, on its own circuit.
    for (ci, c) in wrapper::CIRCUITS.iter().enumerate() {
        for (sw, _) in c.switches {
            let p = DIST_INFO.param(sw).unwrap();
            for opt in 0..p.choices.len() {
                if opt as f32 == p.default {
                    continue;
                }
                push(
                    format!("{}_{}_{}", slug(c.name), slug(sw), slug(p.choices[opt])),
                    next_input(),
                    48_000,
                    vec![("circuit", ci as f32), (sw, opt as f32)],
                    vec![("drive", 0.75)],
                );
            }
        }
    }
    // Input trim, the extremes, Link.
    push("trim_hot_klon".into(), "guitar.wav", 48_000, vec![("circuit", 5.0), ("trim", 1.0)], vec![]);
    push("trim_low_muff".into(), "drums.wav", 48_000, vec![("circuit", 2.0), ("trim", 0.0)], vec![]);
    push("ts_drive_0".into(), "synthetic.wav", 48_000, vec![("drive", 0.0), ("tone", 0.0), ("level", 1.0)], vec![]);
    push("ts_drive_1".into(), "synthetic.wav", 48_000, vec![("drive", 1.0), ("tone", 1.0), ("level", 0.1)], vec![]);
    push("link_ts".into(), "song.wav", 48_000, vec![("link", 1.0), ("drive", 0.8)], vec![]);
    push("link_rat_swap".into(), "guitar.wav", 48_000, vec![("link", 1.0), ("circuit", 1.0)], vec![]);
    push("link_muff_after".into(), "rhodes.wav", 48_000, vec![("link", 1.0), ("circuit", 2.0)], vec![("drive", 0.3)]);
    push(
        "link_ocd_anchors".into(),
        "piano.wav",
        48_000,
        vec![("link", 1.0), ("ocdLo", 0.9), ("ocdHi", 0.2), ("circuit", 11.0)],
        vec![("drive", 0.6)],
    );
    push("bypass".into(), "piano.wav", 48_000, vec![("bypass", 1.0), ("drive", 1.0)], vec![]);
    push("fuzz_face_96k".into(), "guitar.wav", 96_000, vec![("circuit", 3.0)], vec![]);
    push("tube_driver_250v_44k".into(), "song.wav", 44_100, vec![("circuit", 8.0), ("tdPlate", 2.0)], vec![]);
    // The first 20 factory presets by file name: circuit and switches, then Drive/Tone and the
    // matched Level the editor's preset load gives (`applyMatchedLevel`).
    for (name, json) in factory_presets("fs_dist").iter().take(20) {
        let p = FsPreset::parse(json, &DIST_INFO).unwrap();
        let circuit = p.values["circuit"];
        let (drive, tone) = (p.values["drive"], p.values["tone"]);
        let set: Vec<(&str, f32)> = p.values.iter().filter(|(k, _)| !matches!(k.as_str(), "drive" | "tone")).map(|(k, v)| (k.as_str(), *v)).collect();
        let level = wrapper::link_level_for(defaults, circuit.round() as i32, drive);
        push(format!("preset_{}", slug(name)), next_input(), 48_000, set, vec![("drive", drive), ("tone", tone), ("level", level)]);
    }
    let text: Vec<String> = cases.iter().map(|c| format!("  {}", serde_json::to_string(c).unwrap())).collect();
    println!("CASES-BEGIN\n[\n{}\n]\nCASES-END", text.join(",\n"));
}

// ---------------------------------------------------------------- Link under host automation

fn block(s: &mut Dist) {
    let mut io = vec![sine(64, 0.3), sine(64, 0.3)];
    s.process(&mut io, 64);
}

#[test]
fn automated_drive_moves_level_as_the_command_path_does() {
    let mut s = prepared();
    s.set_param("link", 1.0);
    s.set_param("level", rendered_state(&[("link".into(), 1.0)], &[])["level"]);
    block(&mut s);
    // The host now sends only Drive (an automation lane), block after block.
    for drive in [0.9f32, 0.2, 0.65] {
        s.set_param("drive", drive);
        block(&mut s);
        let want = rendered_state(&[("link".into(), 1.0)], &[("drive".into(), drive)])["level"];
        assert_eq!(s.param("level"), Some(want), "drive {drive}");
        assert!(s.dsp.level_smoothed().is_finite());
    }
    // Link off: Level stays.
    s.set_param("link", 0.0);
    s.set_param("drive", 0.1);
    block(&mut s);
    assert_eq!(s.param("level"), Some(rendered_state(&[("link".into(), 1.0)], &[("drive".into(), 0.65)])["level"]));
}

#[test]
fn a_level_lane_or_a_host_level_wins_over_link() {
    let mut s = prepared();
    s.set_param("link", 1.0);
    block(&mut s);
    s.set_param_automated("level", true);
    s.set_param("level", 0.2);
    block(&mut s);
    s.set_param("drive", 0.95);
    block(&mut s);
    assert_eq!(s.param("level"), Some(0.2), "the Level lane wins");
    s.set_param_automated("level", false);
    s.set_param("drive", 0.9);
    s.set_param("level", 0.33);
    block(&mut s);
    assert_eq!(s.param("level"), Some(0.33), "a Level sent with the Drive wins that block");
    s.set_param("drive", 0.8);
    block(&mut s);
    assert_ne!(s.param("level"), Some(0.33), "then Link follows Drive again");
}

#[test]
fn link_following_does_not_allocate() {
    let mut s = prepared();
    s.set_param("link", 1.0);
    let engine_ptr: *const DistEngineDsp = &*s.dsp;
    let mut io = vec![sine(128, 0.5), sine(128, 0.5)];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..500 {
        s.set_param("drive", (blk % 11) as f32 / 10.0);
        s.set_param_automated("level", blk % 7 == 0);
        s.process(&mut io, 128);
    }
    assert_eq!(engine_ptr, &*s.dsp as *const DistEngineDsp);
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn a_reset_does_not_move_a_stored_level_off_links_curve() {
    // A Level the session stores (here set by hand after Link went on, which the C++ also keeps
    // until a listened parameter moves) is what plays after a seek: the full push that follows a
    // reset or a prepare does not re-run Link when no lane drives a listened parameter.
    let mut s = prepared();
    s.set_param("link", 1.0);
    block(&mut s);
    s.set_param("level", 0.42);
    block(&mut s);
    assert_eq!(s.param("level"), Some(0.42));
    for _ in 0..2 {
        s.reset();
        block(&mut s);
        assert_eq!(s.param("level"), Some(0.42), "a reset keeps the stored Level");
    }
    s.prepare(48_000.0, 512, 2);
    block(&mut s);
    assert_eq!(s.param("level"), Some(0.42), "so does a re-prepare");
    // A fresh module whose session stores an off-curve Level plays it too.
    let mut f = Dist::new();
    f.set_param("link", 1.0);
    f.set_param("level", 0.42);
    f.prepare(48_000.0, 512, 2);
    block(&mut f);
    assert_eq!(f.param("level"), Some(0.42));
}

#[test]
fn a_reset_with_a_laned_drive_follows_the_lane() {
    // With a lane on Drive, Level follows the lane's value at the new position after a seek.
    let mut s = prepared();
    s.set_param("link", 1.0);
    s.set_param("level", 0.42);
    s.set_param_automated("drive", true);
    block(&mut s);
    s.reset();
    block(&mut s);
    let want = rendered_state(&[("link".into(), 1.0)], &[("drive".into(), 0.5)])["level"];
    assert_eq!(s.param("level"), Some(want), "the full push re-runs Link under a listened lane");
    // The lane moves Drive as the transport jumps: Level follows from the first block.
    s.set_param("drive", 0.85);
    s.reset();
    block(&mut s);
    let want = rendered_state(&[("link".into(), 1.0)], &[("drive".into(), 0.85)])["level"];
    assert_eq!(s.param("level"), Some(want));
    // A laned Level still wins.
    s.set_param_automated("level", true);
    s.set_param("level", 0.1);
    s.reset();
    block(&mut s);
    assert_eq!(s.param("level"), Some(0.1));
}
