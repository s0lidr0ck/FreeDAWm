// The C++ literals (0.7071f) are kept as written, not swapped for the constants.
#![allow(clippy::approx_constant)]
use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

const SR: f32 = 48_000.0;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / SR).sin()).collect()
}

/// A short burst then silence: what a reverb is for.
fn burst(len: usize, on: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| if i < on { amp * (std::f32::consts::TAU * 330.0 * i as f32 / SR).sin() } else { 0.0 }).collect()
}

fn prepared() -> ReverbEngine {
    let mut s = ReverbEngine::new();
    s.prepare(SR, 512, 2);
    s
}

fn run(s: &mut ReverbEngine, io: &mut [Vec<f32>], block: usize) {
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

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// A setting away from every default, so a reset that left anything behind would show.
const BUSY: &[(&str, f32)] = &[
    ("decay", 0.8),
    ("mix", 0.7),
    ("size", 0.3),
    ("preDelay", 0.5),
    ("level", 0.6),
    ("lowCut", 0.5),
    ("highCut", 0.6),
    ("damping", 0.4),
    ("bass", 0.7),
    ("character", 0.8),
    ("diffusion", 0.4),
    ("modRate", 0.7),
    ("modDepth", 0.8),
    ("width", 0.8),
    ("duck", 0.3),
    ("swell", 0.2),
    ("color", 0.0),
    ("attack", 0.3),
];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = REVERB_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            "decay",
            "mix",
            "machine",
            "size",
            "preDelay",
            "bypass",
            "level",
            "lowCut",
            "highCut",
            "damping",
            "bass",
            "character",
            "diffusion",
            "modRate",
            "modDepth",
            "width",
            "duck",
            "swell",
            "freeze",
            "trails",
            "interval",
            "sync",
            "division",
            "tempo",
            "tempoSrc",
            "color",
            "attack"
        ]
    );
    let defaults: Vec<f32> = REVERB_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(
        defaults,
        [
            0.5041, 0.3, 1.0, 0.5, 0.2828, 0.0, 0.7071, 0.3544, 0.8295, 0.5981, 0.5, 0.5, 0.7, 0.4537, 0.3, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 3.0,
            0.4, 0.0, 2.0, 0.0
        ]
    );
    // The editor's readouts (`formatValue`).
    assert_eq!(REVERB_PARAMS[d::P_DECAY].format(0.5041), "2.5 s");
    assert_eq!(REVERB_PARAMS[d::P_DECAY].format(1.0), "30 s");
    assert_eq!(REVERB_PARAMS[d::P_DECAY].format(0.0), "200 ms");
    assert_eq!(REVERB_PARAMS[d::P_PRE_DELAY].format(0.2828), "20 ms");
    assert_eq!(REVERB_PARAMS[d::P_LEVEL].format(0.7071), "+0.0 dB");
    assert_eq!(REVERB_PARAMS[d::P_LOW_CUT].format(0.3544), "80 Hz");
    assert_eq!(REVERB_PARAMS[d::P_HIGH_CUT].format(0.8295), "12.0 k");
    assert_eq!(REVERB_PARAMS[d::P_DAMPING].format(0.5981), "6.00 k");
    assert_eq!(REVERB_PARAMS[d::P_DAMPING].format(1.0), "Off");
    assert_eq!(REVERB_PARAMS[d::P_MIX].format(1.0), "Wet only");
    assert_eq!(REVERB_PARAMS[d::P_MOD_RATE].format(0.4537), "0.50 Hz");
    assert_eq!(REVERB_PARAMS[d::P_TEMPO].format(0.4), "120.0");
    assert_eq!(REVERB_PARAMS[d::P_WIDTH].format(0.5), "100 %");
    assert_eq!(REVERB_PARAMS[d::P_SWELL].format(0.0), "Off");
    assert_eq!(REVERB_PARAMS[d::P_BASS].format(0.5), "x 1.00");
    assert_eq!(REVERB_PARAMS[d::P_MACHINE].choices.len(), 6);
    assert_eq!(REVERB_PARAMS[d::P_DIVISION].choices.len(), 9);
    assert_eq!(rc::division_text(3, 120.0), "1/16  \u{b7}  125 ms");
}

