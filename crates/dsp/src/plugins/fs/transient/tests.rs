use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

const SR: usize = 48_000;

fn sine(len: usize, freq: f32, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR as f32).sin()).collect()
}

/// A drum-ish test signal: a decaying 180 Hz tone with a noisy click on top, every 250 ms.
fn hits(len: usize, amp: f32) -> Vec<f32> {
    let mut seed = 0x1234_5678u32;
    (0..len)
        .map(|i| {
            let t = (i % (SR / 4)) as f32 / SR as f32;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            let body = (std::f32::consts::TAU * 180.0 * t).sin() * (-t * 12.0).exp();
            let click = noise * (-t * 600.0).exp();
            amp * (0.7 * body + 0.5 * click)
        })
        .collect()
}

fn prepared() -> Transient {
    let mut s = Transient::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut Transient, io: &mut [Vec<f32>], block: usize) {
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

/// [`run`], latency-compensated: the latency's worth of zeros fed on and dropped off the front, so
/// the output lines up with the input it is the shaping of.
fn run_aligned(s: &mut Transient, io: &mut [Vec<f32>], block: usize) {
    let lat = s.latency();
    for c in io.iter_mut() {
        c.resize(c.len() + lat, 0.0);
    }
    run(s, io, block);
    for c in io.iter_mut() {
        c.drain(..lat);
    }
}

/// `x` delayed by `lat` samples (zeros first), same length.
fn delayed(x: &[f32], lat: usize) -> Vec<f32> {
    (0..x.len()).map(|i| if i < lat { 0.0 } else { x[i - lat] }).collect()
}

fn all_finite(io: &[Vec<f32>]) -> bool {
    io.iter().flatten().all(|v| v.is_finite())
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Both amounts well off zero, so the shaping (and both Focus filters) run.
fn shaping(s: &mut Transient) {
    s.set_param("attack", 0.9);
    s.set_param("sustain", 0.15);
}

const HOSTILE: [f32; 9] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0];
const CHOICES: [&str; 3] = ["source", "attackFocus", "sustainFocus"];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = TRANSIENT_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(ids, ["source", "attack", "sustain", "attackTime", "sustainTime", "attackFocus", "sustainFocus", "mix", "output", "bypass"]);
    let names: Vec<&str> = TRANSIENT_PARAMS.iter().map(|p| p.name).collect();
    assert_eq!(names, ["Source", "Attack", "Sustain", "Attack Time", "Sustain Time", "Attack Focus", "Sustain Focus", "Mix", "Output", "Bypass"]);
    let defaults: Vec<f32> = TRANSIENT_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(defaults, [0.0, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 1.0, 0.5, 0.0]);
    assert_eq!(TRANSIENT_PARAMS[d::P_SOURCE].choices, SOURCE_NAMES);
    assert_eq!(TRANSIENT_PARAMS[d::P_ATTACK_FOCUS].choices, FOCUS_NAMES);
    assert_eq!(TRANSIENT_PARAMS[d::P_SUSTAIN_FOCUS].choices, FOCUS_NAMES);
    assert_eq!(TRANSIENT_PARAMS[d::P_BYPASS].group, "", "the engine's bypass is hidden");
    let names: Vec<&str> = transient_config::SOURCES.iter().map(|s| s.name).collect();
    assert_eq!(names, SOURCE_NAMES, "TransientConfig.h names the Parameters.h choices");
    // The editor's formatters, through the engine's own maps.
    assert_eq!(TRANSIENT_PARAMS[d::P_ATTACK].format(0.5), "0 dB");
    assert_eq!(TRANSIENT_PARAMS[d::P_ATTACK].format(1.0), "+18.0 dB");
    assert_eq!(TRANSIENT_PARAMS[d::P_ATTACK].format(0.25), "-9.0 dB");
    assert_eq!(TRANSIENT_PARAMS[d::P_SUSTAIN].format(0.0), "-24.0 dB");
    assert_eq!(TRANSIENT_PARAMS[d::P_SUSTAIN_TIME].format(0.5), "Source");
    assert_eq!(TRANSIENT_PARAMS[d::P_ATTACK_TIME].format(1.0), "x4.0");
    assert_eq!(TRANSIENT_PARAMS[d::P_ATTACK_TIME].format(0.0), "x0.25");
    assert_eq!(TRANSIENT_PARAMS[d::P_MIX].format(1.0), "100 %");
    assert_eq!(TRANSIENT_PARAMS[d::P_OUTPUT].format(0.5), "0.0 dB");
    assert_eq!(TRANSIENT_PARAMS[d::P_OUTPUT].format(0.0), "-12.0 dB");
    assert_eq!(signed_db(-0.04), "0.0 dB");
}

