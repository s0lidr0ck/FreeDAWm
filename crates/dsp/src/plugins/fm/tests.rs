use super::reference;
use super::*;
use crate::plugins::fs::FsPreset;

const SR: f32 = 48_000.0;

fn synth(set: &[(&str, f32)]) -> FmSynth {
    let mut s = FmSynth::new();
    s.prepare(SR, 512, 2);
    for (id, v) in set {
        assert!(s.set_param(id, *v), "{id}");
    }
    s
}

fn render_with(s: &mut FmSynth, notes: &[(usize, u8, u8)], frames: usize, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    let mut io = vec![vec![0.0f32; block]; 2];
    let mut at = 0;
    while at < frames {
        for (when, note, vel) in notes.iter().filter(|(w, _, _)| (at..at + block).contains(w)) {
            s.note_on(when - at, *note, *vel);
        }
        s.process(&mut io, block);
        assert_eq!(io[0], io[1], "the same on both sides");
        out.extend_from_slice(&io[0]);
        at += block;
    }
    out
}

fn render(set: &[(&str, f32)], notes: &[(usize, u8, u8)], frames: usize) -> Vec<f32> {
    render_with(&mut synth(set), notes, frames, 256)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn mag(x: &[f32], hz: f64) -> f64 {
    let n = x.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n).cos();
        let a = std::f64::consts::TAU * hz * i as f64 / f64::from(SR);
        re += f64::from(*v) * w * a.cos();
        im += f64::from(*v) * w * a.sin();
    }
    (re * re + im * im).sqrt()
}

/// Energy where sidebands above half the sample rate fold back to, below 20 kHz (more than 40 Hz
/// from any harmonic of `f0`), over the energy at the harmonics, dB. Folding that lands between
/// 20 kHz and half the sample rate (the half-band filters' transition band) is not counted.
fn aliasing_db(x: &[f32], f0: f64) -> f64 {
    let nyq = f64::from(SR) / 2.0;
    let near = |f: f64| (f - (f / f0).round() * f0).abs() < 40.0;
    let mut harm = 0.0;
    let mut k = 1.0;
    while k * f0 < nyq - 100.0 {
        harm += mag(x, k * f0).powi(2);
        k += 1.0;
    }
    let mut alias = 0.0;
    let mut seen: Vec<f64> = Vec::new();
    for h in 1..=60 {
        for j in 1..=2 {
            let f = (f64::from(SR) * j as f64 - h as f64 * f0).abs();
            if f > 40.0 && f < 20_000.0 && !near(f) && !seen.iter().any(|s| (s - f).abs() < 5.0) {
                seen.push(f);
                alias += mag(x, f).powi(2);
            }
        }
    }
    10.0 * (alias.max(1e-30) / harm).log10()
}

