use super::bells::{fit_bells, fit_error};
use super::measure::measure_audio;
use super::profile::{ALL, Curve, FACTORY_TILTS, LOUD, MID, PREPARED_LEN, QUIET, SIDE, SUM, b64_decode, b64_encode, factory_profiles, grid_hz, grid_pos, prepare_state};
use super::*;
use crate::LinkSlab;
use crate::plugins::fs::FsPreset;

const SR: f32 = 48_000.0;
const N: usize = 4096;

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

/// Pink noise (Paul Kellet's economy filter), about -20 dBFS RMS.
fn pink(len: usize, seed: u32) -> Vec<f32> {
    let mut r = Rng(seed);
    let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
    (0..len)
        .map(|_| {
            let w = r.next();
            b0 = 0.99765 * b0 + w * 0.099_046;
            b1 = 0.96300 * b1 + w * 0.296_516_4;
            b2 = 0.57000 * b2 + w * 1.052_691_3;
            (b0 + b1 + b2 + w * 0.1848) * 0.06
        })
        .collect()
}

fn white(len: usize, seed: u32) -> Vec<f32> {
    let mut r = Rng(seed);
    (0..len).map(|_| r.next() * 0.1).collect()
}

/// A one-pole low-pass at `hz`: a darker version of `x`.
fn dark(x: &[f32], hz: f32) -> Vec<f32> {
    let a = (-std::f32::consts::TAU * hz / SR).exp();
    let mut y = 0.0f32;
    x.iter()
        .map(|v| {
            y = v * (1.0 - a) + y * a;
            y * 3.0
        })
        .collect()
}

fn stereo(x: Vec<f32>) -> Vec<Vec<f32>> {
    vec![x.clone(), x]
}

fn copyq(set: &[(&str, f32)]) -> CopyQ {
    let mut t = CopyQ::new();
    t.prepare(SR, 512, 2);
    for (id, v) in set {
        assert!(t.set_param(id, *v), "{id}");
    }
    t
}

/// Runs the channels through in blocks of `block` and takes the latency off the front.
fn run(t: &mut CopyQ, input: &[Vec<f32>], block: usize) -> Vec<Vec<f32>> {
    let lat = t.latency();
    let len = input[0].len();
    let mut padded: Vec<Vec<f32>> = input.iter().map(|c| c.iter().copied().chain(std::iter::repeat_n(0.0, lat)).collect()).collect();
    let total = len + lat;
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        let mut buf: Vec<Vec<f32>> = padded.iter().map(|c| c[pos..pos + n].to_vec()).collect();
        t.process(&mut buf, n);
        for (c, b) in padded.iter_mut().zip(&buf) {
            c[pos..pos + n].copy_from_slice(b);
        }
        pos += n;
    }
    padded.into_iter().map(|c| c[lat..].to_vec()).collect()
}

fn profile_of(x: &[Vec<f32>], name: &str) -> Profile {
    let refs: Vec<&[f32]> = x.iter().map(Vec::as_slice).collect();
    measure_audio(&refs, f64::from(SR), 0, usize::MAX, 24.0, name)
}

/// The second half of a rendering, measured.
fn tail_profile(y: &[Vec<f32>]) -> Profile {
    let half: Vec<Vec<f32>> = y.iter().map(|c| c[c.len() / 2..].to_vec()).collect();
    profile_of(&half, "out")
}

/// How unevenly `a` differs from `b` between `lo` and `hi` Hz: the standard deviation of the
/// difference, dB (0 when one is the other at another level).
fn spread(a: &Curve, b: &Curve, lo: f32, hi: f32) -> f32 {
    let d: Vec<f32> = (0..GRID_N).filter(|i| (lo..=hi).contains(&grid_hz(*i))).map(|i| a[i] - b[i]).collect();
    let mean = d.iter().sum::<f32>() / d.len() as f32;
    (d.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / d.len() as f32).sqrt()
}

fn at(hz: f32) -> usize {
    grid_pos(hz).round() as usize
}

fn state_with(targets: &[(f32, &Profile)], source: Option<&Profile>) -> Vec<u8> {
    CopyQState { targets: targets.iter().map(|(w, p)| (*w, (*p).clone())).collect(), source: source.cloned() }.write()
}

fn rms_db(x: &[f32]) -> f32 {
    10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).max(1.0e-20).log10()
}

fn live(t: &CopyQ, i: usize) -> f32 {
    t.analysis().unwrap().live.get(i)
}

const LEN: usize = 48_000 * 8;

