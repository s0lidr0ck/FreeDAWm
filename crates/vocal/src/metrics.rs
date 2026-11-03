//! Objective quality measures (ports of the spike's `stress.py::flux` and `evaluate.py`): steady
//! frames, harmonic flux ("wobble") and octave-folded pitch error.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::track::PitchTrack;

/// Half-width of the steadiness window (frames): 11 frames in all.
const STEADY_HALF: usize = 5;
/// Largest pitch range inside a steady window (cents).
const STEADY_RANGE_CENTS: f32 = 40.0;
/// Flux STFT size and hop at 48 kHz.
const FLUX_N: usize = 1024;
const FLUX_HOP_48K: f64 = 240.0;
/// Flux band (Hz, exclusive at both ends).
const FLUX_LO_HZ: f64 = 300.0;
const FLUX_HI_HZ: f64 = 4000.0;
/// Bins more than this far below the frame's band maximum are ignored (dB).
const FLUX_RANGE_DB: f64 = 30.0;
/// scipy's interpolated steadiness threshold.
const STEADY_INTERP_MIN: f64 = 0.99;
/// Magnitude floor before the dB conversion (`stress.py`).
const MAG_FLOOR: f64 = 1e-7;

/// Frames at the centre of an 11-frame window that is all voiced with a pitch range under 40 c
/// (`stress.py`'s `steady`). The first and last 5 frames are never steady.
pub fn steady_frames(track: &PitchTrack) -> Vec<bool> {
    let n = track.frames.len();
    (0..n)
        .map(|i| {
            if i < STEADY_HALF || i + STEADY_HALF >= n {
                return false;
            }
            let Some(w) = track.frames.get(i - STEADY_HALF..=i + STEADY_HALF) else { return false };
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for f in w {
                match f.cents.filter(|c| c.is_finite()) {
                    Some(c) => {
                        lo = lo.min(c);
                        hi = hi.max(c);
                    }
                    None => return false,
                }
            }
            hi - lo < STEADY_RANGE_CENTS
        })
        .collect()
}

/// Harmonic flux (dB), the spike's `stress.py::flux`: a 1024-point periodic-Hann STFT with hop
/// `240·sr/48000` (scipy `stft` framing: frame `j` centred on sample `j·hop`). For each pair of
/// neighbouring STFT frames that both map to steady analysis frames (`steady` interpolated to the
/// frame time > 0.99), the mean |ΔdB| over the 300–4000 Hz bins within 30 dB of the first frame's
/// band maximum; then the mean over those pairs. 0.0 when no pair qualifies.
pub fn harmonic_flux_db(samples: &[f32], sr: u32, steady: &[bool], hop_samples: u32) -> f32 {
    if samples.is_empty() || sr == 0 || hop_samples == 0 || steady.is_empty() {
        return 0.0;
    }
    let hop = (FLUX_HOP_48K * f64::from(sr) / 48_000.0).round().max(1.0) as usize;
    let n = FLUX_N;
    let half = n / 2;
    let bins: Vec<usize> = (0..=half)
        .filter(|&k| {
            let f = k as f64 * f64::from(sr) / n as f64;
            f > FLUX_LO_HZ && f < FLUX_HI_HZ
        })
        .collect();
    if bins.is_empty() {
        return 0.0;
    }
    let win: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).collect();
    let wsum: f64 = win.iter().sum();
    let fft: Arc<dyn Fft<f64>> = FftPlanner::new().plan_fft_forward(n);
    let mut buf = vec![Complex::default(); n];
    let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    let x = |i: i64| usize::try_from(i).ok().and_then(|j| samples.get(j)).map_or(0.0, |&v| if v.is_finite() { f64::from(v) } else { 0.0 });

    // The steadiness of STFT frame j (np.interp of the 0/1 steady track at the frame's time).
    let steady_f: Vec<f64> = steady.iter().map(|&s| if s { 1.0 } else { 0.0 }).collect();
    let is_steady = |j: usize| {
        let pos = (j * hop) as f64 / f64::from(hop_samples);
        let i = pos.floor() as usize;
        let v = match (steady_f.get(i), steady_f.get(i + 1)) {
            (Some(&a), Some(&b)) => a + (b - a) * (pos - i as f64),
            (Some(&a), None) => a,
            _ => steady_f.last().copied().unwrap_or(0.0),
        };
        v > STEADY_INTERP_MIN
    };
    let mut spectrum = |j: usize| -> Vec<f64> {
        let start = (j * hop) as i64 - half as i64;
        for (i, (b, w)) in buf.iter_mut().zip(&win).enumerate() {
            *b = Complex::new(x(start + i as i64) * w, 0.0);
        }
        fft.process_with_scratch(&mut buf, &mut scratch);
        bins.iter().map(|&k| 20.0 * (buf.get(k).map_or(0.0, |z| z.norm()) / wsum + MAG_FLOOR).log10()).collect()
    };

    let frames = samples.len().div_ceil(hop) + 1;
    let mut prev: Option<(usize, Vec<f64>)> = None;
    let (mut sum, mut count) = (0.0f64, 0usize);
    for j in 0..frames {
        if !is_steady(j) {
            prev = None;
            continue;
        }
        let a = spectrum(j);
        if let Some((pj, pa)) = &prev
            && *pj + 1 == j
        {
            let max = pa.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let (mut s, mut c) = (0.0f64, 0usize);
            for (p, q) in pa.iter().zip(&a) {
                if *p > max - FLUX_RANGE_DB {
                    s += (q - p).abs();
                    c += 1;
                }
            }
            if c > 0 {
                sum += s / c as f64;
                count += 1;
            }
        }
        prev = Some((j, a));
    }
    if count == 0 { 0.0 } else { (sum / count as f64) as f32 }
}

