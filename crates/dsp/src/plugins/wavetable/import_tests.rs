//! The slicing modes on synthetic signals: a known harmonic series comes back as itself, a
//! brightening sound gives brightening waves, the tables made do not alias at high notes, and
//! silence, a constant level and garbage are handled.

use super::super::tables::{FRAME_LEN, level_for};
use super::super::user::{BuiltTable, UserTable, read};
use super::*;

const SR: f32 = 48_000.0;
/// The series every mode must give back: (harmonic, amplitude).
const SERIES: [(usize, f32); 5] = [(1, 1.0), (2, 0.5), (3, 0.25), (5, 0.3), (8, 0.1)];

/// `secs` of the series at `f0` (Hz), its harmonics' amplitudes scaled by `shape(k, t)` (t: 0..1
/// through the signal).
fn tone(f0: f64, secs: f64, shape: impl Fn(usize, f64) -> f64, series: &[(usize, f32)]) -> Vec<f32> {
    let n = (f64::from(SR) * secs) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(SR);
            series
                .iter()
                .map(|(k, a)| f64::from(*a) * shape(*k, i as f64 / n as f64) * (std::f64::consts::TAU * f0 * *k as f64 * t).sin())
                .sum::<f64>() as f32
        })
        .collect()
}

/// A saw's harmonics (up to `h`) under a low-pass whose corner rises from `lo` to `hi` harmonics.
fn brightening(k: usize, t: f64) -> f64 {
    let kc = 2.0 * (40.0f64 / 2.0).powf(t);
    1.0 / (1.0 + (k as f64 / kc).powi(4)).sqrt()
}

fn saw_series(h: usize) -> Vec<(usize, f32)> {
    (1..=h).map(|k| (k, 1.0 / k as f32)).collect()
}

/// Each wave's harmonics 1..=8 relative to its first, against [`SERIES`].
fn assert_series(waves: &[Vec<f32>], tol: f32, what: &str) {
    assert!(!waves.is_empty(), "{what}: no waves");
    for (i, w) in waves.iter().enumerate() {
        assert_eq!(w.len(), WAVE_LEN);
        let a = harmonic_amplitudes(w, 12);
        for k in 1..=12 {
            let want = SERIES.iter().find(|(h, _)| *h == k).map_or(0.0, |(_, a)| *a);
            let got = a[k - 1] / a[0];
            assert!((got - want).abs() < tol, "{what} wave {i}: harmonic {k} at {got:.4}, want {want}");
        }
    }
}

fn pitch_track(len: usize, hop: usize, hz: f32) -> Vec<Option<f32>> {
    (0..len.div_ceil(hop)).map(|_| Some(hz)).collect()
}