#[test]
fn registered_with_no_latency_and_the_real_tail() {
    let mut p = crate::create("fs_reverb").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    p.prepare(SR, 512, 2);
    assert_eq!(p.latency(), 0);
    // Hall at 2.5 s: the C++'s fixed 30 s is the floor.
    assert_eq!(p.tail_samples(), (CPP_TAIL_S * SR) as usize);
    // Thirty seconds of decay with the lows at 2.5x ring 75 s.
    p.set_param("decay", 1.0);
    p.set_param("bass", 1.0);
    let t = p.tail_samples() as f32 / SR;
    assert!((t - 75.02).abs() < 0.1, "{t}");
    // The plate's lows are short (x0.5) but its T60 runs 1.25x long.
    p.set_param("machine", 2.0);
    let t = p.tail_samples() as f32 / SR;
    assert!((t - 46.9).abs() < 0.2, "{t}");
    // A synced pre-delay counts from the parameters at the tempo in force: a quarter at the
    // Manual 40 BPM adds 1.5 s, at a 120 BPM host 0.5 s.
    p.set_param("sync", 1.0);
    p.set_param("division", 8.0);
    p.set_param("tempo", 0.0);
    let t = p.tail_samples() as f32 / SR;
    assert!((t - (46.875 + 1.5)).abs() < 0.2, "{t}");
    p.set_transport(&Transport { tempo: 120.0, ..Transport::default() });
    let t = p.tail_samples() as f32 / SR;
    assert!((t - (46.875 + 0.5)).abs() < 0.2, "{t}");
    // Frozen: the tail never ends.
    p.set_param("freeze", 1.0);
    assert_eq!(p.tail_samples(), (FROZEN_TAIL_S * SR) as usize);
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_reverb");
    assert_eq!(presets.len(), 68);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &REVERB_INFO).unwrap();
        assert_eq!(&p.name, name);
        for key in ["machine", "decay", "mix", "size", "preDelay", "interval", "division", "color", "sync", "freeze"] {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
        assert!(!p.values.contains_key("bypass"), "{name}");
    }
}

#[test]
fn machine_decay_matches_apply_machine() {
    // The graph's table is the engine's: every machine, as `applyMachine` sets it.
    for m in 0..6 {
        let mut e = ReverbEngineDsp::new();
        e.set_parameter(d::P_MACHINE, m as f32);
        let mut io = vec![vec![0.0f32; 20_000], vec![0.0f32; 20_000]];
        e.process(&mut io, 2, 20_000); // past the flush's swap point
        assert_eq!(e.machine(), m);
        let (scale, bass, hf, spring) = e.machine_decay_traits();
        let md = rc::machine_decay(m);
        assert_eq!((md.decay_scale, md.bass_mult, md.spring), (scale, bass, spring), "machine {m}");
        if !spring {
            assert_eq!(md.hf_ratio, hf, "machine {m}");
        }
        assert_eq!(md.damp_max, e.damp_limits().1, "machine {m}");
    }
}

#[test]
fn decay_shape_follows_the_maps() {
    let s = rc::decay_shape(1, 0.5041, 0.5, 0.5981, 2, false, 0.02);
    assert!((s.t60_mid - 2.5).abs() < 0.01);
    assert_eq!(s.t60_low, s.t60_mid);
    assert!((s.t60_high - 2.5 * 0.35).abs() < 0.01);
    assert!((s.hi_hz - 6000.0).abs() < 5.0);
    assert_eq!(s.lo_hz, 300.0);
    // 1970s caps the damping at 6 kHz, the plate at 2.5 kHz; the spring has one band.
    assert_eq!(rc::decay_shape(1, 0.5, 0.5, 1.0, 0, false, 0.0).hi_hz, 6000.0);
    assert_eq!(rc::decay_shape(2, 0.5, 0.5, 1.0, 2, false, 0.0).hi_hz, 2500.0);
    let sp = rc::decay_shape(3, 0.5, 1.0, 1.0, 0, false, 0.0);
    assert_eq!((sp.t60_low, sp.t60_high), (sp.t60_mid, sp.t60_mid));
    assert_eq!(sp.hi_hz, 20000.0, "a spring has no colour");
}

