//! FFT magnitude analysis for analyzers and EQ displays.

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

/// Largest FFT size accepted (larger requests are clamped).
pub const MAX_FFT: usize = 1 << 18;

/// Hann-windowed magnitude spectrum in dBFS, `fft_size / 2 + 1` bins (bin `k` is at
/// `k · sr / fft_size` Hz). A full-scale sine reads ≈ 0 dB. Input shorter than `fft_size` is
/// zero-padded; longer input uses the first `fft_size` samples. Floored at -144 dB.
pub fn magnitude_db(samples: &[f32], fft_size: usize) -> Vec<f32> {
    let mut out = Vec::new();
    Analyzer::default().magnitude_db_into(samples, fft_size, &mut out);
    out
}

/// [`magnitude_db`] for repeated use: the FFT plan, the Hann window and the work buffers are
/// built for a size once and kept, so a steady stream of same-size spectra (an analyzer redrawn
/// every frame) allocates nothing after the first.
#[derive(Clone, Default)]
pub struct Analyzer {
    n: usize,
    fft: Option<Arc<dyn Fft<f32>>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl std::fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyzer").field("n", &self.n).finish_non_exhaustive()
    }
}

impl Analyzer {
    /// As [`magnitude_db`], written into `out` (resized to `fft_size / 2 + 1`; empty below 2).
    pub fn magnitude_db_into(&mut self, samples: &[f32], fft_size: usize, out: &mut Vec<f32>) {
        let n = fft_size.min(MAX_FFT);
        out.clear();
        if n < 2 {
            return;
        }
        if self.n != n || self.fft.is_none() {
            let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
            self.scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
            self.window = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
            self.buf = vec![Complex::new(0.0, 0.0); n];
            self.fft = Some(fft);
            self.n = n;
        }
        let Some(fft) = self.fft.as_ref() else { return };
        for (i, (b, w)) in self.buf.iter_mut().zip(&self.window).enumerate() {
            let x = samples.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            *b = Complex::new(x * w, 0.0);
        }
        fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        // Hann coherent gain is 0.5; a sine's energy splits across ±f.
        let scale = 2.0 / (n as f32 * 0.5);
        out.extend(self.buf.iter().take(n / 2 + 1).map(|c| crate::gain_to_db(c.norm() * scale)));
    }
}

/// Frequency in Hz of bin `k` for a given FFT size.
pub fn bin_hz(k: usize, fft_size: usize, sample_rate: f32) -> f32 {
    if fft_size == 0 { 0.0 } else { k as f32 * sample_rate / fft_size as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peak_near_zero_db() {
        let sr = 48_000.0;
        let n = 4096;
        let k = 100;
        let f = bin_hz(k, n, sr);
        let x: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * f * i as f32 / sr).sin()).collect();
        let m = magnitude_db(&x, n);
        assert_eq!(m.len(), n / 2 + 1);
        assert!(m[k].abs() < 0.1, "{}", m[k]);
        assert!(magnitude_db(&x, 0).is_empty());
    }

    #[test]
    fn analyzer_keeps_its_plan_and_matches_magnitude_db() {
        let n = 4096;
        let x: Vec<f32> = (0..n).map(|i| (i as f32 * 0.03).sin() * 0.5).collect();
        let y: Vec<f32> = (0..n).map(|i| (i as f32 * 0.11).cos() * 0.2).collect();
        let mut a = Analyzer::default();
        let mut out = Vec::new();
        a.magnitude_db_into(&x, n, &mut out);
        assert_eq!(out, magnitude_db(&x, n));
        let plan = a.buf.as_ptr();
        let cap = out.capacity();
        a.magnitude_db_into(&y, n, &mut out);
        assert_eq!(out, magnitude_db(&y, n));
        assert_eq!((a.buf.as_ptr(), out.capacity()), (plan, cap), "same buffers, nothing re-made");
        // A new size re-plans; a hostile size is empty.
        a.magnitude_db_into(&x, 1024, &mut out);
        assert_eq!(out, magnitude_db(&x, 1024));
        a.magnitude_db_into(&x, 1, &mut out);
        assert!(out.is_empty());
    }
}
