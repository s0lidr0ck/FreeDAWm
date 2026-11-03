// The C++ literals (0.7071, 0.6215) are kept as written.
#![allow(clippy::approx_constant)]
use super::delay_config::{self as dc, Side, TapSetup};
use super::delay_engine_dsp as d;
use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
}

fn prepared() -> Delay {
    let mut s = Delay::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut Delay, io: &mut [Vec<f32>], block: usize) {
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

fn readouts(s: &Delay) -> [f32; 8] {
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    r
}

/// A setting away from every default, so a reset that left anything behind would show.
const BUSY: &[(&str, f32)] = &[
    ("time", 0.3),
    ("feedback", 0.8),
    ("mix", 0.7),
    ("routing", 2.0),
    ("trim", 0.6),
    ("level", 0.6),
    ("lowCut", 0.5),
    ("highCut", 0.6),
    ("character", 0.8),
    ("modRate", 0.7),
    ("modDepth", 0.5),
    ("diffusion", 0.4),
    ("duck", 0.3),
    ("swing", 0.7),
    ("pitch", 2.0),
    ("width", 0.8),
    ("threshold", 0.2),
    ("focus", 0.3),
    ("noise", 0.5),
    ("filtMod", 0.4),
    ("modSync", 1.0),
];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = DELAY_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            "time",
            "feedback",
            "mix",
            "style",
            "routing",
            "bypass",
            "trim",
            "level",
            "sync",
            "division",
            "lowCut",
            "highCut",
            "character",
            "modRate",
            "modDepth",
            "offset",
            "heads",
            "diffusion",
            "duck",
            "swing",
            "pitch",
            "freeze",
            "width",
            "trails",
            "tempo",
            "tempoSrc",
            "threshold",
            "focus",
            "tap",
            "reverse",
            "noise",
            "filtMod",
            "pattern",
            "modSync",
            "divisionR"
        ]
    );
    let defaults: Vec<f32> = DELAY_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(
        defaults,
        [
            0.6215, 0.35, 0.3, 0.0, 1.0, 0.0, 0.3333, 0.7071, 0.0, 6.0, 0.3544, 0.8295, 0.3, 0.4537, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.5,
            1.0, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0
        ]
    );
    assert_eq!(DELAY_PARAMS[d::P_TIME].format(0.6215), "350 ms");
    assert_eq!(DELAY_PARAMS[d::P_TIME].format(1.0), "2.00 s");
    assert_eq!(DELAY_PARAMS[d::P_TRIM].format(0.3333), "-0.0 dB");
    assert_eq!(DELAY_PARAMS[d::P_LOW_CUT].format(0.3544), "80 Hz");
    assert_eq!(DELAY_PARAMS[d::P_HIGH_CUT].format(0.8295), "12.0 k");
    assert_eq!(DELAY_PARAMS[d::P_TEMPO].format(0.4), "120.0");
    assert_eq!(DELAY_PARAMS[d::P_FEEDBACK].format(1.0), "115 %");
    assert_eq!(DELAY_PARAMS[d::P_DIVISION].choices.len(), 16);
    assert_eq!(DELAY_PARAMS[d::P_DIVISION_R].choices.len(), 17);
    assert_eq!(dc::division_text(6.0, 120.0), "1/8.  \u{b7}  375 ms");
}

#[test]
fn registered_with_no_latency_and_an_eight_second_tail() {
    let mut p = crate::create("fs_delay").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    p.prepare(48_000.0, 512, 2);
    assert_eq!(p.latency(), 0);
    assert_eq!(p.tail_samples(), 384_000);
}

#[test]
fn readouts_report_the_echo_time() {
    // Default: 350 ms on both lines (no diffusion or pitch in the loop), the Manual BPM.
    let mut s = prepared();
    let mut io = vec![sine(512, 0.3), sine(512, 0.3)];
    s.process(&mut io, 512);
    let r = readouts(&s);
    assert!((r[0] - 350.0).abs() < 0.05, "{}", r[0]);
    assert!((r[1] - 350.0).abs() < 0.05);
    assert_eq!(r[2], 120.0);
    assert_eq!(r[3], 0.0);
    // Diffusion takes its allpasses off the line, so the echo still comes back at 350 ms.
    s.set_param("diffusion", 1.0);
    s.process(&mut io, 512);
    let r = readouts(&s);
    assert!((r[0] - 350.0).abs() < 0.05, "{}", r[0]);
    // And the UI's own map agrees.
    let (l, rr) = dc::delay_ms(0.6215, false, 6, 1, 0.5, 0.5, 0, 120.0);
    assert!((l - 350.0).abs() < 0.05 && (rr - 350.0).abs() < 0.05);
}