#[test]
fn the_tail_decays_at_the_set_rt60() {
    // A burst into Hall, wet only, Low Decay x1 and Damping off (so the band split leaves the mids
    // alone): the tail's level falls about 60 dB per T60.
    let mut s = prepared();
    for (k, v) in [("mix", 1.0), ("decay", 0.3), ("damping", 1.0), ("modDepth", 0.0), ("lowCut", 0.0), ("highCut", 1.0)] {
        s.set_param(k, v);
    }
    let t60 = d::knob_to_decay_sec(0.3);
    let len = (SR * 3.0) as usize;
    let noise: Vec<f32> = (0..len).map(|i| if i < 2400 { ((i * 7919 % 1000) as f32 / 500.0 - 1.0) * 0.5 } else { 0.0 }).collect();
    let mut io = vec![noise.clone(), noise];
    run(&mut s, &mut io, 512);
    let win = |t: f32| {
        let a = (t * SR) as usize;
        20.0 * rms(&io[0][a..a + 2400]).log10()
    };
    let (a, b) = (win(0.4), win(0.4 + t60 * 0.5));
    let drop = a - b;
    assert!((drop - 30.0).abs() < 6.0, "T60 {t60}: {a:.1} -> {b:.1} dB, a drop of {drop:.1} over half a T60");
}

#[test]
fn freeze_holds_the_tail_and_stays_bounded() {
    // Freeze after a loud burst at the longest decay: the tail holds (no decay; the Spring excepted,
    // as in the C++), never grows past a bound, and stays finite over 20 s. Then unfreezing lets it die.
    for machine in 0..6 {
        let mut s = prepared();
        for (k, v) in [("machine", machine as f32), ("mix", 1.0), ("decay", 1.0), ("size", 1.0), ("modDepth", 1.0), ("character", 1.0), ("bass", 1.0)]
        {
            s.set_param(k, v);
        }
        // Past the machine flush, then a full-scale burst.
        let mut io = vec![vec![0.0f32; 24_000], vec![0.0f32; 24_000]];
        run(&mut s, &mut io, 512);
        let mut io = vec![burst(48_000, 24_000, 1.0), burst(48_000, 24_000, -1.0)];
        run(&mut s, &mut io, 512);
        s.set_param("freeze", 1.0);
        let mut levels = Vec::new();
        for _ in 0..40 {
            let mut io = vec![sine(24_000, 1.0), sine(24_000, 1.0)];
            // The send is closed while frozen: the dry still passes (mix 1 = wet only, so the
            // output is the frozen tail alone).
            run(&mut s, &mut io, 512);
            assert!(all_finite(&io), "machine {machine}");
            assert!(peak(&io[0]) < 8.0, "machine {machine}: frozen tail bounded, peak {}", peak(&io[0]));
            levels.push(rms(&io[0]));
        }
        let (first, last) = (levels[4], levels[39]);
        assert!(first > 1e-4, "machine {machine}: something is frozen ({first})");
        if machine == 3 {
            // The C++ Spring ignores Freeze in its loop (`spring` uses `sprGain`, never
            // `freezeSm`): only the send closes, and the tank rings down at its T60.
            assert!(last < first * 0.1, "machine {machine}: the spring rings down ({first} -> {last})");
        } else {
            assert!(last > first * 0.1, "machine {machine}: the frozen tail holds ({first} -> {last})");
        }
        s.set_param("freeze", 0.0);
        s.set_param("decay", 0.0);
        let mut io = vec![vec![0.0f32; 96_000], vec![0.0f32; 96_000]];
        run(&mut s, &mut io, 512);
        assert!(all_finite(&io));
        assert!(rms(&io[0][90_000..]) < first * 0.01, "machine {machine}: unfrozen, it dies");
    }
}

#[test]
fn max_settings_stay_bounded() {
    // Every machine at the longest decay, biggest size, slowest lows, full modulation, character
    // and diffusion, the longest synced pre-delay (a quarter at 40 BPM, 1.5 s), fed full-scale
    // noise-like squares for 10 s: finite and bounded, and the buffers never grow.
    for machine in 0..6 {
        for color in 0..3 {
            let mut s = prepared();
            for (k, v) in [
                ("machine", machine as f32),
                ("color", color as f32),
                ("decay", 1.0),
                ("size", 1.0),
                ("bass", 1.0),
                ("damping", 1.0),
                ("modDepth", 1.0),
                ("modRate", 1.0),
                ("character", 1.0),
                ("diffusion", 1.0),
                ("level", 1.0),
                ("mix", 1.0),
                ("width", 1.0),
                ("attack", 1.0),
                ("sync", 1.0),
                ("division", 8.0),
                ("tempo", 0.0),
                ("tempoSrc", 1.0),
            ] {
                s.set_param(k, v);
            }
            let engine_ptr: *const ReverbEngineDsp = s.engine();
            let sq: Vec<f32> = (0..(SR as usize * 10)).map(|i| if (i / 41) % 2 == 0 { 1.0 } else { -1.0 }).collect();
            let mut io = vec![sq.clone(), sq];
            run(&mut s, &mut io, 512);
            assert!(all_finite(&io), "machine {machine} color {color}");
            let pk = peak(&io[0]).max(peak(&io[1]));
            assert!(pk < 16.0, "machine {machine} color {color}: peak {pk}");
            assert!(rms(&io[0][(SR as usize * 9)..]) > 1e-3, "machine {machine} color {color}: still sounding");
            assert_eq!(engine_ptr, s.engine() as *const ReverbEngineDsp);
        }
    }
}