#[test]
fn unit_maps_and_source_table() {
    assert_eq!(d::knob_to_attack_db(0.5), 0.0);
    assert_eq!(d::knob_to_sustain_db(1.0), 24.0);
    assert_eq!(d::knob_to_time_scale(0.5), 1.0);
    assert_eq!(d::knob_to_time_scale(0.0), 0.25);
    assert_eq!(d::knob_to_out_db(1.0), 12.0);
    assert_eq!(d::attack_knob_from_db(18.0), 1.0);
    assert_eq!(d::sustain_knob_from_db(-100.0), 0.0);
    // v2's rows, in the struct's member order (bands, level, gain, fast, slowAtt, hold, memory,
    // attFull, susFull). Piano is the owner-approved prototype's configuration exactly.
    let piano = d::spec(d::S_PIANO);
    assert_eq!(
        *piano,
        d::SourceSpec {
            det_low_hz: 80.0,
            det_high_hz: 6000.0,
            sus_low_hz: 60.0,
            sus_high_hz: 8000.0,
            level_ms: 20.0,
            gain_ms: 60.0,
            fast_ms: 1.0,
            slow_att_ms: 18.0,
            hold_ms: 25.0,
            memory_ms: 4000.0,
            att_full_db: 6.0,
            sus_full_db: 8.0,
            onset_release_ms: 3.0,
            onset_ramp_ms: 0.0,
            onset_attack_db: 0.0,
        }
    );
    let drums = d::spec(d::S_DRUMS);
    assert_eq!((drums.level_ms, drums.gain_ms, drums.hold_ms, drums.att_full_db, drums.sus_full_db), (12.0, 36.0, 40.0, 12.0, 12.0));
    let guitar = d::spec(d::S_GUITAR);
    assert_eq!((guitar.det_low_hz, guitar.det_high_hz, guitar.sus_low_hz), (1200.0, 8000.0, 70.0));
    assert_eq!((guitar.hold_ms, guitar.att_full_db, guitar.sus_full_db), (40.0, 8.0, 2.0));
    let bass = d::spec(d::S_BASS);
    assert_eq!((bass.memory_ms, bass.fast_ms, bass.hold_ms, bass.sus_full_db), (1500.0, 1.5, 40.0, 15.0));
    // v1's bands, slow attack, memory and full-scale ratios are kept on every source.
    let v1: [[f32; 8]; 7] = [
        [40.0, 12000.0, 40.0, 12000.0, 10.0, 800.0, 12.0, 12.0],
        [200.0, 14000.0, 150.0, 14000.0, 6.0, 500.0, 12.0, 4.0],
        [1200.0, 8000.0, 70.0, 8000.0, 8.0, 2000.0, 8.0, 2.0],
        [80.0, 6000.0, 60.0, 8000.0, 18.0, 4000.0, 6.0, 8.0],
        [40.0, 2500.0, 35.0, 3000.0, 20.0, 1500.0, 8.0, 15.0],
        [300.0, 6000.0, 90.0, 8000.0, 20.0, 1000.0, 6.0, 4.0],
        [40.0, 12000.0, 40.0, 12000.0, 15.0, 800.0, 8.0, 12.0],
    ];
    for (i, r) in v1.iter().enumerate() {
        let p = d::spec(i as i32);
        assert_eq!(
            [p.det_low_hz, p.det_high_hz, p.sus_low_hz, p.sus_high_hz, p.slow_att_ms, p.memory_ms, p.att_full_db, p.sus_full_db],
            *r,
            "source {i}"
        );
    }
    assert_eq!(d::spec(-3), d::spec(0));
    assert_eq!(d::spec(99), d::spec(6));
}

#[test]
fn registered_with_the_3_ms_lookahead_as_latency() {
    // v2: `round(0.003 * sr)`, for every source, bypassed or not, and with both amounts at zero.
    let mut p = crate::create("fs_transient").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    for (sr, want) in [(44_100.0, 132), (48_000.0, 144), (96_000.0, 288), (192_000.0, 576)] {
        for source in 0..7 {
            for bypass in [0.0, 1.0] {
                p.set_param("source", source as f32);
                p.set_param("bypass", bypass);
                p.prepare(sr, 512, 2);
                assert_eq!(p.latency(), want, "{sr} Hz, source {source}, bypass {bypass}");
                let mut io = vec![hits(512, 0.5), hits(512, 0.5)];
                p.process(&mut io, 512);
                assert_eq!(p.latency(), want);
            }
        }
    }
    assert!(p.analysis().is_some());
}

#[test]
fn zero_amounts_are_the_delayed_input_sample_for_sample() {
    // Every source, every focus: attack and sustain at 0 dB pass the input delayed by the
    // reported latency bit for bit (zeros first), and so does Mix at 0 (with the shaping on).
    for source in 0..7 {
        for focus in 0..4 {
            let mut s = prepared();
            s.set_param("source", source as f32);
            s.set_param("attackFocus", focus as f32);
            s.set_param("sustainFocus", (3 - focus) as f32);
            s.set_param("attackTime", 0.2);
            let src = hits(9600, 0.6);
            let mut io = vec![src.clone(), src.clone()];
            run(&mut s, &mut io, 512);
            assert_eq!(s.latency(), 144);
            let want = delayed(&src, s.latency());
            assert_eq!(io[0], want, "source {source}, focus {focus}");
            assert_eq!(io[1], want);
        }
    }
    let mut s = Transient::new();
    shaping(&mut s);
    s.set_param("mix", 0.0);
    s.prepare(48_000.0, 512, 2);
    let src = hits(9600, 0.6);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    assert_eq!(io[0], delayed(&src, 144), "mix 0");
}

#[test]
fn attack_shapes_the_front_and_sustain_the_tail() {
    let measure = |id: &str, v: f32| {
        let mut s = prepared();
        s.set_param(id, v);
        let src = hits(SR, 0.5);
        let mut io = vec![src.clone(), src.clone()];
        run_aligned(&mut s, &mut io, 512);
        // The second hit (warm detectors): its first 5 ms and its tail from 120 to 200 ms (v1: from
        // 80 ms; v2's Drums hold the attack shape 40 ms, so at 80 ms an Attack of +18 still
        // lifts it about 1.3 dB, letting go smoothly instead of stepping).
        let hit = SR / 4;
        let front = |x: &[f32]| rms(&x[hit..hit + 240]);
        let tail = |x: &[f32]| rms(&x[hit + 5760..hit + 9600]);
        (20.0 * (front(&io[0]) / front(&src)).log10(), 20.0 * (tail(&io[0]) / tail(&src)).log10())
    };
    let (front, tail) = measure("attack", 1.0);
    assert!(front > 3.0, "attack +18 lifts the onset: {front} dB");
    assert!(tail.abs() < 1.0, "and leaves the tail: {tail} dB");
    let (front, tail) = measure("sustain", 0.0);
    assert!(tail < -6.0, "sustain -24 cuts the tail: {tail} dB");
    // v2 lets the previous hit's -24 dB go over 3 ms from when the detector hears the new one,
    // 3 ms ahead of the audio: mostly gone, not all, in the first 5 ms (about -4 dB; a tail
    // would be cut 24). v1's bar was -3.
    assert!(front > -6.0, "and mostly spares the onset: {front} dB");
}