#[test]
fn tempo_sync_follows_transport() {
    // Stopped, playing, looping back, seeking and a tempo change between blocks: synced note values
    // follow the session tempo (the Manual BPM is the fallback with no host tempo), the output stays
    // finite, and nothing is re-made.
    let mut s = prepared();
    s.set_param("sync", 1.0);
    s.set_param("division", 7.0); // 1/4
    s.set_param("tempo", 0.0); // 40 bpm if the host tempo were ignored
    let engine_ptr: *const DelayEngineDsp = s.engine();
    let mut io = vec![sine(256, 0.5), sine(256, 0.5)];
    let mut t = Transport { tempo: 90.0, ..Transport::default() };
    let echo = |s: &mut Delay, t: &Transport, io: &mut Vec<Vec<f32>>| {
        s.set_transport(t);
        s.process(io, 256);
        assert!(all_finite(io));
        readouts(s)[0]
    };
    let q = |bpm: f32| 60_000.0 / bpm;
    // Stopped: the session tempo still sets the time.
    assert!((echo(&mut s, &t, &mut io) - q(90.0)).abs() < 0.05);
    t.playing = true;
    for blk in 0..40 {
        t.samples = blk * 256;
        t.quarters = t.samples as f64 / 48_000.0 * 1.5;
        assert!((echo(&mut s, &t, &mut io) - q(90.0)).abs() < 0.05);
    }
    // Looping back and seeking: the position jumps, the time does not.
    t.samples = 0;
    t.quarters = 0.0;
    assert!((echo(&mut s, &t, &mut io) - q(90.0)).abs() < 0.05);
    s.reset();
    t.samples = 480_000;
    assert!((echo(&mut s, &t, &mut io) - q(90.0)).abs() < 0.05);
    // A tempo change lands within the block it arrives in (the engine re-reads every 32 samples).
    t.tempo = 150.0;
    assert!((echo(&mut s, &t, &mut io) - q(150.0)).abs() < 0.05);
    // Mod Sync and the Dual R Note follow it too.
    s.set_param("routing", 3.0);
    s.set_param("divisionR", 5.0); // 1/8
    assert!((readouts_after(&mut s, &t, &mut io)[1] - q(150.0) * 0.5).abs() < 0.05);
    // No usable host tempo: the Manual BPM, as the C++.
    t.tempo = f64::NAN;
    assert!((echo(&mut s, &t, &mut io) - q(40.0)).abs() < 0.05);
    t.tempo = 5.0;
    assert!((echo(&mut s, &t, &mut io) - q(40.0)).abs() < 0.05);
    // Tempo Src Manual ignores the host.
    t.tempo = 150.0;
    s.set_param("tempoSrc", 1.0);
    assert!((echo(&mut s, &t, &mut io) - q(40.0)).abs() < 0.05);
    // Not synced: the Time knob.
    s.set_param("sync", 0.0);
    assert!((echo(&mut s, &t, &mut io) - 350.0).abs() < 0.05);
    assert_eq!(engine_ptr, s.engine() as *const DelayEngineDsp);
}

fn readouts_after(s: &mut Delay, t: &Transport, io: &mut [Vec<f32>]) -> [f32; 8] {
    s.set_transport(t);
    s.process(io, 256);
    readouts(s)
}

#[test]
fn no_transport_is_the_cpp_path() {
    // The render tool gives no transport: a module that never sees one renders bit-identically to
    // one told "no tempo" (0), so the fidelity cases cover the C++ path.
    let mut a = prepared();
    let mut b = prepared();
    for m in [&mut a, &mut b] {
        m.set_param("sync", 1.0);
        m.set_param("modSync", 1.0);
        m.set_param("modDepth", 0.6);
    }
    b.set_transport(&Transport { tempo: 0.0, ..Transport::default() });
    let src = vec![sine(8192, 0.5), sine(8192, -0.4)];
    let (mut x, mut y) = (src.clone(), src.clone());
    run(&mut a, &mut x, 512);
    run(&mut b, &mut y, 512);
    assert_eq!(x, y);
}

