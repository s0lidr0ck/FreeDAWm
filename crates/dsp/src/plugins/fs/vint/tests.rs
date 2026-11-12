use super::*;
use crate::Unit;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

const CIRCUITS: usize = 8;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
}

fn prepared() -> Vint {
    let mut s = Vint::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut Vint, io: &mut [Vec<f32>], block: usize) {
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

/// A strong setting of each circuit, so the wet path is clearly in use.
fn strong(s: &mut Vint, circuit: usize) {
    s.set_param("circuit", circuit as f32);
    let sets: &[(&str, f32)] = match circuit {
        0 => &[("pasLowBoost", 1.0), ("pasLowAtten", 0.6), ("pasHighBoost", 0.8), ("pasHighAtten", 0.5)],
        1 => &[("conLowGain", 1.0), ("conMidGain", 0.9), ("conHighGain", 0.9), ("conHpf", 2.0), ("conPre", 0.0)],
        2 => &[("proLowGain", 10.0), ("proMidGain", 0.0), ("proHighGain", 10.0), ("proFilter", 1.0)],
        3 => &[("briLowGain", 1.0), ("briHmGain", 0.9), ("briHighGain", 0.0), ("briHpf", 0.4), ("briLpf", 0.4), ("briBlack", 1.0)],
        4 => &[("enhAmount", 1.0), ("enhLow", 1.0)],
        5 => &[("airAmount", 1.0), ("airBands", 0.0)],
        6 => &[("smpBits", 0.0), ("smpDrive", 1.0), ("smpMachine", 2.0), ("smpRate", 0.2)],
        _ => &[("pasMidLowPeak", 1.0), ("pasMidDip", 1.0), ("pasMidHighPeak", 1.0)],
    };
    for (id, v) in sets {
        assert!(s.set_param(id, *v), "{id}");
    }
    s.set_param("drive", 1.0);
}

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = VINT_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), 64, "Parameters.h has 64 entries (conPre was appended last)");
    assert_eq!(&ids[..7], ["circuit", "stage", "drive", "mix", "output", "quality", "bypass"]);
    assert_eq!(ids[dsp::P_PAS_MID_HIGH_PEAK], "pasMidHighPeak");
    assert_eq!(ids[dsp::P_CON_HIGH_GAIN], "conHighGain");
    assert_eq!(ids[dsp::P_PRO_FILTER], "proFilter");
    assert_eq!(ids[dsp::P_BRI_BLACK], "briBlack");
    assert_eq!(ids[dsp::P_ENH_LOW], "enhLow");
    assert_eq!(ids[dsp::P_AIR_REF], "airRef");
    assert_eq!(ids[dsp::P_SMP_FILTER], "smpFilter");
    assert_eq!(ids[dsp::P_CON_PRE], "conPre");
    let defaults: Vec<f32> = VINT_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(defaults, dsp::DEFAULTS.to_vec(), "Parameters.h's defaults are defaultOf's");
    for p in &VINT_PARAMS {
        let want = match p.id {
            "circuit" | "stage" | "drive" | "mix" | "output" | "quality" => "Global",
            "bypass" => "",
            id if id.starts_with("pas") => "Passive",
            id if id.starts_with("con") => "Console",
            id if id.starts_with("pro") => "Proportional",
            id if id.starts_with("bri") => "British",
            id if id.starts_with("enh") => "Enhancer",
            id if id.starts_with("air") => "Air",
            _ => "Sampler",
        };
        assert_eq!(p.group, want, "{}", p.id);
        if p.unit == Unit::Choice {
            assert!(!p.choices.is_empty() && p.default < p.choices.len() as f32, "{}", p.id);
        }
    }
    assert_eq!(VINT_PARAMS[dsp::P_CON_PRE].choices.len(), 22);
    assert_eq!(VINT_PARAMS[dsp::P_DRIVE].format(0.25), "+0.0 dB");
    assert_eq!(VINT_PARAMS[dsp::P_OUTPUT].format(1.0), "+12.0 dB");
    assert_eq!(VINT_PARAMS[dsp::P_PAS_LOW_BOOST].format(0.5), "5.0");
    assert_eq!(VINT_PARAMS[dsp::P_CON_MID_GAIN].format(1.0), "+18.0 dB");
    assert_eq!(VINT_PARAMS[dsp::P_BRI_HPF].format(0.0), "Out");
    assert_eq!(VINT_PARAMS[dsp::P_BRI_HM_FREQ].format(0.5), "3.00 kHz");
    assert_eq!(VINT_PARAMS[dsp::P_BRI_LM_Q].format(0.25), "Q 1.00");
    assert_eq!(VINT_PARAMS[dsp::P_AIR_REF].format(0.5), "-16.0 dBFS");
    assert_eq!(VINT_PARAMS[dsp::P_SMP_BITS].format(0.5), "12.0 bits");
    assert_eq!(VINT_PARAMS[dsp::P_SMP_RATE].format(1.0), "48.00 kHz");
}

