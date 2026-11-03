//! A pYIN-style monophonic pitch tracker at 5 ms hops.
//!
//! 1. Non-finite samples become 0.0.
//! 2. Per frame (`W = next_pow2(ceil(sr * 0.042))` samples, centred on `i * hop`, zero-padded at the
//!    edges): the YIN difference function from an FFT autocorrelation, divided by the number of
//!    terms in each lag (so the analysis stays centred on the frame for every lag), then its
//!    cumulative-mean-normalised form `d'(tau)`.
//! 3. pYIN candidates: 100 thresholds `0.01..=1.00` weighted by a Beta(2, 18) prior; for each, the
//!    first local minimum of `d'` below it (else the deepest in-range minimum at 1 % of the
//!    weight), its lag refined by parabolic interpolation. The search starts at lag 2, below the
//!    1000 Hz lag, so a tone above the range is recognised (and dropped) rather than read as its
//!    sub-octave.
//! 4. A Viterbi path over 20-cent pitch bins from 70 Hz to 1000 Hz, each with an unvoiced twin:
//!    triangular transitions over ±12 bins per frame, voicing switch probability 0.01.
//! 5. Output cents are the interpolated candidate inside the chosen bin; frames quieter than
//!    -60 dB are unvoiced, and so are frames whose best candidate in that bin has a probability of
//!    0.02 or less (a candidate-probability gate). It drops what only the no-trough fallback
//!    supports (at most 0.01: white noise would otherwise wander along its deepest minimum near
//!    70–80 Hz) and the barely supported frames at note onsets and tails, where the pitch is not
//!    yet reliable (tuned on the real take against the spike's Harvest track).
//! 6. Octave repair against the median of the voiced cents within ±25 frames.
//! 7. `level_db` is the RMS level over ±10 ms.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::VocalError;

/// Lowest tracked fundamental (Hz).
const F0_FLOOR: f64 = 70.0;
/// Highest tracked fundamental (Hz).
const F0_CEIL: f64 = 1000.0;
/// Frame length in seconds before rounding up to a power of two.
const FRAME_SECS: f64 = 0.042;
/// Width of one Viterbi pitch bin (cents).
const BIN_CENTS: f64 = 20.0;
/// pYIN thresholds `0.01, 0.02, ..., 1.00`.
const N_THRESHOLDS: usize = 100;
/// Beta prior over the thresholds.
const BETA_A: i32 = 2;
const BETA_B: i32 = 18;
/// Weight kept by a threshold that finds no minimum below it (given to the deepest minimum).
const NO_TROUGH: f64 = 0.01;
/// Candidate-probability gate: a path frame is voiced only when its best candidate in the chosen
/// bin has a probability above this. Twice the most the no-trough fallback alone can give
/// ([`NO_TROUGH`]), so noise is dropped, and high enough to drop the weakly supported onset and
/// tail frames (0.02 keeps the real-take test's p90 and voicing agreement both inside their bars
/// with margin; just above 0.01 failed p90, 0.05 left voicing at 90.4 %).
const MIN_PATH_PROB: f64 = 0.02;
/// Largest pitch move per frame in the transition model (bins).
const MAX_STEP: usize = 12;
/// Probability of switching between voiced and unvoiced from one frame to the next.
const SWITCH_PROB: f64 = 0.01;
/// Emission floor (keeps the log finite).
const EMISSION_FLOOR: f64 = 1e-12;
/// Frames quieter than this are unvoiced.
const SILENCE_DB: f32 = -60.0;
/// RMS window for `level_db` (seconds, whole width).
const LEVEL_WIN_SECS: f64 = 0.02;
/// Octave repair window (frames each side).
const OCTAVE_HALF: usize = 25;
/// Check the cancel flag once per this many frames.
const CANCEL_EVERY: usize = 64;
/// Longest input accepted (seconds): bounds the Viterbi back-pointer table.
pub const MAX_ANALYSIS_SECS: u64 = 30 * 60;
/// Accepted sample rates.
const MIN_RATE: u32 = 8_000;
const MAX_RATE: u32 = 384_000;

/// One analysis frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PitchFrame {
    /// Pitch as MIDI note × 100 (A4 = 6900); `None` when unvoiced.
    pub cents: Option<f32>,
    /// RMS level over ±10 ms around the frame centre (dB re full scale).
    pub level_db: f32,
    /// pYIN voicing probability (sum of the in-range candidate probabilities, 0..=1).
    pub voiced_prob: f32,
}