#[test]
fn tap_times_the_tempo() {
    // Two taps half a second apart: 120 BPM, reported once, and a synced delay follows it over the
    // host tempo (as the C++: a tap means you meant it).
    let mut s = prepared();
    s.set_param("sync", 1.0);
    s.set_param("division", 7.0);
    let t = Transport { tempo: 90.0, playing: true, ..Transport::default() };
    let mut io = vec![vec![0.0f32; 480], vec![0.0f32; 480]];
    let mut step = |s: &mut Delay, tap: f32| {
        s.set_param("tap", tap);
        s.set_transport(&t);
        s.process(&mut io, 480);
    };
    step(&mut s, 1.0);
    step(&mut s, 0.0);
    for _ in 0..48 {
        step(&mut s, 0.0);
    }
    assert_eq!(readouts(&s)[4], 0.0, "one tap is not a tempo");
    step(&mut s, 1.0);
    step(&mut s, 0.0);
    let r = readouts(&s);
    assert_eq!(r[4], 1.0);
    assert!((r[5] - 120.0).abs() < 1e-3, "{}", r[5]);
    assert!((r[2] - 120.0).abs() < 1e-3);
    assert!((r[0] - 500.0).abs() < 0.05, "{}", r[0]);
    // Touching Tempo Src hands the tempo back to the host; a reset forgets the tap.
    s.set_param("tempoSrc", 1.0);
    step(&mut s, 0.0);
    s.set_param("tempoSrc", 0.0);
    step(&mut s, 0.0);
    assert!((readouts(&s)[2] - 90.0).abs() < 1e-3);
    // Tap again: the tapped tempo is back in force until a reset forgets it.
    step(&mut s, 1.0);
    step(&mut s, 0.0);
    for _ in 0..48 {
        step(&mut s, 0.0);
    }
    step(&mut s, 1.0);
    step(&mut s, 0.0);
    assert_eq!(readouts(&s)[4], 2.0);
    assert!((readouts(&s)[2] - 120.0).abs() < 1e-3);
    assert_eq!(readouts(&s)[6], 1.0, "off the session tempo, and the UI can see it");
    // Play / Stop / Locate (a reset) keeps the tap, as `prepareToPlay` keeps the C++'s.
    s.reset();
    step(&mut s, 0.0);
    assert!((readouts(&s)[2] - 120.0).abs() < 1e-3);
    assert_eq!(readouts(&s)[6], 1.0);
    // Host (touching Tempo Src) returns to the session tempo.
    s.set_param("tempoSrc", 1.0);
    step(&mut s, 0.0);
    s.set_param("tempoSrc", 0.0);
    step(&mut s, 0.0);
    assert!((readouts(&s)[2] - 90.0).abs() < 1e-3);
    assert_eq!(readouts(&s)[6], 0.0);
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_delay");
    assert_eq!(presets.len(), 78);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &DELAY_INFO).unwrap();
        assert_eq!(&p.name, name);
        for key in ["style", "time", "feedback", "mix", "routing", "sync", "division", "heads", "pitch", "pattern", "divisionR"] {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
        assert!(!p.values.contains_key("bypass"), "{name}");
    }
}

#[test]
fn hostile_params_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
        for style in 0..6 {
            let mut s = prepared();
            s.set_param("style", style as f32);
            // Bypass stays off: a hostile value that turned it on would hide the wet path.
            for info in DELAY_PARAMS.iter().filter(|p| p.id != "style" && p.id != "bypass") {
                assert!(s.set_param(info.id, bad));
            }
            assert_eq!(s.param("bypass"), Some(0.0));
            s.set_transport(&Transport { tempo: f64::from(bad), ..Transport::default() });
            let mut io = vec![sine(8192, 0.9), sine(8192, -0.9)];
            run(&mut s, &mut io, 256);
            assert!(all_finite(&io), "style {style}, value {bad}");
        }
    }
}

#[test]
fn hostile_choice_values_stay_finite() {
    let choices: Vec<&ParamInfo> = DELAY_PARAMS.iter().filter(|p| p.unit == crate::Unit::Choice).collect();
    assert_eq!(choices.len(), 8);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.49, 0.5, 5.5, 15.6, 99.0, 1e9] {
        let mut s = prepared();
        s.set_param("feedback", 1.0);
        s.set_param("sync", 1.0);
        for c in &choices {
            assert!(s.set_param(c.id, bad));
        }
        let mut io = vec![sine(8192, 0.9), sine(8192, -0.9)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "choices at {bad}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        for style in 0..6 {
            let mut s = prepared();
            s.set_param("style", style as f32);
            s.set_param("feedback", 0.9);
            let src = [sine(8192, 0.9), sine(8192, -0.9)];
            let mut a = vec![src[0][..4096].to_vec(), src[1][..4096].to_vec()];
            run(&mut s, &mut a, 256);
            assert!(s.set_param("bypass", bad));
            let mut b = vec![src[0][4096..].to_vec(), src[1][4096..].to_vec()];
            run(&mut s, &mut b, 256);
            assert!(all_finite(&a) && all_finite(&b), "style {style}, bypass {bad}");
        }
    }
}

