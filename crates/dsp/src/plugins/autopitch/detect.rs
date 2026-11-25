//! Plumb's pitch detector: causal YIN (the cumulative-mean-normalised difference computed by
//! FFT) over the last `2 x` the longest period of the voice range, every detection hop, with a
//! preference for continuity against octave jumps. It sees only input that has arrived; its
//! estimate belongs to the middle of its window (half a window ago).
//!
//! Every buffer and FFT plan is made in [`Detector::new`] (off the audio thread);
//! [`Detector::analyse`] never allocates.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// The lowest pitch it listens for by default (Hz): below any sung bass note. Not a user setting
/// in Studio mode (the 43 ms engine has room for the window it needs).
pub(super) const FLOOR_HZ: f64 = 70.0;
/// The highest pitch it listens for (Hz).
pub(super) const CEIL_HZ: f64 = 1400.0;
/// Below this level (dBFS, the window's RMS) the input counts as silence.
const SILENCE_DB: f64 = -55.0;
/// A dip of the difference function above this is no pitch (breath, noise).
const UNVOICED_DIP: f64 = 0.3;
/// YIN's absolute threshold.
const YIN_THRESHOLD: f64 = 0.15;

/// MIDI cents (6900 = A4) of a frequency.
pub(super) fn hz_to_cents(hz: f64) -> f64 {
    6900.0 + 1200.0 * (hz / 440.0).log2()
}

/// The frequency of MIDI cents.
pub(super) fn cents_to_hz(c: f64) -> f64 {
    440.0 * 2f64.powf((c - 6900.0) / 1200.0)
}

pub(super) struct Detector {
    sr: f64,
    /// Longest lag (samples) and the window compared against itself.
    l: usize,
    w: usize,
    min_lag: usize,
    m: usize,
    fwd: Arc<dyn Fft<f64>>,
    inv: Arc<dyn Fft<f64>>,
    a: Vec<Complex<f64>>,
    b: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    d: Vec<f64>,
    pre: Vec<f64>,
    /// The frame to analyse (oldest sample first), filled by the caller.
    frame: Vec<f64>,
    prev: Option<f64>,
}