/// A clip's pitch track: frame `i` is centred on sample `i * hop_samples`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PitchTrack {
    pub sample_rate: u32,
    pub hop_samples: u32,
    pub frames: Vec<PitchFrame>,
}

impl PitchTrack {
    /// The frame whose centre is nearest sample `s`, clamped to the last frame (0 when empty).
    pub fn frame_of_sample(&self, s: u64) -> usize {
        let hop = u64::from(self.hop_samples);
        if self.frames.is_empty() || hop == 0 {
            return 0;
        }
        let i = s.saturating_add(hop / 2) / hop;
        usize::try_from(i).unwrap_or(usize::MAX).min(self.frames.len() - 1)
    }

    /// The sample frame `i` is centred on.
    pub fn sample_of_frame(&self, i: usize) -> u64 {
        u64::try_from(i).unwrap_or(u64::MAX).saturating_mul(u64::from(self.hop_samples))
    }
}

/// Tracks the pitch of a monophonic recording (worker threads only).
///
/// Returns [`VocalError::Unsupported`] for a sample rate outside 8–384 kHz or an input longer
/// than 30 minutes, and [`VocalError::Cancelled`] once `cancel` is set (checked every 64 frames).
pub fn track_pitch(samples: &[f32], sample_rate: u32, cancel: &AtomicBool) -> Result<PitchTrack, VocalError> {
    if !(MIN_RATE..=MAX_RATE).contains(&sample_rate) {
        return Err(VocalError::Unsupported(format!("sample rate {sample_rate} Hz")));
    }
    let max_len = MAX_ANALYSIS_SECS.saturating_mul(u64::from(sample_rate));
    if u64::try_from(samples.len()).unwrap_or(u64::MAX) > max_len {
        return Err(VocalError::Unsupported(format!("clips longer than {} minutes", MAX_ANALYSIS_SECS / 60)));
    }
    check(cancel)?;
    let sr = f64::from(sample_rate);
    let hop = ((sr / 200.0).round() as usize).max(1);
    let x: Vec<f64> = samples.iter().map(|&s| if s.is_finite() { f64::from(s) } else { 0.0 }).collect();
    let n_frames = if x.is_empty() { 0 } else { (x.len() - 1) / hop + 1 };

    let levels = frame_levels(&x, sr, hop, n_frames);
    let mut yin = Yin::new(sr);
    let mut cands: Vec<Vec<Cand>> = Vec::with_capacity(n_frames);
    let mut i = 0;
    while i < n_frames {
        if i % CANCEL_EVERY == 0 {
            check(cancel)?;
        }
        let loud = |k: usize| levels.get(k).is_some_and(|&l| l >= SILENCE_DB);
        let a = loud(i).then_some(i * hop);
        let b = (i + 1 < n_frames && loud(i + 1)).then_some((i + 1) * hop);
        let (ca, cb) = yin.analyse_pair(&x, a, b);
        cands.push(ca);
        if i + 1 < n_frames {
            cands.push(cb);
        }
        i += 2;
    }

    let bins = Bins::new();
    let path = viterbi(&cands, &bins, cancel)?;

    let mut cents: Vec<Option<f32>> = path
        .iter()
        .zip(&cands)
        .zip(&levels)
        .map(|((state, cs), &level)| {
            let b = (*state)?;
            if level < SILENCE_DB {
                return None;
            }
            // The candidate-probability gate: a candidate held up only by the no-trough fallback
            // (noise would drift along it) or barely supported (onsets, tails) is not a pitch.
            cs.iter()
                .filter(|c| bins.of(c.cents) == Some(b) && c.prob > MIN_PATH_PROB)
                .max_by(|p, q| p.prob.total_cmp(&q.prob))
                .map(|c| c.cents as f32)
        })
        .collect();
    fix_octaves(&mut cents);

    let frames = cents
        .iter()
        .zip(&cands)
        .zip(&levels)
        .map(|((&c, cs), &level_db)| PitchFrame { cents: c, level_db, voiced_prob: cs.iter().map(|c| c.prob).sum::<f64>().clamp(0.0, 1.0) as f32 })
        .collect();
    Ok(PitchTrack { sample_rate, hop_samples: u32::try_from(hop).unwrap_or(u32::MAX), frames })
}