#[test]
fn tempo_sync_follows_transport() {
    // Stopped, playing, looping back, seeking and a tempo change between blocks: a synced pre-delay
    // follows the host tempo (Manual BPM is the fallback when the host gives none), the output stays
    // finite, and nothing is re-made.
    let mut s = prepared();
    s.set_param("sync", 1.0);
    s.set_param("division", 8.0); // 1/4: one beat
    s.set_param("tempo", 0.0); // 40 BPM if the host tempo were ignored
    let engine_ptr: *const ReverbEngineDsp = s.engine();
    let mut io = vec![sine(256, 0.5), sine(256, 0.5)];
    let mut r = [f32::NAN; 8];
    let mut t = Transport { tempo: 90.0, ..Transport::default() };
    let pre_ms = |s: &mut ReverbEngine, t: &Transport, io: &mut Vec<Vec<f32>>, r: &mut [f32; 8]| {
        s.set_transport(t);
        s.process(io, 256);
        s.readouts(r);
        assert!(all_finite(io));
        r[2]
    };
    // Stopped: the session tempo still sets it.
    assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 60_000.0 / 90.0).abs() < 0.1);
    assert_eq!(r[3], 90.0);
    t.playing = true;
    for blk in 0..40 {
        t.samples = blk * 256;
        t.quarters = t.samples as f64 / 48_000.0 * 1.5;
        assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 60_000.0 / 90.0).abs() < 0.1);
    }
    // Looping back and seeking: the position jumps, the pre-delay does not.
    t.samples = 0;
    t.quarters = 0.0;
    assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 60_000.0 / 90.0).abs() < 0.1);
    s.reset();
    t.samples = 480_000;
    assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 60_000.0 / 90.0).abs() < 0.1);
    // A tempo change lands within the block it arrives in (the engine re-reads every 32 samples).
    t.tempo = 150.0;
    assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 400.0).abs() < 0.1);
    // No usable host tempo: Manual BPM, as the C++.
    t.tempo = f64::NAN;
    assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 1500.0).abs() < 0.1);
    t.tempo = 5.0;
    assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 1500.0).abs() < 0.1);
    // Tempo Src on Manual ignores the host.
    t.tempo = 150.0;
    s.set_param("tempoSrc", 1.0);
    assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 1500.0).abs() < 0.1);
    // Free pre-delay ignores every tempo.
    s.set_param("sync", 0.0);
    s.set_param("tempoSrc", 0.0);
    assert!((pre_ms(&mut s, &t, &mut io, &mut r) - 20.0).abs() < 0.1);
    assert_eq!(engine_ptr, s.engine() as *const ReverbEngineDsp);
}

#[test]
fn synced_pre_delay_moves_the_wet_by_the_beat() {
    // An impulse into a synced Room (1/4 note) arrives one beat later at 120 BPM than at 240 BPM:
    // the difference of the first-arrival times is the difference of the beats (250 ms).
    let first_arrival = |bpm: f64| {
        let mut s = prepared();
        for (k, v) in [("machine", 0.0), ("sync", 1.0), ("division", 8.0), ("mix", 1.0), ("character", 1.0)] {
            s.set_param(k, v);
        }
        s.set_transport(&Transport { tempo: bpm, playing: true, ..Transport::default() });
        // Past the flush (0.33 s) and the pre-delay crossfade.
        let mut io = vec![vec![0.0f32; 24_000], vec![0.0f32; 24_000]];
        run(&mut s, &mut io, 512);
        let mut imp = vec![0.0f32; 48_000];
        imp[0] = 1.0;
        let mut io = vec![imp.clone(), imp];
        run(&mut s, &mut io, 512);
        io[0].iter().position(|v| v.abs() > 1e-3).unwrap_or(usize::MAX)
    };
    let (slow, fast) = (first_arrival(120.0), first_arrival(240.0));
    let diff = slow as i64 - fast as i64;
    assert!((diff - 12_000).abs() < 50, "{slow} vs {fast}");
}