#[test]
fn max_settings_stay_bounded() {
    // Review Focus 2: 2 s, maximum feedback, every style and routing, with freeze, reverse, pitch,
    // all heads and +24 dB in, fed a loud square for a second then left to recirculate: finite, and
    // the loop's soft limiter keeps it bounded. Nothing grows: the lines keep their buffers.
    // Then the same at 192 kHz (all heads, Reverse, +Oct), where the fixed lines clamp hardest.
    let sq: Vec<f32> = (0..48_000).map(|i| if (i / 60) % 2 == 0 { 1.0 } else { -1.0 }).collect();
    let passes: [(f32, &[(f32, f32, f32)]); 2] =
        [(48_000.0, &[(0.0, 0.0, 0.0), (1.0, 0.0, 4.0), (0.0, 1.0, 1.0)]), (192_000.0, &[(0.0, 1.0, 4.0), (1.0, 0.0, 4.0)])];
    for (sr, combos) in passes {
        for style in 0..6 {
            for routing in 0..4 {
                for &(freeze, reverse, pitch) in combos {
                    let mut s = Delay::new();
                    s.prepare(sr, 512, 2);
                    for (k, v) in [
                        ("style", style as f32),
                        ("routing", routing as f32),
                        ("time", 1.0),
                        ("feedback", 1.0),
                        ("mix", 1.0),
                        ("trim", 1.0),
                        ("level", 1.0),
                        ("heads", 7.0),
                        ("modDepth", 1.0),
                        ("diffusion", 1.0),
                        ("character", 1.0),
                        ("noise", 1.0),
                        ("width", 1.0),
                        ("pitch", pitch),
                        ("reverse", reverse),
                    ] {
                        s.set_param(k, v);
                    }
                    let ptrs = s.engine().buffer_ptrs();
                    let mut io = vec![sq.clone(), sq.clone()];
                    run(&mut s, &mut io, 512);
                    s.set_param("freeze", freeze);
                    let mut tail = vec![vec![0.0f32; 144_000], vec![0.0f32; 144_000]];
                    run(&mut s, &mut tail, 512);
                    assert!(all_finite(&io) && all_finite(&tail), "style {style} routing {routing} freeze {freeze}");
                    let peak = tail.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
                    // Loop content <= 1 (soft limiter); the wet is trim-compensated (x 1/0.05 at most
                    // never applies at +24 dB) and Width/Level scale it by at most 2 x 2.
                    assert!(peak < 8.0, "{sr} Hz style {style} routing {routing} freeze {freeze}: peak {peak}");
                    assert_eq!(ptrs, s.engine().buffer_ptrs());
                }
            }
        }
    }
}

#[test]
fn freeze_holds_the_loop() {
    // Frozen, the repeats neither decay nor take new input.
    let mut s = prepared();
    s.set_param("time", 0.3);
    s.set_param("feedback", 0.5);
    s.set_param("mix", 1.0);
    let mut io = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut s, &mut io, 512);
    s.set_param("freeze", 1.0);
    let mut a = vec![vec![0.0f32; 96_000], vec![0.0f32; 96_000]];
    run(&mut s, &mut a, 512);
    let early = rms(&a[0][24_000..48_000]);
    let late = rms(&a[0][72_000..96_000]);
    assert!(early > 0.01 && (late / early - 1.0).abs() < 0.1, "{early} {late}");
}

#[test]
fn switching_styles_and_choices_mid_stream_is_safe() {
    // No quality setting; the model switches are style, routing, heads, pitch and pattern.
    let mut s = prepared();
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = sine(128 * 600, 0.8);
    let engine_ptr: *const DelayEngineDsp = s.engine();
    let ptrs = s.engine().buffer_ptrs();
    s.set_param("feedback", 0.9);
    for blk in 0..600 {
        s.set_param("style", (blk % 6) as f32);
        s.set_param("routing", ((blk / 2) % 4) as f32);
        s.set_param("heads", (blk % 8) as f32);
        s.set_param("pitch", ((blk / 3) % 5) as f32);
        s.set_param("pattern", ((blk / 5) % 6) as f32);
        s.set_param("reverse", ((blk / 7) % 2) as f32);
        s.set_param("sync", ((blk / 11) % 2) as f32);
        s.set_param("time", (blk % 10) as f32 / 10.0);
        for b in &mut buf {
            b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
        }
        s.process(&mut buf, 128);
        assert!(all_finite(&buf), "block {blk}");
    }
    assert_eq!(engine_ptr, s.engine() as *const DelayEngineDsp);
    assert_eq!(ptrs, s.engine().buffer_ptrs());
}

