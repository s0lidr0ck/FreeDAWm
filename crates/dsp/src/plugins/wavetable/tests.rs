use super::reference;
use super::*;
use crate::plugins::fs::FsPreset;

const SR: f32 = 48_000.0;

fn synth(set: &[(&str, f32)], ch: usize) -> WavetableSynth {
    let mut s = WavetableSynth::new();
    s.prepare(SR, 512, ch);
    for (id, v) in set {
        assert!(s.set_param(id, *v), "{id}");
    }
    s
}

/// Renders `frames` (stereo) with notes `(at, note, velocity)` held to the end, in blocks of
/// `block`.
fn render_with(s: &mut WavetableSynth, notes: &[(usize, u8, u8)], frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut io = vec![vec![0.0f32; block]; 2];
    let mut at = 0;
    while at < frames {
        for (when, note, vel) in notes.iter().filter(|(w, _, _)| (at..at + block).contains(w)) {
            s.note_on(when - at, *note, *vel);
        }
        s.process(&mut io, block);
        l.extend_from_slice(&io[0]);
        r.extend_from_slice(&io[1]);
        at += block;
    }
    (l, r)
}

fn render(set: &[(&str, f32)], notes: &[(usize, u8, u8)], frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut s = synth(set, 2);
    render_with(&mut s, notes, frames, 256)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Hann-windowed DFT magnitude of `x` at `hz`.
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

/// What folds back, against the harmonics, in dB: energy at every frequency a harmonic up to
/// the 200th (or an interpolation image of one) lands on once folded, more than 40 Hz from any
/// harmonic, over the energy at the harmonics below half the sample rate.
fn aliasing_db(x: &[f32], f0: f64) -> f64 {
    let nyq = f64::from(SR) / 2.0;
    let near_harmonic = |f: f64| {
        let k = (f / f0).round();
        (f - k * f0).abs() < 40.0
    };
    let mut harm = 0.0;
    let mut k = 1.0;
    while k * f0 < nyq - 100.0 {
        harm += mag(x, k * f0).powi(2);
        k += 1.0;
    }
    let mut alias = 0.0;
    let mut seen: Vec<f64> = Vec::new();
    for h in 1..=200 {
        for j in 1..=2 {
            let f = (f64::from(SR) * j as f64 - h as f64 * f0).abs();
            if f > 40.0 && f < nyq - 40.0 && !near_harmonic(f) && !seen.iter().any(|s| (s - f).abs() < 5.0) {
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
    for p in &presets::WAVETABLE_PRESETS {
        assert!(names.insert(p.name.to_lowercase()), "{}", p.name);
        assert!(!p.group.is_empty() && !p.desc.is_empty());
        for (id, v) in p.set {
            let info = WAVETABLE_INFO.param(id).unwrap_or_else(|| panic!("{}: {id}", p.name));
            assert_eq!(info.clamp(*v), *v, "{}: {id}", p.name);
        }
    }
    for (name, json) in presets::factory_presets() {
        let p = FsPreset::parse(json, &WAVETABLE_INFO).unwrap();
        assert_eq!(&p.name, name);
        assert_eq!(p.values.len(), WAVETABLE_INFO.params.len());
    }
    assert_eq!(
        crate::plugins::fs::factory_presets(WAVETABLE_INFO.id).len(),
        presets::WAVETABLE_PRESETS.len() + presets::PATCHED_PRESETS.len() + super::suite::SUITE.len()
    );
}

/// The band limit at work: a saw-like frame at 2.6 kHz read at the level `level_for` picks
/// leaves next to nothing between the harmonics, where the fullest level (what an oscillator
/// without the mip-map would read) folds its upper harmonics back.
#[test]
fn the_mip_map_keeps_a_high_note_from_aliasing() {
    let t = tables::tables();
    let f0 = f64::from(note_hz(100.0));
    let inc = (f0 / f64::from(SR)) as f32;
    let read = |level: usize| {
        let mut ph = 0.0f32;
        (0..16_384)
            .map(|_| {
                let y = t.read(0, 0.66, level, ph);
                ph = (ph + inc).fract();
                y
            })
            .collect::<Vec<f32>>()
    };
    let ours = aliasing_db(&read(tables::level_for(inc)), f0);
    let naive = aliasing_db(&read(0), f0);
    eprintln!("aliasing at {f0:.0} Hz: band-limited {ours:.1} dB, fullest table {naive:.1} dB");
    assert!(ours < -70.0, "band-limited: {ours:.1} dB");
    assert!(naive > -40.0, "the fullest table does fold back ({naive:.1} dB)");
    // Through the whole synth, the filter open and seven detuned copies: still well down.
    let (l, _) = render(
        &[("position", 66.0), ("cutoff", 20_000.0), ("unison", 6.0), ("unison_spread", 0.0), ("amp_sustain", 100.0)],
        &[(0, 100, 127)],
        20_480,
    );
    let synth = aliasing_db(&l[4_096..], f0);
    eprintln!("aliasing through the synth with unison: {synth:.1} dB");
    // The detuned copies put energy within ±20 cents of each harmonic, wider than the 40 Hz
    // kept clear of them at the top, so this bound is looser.
    assert!(synth < -50.0, "{synth:.1} dB");
}

/// The tables' frames morph: the Harmonic Sweep is darker at its start than at its end, and
/// the Pulse Width table's narrow end has more of its upper harmonics than its square start.
#[test]
fn position_moves_through_the_table() {
    let t = tables::tables();
    let bright = |bank: usize, pos: f32| {
        let mut x = vec![0.0f32; 4096];
        let mut ph = 0.0f32;
        for y in &mut x {
            *y = t.read(bank, pos, 0, ph);
            ph = (ph + 64.0 / 4096.0).fract();
        }
        // Harmonics 8 to 30 against 1 to 3 (64 cycles in 4096 samples at 48 kHz: f0 = 750 Hz).
        let f0 = 750.0;
        let hi: f64 = (8..30).map(|k| mag(&x, k as f64 * f0)).sum();
        let lo: f64 = (1..4).map(|k| mag(&x, k as f64 * f0)).sum();
        hi / lo
    };
    assert!(bright(1, 1.0) > bright(1, 0.1) * 10.0, "{} {}", bright(1, 1.0), bright(1, 0.1));
    assert!(bright(2, 1.0) > bright(2, 0.0) * 2.0);
    // Between two frames the read is their crossfade.
    let (a, b) = (t.read(5, 0.0, 0, 0.3), t.read(5, 1.0 / 31.0, 0, 0.3));
    assert!((t.read(5, 0.5 / 31.0, 0, 0.3) - 0.5 * (a + b)).abs() < 1e-5);
}

/// Envelope 2, velocity, the mod wheel and the LFO each move the position.
#[test]
fn envelope_velocity_wheel_and_lfo_move_the_position() {
    let base = [("table", 1.0), ("position", 0.0), ("amp_sustain", 100.0), ("cutoff", 20_000.0)];
    let with = |extra: &[(&'static str, f32)]| base.iter().copied().chain(extra.iter().copied()).collect::<Vec<_>>();
    // Brightness: energy above 2 kHz over the whole, on A3 (220 Hz).
    let bright = |x: &[f32]| (10..40).map(|k| mag(x, 220.0 * k as f64)).sum::<f64>() / (1..4).map(|k| mag(x, 220.0 * k as f64)).sum::<f64>();
    let plain = render(&base, &[(0, 57, 100)], 24_000).0;
    // Envelope 2 opens it at the start, then decays back (its sustain is 0).
    let env = render(&with(&[("env2_position", 100.0), ("env2_decay", 200.0)]), &[(0, 57, 100)], 24_000).0;
    assert!(bright(&env[256..2_304]) > bright(&plain[256..2_304]) * 5.0);
    assert!(bright(&env[20_000..]) < bright(&env[256..2_304]) * 0.5);
    // Velocity: a hard key brighter than a soft one.
    let vel = with(&[("velocity_position", 100.0)]);
    let soft = render(&vel, &[(0, 57, 20)], 8_192).0;
    let hard = render(&vel, &[(0, 57, 127)], 8_192).0;
    assert!(bright(&hard[2_048..]) > bright(&soft[2_048..]) * 3.0);
    // The mod wheel.
    let mut s = synth(&with(&[("wheel_position", 100.0)]), 2);
    let down = render_with(&mut s, &[(0, 57, 100)], 8_192, 256).0;
    s.midi(0, [0xB0, 1, 127]);
    let up = render_with(&mut s, &[], 8_192, 256).0;
    assert!(bright(&up[2_048..]) > bright(&down[2_048..]) * 3.0);
    // The LFO: brightness rises and falls with it.
    let lfo = render(&with(&[("lfo_position", 100.0), ("lfo_rate", 2.0), ("position", 50.0)]), &[(0, 57, 100)], 48_000).0;
    let (a, b) = (bright(&lfo[5_000..8_000]), bright(&lfo[17_000..20_000]));
    assert!(a > b * 2.0 || b > a * 2.0, "{a} {b}");
}

#[test]
fn unison_spreads_and_a_single_copy_is_the_same_on_both_sides() {
    let (l, r) = render(&[("amp_sustain", 100.0)], &[(0, 57, 100)], 8_192);
    assert_eq!(l, r);
    let (l, r) = render(&[("amp_sustain", 100.0), ("unison", 4.0), ("unison_spread", 100.0)], &[(0, 57, 100)], 24_000);
    let diff: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
    assert!(rms(&diff[4_000..]) > rms(&l[4_000..]) * 0.2);
    // The stack keeps one copy's level within 3 dB.
    let one = rms(&render(&[("amp_sustain", 100.0)], &[(0, 57, 100)], 24_000).0[4_000..]);
    let both = ((rms(&l[4_000..]).powi(2) + rms(&r[4_000..]).powi(2)) / 2.0).sqrt();
    assert!((20.0 * (both / one).log10()).abs() < 3.0, "{both} {one}");
}

/// Seventeen keys on sixteen voices: the oldest released or quietest gives way, nothing breaks;
/// the sustain pedal holds released notes until it is lifted.
#[test]
fn voice_stealing_and_the_sustain_pedal() {
    let mut s = synth(&[("amp_sustain", 100.0), ("amp_release", 50.0)], 2);
    let mut io = vec![vec![0.0f32; 256]; 2];
    for n in 0..17u8 {
        s.note_on(0, 40 + n, 100);
    }
    s.process(&mut io, 256);
    assert_eq!(s.voices.iter().filter(|v| v.active()).count(), VOICES);
    assert!(s.voices.iter().any(|v| v.note == 56), "the newest key sounds");
    s.all_notes_off();
    for _ in 0..40 {
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
    for _ in 0..40 {
        s.process(&mut io, 256);
    }
    assert!(s.voices.iter().all(|v| !v.active()), "let go with the pedal");
}

/// Pitch bend moves the pitch by Bend Range; the output's fundamental follows.
#[test]
fn pitch_bend_and_the_wheel_vibrato() {
    let mut s = synth(&[("amp_sustain", 100.0), ("table", 0.0)], 2);
    s.midi(0, [0xE0, 0x7F, 0x7F]);
    let (l, _) = render_with(&mut s, &[(0, 69, 100)], 16_384, 256);
    // A4 bent up two semitones: B4 (493.9 Hz), not A4.
    assert!(mag(&l[4_096..], 493.88) > mag(&l[4_096..], 440.0) * 10.0);
    s.midi(0, [0xB0, 121, 0]);
    s.set_param("mod_wheel", 50.0);
    s.midi(0, [0xB0, 1, 127]);
    let mut io = vec![vec![0.0f32; 64]; 2];
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for _ in 0..400 {
        s.process(&mut io, 64);
        lo = lo.min(s.pitch_cents());
        hi = hi.max(s.pitch_cents());
    }
    assert!(lo < -45.0 && hi > 45.0, "{lo} {hi}");
}

/// The same notes rendered in different block sizes are the same samples (a bounce and
/// playback use different blocks).
#[test]
fn block_size_does_not_change_the_sound() {
    let set = [("table", 3.0), ("env2_position", 50.0), ("unison", 2.0), ("lfo_position", 30.0)];
    let notes = [(0usize, 48u8, 100u8), (700, 55, 90), (3_001, 60, 127)];
    let mut a = synth(&set, 2);
    let mut b = synth(&set, 2);
    let x = render_with(&mut a, &notes, 12_288, 64);
    let y = render_with(&mut b, &notes, 12_288, 512);
    assert_eq!(x, y);
}

/// Levels: a held chord at the defaults and with every preset stays under full scale and well
/// above silence.
#[test]
fn the_defaults_and_presets_play_at_sensible_levels() {
    let chord = [(0usize, 48u8, 110u8), (0, 55, 110), (0, 60, 110), (0, 64, 110)];
    let mut sets: Vec<(&str, &[(&str, f32)])> = vec![("defaults", &[])];
    sets.extend(presets::WAVETABLE_PRESETS.iter().map(|p| (p.name, p.set)));
    for (name, set) in sets {
        let (l, r) = render(set, &chord, 48_000);
        let peak = l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs()));
        let level = 20.0 * rms(&l[24_000..]).max(rms(&l[4_000..12_000])).log10();
        assert!(peak < 1.0 && level > -40.0, "{name}: peak {peak}, rms {level:.1} dB");
    }
}

/// What the synth plays at its defaults and with each factory preset (a chord, a bend, the mod
/// wheel, releases and a late note), pinned as a fingerprint within
/// [`crate::plugins::golden::TOLERANCE`] (1e-4, -80 dB; not bit for bit, see that module), so a
/// change to the sound is a change on purpose.
#[test]
fn the_defaults_and_the_factory_presets_are_pinned() {
    let sets: Vec<(&str, &[(&str, f32)])> =
        std::iter::once(("defaults", &[][..])).chain(presets::WAVETABLE_PRESETS.iter().map(|p| (p.name, p.set))).collect();
    let got: Vec<Vec<f32>> = sets
        .iter()
        .map(|(_, set)| {
            let mut s = WavetableSynth::new();
            for (id, v) in *set {
                s.set_param(id, *v);
            }
            crate::plugins::golden::fingerprint(&mut s)
        })
        .collect();
    let names: Vec<&str> = sets.iter().map(|(n, _)| *n).collect();
    crate::plugins::golden::check(&names, &got, reference::REF, "src/plugins/wavetable/reference.rs");
}
