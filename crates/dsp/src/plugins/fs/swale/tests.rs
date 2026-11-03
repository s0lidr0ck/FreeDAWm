use super::*;
use crate::plugins::fs::{FsPreset, factory_presets};
use std::time::Instant;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
}

/// Separated plucked notes (a quarter second apart, decaying fast), so the trigger fires.
fn plucks(len: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let t = (i % 12_000) as f32 / 48_000.0;
            let env = (-t * 18.0).exp();
            let x = i as f32 / 48_000.0;
            amp * env * ((std::f32::consts::TAU * 196.0 * x).sin() + 0.4 * (std::f32::consts::TAU * 392.0 * x).sin())
        })
        .collect()
}

fn prepared() -> Swale {
    let mut s = Swale::new();
    s.prepare(48_000.0, 512, 2);
    s
}

fn run(s: &mut Swale, io: &mut [Vec<f32>], block: usize) {
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

fn transport(playing: bool, tempo: f64, quarters: f64) -> Transport {
    Transport { playing, tempo, quarters, numerator: 4, denominator: 4, ..Transport::default() }
}

const HOSTILE: [f32; 9] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0];
const CHOICES: [&str; 15] = [
    "mode", "curve", "vca", "atkSync", "atkNote", "holdSync", "holdNote", "relSync", "relNote", "rate", "peakBeat", "tempoSrc", "phrase", "bypass",
    "bpm",
];

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    let ids: Vec<&str> = SWALE_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            "attack",
            "hold",
            "release",
            "sens",
            "mix",
            "level",
            "mode",
            "bypass",
            "trim",
            "curve",
            "retrig",
            "detHp",
            "floorAmt",
            "bloom",
            "vca",
            "alternate",
            "atkSync",
            "atkNote",
            "holdSync",
            "holdNote",
            "relSync",
            "relNote",
            "rate",
            "peakBeat",
            "tempoSrc",
            "bpm",
            "phrase"
        ]
    );
    let defaults: Vec<f32> = SWALE_PARAMS.iter().map(|p| p.default).collect();
    assert_eq!(
        defaults,
        [
            0.55, 0.0, 0.5, 0.5, 1.0, 0.6667, 0.0, 0.0, 0.3333, 1.0, 0.7, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 9.0, 0.0, 18.0, 0.0, 12.0, 6.0, 1.0, 0.0,
            0.4, 1.0
        ]
    );
    let names: Vec<&str> = SWALE_PARAMS.iter().map(|p| p.name).collect();
    assert_eq!(names[10], "Retrigger");
    assert_eq!(names[11], "Det Low Cut");
    assert_eq!(names[26], "Swell");
    assert_eq!(SWALE_PARAMS[d::P_ATK_NOTE].choices.len(), 18);
    assert_eq!(SWALE_PARAMS[d::P_HOLD_NOTE].choices.len(), 19);
    assert_eq!(SWALE_PARAMS[d::P_HOLD_NOTE].choices[18], "None");
    assert_eq!(SWALE_PARAMS[d::P_RATE].choices[6], "1/2");
    assert_eq!(SWALE_PARAMS[d::P_BYPASS].group, "", "the engine's bypass is hidden");
    // formatValue, through the engine's own maps
    assert_eq!(SWALE_PARAMS[d::P_ATTACK].format(0.55), "198 ms");
    assert_eq!(SWALE_PARAMS[d::P_ATTACK].format(1.0), "4.00 s");
    assert_eq!(SWALE_PARAMS[d::P_HOLD].format(0.0), "0 ms");
    assert_eq!(SWALE_PARAMS[d::P_HOLD].format(0.5), "1.00 s");
    assert_eq!(SWALE_PARAMS[d::P_SENS].format(0.5), "-43 dB");
    assert_eq!(SWALE_PARAMS[d::P_MIX].format(1.0), "100 %");
    assert_eq!(SWALE_PARAMS[d::P_LEVEL].format(0.6667), "+0.0 dB");
    assert_eq!(SWALE_PARAMS[d::P_LEVEL].format(1.0), "+12.0 dB");
    assert_eq!(SWALE_PARAMS[d::P_TRIM].format(0.0), "-12.0 dB");
    assert_eq!(SWALE_PARAMS[d::P_RETRIG].format(0.7), "7.0");
    assert_eq!(SWALE_PARAMS[d::P_DET_HP].format(0.3), "53 Hz");
    assert_eq!(SWALE_PARAMS[d::P_FLOOR].format(0.0), "Silent");
    assert_eq!(SWALE_PARAMS[d::P_FLOOR].format(0.7), "-26 dB");
    assert_eq!(SWALE_PARAMS[d::P_BPM].format(0.4), "120");
    assert_eq!(swale_config::format_value(d::P_HOLD_NOTE, 18.0), "None");
    assert_eq!(swale_config::format_value(d::P_RATE, 99.0), "1/32");
}

