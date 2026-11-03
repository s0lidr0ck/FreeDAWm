use super::*;
use crate::SampleBuffer;

fn sine(len: usize, amp: f32) -> Vec<f32> {
    (0..len).map(|i| amp * (std::f32::consts::TAU * 196.0 * i as f32 / 48_000.0).sin()).collect()
}

/// A plucked-ish guitar line: decaying notes, so Auto Level has something to learn.
fn guitar(len: usize, amp: f32) -> Vec<f32> {
    let notes = [196.0f32, 247.0, 294.0, 392.0];
    (0..len)
        .map(|i| {
            let note = notes[(i / 6000) % notes.len()];
            let t = (i % 6000) as f32 / 48_000.0;
            amp * (std::f32::consts::TAU * note * t).sin() * (-t * 6.0).exp()
        })
        .collect()
}

fn prepared() -> Amp {
    let mut a = Amp::new();
    a.prepare(48_000.0, 512, 2);
    a
}

fn run(s: &mut Amp, io: &mut [Vec<f32>], block: usize) {
    let len = io[0].len();
    let mut pos = 0;
    let mut buf = vec![vec![0.0f32; block]; io.len()];
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

fn ir_asset(len: usize, rate: f64) -> Arc<dyn SampleSource> {
    let ir: Vec<f32> = (0..len).map(|i| ((i as f32) * 0.21).sin() * (-(i as f32) / (len as f32 / 6.0)).exp()).collect();
    Arc::new(ImpulseAsset::new(&ir, 48_000.0, rate).unwrap())
}

#[test]
fn table_matches_the_cpp_order_and_defaults() {
    assert_eq!(AMP_PARAMS.len(), 83);
    let ids: Vec<&str> = AMP_PARAMS.iter().map(|p| p.id).collect();
    assert_eq!(
        &ids[..19],
        [
            "amp",
            "input",
            "volume",
            "bright",
            "treble",
            "middle",
            "bass",
            "reverb",
            "speed",
            "intensity",
            "master",
            "pullBoost",
            "volume2",
            "presence",
            "cab",
            "output",
            "quality",
            "bypass",
            "channel"
        ]
    );
    assert_eq!(ids[d::P_JTM_VOLUME], "jtmVolume");
    assert_eq!(ids[d::P_AUTO_LEVEL], "autoLevel");
    assert_eq!(ids[d::P_DLX_CHANNEL], "dlxChannel");
    assert_eq!(ids[d::P_SUP_BRIGHT], "supBright");
    assert_eq!(ids[d::P_PRN_AUTO_LEVEL], "prnAutoLevel");
    assert_eq!(ids[d::P_AC_CHANNEL], "acChannel");
    assert_eq!(ids[d::P_OR_AUTO_LEVEL], "orAutoLevel");
    let def = |id: &str| AMP_INFO.param(id).unwrap().default;
    assert_eq!((def("volume"), def("master"), def("autoLevel"), def("supBass"), def("acCut"), def("orHfDrive")), (0.35, 0.8, 1.0, 0.45, 0.3, 0.3));
    assert_eq!(AMP_INFO.param("amp").unwrap().choices, AMPS);
    assert_eq!(AMP_INFO.param("orFac").unwrap().choices.len(), 6);
    assert_eq!(AMP_INFO.param("input").unwrap().format(0.75), "+12.0 dB");
    assert_eq!(AMP_INFO.param("volume").unwrap().format(0.35), "3.5");
    for (a, knobs) in KEY_KNOBS.iter().enumerate() {
        for k in knobs {
            assert_eq!(param_amp(k), Some(a), "{k}");
        }
    }
    assert_eq!(short_name("Input trim (AC30)"), "Input trim");
    assert_eq!(short_name("Oversampling"), "Oversampling");
}

#[test]
fn roles_match_the_cpp_tables() {
    use d::Role;
    assert_eq!(d::param(d::AMP_TWIN, Role::Volume), Some(d::P_VOLUME));
    assert_eq!(d::param(d::AMP_MARSHALL, Role::Reverb), None);
    assert_eq!(d::param(d::AMP_DELUXE, Role::Middle), None);
    assert_eq!(d::param(d::AMP_AC30, Role::Volume), Some(d::P_AC_BRILLIANT));
    assert_eq!(d::param(d::AMP_ORANGE, Role::Volume), Some(d::P_OR_GAIN));
    assert_eq!(d::param(99, Role::Cab), Some(d::P_CAB), "an unknown amp is the Twin");
}

#[test]
fn registered_with_32_samples_latency_and_a_three_second_tail() {
    let p = crate::create("fs_amp").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    assert_eq!(p.latency(), 32);
    assert!(p.tail_samples() >= 3 * 48_000);
    let mut a = prepared();
    a.set_param("quality", 1.0);
    let mut io = vec![sine(512, 0.3), sine(512, 0.3)];
    a.process(&mut io, 512);
    assert_eq!(a.latency(), 32, "the same at 4x");
}

#[test]
fn every_factory_preset_parses() {
    assert_eq!(PRESETS.len(), 32);
    for (name, json) in PRESETS {
        let p = crate::plugins::fs::FsPreset::parse(json, &AMP_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert!(p.values.contains_key("amp"), "{name}");
        assert!(!p.values.contains_key("bypass") && !p.values.contains_key("quality"), "{name}");
    }
    let thin = PRESETS.iter().find(|(n, _)| *n == "Orange Thin FAC").unwrap();
    let p = crate::plugins::fs::FsPreset::parse(thin.1, &AMP_INFO).unwrap();
    assert_eq!(p.values["amp"], 6.0);
    assert_eq!(p.values["orFac"], 4.0, "\"5\" is the fifth position");
}

#[test]
fn every_amp_sounds_and_stays_finite() {
    for amp in 0..NUM_AMPS {
        let mut a = prepared();
        a.set_param("amp", amp as f32);
        let mut io = vec![guitar(24_000, 0.5), guitar(24_000, 0.5)];
        run(&mut a, &mut io, 512);
        assert!(all_finite(&io), "amp {amp}");
        let r = rms(&io[0][12_000..]);
        assert!(r > 1e-3 && r < 1.0, "amp {amp}: rms {r}");
        assert_eq!(io[0], io[1], "the mono result on both channels");
    }
}

#[test]
fn hostile_params_stay_finite() {
    let hostile = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, 0.0, 1.0];
    for amp in 0..NUM_AMPS {
        let mut a = prepared();
        a.set_param("amp", amp as f32);
        for (k, v) in hostile.iter().enumerate() {
            for p in AMP_PARAMS.iter().filter(|p| p.id != "bypass" && p.id != "amp") {
                a.set_param(p.id, *v);
            }
            let mut io = vec![sine(1024, 0.9), sine(1024, 0.9)];
            run(&mut a, &mut io, 256);
            assert!(all_finite(&io), "amp {amp}, value #{k}");
        }
    }
}

#[test]
fn hostile_choice_values_stay_finite() {
    let mut a = prepared();
    for v in [f32::NAN, -3.0, 0.49, 0.51, 6.6, 99.0] {
        for p in AMP_PARAMS.iter().filter(|p| p.unit == crate::Unit::Choice) {
            a.set_param(p.id, v);
        }
        let mut io = vec![sine(1024, 0.9), sine(1024, 0.9)];
        run(&mut a, &mut io, 512);
        assert!(all_finite(&io), "{v}");
    }
}

#[test]
fn hostile_bypass_values_stay_finite() {
    let mut a = prepared();
    for v in [f32::NAN, f32::INFINITY, -1.0, 0.5, 1.0, 0.0, 7.0] {
        a.set_param("bypass", v);
        let mut io = vec![sine(1024, 0.9), sine(1024, 0.9)];
        run(&mut a, &mut io, 333);
        assert!(all_finite(&io), "{v}");
    }
}

#[test]
fn bypass_passes_the_input_delayed_by_the_latency() {
    let mut a = prepared();
    a.set_param("bypass", 1.0);
    let x = sine(2048, 0.5);
    let mut io = vec![x.clone(), x.clone()];
    run(&mut a, &mut io, 512);
    // (the C++ line is `latencySamples() + 1` long: 33 samples)
    for i in 40..2048 {
        assert!((io[0][i] - x[i - 33]).abs() < 1e-6, "{i}");
    }
}

#[test]
fn switching_quality_and_model_mid_stream_is_safe() {
    let mut a = prepared();
    let ptrs = a.convolver().unwrap().buffer_ptrs();
    a.set_asset("irPathTwin", Some(ir_asset(4000, 48_000.0)));
    for blk in 0..140 {
        a.set_param("amp", (blk % 7) as f32);
        a.set_param("quality", ((blk / 3) % 2) as f32);
        a.set_param("cab", (blk % 3) as f32);
        a.set_param("orFac", (blk % 6) as f32);
        a.set_param("acChannel", (blk % 3) as f32);
        let mut io = vec![guitar(512, 0.7), guitar(512, 0.7)];
        a.process(&mut io, 512);
        assert!(all_finite(&io), "block {blk}");
    }
    assert_eq!(ptrs, a.convolver().unwrap().buffer_ptrs());
}

#[test]
fn mono_and_wide_buffers() {
    let mut a = Amp::new();
    a.prepare(48_000.0, 512, 1);
    let mut mono = vec![guitar(4096, 0.5)];
    run(&mut a, &mut mono, 512);
    assert!(all_finite(&mono) && rms(&mono[0]) > 1e-3);
    let mut b = Amp::new();
    b.prepare(48_000.0, 512, 6);
    let marker = 0.123f32;
    let mut wide: Vec<Vec<f32>> = (0..6).map(|c| if c < 2 { guitar(4096, 0.5) } else { vec![marker; 4096] }).collect();
    run(&mut b, &mut wide, 512);
    assert!(all_finite(&wide));
    assert!(wide[2..].iter().flatten().all(|v| *v == marker), "channels past two untouched");
}

#[test]
fn silence_tail_is_cheap() {
    for amp in [d::AMP_TWIN, d::AMP_ORANGE] {
        let time = |sig_s: usize, sil_s: usize| {
            let mut a = prepared();
            a.set_param("amp", amp as f32);
            let mut x = guitar(48_000 * sig_s, 0.8);
            x.extend(std::iter::repeat_n(0.0, 48_000 * sil_s));
            let mut io = vec![x.clone(), x];
            let t = std::time::Instant::now();
            run(&mut a, &mut io, 512);
            assert!(all_finite(&io));
            t.elapsed().as_secs_f64()
        };
        let tail = time(1, 5);
        let full = time(6, 0);
        assert!(tail < 4.0 * full, "amp {amp}: silence tail {tail:.3}s vs signal {full:.3}s");
    }
}

#[test]
fn process_does_not_allocate() {
    // No counting allocator in this crate: every buffer is made in `prepare`; the engine, every
    // heap buffer inside it (each amp's networks, oversamplers, spring tank and power stage, for
    // both factors; the mono and bypass lines), its convolution state and the host buffers keep
    // their addresses and capacities across 1000 blocks of knob, amp, factor, input and IR switching.
    let mut a = prepared();
    a.set_asset("irPathTwin", Some(ir_asset(9000, 48_000.0)));
    a.set_asset("irPathJtm", Some(ir_asset(3000, 48_000.0)));
    let engine: *const AmpEngineDsp = a.engine();
    let heap = a.engine().heap_blocks();
    assert!(heap.len() > 200, "{} entries", heap.len());
    let conv = a.convolver().unwrap().buffer_ptrs();
    let mut io = vec![guitar(256, 0.7), guitar(256, 0.7)];
    let host: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
    for blk in 0..1000 {
        a.set_param("amp", ((blk / 50) % NUM_AMPS) as f32);
        a.set_param("acChannel", (blk % 3) as f32);
        a.set_param("bypass", if blk % 97 == 0 { 1.0 } else { 0.0 });
        a.set_param("cab", 1.0);
        a.set_param("jtmCab", 1.0);
        a.set_param("treble", (blk % 10) as f32 / 10.0);
        a.set_param("quality", ((blk / 250) % 2) as f32);
        a.process(&mut io, 256);
    }
    assert_eq!(engine, a.engine() as *const AmpEngineDsp);
    assert!(heap == a.engine().heap_blocks(), "a buffer inside the engine moved");
    assert_eq!(conv, a.convolver().unwrap().buffer_ptrs());
    assert_eq!(host, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
    assert!(all_finite(&io));
}

#[test]
fn reset_does_not_allocate() {
    let mut a = prepared();
    a.set_asset("irPathTwin", Some(ir_asset(9000, 48_000.0)));
    a.set_param("cab", 1.0);
    let mut io = vec![guitar(512, 0.5), guitar(512, 0.5)];
    a.process(&mut io, 512);
    let engine: *const AmpEngineDsp = a.engine();
    let heap = a.engine().heap_blocks();
    let conv = a.convolver().unwrap().buffer_ptrs();
    for amp in 0..NUM_AMPS {
        a.set_param("amp", amp as f32);
        a.reset();
        assert_eq!(engine, a.engine() as *const AmpEngineDsp);
        assert!(heap == a.engine().heap_blocks(), "amp {amp}: a buffer inside the engine moved on reset");
        assert_eq!(conv, a.convolver().unwrap().buffer_ptrs());
        a.process(&mut io, 512);
    }
    assert!(all_finite(&io));
}

/// A settled, non-default module: the Twin with an IR, the reverb up, the vibrato on, Auto Level
/// off on every amp (Auto Level's learned gain is engine state: see the next test), after `warm`
/// samples of a line.
fn settled(amp: usize, warm: usize) -> Amp {
    let mut a = Amp::new();
    a.prepare(44_100.0, 512, 2);
    a.set_asset("irPathTwin", Some(ir_asset(30_000, 44_100.0)));
    for p in AMP_PARAMS.iter().filter(|p| p.id.ends_with("utoLevel")) {
        a.set_param(p.id, 0.0);
    }
    for (k, v) in
        [("amp", amp as f32), ("cab", 1.0), ("reverb", 0.7), ("intensity", 0.6), ("volume", 0.7), ("jtmIntensity", 0.5), ("prnIntensity", 0.8)]
    {
        a.set_param(k, v);
    }
    let mut x = vec![guitar(warm, 0.6), guitar(warm, 0.6)];
    run(&mut a, &mut x, 333);
    a
}

#[test]
fn resets_from_different_histories_render_bit_identically() {
    // SoundCraft resets on every Play, Stop and Locate: `prepareToPlay` on the live instance. Two
    // modules with the same settings, one after 1 s of playing and one after 2.7 s of something
    // else, render the same 4 s after a reset (past the IR, the spring tank's decay and several
    // vibrato and tremolo cycles), sample for sample.
    let src = vec![guitar(176_400, 0.5), guitar(176_400, 0.5)];
    // After a reset, silence in gives only the power-up thump: measured per amp (see
    // `power_up_from_silence_matches_the_cpp`), allowed 50% more.
    const THUMP: [f32; NUM_AMPS] = [9.1e-6, 7.4e-4, 9.7e-4, 7.2e-4, 1.3e-3, 3.3e-4, 1.3e-4];
    for (amp, thump) in THUMP.iter().enumerate() {
        let mut a = settled(amp, 44_100);
        let mut b = settled(amp, 3_000);
        let mut other = vec![sine(117_000, 0.9), sine(117_000, -0.2)];
        run(&mut b, &mut other, 256);
        a.reset();
        b.reset();
        let (mut x, mut y) = (src.clone(), src.clone());
        run(&mut a, &mut x, 512);
        run(&mut b, &mut y, 512);
        assert!(x == y, "amp {amp}");
        // and a reset clears the tank and the convolution: silence in, silence out but for the
        // amp's own power-up settling
        a.reset();
        let mut quiet = vec![vec![0.0f32; 8192]; 2];
        run(&mut a, &mut quiet, 512);
        let (i, m) = quiet[0].iter().enumerate().fold((0, 0.0f32), |(bi, bm), (i, v)| if v.abs() > bm { (i, v.abs()) } else { (bi, bm) });
        assert!(m < 1.5 * thump, "amp {amp}: the reset left a tail: {m} at {i}; {:?}", &quiet[0][..8]);
    }
}

#[test]
fn a_reset_keeps_the_learned_auto_level_and_does_not_glide() {
    // The C++ `prepare` leaves Auto Level's state alone (the forced push only re-primes it): after
    // a reset the level is where it was, not a glide up from unity.
    let mut a = prepared();
    let mut x = vec![guitar(96_000, 0.3), guitar(96_000, 0.3)];
    run(&mut a, &mut x, 512);
    let before = *a.engine().auto_level(d::AMP_TWIN).unwrap();
    assert!((before.gain - 1.0).abs() > 0.05, "Auto Level learned something: {}", before.gain);
    a.reset();
    let after = *a.engine().auto_level(d::AMP_TWIN).unwrap();
    assert_eq!((after.gain, after.target, after.di, after.out), (before.gain, before.target, before.di, before.out));
    let mut y = vec![guitar(512, 0.3), guitar(512, 0.3)];
    a.process(&mut y, 512);
    let g = a.engine().auto_level(d::AMP_TWIN).unwrap().gain;
    assert!((g - before.gain).abs() < 0.05 * before.gain.abs(), "no glide: {} -> {g}", before.gain);
}

#[test]
fn max_settings_stay_bounded() {
    // Every amp flat out, 4x, the longest IR accepted on every amp: finite, and nothing grows.
    let mut a = prepared();
    let long = ir_asset((ir::MAX_IR_SECONDS * 48_000.0) as usize - 600, 48_000.0);
    for k in IR_KEYS {
        assert!(a.set_asset(k, Some(Arc::clone(&long))));
    }
    let ptrs = a.convolver().unwrap().buffer_ptrs();
    for p in AMP_PARAMS.iter().filter(|p| p.id != "bypass" && p.unit != crate::Unit::Choice && p.unit != crate::Unit::Toggle) {
        a.set_param(p.id, 1.0);
    }
    for p in AMP_PARAMS.iter().filter(|p| p.id.contains("Cab") || p.id == "cab") {
        a.set_param(p.id, 1.0);
    }
    a.set_param("quality", 1.0);
    for amp in 0..NUM_AMPS {
        a.set_param("amp", amp as f32);
        let mut io = vec![guitar(8192, 1.0), guitar(8192, 1.0)];
        run(&mut a, &mut io, 512);
        assert!(all_finite(&io), "amp {amp}");
        // (Auto Level's limiter holds -1 dBFS; the Output trim's +24 dB is after it)
        assert!(io[0].iter().all(|v| v.abs() < 16.0), "amp {amp}");
    }
    assert_eq!(ptrs, a.convolver().unwrap().buffer_ptrs());
    assert!(a.tail_samples() >= long.frames());
}

#[test]
fn the_ir_cabinet_uses_the_asset_and_falls_back_to_built_in() {
    let render = |a: &mut Amp| {
        let mut io = vec![guitar(9600, 0.5), guitar(9600, 0.5)];
        run(a, &mut io, 512);
        io
    };
    let mut builtin = prepared();
    let reference = render(&mut builtin);

    // IR chosen, none loaded: the built-in cabinet
    let mut a = prepared();
    a.set_param("cab", 1.0);
    assert_eq!(render(&mut a), reference);

    // a plain sample source (not an `ImpulseAsset`) or one at another rate is not usable
    let mut b = prepared();
    b.set_param("cab", 1.0);
    assert!(b.set_asset("irPathTwin", Some(Arc::new(SampleBuffer { sample_rate: 48_000.0, channels: vec![vec![0.5; 64]] }))));
    assert_eq!(render(&mut b), reference);
    let mut c = prepared();
    c.set_param("cab", 1.0);
    c.set_asset("irPathTwin", Some(ir_asset(500, 44_100.0)));
    assert_eq!(render(&mut c), reference);
    let mut r = [0.0f32; 8];
    c.readouts(&mut r);
    assert_eq!((r[5], r[7]), (0.0, 0.0), "built-in in use, no usable IR");

    // a usable one convolves, on the Twin only
    let mut e = prepared();
    e.set_param("cab", 1.0);
    e.set_asset("irPathTwin", Some(ir_asset(500, 48_000.0)));
    let with_ir = render(&mut e);
    assert!(all_finite(&with_ir) && with_ir != reference);
    e.readouts(&mut r);
    assert_eq!((r[5], r[7]), (1.0, 1.0));
    assert!(!e.set_asset("irPathNope", None), "unknown key");
}

#[test]
fn replaced_assets_come_back_for_the_host_to_drop() {
    let mut a = prepared();
    a.set_param("cab", 1.0);
    let first = ir_asset(800, 48_000.0);
    let second = ir_asset(1600, 48_000.0);
    a.set_asset("irPathTwin", Some(Arc::clone(&first)));
    let mut io = vec![guitar(512, 0.5), guitar(512, 0.5)];
    a.process(&mut io, 512);
    a.set_asset("irPathTwin", Some(Arc::clone(&second)));
    // the old one comes back at once (the slot's reference) and again after the crossfade (the
    // convolution's); meanwhile the plugin keeps one of its own
    let mut back = Vec::new();
    for _ in 0..8 {
        a.process(&mut io, 512);
        while let Some(x) = a.take_released_asset() {
            back.push(x);
        }
    }
    assert!(all_finite(&io));
    assert!(back.iter().all(|x| std::ptr::addr_eq(Arc::as_ptr(x), Arc::as_ptr(&first))));
    assert_eq!(back.len(), 2);
    drop(back);
    assert_eq!(Arc::strong_count(&first), 1, "the plugin holds none of the old one");
}

#[test]
fn tone_curves_follow_the_knobs_for_every_amp() {
    let freqs: Vec<f32> = (0..64).map(|i| 20.0 * 1000f32.powf(i as f32 / 63.0)).collect();
    for amp in 0..NUM_AMPS {
        let mut v: Vec<f32> = AMP_PARAMS.iter().map(|p| p.default).collect();
        v[d::P_AMP] = amp as f32;
        let mut lo = vec![0.0; 64];
        let mut hi = vec![0.0; 64];
        let treble = d::param(amp, d::Role::Treble).unwrap();
        v[treble] = 0.0;
        tone_curve(&v, 48_000.0, &freqs, &mut lo);
        v[treble] = 1.0;
        tone_curve(&v, 48_000.0, &freqs, &mut hi);
        assert!(lo.iter().chain(&hi).all(|x| x.is_finite()), "amp {amp}");
        // more treble lifts the top relative to the bottom
        let tilt = |c: &[f32]| c[60] - c[8];
        assert!(tilt(&hi) > tilt(&lo) + 1.0, "amp {amp}: {} vs {}", tilt(&hi), tilt(&lo));
    }
}

#[test]
fn the_ac30_curve_on_normal_skips_the_top_boost_stack() {
    let freqs: Vec<f32> = (0..64).map(|i| 20.0 * 1000f32.powf(i as f32 / 63.0)).collect();
    let curve = |input: f32, treble: f32, cut: f32| {
        let mut v: Vec<f32> = AMP_PARAMS.iter().map(|p| p.default).collect();
        v[d::P_AMP] = d::AMP_AC30 as f32;
        v[d::P_AC_CHANNEL] = input;
        v[d::P_AC_TREBLE] = treble;
        v[d::P_AC_CUT] = cut;
        let mut out = vec![0.0; 64];
        tone_curve(&v, 48_000.0, &freqs, &mut out);
        assert!(out.iter().all(|x| x.is_finite()));
        out
    };
    let tilt = |c: &[f32]| c[60] - c[8];
    // Normal: the Treble pot only loads the mixer node from the idle Top Boost side (a small
    // effect, unlike on Top Boost), and the Cut takes the top off
    let normal = (tilt(&curve(1.0, 1.0, 0.3)) - tilt(&curve(1.0, 0.0, 0.3))).abs();
    let top_boost = (tilt(&curve(0.0, 1.0, 0.3)) - tilt(&curve(0.0, 0.0, 0.3))).abs();
    assert!(normal < 0.5 * top_boost, "Treble on Normal {normal} dB vs Top Boost {top_boost} dB");
    assert!(tilt(&curve(1.0, 0.5, 1.0)) < tilt(&curve(1.0, 0.5, 0.0)) - 3.0);
    // Both: the Top Boost side is in the sum, so Treble moves it again; Top Boost as before
    assert!(tilt(&curve(2.0, 1.0, 0.3)) > tilt(&curve(2.0, 0.0, 0.3)) + 0.5);
    assert_ne!(curve(0.0, 0.5, 0.3), curve(1.0, 0.5, 0.3));
}

#[test]
fn a_block_longer_than_prepared_runs_in_chunks() {
    // `prepare` said 512; a host handing over 1300 frames gets all of them processed, exactly as
    // the same audio in blocks of 512, 512 and 276.
    let src = vec![guitar(1300 * 4, 0.6), guitar(1300 * 4, 0.6)];
    let (mut a, mut b) = (prepared(), prepared());
    let mut x = src.clone();
    run(&mut a, &mut x, 1300);
    let mut y = src.clone();
    for blk in (0..4).map(|i| i * 1300) {
        let mut pos = blk;
        for n in [512, 512, 276] {
            let mut io = vec![y[0][pos..pos + n].to_vec(), y[1][pos..pos + n].to_vec()];
            b.process(&mut io, n);
            y[0][pos..pos + n].copy_from_slice(&io[0]);
            y[1][pos..pos + n].copy_from_slice(&io[1]);
            pos += n;
        }
    }
    assert!(x == y, "chunked differs from the host's long blocks");
    assert!(x[0][1200..1300] != src[0][1200..1300], "the frames past 512 were processed");
}

#[test]
fn power_up_from_silence_matches_the_cpp() {
    // A fresh amp fed digital silence settles from its solved idle: a faint thump, then decay.
    // The C++ does the same; AmpEngine_Render on 2 s of silence (48 kHz, block 512, `--set amp=N`)
    // peaks in the first 100 ms at these levels, per amp in table order.
    const CPP_FIRST_100MS: [f32; NUM_AMPS] = [4.0e-5, 6.9e-4, 9.7e-4, 7.2e-4, 1.3e-3, 3.3e-4, 1.3e-4];
    for (amp, want) in CPP_FIRST_100MS.iter().enumerate() {
        let mut a = prepared();
        a.set_param("amp", amp as f32);
        let mut io = vec![vec![0.0f32; 96_000]; 2];
        run(&mut a, &mut io, 512);
        let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let first = peak(&io[0][..4800]);
        assert!((first / want - 1.0).abs() < 0.1, "amp {amp}: first 100 ms peak {first:.2e}, C++ {want:.2e}");
        let last = peak(&io[0][86_400..]);
        assert!(last < 1e-6, "amp {amp}: still settling after 1.8 s: {last:.2e}");
    }
}

#[test]
fn editor_meters_are_running_maxima_and_the_supply() {
    let mut s = prepared();
    let l = s.analysis().unwrap();
    assert_eq!(l.live.len(), LIVE_LEN);
    let mut io = vec![guitar(24_000, 0.5), guitar(24_000, 0.5)];
    run(&mut s, &mut io, 512);
    let live = &l.live;
    assert!(live.get(LIVE_IN_PEAK) > 0.0 && live.get(LIVE_OUT_PEAK) > 0.0 && live.get(LIVE_WATTS) > 0.0);
    assert_eq!(live.get(LIVE_B_PLUS), s.dsp.meters.b_plus);
    assert_eq!(live.get(LIVE_B_PLUS_IDLE), s.dsp.meters.b_plus_idle);
    assert!(live.take(LIVE_WATTS) >= s.dsp.meters.watts, "a running maximum");
    assert_eq!(live.get(LIVE_WATTS), 0.0, "taken");
}
