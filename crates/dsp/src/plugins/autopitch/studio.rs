//! Studio mode: the Vocal Editor's shifter (`soundcraft_vocal::resynth`: a phase-locked phase
//! vocoder that keeps the spectral envelope, with one phase offset of the fundamental shared by
//! every harmonic and every channel) run block by block. A frame of `n` samples (2048 at 48 kHz:
//! about 43 ms at any rate) every `n / 8`; the output is the input delayed by `n`, moved by the
//! correction the corrector logged for each frame's centre. Each period keeps the waveform shape
//! it was sung with; a stereo pair keeps its image (one correction, one phase offset).
//!
//! Where the voice is let through (unvoiced, unsure) the output crossfades (5 ms) to the delayed
//! input exactly. A silent frame costs no FFT; a frame that moves nothing costs one.
//!
//! Every buffer and FFT plan is made in [`Studio::new`]; nothing allocates afterwards.

use super::detect::cents_to_hz;
use super::front::Front;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f64::consts::{PI, TAU};
use std::sync::Arc;

/// A ratio this close to 1 is no shift.
const IDENTITY_EPS: f64 = 1e-4;
/// How fast the fundamental's phase offset eases back to 0 where nothing is shifted
/// (`resynth::STEER_RATE`).
const STEER_RATE: f64 = 0.001_734;
/// The crossfade between the moved and the delayed input voice (ms).
const VOICED_FADE_MS: f64 = 5.0;

/// The STFT size, hop and cepstral lifter at `sr` (`resynth::stft_sizes`: 2048 at 48 kHz).
pub(super) fn sizes(sr: f64) -> (usize, usize, usize) {
    let target = 2048.0 * sr / 48_000.0;
    let e = target.max(1.0).log2().round().clamp(8.0, 16.0) as u32;
    let n = 1usize << e;
    let lifter = ((48.0 * sr / 48_000.0).round() as usize).clamp(2, n / 2);
    (n, n / 8, lifter)
}

fn wrap_pi(a: f64) -> f64 {
    (a + PI).rem_euclid(TAU) - PI
}

pub(super) struct Studio {
    n: usize,
    h: usize,
    fwd: Arc<dyn Fft<f64>>,
    inv: Arc<dyn Fft<f64>>,
    win: Vec<f64>,
    lif: Vec<f64>,
    buf: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    mag: Vec<f64>,
    ph: Vec<f64>,
    w_inst: Vec<f64>,
    env: Vec<f64>,
    env_w: Vec<f64>,
    mag_n: Vec<f64>,
    ks: Vec<usize>,
    new_ph: Vec<f64>,
    peaks: Vec<usize>,
    pk_ph: Vec<f64>,
    prev_ph: Vec<Vec<f64>>,
    /// Overlap-added output per channel and the window's sum, by input sample (rings).
    acc: Vec<Vec<f64>>,
    wsum: Vec<f64>,
    amask: usize,
    /// The next frame's first input sample.
    next_start: u64,
    /// Output samples emitted since the last reset.
    emitted: u64,
    delta: f64,
    w0: f64,
    /// The formant shift's voiced weight per frame (eased over a few frames).
    fvoice: f64,
    mask_state: f64,
    mask_coef: f64,
}