#[test]
fn engine_maps_match_the_header() {
    assert_eq!(d::note_beats(0), 16.0);
    assert_eq!(d::note_beats(9), 1.0);
    assert_eq!(d::note_beats(5), 8.0f32 / 3.0);
    assert_eq!(d::note_beats(18), 0.0);
    assert_eq!(d::note_beats(-4), 16.0);
    assert_eq!(d::note_beats(99), 0.0);
    assert_eq!(d::knob_to_bpm(0.4), 120.0);
    assert!((d::knob_to_stage_sec(0.0) - 0.005).abs() < 1e-15);
    assert!((d::knob_to_stage_sec(1.0) - 4.0).abs() < 1e-12);
    assert_eq!(d::knob_to_hold_sec(1.0), 4.0);
    for c in 0..4 {
        assert!(d::shape(c, 0.0).abs() < 1e-3, "curve {c}");
        assert!((d::shape(c, 1.0) - 1.0).abs() < 1e-3, "curve {c}");
        assert_eq!(d::shape(c, 2.0), d::shape(c, 1.0));
    }
    // Exp starts slow, Log fast.
    assert!(d::shape(1, 0.5) < 0.5 && d::shape(2, 0.5) > 0.5);
    let t = swale_config::stage_times(2, 120.0, false, 0.55, 9, false, 0.0, 18, false, 0.5, 12, 15);
    assert!(t.fitted, "a 1/16 Rate cannot hold the default shape");
    assert!((t.attack + t.hold + t.release - t.cycle).abs() < 1e-12);
}

#[test]
fn registered_with_no_latency_and_a_short_tail() {
    let mut p = crate::create("fs_swale").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    for mode in 0..3 {
        p.set_param("mode", mode as f32);
        p.prepare(48_000.0, 512, 2);
        let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
        p.process(&mut io, 512);
        assert_eq!(p.latency(), 0);
        assert_eq!(p.tail_samples(), 960);
    }
    p.prepare(96_000.0, 512, 2);
    assert_eq!(p.tail_samples(), 1920);
}

#[test]
fn pick_mode_rests_closed_and_swells_in() {
    // Pick, Per Note: the pick never gets through, the note rises over Attack.
    let mut s = prepared();
    s.set_param("phrase", 0.0);
    s.set_param("attack", 0.3); // ~37 ms
    s.set_param("curve", 0.0);
    let src = plucks(48_000, 0.5);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 256);
    assert!(all_finite(&io));
    // The first millisecond after the first pick is all but silent; later it opens.
    let head = rms(&io[0][..48]);
    let body = rms(&io[0][2400..4800]);
    assert!(head < 0.02 * rms(&src[..48]).max(1e-6) + 1e-4, "the pick got through: {head}");
    assert!(body > 0.2 * rms(&src[2400..4800]), "never opened: {body}");
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert!(r[4] >= 3.0, "a swell per pick: {r:?}");
}