#[test]
fn stereo_is_linked() {
    let mut s = prepared();
    shaping(&mut s);
    let l = hits(SR / 2, 0.6);
    let r: Vec<f32> = l.iter().map(|x| x * 0.25).collect();
    let mut io = vec![l, r];
    run(&mut s, &mut io, 512);
    for i in (0..SR / 2).step_by(97) {
        assert!((io[1][i] - io[0][i] * 0.25).abs() < 1e-6, "sample {i}: {} vs {}", io[1][i], io[0][i]);
    }
}

#[test]
fn prepare_pushes_before_preparing() {
    // `prepareToPlay` pushes the parameters first: the glides start at the set values.
    let mut a = Transient::new();
    a.set_param("attack", 1.0);
    a.set_param("source", 2.0);
    a.prepare(48_000.0, 512, 2);
    let mut io = vec![hits(256, 0.5), hits(256, 0.5)];
    a.process(&mut io, 256);
    let mut r = [f32::NAN; 8];
    a.readouts(&mut r);
    // Set after the prepare instead, the glide starts at 0 dB and has barely moved by then.
    let mut b = prepared();
    b.set_param("attack", 1.0);
    b.set_param("source", 2.0);
    let mut io2 = vec![hits(256, 0.5), hits(256, 0.5)];
    b.process(&mut io2, 256);
    let mut r2 = [f32::NAN; 8];
    b.readouts(&mut r2);
    assert!(r[4] > r2[4] * 2.0 && r[4] > 1.0, "{} vs {}", r[4], r2[4]);
}

#[test]
fn readouts_and_scope_bins() {
    let mut s = prepared();
    s.set_param("attack", 1.0);
    let src = hits(SR, 0.5);
    let a = s.analysis().unwrap();
    assert!(Arc::ptr_eq(&a, &s.analysis().unwrap()), "the same streams every call");
    // The first hit, then one block that holds the second hit's front.
    run(&mut s, &mut [src[..SR / 4].to_vec(), src[..SR / 4].to_vec()], 512);
    let front = src[SR / 4..SR / 4 + 512].to_vec();
    let mut blk = vec![front.clone(), front.clone()];
    s.process(&mut blk, 512);
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert_eq!(r[0], gain_to_db(peak(&[front], 1, 512)), "{r:?}");
    assert!(r[1].is_finite(), "{r:?}");
    assert!(r[4] > 3.0 && r[4] <= 18.0, "a boost on the front: {r:?}");
    assert_eq!(r[5], 0.0, "sustain at 0 dB");
    assert_eq!(r[2], s.engine().last_attack_db());
    assert!(r[6].is_finite() && r[6] > -60.0, "{r:?}");
    assert!(r[7].is_finite() && r[7] > -60.0, "{r:?}");
    // The scope: one 4 ms bin per 192 samples at 48 kHz, four values each.
    let written = a.post.written();
    assert_eq!(written, (SR / 4 + 512) / 192 * d::SCOPE_BIN_VALUES);
    assert_eq!(a.pre.written(), 0, "pre is not fed");
    let (bins, w) = s.engine().scope();
    assert_eq!(w as usize, written / d::SCOPE_BIN_VALUES);
    let last = bins[w as usize - 1];
    let mut tail = [0.0f32; 4];
    a.post.latest(&mut tail);
    assert_eq!(tail, [last.in_peak, last.out_peak, last.att_db, last.sus_db]);
    assert!((s.engine().scope_bins_per_second() - 250.0).abs() < 1e-3);
    // Silence: the meters read silence and nothing is shaped (v2's held attack shape lets go
    // exponentially, so after a second it is a few 1e-10 dB rather than exactly 0).
    let mut quiet = vec![vec![0.0f32; SR], vec![0.0f32; SR]];
    run(&mut s, &mut quiet, 512);
    s.readouts(&mut r);
    assert_eq!(r[0], gain_to_db(0.0));
    assert!(r[4].abs() < 1e-6 && r[5] == 0.0, "{r:?}");
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_transient");
    assert_eq!(presets.len(), 44);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &TRANSIENT_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert!(!p.values.contains_key("bypass"), "{name}");
        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        for key in raw["values"].as_object().unwrap().keys() {
            assert!(p.values.contains_key(key), "{name}: {key} did not resolve");
        }
        for key in ["source", "attack", "sustain", "attackTime", "sustainTime", "attackFocus", "sustainFocus", "mix"] {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
        assert!(!p.values.contains_key("output"), "{name}: a preset leaves Output alone");
    }
    let piano = presets.iter().find(|(n, _)| *n == "Hammer Up, Ring Down").unwrap();
    let p = FsPreset::parse(piano.1, &TRANSIENT_INFO).unwrap();
    assert_eq!(p.values["source"], 3.0);
    assert_eq!(p.values["sustain"], 0.125);
    let pick = presets.iter().find(|(n, _)| *n == "Soft Pick").unwrap();
    assert_eq!(FsPreset::parse(pick.1, &TRANSIENT_INFO).unwrap().values["attackFocus"], 3.0);
}