#[test]
fn mono_and_wide_buffers() {
    // Mono: one channel processed (the left path, as the C++ with one channel).
    let mut s = Delay::new();
    s.prepare(48_000.0, 512, 1);
    let mut mono = vec![sine(48_000, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);

    // Six channels: the first two processed, the rest untouched.
    let mut s = Delay::new();
    s.prepare(48_000.0, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(48_000, 0.1 * (c + 1) as f32)).collect();
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
    for style in 0..6 {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            s.set_param("style", style as f32);
            s.set_param("feedback", 0.7);
            s.set_param("diffusion", 0.5);
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
    // No counting allocator in this crate: the engine's only heap memory is its line, allpass and
    // shifter buffers, all made in `new`; their addresses and the engine's are the whole story,
    // and the host buffers keep their pointers and capacities across 1000 blocks of switching.
    let mut s = prepared();
    let engine_ptr: *const DelayEngineDsp = s.engine();
    let ptrs = s.engine().buffer_ptrs();
    let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
    let host: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        s.set_param("style", ((blk / 3) % 6) as f32);
        s.set_param("pitch", (blk % 5) as f32);
        s.set_param("tap", (blk % 2) as f32);
        s.set_transport(&Transport { tempo: 60.0 + blk as f64, playing: true, ..Transport::default() });
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, s.engine() as *const DelayEngineDsp);
    assert_eq!(ptrs, s.engine().buffer_ptrs());
    assert_eq!(host, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is put back in
    // place, never re-made, and so are its buffers.
    let mut s = prepared();
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const DelayEngineDsp = s.engine();
    let ptrs = s.engine().buffer_ptrs();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const DelayEngineDsp);
        assert_eq!(ptrs, s.engine().buffer_ptrs());
        s.process(&mut io, 512);
    }
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(engine_ptr, s.engine() as *const DelayEngineDsp);
    assert_eq!(ptrs, s.engine().buffer_ptrs());
    assert!(all_finite(&io));
}

/// A settled, non-default module: the busy settings at `style`, run for 1.5 s at 133 bpm.
fn settled(style: i32) -> Delay {
    let mut m = Delay::new();
    m.prepare(44_100.0, 512, 2);
    for (k, v) in BUSY {
        m.set_param(k, *v);
    }
    m.set_param("style", style as f32);
    m.set_param("heads", 7.0);
    m.set_param("sync", 1.0);
    m.set_transport(&Transport { tempo: 133.0, playing: true, ..Transport::default() });
    let mut warm = vec![sine(66_150, 0.9), sine(66_150, -0.3)];
    run(&mut m, &mut warm, 333);
    m
}

#[test]
fn resets_from_the_same_state_render_bit_identically() {
    // SoundCraft resets on every Play, Stop and Locate: `prepareToPlay` on the live instance. Two
    // modules with the same settings render the same 60000 samples after a reset (well past two
    // repeats, so the Tape and Oil Can drift is covered), sample for sample, however much and
    // whatever each played before the reset; every style.
    let src = vec![sine(60_000, 0.7), sine(60_000, -0.5)];
    let t = Transport { tempo: 133.0, playing: true, ..Transport::default() };
    for style in 0..6 {
        let mut a = settled(style);
        let mut b = settled(style);
        // b plays something else first, then both are reset.
        let mut other = vec![sine(23_456, 0.2), sine(23_456, 0.9)];
        run(&mut b, &mut other, 256);
        for m in [&mut a, &mut b] {
            m.reset();
            m.set_transport(&t);
        }
        let (mut x, mut y) = (src.clone(), src.clone());
        run(&mut a, &mut x, 512);
        run(&mut b, &mut y, 512);
        assert_eq!(x, y, "style {style}");
        // And a reset clears the loop: silence in, silence out.
        a.reset();
        let mut quiet = vec![vec![0.0f32; 8192], vec![0.0f32; 8192]];
        run(&mut a, &mut quiet, 512);
        assert!(quiet.iter().flatten().all(|v| v.abs() < 1e-3), "style {style}: the reset left a tail");
    }
}

#[test]
fn a_reset_seats_the_right_line_at_its_own_time() {
    // Dual, Tape, R Offset 0 (R = half of L): after a reset the right line's first repeat is at its
    // own time, not a glide in from the left's (the C++ seats both at the left's target).
    let mut s = prepared();
    for (k, v) in [("style", 3.0), ("routing", 3.0), ("offset", 0.0), ("time", 0.6), ("feedback", 0.0), ("mix", 1.0)] {
        s.set_param(k, v);
    }
    let mut warm = vec![sine(24_000, 0.5), sine(24_000, 0.5)];
    run(&mut s, &mut warm, 512);
    s.set_param("time", 0.5); // L 200 ms, R 100 ms
    s.reset();
    let r = readouts(&s);
    assert!((r[0] - 200.0).abs() < 0.5 && (r[1] - 100.0).abs() < 0.5, "{} {}", r[0], r[1]);
    let mut x = vec![0.0f32; 24_000];
    for (i, v) in x[..480].iter_mut().enumerate() {
        *v = 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin();
    }
    let mut io = vec![x.clone(), x];
    run(&mut s, &mut io, 512);
    let centre = |c: &[f32], a: usize, b: usize| {
        let (m, w) = c[a..b].iter().enumerate().fold((0.0f32, 0.0f32), |(m, w), (i, v)| (m + (a + i) as f32 * v * v, w + v * v));
        m / w
    };
    // The burst's centre is 240 samples in; each side's repeat is centred one delay later.
    let right = centre(&io[1], 2400, 7200);
    let left = centre(&io[0], 7200, 12_000);
    assert!((right - (240.0 + 4800.0)).abs() < 120.0, "right repeat centred at {right}");
    assert!((left - (240.0 + 9600.0)).abs() < 120.0, "left repeat centred at {left}");
}