#[test]
fn rhythmic_mode_repeats_per_rate_without_input() {
    // Rhythmic at 120 BPM (no host: the BPM knob), Rate 1/4: one swell every 0.5 s.
    let mut s = prepared();
    s.set_param("mode", 2.0);
    s.set_param("rate", 9.0);
    s.set_param("attack", 0.2);
    s.set_param("release", 0.2);
    let mut io = vec![sine(96_000, 0.5), sine(96_000, 0.5)];
    run(&mut s, &mut io, 512);
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert!((3.0..=5.0).contains(&r[4]), "about four swells in two seconds: {r:?}");
    assert_eq!(r[5], 120.0);
    assert!((s.engine().stage_secs().3 - 0.5).abs() < 1e-12);
}

#[test]
fn readouts_report_the_envelope_detector_triggers_and_tempo() {
    let mut s = prepared();
    s.set_param("mode", 1.0); // One-Shot
    s.set_param("attack", 0.0);
    s.set_param("hold", 0.5);
    let mut io = vec![plucks(24_000, 0.6), plucks(24_000, 0.6)];
    run(&mut s, &mut io, 512);
    let mut r = [f32::NAN; 8];
    s.readouts(&mut r);
    assert!((r[0] - gain_to_db(0.0)).is_finite() || r[0].is_finite());
    assert!(r[1].is_finite());
    assert!(r[2] > 0.9 && r[2] <= 1.0001, "open in the hold: {r:?}");
    assert!(r[3] > 0.0, "the detector sees the notes: {r:?}");
    assert_eq!(r[4], 2.0, "two picks in half a second: {r:?}");
    assert_eq!(r[5], 120.0);
    s.set_transport(&transport(true, 90.0, 0.0));
    run(&mut s, &mut io, 512);
    s.readouts(&mut r);
    assert_eq!(r[5], 90.0);
}

#[test]
fn params_are_pushed_only_when_they_move() {
    // Mode's setter resets the state machine when the mode changes: re-sending the same value
    // every block changes nothing, and a real change mid-swell sends the swell into release.
    let src = plucks(48_000, 0.6);
    let block = 480;
    let render = |resend: bool, switch_at: Option<usize>| {
        let mut s = prepared();
        s.set_param("mode", 1.0);
        s.set_param("hold", 0.3);
        let mut out = Vec::new();
        for (i, chunk) in src.chunks(block).enumerate() {
            let mut x = vec![chunk.to_vec(), chunk.to_vec()];
            if resend {
                s.set_param("mode", 1.0);
            }
            if switch_at == Some(i) {
                s.set_param("mode", 0.0);
            }
            s.process(&mut x, chunk.len());
            out.extend_from_slice(&x[0]);
        }
        out
    };
    let a = render(false, None);
    assert_eq!(a, render(true, None));
    assert_ne!(a, render(false, Some(20)));
}