fn check(cancel: &AtomicBool) -> Result<(), VocalError> {
    if cancel.load(Ordering::Relaxed) { Err(VocalError::Cancelled) } else { Ok(()) }
}

fn hz_to_cents(hz: f64) -> f64 {
    6900.0 + 1200.0 * (hz / 440.0).log2()
}

/// `10 * log10(mean(x^2) + 1e-12)` over ±10 ms around each frame centre (the spike's `frame_rms_db`).
fn frame_levels(x: &[f64], sr: f64, hop: usize, n_frames: usize) -> Vec<f32> {
    let mut prefix = Vec::with_capacity(x.len() + 1);
    let mut acc = 0.0f64;
    prefix.push(acc);
    for v in x {
        acc += v * v;
        prefix.push(acc);
    }
    let h = (LEVEL_WIN_SECS * sr / 2.0) as usize;
    (0..n_frames)
        .map(|i| {
            let c = i.saturating_mul(hop);
            let lo = c.saturating_sub(h).min(x.len());
            let hi = c.saturating_add(h).min(x.len());
            if hi <= lo {
                return -120.0;
            }
            let sum = prefix.get(hi).copied().unwrap_or(0.0) - prefix.get(lo).copied().unwrap_or(0.0);
            (10.0 * (sum.max(0.0) / (hi - lo) as f64 + 1e-12).log10()) as f32
        })
        .collect()
}

/// One pitch candidate of a frame.
#[derive(Debug, Clone, Copy)]
struct Cand {
    /// The integer lag of the `d'` minimum it came from.
    tau: usize,
    /// Interpolated pitch (MIDI cents).
    cents: f64,
    /// Accumulated prior weight.
    prob: f64,
}

/// The per-frame YIN / pYIN analysis, with its buffers.
struct Yin {
    sr: f64,
    w: usize,
    tau_min: usize,
    tau_max: usize,
    cents_lo: f64,
    cents_hi: f64,
    weights: Vec<f64>,
    fwd: Arc<dyn Fft<f64>>,
    inv: Arc<dyn Fft<f64>>,
    buf: Vec<Complex<f64>>,
    spec: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    frame_a: Vec<f64>,
    frame_b: Vec<f64>,
    prefix: Vec<f64>,
    dn: Vec<f64>,
    dp: Vec<f64>,
}