#[test]
fn hostile_params_stay_finite() {
    // Bypass stays off (it would hide the wet path); the choices are set per case.
    let skip = ["source", "attackFocus", "sustainFocus", "bypass"];
    for bad in HOSTILE {
        for source in 0..7 {
            for focus in 0..4 {
                let mut s = prepared();
                s.set_param("source", source as f32);
                s.set_param("attackFocus", focus as f32);
                s.set_param("sustainFocus", ((focus + source) % 4) as f32);
                for info in TRANSIENT_PARAMS.iter().filter(|p| !skip.contains(&p.id)) {
                    assert!(s.set_param(info.id, bad));
                }
                assert_eq!(s.param("bypass"), Some(0.0));
                let mut io = vec![hits(4096, 0.9), hits(4096, -0.9)];
                run(&mut s, &mut io, 256);
                assert!(all_finite(&io), "source {source}, focus {focus}, value {bad}");
            }
        }
    }
}

#[test]
fn hostile_choice_params_stay_finite() {
    for bad in HOSTILE {
        let mut s = prepared();
        shaping(&mut s);
        for id in CHOICES {
            assert!(s.set_param(id, bad));
        }
        assert_eq!(s.param("bypass"), Some(0.0));
        let mut io = vec![hits(4096, 0.9), hits(4096, -0.9)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "value {bad}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        for source in 0..7 {
            let mut s = prepared();
            shaping(&mut s);
            s.set_param("source", source as f32);
            let src = [hits(4096, 0.9), hits(4096, -0.9)];
            // Half the run with the wet path engaged, then the hostile bypass value mid-stream.
            let mut a = vec![src[0][..2048].to_vec(), src[1][..2048].to_vec()];
            run(&mut s, &mut a, 256);
            assert!(s.set_param("bypass", bad));
            let mut b = vec![src[0][2048..].to_vec(), src[1][2048..].to_vec()];
            run(&mut s, &mut b, 256);
            assert!(all_finite(&a) && all_finite(&b), "source {source}, bypass {bad}");
        }
    }
}

#[test]
fn hostile_input_stays_finite() {
    // Loud, DC and denormal input: the ratios use a tiny floor, so nothing divides by zero.
    let mut s = prepared();
    shaping(&mut s);
    let mut io = vec![vec![1e30f32; 4096], vec![1e-40f32; 4096]];
    run(&mut s, &mut io, 256);
    assert!(all_finite(&io));
}