impl Studio {
    /// Allocates (off the audio thread).
    pub(super) fn new(sr: f64, channels: usize) -> Studio {
        let (n, h, lifter) = sizes(sr);
        let mut p = FftPlanner::new();
        let fwd = p.plan_fft_forward(n);
        let inv = p.plan_fft_inverse(n);
        let sl = fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len());
        let k = n / 2 + 1;
        let alen = (2 * n).next_power_of_two();
        Studio {
            n,
            h,
            fwd,
            inv,
            win: (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()).collect(),
            lif: (0..n).map(|q| if q < lifter || q + lifter > n { 1.0 } else { 0.0 }).collect(),
            buf: vec![Complex::default(); n],
            scratch: vec![Complex::default(); sl],
            mag: vec![0.0; k],
            ph: vec![0.0; k],
            w_inst: vec![0.0; k],
            env: vec![0.0; k],
            env_w: vec![0.0; k],
            mag_n: vec![0.0; k],
            ks: vec![0; k],
            new_ph: vec![0.0; k],
            peaks: Vec::with_capacity(k),
            pk_ph: Vec::with_capacity(k),
            prev_ph: vec![vec![0.0; k]; channels],
            acc: vec![vec![0.0; alen]; channels],
            wsum: vec![0.0; alen],
            amask: alen - 1,
            next_start: 0,
            emitted: 0,
            delta: 0.0,
            w0: 0.0,
            fvoice: 0.0,
            mask_state: 0.0,
            mask_coef: 1.0 - (-1.0 / (VOICED_FADE_MS * 1e-3 * sr)).exp(),
        }
    }

    /// The delay (samples): one frame.
    pub(super) fn latency(&self) -> usize {
        self.n
    }

    /// Input history the engine reads back (samples).
    pub(super) fn history(&self) -> usize {
        2 * self.n
    }

    /// The frame hop (detections run every half of it).
    pub(super) fn frame_hop(&self) -> usize {
        self.h
    }

    pub(super) fn reset(&mut self) {
        self.prev_ph.iter_mut().for_each(|c| c.iter_mut().for_each(|v| *v = 0.0));
        self.acc.iter_mut().for_each(|c| c.iter_mut().for_each(|v| *v = 0.0));
        self.wsum.iter_mut().for_each(|v| *v = 0.0);
        self.next_start = 0;
        self.emitted = 0;
        self.delta = 0.0;
        self.w0 = 0.0;
        self.fvoice = 0.0;
        self.mask_state = 0.0;
    }

    /// Runs every frame the input now allows. `formant_st`: the formant shift on voiced frames.
    pub(super) fn run_frames(&mut self, f: &Front, formant_st: f64) {
        while self.next_start + self.n as u64 <= f.n_in {
            let start = self.next_start;
            self.frame(f, start, formant_st);
            self.next_start += self.h as u64;
        }
    }

    fn frame(&mut self, f: &Front, start: u64, formant_st: f64) {
        let (n, h) = (self.n, self.h);
        let k_len = n / 2 + 1;
        let half = n / 2;
        let last = k_len - 1;
        let (v, cents, corr) = f.at((start + half as u64) as f64);
        let r = 2f64.powf(corr.clamp(-2400.0, 2400.0) / 1200.0);
        let identity = (r - 1.0).abs() < IDENTITY_EPS;
        // The phase offset of the fundamental (resynth's `advance_delta`).
        if !v {
            self.delta = 0.0;
            self.w0 = 0.0;
        } else {
            self.w0 = TAU * cents_to_hz(cents) / f.sr;
            let mut dl = wrap_pi(self.delta + self.w0 * (r - 1.0) * h as f64);
            if identity {
                let step = STEER_RATE * self.w0 * h as f64;
                dl = if dl.abs() <= step { 0.0 } else { dl - step * dl.signum() };
            }
            self.delta = if dl.is_finite() { dl } else { 0.0 };
        }
        // Formant shift on voiced frames only, eased in and out over about three frames.
        self.fvoice += ((if v { 1.0 } else { 0.0 }) - self.fvoice) * 0.4;
        if self.fvoice < 1e-4 {
            self.fvoice = 0.0;
        }
        let fr = 2f64.powf(formant_st.clamp(-12.0, 12.0) * self.fvoice / 12.0);
        let f_identity = (fr - 1.0).abs() < IDENTITY_EPS;
        let (delta, w0) = (self.delta, self.w0);
        let harm = |w: f64| if w0 > 0.0 { (w / w0).round().max(0.0) } else { 0.0 };
        let inv_n = 1.0 / n as f64;
        let am = self.amask;
        for i in 0..n {
            if let Some(w) = self.wsum.get_mut((start as usize + i) & am) {
                *w += self.win[i] * self.win[i];
            }
        }
        for c in 0..self.acc.len() {
            // A silent frame: nothing to add, and the phases it would measure are all zero.
            if (0..n).all(|i| f.x(c, start + i as u64) == 0.0) {
                if let Some(p) = self.prev_ph.get_mut(c) {
                    p.iter_mut().for_each(|v| *v = 0.0);
                }
                continue;
            }
            for i in 0..n {
                let j = (i + half) % n;
                self.buf[i] = Complex::new(f64::from(f.x(c, start + j as u64)) * self.win[j], 0.0);
            }
            self.fwd.process_with_scratch(&mut self.buf, &mut self.scratch);
            let Some(prev) = self.prev_ph.get_mut(c) else { continue };
            for (k, pk) in prev.iter_mut().enumerate().take(k_len) {
                let z = self.buf[k];
                self.mag[k] = z.norm() + 1e-12;
                let phv = z.im.atan2(z.re);
                let omega = TAU * k as f64 / n as f64;
                let dphi = (phv - *pk - omega * h as f64 + PI).rem_euclid(TAU) - PI;
                self.w_inst[k] = omega + dphi / h as f64;
                *pk = phv;
                self.ph[k] = phv;
            }
            let Some(acc) = self.acc.get_mut(c) else { continue };
            if identity && delta == 0.0 && f_identity {
                // Nothing moves: the frame resynthesises to its own windowed input.
                for i in 0..n {
                    let x = f64::from(f.x(c, start + i as u64));
                    if let Some(a) = acc.get_mut((start as usize + i) & am) {
                        *a += x * self.win[i] * self.win[i];
                    }
                }
                continue;
            }
            // Cepstral envelope: env = exp(Re rfft(irfft(ln mag) * lifter)).
            for k in 0..k_len {
                self.buf[k] = Complex::new(self.mag[k].ln(), 0.0);
            }
            for k in 1..n / 2 {
                self.buf[n - k] = self.buf[k];
            }
            self.inv.process_with_scratch(&mut self.buf, &mut self.scratch);
            for q in 0..n {
                self.buf[q] = Complex::new(self.buf[q].re * inv_n * self.lif[q], 0.0);
            }
            self.fwd.process_with_scratch(&mut self.buf, &mut self.scratch);
            for k in 0..k_len {
                self.env[k] = self.buf[k].re.exp();
            }
            if f_identity {
                self.env_w.copy_from_slice(&self.env);
            } else {
                for k in 0..k_len {
                    let src = k as f64 / fr;
                    let i0 = src.floor() as usize;
                    self.env_w[k] = if i0 >= last { self.env[last] } else { self.env[i0] + (self.env[i0 + 1] - self.env[i0]) * (src - i0 as f64) };
                }
            }
            // Shift the excitation, keep (or move) the envelope.
            for k in 0..k_len {
                let src = k as f64 / r;
                let exc_at = |j: usize| self.mag[j] / self.env[j];
                let exc_n = if src > last as f64 {
                    0.0
                } else {
                    let i0 = src.floor() as usize;
                    if i0 >= last { exc_at(last) } else { exc_at(i0) + (exc_at(i0 + 1) - exc_at(i0)) * (src - i0 as f64) }
                };
                self.mag_n[k] = exc_n * self.env_w[k];
                self.ks[k] = (src.round_ties_even().max(0.0) as usize).min(last);
            }
            if identity && delta == 0.0 {
                self.new_ph.copy_from_slice(&self.ph);
            } else if identity {
                for k in 0..k_len {
                    self.new_ph[k] = self.ph[k] + harm(TAU * k as f64 / n as f64) * delta;
                }
            } else {
                self.peaks.clear();
                for k in 1..last {
                    if self.mag_n[k] > self.mag_n[k - 1] && self.mag_n[k] >= self.mag_n[k + 1] && self.peaks.len() < self.peaks.capacity() {
                        self.peaks.push(k);
                    }
                }
                if self.peaks.is_empty() {
                    for k in 0..k_len {
                        let j = self.ks[k];
                        self.new_ph[k] = self.ph[j] + harm(self.w_inst[j]) * delta;
                    }
                } else {
                    self.pk_ph.clear();
                    for &pk in &self.peaks {
                        let j = self.ks[pk];
                        self.pk_ph.push(self.ph[j] + harm(self.w_inst[j]) * delta);
                    }
                    let mut owner = 0usize;
                    for k in 0..k_len {
                        while owner + 1 < self.peaks.len() && (self.peaks[owner] + self.peaks[owner + 1]) / 2 < k {
                            owner += 1;
                        }
                        let pk = self.peaks[owner];
                        self.new_ph[k] = self.pk_ph[owner] + (self.ph[self.ks[k]] - self.ph[self.ks[pk]]);
                    }
                }
            }
            for k in 0..k_len {
                self.buf[k] = Complex::from_polar(self.mag_n[k], self.new_ph[k]);
            }
            for k in 1..n / 2 {
                self.buf[n - k] = self.buf[k].conj();
            }
            self.inv.process_with_scratch(&mut self.buf, &mut self.scratch);
            for i in 0..n {
                let y = self.buf[(i + half) % n].re * inv_n * self.win[i];
                if let Some(a) = acc.get_mut((start as usize + i) & am) {
                    *a += if y.is_finite() { y } else { 0.0 };
                }
            }
        }
    }

    /// Writes the output for the input samples just taken (`take` of them, the last ones) into
    /// `io` from `pos`: the input `n` samples ago, moved where the voice is corrected, mixed with
    /// that delayed input by `mix` (0..1 per sample).
    pub(super) fn emit(&mut self, f: &Front, io: &mut [Vec<f32>], pos: usize, take: usize, mut mix: impl FnMut() -> f32) {
        let n = self.n as u64;
        let am = self.amask;
        for i in 0..take {
            let o = self.emitted;
            self.emitted += 1;
            let m = mix();
            let Some(src) = o.checked_sub(n) else {
                for buf in io.iter_mut().take(f.channels) {
                    if let Some(y) = buf.get_mut(pos + i) {
                        *y = 0.0;
                    }
                }
                continue;
            };
            let slot = src as usize & am;
            let ws = self.wsum.get(slot).copied().unwrap_or(0.0);
            let ms = self.mask_state;
            for (c, acc) in self.acc.iter_mut().enumerate() {
                let dry = f.x(c, src);
                let a = acc.get(slot).copied().unwrap_or(0.0);
                if let Some(v) = acc.get_mut(slot) {
                    *v = 0.0;
                }
                let wet = if ws > 1e-3 { a / ws } else { f64::from(dry) };
                let moved = f64::from(dry) + ms * (wet - f64::from(dry));
                let moved = if moved.is_finite() { moved as f32 } else { dry };
                let out = if m <= 0.0 { dry } else { dry + m * (moved - dry) };
                if let Some(y) = io.get_mut(c).and_then(|b| b.get_mut(pos + i)) {
                    *y = out;
                }
            }
            if let Some(w) = self.wsum.get_mut(slot) {
                *w = 0.0;
            }
            // Towards the voiced state at the input time (5 ms one-pole), exactly 0 and 1 at rest.
            let (v, _, _) = f.at(src as f64);
            let goal = if v { 1.0 } else { 0.0 };
            self.mask_state += (goal - self.mask_state) * self.mask_coef;
            if (self.mask_state - goal).abs() < 1e-6 {
                self.mask_state = goal;
            }
        }
    }

    /// Addresses and capacities of the heap buffers (tests: nothing reallocates).
    #[cfg(test)]
    pub(super) fn buffers(&self) -> Vec<(usize, usize)> {
        fn of<T>(v: &Vec<T>) -> (usize, usize) {
            (v.as_ptr() as usize, v.capacity())
        }
        let mut out = vec![of(&self.win), of(&self.buf), of(&self.scratch), of(&self.mag), of(&self.peaks), of(&self.pk_ph), of(&self.wsum)];
        out.extend(self.acc.iter().map(of));
        out.extend(self.prev_ph.iter().map(of));
        out
    }
}