impl Yin {
    fn new(sr: f64) -> Self {
        let w = ((sr * FRAME_SECS).ceil() as usize).next_power_of_two();
        let n = 2 * w;
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let scratch_len = fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len());
        let tau_max = ((sr / F0_FLOOR).ceil() as usize).min(w.saturating_sub(2));
        let tau_min = ((sr / F0_CEIL).floor() as usize).max(2).min(tau_max);
        // Beta(2, 18) density at each threshold, normalised.
        let mut weights: Vec<f64> = (1..=N_THRESHOLDS)
            .map(|j| {
                let s = j as f64 / N_THRESHOLDS as f64;
                s.powi(BETA_A - 1) * (1.0 - s).powi(BETA_B - 1)
            })
            .collect();
        let total: f64 = weights.iter().sum();
        if total > 0.0 {
            weights.iter_mut().for_each(|v| *v /= total);
        }
        Yin {
            sr,
            w,
            tau_min,
            tau_max,
            cents_lo: hz_to_cents(F0_FLOOR),
            cents_hi: hz_to_cents(F0_CEIL),
            weights,
            fwd,
            inv,
            buf: vec![Complex::default(); n],
            spec: vec![Complex::default(); n],
            scratch: vec![Complex::default(); scratch_len],
            frame_a: vec![0.0; w],
            frame_b: vec![0.0; w],
            prefix: vec![0.0; w + 1],
            dn: vec![0.0; tau_max + 2],
            dp: vec![1.0; tau_max + 2],
        }
    }

    /// Analyses up to two frames (centred on the given samples) with one FFT pair: frame `a` in the
    /// real part, `b` in the imaginary part. `None` gives no candidates.
    fn analyse_pair(&mut self, x: &[f64], a: Option<usize>, b: Option<usize>) -> (Vec<Cand>, Vec<Cand>) {
        if a.is_none() && b.is_none() {
            return (Vec::new(), Vec::new());
        }
        let w = self.w;
        fill_frame(&mut self.frame_a, x, a);
        fill_frame(&mut self.frame_b, x, b);
        for (k, z) in self.buf.iter_mut().enumerate() {
            *z = match (self.frame_a.get(k), self.frame_b.get(k)) {
                (Some(&re), Some(&im)) => Complex::new(re, im),
                _ => Complex::default(),
            };
        }
        self.fwd.process_with_scratch(&mut self.buf, &mut self.scratch);
        // Split the packed spectrum into the two power spectra: |A|^2 + i |B|^2.
        let n = self.buf.len();
        for k in 0..n {
            let zk = self.buf.get(k).copied().unwrap_or_default();
            let zn = self.buf.get((n - k) % n).copied().unwrap_or_default().conj();
            let pa = (zk + zn).norm_sqr() / 4.0;
            let pb = (zk - zn).norm_sqr() / 4.0;
            if let Some(s) = self.spec.get_mut(k) {
                *s = Complex::new(pa, pb);
            }
        }
        self.inv.process_with_scratch(&mut self.spec, &mut self.scratch);
        let scale = 1.0 / n as f64;
        let ca = if a.is_some() {
            let acf: Vec<f64> = self.spec.iter().take(w).map(|z| z.re * scale).collect();
            self.candidates(true, &acf)
        } else {
            Vec::new()
        };
        let cb = if b.is_some() {
            let acf: Vec<f64> = self.spec.iter().take(w).map(|z| z.im * scale).collect();
            self.candidates(false, &acf)
        } else {
            Vec::new()
        };
        (ca, cb)
    }

    /// pYIN candidates of one frame from its autocorrelation `acf` (lags `0..w`).
    fn candidates(&mut self, use_a: bool, acf: &[f64]) -> Vec<Cand> {
        let w = self.w;
        let frame = if use_a { &self.frame_a } else { &self.frame_b };
        let mut acc = 0.0;
        for (k, v) in frame.iter().enumerate() {
            if let Some(p) = self.prefix.get_mut(k) {
                *p = acc;
            }
            acc += v * v;
        }
        if let Some(p) = self.prefix.get_mut(w) {
            *p = acc;
        }
        let total = acc;
        let pre = |k: usize| self.prefix.get(k).copied().unwrap_or(0.0);
        // Mean squared difference per lag, then the cumulative-mean-normalised form.
        let mut sum = 0.0;
        for tau in 1..=self.tau_max {
            let e1 = pre(w - tau);
            let e2 = total - pre(tau);
            let r = acf.get(tau).copied().unwrap_or(0.0);
            let d = ((e1 + e2 - 2.0 * r).max(0.0)) / (w - tau) as f64;
            if let Some(v) = self.dn.get_mut(tau) {
                *v = d;
            }
            sum += d;
            let dp = if sum > 0.0 { d * tau as f64 / sum } else { 1.0 };
            if let Some(v) = self.dp.get_mut(tau) {
                *v = dp;
            }
        }
        let dp = |t: usize| self.dp.get(t).copied().unwrap_or(1.0);
        // Interior local minima of d', in lag order (lag 2 up, so above-range pitches show first).
        let minima: Vec<(usize, f64)> = (2..self.tau_max).filter(|&t| dp(t) < dp(t - 1) && dp(t) <= dp(t + 1)).map(|t| (t, dp(t))).collect();
        let deepest = minima.iter().filter(|(t, _)| *t >= self.tau_min).min_by(|p, q| p.1.total_cmp(&q.1)).map(|&(t, _)| t);
        let mut out: Vec<Cand> = Vec::new();
        for (j, &wt) in self.weights.iter().enumerate() {
            let s = (j + 1) as f64 / N_THRESHOLDS as f64;
            let (tau, p) = match minima.iter().find(|(_, v)| *v < s) {
                Some(&(t, _)) => (t, wt),
                None => match deepest {
                    Some(t) => (t, wt * NO_TROUGH),
                    None => continue,
                },
            };
            if tau < self.tau_min || p <= 0.0 {
                continue; // above 1000 Hz (or no weight): no in-range candidate
            }
            if let Some(c) = out.iter_mut().find(|c| c.tau == tau) {
                c.prob += p;
                continue;
            }
            let cents = hz_to_cents(self.sr / self.refine(tau));
            if cents.is_finite() && cents >= self.cents_lo && cents < self.cents_hi {
                out.push(Cand { tau, cents, prob: p });
            }
        }
        out
    }

    /// Parabolic interpolation of the lag around the minimum at `tau` (on the difference function).
    fn refine(&self, tau: usize) -> f64 {
        let v = |t: usize| self.dn.get(t).copied().unwrap_or(0.0);
        let (a, b, c) = (v(tau - 1), v(tau), v(tau + 1));
        let denom = a - 2.0 * b + c;
        let shift = if denom > 0.0 { (0.5 * (a - c) / denom).clamp(-1.0, 1.0) } else { 0.0 };
        tau as f64 + shift
    }
}

