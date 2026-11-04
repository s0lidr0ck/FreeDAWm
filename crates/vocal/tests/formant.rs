//! Formant shift (Vocal Studio spec 2026-11-03 §4): the spectral envelope moves by the asked
//! semitones while the pitch stays where the shift curve puts it, per clip and per note.

mod common;

use common::{harmonic_tone_with, median, no_cancel, noise};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use soundcraft_vocal::resynth::{changed_frames_formant, render_spans_formant, splice, stft_sizes};
use soundcraft_vocal::{EditCurves, NoteEdit, PitchTrack, analyse, formant_curve, track_pitch};
use std::collections::BTreeMap;
use std::sync::OnceLock;

const FORMANTS: [f64; 3] = [700.0, 1200.0, 2600.0];
const BANDWIDTHS: [f64; 3] = [100.0, 110.0, 160.0];
const SR: u32 = 48_000;

fn vowel(secs: f32) -> Vec<f32> {
    let f0 = 150.0;
    let amp = |k: usize| {
        let f = f0 * k as f64;
        let res = |i: usize, a: f64| a / (1.0 + ((f - FORMANTS[i]) / BANDWIDTHS[i]).powi(2));
        res(0, 1.0) + res(1, 0.6) + res(2, 0.3) + 0.01 / k as f64
    };
    harmonic_tone_with(SR, secs, |_| f0 as f32, 60, amp)
}

struct Fx {
    x: Vec<f32>,
    t: PitchTrack,
    v1_end: usize,
}

/// A vowel, a little noise, a vowel.
fn fx() -> &'static Fx {
    static F: OnceLock<Fx> = OnceLock::new();
    F.get_or_init(|| {
        let mut x = vowel(0.5);
        let v1_end = x.len();
        x.extend(noise(SR, 0.2, 7).iter().map(|v| v * 0.3));
        x.extend(vowel(0.5));
        let t = track_pitch(&x, SR, &no_cancel()).unwrap();
        Fx { x, t, v1_end }
    })
}

fn render(f: &Fx, shift: f32, formant: f32) -> Vec<f32> {
    let n = f.t.frames.len();
    let hop = f.t.hop_samples as usize;
    let end = (f.v1_end / hop).min(n);
    let mut c = EditCurves { shift_cents: vec![0.0; n], gain_db: vec![0.0; n] };
    let mut fm = vec![0.0f32; n];
    for (i, fr) in f.t.frames.iter().enumerate().take(end) {
        if fr.cents.is_some() {
            c.shift_cents[i] = shift;
            fm[i] = formant;
        }
    }
    let frames = changed_frames_formant(None, &c, &fm);
    let spans = render_spans_formant(&f.x, &f.t, &c, &fm, &frames, &no_cancel()).unwrap();
    let mut y = f.x.clone();
    splice(&mut y, &spans);
    y
}

fn tracked_shift(f: &Fx, y: &[f32], a: usize, b: usize) -> f32 {
    let ty = track_pitch(y, SR, &no_cancel()).unwrap();
    let hop = f.t.hop_samples as usize;
    median((a / hop..b / hop).filter_map(|i| Some(ty.frames.get(i)?.cents? - f.t.frames.get(i)?.cents?)))
}

/// The mean cepstral envelope (natural-log magnitude) over `[a, b)`.
fn envelope(x: &[f32], a: usize, b: usize) -> Vec<f64> {
    let (n, h, lifter) = stft_sizes(SR);
    let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
    let win: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).collect();
    let mut acc = vec![0.0f64; n / 2 + 1];
    let mut count = 0;
    let mut p = a;
    while p + n <= b {
        let mut buf: Vec<Complex<f64>> = (0..n).map(|i| Complex::new(f64::from(x[p + i]) * win[i], 0.0)).collect();
        fft.process(&mut buf);
        let mut cep: Vec<Complex<f64>> = buf.iter().map(|z| Complex::new((z.norm() + 1e-12).ln(), 0.0)).collect();
        fft.process(&mut cep);
        for (q, z) in cep.iter_mut().enumerate() {
            let keep = q < lifter || q > n - lifter;
            *z = if keep { Complex::new(z.re / n as f64, 0.0) } else { Complex::default() };
        }
        fft.process(&mut cep);
        for (k, v) in acc.iter_mut().enumerate() {
            *v += cep[k].re;
        }
        count += 1;
        p += h;
    }
    assert!(count > 0);
    acc.iter().map(|v| v / count as f64).collect()
}

/// RMS difference (dB) of two log envelopes over 300–4000 Hz, the mean difference removed.
fn distance_db(a: &[f64], b: &[f64]) -> f64 {
    let bin_hz = f64::from(SR) / (2 * (a.len() - 1)) as f64;
    let d: Vec<f64> = a
        .iter()
        .zip(b)
        .enumerate()
        .filter(|(k, _)| (300.0..=4000.0).contains(&(*k as f64 * bin_hz)))
        .map(|(_, (x, y))| (x - y) * 20.0 / std::f64::consts::LN_10)
        .collect();
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    (d.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / d.len() as f64).sqrt()
}

/// `env` with every frequency multiplied by `r`.
fn scaled(env: &[f64], r: f64) -> Vec<f64> {
    (0..env.len())
        .map(|k| {
            let src = k as f64 / r;
            let i = src.floor() as usize;
            match (env.get(i), env.get(i + 1)) {
                (Some(a), Some(b)) => a + (b - a) * (src - i as f64),
                _ => *env.last().unwrap(),
            }
        })
        .collect()
}