/// Octave-folded pitch error `|(a − b + 600) mod 1200 − 600|` (cents) over the frames where `mask`
/// is set and both tracks are voiced: `(median, p90)` (numpy's linear-interpolated percentiles),
/// or `(NaN, NaN)` when no frame qualifies.
pub fn pitch_error_cents(a: &PitchTrack, b: &PitchTrack, mask: &[bool]) -> (f32, f32) {
    let mut err: Vec<f64> = a
        .frames
        .iter()
        .zip(&b.frames)
        .zip(mask)
        .filter(|(_, m)| **m)
        .filter_map(|((fa, fb), _)| {
            let (ca, cb) = (fa.cents.filter(|c| c.is_finite())?, fb.cents.filter(|c| c.is_finite())?);
            Some((f64::from(ca) - f64::from(cb) + 600.0).rem_euclid(1200.0) - 600.0)
        })
        .map(f64::abs)
        .collect();
    err.sort_by(f64::total_cmp);
    (percentile(&err, 0.5) as f32, percentile(&err, 0.9) as f32)
}

/// numpy `percentile(v, 100·q)` (linear) of sorted `v`; NaN when empty.
fn percentile(v: &[f64], q: f64) -> f64 {
    let Some(&last) = v.last() else { return f64::NAN };
    let pos = q.clamp(0.0, 1.0) * (v.len() - 1) as f64;
    let i = pos.floor() as usize;
    match (v.get(i), v.get(i + 1)) {
        (Some(&lo), Some(&hi)) => lo + (hi - lo) * (pos - i as f64),
        (Some(&lo), None) => lo,
        _ => last,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::PitchFrame;

    fn track(cents: &[Option<f32>]) -> PitchTrack {
        PitchTrack {
            sample_rate: 48_000,
            hop_samples: 240,
            frames: cents.iter().map(|&c| PitchFrame { cents: c, level_db: -20.0, voiced_prob: 1.0 }).collect(),
        }
    }

    #[test]
    fn steady_needs_eleven_voiced_frames_within_40c() {
        let mut c: Vec<Option<f32>> = vec![Some(6900.0); 20];
        let t = track(&c);
        let s = steady_frames(&t);
        assert_eq!(s.iter().filter(|&&v| v).count(), 10);
        assert!(!s[4] && s[5] && s[14] && !s[15]);
        c[10] = None;
        assert!(!steady_frames(&track(&c))[10]);
        c[10] = Some(6940.0);
        assert!(!steady_frames(&track(&c))[10]);
        c[10] = Some(6939.0);
        assert!(steady_frames(&track(&c))[10]);
    }

    #[test]
    fn pitch_error_folds_octaves() {
        let a = track(&[Some(6900.0), Some(8100.0), Some(6910.0), None]);
        let b = track(&[Some(6900.0), Some(6900.0), Some(6900.0), Some(6900.0)]);
        let (med, p90) = pitch_error_cents(&a, &b, &[true; 4]);
        assert!((med - 0.0).abs() < 1e-6, "{med}");
        assert!((p90 - 8.0).abs() < 1e-4, "{p90}");
        assert!(pitch_error_cents(&a, &b, &[false; 4]).0.is_nan());
    }

    #[test]
    fn flux_of_a_steady_tone_is_small_and_of_noise_large() {
        let sr = 48_000u32;
        let n = sr as usize;
        let tone: Vec<f32> = (0..n)
            .map(|i| (1..8).map(|k| (std::f32::consts::TAU * 220.0 * k as f32 * i as f32 / sr as f32).sin() / k as f32).sum::<f32>() * 0.2)
            .collect();
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let noise: Vec<f32> = (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                ((s >> 40) as f32 / (1u64 << 24) as f32) - 0.5
            })
            .collect();
        // Steady away from the ends (the STFT frames there see the zero padding).
        let steady: Vec<bool> = (0..=n / 240).map(|i| (10..n / 240 - 10).contains(&i)).collect();
        let ft = harmonic_flux_db(&tone, sr, &steady, 240);
        let fnz = harmonic_flux_db(&noise, sr, &steady, 240);
        assert!(ft < 0.2, "tone flux {ft}");
        assert!(fnz > 3.0, "noise flux {fnz}");
        assert_eq!(harmonic_flux_db(&tone, sr, &vec![false; n / 240 + 1], 240), 0.0);
    }
}