/// Copies the `out.len()` samples centred on `centre` (zero outside the input; all zero for `None`).
fn fill_frame(out: &mut [f64], x: &[f64], centre: Option<usize>) {
    let Some(c) = centre else {
        out.iter_mut().for_each(|v| *v = 0.0);
        return;
    };
    let half = out.len() / 2;
    for (k, v) in out.iter_mut().enumerate() {
        *v = (c + k).checked_sub(half).and_then(|i| x.get(i)).copied().unwrap_or(0.0);
    }
}

/// The Viterbi pitch grid: 20-cent bins from 70 Hz to 1000 Hz.
struct Bins {
    lo: f64,
    n: usize,
}

impl Bins {
    fn new() -> Self {
        let lo = hz_to_cents(F0_FLOOR);
        let n = ((hz_to_cents(F0_CEIL) - lo) / BIN_CENTS).ceil() as usize;
        Bins { lo, n: n.max(1) }
    }

    fn of(&self, cents: f64) -> Option<usize> {
        let b = ((cents - self.lo) / BIN_CENTS).floor();
        (b >= 0.0 && b < self.n as f64).then_some(b as usize)
    }
}

/// The most likely voiced-bin / unvoiced path: `Some(bin)` for voiced frames.
fn viterbi(cands: &[Vec<Cand>], bins: &Bins, cancel: &AtomicBool) -> Result<Vec<Option<usize>>, VocalError> {
    let nb = bins.n;
    let ns = 2 * nb;
    let t_len = cands.len();
    if t_len == 0 {
        return Ok(Vec::new());
    }
    let span = 2 * MAX_STEP + 1;
    let tri_total: f64 = (0..span).map(|k| (MAX_STEP + 1 - k.abs_diff(MAX_STEP)) as f64).sum();
    let ltri: Vec<f64> = (0..span).map(|k| ((MAX_STEP + 1 - k.abs_diff(MAX_STEP)) as f64 / tri_total).ln()).collect();
    let (l_stay, l_switch) = ((1.0 - SWITCH_PROB).ln(), SWITCH_PROB.ln());
    let code_switch = u8::try_from(span).unwrap_or(u8::MAX);

    let mut em_v = vec![0.0f64; nb];
    let emissions = |cs: &[Cand], em_v: &mut [f64]| -> f64 {
        let mut probs = vec![0.0f64; nb];
        let mut vp = 0.0;
        for c in cs {
            if let Some(p) = bins.of(c.cents).and_then(|b| probs.get_mut(b)) {
                *p += c.prob;
            }
            vp += c.prob;
        }
        for (e, p) in em_v.iter_mut().zip(&probs) {
            *e = p.max(EMISSION_FLOOR).ln();
        }
        ((1.0 - vp.min(1.0)) / nb as f64).max(EMISSION_FLOOR).ln()
    };

    let mut prev = vec![0.0f64; ns];
    let mut next = vec![0.0f64; ns];
    let l_init = (1.0 / ns as f64).ln();
    let em_u = emissions(cands.first().map_or(&[][..], |c| c.as_slice()), &mut em_v);
    for (s, d) in prev.iter_mut().enumerate() {
        *d = l_init + if s < nb { em_v.get(s).copied().unwrap_or(0.0) } else { em_u };
    }
    let mut back = vec![0u8; t_len.saturating_mul(ns)];
    for t in 1..t_len {
        if t % CANCEL_EVERY == 0 {
            check(cancel)?;
        }
        let em_u = emissions(cands.get(t).map_or(&[][..], |c| c.as_slice()), &mut em_v);
        let row = back.get_mut(t * ns..(t + 1) * ns).unwrap_or(&mut []);
        for s in 0..ns {
            let voiced = s < nb;
            let b_to = s % nb;
            let (same_base, other_base) = if voiced { (0, nb) } else { (nb, 0) };
            let lo = b_to.saturating_sub(MAX_STEP);
            let hi = (b_to + MAX_STEP).min(nb - 1);
            let mut best = f64::NEG_INFINITY;
            let mut code = 0u8;
            for b_from in lo..=hi {
                let k = b_from + MAX_STEP - b_to;
                let lt = ltri.get(k).copied().unwrap_or(f64::NEG_INFINITY);
                let stay = prev.get(same_base + b_from).copied().unwrap_or(f64::NEG_INFINITY) + lt + l_stay;
                let sw = prev.get(other_base + b_from).copied().unwrap_or(f64::NEG_INFINITY) + lt + l_switch;
                let kc = u8::try_from(k).unwrap_or(0);
                if stay > best {
                    best = stay;
                    code = kc;
                }
                if sw > best {
                    best = sw;
                    code = kc.saturating_add(code_switch);
                }
            }
            let em = if voiced { em_v.get(b_to).copied().unwrap_or(0.0) } else { em_u };
            if let Some(d) = next.get_mut(s) {
                *d = best + em;
            }
            if let Some(r) = row.get_mut(s) {
                *r = code;
            }
        }
        // Keep the log-probabilities near zero.
        let top = next.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if top.is_finite() {
            next.iter_mut().for_each(|d| *d -= top);
        }
        std::mem::swap(&mut prev, &mut next);
    }

    let mut s = prev.iter().enumerate().max_by(|p, q| p.1.total_cmp(q.1)).map_or(nb, |(i, _)| i);
    let mut path = vec![None; t_len];
    for t in (0..t_len).rev() {
        if let Some(p) = path.get_mut(t) {
            *p = (s < nb).then_some(s % nb);
        }
        if t == 0 {
            break;
        }
        let code = back.get(t * ns + s).copied().unwrap_or(0);
        let switched = code >= code_switch;
        let k = usize::from(if switched { code - code_switch } else { code });
        let voiced_to = s < nb;
        let b_from = (s % nb + k).saturating_sub(MAX_STEP).min(nb - 1);
        let voiced_from = voiced_to != switched;
        s = if voiced_from { b_from } else { nb + b_from };
    }
    Ok(path)
}