#[test]
fn a_reset_does_not_glide_or_fade() {
    // Tape at 100 ms, Level and Mix away from their defaults, after playing at 600 ms: once reset,
    // the first repeat is at 100 ms (no motor swoop from the old time or from 350 ms) and the dry
    // path is already at Level (no ramp from the default).
    let mut s = prepared();
    s.set_param("style", 3.0);
    s.set_param("time", 0.75); // 631 ms
    let mut warm = vec![sine(48_000, 0.5), sine(48_000, 0.5)];
    run(&mut s, &mut warm, 512);
    s.set_param("time", 0.3485); // 100 ms
    s.set_param("level", 0.5);
    s.set_param("mix", 0.25);
    s.set_param("feedback", 0.0);
    s.reset();
    let r = readouts(&s);
    assert!((r[0] - 100.0).abs() < 0.5, "{}", r[0]);
    // A burst at 0: the dry passes at Level x 1 at once; the repeat lands at 100 ms.
    let mut x = vec![0.0f32; 24_000];
    for (i, v) in x[..480].iter_mut().enumerate() {
        *v = 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin();
    }
    let mut io = vec![x.clone(), x.clone()];
    run(&mut s, &mut io, 512);
    let level = d::knob_to_level_gain(0.5);
    assert!((io[0][12] - x[12] * level).abs() < 1e-3 * level, "no Level ramp: {} vs {}", io[0][12], x[12] * level);
    let energy = |a: usize, b: usize| io[0][a..b].iter().map(|v| v * v).sum::<f32>();
    let at = r[0] / 1000.0 * 48_000.0;
    let on = energy(at as usize, at as usize + 480);
    let early = energy(600, at as usize - 200);
    let late = energy(at as usize + 900, at as usize + 4000);
    assert!(on > 50.0 * early.max(1e-9) && on > 50.0 * late.max(1e-9), "repeat in place: {early} {on} {late}");
}

#[test]
fn restore_initial_is_a_new_engine() {
    let mut e = DelayEngineDsp::new();
    for (i, v) in [(d::P_STYLE, 3.0), (d::P_TIME, 0.1), (d::P_TRIM, 0.9), (d::P_SYNC, 1.0), (d::P_PITCH, 4.0)] {
        e.set_parameter(i, v);
    }
    e.prepare(96_000.0);
    let mut io = vec![sine(1000, 0.5), sine(1000, 0.5)];
    e.process(&mut io, 2, 1000);
    e.restore_initial();
    assert!(e == DelayEngineDsp::new());
}

#[test]
fn bypass_passes_the_input_and_trails() {
    let mut s = prepared();
    s.set_param("bypass", 1.0);
    s.set_param("trails", 0.0);
    let src = sine(48_000, 0.6);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    // The bypass crossfade starts from 0 (0.002 a sample): settled after a few thousand samples.
    let tail: Vec<f32> = io[0][24_000..].to_vec();
    assert!(tail.iter().zip(&src[24_000..]).all(|(a, b)| (a - b).abs() < 1e-4));
}

#[test]
fn every_style_echoes() {
    // Each style puts a repeat 350 ms after an impulse.
    for style in 0..6 {
        let mut s = prepared();
        s.set_param("style", style as f32);
        s.set_param("mix", 1.0);
        let mut imp = vec![0.0f32; 48_000];
        imp[100] = 0.8;
        let mut io = vec![imp.clone(), imp];
        run(&mut s, &mut io, 512);
        let at = 100 + 16_800;
        let near = rms(&io[0][at - 2400..at + 2400]);
        let before = rms(&io[0][4000..12_000]);
        assert!(near > 4.0 * before.max(1e-6), "style {style}: {near} vs {before}");
    }
}