#[test]
fn switching_quality_and_model_mid_stream_is_safe() {
    // No quality setting in this engine: source, the focuses and the times are what switch.
    let mut s = prepared();
    shaping(&mut s);
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = hits(128 * 500, 0.8);
    for blk in 0..500 {
        s.set_param("source", (blk % 7) as f32);
        s.set_param("attackFocus", ((blk / 2) % 4) as f32);
        s.set_param("sustainFocus", ((blk / 3) % 4) as f32);
        s.set_param("attackTime", (blk % 5) as f32 / 4.0);
        s.set_param("sustainTime", (blk % 6) as f32 / 5.0);
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
    let mut s = Transient::new();
    s.prepare(48_000.0, 512, 1);
    shaping(&mut s);
    let mut mono = vec![hits(9600, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    // Six channels: the first two processed, the rest untouched.
    let mut s = Transient::new();
    s.prepare(48_000.0, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| hits(9600, 0.1 * (c + 1) as f32)).collect();
    let orig = wide.clone();
    shaping(&mut s);
    run(&mut s, &mut wide, 512);
    assert!(all_finite(&wide));
    assert_ne!(wide[0], orig[0]);
    assert_ne!(wide[1], orig[1]);
    assert_eq!(wide[2..], orig[2..]);

    // Fewer buffers than prepared, short buffers, zero frames: no panic.
    let mut s = prepared();
    shaping(&mut s);
    let mut one = vec![vec![0.3f32; 10]];
    s.process(&mut one, 512);
    s.process(&mut [], 64);
    let mut empty = vec![Vec::new(), Vec::new()];
    s.process(&mut empty, 64);
    let mut ragged = vec![vec![0.3f32; 64], vec![0.3f32; 10]];
    s.process(&mut ragged, 64);
    assert!(all_finite(&ragged) && all_finite(&one));
}

#[test]
fn silence_tail_is_cheap() {
    let mut cases = Vec::new();
    for source in 0..7 {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            shaping(&mut s);
            s.set_param("source", source as f32);
            s.set_param("attackFocus", 1.0);
            s.set_param("sustainFocus", 3.0);
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
        cases.push((source, tail, full));
        assert!(tail < 4.0 * full, "source {source}: silence tail {tail:.3}s vs signal {full:.3}s");
    }
    println!("{cases:?}");
}

#[test]
fn process_does_not_allocate() {
    // No counting allocator in this crate: the engine's only heap block is its scope ring, made in
    // `new`; its address and size across 1000 blocks of switching are the whole story, the
    // analysis ring is the same `Arc`, and the host buffers keep theirs.
    let mut s = prepared();
    shaping(&mut s);
    let engine_ptr: *const TransientEngineDsp = s.engine();
    let before = s.engine().heap_block();
    let delay = s.engine().delay_blocks();
    assert!(delay.iter().all(|&(_, cap)| cap >= 144), "the delay line is sized in prepare: {delay:?}");
    let ring = s.analysis().unwrap();
    let src = hits(256, 0.7);
    let mut io = vec![src.clone(), src.clone()];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        for c in &mut io {
            c.copy_from_slice(&src);
        }
        s.set_param("source", ((blk / 4) % 7) as f32);
        s.set_param("attackFocus", (blk % 4) as f32);
        s.set_param("sustainTime", (blk % 3) as f32 / 3.0);
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, s.engine() as *const TransientEngineDsp);
    assert_eq!(before, s.engine().heap_block());
    assert_eq!(delay, s.engine().delay_blocks(), "the delay line is never re-made in process");
    assert!(Arc::ptr_eq(&ring, &s.analysis().unwrap()));
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is rebuilt
    // and re-prepared in place, its scope ring re-filled, never re-made.
    let mut s = prepared();
    shaping(&mut s);
    let mut io = vec![hits(512, 0.5), hits(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const TransientEngineDsp = s.engine();
    let block = s.engine().heap_block();
    let delay = s.engine().delay_blocks();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const TransientEngineDsp);
        assert_eq!(block, s.engine().heap_block());
        assert_eq!(delay, s.engine().delay_blocks(), "the delay line is kept across a reset");
        s.process(&mut io, 512);
    }
    // And a prepare at another rate re-uses it too.
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(block, s.engine().heap_block());
    assert!(all_finite(&io));
}

#[test]
fn bypass_lands_exactly_on_the_input() {
    for source in 0..7 {
        let mut s = prepared();
        shaping(&mut s);
        s.set_param("source", source as f32);
        s.set_param("output", 0.9);
        s.set_param("bypass", 1.0);
        let src = hits(SR, 0.6);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        // The 15 ms glide lands (within 1e-9 it snaps), then it is the input delayed by the
        // latency (reported bypassed too), exactly.
        assert_eq!(io[0][SR / 2..], delayed(&src, 144)[SR / 2..], "source {source}");
    }
}

#[test]
fn reset_matches_a_fresh_module() {
    // After a reset the module renders bit for bit as a fresh one given the same (non-default)
    // parameters and then prepared (the C++ wrapper's path: `prepareToPlay` pushes them first, so
    // no glide runs from the defaults), whatever it played before.
    let setup = |s: &mut Transient| {
        for (id, v) in [
            ("source", 3.0),
            ("attack", 0.8),
            ("sustain", 0.1),
            ("attackTime", 0.3),
            ("sustainTime", 0.8),
            ("attackFocus", 3.0),
            ("sustainFocus", 1.0),
            ("mix", 0.7),
            ("output", 0.0),
        ] {
            s.set_param(id, v);
        }
    };
    let src = vec![hits(9600, 0.7), hits(9600, -0.4)];
    let mut fresh = Transient::new();
    setup(&mut fresh);
    fresh.prepare(48_000.0, 512, 2);
    let mut want = src.clone();
    run(&mut fresh, &mut want, 512);

    let mut s = prepared();
    setup(&mut s);
    let mut warm = vec![hits(7000, 0.9), sine(7000, 300.0, -0.3)];
    run(&mut s, &mut warm, 333);
    s.reset();
    let mut got = src.clone();
    run(&mut s, &mut got, 512);
    assert_eq!(got, want, "a reset module renders as a fresh, prepared one");
    s.reset();
    let mut again = src.clone();
    run(&mut s, &mut again, 512);
    assert_eq!(again, want);
    assert!(all_finite(&got));
    // The scope restarts too: the bins after a reset are the fresh module's.
    let (fresh_bins, fw) = fresh.engine().scope();
    let (bins, w) = s.engine().scope();
    assert_eq!(fw, w);
    assert_eq!(fresh_bins, bins);
    // No glide from the defaults: Output at -12 dB holds from the first sample after a reset
    // (Mix 0.7 of an identity wet path is the input), never louder than the input scaled by it.
    let g = 10f32.powf(-12.0 / 20.0);
    let mut s = prepared();
    s.set_param("output", 0.0);
    s.set_param("mix", 0.0);
    let mut io = vec![hits(4096, 0.5), hits(4096, 0.5)];
    run(&mut s, &mut io, 512);
    s.reset();
    let src = hits(4096, 0.5);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    // (the delay line restarts too: zeros, then the input)
    let src = delayed(&src, s.latency());
    for i in 0..4096 {
        assert!((io[0][i] - src[i] * g).abs() < 1e-6, "sample {i}: {} vs {}", io[0][i], src[i] * g);
    }
}

#[test]
fn a_reset_is_independent_of_what_played_before() {
    // Two modules with different histories (input, every knob moved, source and focus switches,
    // bypass mid-glide), brought to the same values and reset, render bit for bit the same over
    // 60000 samples (the longest hold and release several times over).
    let last = |s: &mut Transient| {
        for p in TRANSIENT_PARAMS.iter() {
            s.set_param(p.id, p.default);
        }
        for (id, v) in [
            ("source", 5.0),
            ("attack", 0.85),
            ("sustain", 0.2),
            ("attackTime", 0.9),
            ("sustainTime", 0.1),
            ("attackFocus", 2.0),
            ("sustainFocus", 3.0),
            ("mix", 0.6),
            ("output", 0.7),
        ] {
            s.set_param(id, v);
        }
    };
    let mut a = prepared();
    shaping(&mut a);
    a.set_param("source", 1.0);
    let mut warm = vec![hits(20_000, 0.9), sine(20_000, 300.0, -0.3)];
    run(&mut a, &mut warm, 333);
    a.set_param("bypass", 1.0);
    let mut warm = vec![hits(100, 0.9), hits(100, 0.9)];
    run(&mut a, &mut warm, 100);
    last(&mut a);

    let mut b = prepared();
    b.set_param("attackFocus", 1.0);
    b.set_param("output", 1.0);
    let mut warm = vec![sine(9_000, 90.0, 0.8), hits(9_000, 0.2)];
    run(&mut b, &mut warm, 512);
    last(&mut b);

    a.reset();
    b.reset();
    let src = vec![hits(60_000, 0.7), hits(60_000, -0.4)];
    let (mut x, mut y) = (src.clone(), src.clone());
    run(&mut a, &mut x, 512);
    run(&mut b, &mut y, 256);
    assert!(x == y, "two histories, one render after a reset");
    assert!(all_finite(&x));
}

// ---------------------------------------------------------------- v2: transparent detectors

/// A piano-like passage, mono: three struck chords (a hammer click, partials at 220 / 330 / 330.7
/// / 523 Hz -- one pair beating like a unison string pair -- ringing down over about a second),
/// the last one softer.
fn piano_chords(len: usize) -> Vec<f32> {
    let mut x = vec![0.0f32; len];
    let mut seed = 0x0bad_5eedu32;
    for (t0, amp) in [(0.05f64, 0.30f64), (0.70, 0.25), (1.35, 0.12)] {
        let start = (t0 * SR as f64) as usize;
        for (i, v) in x.iter_mut().enumerate().skip(start) {
            let t = (i - start) as f64 / SR as f64;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(seed >> 8) / f64::from(1u32 << 24) - 0.5;
            let w = std::f64::consts::TAU * t;
            let ring = (0.5 * (w * 220.0).sin() + 0.3 * ((w * 330.0).sin() + (w * 330.7).sin()) + 0.15 * (w * 523.25).sin())
                * (-t / 0.9).exp()
                * (1.0 - (-t / 0.002).exp());
            *v += (amp * (0.6 * noise * (-t / 0.004).exp() + ring)) as f32;
        }
    }
    x
}

/// The owner-approved prototype's maths (`tproto-reference.rs`), kept as it is written there,
/// with the approved configuration: `PE=1 LA=3 HD=25 AGU=3 AGD=1 FA=1 SA=18 DEAD=2.5 AFULL=8.5
/// AG=1` and args `<sus_db> 20 60 3 <tag> 1 4000 60`, `ATT` the attack amount. Mono, 48 kHz, the
/// Piano bands (sustain 60..8000 Hz, attack 80..6000 Hz), susFull 8. Its output is
/// `x[n - 144] * 10^((g + ga) / 20)` in f64, written as f32.
fn prototype(x: &[f32], att_db: f64, sus_db: f64) -> Vec<f32> {
    use std::f64::consts::PI;
    #[derive(Clone, Copy)]
    struct Bq {
        b0: f64,
        b1: f64,
        b2: f64,
        a1: f64,
        a2: f64,
        z1: f64,
        z2: f64,
    }
    impl Bq {
        fn rbj(hp: bool, fc: f64, fs: f64, q: f64) -> Bq {
            let w = 2.0 * PI * fc / fs;
            let (cw, al) = (w.cos(), w.sin() / (2.0 * q));
            let a0 = 1.0 + al;
            let (b0, b1) = if hp { ((1.0 + cw) * 0.5 / a0, -(1.0 + cw) / a0) } else { ((1.0 - cw) * 0.5 / a0, (1.0 - cw) / a0) };
            Bq { b0, b1, b2: b0, a1: -2.0 * cw / a0, a2: (1.0 - al) / a0, z1: 0.0, z2: 0.0 }
        }
        fn run(&mut self, x: f64) -> f64 {
            let y = self.b0 * x + self.z1;
            self.z1 = self.b1 * x - self.a1 * y + self.z2;
            self.z2 = self.b2 * x - self.a2 * y;
            y
        }
    }
    let k = |ms: f64| 1.0 - (-1.0 / (ms * 48.0)).exp();
    let fs = 48000.0;
    let mut sb =
        [Bq::rbj(true, 60.0, fs, 0.5412), Bq::rbj(true, 60.0, fs, 1.3066), Bq::rbj(false, 8000.0, fs, 0.5412), Bq::rbj(false, 8000.0, fs, 1.3066)];
    let mut ab =
        [Bq::rbj(true, 80.0, fs, 0.5412), Bq::rbj(true, 80.0, fs, 1.3066), Bq::rbj(false, 6000.0, fs, 0.5412), Bq::rbj(false, 6000.0, fs, 1.3066)];
    let (ms_tau, g_tau, on_tau, memory_ms, rise_ms, sus_full) = (20.0, 60.0, 3.0, 4000.0, 60.0, 8.0);
    let (fa, sa, dead, afull, agu, agd, hd, la) = (1.0, 18.0, 2.5, 8.5, 3.0, 1.0, 25.0, 3.0);
    let (mut m1, mut m2) = (0.0f64, 0.0f64);
    let (mut f1, mut f2, mut s1) = (0.0f64, 0.0f64, 0.0f64);
    let (mut q1, mut q2, mut qs, mut ga) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut cprev = 0.0f64;
    let mut hold_sh = 0.0f64;
    let mut peak = -200.0f64;
    let mut g = 0.0f64;
    let mut onset_left = 0i32;
    let (km, kg, kgo, kr) = (k(ms_tau), k(g_tau), k(on_tau), k(memory_ms));
    let (kf, ks) = (k(1.0), k(20.0));
    let dn = (la * 48.0f64).round() as usize;
    let mut y = Vec::with_capacity(x.len());
    for (idx, &v) in x.iter().enumerate() {
        let v = f64::from(v);
        let mut b = v;
        for f in &mut sb {
            b = f.run(b);
        }
        let mut c = v;
        for f in &mut ab {
            c = f.run(c);
        }
        m1 += km * (b * b - m1);
        m2 += km * (m1 - m2);
        let l = 10.0 * (m2 + 1e-18).log10();
        f1 += kf * (c * c - f1);
        f2 += kf * (f1 - f2);
        s1 += ks * (f2 - s1);
        let att = 10.0 * ((f2 + 1e-18) / (s1 + 1e-18)).log10();
        if att > 6.0 {
            onset_left = 480; // 10 ms
        }
        if onset_left > 0 {
            onset_left -= 1;
        }
        if l > peak || onset_left > 0 {
            peak = peak.max(l).min(if onset_left > 0 { l } else { f64::MAX });
            if onset_left > 0 {
                peak = l;
            }
        } else {
            peak += kr * (l - peak);
        }
        let live = ((l + 90.0) / 10.0).clamp(0.0, 1.0);
        let shape = ((peak - l) / sus_full).clamp(0.0, 1.0) * live;
        let target = if onset_left > 0 { 0.0 } else { sus_db * shape };
        g += if onset_left > 0 {
            kgo
        } else if target > g {
            k(rise_ms)
        } else {
            kg
        } * (target - g);
        let cd = {
            let d = c - cprev;
            cprev = c;
            d
        };
        q1 += k(fa) * (cd * cd - q1);
        q2 += k(fa) * (q1 - q2);
        qs += k(sa) * (q2 - qs);
        let ar = 10.0 * ((q2 + 1e-18) / (qs + 1e-18)).log10();
        let ash = ((ar - dead) / (afull - dead)).clamp(0.0, 1.0) * live;
        hold_sh = if ash > hold_sh { ash } else { hold_sh + k(hd) * (ash - hold_sh) };
        let at = att_db * hold_sh;
        ga += k(if at.abs() > ga.abs() { agu } else { agd }) * (at - ga);
        let xd = if idx >= dn { f64::from(x[idx - dn]) } else { 0.0 };
        y.push((xd * 10f64.powf((g + ga) / 20.0)) as f32);
    }
    y
}

#[test]
fn piano_matches_the_approved_prototype() {
    // The owner approved v2 by ear on the prototype's renders (Piano, 48 kHz, mono, Full focus).
    // The engine must reproduce it to within float rounding: through the whole module (mono,
    // Source Piano, Mix 100 %, Output 0 dB), the output against the prototype's maths above.
    let src = piano_chords(2 * SR);
    for (att, sus) in [(12.0f32, -12.0f32), (6.0, -6.0), (-9.0, 6.0)] {
        let mut s = Transient::new();
        s.set_param("source", 3.0);
        s.set_param("attack", d::attack_knob_from_db(att));
        s.set_param("sustain", d::sustain_knob_from_db(sus));
        s.prepare(48_000.0, 512, 1);
        let mut io = vec![src.clone()];
        run(&mut s, &mut io, 512);
        let want = prototype(&src, f64::from(att), f64::from(sus));
        let worst = io[0].iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let differ = io[0].iter().zip(&want).filter(|(a, b)| a != b).count();
        let peak = want.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(worst <= 1e-6 * peak.max(1e-3), "attack {att}, sustain {sus}: worst {worst} ({differ} samples differ), peak {peak}");
        // and the shaping is really there: not a trivially equal pair of near-unity gains
        let moved = io[0].iter().zip(delayed(&src, 144)).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(moved > 0.01, "attack {att}, sustain {sus}: the gains moved the audio ({moved})");
    }
}

/// The total gain (attack + sustain, dB) applied to each sample, processed one sample at a time.
fn gain_trace(source: i32, att: f32, sus: f32, x: &[f32]) -> Vec<f32> {
    let mut s = Transient::new();
    s.set_param("source", source as f32);
    s.set_param("attack", d::attack_knob_from_db(att));
    s.set_param("sustain", d::sustain_knob_from_db(sus));
    s.prepare(48_000.0, 1, 1);
    let mut io = vec![vec![0.0f32]];
    x.iter()
        .map(|&v| {
            io[0][0] = v;
            s.process(&mut io, 1);
            s.engine().last_attack_db() + s.engine().last_sustain_db()
        })
        .collect()
}

#[test]
fn the_gain_never_steps_more_than_0_3_db_a_sample() {
    // v1 jumped 1.4 dB a sample on a piano's sustain and 9.3 dB on its attack. v2, at Attack +12 /
    // Sustain -12 on a piano-like decaying chord: no one-sample step over 0.3 dB, for the Piano
    // source the owner approved and every tuned one, while the gains really move both ways.
    let x = piano_chords(2 * SR);
    for source in 0..7 {
        let g = gain_trace(source, 12.0, -12.0, &x);
        let step = g.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let hi = g.iter().copied().fold(f32::MIN, f32::max);
        let lo = g.iter().copied().fold(f32::MAX, f32::min);
        assert!(step <= 0.3, "source {source}: a {step} dB one-sample step");
        assert!(hi > 3.0 && lo < -3.0, "source {source}: the gains moved ({lo} .. {hi} dB)");
    }
}

#[test]
fn stereo_link_takes_the_louder_channel() {
    // Each detector takes the larger of the two channels' squared values before smoothing, so a
    // right channel that is always the quieter changes nothing: the left comes out exactly as it
    // does alone, and the right gets the same gain (its ratio to the left is kept).
    let l = piano_chords(SR);
    let r: Vec<f32> = l.iter().map(|v| v * 0.25).collect();
    let mut st = Transient::new();
    shaping(&mut st);
    st.set_param("source", 3.0);
    st.prepare(48_000.0, 512, 2);
    let mut io = vec![l.clone(), r];
    run(&mut st, &mut io, 512);
    let mut mono = Transient::new();
    shaping(&mut mono);
    mono.set_param("source", 3.0);
    mono.prepare(48_000.0, 512, 1);
    let mut alone = vec![l];
    run(&mut mono, &mut alone, 512);
    assert_eq!(io[0], alone[0], "the louder channel decides");
    for (i, (r, l)) in io[1].iter().zip(&io[0]).enumerate() {
        assert!((r - l * 0.25).abs() <= 1e-6 * l.abs().max(1e-3), "sample {i}");
    }
    // And when the right is the louder one, it decides for both.
    let r = piano_chords(SR);
    let l: Vec<f32> = r.iter().map(|v| v * 0.5).collect();
    st.reset();
    let mut io = vec![l, r.clone()];
    run(&mut st, &mut io, 512);
    mono.reset();
    let mut alone = vec![r];
    run(&mut mono, &mut alone, 512);
    assert_eq!(io[1], alone[0]);
}

/// The attack gain (dB) per sample of a held note, the Source switched from `from` to `to` at
/// `at` samples (Attack +12), processed one sample at a time.
fn attack_across_a_source_switch(from: i32, to: i32, at: usize, x: &[f32]) -> Vec<f32> {
    let mut s = Transient::new();
    s.set_param("source", from as f32);
    s.set_param("attack", d::attack_knob_from_db(12.0));
    s.prepare(48_000.0, 1, 1);
    let mut io = vec![vec![0.0f32]];
    x.iter()
        .enumerate()
        .map(|(i, &v)| {
            if i == at {
                s.set_param("source", to as f32);
            }
            io[0][0] = v;
            s.process(&mut io, 1);
            s.engine().last_attack_db()
        })
        .collect()
}

#[test]
fn a_source_switch_mid_note_is_not_an_attack() {
    // A note that started long before (its attack long gone), then the Source switched under it:
    // the new detector bands start from the old ones' state. That must not read as an onset.
    let len = 2 * SR;
    let note: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let w = std::f32::consts::TAU * t;
            0.4 * ((w * 110.0).sin() + 0.5 * (w * 330.0).sin() + 0.25 * (w * 1760.0).sin()) * (-t / 3.0).exp() * (1.0 - (-t / 0.002).exp())
        })
        .collect();
    let at = SR;
    let mut worst = 0.0f32;
    for from in 0..7 {
        for to in 0..7 {
            if from == to {
                continue;
            }
            let g = attack_across_a_source_switch(from, to, at, &note);
            let after = g[at..at + SR / 4].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            println!("source {from} -> {to}: attack after the switch {after:.2} dB");
            worst = worst.max(after);
        }
    }
    assert!(worst < 1.0, "a Source switch read as an onset: {worst} dB");
}