#[test]
fn registered_listed_and_reports_its_latency() {
    let mut p = crate::create("fs_copyq").unwrap();
    assert_eq!(p.info().category, Category::FireStation);
    assert!(p.info().listed());
    assert_eq!(p.latency(), 4096);
    p.prepare(96_000.0, 512, 2);
    assert_eq!(p.latency(), 8192);
    p.prepare(192_000.0, 512, 2);
    assert_eq!(p.latency(), 16_384);
    assert!(p.set_param("phase", 1.0));
    assert_eq!(p.latency(), 0, "minimum phase has no latency");
    let e = crate::plugins::fs::engine("fs_copyq").unwrap();
    assert!(e.fidelity.is_none(), "there is no C++ original");
    assert_eq!(crate::plugin_actions("fs_copyq"), ["reset_measure"]);
    assert_eq!(COPYQ_PARAMS.len(), NUM_PARAMS);
    assert_eq!(COPYQ_PARAMS[P_BYPASS].id, "bypass");
    assert_eq!(COPYQ_PARAMS[P_BYPASS].group, "", "the engine's bypass is hidden");
    assert_eq!(COPYQ_PARAMS[P_LINK].id, "link");
    assert_eq!(COPYQ_PARAMS[P_RANGE_AMOUNT].id, "amtLow");
    assert_eq!(COPYQ_PARAMS[P_SPLIT + 2].id, "split3");
    assert_eq!(COPYQ_PARAMS[P_AMOUNT].format(-50.0), "-50%");
    assert!(KEY_KNOBS.iter().all(|k| COPYQ_INFO.param(k).is_some()));
    // The link roles the host orders by.
    let role = |v: f32| crate::link_role(&COPYQ_INFO, |id| (id == "link").then_some(v));
    assert_eq!([role(0.0), role(1.0), role(2.0), role(3.0)], [None, Some(LinkRole::Producer), Some(LinkRole::Consumer), Some(LinkRole::Peer)]);
}

