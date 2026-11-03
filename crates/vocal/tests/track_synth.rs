//! The pitch tracker against synthetic signals with a known pitch.

mod common;

use common::{harmonic_tone, harmonic_tone_with, hz_to_cents, median, no_cancel, noise};
use soundcraft_vocal::{PitchTrack, VocalError, track_pitch};
use std::sync::atomic::AtomicBool;

fn mid(t: &PitchTrack) -> &[soundcraft_vocal::PitchFrame] {
    &t.frames[40..t.frames.len() - 40]
}

fn unvoiced_share(t: &PitchTrack) -> f32 {
    t.frames.iter().filter(|f| f.cents.is_none()).count() as f32 / t.frames.len() as f32
}

#[test]
fn tracks_a_steady_tone_within_one_cent() {
    // 220 Hz * 2^(17/1200), 12 harmonics, 1 s, 48 kHz
    let x = harmonic_tone(48_000, 1.0, |_| 220.0 * 2f32.powf(17.0 / 1200.0), 12);
    let t = track_pitch(&x, 48_000, &no_cancel()).unwrap();
    let mid = mid(&t);
    assert!(mid.iter().all(|f| f.cents.is_some()));
    let med = median(mid.iter().filter_map(|f| f.cents));
    assert!((med - (5700.0 + 17.0)).abs() <= 1.0, "{med}"); // A3 = 5700
    assert_eq!(t.hop_samples, 240);
    assert_eq!(t.sample_rate, 48_000);
}

#[test]
fn follows_vibrato() {
    // 330 Hz ± 30 c at 5.5 Hz: every mid frame within 5 c of the true curve
    let curve = |t: f32| 330.0 * 2f32.powf(30.0 * (std::f32::consts::TAU * 5.5 * t).sin() / 1200.0);
    let x = harmonic_tone(48_000, 1.5, curve, 12);
    let t = track_pitch(&x, 48_000, &no_cancel()).unwrap();
    let mut worst = 0.0f32;
    for (i, f) in t.frames.iter().enumerate().take(t.frames.len() - 40).skip(40) {
        let c = f.cents.unwrap_or_else(|| panic!("frame {i} unvoiced"));
        let truth = hz_to_cents(curve(t.sample_of_frame(i) as f32 / 48_000.0));
        worst = worst.max((c - truth).abs());
    }
    assert!(worst <= 5.0, "worst {worst} c");
}

#[test]
fn tracks_at_44100() {
    let x = harmonic_tone(44_100, 1.0, |_| 220.0 * 2f32.powf(17.0 / 1200.0), 12);
    let t = track_pitch(&x, 44_100, &no_cancel()).unwrap();
    assert_eq!(t.hop_samples, 221); // round(44100 / 200)
    let mid = mid(&t);
    assert!(mid.iter().all(|f| f.cents.is_some()));
    let med = median(mid.iter().filter_map(|f| f.cents));
    assert!((med - 5717.0).abs() <= 1.0, "{med}");
}

#[test]
fn noise_is_unvoiced() {
    for seed in 1u64..=20 {
        let t = track_pitch(&noise(48_000, 1.0, seed), 48_000, &no_cancel()).unwrap();
        let u = unvoiced_share(&t);
        assert!(u >= 0.95, "seed {seed}: only {u} unvoiced");
    }
}

#[test]
fn out_of_range_tones_are_unvoiced() {
    for hz in [50.0f32, 1500.0] {
        let t = track_pitch(&harmonic_tone(48_000, 1.0, |_| hz, 1), 48_000, &no_cancel()).unwrap();
        let u = unvoiced_share(&t);
        assert!(u >= 0.9, "{hz} Hz: only {u} unvoiced");
    }
}

#[test]
fn weak_fundamental_has_no_octave_errors() {
    // 200 Hz, fundamental at -30 dB vs h2 (h2.. at 1/k)
    let amp = |k: usize| if k == 1 { 0.5 * 10f64.powf(-30.0 / 20.0) } else { 1.0 / k as f64 };
    let x = harmonic_tone_with(48_000, 1.0, |_| 200.0, 12, amp);
    let t = track_pitch(&x, 48_000, &no_cancel()).unwrap();
    let truth = hz_to_cents(200.0);
    let mid = mid(&t);
    let voiced: Vec<f32> = mid.iter().filter_map(|f| f.cents).collect();
    assert!(voiced.len() as f32 >= 0.9 * mid.len() as f32, "{} of {} voiced", voiced.len(), mid.len());
    for c in voiced {
        assert!((c - truth).abs() <= 600.0, "{c} vs {truth}");
    }
}

#[test]
fn short_silent_and_nan_input_give_no_notes() {
    for x in [vec![], vec![0.0; 100], vec![0.0; 48_000], vec![f32::NAN; 4800], vec![f32::INFINITY; 4800]] {
        let t = track_pitch(&x, 48_000, &no_cancel()).unwrap();
        assert!(t.frames.iter().all(|f| f.cents.is_none()));
        assert!(t.frames.iter().all(|f| f.level_db.is_finite() && f.voiced_prob.is_finite()));
    }
}

#[test]
fn cancel_returns_cancelled() {
    let x = harmonic_tone(48_000, 1.0, |_| 220.0, 12);
    let r = track_pitch(&x, 48_000, &AtomicBool::new(true));
    assert!(matches!(r, Err(VocalError::Cancelled)));
}

#[test]
fn frame_sample_mapping_round_trips() {
    let t = track_pitch(&vec![0.0; 4800], 48_000, &no_cancel()).unwrap();
    assert_eq!(t.frames.len(), 20);
    assert_eq!(t.sample_of_frame(3), 720);
    assert_eq!(t.frame_of_sample(720), 3);
    assert_eq!(t.frame_of_sample(839), 3);
    assert_eq!(t.frame_of_sample(841), 4);
    assert_eq!(t.frame_of_sample(u64::MAX), 19);
}

#[test]
fn bad_sample_rate_is_unsupported() {
    let r = track_pitch(&[0.0; 10], 0, &no_cancel());
    assert!(matches!(r, Err(VocalError::Unsupported(_))));
}