#[test]
fn a_hit_into_a_ring_keeps_its_front() {
    // Fix round 1. Drums at Attack +12 / Sustain -24: each hit lands while the last one's ring is
    // cut deep. The sustain cut must be gone from the hit's first millisecond at the output, as it
    // is for a hit out of silence (within 1.5 dB), without a one-sample step over 0.3 dB at
    // Sustain -12. Before the fix (a 3 ms one-pole from a 6 dB / 20 ms detector) it was about -8.
    let x = hits(2 * SR, 0.5);
    for source in [d::S_DRUMS, d::S_PERCUSSION] {
        let mut s = Transient::new();
        s.set_param("source", source as f32);
        s.set_param("attack", d::attack_knob_from_db(12.0));
        s.set_param("sustain", d::sustain_knob_from_db(-24.0));
        s.prepare(48_000.0, 1, 1);
        let lat = s.latency();
        let mut io = vec![vec![0.0f32]];
        let sus: Vec<f32> = x
            .iter()
            .map(|&v| {
                io[0][0] = v;
                s.process(&mut io, 1);
                s.engine().last_sustain_db()
            })
            .collect();
        for k in 1..8 {
            let h = k * SR / 4;
            let before = sus[h + lat - 240];
            let front = sus[h + lat..h + lat + 48].iter().sum::<f32>() / 48.0;
            println!("source {source}, hit {k}: ring {before:.2} dB, front {front:.2} dB");
            assert!(before < -10.0, "source {source}, hit {k}: the ring is cut ({before} dB)");
            assert!(front > -1.5, "source {source}, hit {k}: the front keeps {front} dB of the cut");
        }
        let g = gain_trace(source, 12.0, -12.0, &x);
        let step = g.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(step <= 0.3, "source {source}: a {step} dB step at Sustain -12");
    }
}