#[test]
fn no_transport_is_the_cpp_path() {
    // The render tool gives no transport: a module that never sees one renders bit-identically to
    // one told "no tempo" (0), so the fidelity cases cover the C++ path.
    let mut a = prepared();
    let mut b = prepared();
    for m in [&mut a, &mut b] {
        m.set_param("sync", 1.0);
        m.set_param("machine", 0.0);
    }
    b.set_transport(&Transport { tempo: 0.0, ..Transport::default() });
    let src = vec![sine(8192, 0.5), sine(8192, -0.4)];
    let (mut x, mut y) = (src.clone(), src.clone());
    run(&mut a, &mut x, 512);
    run(&mut b, &mut y, 512);
    assert_eq!(x, y);
}

#[test]
fn hostile_params_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
        for machine in 0..6 {
            let mut s = prepared();
            s.set_param("machine", machine as f32);
            // Bypass stays off: a hostile value that turned it on would hide the wet path.
            for info in REVERB_PARAMS.iter().filter(|p| p.id != "machine" && p.id != "bypass") {
                assert!(s.set_param(info.id, bad));
            }
            assert_eq!(s.param("bypass"), Some(0.0));
            s.set_transport(&Transport { tempo: f64::from(bad), ..Transport::default() });
            let mut io = vec![sine(24_000, 0.9), sine(24_000, -0.9)];
            run(&mut s, &mut io, 256);
            assert!(all_finite(&io), "machine {machine}, value {bad}");
        }
    }
}

#[test]
fn hostile_choice_values_stay_finite() {
    // Every choice (machine, interval, division, tempo source, colour) at and past its ends.
    let choices: Vec<&ParamInfo> = REVERB_PARAMS.iter().filter(|p| p.unit == crate::Unit::Choice).collect();
    assert_eq!(choices.len(), 5);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.49, 0.5, 2.5, 5.6, 99.0, 1e9] {
        let mut s = prepared();
        s.set_param("decay", 1.0);
        s.set_param("sync", 1.0);
        for c in &choices {
            assert!(s.set_param(c.id, bad));
        }
        let mut io = vec![sine(24_000, 0.9), sine(24_000, -0.9)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "choices at {bad}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        for machine in 0..6 {
            let mut s = prepared();
            s.set_param("machine", machine as f32);
            s.set_param("decay", 1.0);
            let src = [sine(24_000, 0.9), sine(24_000, -0.9)];
            let mut a = vec![src[0][..12_000].to_vec(), src[1][..12_000].to_vec()];
            run(&mut s, &mut a, 256);
            assert!(s.set_param("bypass", bad));
            let mut b = vec![src[0][12_000..].to_vec(), src[1][12_000..].to_vec()];
            run(&mut s, &mut b, 256);
            assert!(all_finite(&a) && all_finite(&b), "machine {machine}, bypass {bad}");
        }
    }
}

#[test]
fn switching_machines_and_colours_mid_stream_is_safe() {
    // This engine has no quality setting; its model switches are the machine, colour and interval,
    // plus freeze and sync.
    let mut s = prepared();
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = sine(128 * 1200, 0.8);
    let engine_ptr: *const ReverbEngineDsp = s.engine();
    for blk in 0..1200 {
        s.set_param("machine", ((blk / 7) % 6) as f32);
        s.set_param("color", ((blk / 3) % 3) as f32);
        s.set_param("interval", (blk % 3) as f32);
        s.set_param("freeze", ((blk / 50) % 2) as f32);
        s.set_param("sync", ((blk / 11) % 2) as f32);
        s.set_param("division", (blk % 9) as f32);
        for b in &mut buf {
            b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
        }
        s.process(&mut buf, 128);
        assert!(all_finite(&buf), "block {blk}");
    }
    assert_eq!(engine_ptr, s.engine() as *const ReverbEngineDsp);
}