impl Detector {
    /// A detector for `sr` Hz listening down to `floor` Hz (allocates: off the audio thread).
    pub(super) fn new(sr: f64, floor: f64) -> Detector {
        let floor = if floor.is_finite() { floor.clamp(40.0, 400.0) } else { FLOOR_HZ };
        let l = (sr / floor).ceil() as usize + 2;
        let w = l;
        let min_lag = ((sr / CEIL_HZ).floor() as usize).max(2);
        let m = (2 * w + l).next_power_of_two();
        let mut p = FftPlanner::new();
        let fwd = p.plan_fft_forward(m);
        let inv = p.plan_fft_inverse(m);
        let sl = fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len());
        Detector {
            sr,
            l,
            w,
            min_lag,
            m,
            fwd,
            inv,
            a: vec![Complex::default(); m],
            b: vec![Complex::default(); m],
            scratch: vec![Complex::default(); sl],
            d: vec![0.0; l + 2],
            pre: vec![0.0; w + l + 1],
            frame: vec![0.0; w + l],
            prev: None,
        }
    }

    /// Samples the detector looks at.
    pub(super) fn frame_len(&self) -> usize {
        self.w + self.l
    }

    pub(super) fn frame_mut(&mut self) -> &mut [f64] {
        &mut self.frame
    }

    pub(super) fn reset(&mut self) {
        self.prev = None;
    }

    fn cents_of(&self, tau: usize) -> f64 {
        hz_to_cents(self.sr / tau as f64)
    }

    /// The pitch (MIDI cents) of the frame, `None` when unvoiced, and its confidence (0..1).
    pub(super) fn analyse(&mut self) -> (Option<f64>, f64) {
        let (w, l, m) = (self.w, self.l, self.m);
        let len = self.frame.len().max(1);
        let rms = (self.frame.iter().map(|v| v * v).sum::<f64>() / len as f64).sqrt();
        if !rms.is_finite() || rms < 10f64.powf(SILENCE_DB / 20.0) {
            self.prev = None;
            return (None, 0.0);
        }
        for i in 0..m {
            let x = self.frame.get(i).copied().unwrap_or(0.0);
            self.a[i] = Complex::new(if i < w { x } else { 0.0 }, 0.0);
            self.b[i] = Complex::new(if i < w + l { x } else { 0.0 }, 0.0);
        }
        self.fwd.process_with_scratch(&mut self.a, &mut self.scratch);
        self.fwd.process_with_scratch(&mut self.b, &mut self.scratch);
        for (a, b) in self.a.iter_mut().zip(&self.b) {
            *a = a.conj() * b;
        }
        self.inv.process_with_scratch(&mut self.a, &mut self.scratch);
        self.pre[0] = 0.0;
        for i in 0..w + l {
            let x = self.frame.get(i).copied().unwrap_or(0.0);
            self.pre[i + 1] = self.pre[i] + x * x;
        }
        let e0 = self.pre[w];
        let mut cum = 0.0;
        self.d[0] = 1.0;
        for tau in 1..=l {
            let r = self.a[tau].re / m as f64;
            let et = self.pre[tau + w] - self.pre[tau];
            let dv = (e0 + et - 2.0 * r).max(0.0);
            cum += dv;
            self.d[tau] = if cum > 0.0 { dv * tau as f64 / cum } else { 1.0 };
        }
        let lo = self.min_lag.max(2);
        let is_min = |d: &[f64], tau: usize| d[tau] < d[tau - 1] && d[tau] <= d[tau + 1];
        // The deepest local minimum in range.
        let mut best: Option<(usize, f64)> = None;
        for tau in lo..l {
            if is_min(&self.d, tau) && best.is_none_or(|b| self.d[tau] < b.1) {
                best = Some((tau, self.d[tau]));
            }
        }
        let Some(deepest) = best else {
            self.prev = None;
            return (None, 0.0);
        };
        // YIN: the first minimum under the threshold (else the deepest).
        let under = YIN_THRESHOLD.max(deepest.1 + 0.1);
        let first = (lo..l).find(|&tau| is_min(&self.d, tau) && self.d[tau] < under).map_or(deepest, |tau| (tau, self.d[tau]));
        let mut pick = first;
        // Continuity: when the pick jumps about an octave or more from the last pitch, a good
        // minimum near the last pitch wins.
        if let Some(pc) = self.prev
            && (self.cents_of(first.0) - pc).abs() > 700.0
        {
            let good = (deepest.1 + 0.12).max(YIN_THRESHOLD);
            let mut alt: Option<(usize, f64, f64)> = None;
            for tau in lo..l {
                if is_min(&self.d, tau) && self.d[tau] < good {
                    let dist = (self.cents_of(tau) - pc).abs();
                    if alt.is_none_or(|a| dist < a.2) {
                        alt = Some((tau, self.d[tau], dist));
                    }
                }
            }
            if let Some((tau, v, dist)) = alt
                && dist < 300.0
            {
                pick = (tau, v);
            }
        }
        let tau = pick.0;
        let (y0, y1, y2) = (self.d[tau - 1], self.d[tau], self.d[tau + 1]);
        let den = y0 - 2.0 * y1 + y2;
        let off = if den.abs() > 1e-12 { (0.5 * (y0 - y2) / den).clamp(-1.0, 1.0) } else { 0.0 };
        let conf = (1.0 - y1).clamp(0.0, 1.0);
        if y1 > UNVOICED_DIP {
            self.prev = None;
            return (None, conf);
        }
        let c = hz_to_cents(self.sr / (tau as f64 + off));
        if !c.is_finite() {
            self.prev = None;
            return (None, 0.0);
        }
        self.prev = Some(c);
        (Some(c), conf)
    }
}