#[test]
fn registered_and_reports_no_latency() {
    let p = crate::create("fs_vint").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    assert_eq!(p.latency(), 0, "the C++ reports no latency");
}

#[test]
fn the_vu_takes_the_loudest_block_since_the_editor_last_looked() {
    // `outLevel`: raised to each block's RMS, `exchange (0)` on the editor's 30 Hz tick.
    let mut s = prepared();
    let mut loud = vec![sine(512, 0.5), sine(512, 0.5)];
    run(&mut s, &mut loud, 512);
    let mut quiet = vec![sine(512, 0.05), sine(512, 0.05)];
    run(&mut s, &mut quiet, 512);
    let live = s.analysis().unwrap();
    let got = live.live.take(LIVE_OUT_LEVEL);
    assert!((got - 0.5 / 2f32.sqrt()).abs() < 0.03, "the loud block, not the last: {got}");
    assert_eq!(live.live.take(LIVE_OUT_LEVEL), 0.0, "taken");
}

#[test]
fn readouts_meter_and_drive() {
    let mut s = prepared();
    let mut io = vec![sine(4800, 0.5), sine(4800, 0.5)];
    run(&mut s, &mut io, 512);
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert!((r[0] - gain_to_db(0.5)).abs() < 0.1, "{r:?}");
    assert!(r[1].is_finite() && (r[1] - r[0]).abs() < 1.0, "the default Passive is flat: {r:?}");
    assert_eq!(r[2], 0.0);
    s.set_param("drive", 1.0);
    s.readouts(&mut r);
    assert_eq!(r[2], 18.0);
    // The last block's output RMS (the VU's `outLevel`): a 0.5 sine through a flat Passive.
    assert!((r[3] - 0.5 / 2f32.sqrt()).abs() < 0.03, "{r:?}");
    assert!(r[4].is_nan());
    s.reset();
    s.readouts(&mut r);
    assert_eq!(r[3], 0.0, "a reset clears it");
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_vint");
    assert_eq!(presets.len(), 39);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &VINT_INFO).unwrap();
        assert_eq!(&p.name, name);
        // Every key a preset carries is a parameter, and every choice label resolves.
        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        for key in raw["values"].as_object().unwrap().keys() {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
    }
}

