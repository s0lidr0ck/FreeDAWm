//! Resynthesis against a synthetic vowel: bit-identical passthrough, shift accuracy, formants,
//! untouched noise, gain, bounds and cancel.

mod common;

use common::{harmonic_tone_with, median, no_cancel, noise};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use soundcraft_vocal::resynth::{changed_frames, render_all, render_spans, splice, stft_sizes};
use soundcraft_vocal::{EditCurves, NoteEdit, PitchClassSet, PitchTrack, VocalError, analyse, build_curves, track_pitch};
use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

const FORMANTS: [f64; 3] = [700.0, 1200.0, 2600.0];

/// Formant bandwidths (Hz) of the test vowel.
/// Sung-vowel-like bandwidths (F1/F2 80–130 Hz, F3 150+ Hz).
const BANDWIDTHS: [f64; 3] = [100.0, 110.0, 160.0];

/// A 150 Hz harmonic series shaped by formant peaks at 700/1200/2600 Hz.
fn vowel(sr: u32, secs: f32) -> Vec<f32> {
    vowel_with(sr, secs, 150.0, BANDWIDTHS)
}

/// A harmonic series at `f0` shaped by resonances at [`FORMANTS`] with bandwidths `bw`
/// (amplitudes 1, 0.6, 0.3) over a gentle 1/k floor.
fn vowel_with(sr: u32, secs: f32, f0: f64, bw: [f64; 3]) -> Vec<f32> {
    let amp = |k: usize| {
        let f = f0 * k as f64;
        let res = |i: usize, a: f64| a / (1.0 + ((f - FORMANTS[i]) / bw[i]).powi(2));
        res(0, 1.0) + res(1, 0.6) + res(2, 0.3) + 0.01 / k as f64
    };
    harmonic_tone_with(sr, secs, |_| f0 as f32, 60, amp)
}

struct Fx {
    sr: u32,
    x: Vec<f32>,
    t: PitchTrack,
    /// First vowel: samples `[0, v1_end)`; noise `[v1_end, v2_start)`; second vowel to the end.
    v1_end: usize,
    v2_start: usize,
}

fn make_fixture(sr: u32) -> Fx {
    let mut x = vowel(sr, 0.5);
    let v1_end = x.len();
    x.extend(noise(sr, 0.2, 7).iter().map(|v| v * 0.3));
    let v2_start = x.len();
    x.extend(vowel(sr, 0.5));
    let t = track_pitch(&x, sr, &no_cancel()).unwrap();
    Fx { sr, x, t, v1_end, v2_start }
}

fn fx() -> &'static Fx {
    static F: OnceLock<Fx> = OnceLock::new();
    F.get_or_init(|| make_fixture(48_000))
}

fn nc() -> AtomicBool {
    no_cancel()
}

fn zero_curves(t: &PitchTrack) -> EditCurves {
    EditCurves { shift_cents: vec![0.0; t.frames.len()], gain_db: vec![0.0; t.frames.len()] }
}

/// Frames whose centre sample lies in `[a, b)`.
fn frames_in(t: &PitchTrack, a: usize, b: usize) -> Range<usize> {
    let hop = t.hop_samples as usize;
    let f0 = a.div_ceil(hop).min(t.frames.len());
    let f1 = b.div_ceil(hop).min(t.frames.len());
    f0..f1
}

fn curves(t: &PitchTrack, frames: Range<usize>, cents: f32, gain_db: f32) -> EditCurves {
    let mut c = zero_curves(t);
    for i in frames {
        c.shift_cents[i] = cents;
        c.gain_db[i] = gain_db;
    }
    c
}

/// Median of (output pitch - input pitch) over the frames of `[a, b)` where both are voiced.
fn tracked_shift(f: &Fx, y: &[f32], a: usize, b: usize) -> f32 {
    let ty = track_pitch(y, f.sr, &no_cancel()).unwrap();
    median(frames_in(&f.t, a, b).filter_map(|i| Some(ty.frames.get(i)?.cents? - f.t.frames[i].cents?)))
}

#[test]
fn zero_edit_is_bit_identical() {
    let f = fx();
    let y = render_all(&f.x, &f.t, &zero_curves(&f.t), &nc()).unwrap();
    assert_eq!(y, f.x);
}