#[test]
fn attack_time_extremes_still_boost() {
    // Fix round 1. Attack Time scales the slow average the fast one is held against: at x0.25 the
    // ratio must still develop. The boost over the first 7 ms of a hit (warm detectors), Attack
    // +12, at Attack Time 0, 0.5 and 1, Drums on drum hits and Piano on piano chords.
    let boost = |source: i32, time: f32, x: &[f32], at: usize| {
        let mut s = Transient::new();
        s.set_param("source", source as f32);
        s.set_param("attack", d::attack_knob_from_db(12.0));
        s.set_param("attackTime", time);
        s.prepare(48_000.0, 512, 1);
        let mut io = vec![x.to_vec()];
        run_aligned(&mut s, &mut io, 512);
        20.0 * (rms(&io[0][at..at + 336]) / rms(&x[at..at + 336])).log10()
    };
    let drums = hits(SR, 0.5);
    let piano = piano_chords(SR);
    for (source, x, at) in [(d::S_DRUMS, &drums, SR / 4), (d::S_PIANO, &piano, (0.70 * SR as f64) as usize)] {
        let b: Vec<f32> = [0.0, 0.5, 1.0].iter().map(|&t| boost(source, t, x, at)).collect();
        println!("source {source}: first 7 ms at Attack Time 0 / 0.5 / 1: {b:?} dB");
        assert!(b.iter().all(|&v| v > 3.0), "source {source}: {b:?}");
    }
}

#[test]
fn a_minute_of_silence_stays_cheap() {
    // Fix round 1. The one-pole states flush to 0 below 1e-30, so after a note a minute of
    // silence costs no more than a minute of signal (no denormal crawl), and the gains land on
    // 0 dB exactly: the output is the delayed input again, bit for bit.
    for source in 0..7 {
        let time = |signal: bool| {
            let mut s = prepared();
            shaping(&mut s);
            s.set_param("source", source as f32);
            let n = 60 * SR;
            let mut io = vec![hits(SR, 0.9), hits(SR, 0.9)];
            for c in &mut io {
                if signal {
                    *c = hits(n + SR, 0.9);
                } else {
                    c.resize(n + SR, 0.0);
                }
            }
            let t = Instant::now();
            run(&mut s, &mut io, 512);
            (t.elapsed().as_secs_f64(), s.engine().last_attack_db(), s.engine().last_sustain_db())
        };
        let (quiet, att, sus) = time(false);
        let (busy, ..) = time(true);
        assert_eq!((att, sus), (0.0, 0.0), "source {source}: the gains land on 0 dB");
        assert!(quiet < 2.0 * busy, "source {source}: a minute of silence {quiet:.3}s vs signal {busy:.3}s");
    }
}