#[test]
fn every_preset_is_well_formed_and_parses() {
    let mut names = std::collections::HashSet::new();
    for p in &presets::FM_PRESETS {
        assert!(names.insert(p.name.to_lowercase()), "{}", p.name);
        assert!(!p.group.is_empty() && !p.desc.is_empty());
        for (id, v) in p.set {
            let info = FM_INFO.param(id).unwrap_or_else(|| panic!("{}: {id}", p.name));
            assert_eq!(info.clamp(*v), *v, "{}: {id}", p.name);
        }
    }
    for (name, json) in presets::factory_presets() {
        let p = FsPreset::parse(json, &FM_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert_eq!(p.values.len(), FM_INFO.params.len());
    }
    assert_eq!(crate::plugins::fs::factory_presets(FM_INFO.id).len(), presets::FM_PRESETS.len() + presets::PATCHED_PRESETS.len());
}

#[test]
fn the_parameters_are_four_operators_then_the_globals() {
    let ids: Vec<&str> = FM_INFO.params.iter().map(|p| p.id).collect();
    // The 62 the synth had, then Env 2 and the four macros (the modulation matrix's).
    assert_eq!(ids.len(), 70);
    assert_eq!(&ids[ENV2_A..], &["env2_attack", "env2_decay", "env2_sustain", "env2_release", "macro1", "macro2", "macro3", "macro4"]);
    assert_eq!(&ids[..3], &["op1_coarse", "op1_fine", "op1_detune"]);
    assert_eq!(ids[3 * OP_N + FEEDBACK], "op4_feedback");
    assert_eq!(ids[ALGORITHM], "algorithm");
    assert_eq!(ids[LFO_MOD], "lfo_mod");
    let set: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(set.len(), ids.len(), "ids are unique");
    // Every algorithm's modulators sit above the operator they modulate (so one pass from
    // operator 4 down computes them first), and every algorithm is heard.
    for (a, m) in MODULATORS.iter().enumerate() {
        for (n, mask) in m.iter().enumerate() {
            assert_eq!(mask & ((1u8 << (n + 1)) - 1), 0, "algorithm {a}, operator {}", n + 1);
        }
        assert_ne!(CARRIERS[a], 0);
        // A carrier modulates nothing.
        for (n, _) in m.iter().enumerate().filter(|(n, _)| CARRIERS[a] & (1 << n) != 0) {
            assert!(m.iter().all(|mask| mask & (1 << n) == 0), "algorithm {a}: carrier {} modulates", n + 1);
        }
    }
}

/// With its modulator at 0 the default patch is a sine at the key; raising the modulator
/// (ratio 1) brings in the harmonics, more with a higher level.
#[test]
fn a_modulator_adds_harmonics_by_its_level() {
    let f0 = 220.0;
    let partials = |x: &[f32]| (2..10).map(|k| mag(x, k as f64 * f0)).sum::<f64>() / mag(x, f0);
    let sine = render(&[("op2_level", 0.0), ("op1_sustain", 100.0)], &[(0, 57, 100)], 16_384);
    assert!(partials(&sine[4_096..]) < 0.01, "{}", partials(&sine[4_096..]));
    let some = render(&[("op2_level", 30.0), ("op1_sustain", 100.0), ("op2_sustain", 100.0)], &[(0, 57, 100)], 16_384);
    let more = render(&[("op2_level", 70.0), ("op1_sustain", 100.0), ("op2_sustain", 100.0)], &[(0, 57, 100)], 16_384);
    assert!(partials(&some[4_096..]) > 0.05);
    assert!(partials(&more[4_096..]) > partials(&some[4_096..]) * 2.0);
    // A ratio of 0.5 puts partials at half the key between the harmonics.
    let half = render(&[("op2_level", 60.0), ("op2_coarse", 0.0), ("op2_sustain", 100.0), ("op1_sustain", 100.0)], &[(0, 57, 100)], 16_384);
    assert!(mag(&half[4_096..], 330.0) > mag(&sine[4_096..], 330.0) * 100.0);
}

/// Feedback turns a lone sine towards a saw: its harmonics fall off about as 1/k.
#[test]
fn feedback_brightens_an_operator() {
    let set = |fb: f32| vec![("op2_level", 0.0), ("op1_sustain", 100.0), ("op1_feedback", fb)];
    let x = render(&set(90.0), &[(0, 57, 100)], 16_384);
    let h = |k: usize| mag(&x[4_096..], 220.0 * k as f64);
    assert!(h(2) > h(1) * 0.2 && h(3) > h(1) * 0.1, "{} {} {}", h(1), h(2), h(3));
    assert!(x.iter().all(|v| v.is_finite() && v.abs() < 1.0));
}

/// Each algorithm sounds and differs from the others with all four operators up.
#[test]
fn every_algorithm_plays_its_own_sound() {
    let mut seen: Vec<Vec<f32>> = Vec::new();
    for a in 0..8 {
        let set = [("algorithm", a as f32), ("op2_level", 50.0), ("op3_level", 50.0), ("op3_coarse", 2.0), ("op4_level", 50.0), ("op4_coarse", 3.0)];
        let x = render(&set, &[(0, 60, 100)], 8_192);
        assert!(rms(&x[2_048..]) > 0.01, "algorithm {a} is heard");
        assert!(seen.iter().all(|y| y != &x), "algorithm {a} differs");
        seen.push(x);
    }
}

/// Velocity reaches an operator by its Velocity share; key scaling turns it down above middle C.
#[test]
fn velocity_and_key_scaling() {
    let set = [("op2_level", 0.0), ("op1_sustain", 100.0), ("op1_velocity", 100.0)];
    let soft = rms(&render(&set, &[(0, 60, 30)], 8_192)[2_048..]);
    let hard = rms(&render(&set, &[(0, 60, 127)], 8_192)[2_048..]);
    assert!((hard / soft - 127.0 / 30.0).abs() < 0.1, "{hard} {soft}");
    let none = [("op2_level", 0.0), ("op1_sustain", 100.0), ("op1_velocity", 0.0)];
    let (a, b) = (rms(&render(&none, &[(0, 60, 30)], 8_192)[2_048..]), rms(&render(&none, &[(0, 60, 127)], 8_192)[2_048..]));
    assert!((a / b - 1.0).abs() < 1e-3);
    let ks = [("op2_level", 0.0), ("op1_sustain", 100.0), ("op1_key_scale", -6.0)];
    let mid = rms(&render(&ks, &[(0, 60, 100)], 8_192)[2_048..]);
    let up = rms(&render(&ks, &[(0, 84, 100)], 8_192)[2_048..]);
    assert!((20.0 * (up / mid).log10() + 12.0).abs() < 0.5, "two octaves up at -6 dB each");
}

/// A fixed-frequency operator plays the same pitch on every key.
#[test]
fn a_fixed_operator_ignores_the_key() {
    let set = [("op2_level", 0.0), ("op1_sustain", 100.0), ("op1_fixed", 1.0), ("op1_freq", 1000.0)];
    for note in [40u8, 60, 90] {
        let x = render(&set, &[(0, note, 100)], 8_192);
        assert!(mag(&x[2_048..], 1000.0) > mag(&x[2_048..], 1100.0) * 50.0, "{note}");
    }
}

/// Four times the rate and the half-bands keep a bright high note's folded sidebands well down;
/// at the plain rate they come back far louder. Measured on C7 (2.1 kHz) with a modulator at full
/// level (about 12.6 radians), and with feedback (a saw-like series that never ends).
#[test]
fn oversampling_keeps_the_sidebands_from_folding_back() {
    let f0 = f64::from(note_hz(96.0));
    let measure = |set: &[(&str, f32)]| {
        let mut a = synth(set);
        let ours = render_with(&mut a, &[(0, 96, 127)], 20_480, 256);
        let mut b = synth(set);
        b.oversample = false;
        b.update();
        let plain = render_with(&mut b, &[(0, 96, 127)], 20_480, 256);
        (aliasing_db(&ours[4_096..], f0), aliasing_db(&plain[4_096..], f0))
    };
    let (o, p) = measure(&[("op1_sustain", 100.0), ("op2_level", 100.0), ("op2_sustain", 100.0), ("op2_velocity", 0.0)]);
    eprintln!("FM aliasing at {f0:.0} Hz, index 12.6: oversampled {o:.1} dB, plain rate {p:.1} dB");
    assert!(o < p - 20.0, "{o:.1} against {p:.1}");
    assert!(o < -60.0, "{o:.1}");
    let (o, p) = measure(&[("op1_sustain", 100.0), ("op2_level", 85.0), ("op2_sustain", 100.0), ("op2_feedback", 40.0)]);
    eprintln!("FM aliasing at {f0:.0} Hz, with feedback: oversampled {o:.1} dB, plain rate {p:.1} dB");
    assert!(o < p - 20.0, "{o:.1} against {p:.1}");
    assert!(o < -60.0, "{o:.1}");
}

#[test]
fn voice_stealing_and_the_sustain_pedal() {
    let mut s = synth(&[("op1_release", 50.0), ("op2_release", 50.0), ("op3_release", 50.0), ("op4_release", 50.0)]);
    let mut io = vec![vec![0.0f32; 256]; 2];
    for n in 0..17u8 {
        s.note_on(0, 40 + n, 100);
    }
    s.process(&mut io, 256);
    assert_eq!(s.voices.iter().filter(|v| v.active()).count(), VOICES);
    assert!(s.voices.iter().any(|v| v.note == 56));
    s.all_notes_off();
    for _ in 0..60 {
        s.process(&mut io, 256);
    }
    assert!(s.voices.iter().all(|v| !v.active()));
    s.midi(0, [0xB0, 64, 127]);
    s.note_on(1, 60, 100);
    s.note_off(100, 60);
    for _ in 0..40 {
        s.process(&mut io, 256);
    }
    assert!(s.voices.iter().any(|v| v.active() && v.note == 60), "held by the pedal");
    s.midi(0, [0xB0, 64, 0]);
    for _ in 0..60 {
        s.process(&mut io, 256);
    }
    assert!(s.voices.iter().all(|v| !v.active()));
}

#[test]
fn pitch_bend_wheel_and_lfo() {
    let set = [("op2_level", 0.0), ("op1_sustain", 100.0)];
    let mut s = synth(&set);
    s.midi(0, [0xE0, 0x7F, 0x7F]);
    let x = render_with(&mut s, &[(0, 69, 100)], 16_384, 256);
    assert!(mag(&x[4_096..], 493.88) > mag(&x[4_096..], 440.0) * 10.0);
    // The mod wheel deepens the modulators when Mod Wheel to Modulators is up.
    let set = [("op2_level", 20.0), ("op2_sustain", 100.0), ("op1_sustain", 100.0), ("wheel_mod", 100.0)];
    let partials = |x: &[f32]| (2..10).map(|k| mag(x, k as f64 * 220.0)).sum::<f64>() / mag(x, 220.0);
    let mut s = synth(&set);
    let down = render_with(&mut s, &[(0, 57, 100)], 8_192, 256);
    s.midi(0, [0xB0, 1, 127]);
    let up = render_with(&mut s, &[], 8_192, 256);
    assert!(partials(&up[2_048..]) > partials(&down[2_048..]) * 1.8, "{} {}", partials(&up[2_048..]), partials(&down[2_048..]));
    // The LFO to level makes a tremolo.
    let x = render(&[("op2_level", 0.0), ("op1_sustain", 100.0), ("lfo_amp", 100.0), ("lfo_rate", 4.0), ("lfo_wave", 2.0)], &[(0, 60, 100)], 48_000);
    let (a, b) = (rms(&x[2_000..5_000]), rms(&x[8_000..11_000]));
    assert!(b < a * 0.1 || a < b * 0.1, "{a} {b}");
}

#[test]
fn block_size_does_not_change_the_sound() {
    let set = [("algorithm", 4.0), ("op2_level", 50.0), ("op4_level", 40.0), ("op2_feedback", 30.0), ("lfo_mod", 30.0)];
    let notes = [(0usize, 48u8, 100u8), (700, 55, 90), (3_001, 60, 127)];
    let x = render_with(&mut synth(&set), &notes, 12_288, 64);
    let y = render_with(&mut synth(&set), &notes, 12_288, 512);
    assert_eq!(x, y);
}

#[test]
fn the_defaults_and_presets_play_at_sensible_levels() {
    let chord = [(0usize, 48u8, 110u8), (0, 55, 110), (0, 60, 110), (0, 64, 110)];
    let mut sets: Vec<(&str, &[(&str, f32)])> = vec![("defaults", &[])];
    sets.extend(presets::FM_PRESETS.iter().map(|p| (p.name, p.set)));
    for (name, set) in sets {
        let x = render(set, &chord, 48_000);
        let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let level = 20.0 * rms(&x[2_000..10_000]).log10();
        assert!(peak < 1.0 && level > -40.0, "{name}: peak {peak}, rms {level:.1} dB");
    }
}

#[test]
fn the_tail_covers_the_release() {
    let mut s = synth(&[]);
    let mut io = vec![vec![0.0f32; 512]; 2];
    s.note_on(0, 60, 127);
    s.process(&mut io, 512);
    s.note_off(0, 60);
    let tail = s.tail_samples();
    let mut last = 0;
    for b in 0..(tail / 512 + 200) {
        s.process(&mut io, 512);
        if io[0].iter().any(|x| x.abs() > 1.0e-6) {
            last = (b + 1) * 512;
        }
    }
    assert!(last <= tail + 512, "{last} {tail}");
}

/// What the synth plays at its defaults and with each factory preset, pinned as a fingerprint
/// within [`crate::plugins::golden::TOLERANCE`] (1e-4, -80 dB; not bit for bit, see that
/// module), so a change to the sound is a change on purpose.
#[test]
fn the_defaults_and_the_factory_presets_are_pinned() {
    let sets: Vec<(&str, &[(&str, f32)])> =
        std::iter::once(("defaults", &[][..])).chain(presets::FM_PRESETS.iter().map(|p| (p.name, p.set))).collect();
    let got: Vec<Vec<f32>> = sets
        .iter()
        .map(|(_, set)| {
            let mut s = FmSynth::new();
            for (id, v) in *set {
                s.set_param(id, *v);
            }
            crate::plugins::golden::fingerprint(&mut s)
        })
        .collect();
    let names: Vec<&str> = sets.iter().map(|(n, _)| *n).collect();
    crate::plugins::golden::check(&names, &got, reference::REF, "src/plugins/fm/reference.rs");
}