#[test]
fn hostile_params_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
        for circuit in 0..CIRCUITS {
            let mut s = prepared();
            s.set_param("circuit", circuit as f32);
            // Bypass stays off: a hostile value that turned it on would hide the wet path.
            for info in VINT_PARAMS.iter().filter(|p| p.id != "circuit" && p.id != "bypass") {
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
    // Every choice and switch, alone, at values a host should never send (between steps, past the
    // ends, non-finite), on the circuit it belongs to.
    let circuit_of = |id: &str| match id {
        id if id.starts_with("pasMid") => 7.0,
        id if id.starts_with("con") => 1.0,
        id if id.starts_with("pro") => 2.0,
        id if id.starts_with("bri") => 3.0,
        id if id.starts_with("enh") => 4.0,
        id if id.starts_with("air") => 5.0,
        id if id.starts_with("smp") => 6.0,
        _ => 0.0,
    };
    for info in VINT_PARAMS.iter().filter(|p| matches!(p.unit, Unit::Choice | Unit::Toggle) && p.id != "bypass") {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.4999, 0.5, 1.5, 2.5, 6.5, 21.4, 99.0, 1e9] {
            let mut s = prepared();
            if info.id != "circuit" {
                s.set_param("circuit", circuit_of(info.id));
            }
            assert!(s.set_param(info.id, bad));
            let mut io = vec![sine(1536, 0.9), sine(1536, -0.9)];
            run(&mut s, &mut io, 512);
            assert!(all_finite(&io), "{} = {bad}", info.id);
        }
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        for circuit in 0..CIRCUITS {
            let mut s = prepared();
            strong(&mut s, circuit);
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
    let src = sine(128 * 400, 0.8);
    for blk in 0..400 {
        // Every circuit after every other (the Passive after a staged circuit takes the
        // transformers' path while the stage fades), at every quality.
        s.set_param("circuit", ((blk * 3) % CIRCUITS) as f32);
        s.set_param("quality", ((blk / 8) % 3) as f32);
        s.set_param("stage", ((blk / 5) % 2) as f32);
        s.set_param("conPre", ((blk * 7) % 22) as f32);
        s.set_param("briBlack", (blk % 2) as f32);
        s.set_param("smpMachine", (blk % 3) as f32);
        s.set_param("enhType", (blk % 3) as f32);
        s.set_param("drive", (blk % 10) as f32 / 9.0);
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
    let mut s = Vint::new();
    s.prepare(48_000.0, 512, 1);
    strong(&mut s, 0);
    let mut mono = vec![sine(4096, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    // Six channels: the first two processed, the rest untouched.
    let mut s = Vint::new();
    s.prepare(48_000.0, 512, 6);
    strong(&mut s, 3);
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
    let mut empty = vec![Vec::new(), Vec::new()];
    s.process(&mut empty, 64);
    let mut ragged = vec![vec![0.1f32; 100], vec![0.1f32; 7]];
    s.process(&mut ragged, 100);
    assert!(all_finite(&ragged));
}

#[test]
fn silence_tail_is_cheap() {
    let sr = 48_000usize;
    let mut cases = Vec::new();
    for circuit in 0..CIRCUITS {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            strong(&mut s, circuit);
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
    // No counting allocator in this crate: every heap block the engine owns (its channels, the
    // solve scratch, each solver's inverse) is allocated at construction; their addresses and the
    // host buffers' pointers and capacities stay put across 1000 blocks of switching.
    let mut s = prepared();
    let engine_ptr: *const VintEngineDsp = &*s.dsp;
    let blocks = s.dsp.heap_blocks();
    let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        s.set_param("quality", (blk % 3) as f32);
        s.set_param("circuit", ((blk / 3) % CIRCUITS) as f32);
        s.set_param("briLmGain", (blk % 7) as f32 / 6.0);
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, &*s.dsp as *const VintEngineDsp);
    assert_eq!(blocks, s.dsp.heap_blocks());
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is
    // re-initialised in place, every heap block it owns kept (same addresses, same capacities).
    let mut s = prepared();
    strong(&mut s, 3);
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const VintEngineDsp = &*s.dsp;
    let blocks = s.dsp.heap_blocks();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, &*s.dsp as *const VintEngineDsp);
        assert_eq!(blocks, s.dsp.heap_blocks());
        s.process(&mut io, 512);
    }
    assert!(all_finite(&io));
}

#[test]
fn bypass_lands_exactly_on_the_input() {
    let mut s = prepared();
    strong(&mut s, 1);
    s.set_param("bypass", 1.0);
    let src = sine(48_000, 0.6);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    // 15 ms glide, then exactly the input.
    assert_eq!(io[0][24_000..], src[24_000..]);
}

#[test]
fn every_circuit_alone_changes_the_sound() {
    for circuit in 0..CIRCUITS {
        let mut s = prepared();
        strong(&mut s, circuit);
        let src = sine(9600, 0.3);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        let diff: f32 = io[0][4800..].iter().zip(&src[4800..]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(diff > 1e-3, "circuit {circuit}: {diff}");
    }
}

#[test]
fn frequency_response_shows_the_curve() {
    let mut v: Vec<f32> = VINT_PARAMS.iter().map(|p| p.default).collect();
    let freqs = [100.0, 1000.0, 12000.0];
    let mut out = [0.0f32; 3];
    frequency_response(&v, 48_000.0, &freqs, &mut out);
    for (f, db) in freqs.iter().zip(out) {
        assert!(db.abs() < 0.3, "the default Passive is flat: {f} Hz {db} dB");
    }
    v[dsp::P_PAS_HIGH_BOOST] = 1.0;
    v[dsp::P_PAS_HIGH_FREQ] = 5.0; // 12k
    frequency_response(&v, 48_000.0, &freqs, &mut out);
    assert!(out[2] > 8.0 && out[0].abs() < 1.0, "{out:?}");
    // Hostile inputs: no panic, finite.
    let mut junk = [0.0f32; 4];
    frequency_response(&[f32::NAN; 3], f32::NAN, &[f32::NAN, -5.0, 1e9, 20.0], &mut junk);
    assert!(junk.iter().all(|d| d.is_finite()));
}

#[test]
fn frequency_response_ignores_the_startup_thump() {
    // A fresh Console with its stage on puts out about -57 dBFS with no input at all (the engine's
    // real behaviour); read against the impulse it would pose as low-frequency gain. Measured
    // against silence, the Console with every band off is flat at the low end.
    let mut v: Vec<f32> = VINT_PARAMS.iter().map(|p| p.default).collect();
    v[dsp::P_CIRCUIT] = 1.0;
    v[dsp::P_CON_LOW_FREQ] = 0.0;
    let freqs = [20.0, 50.0, 100.0];
    let mut out = [0.0f32; 3];
    frequency_response(&v, 48_000.0, &freqs, &mut out);
    for (f, db) in freqs.iter().zip(out) {
        assert!(db.abs() < 1.0, "the Console with its bands off is flat: {f} Hz {db} dB");
    }
    // The 80 Hz high-pass rolls the low end off, monotonically, with no notch above it.
    v[dsp::P_CON_HPF] = 2.0;
    let freqs = [30.0, 50.0, 80.0, 120.0, 200.0, 400.0];
    let mut out = [0.0f32; 6];
    frequency_response(&v, 48_000.0, &freqs, &mut out);
    assert!(out[0] < out[5] - 12.0, "30 Hz well below 400 Hz: {out:?}");
    assert!(out.windows(2).all(|w| w[0] <= w[1] + 0.3), "no notch: {out:?}");
    assert!(out[4].abs() < 1.5 && out[5].abs() < 1.0, "flat above: {out:?}");
}

#[test]
fn prepare_twice_is_prepare_once() {
    // `soundcraft_dsp::create` prepares at 48 kHz and the host prepares again; the C++ host calls
    // `prepareToPlay` once on a fresh processor. A second prepare must not change the sound (in
    // the engine each `prepare` advances the 32-sample re-solve cadence of gliding sections).
    let render = |twice: bool| {
        let mut s = Vint::new();
        s.prepare(48_000.0, 512, 2);
        if twice {
            s.prepare(48_000.0, 512, 2);
        }
        s.set_param("circuit", 3.0);
        s.set_param("briHmGain", 0.9);
        let mut io = vec![sine(4096, 0.5), sine(4096, 0.5)];
        run(&mut s, &mut io, 512);
        io
    };
    assert_eq!(render(false), render(true));
}

#[test]
fn reset_is_a_fresh_prepare() {
    // A gliding British knob, where a non-idempotent re-prepare of the same engine would show.
    let set = |s: &mut Vint| {
        strong(s, 3);
        s.set_param("briHmGain", 0.7);
    };
    let src = [sine(4096, 0.5), sine(4096, -0.4)];
    let mut used = prepared();
    set(&mut used);
    let mut io = vec![src[0].clone(), src[1].clone()];
    run(&mut used, &mut io, 512);
    used.reset();
    // A knob turned after the reset glides; its sections re-solve on the C++ cadence only if the
    // reset was a fresh prepare.
    used.set_param("briHmGain", 0.2);
    let mut after_reset = vec![src[0].clone(), src[1].clone()];
    run(&mut used, &mut after_reset, 512);

    let mut fresh = Vint::new();
    set(&mut fresh);
    fresh.prepare(48_000.0, 512, 2);
    fresh.set_param("briHmGain", 0.2);
    let mut want = vec![src[0].clone(), src[1].clone()];
    run(&mut fresh, &mut want, 512);
    assert_eq!(after_reset, want);
}

#[test]
fn the_maximizer_is_the_882s_circuit() {
    // Through the whole engine (stage off): the circuit's own figures, `vint_maximizer`'s.
    let mut v: Vec<f32> = VINT_PARAMS.iter().map(|p| p.default).collect();
    v[dsp::P_CIRCUIT] = 4.0;
    v[dsp::P_STAGE] = 0.0;
    v[dsp::P_ENH_TYPE] = 1.0;
    let freqs = [50.0, 700.0, 5000.0];
    let mut out = [0.0f32; 3];
    // PROCESS at maximum: +13.5 dB at 5 kHz, the lows where they were.
    v[dsp::P_ENH_AMOUNT] = 1.0;
    frequency_response(&v, 48_000.0, &freqs, &mut out);
    assert!((out[2] - 13.5).abs() < 0.5 && (out[0] + 0.7).abs() < 0.5, "{out:?}");
    // LO CONTOUR at maximum: +14.6 dB at 50 Hz, the highs within a dB of where they were.
    v[dsp::P_ENH_AMOUNT] = 0.0;
    v[dsp::P_ENH_LOW] = 1.0;
    frequency_response(&v, 48_000.0, &freqs, &mut out);
    assert!((out[0] - 14.6).abs() < 0.5 && (out[2] + 1.1).abs() < 0.5, "{out:?}");
    // Both at minimum is the input itself (the unit's switch at OUT).
    v[dsp::P_ENH_LOW] = 0.0;
    frequency_response(&v, 48_000.0, &freqs, &mut out);
    assert!(out.iter().all(|d| d.abs() < 0.05), "{out:?}");
}

#[test]
fn the_maximizer_closes_its_highs_on_faint_signals() {
    // PROCESS at maximum, a 5 kHz tone: +13.5 dB on programme; under the 882's sensitivity
    // (-45 dBu: -63 dBFS) the VCA takes the boost away.
    let gain_at = |amp: f32| {
        let mut s = prepared();
        s.set_param("circuit", 4.0);
        s.set_param("stage", 0.0);
        s.set_param("enhType", 1.0);
        s.set_param("enhAmount", 1.0);
        let src: Vec<f32> = (0..96_000).map(|i| amp * (std::f32::consts::TAU * 5000.0 * i as f32 / 48_000.0).sin()).collect();
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        let peak = io[0][72_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        20.0 * (peak / amp).log10()
    };
    let (programme, faint) = (gain_at(0.1), gain_at(1.0e-5));
    assert!((programme - 13.5).abs() < 0.5, "{programme}");
    assert!(faint.abs() < 1.5, "{faint}");
}