/// The envelope's peak frequency (Hz) between `lo` and `hi`.
fn peak_hz(env: &[f64], lo: f64, hi: f64) -> f64 {
    let bin_hz = f64::from(SR) / (2 * (env.len() - 1)) as f64;
    (0..env.len()).filter(|k| (lo..=hi).contains(&(*k as f64 * bin_hz))).max_by(|a, b| env[*a].total_cmp(&env[*b])).unwrap() as f64 * bin_hz
}

#[test]
fn formant_shift_moves_the_envelope_and_keeps_the_pitch() {
    let f = fx();
    let (a, b) = (SR as usize / 10, f.v1_end - SR as usize / 10);
    let y = render(f, 0.0, 4.0);
    let got = tracked_shift(f, &y, a, b);
    assert!(got.abs() <= 3.0, "the pitch moved by {got} c");
    let (ei, eo) = (envelope(&f.x, a, b), envelope(&y, a, b));
    let r = 2f64.powf(4.0 / 12.0);
    let to_scaled = distance_db(&eo, &scaled(&ei, r));
    let to_input = distance_db(&eo, &ei);
    eprintln!("envelope vs scaled {to_scaled:.2} dB, vs input {to_input:.2} dB");
    assert!(to_scaled * 2.0 < to_input, "the envelope did not move: {to_scaled:.2} vs {to_input:.2} dB");
    // F3 (2600 Hz, resolved) moves up about four semitones.
    let (pi, po) = (peak_hz(&ei, 2200.0, 4000.0), peak_hz(&eo, 2200.0, 4000.0));
    assert!((po / pi - r).abs() < 0.06, "F3 {pi} Hz -> {po} Hz, want ×{r:.3}");
    // Down: the envelope moves the other way.
    let y = render(f, 0.0, -3.0);
    let eo = envelope(&y, a, b);
    let po = peak_hz(&eo, 1800.0, 3200.0);
    assert!(po < pi * 0.9, "F3 {pi} Hz -> {po} Hz down");
}

#[test]
fn formant_and_pitch_shift_are_independent() {
    let f = fx();
    let (a, b) = (SR as usize / 10, f.v1_end - SR as usize / 10);
    let y = render(f, 300.0, -2.0);
    let got = tracked_shift(f, &y, a, b);
    assert!((got - 300.0).abs() <= 4.0, "the pitch moved {got} c, not 300");
    let (ei, eo) = (envelope(&f.x, a, b), envelope(&y, a, b));
    // The envelope follows the formant shift (−2 st), not the pitch shift (+3 st).
    let by_formant = distance_db(&eo, &scaled(&ei, 2f64.powf(-2.0 / 12.0)));
    let by_pitch = distance_db(&eo, &scaled(&ei, 2f64.powf(3.0 / 12.0)));
    assert!(by_formant < by_pitch, "{by_formant:.2} dB vs {by_pitch:.2} dB");
}

#[test]
fn zero_formant_renders_as_before() {
    let f = fx();
    let n = f.t.frames.len();
    let c = EditCurves { shift_cents: vec![0.0; n], gain_db: vec![0.0; n] };
    assert!(changed_frames_formant(None, &c, &vec![0.0; n]).is_empty());
    // A formant-only change is a changed frame; the same curves again are not.
    let mut fm = vec![0.0f32; n];
    fm[10] = 1.0;
    assert_eq!(changed_frames_formant(None, &c, &fm), vec![10..11]);
    assert!(changed_frames_formant(Some((&c, &fm)), &c, &fm).is_empty());
    assert_eq!(changed_frames_formant(Some((&c, &fm)), &c, &[]), vec![10..11]);
}

#[test]
fn formant_curve_per_clip_and_note() {
    let f = fx();
    let a = analyse(&f.x, SR, &no_cancel()).unwrap();
    let none = BTreeMap::new();
    assert!(formant_curve(&a, &none, 0.0).iter().all(|v| *v == 0.0), "no shift: all zero");
    let clip = formant_curve(&a, &none, 2.5);
    for (v, fr) in clip.iter().zip(&a.track.frames) {
        if fr.cents.is_none() {
            assert_eq!(*v, 0.0, "unvoiced frames never shift");
        }
    }
    let voiced_mid = a.notes[0].start as usize + (a.notes[0].end - a.notes[0].start) as usize / 2;
    assert_eq!(clip[voiced_mid], 2.5);
    assert!(formant_curve(&a, &none, 40.0).iter().all(|v| v.abs() <= 12.0), "clamped to ±12");
    // A note's own shift adds on its frames only, ramped at its edges.
    let n0 = &a.notes[0];
    let edits = BTreeMap::from([(n0.id, NoteEdit { formant_st: -4.0, ..NoteEdit::default() })]);
    let c = formant_curve(&a, &edits, 1.0);
    assert_eq!(c[voiced_mid], -3.0);
    let other = &a.notes[a.notes.len() - 1];
    let mid2 = other.start as usize + (other.end - other.start) as usize / 2;
    assert_eq!(c[mid2], 1.0, "the other note keeps the clip's shift");
    // Saved only when set, and not an identity change of pitch or level.
    let e = NoteEdit { formant_st: 3.0, ..NoteEdit::default() };
    assert!(e.is_identity());
    assert!(serde_json::to_value(NoteEdit::default()).unwrap().get("formant_st").is_none());
    assert_eq!(serde_json::to_value(&e).unwrap()["formant_st"], 3.0);
    assert_eq!(NoteEdit { formant_st: f32::NAN, ..e.clone() }.clamped().formant_st, 0.0);
    assert_eq!(NoteEdit { formant_st: 30.0, ..e }.clamped().formant_st, 12.0);
}