#[test]
fn untouched_frames_are_bit_identical() {
    let f = fx();
    let (n, _, _) = stft_sizes(f.sr);
    let c = curves(&f.t, frames_in(&f.t, 0, f.v1_end), 50.0, 0.0);
    let y = render_all(&f.x, &f.t, &c, &nc()).unwrap();
    assert_eq!(y.len(), f.x.len());
    let from = f.v1_end + n + 480;
    assert_eq!(&y[from..], &f.x[from..]);
    assert_ne!(&y[..f.v1_end], &f.x[..f.v1_end], "the shifted vowel changed");
}

#[test]
fn shift_is_accurate() {
    let f = fx();
    let (a, b) = (f.sr as usize / 10, f.v1_end - f.sr as usize / 10);
    for target in [50.0f32, -30.0] {
        let c = curves(&f.t, frames_in(&f.t, 0, f.v1_end), target, 0.0);
        let y = render_all(&f.x, &f.t, &c, &nc()).unwrap();
        let got = tracked_shift(f, &y, a, b);
        assert!((got - target).abs() <= 1.0, "target {target} c, tracked {got} c");
    }
}

#[test]
fn note_offset_of_100_cents_renders_100_cents_up() {
    // Through the whole path: analysis, a dragged note's offset, its curves and the render.
    let f = fx();
    let a = analyse(&f.x, f.sr, &nc()).unwrap();
    let first = a.notes.first().expect("the first vowel is a note");
    let edits = std::collections::BTreeMap::from([(first.id, NoteEdit { offset_cents: 100.0, ..NoteEdit::default() })]);
    let c = build_curves(&a, &edits, &None::<PitchClassSet>);
    let y = render_all(&f.x, &a.track, &c, &nc()).unwrap();
    let (s0, s1) = (first.start as usize * a.track.hop_samples as usize, first.end as usize * a.track.hop_samples as usize);
    let (lo, hi) = (s0 + f.sr as usize / 10, s1.saturating_sub(f.sr as usize / 10));
    assert!(hi > lo + f.sr as usize / 10, "note {s0}..{s1}");
    let got = tracked_shift(f, &y, lo, hi);
    assert!((got - 100.0).abs() <= 2.0, "tracked {got} c");
}

#[test]
fn shift_is_accurate_at_44100() {
    let f = make_fixture(44_100);
    let (a, b) = (f.sr as usize / 10, f.v1_end - f.sr as usize / 10);
    let c = curves(&f.t, frames_in(&f.t, 0, f.v1_end), 50.0, 0.0);
    let y = render_all(&f.x, &f.t, &c, &nc()).unwrap();
    let got = tracked_shift(&f, &y, a, b);
    assert!((got - 50.0).abs() <= 1.5, "tracked {got} c");
}