/// The spike's `fix_octaves_np`: a voiced frame about an octave above (below) the median of the
/// voiced frames within ±25 frames moves down (up) an octave.
fn fix_octaves(cents: &mut [Option<f32>]) {
    let orig: Vec<Option<f32>> = cents.to_vec();
    let mut win: Vec<f32> = Vec::with_capacity(2 * OCTAVE_HALF + 1);
    for (i, out) in cents.iter_mut().enumerate() {
        let Some(c) = *out else { continue };
        let lo = i.saturating_sub(OCTAVE_HALF);
        let hi = (i + OCTAVE_HALF + 1).min(orig.len());
        win.clear();
        win.extend(orig.get(lo..hi).unwrap_or(&[]).iter().flatten());
        let Some(med) = median(&mut win) else { continue };
        let d = c - med;
        if (d - 1200.0).abs() < 250.0 {
            *out = Some(c - 1200.0);
        } else if (d + 1200.0).abs() < 250.0 {
            *out = Some(c + 1200.0);
        }
    }
}

/// The median (mean of the middle two for an even count); `None` when empty.
fn median(v: &mut [f32]) -> Option<f32> {
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n == 0 {
        return None;
    }
    let hi = v.get(n / 2).copied()?;
    if n % 2 == 1 { Some(hi) } else { Some((v.get(n / 2 - 1).copied()? + hi) / 2.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octave_repair_moves_an_outlier_back() {
        let mut c: Vec<Option<f32>> = vec![Some(6000.0); 40];
        c[20] = Some(7200.0);
        c[21] = Some(4810.0);
        fix_octaves(&mut c);
        assert_eq!(c[20], Some(6000.0));
        assert_eq!(c[21], Some(6010.0));
        assert_eq!(c[0], Some(6000.0));
    }

    #[test]
    fn median_handles_even_and_empty() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [3.0, 1.0]), Some(2.0));
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
    }
}