#[test]
fn every_factory_preset_parses() {
    let presets = factory_presets("fs_swale");
    assert_eq!(presets.len(), 40);
    for (name, json) in presets {
        let p = FsPreset::parse(json, &SWALE_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert!(!p.values.contains_key("bypass"), "{name}");
        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        for key in raw["values"].as_object().unwrap().keys() {
            assert!(p.values.contains_key(key), "{name}: {key} did not resolve");
        }
        for key in ["mode", "attack", "curve", "vca", "rate", "phrase", "holdNote"] {
            assert!(p.values.contains_key(key), "{name}: {key}");
        }
    }
    let stutter = presets.iter().find(|(n, _)| *n == "32nd Stutter").unwrap();
    let p = FsPreset::parse(stutter.1, &SWALE_INFO).unwrap();
    assert_eq!(p.values["mode"], 2.0);
    assert_eq!(p.values["rate"], 17.0);
    assert_eq!(p.values["holdNote"], 18.0);
    assert_eq!(p.values["phrase"], 0.0);
}

#[test]
fn hostile_params_stay_finite() {
    // Bypass stays off (it would hide the wet path); the choices are set per case.
    for bad in HOSTILE {
        for mode in 0..3 {
            for vca in 0..2 {
                let mut s = prepared();
                s.set_param("mode", mode as f32);
                s.set_param("vca", vca as f32);
                for info in SWALE_PARAMS.iter().filter(|p| !CHOICES.contains(&p.id) && p.id != "bypass") {
                    assert!(s.set_param(info.id, bad));
                }
                assert_eq!(s.param("bypass"), Some(0.0));
                let mut io = vec![plucks(9600, 0.9), sine(9600, -0.9)];
                run(&mut s, &mut io, 256);
                assert!(all_finite(&io), "mode {mode}, vca {vca}, value {bad}");
                assert!(io.iter().flatten().all(|v| v.abs() < 100.0), "mode {mode}: past the guard");
            }
        }
    }
}

#[test]
fn hostile_choice_params_stay_finite() {
    for bad in HOSTILE {
        let mut s = prepared();
        s.set_param("bloom", 1.0);
        s.set_param("alternate", 1.0);
        for id in CHOICES.iter().filter(|id| **id != "bypass") {
            assert!(s.set_param(id, bad));
        }
        assert_eq!(s.param("bypass"), Some(0.0));
        let mut io = vec![plucks(9600, 0.9), sine(9600, -0.9)];
        run(&mut s, &mut io, 256);
        assert!(all_finite(&io), "value {bad}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
        for mode in 0..3 {
            let mut s = prepared();
            s.set_param("mode", mode as f32);
            s.set_param("vca", 1.0);
            let src = [plucks(4096, 0.9), sine(4096, -0.9)];
            let mut a = vec![src[0][..2048].to_vec(), src[1][..2048].to_vec()];
            run(&mut s, &mut a, 256);
            assert!(s.set_param("bypass", bad));
            let mut b = vec![src[0][2048..].to_vec(), src[1][2048..].to_vec()];
            run(&mut s, &mut b, 256);
            assert!(all_finite(&a) && all_finite(&b), "mode {mode}, bypass {bad}");
        }
    }
}

#[test]
fn hostile_transport_stays_finite() {
    for (tempo, quarters) in
        [(f64::NAN, 0.0), (f64::INFINITY, 1.0), (-120.0, f64::NAN), (1e12, 1e12), (0.0, -5.0), (19.9, 3.0), (999.0, f64::NEG_INFINITY), (20.0, 1e300)]
    {
        for mode in 0..3 {
            let mut s = prepared();
            s.set_param("mode", mode as f32);
            s.set_param("atkSync", 1.0);
            s.set_param("relSync", 1.0);
            let io = [plucks(4096, 0.7), plucks(4096, 0.7)];
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
    // Rhythmic, Rate 1/4 (one beat per cycle), synced Attack: everything follows the host.
    let sr = 48_000.0;
    let block = 512usize;
    let mut s = prepared();
    s.set_param("mode", 2.0);
    s.set_param("rate", 9.0);
    s.set_param("atkSync", 1.0);
    s.set_param("atkNote", 12.0); // 1/8: half a beat
    let silence = vec![vec![0.0f32; block], vec![0.0f32; block]];
    let step = |s: &mut Swale, t: Transport| {
        s.set_transport(&t);
        let mut io = silence.clone();
        s.process(&mut io, block);
        assert!(all_finite(&io));
    };
    let mut r = [0.0f32; 8];

    // Stopped, host tempo 150: the clock free-runs at the host's rate (no grid lock), and the
    // stage lengths follow the host tempo.
    step(&mut s, transport(false, 150.0, 7.3));
    let p0 = s.engine().phase();
    step(&mut s, transport(false, 150.0, 7.3));
    let per_block = 150.0 / 60.0 * block as f64 / sr;
    assert!(wrap_diff(s.engine().phase(), p0 + per_block).abs() < 1e-6, "stopped: free-runs at 150 BPM");
    assert!((s.engine().stage_secs().0 - 0.5 * 60.0 / 150.0).abs() < 1e-9, "1/8 at 150 BPM");
    for _ in 0..4 {
        step(&mut s, transport(false, 150.0, 7.3));
    }
    s.readouts(&mut r);
    assert_eq!(r[5], 150.0, "the host tempo is in use while stopped");

    // Playing: the clock is pulled onto the grid (ppq / beats, fractional part) at the block start.
    let mut q = 10.25;
    s.set_transport(&transport(true, 120.0, q));
    assert!((s.engine().phase() - 0.25).abs() < 1e-12, "locked to the bar position");
    let mut io = silence.clone();
    s.process(&mut io, block);
    // ... and runs at the host tempo, so the next block start agrees with the grid (no re-snap).
    q += 120.0 / 60.0 * block as f64 / sr;
    let before = s.engine().phase();
    assert!(wrap_diff(before, q).abs() < 1e-6, "{before} vs {q}");
    s.set_transport(&transport(true, 120.0, q));
    assert_eq!(s.engine().phase(), before, "inside the 0.01 tolerance: no jump");

    // Seek forward, then a loop back: the next block snaps to the new position.
    s.set_transport(&transport(true, 120.0, 33.6));
    assert!((s.engine().phase() - 0.6).abs() < 1e-9, "sought to beat 33.6");
    step(&mut s, transport(true, 120.0, 33.6));
    s.set_transport(&transport(true, 120.0, 4.9));
    assert!((s.engine().phase() - 0.9).abs() < 1e-9, "looped back to beat 4.9");
    // A DAW seek resets the module first: the clock restarts and snaps on the next block.
    s.reset();
    assert_eq!(s.engine().phase(), 0.0);
    s.set_transport(&transport(true, 120.0, 21.4));
    assert!((s.engine().phase() - 0.4).abs() < 1e-9, "after a reset: on the grid again");

    // A tempo change: the rate and the stage lengths follow from the next block on.
    s.set_transport(&transport(true, 60.0, 4.9));
    let a = s.engine().phase();
    let mut io = silence.clone();
    s.process(&mut io, block);
    let got = wrap_diff(s.engine().phase(), a);
    assert!((got - block as f64 / sr).abs() < 1e-6, "60 BPM rate: {got}");
    assert!((s.engine().stage_secs().0 - 0.5).abs() < 1e-9, "1/8 at 60 BPM");
    assert!((s.engine().stage_secs().3 - 1.0).abs() < 1e-9, "1/4 at 60 BPM");
    for i in 1..5 {
        step(&mut s, transport(true, 60.0, 4.9 + i as f64 * block as f64 / sr));
    }
    s.readouts(&mut r);
    assert_eq!(r[5], 60.0, "the tempo in use: {r:?}");

    // The swell tops land on the beat with Peak on Beat: at beat 0.0 the shape is at the top.
    let mut s = prepared();
    s.set_param("mode", 2.0);
    s.set_param("rate", 9.0);
    s.set_param("attack", 0.3);
    s.set_param("release", 0.3);
    s.set_param("curve", 0.0);
    let mut out = Vec::new();
    for blk in 0..800 {
        s.set_transport(&transport(true, 120.0, (blk * 120) as f64 * 2.0 / sr));
        let mut io = vec![vec![0.5f32; 120], vec![0.5f32; 120]];
        s.process(&mut io, 120);
        out.extend_from_slice(&io[0]);
    }
    // Beats fall every 24000 samples at 120 BPM; half-way between them the shape is shut.
    for beat in 1..4 {
        let at = out[beat * 24_000];
        assert!(at > 0.45, "beat {beat}: on the beat the swell is at its top ({at})");
        assert!(out[beat * 24_000 + 12_000] < 0.05, "beat {beat}: shut between beats");
    }

    // Manual tempo source (the C++ path, hidden in the DAW): the host grid is ignored.
    let mut s = prepared();
    s.set_param("mode", 2.0);
    s.set_param("tempoSrc", 1.0);
    step(&mut s, transport(true, 120.0, 13.5));
    s.set_transport(&transport(true, 120.0, 2.37));
    let p = s.engine().phase();
    assert!((p - 0.185).abs() > 0.01, "Manual: not pulled onto the host grid ({p})");
    // No transport at all (the render tool's case): the BPM knob.
    let mut s = prepared();
    s.set_param("mode", 2.0);
    s.set_param("bpm", 0.0);
    let mut io = vec![vec![0.0f32; 48_000], vec![0.0f32; 48_000]];
    run(&mut s, &mut io, 512);
    s.readouts(&mut r);
    assert_eq!(r[5], 40.0);
}

#[test]
fn switching_quality_and_model_mid_stream_is_safe() {
    // No quality setting: mode, VCA, curve, phrase and the syncs are what switch.
    let mut s = prepared();
    s.set_param("bloom", 0.8);
    s.set_param("alternate", 1.0);
    let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
    let src = plucks(128 * 700, 0.9);
    for blk in 0..700 {
        s.set_param("mode", (blk % 3) as f32);
        s.set_param("vca", ((blk / 2) % 2) as f32);
        s.set_param("curve", ((blk / 3) % 4) as f32);
        s.set_param("phrase", ((blk / 5) % 2) as f32);
        s.set_param("atkSync", ((blk / 7) % 2) as f32);
        s.set_param("rate", (blk % 18) as f32);
        s.set_param("floorAmt", (blk % 11) as f32 / 10.0);
        s.set_transport(&transport(blk % 2 == 0, 80.0 + blk as f64, blk as f64 * 0.37));
        for b in &mut buf {
            b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
        }
        s.process(&mut buf, 128);
        assert!(all_finite(&buf), "block {blk}");
        assert!(buf.iter().flatten().all(|v| v.abs() < 100.0), "block {blk}");
    }
}

#[test]
fn mono_and_wide_buffers() {
    // Mono: one channel processed (Alternate ignored, so every swell is heard).
    let mut s = Swale::new();
    s.prepare(48_000.0, 512, 1);
    s.set_param("alternate", 1.0);
    s.set_param("mode", 2.0);
    let mut mono = vec![sine(48_000, 0.5)];
    let orig = mono.clone();
    run(&mut s, &mut mono, 512);
    assert!(all_finite(&mono));
    assert_ne!(mono, orig);
    // Every half-second cycle swells (Alternate would silence every other one in stereo).
    for k in 0..2 {
        assert!(rms(&mono[0][k * 24_000..(k + 1) * 24_000]) > 0.05, "cycle {k}");
    }

    // Six channels: the first two processed, the rest untouched.
    let mut s = Swale::new();
    s.prepare(48_000.0, 512, 6);
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| plucks(9600, 0.1 * (c + 1) as f32)).collect();
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
    for (mode, vca, bloom) in [(0, 0, 0.0), (1, 1, 0.5), (2, 0, 1.0), (0, 1, 1.0)] {
        let time = |signal_s: usize, silence_s: usize| {
            let mut s = prepared();
            s.set_param("mode", mode as f32);
            s.set_param("vca", vca as f32);
            s.set_param("bloom", bloom);
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
        cases.push((mode, vca, tail, full));
        assert!(tail < 4.0 * full, "mode {mode}, vca {vca}: silence tail {tail:.3}s vs signal {full:.3}s");
    }
    assert_eq!(cases.len(), 4);
}

#[test]
fn denormal_levels_never_trigger() {
    // The C++ runs under flush-to-zero; here denormals survive. No decision can see them: a
    // signal far below every threshold (and its decaying followers) never fires a swell.
    let mut s = prepared();
    s.set_param("sens", 1.0); // the most sensitive line, -66 dB
    s.set_param("retrig", 0.0);
    let tiny: Vec<f32> = (0..96_000).map(|i| if i % 2 == 0 { 1e-39 } else { -1e-39 }).collect();
    let mut io = vec![plucks(24_000, 0.5), plucks(24_000, 0.5)];
    run(&mut s, &mut io, 512);
    let mut r = [0.0f32; 8];
    s.readouts(&mut r);
    let before = r[4];
    let mut io = vec![tiny.clone(), tiny];
    run(&mut s, &mut io, 512);
    s.readouts(&mut r);
    assert_eq!(r[4], before, "no swell from denormal input");
    assert!(all_finite(&io));
}

#[test]
fn process_does_not_allocate() {
    // No counting allocator in this crate (`unsafe_code` is forbidden, so no `GlobalAlloc`): the
    // engine is plain data (no `Vec`) in the one box made in `new`, so the box and the host
    // buffers keeping their addresses across 1000 blocks of switching is the whole story.
    let mut s = prepared();
    let engine_ptr: *const SwaleDsp = s.engine();
    let src = plucks(256, 0.7);
    let mut io = vec![src.clone(), src.clone()];
    let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        for c in &mut io {
            c.copy_from_slice(&src);
        }
        s.set_param("mode", ((blk / 4) % 3) as f32);
        s.set_param("vca", (blk % 2) as f32);
        s.set_param("bloom", (blk % 3) as f32 * 0.4);
        s.set_transport(&transport(true, 128.0, blk as f64 * 0.1));
        s.process(&mut io, 256);
    }
    assert_eq!(engine_ptr, s.engine() as *const SwaleDsp);
    assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    // `MixEngine::reset` runs on the audio thread (Play / Stop / Locate): the engine is prepared
    // in its own box, never re-made. The box pointer is the whole check because `SwaleDsp` has
    // no heap fields (see `swale_dsp.rs`). If a `Vec` or `Box` field is ever added, this test no
    // longer proves anything and must also compare that buffer's pointer and capacity.
    let mut s = prepared();
    let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
    s.process(&mut io, 512);
    let engine_ptr: *const SwaleDsp = s.engine();
    for _ in 0..3 {
        s.reset();
        assert_eq!(engine_ptr, s.engine() as *const SwaleDsp);
        s.process(&mut io, 512);
    }
    s.prepare(96_000.0, 1024, 2);
    assert_eq!(engine_ptr, s.engine() as *const SwaleDsp);
    assert!(all_finite(&io));
}

/// A module with non-default parameters that has played (and is mid-swell) for a while.
fn warmed(mode: i32) -> Swale {
    let mut s = Swale::new();
    s.prepare(44_100.0, 512, 2);
    for (id, v) in [
        ("mode", mode as f32),
        ("attack", 0.4),
        ("hold", 0.3),
        ("release", 0.35),
        ("sens", 0.7),
        ("mix", 0.8),
        ("level", 0.9),
        ("trim", 0.6),
        ("curve", 3.0),
        ("retrig", 0.4),
        ("detHp", 0.5),
        ("floorAmt", 0.5),
        ("bloom", 0.6),
        ("vca", (mode % 2) as f32),
        ("alternate", 0.7),
        ("relSync", 1.0),
        ("relNote", 10.0),
        // 1/16: a cycle of 0.113 s at the warm-up's 133 BPM, so the test windows cross several.
        ("rate", 15.0),
        ("peakBeat", 0.0),
        ("phrase", 0.0),
    ] {
        s.set_param(id, v);
    }
    s.set_transport(&transport(true, 133.0, 17.25));
    let mut warm = vec![plucks(7000, 0.9), sine(7000, -0.3)];
    run(&mut s, &mut warm, 333);
    s
}

#[test]
fn reset_twice_from_the_same_state_renders_identically() {
    // The DAW's reset is the C++ prepareToPlay on the live instance: two resets of the same
    // state render bit for bit the same, whatever played in between.
    for mode in 0..3 {
        let src = vec![plucks(8192, 0.7), sine(8192, -0.4)];
        let mut a = warmed(mode);
        a.reset();
        let mut r = [0.0f32; 8];
        a.readouts(&mut r);
        let before = r[4];
        let mut first = src.clone();
        run(&mut a, &mut first, 512);
        if mode == 2 {
            // The window crosses Rhythmic cycle boundaries (the clock restarted at the reset).
            a.readouts(&mut r);
            assert!(r[4] - before >= 1.0, "no cycle boundary in the window: {before} -> {}", r[4]);
        }
        // Play something else, then reset again.
        let mut other = vec![sine(5000, 0.9), plucks(5000, 0.2)];
        run(&mut a, &mut other, 256);
        a.reset();
        let mut second = src.clone();
        run(&mut a, &mut second, 512);
        assert_eq!(first, second, "mode {mode}: two resets render the same");
        assert!(all_finite(&first));
        // And a second module reset from the same history renders the same too.
        let mut b = warmed(mode);
        b.reset();
        let mut third = src.clone();
        run(&mut b, &mut third, 512);
        assert_eq!(first, third, "mode {mode}");
        // A different past (Bloom fully open, then moved back) leaves nothing behind either.
        let mut c = warmed(mode);
        c.set_param("bloom", 1.0);
        let mut other = vec![plucks(6000, 0.9), plucks(6000, 0.9)];
        run(&mut c, &mut other, 300);
        c.set_param("bloom", 0.6);
        c.reset();
        let mut fourth = src.clone();
        run(&mut c, &mut fourth, 512);
        assert_eq!(first, fourth, "mode {mode}: a reset is independent of the history");
    }
}

#[test]
fn reset_keeps_the_parameters_and_glides_nowhere() {
    // After a reset the glides start at their targets (no re-glide of Level up from its
    // default), the parameters are as set, and the swell state is cleared.
    for mode in 0..3 {
        let mut s = warmed(mode);
        s.reset();
        let mut io = vec![vec![0.0f32; 1], vec![0.0f32; 1]];
        s.process(&mut io, 0);
        let (sm, target) = s.engine().glides();
        assert_eq!(sm, target, "mode {mode}: every glide at its target");
        assert_eq!(s.engine().phase(), 0.0);
        let mut r = [0.0f32; 8];
        let before = {
            s.readouts(&mut r);
            r[4]
        };
        // Mix at 0.8 and Level at +8.4 dB from the first sample: the dry path is not muted and
        // not gliding (the VCA itself rests closed, as the C++ does after prepare).
        let mut s2 = warmed(mode);
        s2.set_param("mix", 0.0);
        s2.reset();
        let mut io = vec![vec![0.5f32; 4096], vec![0.5f32; 4096]];
        run(&mut s2, &mut io, 512);
        let want = 0.5 * 10f32.powf(d::knob_to_trim_db(0.6) / 20.0) * 10f32.powf(d::knob_to_level_db(0.9) / 20.0);
        assert!((io[0][0] - want).abs() < 1e-4 * want, "mode {mode}: {} vs {want}", io[0][0]);
        assert!(io[0].iter().all(|v| (v - want).abs() < 1e-4 * want), "mode {mode}: flat from the first sample");
        s.readouts(&mut r);
        assert_eq!(r[4], before);
    }
    // The C++-faithful prepare, by contrast, starts Level's glide at the default Level.
    let mut s = Swale::new();
    s.set_param("level", 1.0);
    s.prepare(48_000.0, 512, 2);
    let mut io = vec![vec![0.0f32; 1], vec![0.0f32; 1]];
    s.process(&mut io, 0);
    let (sm, target) = s.engine().glides();
    assert!(sm[1] < target[1], "prepare glides Level as the C++ does");
}

#[test]
fn bypass_passes_the_input() {
    for mode in 0..3 {
        let mut s = prepared();
        s.set_param("mode", mode as f32);
        s.set_param("bypass", 1.0);
        let src = plucks(4800, 0.6);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        assert_eq!(io[0], src, "mode {mode}");
    }
}

#[test]
fn slow_gear_closes_to_its_floor() {
    // The FET never quite shuts: about -44 dB at rest (the Clean VCA closes to silence).
    let mut s = prepared();
    s.set_param("vca", 1.0);
    s.set_param("mode", 1.0);
    s.set_param("sens", 0.0); // -20 dB line: a quiet sine never triggers
    let src = sine(9600, 0.05);
    let mut io = vec![src.clone(), src.clone()];
    run(&mut s, &mut io, 512);
    let ratio = rms(&io[0][4800..]) / rms(&src[4800..]);
    let db = 20.0 * ratio.log10();
    assert!((-50.0..-38.0).contains(&db), "Slow Gear floor {db} dB");
}