#[test]
fn a_machine_change_flushes_the_old_tail() {
    // `setParameter (pMachine)` fades the wet out, swaps, flushes for 0.33 s: right after the switch
    // the old machine's tail does not burst out of the new one.
    let mut s = prepared();
    s.set_param("mix", 1.0);
    s.set_param("decay", 1.0);
    let mut io = vec![sine(48_000, 0.8), sine(48_000, 0.8)];
    run(&mut s, &mut io, 512);
    let loud = rms(&io[0][40_000..]);
    s.set_param("machine", 4.0);
    let mut io = vec![vec![0.0f32; 16_000], vec![0.0f32; 16_000]];
    run(&mut s, &mut io, 512);
    // The wet fades out (0.004 a sample) and stays out for the flush.
    assert!(rms(&io[0][4_000..15_000]) < loud * 1e-3, "{} vs {loud}", rms(&io[0][4_000..15_000]));
}

#[test]
fn mono_and_wide_buffers() {
    // Mono: one channel processed (the left path, x1 = x0, as the C++ with one channel).
    let mut s = ReverbEngine::new();
    s.prepare(SR, 512, 1);
    let mut mono = vec![sine(8192, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    // Six channels: the first two processed, the rest untouched.
    let mut s = ReverbEngine::new();
    s.prepare(SR, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(8192, 0.1 * (c + 1) as f32)).collect();
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
    let sr = SR as usize;
    let mut cases = Vec::new();
    for machine in 0..6 {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            s.set_param("machine", machine as f32);
            s.set_param("decay", 0.3);
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
        cases.push((machine, tail, full));
        assert!(tail < 4.0 * full, "machine {machine}: silence tail {tail:.3}s vs signal {full:.3}s");
    }
    println!("{cases:?}");
}

#[test]
fn process_does_not_allocate() {
    // No counting allocator in this crate: the engine's heap memory is its fixed buffers, made once
    // in `new`; the boxed engine keeps its address, and the host buffers keep their pointers and
    // capacities across 1000 blocks of switching.
    let mut s = prepared();
    let engine_ptr: *const ReverbEngineDsp = s.engine();
    let bufs = s.engine().buffer_ptrs();
    let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        s.set_param("machine", ((blk / 13) % 6) as f32);
        s.set_param("interval", (blk % 3) as f32);
        s.set_param("freeze", ((blk / 40) % 2) as f32);
        s.set_transport(&Transport { tempo: 60.0 + blk as f64, playing: true, ..Transport::default() });
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, s.engine() as *const ReverbEngineDsp);
    assert_eq!(bufs, s.engine().buffer_ptrs());
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is put back in
    // place, never re-made; its buffers are zeroed, not re-allocated.
    let mut s = prepared();
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const ReverbEngineDsp = s.engine();
    let bufs = s.engine().buffer_ptrs();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const ReverbEngineDsp);
        assert_eq!(bufs, s.engine().buffer_ptrs());
        s.process(&mut io, 512);
    }
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(engine_ptr, s.engine() as *const ReverbEngineDsp);
    assert_eq!(bufs, s.engine().buffer_ptrs());
    assert!(all_finite(&io));
}

/// Plays `m` through `len` samples of a busy stereo signal, in odd-sized blocks.
fn warm(m: &mut ReverbEngine, len: usize, amp: f32) {
    let mut io = vec![sine(len, amp), sine(len, -0.6 * amp)];
    run(m, &mut io, 333);
}

#[test]
fn reset_keeps_the_machine_no_flush() {
    // The DAW resets on every Play, Stop and Locate: the wet must be there from the first block
    // (after the pre-delay), not muted by a 0.33 s machine flush.
    for machine in 0..6 {
        let mut s = prepared();
        s.set_param("machine", machine as f32);
        s.set_param("mix", 1.0);
        warm(&mut s, 30_000, 0.8);
        s.reset();
        let mut io = vec![burst(4096, 2048, 0.8), burst(4096, 2048, 0.8)];
        run(&mut s, &mut io, 512);
        // The 20 ms pre-delay is 960 samples; the spring listens half-way round its loop.
        let wet = rms(&io[0][1024..4096]);
        assert!(wet > 1e-3, "machine {machine}: wet {wet} right after a reset");
    }
    // And right after `prepare` (an offline bounce: prepare, then reset) the session's machine is
    // already in force.
    let mut s = ReverbEngine::new();
    s.set_param("machine", 2.0);
    s.set_param("mix", 1.0);
    s.prepare(SR, 512, 2);
    s.reset();
    let mut io = vec![burst(4096, 2048, 0.8), burst(4096, 2048, 0.8)];
    run(&mut s, &mut io, 512);
    assert!(rms(&io[0][1024..4096]) > 1e-3);
    assert_eq!(s.engine().machine(), 2);
}