#[test]
fn the_engine_itself_stays_finite_when_driven_hard() {
    // The output guard in `process` is belt and braces: the translated engine on its own (no
    // guard) stays finite with every loop path open and a hot square in.
    let sq: Vec<f32> = (0..48_000).map(|i| if (i / 37) % 2 == 0 { 4.0 } else { -4.0 }).collect();
    for style in 0..6 {
        let mut e = DelayEngineDsp::new();
        for (i, v) in [
            (d::P_STYLE, style as f32),
            (d::P_TRIM, 1.0),
            (d::P_FEEDBACK, 1.0),
            (d::P_CHARACTER, 1.0),
            (d::P_DIFFUSION, 1.0),
            (d::P_PITCH, 4.0),
            (d::P_MOD_DEPTH, 1.0),
            (d::P_TIME, 0.0),
        ] {
            e.set_parameter(i, v);
        }
        let mut io = vec![sq.clone(), sq.clone()];
        for k in 0..(48_000 / 512) {
            let mut blk: Vec<Vec<f32>> = io.iter().map(|c| c[k * 512..(k + 1) * 512].to_vec()).collect();
            e.process(&mut blk, 2, 512);
            for (c, b) in io.iter_mut().zip(&blk) {
                c[k * 512..(k + 1) * 512].copy_from_slice(b);
            }
        }
        assert!(all_finite(&io), "style {style}");
    }
}

// ------------------------------------------------------------------ the graph's taps

fn setup(style: i32, routing: i32, feedback: f32, ms: f32) -> TapSetup {
    TapSetup { style, routing, heads: 0, pattern: 0, reverse: false, freeze: false, feedback, ms_l: ms, ms_r: ms, loop_ms: 0.0 }
}

#[test]
fn echo_taps_follow_the_feedback_law() {
    // One head: repeats every 250 ms, each the feedback x the last.
    let taps = dc::echo_taps(&setup(0, 1, 0.5, 250.0), 1000.0, 64);
    let left: Vec<_> = taps.iter().filter(|e| e.side == Side::Left).collect();
    assert_eq!(left.len(), 4);
    for (k, e) in left.iter().enumerate() {
        assert!((e.ms - 250.0 * (k + 1) as f32).abs() < 1e-3);
        assert!((e.gain - 0.5f32.powi(k as i32)).abs() < 1e-6);
    }
}

