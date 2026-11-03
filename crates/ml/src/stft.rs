//! A short-time Fourier transform matching `torch.stft` / `torch.istft` with `center=True`,
//! reflect padding, a periodic Hann window (or a given one), `onesided=True`, unnormalized: what
//! the RoFormer/MDX-style models expect on the host side.
//!
//! Spectra are `[bin][frame]` of complex `f32`, `n_fft / 2 + 1` bins and `1 + len / hop` frames.

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// A periodic Hann window (`torch.hann_window(n)`).
pub fn hann(n: usize) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    (0..n).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32).collect()
}

/// `x[i]` with reflect padding beyond either end (`numpy.pad(mode='reflect')`, repeated as
/// needed for very short inputs).
fn reflect(x: &[f32], i: isize) -> f32 {
    let n = x.len() as isize;
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return x.first().copied().unwrap_or(0.0);
    }
    let period = 2 * (n - 1);
    let mut j = i.rem_euclid(period);
    if j >= n {
        j = period - j;
    }
    x.get(j as usize).copied().unwrap_or(0.0)
}

/// A planned STFT.
pub struct Stft {
    pub n_fft: usize,
    pub hop: usize,
    window: Vec<f32>,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
}

impl std::fmt::Debug for Stft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stft").field("n_fft", &self.n_fft).field("hop", &self.hop).finish()
    }
}

impl Stft {
    /// An STFT of `n_fft` points (at least 2) and `hop` (at least 1) with a periodic Hann window.
    pub fn new(n_fft: usize, hop: usize) -> Self {
        Self::with_window(n_fft, hop, hann(n_fft.max(2)))
    }

    /// With an explicit window (padded or cut to `n_fft`).
    pub fn with_window(n_fft: usize, hop: usize, mut window: Vec<f32>) -> Self {
        let n_fft = n_fft.max(2);
        window.resize(n_fft, 0.0);
        let mut planner = FftPlanner::<f32>::new();
        Stft { n_fft, hop: hop.max(1), window, fwd: planner.plan_fft_forward(n_fft), inv: planner.plan_fft_inverse(n_fft) }
    }

    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Frames for a signal of `len` samples.
    pub fn frames(&self, len: usize) -> usize {
        1 + len / self.hop
    }

    /// The spectrum of `x`: `[bin][frame]`.
    pub fn forward(&self, x: &[f32]) -> Vec<Vec<Complex32>> {
        let frames = self.frames(x.len());
        let bins = self.bins();
        let half = (self.n_fft / 2) as isize;
        let mut out = vec![vec![Complex32::new(0.0, 0.0); frames]; bins];
        let mut buf = vec![Complex32::new(0.0, 0.0); self.n_fft];
        for f in 0..frames {
            let start = (f * self.hop) as isize - half;
            for (k, (b, w)) in buf.iter_mut().zip(&self.window).enumerate() {
                *b = Complex32::new(reflect(x, start + k as isize) * w, 0.0);
            }
            self.fwd.process(&mut buf);
            for (row, v) in out.iter_mut().zip(&buf) {
                if let Some(cell) = row.get_mut(f) {
                    *cell = *v;
                }
            }
        }
        out
    }

    /// The signal of `spec` (`[bin][frame]`), `len` samples long (`torch.istft(..., length=len)`).
    pub fn inverse(&self, spec: &[Vec<Complex32>], len: usize) -> Vec<f32> {
        let frames = spec.iter().map(Vec::len).max().unwrap_or(0);
        let n = self.n_fft;
        let half = n / 2;
        let total = n + self.hop * frames.saturating_sub(1);
        let mut acc = vec![0.0f32; total];
        let mut wsum = vec![0.0f32; total];
        let mut buf = vec![Complex32::new(0.0, 0.0); n];
        let scale = 1.0 / n as f32;
        for f in 0..frames {
            // Hermitian-symmetric full spectrum from the one-sided bins.
            for (k, b) in buf.iter_mut().enumerate() {
                let (bin, conj) = if k <= half { (k, false) } else { (n - k, true) };
                let v = spec.get(bin).and_then(|r| r.get(f)).copied().unwrap_or_default();
                *b = if conj { v.conj() } else { v };
            }
            // DC and Nyquist bins are real in a real signal's spectrum (irfft ignores the imaginary part).
            if let Some(b) = buf.first_mut() {
                b.im = 0.0;
            }
            if n.is_multiple_of(2)
                && let Some(b) = buf.get_mut(half)
            {
                b.im = 0.0;
            }
            self.inv.process(&mut buf);
            let off = f * self.hop;
            for (k, (b, w)) in buf.iter().zip(&self.window).enumerate() {
                if let (Some(a), Some(s)) = (acc.get_mut(off + k), wsum.get_mut(off + k)) {
                    *a += b.re * scale * w;
                    *s += w * w;
                }
            }
        }
        (0..len)
            .map(|i| {
                let j = i + half;
                match (acc.get(j), wsum.get(j)) {
                    // As the reference pipelines: where the window envelope vanishes, no division.
                    (Some(a), Some(s)) if *s > 1e-11 => a / s,
                    (Some(a), _) => *a,
                    _ => 0.0,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_is_transparent() {
        let x: Vec<f32> = (0..5000).map(|i| ((i as f32) * 0.013).sin() * 0.5 + ((i as f32) * 0.31).cos() * 0.1).collect();
        for (n, hop) in [(2048, 441), (1024, 256), (64, 16), (6, 2)] {
            let s = Stft::new(n, hop);
            let spec = s.forward(&x);
            assert_eq!(spec.len(), n / 2 + 1);
            assert_eq!(spec[0].len(), 1 + x.len() / hop);
            let y = s.inverse(&spec, x.len());
            let err = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(err < 1e-4, "n_fft {n} hop {hop}: {err}");
        }
    }

    #[test]
    fn matches_a_hand_computed_dft() {
        // torch.stft(x, 4, 2, window=hann(4), center=True) on x = [1, 2, 3, 4, 5]:
        // frame 0 is reflect([3, 2, 1, 2]) * [0, .5, 1, .5] = [0, 1, 1, 1].
        let s = Stft::new(4, 2);
        let spec = s.forward(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(spec.len(), 3);
        assert_eq!(spec[0].len(), 3);
        assert!((spec[0][0] - Complex32::new(3.0, 0.0)).norm() < 1e-6);
        assert!((spec[1][0] - Complex32::new(-1.0, 0.0)).norm() < 1e-6);
        assert!((spec[2][0] - Complex32::new(-1.0, 0.0)).norm() < 1e-6);
    }

    #[test]
    fn short_and_empty_inputs_do_not_panic() {
        let s = Stft::new(2048, 512);
        for len in [0usize, 1, 2, 3, 100] {
            let x = vec![0.25f32; len];
            let spec = s.forward(&x);
            let y = s.inverse(&spec, len);
            assert_eq!(y.len(), len);
            assert!(y.iter().all(|v| v.is_finite()));
        }
    }
}