#[test]
fn a_known_harmonic_series_comes_back_in_every_mode() {
    // Fixed: four frames, each one cycle of the series.
    let cycle = tone(f64::from(SR) / 2048.0, 2048.0 / f64::from(SR), |_, _| 1.0, &SERIES);
    let file: Vec<f32> = cycle.iter().copied().cycle().take(4 * 2048).collect();
    let f = fixed(&file, 2048);
    assert_eq!(f.waves.len(), 4);
    assert!(f.notes.is_empty(), "{:?}", f.notes);
    assert_series(&f.waves, 1e-3, "fixed");
    // The same as 1024-sample frames, and a lone short cycle (600 samples, as single-cycle
    // collections are): resampled through the spectrum.
    let half: Vec<f32> = (0..1024).map(|i| cycle[i * 2]).collect();
    assert_series(&fixed(&half.repeat(3), 1024).waves, 1e-3, "fixed 1024");
    let short: Vec<f32> = (0..600).map(|i| cycle[(i as f64 * 2048.0 / 600.0) as usize]).collect();
    let one = fixed(&short, 2048);
    assert_eq!((one.waves.len(), one.notes.len()), (1, 1));
    assert_series(&one.waves, 0.02, "fixed, a short cycle");
    // Pitch-tracked and spectral, at a pitch whose period is not a whole number of samples.
    for f0 in [220.0f32, 233.3] {
        let x = tone(f64::from(f0), 1.0, |_, _| 1.0, &SERIES);
        let p = pitched(&x, SR, &pitch_track(x.len(), 240, f0 * 1.004), 240, 8);
        assert_eq!(p.waves.len(), 8, "{:?}", p.notes);
        assert_series(&p.waves, 0.01, "pitched");
        // Turned to one phase: the waves are the same wave.
        for w in &p.waves[1..] {
            let d = w.iter().zip(&p.waves[0]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(d < 0.02, "pitched waves differ by {d}");
        }
        let s = spectral(&x, SR, 6, Some(f0));
        assert_eq!(s.waves.len(), 6);
        assert_series(&s.waves, 0.03, "spectral");
    }
}

#[test]
fn a_brightening_sound_gives_brightening_waves_in_every_mode() {
    let series = saw_series(60);
    let rising = |waves: &[Vec<f32>], what: &str| {
        let b: Vec<f32> = waves.iter().map(|w| brightness(w)).collect();
        eprintln!("{what}: brightness {:?}", b.iter().map(|x| format!("{x:.1}")).collect::<Vec<_>>());
        assert!(b.windows(2).all(|w| w[1] >= w[0] * 0.97), "{what}: {b:?}");
        assert!(b[b.len() - 1] > 3.0 * b[0], "{what}: {b:?}");
    };
    // Fixed: 16 frames, each brighter.
    let mut file = Vec::new();
    for i in 0..16 {
        let t = i as f64 / 15.0;
        file.extend(tone(f64::from(SR) / 2048.0, 2048.0 / f64::from(SR), |k, _| brightening(k, t), &series));
    }
    rising(&fixed(&file, 2048).waves, "fixed");
    let x = tone(110.0, 2.0, brightening, &series);
    rising(&pitched(&x, SR, &pitch_track(x.len(), 240, 110.0), 240, 12).waves, "pitched");
    rising(&spectral(&x, SR, 12, Some(110.0)).waves, "spectral");
    rising(&spectral(&x, SR, 12, None).waves, "spectral without a pitch");
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

/// The built-in tables' aliasing measure (`wavetable::tests::aliasing_db`): energy where
/// harmonics up to the 200th fold back to, more than 40 Hz from any harmonic, over the energy at
/// the harmonics, dB.
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
fn imported_tables_do_not_alias_at_high_notes() {
    // Bright sources: a saw with every harmonic to half the rate (pitched, spectral) and a
    // 2048-frame saw table (fixed).
    let saw: Vec<f32> = (0..WAVE_LEN).map(|i| 2.0 * i as f32 / WAVE_LEN as f32 - 1.0).collect();
    let x = tone(93.75, 1.0, |_, _| 1.0, &saw_series(255));
    let mut noise = 0x9E37_79B9u32;
    let hiss: Vec<f32> = (0..48_000)
        .map(|_| {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            noise as f32 / u32::MAX as f32 - 0.5
        })
        .collect();
    let tables = [
        ("fixed", fixed(&saw.repeat(2), 2048)),
        ("pitched", pitched(&x, SR, &pitch_track(x.len(), 240, 93.75), 240, 4)),
        ("spectral", spectral(&x, SR, 4, Some(93.75))),
        ("spectral noise", spectral(&hiss, SR, 4, None)),
    ];
    for (what, sliced) in tables {
        let built = BuiltTable::new(&UserTable::new(what, &sliced.waves).unwrap());
        assert_eq!(built.data().len(), sliced.waves.len() * FRAME_LEN);
        let f0 = 2_637.0f64; // MIDI note 100
        let inc = (f0 / f64::from(SR)) as f32;
        let read_at = |level: usize| {
            let mut ph = 0.0f32;
            (0..16_384)
                .map(|_| {
                    let y = read(built.data(), 0.4, level, ph);
                    ph = (ph + inc).fract();
                    y
                })
                .collect::<Vec<f32>>()
        };
        let ours = aliasing_db(&read_at(level_for(inc)), f0);
        let naive = aliasing_db(&read_at(0), f0);
        eprintln!("{what}: band-limited {ours:.1} dB, fullest level {naive:.1} dB");
        assert!(ours < -70.0, "{what}: {ours:.1} dB");
        assert!(naive > ours + 20.0, "{what}: the fullest level would fold back ({naive:.1} dB)");
    }
}

#[test]
fn silence_a_constant_level_and_garbage_are_handled() {
    let finite = |s: &Sliced| s.waves.iter().flatten().all(|v| v.is_finite());
    let quiet = |s: &Sliced| s.waves.iter().flatten().all(|v| v.abs() < 1e-6);
    let silence = vec![0.0f32; 48_000];
    let dc = vec![0.4f32; 48_000];
    for x in [&silence, &dc] {
        let f = fixed(x, 2048);
        assert!(finite(&f) && f.waves.len() == x.len() / 2048);
        let built = BuiltTable::new(&UserTable::new("x", &f.waves).unwrap());
        assert!(built.data().iter().all(|v| *v == 0.0), "a constant level band-limits to silence");
        let s = spectral(x, SR, 8, None);
        assert!(finite(&s) && quiet(&s) && s.waves.len() == 8);
        let s = spectral(x, SR, 8, Some(200.0));
        assert!(finite(&s) && quiet(&s));
        let p = pitched(x, SR, &vec![None; 200], 240, 8);
        assert!(p.waves.is_empty() && p.notes.len() == 1);
    }
    // Empty input, garbage samples, nonsense pitch and rates.
    assert!(fixed(&[], 2048).waves.is_empty());
    assert!(spectral(&[], SR, 4, None).waves.is_empty());
    assert!(pitched(&[], SR, &[Some(100.0)], 240, 4).waves.is_empty());
    let junk: Vec<f32> = (0..9_000).map(|i| [f32::NAN, f32::INFINITY, 0.3, -1e30][i % 4]).collect();
    for s in [
        fixed(&junk, 2048),
        fixed(&junk, 3),
        spectral(&junk, SR, 40, None),
        spectral(&junk, f32::NAN, 4, Some(f32::NAN)),
        pitched(&junk, SR, &[Some(f32::NAN), Some(-3.0), Some(1e9), Some(200.0)], 0, 99),
        pitched(&junk, 0.0, &[Some(200.0)], 240, 4),
    ] {
        assert!(finite(&s), "{:?}", s.notes);
        assert!(s.waves.len() <= MAX_WAVES);
    }
}

#[test]
fn frame_sizes_come_from_the_clm_chunk_or_the_length() {
    // A RIFF/WAVE with a `clm ` chunk naming 1024 (written here, with a pad byte after an odd chunk).
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    wav.extend_from_slice(b"junk\x03\0\0\0abc\0");
    let clm = b"<!>1024 10000000 wavetable";
    wav.extend_from_slice(b"clm ");
    wav.extend_from_slice(&(clm.len() as u32).to_le_bytes());
    wav.extend_from_slice(clm);
    assert_eq!(clm_frame_size(&wav), Some(1024));
    assert_eq!(guess_frame_size(5000, clm_frame_size(&wav)), (1024, false));
    // Another size named, or none, or a broken file: no size from the chunk.
    assert_eq!(clm_frame_size(&wav.iter().map(|b| if *b == b'1' { b'3' } else { *b }).collect::<Vec<_>>()), None);
    assert_eq!(clm_frame_size(b"RIFF\0\0\0\0WAVEclm \xff\xff\xff\xff<!>2048"), Some(2048));
    assert_eq!(clm_frame_size(b"RIFF"), None);
    assert_eq!(clm_frame_size(b"RIFF\0\0\0\0WAVEfmt \xff\xff\xff\xff"), None);
    let mut x = 7u32;
    for len in [0usize, 12, 20, 300] {
        let junk: Vec<u8> = b"RIFF\0\0\0\0WAVE"
            .iter()
            .copied()
            .chain((0..len).map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            }))
            .collect();
        let _ = clm_frame_size(&junk);
    }
    // From the length.
    assert_eq!(guess_frame_size(256 * 2048, None), (2048, true));
    assert_eq!(guess_frame_size(3 * 1024, None), (1024, true));
    assert_eq!(guess_frame_size(5 * 4096, None).0, 2048, "20480 is ten 2048s");
    assert_eq!(guess_frame_size(3 * 4096, None).0, 2048);
    assert_eq!(guess_frame_size(7 * 256, None), (256, true));
    assert_eq!(guess_frame_size(1000, None), (2048, true));
    // More than 32 frames: 32 spread through them, first and last included.
    let ramp: Vec<f32> = (0..100 * 256).map(|i| ((i / 256) as f32 * 0.37).sin()).collect();
    let f = fixed(&ramp, 256);
    assert_eq!(f.waves.len(), MAX_WAVES);
    assert!(f.notes.iter().any(|n| n.contains("100 frames")));
}