#[test]
fn reset_keeps_the_filter_designs_prepare_keeps() {
    // The C++ `prepare` never redesigns the output EQ or the colour converters: at 44.1 kHz with the
    // default cuts (and the 1970s colour, designed once its fade allows) the designs in force after
    // the first prepare and block stay through every reset.
    let mut s = ReverbEngine::new();
    s.set_param("color", 0.0);
    s.prepare(44_100.0, 512, 2);
    warm(&mut s, 4096, 0.5);
    let want = s.engine().filter_designs();
    // They are the constructor's 48 kHz output-EQ designs, not 44.1 kHz ones.
    let mut at48 = ReverbEngine::new();
    at48.set_param("color", 0.0);
    at48.prepare(48_000.0, 512, 2);
    warm(&mut at48, 4096, 0.5);
    assert_eq!(want[..4], at48.engine().filter_designs()[..4]);
    for _ in 0..3 {
        warm(&mut s, 3000, 0.7);
        s.reset();
        assert_eq!(s.engine().filter_designs(), want);
        warm(&mut s, 512, 0.7);
        assert_eq!(s.engine().filter_designs(), want);
    }
    // Moving a cut redesigns it at the live rate, as the C++ does.
    s.set_param("lowCut", 0.5);
    warm(&mut s, 512, 0.7);
    assert_ne!(s.engine().filter_designs()[0], want[0]);
}

#[test]
fn reset_clears_the_tail() {
    for machine in 0..6 {
        let mut s = prepared();
        for (k, v) in [("machine", machine as f32), ("decay", 1.0), ("mix", 1.0), ("freeze", 0.0)] {
            s.set_param(k, v);
        }
        warm(&mut s, 30_000, 1.0);
        s.reset();
        let mut io = vec![vec![0.0f32; 8192], vec![0.0f32; 8192]];
        run(&mut s, &mut io, 512);
        assert!(peak(&io[0]).max(peak(&io[1])) < 1e-9, "machine {machine}: silence in, silence out");
    }
}

#[test]
fn resets_render_the_same_whatever_came_before() {
    // Two resets of one instance from the same parameters render bit-identically, and so does a
    // fresh instance given the same parameters that ran past its first push (and flush) and was
    // then reset (8192 samples, every machine, 44.1 kHz).
    let src = vec![sine(8192, 0.7), sine(8192, -0.5)];
    for machine in 0..6 {
        let set = |m: &mut ReverbEngine| {
            for (k, v) in BUSY {
                m.set_param(k, *v);
            }
            m.set_param("machine", machine as f32);
            m.set_param("interval", 1.0);
            m.set_param("sync", 0.0);
            m.set_param("freeze", 0.0);
        };
        let mut fresh = ReverbEngine::new();
        set(&mut fresh);
        fresh.prepare(44_100.0, 512, 2);
        warm(&mut fresh, 20_000, 0.9);
        fresh.reset();
        let mut want = src.clone();
        run(&mut fresh, &mut want, 512);

        let mut used = ReverbEngine::new();
        used.prepare(44_100.0, 512, 2);
        used.set_param("machine", ((machine + 2) % 6) as f32);
        used.set_param("sync", 1.0);
        used.set_param("freeze", 1.0);
        used.set_param("color", 1.0);
        used.set_transport(&Transport { tempo: 133.0, ..Transport::default() });
        warm(&mut used, 20_000, 0.9);
        set(&mut used);
        warm(&mut used, 7_000, 0.4);
        used.reset();
        let mut got = src.clone();
        run(&mut used, &mut got, 512);
        assert_eq!(got, want, "machine {machine}: reset after another history");
        // A second reset, after more of a different signal.
        warm(&mut used, 9_000, 0.2);
        used.reset();
        let mut again = src.clone();
        run(&mut used, &mut again, 512);
        assert_eq!(again, want, "machine {machine}: second reset");
    }
}