#[test]
fn echo_taps_match_the_engine() {
    // A short 1 kHz burst through the engine (Digital, no colour, filters wide open, Stereo) comes
    // back where the taps say, at the levels they say.
    let mut s = prepared();
    for (k, v) in [("mix", 0.5), ("feedback", 0.4), ("time", 0.4), ("character", 0.0), ("lowCut", 0.0), ("highCut", 1.0)] {
        s.set_param(k, v);
    }
    let (start, burst) = (20_000usize, 960usize);
    let mut x = vec![0.0f32; 96_000];
    for (i, v) in x[start..start + burst].iter_mut().enumerate() {
        *v = 0.25 * (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin();
    }
    let mut io = vec![x.clone(), x];
    run(&mut s, &mut io, 512);
    let r = readouts(&s);
    let taps = dc::echo_taps(&TapSetup { feedback: 0.4 * 1.15, loop_ms: r[7], ..setup(0, 1, 0.0, r[0]) }, 1000.0, 64);
    let left: Vec<_> = taps.iter().filter(|e| e.side == Side::Left).collect();
    assert!(left.len() >= 3);
    let level = |ms: f32| {
        let at = start + (ms * 48.0).round() as usize;
        rms(&io[0][at + 100..at + burst - 100])
    };
    let quiet = level(left[0].ms * 0.5);
    let first = level(left[0].ms);
    assert!(first > 20.0 * quiet.max(1e-6), "{first} vs {quiet}");
    for e in &left[1..3] {
        let ratio = level(e.ms) / first;
        assert!((ratio - e.gain / left[0].gain).abs() < 0.01, "{ratio} vs {}", e.gain / left[0].gain);
    }
}

#[test]
fn echo_taps_time_heads_through_the_loop_as_the_engine_does() {
    // Drum, heads 1+3, with diffusion in the loop: the engine reads head h at target x (h + 1) and
    // the loop's own delay is passed once, so head 3 comes back at 3 x target + loopComp (not 3 x
    // the echo time). Measured with a short burst.
    let mut s = prepared();
    for (k, v) in [("style", 4.0), ("heads", 5.0), ("diffusion", 0.3), ("time", 0.3485), ("feedback", 0.0), ("mix", 1.0), ("character", 0.0)] {
        s.set_param(k, v);
    }
    // Seated at the settings by a reset once running (a fresh instance glides in from 350 ms, as
    // the C++ does).
    let mut warm = vec![vec![0.0f32; 512], vec![0.0f32; 512]];
    s.process(&mut warm, 512);
    s.reset();
    let mut x = vec![0.0f32; 48_000];
    for (i, v) in x[1000..1240].iter_mut().enumerate() {
        *v = 0.3 * (std::f32::consts::TAU * 800.0 * i as f32 / 48_000.0).sin();
    }
    let mut io = vec![x.clone(), x];
    run(&mut s, &mut io, 512);
    let r = readouts(&s);
    assert!(r[7] > 5.0, "diffusion puts {} ms in the loop", r[7]);
    let t = TapSetup { heads: 5, loop_ms: r[7], ..setup(4, 0, 0.0, r[0]) };
    let taps = dc::echo_taps(&t, 1000.0, 16);
    assert_eq!(taps.len(), 2);
    let target = r[0] - r[7];
    assert!((taps[1].ms - (3.0 * target + r[7])).abs() < 1e-3);
    // The burst's energy centre lands where each tap says (within the diffusion's smear).
    for e in &taps {
        let c = 1000.0 + e.ms * 48.0;
        let (a, b) = (c as usize - 600, c as usize + 3000);
        let (m, w) = io[0][a..b].iter().enumerate().fold((0.0f32, 0.0f32), |(m, w), (i, v)| (m + (a + i) as f32 * v * v, w + v * v));
        let centre = m / w;
        assert!((centre - c).abs() < 450.0, "tap at {} ms: centre {} vs {}", e.ms, centre, c);
    }
}

#[test]
fn echo_taps_heads_patterns_and_freeze() {
    // Tape, heads 1+3: two repeats a cycle, equal power, at 1x and 3x.
    let t = TapSetup { heads: 5, ..setup(3, 1, 0.0, 100.0) };
    let taps = dc::echo_taps(&t, 1000.0, 64);
    let left: Vec<_> = taps.iter().filter(|e| e.side == Side::Left).collect();
    assert_eq!(left.len(), 2);
    assert!((left[0].ms - 100.0).abs() < 1e-3 && (left[1].ms - 300.0).abs() < 1e-3);
    assert!((left[0].gain - 0.5f32.sqrt()).abs() < 1e-6);
    // Digital ignores Heads.
    let t = TapSetup { heads: 5, ..setup(0, 1, 0.0, 100.0) };
    assert_eq!(dc::echo_taps(&t, 1000.0, 64).iter().filter(|e| e.side == Side::Left).count(), 1);
    // Gallop: output taps at 1/2 and 3/4 of the time, then the full-time repeat.
    let t = TapSetup { pattern: 1, ..setup(0, 0, 0.0, 400.0) };
    let taps = dc::echo_taps(&t, 1000.0, 64);
    let ms: Vec<f32> = taps.iter().map(|e| e.ms).collect();
    assert_eq!(ms, [200.0, 300.0, 400.0]);
    assert!(taps.iter().all(|e| e.side == Side::Both));
    // Ping-pong alternates sides, left first.
    let taps = dc::echo_taps(&setup(0, 2, 0.5, 200.0), 1000.0, 64);
    let sides: Vec<Side> = taps.iter().map(|e| e.side).collect();
    assert_eq!(sides[..4], [Side::Left, Side::Right, Side::Left, Side::Right]);
    assert!((taps[1].ms - 400.0).abs() < 1e-3);
    // Freeze: the repeats never fall; capped by the limiter.
    let t = TapSetup { freeze: true, ..setup(0, 0, 0.1, 100.0) };
    let taps = dc::echo_taps(&t, 1000.0, 64);
    assert_eq!(taps.len(), 10);
    assert!(taps.iter().all(|e| e.gain == 1.0));
    // Over-unity feedback is capped, never infinite; garbage is no taps or finite ones.
    let taps = dc::echo_taps(&setup(4, 3, 1.15, 50.0), 2000.0, 128);
    assert!(taps.iter().all(|e| e.gain.is_finite() && e.gain <= 1.0));
    assert!(dc::echo_taps(&setup(0, 1, f32::NAN, f32::NAN), 1000.0, 64).iter().all(|e| e.gain.is_finite()));
    assert!(dc::echo_taps(&setup(0, 1, 0.5, 100.0), f32::NAN, 64).is_empty());
    // The decay envelope falls.
    let env = dc::decay_envelope(&dc::echo_taps(&setup(0, 1, 0.7, 100.0), 1000.0, 64));
    assert!(env.windows(2).all(|w| w[1].1 <= w[0].1));
}

#[test]
fn what_applies_matches_the_editor() {
    let a = dc::what_applies(3, 1, false, 0, false, 0);
    assert!(a.heads && a.time && !a.division && !a.swing && !a.offset && !a.division_r);
    assert!(!dc::what_applies(3, 1, false, 1, false, 0).heads);
    assert!(!dc::what_applies(0, 1, false, 0, false, 0).heads);
    let a = dc::what_applies(0, 3, true, 0, false, 2);
    assert!(a.division && !a.time && a.division_r && !a.offset);
    assert!(dc::what_applies(0, 2, false, 0, false, 0).swing);
    assert_eq!(dc::character_label(2), "Grit");
    assert_eq!(dc::character_label(9), "Character");
}