/// The mean cepstral log envelope (natural log, one value per bin) over `[a, b)`, with
/// `stft_sizes`' N and lifter.
fn envelope(x: &[f32], sr: u32, a: usize, b: usize) -> Vec<f64> {
    let (n, h, lifter) = stft_sizes(sr);
    let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
    let win: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).collect();
    let mut acc = vec![0.0f64; n / 2 + 1];
    let mut count = 0;
    let mut p = a;
    while p + n <= b {
        let mut buf: Vec<Complex<f64>> = (0..n).map(|i| Complex::new(f64::from(x[p + i]) * win[i], 0.0)).collect();
        fft.process(&mut buf);
        // ln|X| is real and even: its (unnormalised) transform is the cepstrum times N.
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

/// Local maxima (Hz) of `env` below 4 kHz.
fn envelope_peaks(env: &[f64], sr: u32) -> Vec<f64> {
    let bin_hz = f64::from(sr) / (2 * (env.len() - 1)) as f64;
    (1..env.len() - 1).filter(|&k| env[k] > env[k - 1] && env[k] >= env[k + 1]).map(|k| k as f64 * bin_hz).filter(|&hz| hz < 4000.0).collect()
}

/// RMS difference (dB) between two log envelopes over 300–4000 Hz, after removing the mean
/// difference (an overall level change is not a formant move).
fn envelope_distance_db(a: &[f64], b: &[f64], sr: u32) -> f64 {
    let bin_hz = f64::from(sr) / (2 * (a.len() - 1)) as f64;
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

fn near(peaks: &[f64], fc: f64) -> f64 {
    peaks.iter().map(|p| (p - fc).abs() / fc).fold(f64::INFINITY, f64::min)
}

/// `env` with every frequency multiplied by `r` (what a shift that does not keep formants does).
fn scaled_envelope(env: &[f64], r: f64) -> Vec<f64> {
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

#[test]
fn formants_stay_put() {
    // The brief asks for envelope peaks within 3 % of 700/1200/2600 Hz. A 48-quefrency lifter
    // resolves about 1 kHz, so the input itself shows 700 and 1200 Hz as one merged peak (~800 Hz)
    // and 2600 Hz on its own. So, as the spike was judged: the output envelope against the input
    // envelope at the same lifter, each resolved peak in place, plus a shape distance.
    let f = fx();
    let (a, b) = (f.sr as usize / 10, f.v1_end - f.sr as usize / 10);
    let r = 2f64.powf(200.0 / 1200.0);
    let c = curves(&f.t, frames_in(&f.t, 0, f.v1_end), 200.0, 0.0);
    let y = render_all(&f.x, &f.t, &c, &nc()).unwrap();
    let got = tracked_shift(f, &y, a, b);
    assert!((got - 200.0).abs() <= 3.0, "the vowel moved by {got} c, not 200");
    let (ei, eo) = (envelope(&f.x, f.sr, a, b), envelope(&y, f.sr, a, b));
    let (pi, po) = (envelope_peaks(&ei, f.sr), envelope_peaks(&eo, f.sr));
    eprintln!("input peaks {pi:?}, output peaks {po:?}");
    // F3 is resolved: it stays within 3 %.
    assert!(near(&pi, FORMANTS[2]) <= 0.03 && near(&po, FORMANTS[2]) <= 0.03, "F3: input {pi:?} output {po:?}");
    // The merged F1/F2 peak: within 10 % (the spike's narrow-formant leakage moves it about 9 %,
    // see the report) and nearer where it was than where an unpreserved shift would put it.
    let low = pi.iter().copied().find(|p| (FORMANTS[0]..=FORMANTS[1]).contains(p)).unwrap_or_else(|| panic!("no merged F1/F2 peak in {pi:?}"));
    let out_low = po.iter().copied().min_by(|p, q| (p - low).abs().total_cmp(&(q - low).abs())).unwrap();
    assert!((out_low - low).abs() / low <= 0.10, "merged peak {low} Hz -> {out_low} Hz");
    assert!((out_low - low).abs() < (out_low - low * r).abs(), "merged peak {low} Hz followed the shift to {out_low} Hz");
    // Envelope shape over 300–4000 Hz, against a shift that does not keep formants.
    let dist = envelope_distance_db(&ei, &eo, f.sr);
    let control = envelope_distance_db(&ei, &scaled_envelope(&ei, r), f.sr);
    eprintln!("envelope distance {dist:.2} dB (unpreserved control {control:.2} dB)");
    assert!(dist <= 2.0, "envelope distance {dist:.2} dB");
    assert!(control >= 2.0 * dist, "control {control:.2} dB does not discriminate from {dist:.2} dB");
}

#[test]
fn unvoiced_noise_is_untouched() {
    let f = fx();
    let c = curves(&f.t, 0..f.t.frames.len(), 50.0, 0.0);
    let y = render_all(&f.x, &f.t, &c, &nc()).unwrap();
    let hop = f.t.hop_samples as usize;
    let voiced: Vec<usize> = f.t.frames.iter().enumerate().filter(|(_, fr)| fr.cents.is_some()).map(|(i, _)| i * hop).collect();
    // 10 ms smoothing + the 1-frame dilation + the 1-frame linear ramp from frames to samples.
    let margin = 480 + 2 * hop;
    let mut checked = 0;
    for (s, (yv, xv)) in y.iter().zip(&f.x).enumerate().take(f.v2_start).skip(f.v1_end) {
        if voiced.iter().all(|&v| s.abs_diff(v) > margin) {
            assert_eq!(yv.to_bits(), xv.to_bits(), "noise sample {s} changed");
            checked += 1;
        }
    }
    assert!(checked > f.sr as usize / 10, "only {checked} noise samples were far from voicing");
}

#[test]
fn gain_applies_only_to_the_note() {
    let f = fx();
    let (n, _, _) = stft_sizes(f.sr);
    let c = curves(&f.t, frames_in(&f.t, 0, f.v1_end), 0.0, 6.0);
    let y = render_all(&f.x, &f.t, &c, &nc()).unwrap();
    let (a, b) = (f.sr as usize / 10, f.v1_end - f.sr as usize / 10);
    let rms = |v: &[f32]| (v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    let ratio = rms(&y[a..b]) / rms(&f.x[a..b]);
    assert!((ratio - 2.0).abs() <= 0.05, "ratio {ratio}");
    let from = f.v1_end + n + 480;
    assert_eq!(&y[from..f.v2_start], &f.x[from..f.v2_start]);
}

#[test]
fn short_input_passes_through() {
    let x: Vec<f32> = vowel(48_000, 0.5).into_iter().take(1000).collect();
    let t = track_pitch(&x, 48_000, &no_cancel()).unwrap();
    let c = curves(&t, 0..t.frames.len(), 80.0, 3.0);
    let y = render_all(&x, &t, &c, &nc()).unwrap();
    assert_eq!(y.len(), x.len());
    assert!(y.iter().all(|v| v.is_finite()));
    let y = render_all(&[], &t, &c, &nc()).unwrap();
    assert!(y.is_empty());
}

#[test]
fn edit_on_first_and_last_note_stays_in_bounds() {
    let f = fx();
    let mut c = curves(&f.t, frames_in(&f.t, 0, f.v1_end), 40.0, 2.0);
    for i in frames_in(&f.t, f.v2_start, f.x.len() + 1) {
        c.shift_cents[i] = -40.0;
        c.gain_db[i] = -2.0;
    }
    let frames = changed_frames(None, &c);
    let spans = render_spans(&f.x, &f.t, &c, &frames, &nc()).unwrap();
    assert!(!spans.is_empty());
    assert_eq!(spans[0].start, 0);
    let last = spans.last().unwrap();
    assert_eq!(last.start as usize + last.samples.len(), f.x.len());
    for w in spans.windows(2) {
        assert!(w[0].start as usize + w[0].samples.len() <= w[1].start as usize, "spans overlap or are unsorted");
    }
    let y = render_all(&f.x, &f.t, &c, &nc()).unwrap();
    assert_eq!(y.len(), f.x.len());
    assert!(y.iter().all(|v| v.is_finite()));
}

#[test]
fn changed_frames_finds_only_the_moved_note() {
    let f = fx();
    let a = analyse(&f.x, f.sr, &no_cancel()).unwrap();
    assert_eq!(a.notes.len(), 2, "{:?}", a.notes);
    let (n1, n2) = (&a.notes[0], &a.notes[1]);
    let mut prev = zero_curves(&f.t);
    let mut next = zero_curves(&f.t);
    for i in n1.start as usize..n1.end as usize {
        prev.shift_cents[i] = 30.0;
        next.shift_cents[i] = 30.0;
    }
    for i in n2.start as usize..n2.end as usize {
        prev.shift_cents[i] = 20.0;
        // a moved note with a smoothed edge (4-frame spill)
        next.shift_cents[i] = -40.0;
    }
    for (k, i) in (n2.start.saturating_sub(4)..n2.start).enumerate() {
        next.shift_cents[i as usize] = -8.0 * (k as f32 + 1.0);
    }
    let r = changed_frames(Some(&prev), &next);
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(r[0].start >= n2.start.saturating_sub(4) && r[0].end <= n2.end + 4, "{r:?} vs note 2 {}..{}", n2.start, n2.end);
    // with no previous curves, both notes are runs of their own
    let all = changed_frames(None, &next);
    assert_eq!(all.len(), 2, "{all:?}");
    // runs closer than 2 STFT frames merge
    let mut gap = zero_curves(&f.t);
    gap.gain_db[10] = 1.0;
    gap.gain_db[12] = 1.0;
    assert_eq!(changed_frames(None, &gap), vec![10..13]);
}

#[test]
fn cancel_stops_rendering() {
    let f = fx();
    let c = curves(&f.t, frames_in(&f.t, 0, f.v1_end), 50.0, 0.0);
    let cancel = AtomicBool::new(true);
    assert_eq!(render_all(&f.x, &f.t, &c, &cancel), Err(VocalError::Cancelled));
}

#[test]
fn changed_frames_ignores_non_finite_noise() {
    let f = fx();
    let zero = zero_curves(&f.t);
    let mut nan = zero_curves(&f.t);
    nan.shift_cents[20] = f32::NAN;
    nan.gain_db[30] = f32::INFINITY;
    // render_spans treats non-finite values as 0, so nothing changed.
    assert_eq!(changed_frames(None, &nan), Vec::<Range<u32>>::new());
    assert_eq!(changed_frames(Some(&zero), &nan), Vec::<Range<u32>>::new());
    assert_eq!(changed_frames(Some(&nan), &zero), Vec::<Range<u32>>::new());
    let mut moved = nan.clone();
    moved.shift_cents[20] = 10.0;
    assert_eq!(changed_frames(Some(&nan), &moved), vec![20..21]);
}

#[test]
fn over_long_span_is_refused_before_rendering() {
    // 61 s at 8 kHz, every frame voiced and shifted: one span with no breath in it. Only the
    // curves are built: the error must come back before any rendering (or its allocation).
    let sr = 8_000u32;
    let hop = 40u32;
    let len = 61 * sr as usize;
    let nf = len / hop as usize + 1;
    let t = PitchTrack {
        sample_rate: sr,
        hop_samples: hop,
        frames: vec![soundcraft_vocal::PitchFrame { cents: Some(5700.0), level_db: -20.0, voiced_prob: 0.9 }; nf],
    };
    let c = EditCurves { shift_cents: vec![50.0; nf], gain_db: vec![0.0; nf] };
    let x = vec![0.0f32; len];
    let t0 = std::time::Instant::now();
    let r = render_spans(&x, &t, &c, &changed_frames(None, &c), &nc());
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    match r {
        Err(VocalError::Unsupported(why)) => assert!(why.contains("no breath for over 60 s"), "{why}"),
        other => panic!("expected Unsupported, got {:?}", other.map(|s| s.len())),
    }
    assert!(ms < 1000.0, "the refusal took {ms:.0} ms: it rendered first");
}

#[test]
fn cancel_mid_render_stops_promptly() {
    // A 20 s fully shifted vowel (synthetic track: every frame voiced), cancelled ~20 ms in.
    let sr = 48_000u32;
    let x = vowel(sr, 20.0);
    let hop = 240usize;
    let nf = x.len() / hop + 1;
    let t = PitchTrack {
        sample_rate: sr,
        hop_samples: hop as u32,
        frames: vec![soundcraft_vocal::PitchFrame { cents: Some(5000.0), level_db: -20.0, voiced_prob: 0.9 }; nf],
    };
    let c = EditCurves { shift_cents: vec![50.0; nf], gain_db: vec![0.0; nf] };
    let cancel = AtomicBool::new(false);
    let (r, done, flagged) = std::thread::scope(|s| {
        let setter = s.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            std::time::Instant::now()
        });
        let r = render_all(&x, &t, &c, &cancel);
        let done = std::time::Instant::now();
        (r, done, setter.join().unwrap())
    });
    assert_eq!(r, Err(VocalError::Cancelled), "the render finished before the flag was set");
    let ms = done.saturating_duration_since(flagged).as_secs_f64() * 1e3;
    println!("returned {ms:.1} ms after the cancel flag");
    assert!(ms < 100.0, "returned {ms:.1} ms after the cancel flag");
}

#[test]
fn stft_sizes_scale_with_rate() {
    assert_eq!(stft_sizes(48_000), (2048, 256, 48));
    assert_eq!(stft_sizes(44_100), (2048, 256, 44));
    assert_eq!(stft_sizes(96_000), (4096, 512, 96));
}

/// Quiet noise (0.3 s), then notes A (150 Hz, 0.4 s), B (200 Hz, `b_frames` analysis frames,
/// never edited) and C (150 Hz, 0.4 s) back to back, then quiet noise (0.3 s), at 48 kHz. Returns
/// the signal, its track and the frame ranges of A and C.
fn three_notes(b_frames: usize) -> (Vec<f32>, PitchTrack, Range<usize>, Range<usize>) {
    let sr = 48_000;
    let quiet = |seed| noise(sr, 0.3, seed).into_iter().map(|v| v * 0.01);
    let mut x: Vec<f32> = quiet(3).collect();
    let a_start = x.len();
    x.extend(vowel(sr, 0.4));
    let a_end = x.len();
    x.extend(vowel_with(sr, b_frames as f32 * 0.005, 200.0, BANDWIDTHS));
    let c_start = x.len();
    x.extend(vowel(sr, 0.4));
    let c_end = x.len();
    x.extend(quiet(5));
    let t = track_pitch(&x, sr, &no_cancel()).unwrap();
    let (fa, fc) = (frames_in(&t, a_start, a_end), frames_in(&t, c_start, c_end));
    (x, t, fa, fc)
}

#[test]
fn incremental_splice_is_seamless() {
    let ms10 = 480;
    // B of 65, 75 and 90 ms: the shifted runs A and C sit 13–18 frames apart. Then the same with
    // B a gain-only note (levelled, not shifted), changing C's shift or B's gain.
    let cases = [(13usize, None), (15, None), (18, None), (15, Some(false)), (15, Some(true)), (18, Some(true))];
    for (b_frames, gain_b) in cases {
        let (x, t, fa, fc) = three_notes(b_frames);
        let fb = fa.end..fc.start;
        let mut prev = zero_curves(&t);
        for i in fa.clone() {
            prev.shift_cents[i] = 50.0;
        }
        for i in fc.clone() {
            prev.shift_cents[i] = 30.0;
        }
        if gain_b.is_some() {
            for i in fb.clone() {
                prev.gain_db[i] = 3.0;
            }
        }
        let mut next = prev.clone();
        if gain_b == Some(true) {
            for i in fb.clone() {
                next.gain_db[i] = -2.0;
            }
        } else {
            for i in fc.clone() {
                next.shift_cents[i] = -40.0;
            }
        }
        let old = render_all(&x, &t, &prev, &nc()).unwrap();
        let fresh = render_all(&x, &t, &next, &nc()).unwrap();
        let spans = render_spans(&x, &t, &next, &changed_frames(Some(&prev), &next), &nc()).unwrap();
        assert!(!spans.is_empty());
        assert!(spans.iter().all(|sp| sp.start > 0 && sp.start as usize + sp.samples.len() < x.len()), "spans reach the input's ends");
        let mut inc = old.clone();
        splice(&mut inc, &spans);
        assert_eq!(inc.len(), x.len());
        let inside = |s: usize| spans.iter().any(|sp| (sp.start as usize..sp.start as usize + sp.samples.len()).contains(&s));
        for s in (0..x.len()).filter(|&s| !inside(s)) {
            if fresh[s].to_bits() == x[s].to_bits() {
                assert_eq!(inc[s].to_bits(), x[s].to_bits(), "B {b_frames} frames: sample {s} should be the input exactly");
            } else {
                assert!((inc[s] - fresh[s]).abs() <= 1e-5, "B {b_frames} frames: sample {s} differs from a fresh render: {} vs {}", inc[s], fresh[s]);
            }
        }
        // The new spans' spliced edges (first and last 10 ms, and the samples just outside) meet
        // the old render. An edge clamped to the input's start or end has nothing to splice into.
        for sp in &spans {
            let (a, b) = (sp.start as usize, sp.start as usize + sp.samples.len());
            let head = if a > 0 { a.saturating_sub(4)..(a + ms10).min(b) } else { 0..0 };
            let tail = if b < x.len() { b.saturating_sub(ms10).max(a)..(b + 4).min(x.len()) } else { 0..0 };
            let edges = head.chain(tail);
            for s in edges {
                assert!((inc[s] - old[s]).abs() <= 1e-5, "B {b_frames} frames: span {a}..{b} edge sample {s}: new {} vs old {}", inc[s], old[s]);
            }
        }
    }
}