#[test]
fn prepare_is_a_fresh_instance() {
    // `prepare` is the C++ `prepareToPlay` on a new instance (as the render tool runs it): a used
    // instance prepared again renders what a new one does (this keeps the fidelity path).
    let src = vec![sine(8192, 0.7), sine(8192, -0.5)];
    for machine in 0..6 {
        let set = |m: &mut ReverbEngine| {
            for (k, v) in BUSY {
                m.set_param(k, *v);
            }
            m.set_param("machine", machine as f32);
        };
        let mut fresh = ReverbEngine::new();
        set(&mut fresh);
        fresh.prepare(44_100.0, 512, 2);
        let mut want = src.clone();
        run(&mut fresh, &mut want, 512);
        let mut used = ReverbEngine::new();
        used.prepare(44_100.0, 512, 2);
        used.set_param("machine", ((machine + 3) % 6) as f32);
        warm(&mut used, 20_000, 0.9);
        set(&mut used);
        used.prepare(44_100.0, 512, 2);
        let mut got = src.clone();
        run(&mut used, &mut got, 512);
        assert_eq!(got, want, "machine {machine}");
    }
}

#[test]
fn absurd_rates_stay_finite() {
    for sr in [1.0e9f32, 1.0e6, 8_000.0, 1.0, f32::NAN, f32::INFINITY, -48_000.0] {
        let mut s = ReverbEngine::new();
        s.set_param("machine", 5.0);
        s.prepare(sr, 512, 2);
        assert!(s.engine().sample_rate() <= 768_000.0);
        let mut io = vec![sine(4096, 0.5), sine(4096, 0.5)];
        run(&mut s, &mut io, 512);
        s.reset();
        run(&mut s, &mut io, 512);
        assert!(all_finite(&io), "{sr}");
    }
}

#[test]
fn restore_initial_is_a_new_engine() {
    let mut e = ReverbEngineDsp::new();
    for (i, v) in [(d::P_MACHINE, 4.0), (d::P_DECAY, 0.9), (d::P_COLOR, 0.0), (d::P_SYNC, 1.0), (d::P_INTERVAL, 2.0)] {
        e.set_parameter(i, v);
    }
    e.prepare(96_000.0);
    let mut io = vec![sine(30_000, 0.5), sine(30_000, 0.5)];
    e.process(&mut io, 2, 30_000);
    e.restore_initial();
    assert!(e == ReverbEngineDsp::new());
}

#[test]
fn bypass_passes_the_input_and_trails() {
    // Bypassed = the untouched input plus the tail when Trails is on; with Trails off, after the
    // 0.002/sample ramp, just the input.
    let src = burst(96_000, 24_000, 0.6);
    for trails in [0.0, 1.0] {
        let mut s = prepared();
        s.set_param("decay", 0.9);
        let mut io = [src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        s.set_param("bypass", 1.0);
        s.set_param("trails", trails);
        let mut io = vec![vec![0.0f32; 48_000], vec![0.0f32; 48_000]];
        run(&mut s, &mut io, 512);
        let tail = rms(&io[0][24_000..]);
        if trails > 0.5 {
            assert!(tail > 1e-4, "Trails rings on: {tail}");
        } else {
            assert!(tail < 1e-6, "no Trails: silence after the ramp ({tail})");
        }
    }
}

#[test]
fn every_machine_reverberates() {
    for machine in 0..6 {
        let mut s = prepared();
        s.set_param("machine", machine as f32);
        s.set_param("mix", 1.0);
        let len = 96_000;
        let mut io = vec![burst(len, 30_000, 0.5), burst(len, 30_000, 0.5)];
        run(&mut s, &mut io, 512);
        let ring = rms(&io[0][60_000..70_000]);
        assert!(ring > 1e-4 && ring < 1.5, "machine {machine}: ring {ring}");
        assert_ne!(io[0], io[1], "machine {machine}: stereo");
    }
}

#[test]
fn readouts_report_levels_and_settings() {
    let mut s = prepared();
    let mut r = [f32::NAN; 8];
    let mut io = vec![vec![0.5f32; 512], vec![0.25f32; 512]];
    s.process(&mut io, 512);
    s.readouts(&mut r);
    assert!((r[0] - 20.0 * 0.5f32.log10()).abs() < 1e-4);
    assert!(r[1].is_finite());
    assert!((r[2] - 20.0).abs() < 0.05, "{}", r[2]);
    assert_eq!(r[3], 120.0);
    assert_eq!(r[4], 1.0);
    assert!(r[5] >= 0.0 && r[5] < 1e-3);
    assert!(r[6].is_nan() && r[7].is_nan(), "unused slots untouched");
}