#[test]
fn presets_parse_and_leave_capture_link_and_bypass_alone() {
    assert_eq!(PRESETS.len(), 12);
    for (name, json) in PRESETS {
        let p = FsPreset::parse(json, &COPYQ_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert_eq!(p.values.len(), NUM_PARAMS - PRESET_SKIPS.len(), "{name}");
        assert!(PRESET_SKIPS.iter().all(|k| !p.values.contains_key(*k)), "{name}");
        for (id, v) in &p.values {
            let info = COPYQ_INFO.param(id).unwrap();
            assert!((info.min..=info.max).contains(v), "{name}: {id} = {v}");
        }
    }
    assert_eq!(crate::plugins::fs::factory_presets("fs_copyq").len(), 12);
}

#[test]
fn a_profile_survives_its_text_and_hostile_text_is_refused() {
    let p = profile_of(&[pink(96_000, 1), dark(&pink(96_000, 2), 900.0)], "  A\u{7} name ");
    assert!(p.is_measured() && p.stereo);
    assert_eq!(p.name, "A name");
    let back = Profile::from_json(&p.to_json()).unwrap();
    assert_eq!(back.name, p.name);
    assert_eq!(back.stereo, p.stereo);
    for (a, b) in p.curves.iter().flatten().zip(back.curves.iter().flatten()) {
        assert!((a - b).abs() <= 0.006, "{a} {b}");
    }
    for (a, b) in p.level.iter().chain(&p.seconds).zip(back.level.iter().chain(&back.seconds)) {
        assert!((a - b).abs() <= 0.006);
    }
    // Not a profile, the wrong grid, the wrong length, garbage.
    assert!(Profile::from_json("").is_none());
    assert!(Profile::from_json("[1,2]").is_none());
    assert!(Profile::from_json(r#"{"format":"copyq-profile"}"#).is_none());
    let mut v = p.to_value();
    v["grid"]["points"] = serde_json::json!(64);
    assert!(Profile::from_value(&v).is_none());
    let mut v = p.to_value();
    v["curves"] = serde_json::json!("AAAA");
    assert!(Profile::from_value(&v).is_none());
    v["curves"] = serde_json::json!("not base64 !!");
    assert!(Profile::from_value(&v).is_none());
    let mut v = p.to_value();
    v["level"] = serde_json::json!(["x", 1e40, null]);
    v["seconds"] = serde_json::json!([-5]);
    let odd = Profile::from_value(&v).unwrap();
    assert!(odd.level.iter().chain(&odd.seconds).all(|x| x.is_finite()));
    assert!(!odd.is_measured(), "negative seconds read as none");
    assert!(Profile::from_json(&"x".repeat(profile::MAX_TEXT_BYTES + 1)).is_none());
    // base64 both ways, at every length.
    for len in 0..40usize {
        let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
        assert_eq!(b64_decode(&b64_encode(&bytes)).unwrap(), bytes);
    }
}

#[test]
fn pink_measures_flat_white_rises_three_db_an_octave_and_the_tilts_are_made_to_match() {
    let p = profile_of(&[pink(LEN / 2, 3)], "pink");
    assert!(!p.stereo);
    let c = p.curve(ALL, SUM);
    assert!(spread(c, &[0.0; GRID_N], 100.0, 10_000.0) < 1.0, "pink is flat per twelfth octave");
    assert_eq!(p.curve(ALL, MID), c, "mono: the mid is the sum");
    assert!(p.curve(ALL, SIDE).iter().all(|v| *v <= profile::FLOOR_DB + 0.5), "mono: no side");
    let w = profile_of(&[white(LEN / 2, 4)], "white");
    let slope = (w.curve(ALL, SUM)[at(8_000.0)] - w.curve(ALL, SUM)[at(500.0)]) / 4.0;
    assert!((slope - 3.01).abs() < 0.3, "{slope} dB per octave");
    // The made-up profiles: the right slope, pink flat.
    let tilts = factory_profiles();
    assert_eq!(tilts.len(), FACTORY_TILTS.len());
    for (t, (name, slope)) in tilts.iter().zip(FACTORY_TILTS) {
        assert_eq!(t.name, name);
        assert!(t.is_measured() && !t.stereo);
        let got = t.curve(ALL, SUM)[at(8_000.0)] - t.curve(ALL, SUM)[at(1_000.0)];
        assert!((got - slope * 3.0).abs() < 0.01, "{name}");
    }
    assert!(spread(tilts[0].curve(ALL, SUM), w.curve(ALL, SUM), 100.0, 10_000.0) < 1.0, "the flat tilt is white noise");
    // The same sound at another rate measures the same.
    let refs = [pink(LEN / 2, 3)];
    let slices: Vec<&[f32]> = refs.iter().map(Vec::as_slice).collect();
    let hi = measure_audio(&slices, 96_000.0, 0, usize::MAX, 24.0, "x");
    assert!(spread(hi.curve(ALL, SUM), &[0.0; GRID_N], 200.0, 20_000.0) < 1.0, "pink at twice the rate is still flat");
}

#[test]
fn silence_and_bleed_are_left_out_of_a_measurement() {
    let sound = pink(96_000, 5);
    let clean = profile_of(std::slice::from_ref(&sound), "clean");
    // The same sound in bursts, with silence and a dark bleed 40 dB down in between.
    let bleed: Vec<f32> = dark(&pink(96_000, 6), 300.0).iter().map(|v| v * 0.01).collect();
    let mut gappy = Vec::new();
    for chunk in 0..4 {
        gappy.extend_from_slice(&sound[chunk * 24_000..(chunk + 1) * 24_000]);
        gappy.extend_from_slice(&bleed[chunk * 24_000..(chunk + 1) * 24_000]);
        gappy.extend(std::iter::repeat_n(0.0, 24_000));
    }
    let got = profile_of(&[gappy], "gappy");
    assert!(spread(got.curve(ALL, SUM), clean.curve(ALL, SUM), 100.0, 10_000.0) < 0.8);
    assert!(got.seconds[ALL] < 2.6, "about two seconds of it were sound: {}", got.seconds[ALL]);
    // Nothing at all, and too little for one frame, are not measurements.
    assert!(!profile_of(&[vec![0.0; 48_000]], "silence").is_measured());
    assert!(!profile_of(&[vec![0.5; 100]], "short").is_measured());
    assert!(!measure_audio(&[], 48_000.0, 0, 10, 24.0, "none").is_measured());
    assert!(!measure_audio(&[&[f32::NAN; 9000]], f64::NAN, usize::MAX, usize::MAX, f32::NAN, "bad").is_measured());
}

#[test]
fn with_no_target_it_is_a_pure_delay_in_linear_phase_and_nothing_at_all_in_minimum_phase() {
    let x = stereo(pink(48_000, 7));
    let mut t = copyq(&[]);
    let y = run(&mut t, &x, 512);
    let worst = x[0].iter().zip(&y[0]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 2.0e-4, "{worst}");
    let mut t = copyq(&[("phase", 1.0)]);
    assert_eq!(run(&mut t, &x, 512), x);
    // A target but nothing measured yet and Follow off: still nothing to do.
    let mut t = copyq(&[("follow", 0.0)]);
    assert!(t.load_state(&state_with(&[(1.0, &Profile::tilt("dark", -3.0))], None)));
    let y = run(&mut t, &x, 512);
    assert!(x[0].iter().zip(&y[0]).all(|(a, b)| (a - b).abs() < 2.0e-4));
    assert_eq!(live(&t, LIVE_MATCHING), 0.0);
    assert_eq!(live(&t, LIVE_HAS_TARGET), 1.0);
}

#[test]
fn it_makes_one_sound_meet_another() {
    let target = profile_of(&stereo(dark(&pink(LEN / 2, 8), 1_000.0)), "dark");
    let x = stereo(pink(LEN, 9));
    let before = spread(profile_of(&x, "in").curve(ALL, SUM), target.curve(ALL, SUM), 100.0, 10_000.0);
    assert!(before > 5.0, "the two sounds differ: {before}");
    for (phase, close) in [(0.0, 1.0), (1.0, 1.6)] {
        let mut t = copyq(&[("phase", phase), ("limit", 24.0), ("detail", 60.0)]);
        assert!(t.load_state(&state_with(&[(1.0, &target)], None)));
        let y = run(&mut t, &x, 512);
        let out = tail_profile(&y);
        let after = spread(out.curve(ALL, SUM), target.curve(ALL, SUM), 100.0, 10_000.0);
        assert!(after < close, "phase {phase}: {after} dB from the target's shape (was {before})");
        assert_eq!(live(&t, LIVE_MATCHING), 1.0);
        // Match Level: about as loud as it went in.
        let half = LEN / 2;
        let change = rms_db(&y[0][half..]) - rms_db(&x[0][half..]);
        assert!(change.abs() < 4.0, "phase {phase}: the level moved {change} dB");
        // The response shown is the response applied: dark, so the top is cut.
        let cut = live(&t, live_shown(0, 2) + at(8_000.0)) - live(&t, live_shown(0, 2) + at(200.0));
        assert!(cut < -8.0, "phase {phase}: {cut}");
        let mut r = [f32::NAN; 8];
        t.readouts(&mut r);
        assert!(r[0] > 3.0 && r[3] > 5.0 && t.gain_reduction_db() == r[0]);
    }
}

#[test]
fn amount_scales_the_match_and_below_zero_pushes_away() {
    let target = Profile::tilt("dark", -3.0);
    let x = stereo(pink(LEN / 2, 10));
    let tilt_of = |set: &[(&str, f32)]| {
        let mut t = copyq(set);
        assert!(t.load_state(&state_with(&[(1.0, &target)], None)));
        run(&mut t, &x, 512);
        live(&t, live_shown(0, 2) + at(6_000.0)) - live(&t, live_shown(0, 2) + at(300.0))
    };
    let full = tilt_of(&[("limit", 24.0)]);
    let half = tilt_of(&[("limit", 24.0), ("amount", 50.0)]);
    let none = tilt_of(&[("limit", 24.0), ("amount", 0.0)]);
    let away = tilt_of(&[("limit", 24.0), ("amount", -100.0)]);
    let over = tilt_of(&[("limit", 24.0), ("amount", 200.0)]);
    assert!(full < -8.0, "{full}");
    assert!((half - full * 0.5).abs() < 1.5, "{half} against {full}");
    assert!(none.abs() < 0.01, "{none}");
    assert!((away + full).abs() < 1.5, "{away} against {full}");
    assert!(over < full - 3.0, "{over} against {full}");
    // Max Gain holds every frequency.
    let mut t = copyq(&[("limit", 3.0), ("amount", 200.0), ("autoGain", 0.0)]);
    assert!(t.load_state(&state_with(&[(1.0, &target)], None)));
    run(&mut t, &x, 512);
    let worst = (0..GRID_N).map(|i| live(&t, live_shown(0, 2) + i).abs()).fold(0.0f32, f32::max);
    assert!(worst <= 3.001 && worst > 2.0, "{worst}");
    // The ranges: the low end left alone, the top matched.
    let mut t = copyq(&[("limit", 24.0), ("amtLow", 0.0), ("amtLowMid", 0.0), ("rangeHi", 5_000.0)]);
    assert!(t.load_state(&state_with(&[(1.0, &target)], None)));
    run(&mut t, &x, 512);
    let g = |hz: f32| live(&t, live_shown(0, 2) + at(hz));
    assert!(g(60.0).abs() < 0.3 && g(400.0).abs() < 0.3, "{} {}", g(60.0), g(400.0));
    assert!(g(3_000.0).abs() > 2.0, "{}", g(3_000.0));
    assert!(g(12_000.0).abs() < 0.05, "outside the working range: {}", g(12_000.0));
}

#[test]
fn several_profiles_blend_by_weight() {
    let (a, b) = (Profile::tilt("a", 3.0), Profile::tilt("b", -3.0));
    let mid = Profile::blend(&[(1.0, &a), (1.0, &b)]).unwrap();
    assert_eq!(mid.name, "a + b");
    assert!(spread(mid.curve(ALL, SUM), &[0.0; GRID_N], 20.0, 20_000.0) < 0.01, "halfway between the two is flat");
    let lean = Profile::blend(&[(3.0, &a), (1.0, &b)]).unwrap();
    let slope = (lean.curve(ALL, SUM)[at(4_000.0)] - lean.curve(ALL, SUM)[at(1_000.0)]) / 2.0;
    assert!((slope - 1.5).abs() < 0.01, "{slope}");
    assert_eq!(Profile::blend(&[(1.0, &a)]).unwrap().name, "a");
    assert!(Profile::blend(&[]).is_none());
    assert!(Profile::blend(&[(0.0, &a), (f32::NAN, &b), (-1.0, &a)]).is_none());
    assert!(Profile::blend(&[(1.0, &Profile::silent())]).is_none());
    // The state: at most four targets, weights held to 0..1, junk dropped, and it reads back.
    let st = CopyQState { targets: vec![(0.25, a.clone()), (0.75, b.clone())], source: Some(a.clone()) };
    let back = CopyQState::read(&st.write()).unwrap();
    assert_eq!(back.targets.len(), 2);
    assert_eq!((back.targets[0].0, back.targets[1].0), (0.25, 0.75));
    assert_eq!(back.source.unwrap().name, "a");
    let many = CopyQState { targets: vec![(9.0, a.clone()); 7], source: None };
    let back = CopyQState::read(&many.write()).unwrap();
    assert_eq!(back.targets.len(), profile::MAX_TARGETS);
    assert!(back.targets.iter().all(|(w, _)| *w == 1.0));
    assert!(many.write().len() < 64 * 1024, "four profiles fit the 64 KiB a state edit takes: {}", many.write().len());
    let mut full = many.clone();
    full.source = Some(b.clone());
    assert!(full.write().len() < 64 * 1024, "and a frozen source with them: {}", full.write().len());
    assert!(CopyQState::read(b"").is_none());
    assert!(CopyQState::read(&[0xff, 0xfe]).is_none());
    assert!(CopyQState::read(br#"{"format":"copyq-profile"}"#).is_none());
    let odd = CopyQState::read(br#"{"format":"copyq-state","targets":[1,{"weight":"x"},{"profile":{}}],"source":7}"#).unwrap();
    assert!(odd.targets.is_empty() && odd.source.is_none());
    // What the audio thread is handed: always the same length, whatever the state.
    assert_eq!(prepare_state(b"junk").len(), PREPARED_LEN);
    assert_eq!(prepare_state(&st.write()).len(), PREPARED_LEN);
    assert_eq!(crate::prepare_state("fs_copyq", &st.write()).unwrap().len(), PREPARED_LEN);
    let mut shape = Shape::default();
    shape.from_prepared(&prepare_state(&st.write()), 0);
    assert!(shape.present);
    shape.from_prepared(&prepare_state(b"junk"), 0);
    assert!(!shape.present);
    shape.from_prepared(&[1.0; 5], 0);
    assert!(!shape.present);
    shape.from_prepared(&vec![f64::NAN; PREPARED_LEN], 1);
    assert!(!shape.present);
}

#[test]
fn the_state_loads_saves_and_a_frozen_source_is_used_with_follow_off() {
    let mut t = copyq(&[]);
    let empty = t.save_state().unwrap();
    assert!(CopyQState::read(&empty).is_some_and(|s| s.targets.is_empty() && s.source.is_none()));
    assert!(!t.load_state(b"anything") && !t.load_state(&[0xff]));
    let target = Profile::tilt("dark", -3.0);
    let source = Profile::tilt("pink", 0.0);
    let bytes = state_with(&[(1.0, &target)], Some(&source));
    assert!(t.load_state(&bytes));
    assert_eq!(t.save_state().unwrap(), bytes);
    // Follow off with a frozen source: the curve is there at once, in full, and does not move
    // with what plays.
    assert!(t.set_param("follow", 0.0) && t.set_param("limit", 24.0) && t.set_param("detail", 100.0));
    let mut io = stereo(white(24_000, 11));
    t.process(&mut io, 24_000);
    assert_eq!(live(&t, LIVE_HAS_SOURCE), 1.0);
    let g = |t: &CopyQ, hz: f32| live(t, live_shown(0, 2) + at(hz));
    let tilt = g(&t, 8_000.0) - g(&t, 1_000.0);
    assert!((tilt + 9.0).abs() < 0.6, "three octaves at -3 dB each, whatever the input was: {tilt}");
    assert_eq!(live(&t, LIVE_SECONDS), 0.0, "nothing is measured with Follow off");
    // The host's way in is the same state, made ready off the audio thread.
    let mut u = copyq(&[("follow", 0.0), ("limit", 24.0), ("detail", 100.0)]);
    assert!(u.set_prepared_state(&prepare_state(&bytes)));
    let mut io = stereo(white(24_000, 11));
    u.process(&mut io, 24_000);
    assert!((g(&u, 8_000.0) - g(&t, 8_000.0)).abs() < 1.0e-4);
    // An empty preparation clears it.
    assert!(u.set_prepared_state(&[]));
    let mut io = stereo(white(24_000, 12));
    u.process(&mut io, 24_000);
    assert_eq!(live(&u, LIVE_HAS_TARGET), 0.0);
    assert!(g(&u, 8_000.0).abs() < 0.05);
}

#[test]
fn capture_measures_everything_passes_the_audio_and_reset_measure_forgets_it() {
    let x = stereo(dark(&pink(LEN / 2, 13), 800.0));
    let mut t = copyq(&[("capture", 1.0)]);
    // Even with a target, a capturing module leaves the audio alone.
    assert!(t.load_state(&state_with(&[(1.0, &Profile::tilt("bright", 3.0))], None)));
    let y = run(&mut t, &x, 512);
    assert!(x[0].iter().zip(&y[0]).all(|(a, b)| (a - b).abs() < 2.0e-4));
    assert_eq!(live(&t, LIVE_MATCHING), 0.0);
    let seconds = live(&t, LIVE_SECONDS);
    assert!((seconds - 4.0).abs() < 0.2, "every frame of it: {seconds}");
    // The curves the editor saves are the ones a file measurement gives.
    let file = profile_of(&x, "file");
    let mut got = [0.0f32; GRID_N];
    for (i, v) in got.iter_mut().enumerate() {
        *v = live(&t, LIVE_MEASURED + profile::curve_index(ALL, SUM) * GRID_N + i);
    }
    assert!(spread(&got, file.curve(ALL, SUM), 50.0, 15_000.0) < 0.3);
    assert_eq!(live(&t, LIVE_STEREO), 1.0);
    // Capture again starts over; the action forgets it outright.
    assert!(t.set_param("capture", 0.0) && t.set_param("capture", 1.0));
    let mut io = stereo(pink(9_600, 14));
    t.process(&mut io, 9_600);
    assert!(live(&t, LIVE_SECONDS) < 0.3);
    assert!(t.action("reset_measure") && !t.action("nope"));
    // (The frame still holds a little of what just played.)
    let mut io = stereo(vec![0.0; 9_600]);
    t.process(&mut io, 9_600);
    assert!(live(&t, LIVE_SECONDS) < 0.1, "{}", live(&t, LIVE_SECONDS));
}

#[test]
fn mid_and_side_are_matched_apart_so_the_width_carries_over() {
    let (m, s) = (pink(LEN, 15), pink(LEN, 16));
    let image = |side: f32| -> Vec<Vec<f32>> { vec![m.iter().zip(&s).map(|(a, b)| a + b * side).collect(), m.iter().zip(&s).map(|(a, b)| a - b * side).collect()] };
    let (narrow, wide) = (image(0.1), image(0.5));
    let target = profile_of(&wide, "wide");
    assert!(target.stereo);
    let width = |p: &Profile| p.curve(ALL, SIDE)[at(1_000.0)] - p.curve(ALL, MID)[at(1_000.0)];
    assert!((width(&target) + 6.0).abs() < 1.0 && (width(&profile_of(&narrow, "n")) + 20.0).abs() < 1.0);
    let render = |channels: f32| {
        let mut t = copyq(&[("channels", channels), ("limit", 24.0)]);
        assert!(t.load_state(&state_with(&[(1.0, &target)], None)));
        let y = run(&mut t, &narrow, 512);
        (width(&tail_profile(&y)), live(&t, LIVE_SPLIT))
    };
    let (linked, split) = render(0.0);
    assert!((linked + 20.0).abs() < 1.0 && split == 0.0, "one curve leaves the width alone: {linked}");
    let (ms, split) = render(1.0);
    assert!(ms > -11.0 && split == 1.0, "mid/side opens it up: {ms}");
    // Against a mono target there is no side to match: one curve.
    let mut t = copyq(&[("channels", 1.0)]);
    assert!(t.load_state(&state_with(&[(1.0, &Profile::tilt("mono", 0.0))], None)));
    run(&mut t, &narrow[..].iter().map(|c| c[..96_000].to_vec()).collect::<Vec<_>>(), 512);
    assert_eq!(live(&t, LIVE_SPLIT), 0.0);
    // Left/right: a dull right channel is brought up to a target whose sides are alike.
    let lopsided = vec![pink(LEN, 17), dark(&pink(LEN, 18), 1_500.0)];
    let even = profile_of(&[pink(LEN / 2, 19), pink(LEN / 2, 20)], "even");
    let mut t = copyq(&[("channels", 2.0), ("limit", 24.0)]);
    assert!(t.load_state(&state_with(&[(1.0, &even)], None)));
    run(&mut t, &lopsided, 512);
    let top = |ab: usize| live(&t, live_shown(ab, 2) + at(8_000.0)) - live(&t, live_shown(ab, 2) + at(300.0));
    assert!(top(0).abs() < 2.0 && top(1) > 8.0, "left {} right {}", top(0), top(1));
}

#[test]
fn loud_and_quiet_are_measured_apart_and_dynamics_follows_them() {
    // A sound that is bright when loud and dull when quiet, a second and a half of each.
    let seg = 72_000;
    let mut bright_loud = Vec::new();
    let mut plain = Vec::new();
    for i in 0..6 {
        let loud = i % 2 == 0;
        let p = pink(seg, 30 + i as u32);
        let shaped: Vec<f32> = if loud { p.clone() } else { dark(&p, 1_200.0).iter().map(|v| v * 0.12).collect() };
        bright_loud.extend(shaped);
        plain.extend(p.iter().map(|v| if loud { *v } else { v * 0.3 }));
    }
    let target = profile_of(&stereo(bright_loud), "moves");
    assert!(target.has_level(LOUD) && target.has_level(QUIET));
    assert!(target.level[LOUD] > target.level[QUIET] + 6.0, "{:?} {:?}", target.level, target.seconds);
    let top = |p: &Profile, lv: usize| p.curve(lv, SUM)[at(8_000.0)] - p.curve(lv, SUM)[at(300.0)];
    assert!(top(&target, LOUD) > top(&target, QUIET) + 8.0, "loud {} quiet {}", top(&target, LOUD), top(&target, QUIET));
    // Onto a sound whose tone does not change with its level.
    let x = stereo(plain);
    let run_to = |dynamics: f32, upto: usize| {
        let mut t = copyq(&[("dynamics", dynamics), ("limit", 24.0), ("gate", 40.0)]);
        assert!(t.load_state(&state_with(&[(1.0, &target)], None)));
        let part: Vec<Vec<f32>> = x.iter().map(|c| c[..upto].to_vec()).collect();
        let mut io = part;
        for block in 0..upto / 480 {
            let mut b: Vec<Vec<f32>> = io.iter().map(|c| c[block * 480..(block + 1) * 480].to_vec()).collect();
            t.process(&mut b, 480);
        }
        io.clear();
        (live(&t, live_shown(0, 2) + at(8_000.0)) - live(&t, live_shown(0, 2) + at(300.0)), live(&t, LIVE_POSITION))
    };
    // The end of the fifth segment is loud, the end of the sixth quiet.
    let (loud_tilt, loud_pos) = run_to(100.0, seg * 5 - 4_800);
    let (quiet_tilt, quiet_pos) = run_to(100.0, seg * 6 - 4_800);
    assert!(loud_pos > 0.8 && quiet_pos < 0.2, "{loud_pos} {quiet_pos}");
    assert!(loud_tilt > quiet_tilt + 5.0, "bright when loud ({loud_tilt}), dull when quiet ({quiet_tilt})");
    let (a, _) = run_to(0.0, seg * 5 - 4_800);
    let (b, _) = run_to(0.0, seg * 6 - 4_800);
    assert!((a - b).abs() < 1.5, "without Dynamics the curve holds still: {a} {b}");
}

fn slab(producers: u32, consumers: u32, peers: u32) -> Arc<LinkSlab> {
    let s = Arc::new(LinkSlab::new(link::WORDS));
    s.set_members(producers, consumers, peers, link::LAYOUT);
    s.set_member_count(producers + consumers + peers);
    s
}

/// Runs linked modules block by block, each on its own input.
fn run_linked(mods: &mut [CopyQ], inputs: &[Vec<Vec<f32>>]) {
    let len = inputs[0][0].len();
    for block in 0..len / 512 {
        for (m, x) in mods.iter_mut().zip(inputs) {
            let mut b: Vec<Vec<f32>> = x.iter().map(|c| c[block * 512..(block + 1) * 512].to_vec()).collect();
            m.process(&mut b, 512);
        }
    }
}

#[test]
fn a_match_follows_its_groups_send_and_a_group_meets_in_the_middle() {
    let dull = stereo(dark(&pink(LEN / 2, 40), 1_000.0));
    let flat = stereo(pink(LEN / 2, 41));
    let tilt = |t: &CopyQ| live(t, live_shown(0, 2) + at(6_000.0)) - live(t, live_shown(0, 2) + at(300.0));
    // Send and Match.
    let s = slab(1, 1, 0);
    let mut mods = [copyq(&[("link", 1.0)]), copyq(&[("link", 2.0), ("limit", 24.0)])];
    mods[0].set_link(Some(s.clone()), 0);
    mods[1].set_link(Some(s.clone()), 1);
    run_linked(&mut mods, &[dull.clone(), flat.clone()]);
    assert_eq!(live(&mods[0], LIVE_MATCHING), 0.0, "a Send passes its audio");
    assert_eq!((live(&mods[1], LIVE_SENDERS), live(&mods[1], LIVE_LINKED), live(&mods[1], LIVE_MATCHING)), (1.0, 1.0, 1.0));
    assert!(tilt(&mods[1]) < -8.0, "the flat one is darkened towards the dull one: {}", tilt(&mods[1]));
    // The link target wins over the state's; taken out of the group, the state's is back.
    assert!(mods[1].load_state(&state_with(&[(1.0, &Profile::tilt("bright", 3.0))], None)));
    run_linked(&mut mods, &[dull.clone(), flat.clone()]);
    assert!(tilt(&mods[1]) < -8.0);
    mods[1].set_link(None, 0);
    run_linked(&mut mods[1..], std::slice::from_ref(&flat));
    assert!(tilt(&mods[1]) > 5.0, "{}", tilt(&mods[1]));
    // A Send that leaves takes its measurement with it.
    mods[1].set_link(Some(s.clone()), 1);
    mods[0].set_link(None, 0);
    run_linked(&mut mods[1..], std::slice::from_ref(&flat));
    assert_eq!((live(&mods[1], LIVE_SENDERS), live(&mods[1], LIVE_LINKED)), (0.0, 0.0));

    // Two Group members: each comes halfway.
    let s = slab(0, 0, 2);
    let mut mods = [copyq(&[("link", 3.0), ("limit", 24.0)]), copyq(&[("link", 3.0), ("limit", 24.0)])];
    mods[0].set_link(Some(s.clone()), 0);
    mods[1].set_link(Some(s.clone()), 1);
    run_linked(&mut mods, &[dull.clone(), flat.clone()]);
    assert_eq!(live(&mods[0], LIVE_GROUP), 2.0);
    let (a, b) = (tilt(&mods[0]), tilt(&mods[1]));
    assert!(a > 4.0 && b < -4.0 && (a + b).abs() < 2.5, "the dull one up {a}, the flat one down {b}");
    // A Send in the group is the leader: both Group members go to it.
    let s = slab(1, 0, 2);
    let mut mods = [copyq(&[("link", 1.0)]), copyq(&[("link", 3.0), ("limit", 24.0)]), copyq(&[("link", 3.0), ("limit", 24.0)])];
    for (i, m) in mods.iter_mut().enumerate() {
        m.set_link(Some(s.clone()), i);
    }
    run_linked(&mut mods, &[dull.clone(), flat.clone(), flat.clone()]);
    assert!(tilt(&mods[1]) < -8.0 && tilt(&mods[2]) < -8.0);

    // A slab of another plugin's layout, or too short, is no link at all.
    let other = Arc::new(LinkSlab::new(link::WORDS));
    other.set_members(1, 1, 0, 0x1234);
    other.set_member_count(2);
    let mut m = copyq(&[("link", 2.0)]);
    m.set_link(Some(other), 1);
    let short = Arc::new(LinkSlab::new(64));
    short.set_members(1, 1, 0, link::LAYOUT);
    let mut n = copyq(&[("link", 3.0)]);
    n.set_link(Some(short), 99);
    run_linked(std::slice::from_mut(&mut m), std::slice::from_ref(&flat));
    run_linked(std::slice::from_mut(&mut n), std::slice::from_ref(&flat));
    assert_eq!((live(&m, LIVE_LINKED), live(&n, LIVE_LINKED)), (0.0, 0.0));
}

#[test]
fn delta_is_the_output_minus_the_input() {
    let target = Profile::tilt("dark", -3.0);
    let source = Profile::tilt("pink", 0.0);
    let x = stereo(pink(96_000, 50));
    for phase in [0.0, 1.0] {
        let render = |delta: f32| {
            let mut t = copyq(&[("phase", phase), ("follow", 0.0), ("autoGain", 0.0), ("delta", delta)]);
            assert!(t.load_state(&state_with(&[(1.0, &target)], Some(&source))));
            // Let the curve settle before the part compared.
            let mut warm = stereo(pink(48_000, 51));
            t.process(&mut warm, 48_000);
            run(&mut t, &x, 512)
        };
        let (wet, delta) = (render(0.0), render(1.0));
        let from = 24_000;
        let worst = (from..90_000).map(|i| (wet[0][i] - x[0][i] - delta[0][i]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 2.0e-3, "phase {phase}: {worst}");
        assert!(rms_db(&delta[0][from..90_000]) > -60.0, "and it is not silence");
    }
}

#[test]
fn mono_works_and_reset_keeps_the_measurement_and_the_curve() {
    let mut t = CopyQ::new();
    t.prepare(SR, 512, 1);
    assert!(t.set_param("limit", 24.0) && t.set_param("channels", 1.0));
    assert!(t.load_state(&state_with(&[(1.0, &Profile::tilt("dark", -3.0))], None)));
    let x = vec![pink(LEN / 2, 60)];
    let y = run(&mut t, &x, 512);
    assert!(y[0].iter().all(|v| v.is_finite()));
    assert_eq!((live(&t, LIVE_STEREO), live(&t, LIVE_SPLIT)), (0.0, 0.0));
    let g = live(&t, live_shown(0, 2) + at(8_000.0));
    assert!(g < -3.0, "{g}");
    let seconds = live(&t, LIVE_SECONDS);
    t.reset();
    let mut io = vec![vec![0.0f32; 2048]];
    t.process(&mut io, 2048);
    assert!(io[0].iter().all(|v| *v == 0.0), "no audio left in flight");
    assert_eq!(live(&t, LIVE_SECONDS), seconds, "silence adds nothing and reset forgot nothing");
    assert!((live(&t, live_shown(0, 2) + at(8_000.0)) - g).abs() < 0.5);
    // A second channel offered to a mono instance is left untouched.
    let mut io = vec![pink(1024, 61), vec![0.25; 1024]];
    t.process(&mut io, 1024);
    assert!(io[1].iter().all(|v| *v == 0.25));
}

#[test]
fn silence_hostile_values_and_odd_calls_are_harmless() {
    let mut t = copyq(&[]);
    assert!(t.load_state(&state_with(&[(1.0, &Profile::tilt("dark", -3.0))], None)));
    let y = run(&mut t, &stereo(vec![0.0; 48_000]), 512);
    assert!(y.iter().flatten().all(|v| *v == 0.0));
    let mut io = vec![vec![f32::NAN; 8192], vec![f32::INFINITY; 8192]];
    t.process(&mut io, 8192);
    t.process(&mut io, 8192);
    assert!(io.iter().flatten().all(|v| v.is_finite()));
    for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30] {
        for p in &COPYQ_PARAMS {
            assert!(t.set_param(p.id, v));
        }
        let mut io = stereo(pink(8192, 70));
        t.process(&mut io, 8192);
        assert!(io.iter().flatten().all(|v| v.is_finite()), "{v}");
    }
    assert!(!t.set_param("nope", 1.0));
    // Empty and lopsided buffers, more frames asked than given.
    t.process(&mut [], 64);
    t.process(&mut [vec![0.1; 10], vec![0.1; 3]], 64);
    // The splits out of order are the same splits; a hostile rate still prepares.
    let mut d = CopyQDsp::new();
    d.prepare(f64::NAN, 9);
    d.set(Settings { split_hz: [9_000.0, f32::NAN, 40.0], lo_hz: 8_000.0, hi_hz: 100.0, amount: f32::INFINITY, ..Settings::default() });
    let s = d.settings();
    assert!(s.split_hz[0] <= s.split_hz[1] && s.split_hz[1] <= s.split_hz[2]);
    let mut io = stereo(pink(8192, 71));
    d.process(&mut io, 8192);
    assert!(io.iter().flatten().all(|v| v.is_finite()));
    assert_eq!(d.latency_samples(), N);
    // Every curve published is finite.
    let a = t.analysis().unwrap();
    assert!((0..LIVE_LEN).all(|i| a.live.get(i).is_finite()));
    assert_eq!(a.live.len(), LIVE_LEN);
}

#[test]
fn a_curve_becomes_a_few_bells() {
    // What a match looks like: a tilt, a bump and a dip.
    let mut curve = [0.0f32; GRID_N];
    for (i, v) in curve.iter_mut().enumerate() {
        let oct = (grid_hz(i) / 1_000.0).log2();
        *v = -1.2 * oct + 5.0 * (-(oct + 2.5).powi(2) / 0.3).exp() - 4.0 * (-(oct - 1.8).powi(2) / 0.15).exp();
    }
    let bells = fit_bells(&curve, 8);
    assert!(!bells.is_empty() && bells.len() <= 8);
    assert!(bells.windows(2).all(|w| w[0].hz <= w[1].hz), "low to high");
    assert!(bells.iter().all(|b| (20.0..=20_480.0).contains(&b.hz) && (bells::MIN_Q..=bells::MAX_Q).contains(&b.q) && b.gain_db.abs() <= 24.0));
    let err = fit_error(&curve, &bells);
    assert!(err < 0.8, "{err} dB RMS with {} bells", bells.len());
    assert!(fit_error(&curve, &fit_bells(&curve, 3)) > err, "more bells fit closer");
    assert!(fit_bells(&[0.0; GRID_N], 8).is_empty(), "nothing to fit");
    assert!(fit_bells(&[0.2; GRID_N], 8).is_empty(), "under a third of a dB is left alone");
    assert!(fit_bells(&[f32::NAN; GRID_N], 8).is_empty());
    assert!(fit_bells(&curve, 0).is_empty());
    assert!(fit_bells(&curve, 1_000).len() <= 24);
}

#[test]
fn it_costs_little() {
    let mut t = copyq(&[("phase", 1.0)]);
    assert!(t.load_state(&state_with(&[(1.0, &Profile::tilt("dark", -3.0))], None)));
    let mut io = stereo(pink(48_000, 80));
    let start = std::time::Instant::now();
    for _ in 0..4 {
        t.process(&mut io, 48_000);
    }
    let per_second = start.elapsed().as_secs_f64() / 4.0;
    // Debug builds are slow; this only catches something going badly wrong.
    assert!(per_second < 2.0, "{per_second:.3} s per second of audio");
}
