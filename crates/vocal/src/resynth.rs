//! Resynthesis: a formant-preserving, phase-locked phase vocoder (a port of the spike's `m_pv`,
//! `voiced_mask` and `level_follow`), rendered per span so only changed frame ranges are redone.
//!
//! - [`changed_frames`] finds the frame runs whose curves changed.
//! - [`render_spans`] renders each run, dilated by `N + 10 ms`, into a [`RenderedSpan`].
//! - [`splice`] writes spans into a buffer; [`render_all`] does all three over a copy of the input.
//!
//! What comes out bit-identical to the input (with zero gain there):
//! - every sample outside every rendered span;
//! - inside a span, every sample whose voiced mask is exactly 0 (unvoiced audio away from
//!   voicing), since the output there is the input itself.
//!
//! Inside a span, voiced samples whose frames have zero shift still go through the vocoder (it
//! runs over the whole span once any frame in it is shifted) and match the input to about 1e-12,
//! not bit for bit. A span in which no frame is shifted skips the vocoder and is bit-identical
//! apart from its gain.
//!
//! A re-render covers the edited note plus any shifted or gain-changed audio within the influence
//! radius of its ends (a shifted frame changes the output up to `hop + N/2 + 15 ms` away, the last
//! term being the level follow's half-window), repeated until no such frame is that close: up to
//! the whole breath-to-breath phrase. That way a span's edges always land where both an earlier
//! render and the new one equal the dry input, so splicing it into a cached render is seamless.
//! A single span longer than 60 s (no breath for over a minute) is refused rather than rendered.

use crate::{EditCurves, PitchTrack, VocalError};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f64::consts::{PI, TAU};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Changed-frame runs whose gap is at most this many analysis frames merge (2 STFT hops ≈ 10.7 ms;
/// analysis frames are 5 ms).
const MERGE_GAP_FRAMES: usize = 2;
/// The cancel flag is checked every this many STFT frames.
const CANCEL_EVERY: usize = 32;
/// A frame whose ratio is this close to 1 keeps the original phase.
const IDENTITY_EPS: f64 = 1e-4;
/// Shifts are clamped to ± this many cents (two octaves).
const MAX_SHIFT_CENTS: f64 = 2400.0;
const MIN_RATE: u32 = 8_000;
const MAX_RATE: u32 = 384_000;
/// The longest single span rendered (seconds of samples).
const MAX_SPAN_SECS: u32 = 60;

/// Rendered output for samples `[start, start + samples.len())`.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedSpan {
    pub start: u64,
    pub samples: Vec<f32>,
}

/// The STFT size `N`, hop `H` and cepstral lifter at `sr`: (2048, 256, 48) at 48 kHz. `N` scales to
/// the nearest power of two, `H = N / 8`, and the lifter scales by `sr / 48000`.
pub fn stft_sizes(sr: u32) -> (usize, usize, usize) {
    let target = 2048.0 * f64::from(sr) / 48_000.0;
    let e = target.max(1.0).log2().round().clamp(8.0, 16.0) as u32;
    let n = 1usize << e;
    let lifter = ((48.0 * f64::from(sr) / 48_000.0).round() as usize).clamp(2, n / 2);
    (n, n / 8, lifter)
}

fn curve_at(c: &[f32], i: usize) -> f32 {
    c.get(i).copied().unwrap_or(0.0)
}

/// Maximal frame runs where `next` differs from `prev` (or from zero when `prev` is `None`). Runs
/// separated by at most 2 frames (closer than 2 STFT frames) are merged.
pub fn changed_frames(prev: Option<&EditCurves>, next: &EditCurves) -> Vec<Range<u32>> {
    let mut len = next.shift_cents.len().max(next.gain_db.len());
    if let Some(p) = prev {
        len = len.max(p.shift_cents.len()).max(p.gain_db.len());
    }
    // Compared as render_spans sees them: non-finite values count as 0.
    let clean = |v: f32| if v.is_finite() { v } else { 0.0 };
    let pair = |c: &EditCurves, i: usize| (clean(curve_at(&c.shift_cents, i)), clean(curve_at(&c.gain_db, i)));
    let mut runs: Vec<Range<usize>> = Vec::new();
    for i in 0..len {
        let now = pair(next, i);
        let differs = match prev {
            Some(p) => pair(p, i) != now,
            None => now != (0.0, 0.0),
        };
        if !differs {
            continue;
        }
        match runs.last_mut() {
            Some(r) if i - r.end <= MERGE_GAP_FRAMES => r.end = i + 1,
            _ => runs.push(i..i + 1),
        }
    }
    let to_u32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
    runs.into_iter().map(|r| to_u32(r.start)..to_u32(r.end)).collect()
}

/// Writes each span into `base`; the parts of a span outside `base` are ignored.
pub fn splice(base: &mut [f32], spans: &[RenderedSpan]) {
    for span in spans {
        let Ok(start) = usize::try_from(span.start) else { continue };
        for (i, &s) in span.samples.iter().enumerate() {
            if let Some(d) = start.checked_add(i).and_then(|j| base.get_mut(j)) {
                *d = s;
            }
        }
    }
}

/// Renders the whole input: `changed_frames(None, curves)` → [`render_spans`] → [`splice`] into a
/// copy of `input` (worker threads only). A zero-curve render returns the input unchanged.
pub fn render_all(input: &[f32], track: &PitchTrack, curves: &EditCurves, cancel: &AtomicBool) -> Result<Vec<f32>, VocalError> {
    let frames = changed_frames(None, curves);
    let spans = render_spans(input, track, curves, &frames, cancel)?;
    let mut out = input.to_vec();
    splice(&mut out, &spans);
    Ok(out)
}

/// Renders the frame ranges `frames` (worker threads only). Each span covers its frame range
/// dilated by `N + 10 ms`, clamped to the input; spans are sorted and non-overlapping.
///
/// A range is first grown over any non-zero pitch shift or gain within the influence radius of its
/// ends (see the module docs), so a span's edges never fall where an edited frame changes the
/// output (they then splice cleanly into an earlier render). Errors: [`VocalError::Unsupported`]
/// for a sample rate outside 8–384 kHz, a zero hop, or a span over 60 s (checked before anything
/// is rendered), [`VocalError::Cancelled`] once `cancel` is set (checked every 32 STFT frames).
pub fn render_spans(
    input: &[f32],
    track: &PitchTrack,
    curves: &EditCurves,
    frames: &[Range<u32>],
    cancel: &AtomicBool,
) -> Result<Vec<RenderedSpan>, VocalError> {
    let sr = track.sample_rate;
    if !(MIN_RATE..=MAX_RATE).contains(&sr) {
        return Err(VocalError::Unsupported(format!("sample rate {sr} Hz")));
    }
    let hop = track.hop_samples as usize;
    if hop == 0 {
        return Err(VocalError::Unsupported("zero hop".into()));
    }
    if input.is_empty() || frames.is_empty() {
        return Ok(Vec::new());
    }
    let (n, _, _) = stft_sizes(sr);
    let ms10 = sr as usize / 100;
    let shift = sanitize(&curves.shift_cents);
    let gain = sanitize(&curves.gain_db);
    let nf = shift.len().max(gain.len()).max(track.frames.len());
    let dil = n + ms10;
    // A shifted frame j changes the output up to j·hop + hop + N/2 + the level follow's half-window.
    let level_half = level_kernel_len(sr) / 2;
    let reach = (dil + hop + n / 2 + level_half).div_ceil(hop) + 1;
    let sample_spans = span_samples(frames, &shift, &gain, nf, hop, dil, reach, input.len());
    if sample_spans.is_empty() {
        return Ok(Vec::new());
    }
    let max_span = MAX_SPAN_SECS as usize * sr as usize;
    if let Some(&(a, b)) = sample_spans.iter().find(|&&(a, b)| b - a > max_span) {
        let secs = (b - a) as f64 / f64::from(sr);
        return Err(VocalError::Unsupported(format!("a {secs:.1} s render span: no breath for over {MAX_SPAN_SECS} s")));
    }
    let mut r = Renderer::new(input, track, shift, gain, cancel);
    let mut out = Vec::with_capacity(sample_spans.len());
    for (a, b) in sample_spans {
        let samples = r.render(a, b)?;
        out.push(RenderedSpan { start: a as u64, samples });
    }
    Ok(out)
}

/// Finite values (others become 0.0), as f64.
fn sanitize(c: &[f32]) -> Vec<f64> {
    c.iter().map(|&v| if v.is_finite() { f64::from(v) } else { 0.0 }).collect()
}

/// The sample spans `[a, b)` for `frames`: grown over shifted or gain-changed frames within
/// `reach` frames of their ends (repeatedly), dilated by `dil` samples, clamped to `len`, sorted
/// and merged.
fn span_samples(
    frames: &[Range<u32>],
    shift: &[f64],
    gain: &[f64],
    nf: usize,
    hop: usize,
    dil: usize,
    reach: usize,
    len: usize,
) -> Vec<(usize, usize)> {
    let edited = |i: usize| shift.get(i).is_some_and(|&v| v != 0.0) || gain.get(i).is_some_and(|&v| v != 0.0);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for fr in frames {
        let mut s = (fr.start as usize).min(nf);
        let mut e = (fr.end as usize).min(nf);
        if s >= e {
            continue;
        }
        loop {
            let left = (s.saturating_sub(reach)..s).find(|&j| edited(j));
            let right = (e..e.saturating_add(reach).min(nf)).rev().find(|&j| edited(j));
            if left.is_none() && right.is_none() {
                break;
            }
            if let Some(j) = left {
                s = j;
            }
            if let Some(j) = right {
                e = j + 1;
            }
        }
        let a = s.saturating_mul(hop).saturating_sub(dil).min(len);
        let b = (e - 1).saturating_mul(hop).saturating_add(dil).saturating_add(1).min(len);
        if a < b {
            spans.push((a, b));
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

/// `c` linearly interpolated at fractional frame `pos` (numpy `interp`: clamped at both ends).
fn interp_frames(c: &[f64], pos: f64) -> f64 {
    let Some(&last) = c.last() else { return 0.0 };
    let first = c.first().copied().unwrap_or(0.0);
    if pos.is_nan() || pos <= 0.0 {
        return first;
    }
    let i = pos.floor() as usize;
    match (c.get(i), c.get(i + 1)) {
        (Some(&v0), Some(&v1)) => v0 + (v1 - v0) * (pos - i as f64),
        _ => last,
    }
}

/// The level follow's window length: 30 ms (numpy `int(30 * sr / 1000)`).
fn level_kernel_len(sr: u32) -> usize {
    sr as usize * 3 / 100
}

/// Symmetric Hann of `len` points (numpy `hanning`), normalised to sum 1.
fn hann_kernel(len: usize) -> Vec<f64> {
    let len = len.max(1);
    let w: Vec<f64> = if len == 1 { vec![1.0] } else { (0..len).map(|i| 0.5 - 0.5 * (TAU * i as f64 / (len - 1) as f64).cos()).collect() };
    let s: f64 = w.iter().sum();
    if s > 0.0 { w.into_iter().map(|v| v / s).collect() } else { w }
}

struct Renderer<'a> {
    input: &'a [f32],
    hop: usize,
    n: usize,
    h: usize,
    ms10: usize,
    shift: Vec<f64>,
    gain: Vec<f64>,
    /// Per track frame: voiced (dilated by one frame) as 0.0 / 1.0.
    voiced: Vec<f64>,
    cancel: &'a AtomicBool,
    planner: FftPlanner<f64>,
    fwd: Arc<dyn Fft<f64>>,
    inv: Arc<dyn Fft<f64>>,
    win: Vec<f64>,
    lif: Vec<f64>,
    level_kernel: Vec<f64>,
    mask_kernel: Vec<f64>,
    // per-frame scratch (allocated once per call)
    buf: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    mag: Vec<f64>,
    ph: Vec<f64>,
    prev_ph: Vec<f64>,
    out_ph: Vec<f64>,
    w_inst: Vec<f64>,
    env: Vec<f64>,
    mag_n: Vec<f64>,
    ks: Vec<usize>,
    new_ph: Vec<f64>,
    peaks: Vec<usize>,
    pk_ph: Vec<f64>,
}

impl<'a> Renderer<'a> {
    fn new(input: &'a [f32], track: &PitchTrack, shift: Vec<f64>, gain: Vec<f64>, cancel: &'a AtomicBool) -> Self {
        let sr = track.sample_rate;
        let (n, h, lifter) = stft_sizes(sr);
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let scratch_len = fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len());
        let k = n / 2 + 1;
        let v: Vec<bool> = track.frames.iter().map(|f| f.cents.is_some()).collect();
        let voiced = (0..v.len())
            .map(|i| {
                let on = v.get(i).copied().unwrap_or(false)
                    || v.get(i + 1).copied().unwrap_or(false)
                    || i.checked_sub(1).and_then(|j| v.get(j).copied()).unwrap_or(false);
                if on { 1.0 } else { 0.0 }
            })
            .collect();
        let lif = (0..n).map(|q| if q < lifter || q + lifter > n { 1.0 } else { 0.0 }).collect();
        let ms10 = sr as usize / 100;
        Renderer {
            input,
            hop: track.hop_samples as usize,
            n,
            h,
            ms10,
            shift,
            gain,
            voiced,
            cancel,
            planner,
            fwd,
            inv,
            win: (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()).collect(),
            lif,
            level_kernel: hann_kernel(level_kernel_len(sr)),
            mask_kernel: hann_kernel(2 * ms10 + 1),
            buf: vec![Complex::default(); n],
            scratch: vec![Complex::default(); scratch_len],
            mag: vec![0.0; k],
            ph: vec![0.0; k],
            prev_ph: vec![0.0; k],
            out_ph: vec![0.0; k],
            w_inst: vec![0.0; k],
            env: vec![0.0; k],
            mag_n: vec![0.0; k],
            ks: vec![0; k],
            new_ph: vec![0.0; k],
            peaks: Vec::with_capacity(k),
            pk_ph: Vec::with_capacity(k),
        }
    }

    /// The input at sample `i` (0.0 outside the input or when not finite).
    fn x_at(&self, i: i64) -> f64 {
        usize::try_from(i).ok().and_then(|j| self.input.get(j)).map_or(0.0, |&v| if v.is_finite() { f64::from(v) } else { 0.0 })
    }

    fn frame_pos(&self, sample: i64) -> f64 {
        sample as f64 / self.hop as f64
    }

    /// Renders samples `[a, b)` of the input.
    fn render(&mut self, a: usize, b: usize) -> Result<Vec<f32>, VocalError> {
        let n = self.n as i64;
        let (a64, b64) = (a as i64, b as i64);
        // Work region [wa, wb): the span with N of real-input context on each side.
        let (wa, wb) = (a64 - n, b64 + n);
        let x: Vec<f64> = (wa..wb).map(|i| self.x_at(i)).collect();
        let y = self.vocode(wa, wb)?;
        let y = match y {
            Some(mut y) => {
                self.level_follow(&x, &mut y);
                y
            }
            None => x.clone(), // every frame is identity: the vocoder would only add rounding
        };
        let m = self.voiced_mask(a64, b64);
        let span = b - a;
        let fade_in = a > 0;
        let fade_out = b < self.input.len();
        let ramp = self.ms10.max(1) as f64;
        let ctx = self.n;
        let mut out = Vec::with_capacity(span);
        for i in 0..span {
            let xs = x.get(ctx + i).copied().unwrap_or(0.0);
            let mut ys = y.get(ctx + i).copied().unwrap_or(xs);
            if !ys.is_finite() {
                ys = xs;
            }
            let mut e = 1.0f64;
            if fade_in {
                e = e.min(i as f64 / ramp);
            }
            if fade_out {
                e = e.min((span - 1 - i) as f64 / ramp);
            }
            let mv = m.get(i).copied().unwrap_or(0.0);
            let gdb = interp_frames(&self.gain, self.frame_pos(a64 + i as i64));
            let g = if gdb == 0.0 { 1.0 } else { 10f64.powf(gdb / 20.0) };
            out.push(((xs + e * mv * (ys - xs)) * g) as f32);
        }
        Ok(out)
    }

    /// The phase vocoder over the work region `[wa, wb)` (the spike's `m_pv`, with real input as
    /// its padding). `None` when every frame's ratio is the identity.
    fn vocode(&mut self, wa: i64, wb: i64) -> Result<Option<Vec<f64>>, VocalError> {
        let (n, h) = (self.n, self.h);
        let k_len = n / 2 + 1;
        let origin = wa - n as i64;
        let total = (wb - wa) as usize + 2 * n;
        let nfr = (total - n).div_ceil(h) + 1;
        let ratios: Vec<f64> = (0..nfr)
            .map(|f| {
                let centre = origin + (f * h + n / 2) as i64;
                let cents = interp_frames(&self.shift, self.frame_pos(centre)).clamp(-MAX_SHIFT_CENTS, MAX_SHIFT_CENTS);
                2f64.powf(cents / 1200.0)
            })
            .collect();
        if ratios.iter().all(|r| (r - 1.0).abs() < IDENTITY_EPS) {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(VocalError::Cancelled);
            }
            return Ok(None);
        }
        let out_len = (nfr - 1) * h + n;
        let mut out = vec![0.0f64; out_len];
        let mut wsum = vec![0.0f64; out_len];
        self.prev_ph.iter_mut().for_each(|v| *v = 0.0);
        self.out_ph.iter_mut().for_each(|v| *v = 0.0);
        let inv_n = 1.0 / n as f64;
        for (f, &r) in ratios.iter().enumerate() {
            if f % CANCEL_EVERY == 0 && self.cancel.load(Ordering::Relaxed) {
                return Err(VocalError::Cancelled);
            }
            let p = origin + (f * h) as i64;
            // X = rfft(seg * win)
            for i in 0..n {
                let s = self.x_at(p + i as i64) * self.win[i];
                self.buf[i] = Complex::new(s, 0.0);
            }
            self.fwd.process_with_scratch(&mut self.buf, &mut self.scratch);
            for k in 0..k_len {
                let z = self.buf[k];
                self.mag[k] = z.norm() + 1e-12;
                let ph = z.im.atan2(z.re);
                let omega = TAU * k as f64 / n as f64;
                let dphi = (ph - self.prev_ph[k] - omega * h as f64 + PI).rem_euclid(TAU) - PI;
                self.w_inst[k] = omega + dphi / h as f64;
                self.prev_ph[k] = ph;
                self.ph[k] = ph;
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
            // Shift the excitation, keep the envelope.
            let last = k_len - 1;
            for k in 0..k_len {
                let src = k as f64 / r;
                let exc_at = |j: usize| self.mag[j] / self.env[j];
                let exc_n = if src > last as f64 {
                    0.0
                } else {
                    let i0 = src.floor() as usize;
                    if i0 >= last {
                        exc_at(last)
                    } else {
                        let (e0, e1) = (exc_at(i0), exc_at(i0 + 1));
                        e0 + (e1 - e0) * (src - i0 as f64)
                    }
                };
                self.mag_n[k] = exc_n * self.env[k];
                self.ks[k] = (src.round_ties_even().max(0.0) as usize).min(last);
            }
            if (r - 1.0).abs() < IDENTITY_EPS {
                self.new_ph.copy_from_slice(&self.ph);
            } else {
                self.peaks.clear();
                for k in 1..last {
                    if self.mag_n[k] > self.mag_n[k - 1] && self.mag_n[k] >= self.mag_n[k + 1] {
                        self.peaks.push(k);
                    }
                }
                if self.peaks.is_empty() {
                    for k in 0..k_len {
                        self.new_ph[k] = self.ph[self.ks[k]];
                    }
                } else {
                    self.pk_ph.clear();
                    for &pk in &self.peaks {
                        self.pk_ph.push(self.out_ph[pk] + h as f64 * r * self.w_inst[self.ks[pk]]);
                    }
                    // Each bin belongs to its nearest peak (midpoint bounds, numpy searchsorted).
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
            self.out_ph.copy_from_slice(&self.new_ph);
            // y = irfft(mag_n * e^{i new_ph}) * win, overlap-added.
            for k in 0..k_len {
                self.buf[k] = Complex::from_polar(self.mag_n[k], self.new_ph[k]);
            }
            for k in 1..n / 2 {
                self.buf[n - k] = self.buf[k].conj();
            }
            self.inv.process_with_scratch(&mut self.buf, &mut self.scratch);
            let at = f * h;
            for i in 0..n {
                let w = self.win[i];
                out[at + i] += self.buf[i].re * inv_n * w;
                wsum[at + i] += w * w;
            }
        }
        // The work region is out[n .. n + (wb - wa)].
        let len = (wb - wa) as usize;
        Ok(Some(out.iter().zip(&wsum).skip(n).take(len).map(|(o, w)| o / w.max(1e-3)).collect()))
    }

    /// The spike's `level_follow`: y *= clamp(env30ms(x) / env30ms(y), 0.5, 2).
    fn level_follow(&mut self, x: &[f64], y: &mut [f64]) {
        let sq = |v: &[f64]| v.iter().map(|s| s * s).collect::<Vec<f64>>();
        let kernel = std::mem::take(&mut self.level_kernel);
        let ex = conv_same(&mut self.planner, &sq(x), &kernel);
        let ey = conv_same(&mut self.planner, &sq(y), &kernel);
        self.level_kernel = kernel;
        for (i, v) in y.iter_mut().enumerate() {
            let a = (ex.get(i).copied().unwrap_or(0.0).max(0.0) + 1e-10).sqrt();
            let b = (ey.get(i).copied().unwrap_or(0.0).max(0.0) + 1e-10).sqrt();
            *v *= (a / b).clamp(0.5, 2.0);
        }
    }

    /// The spike's `voiced_mask` for samples `[a, b)`: voiced frames (dilated by one frame),
    /// linearly interpolated to samples and smoothed by a normalised Hann of `2 * 10 ms + 1`.
    /// Exactly 0.0 where no voiced sample is within the kernel, exactly 1.0 where all are.
    fn voiced_mask(&mut self, a: i64, b: i64) -> Vec<f64> {
        let half = (self.mask_kernel.len() / 2) as i64;
        let vs: Vec<f64> = (a - half..b + half).map(|s| interp_frames(&self.voiced, self.frame_pos(s))).collect();
        // Prefix counts of non-zero and non-one samples.
        let mut nz = Vec::with_capacity(vs.len() + 1);
        let mut n1 = Vec::with_capacity(vs.len() + 1);
        nz.push(0usize);
        n1.push(0usize);
        for &v in &vs {
            nz.push(nz.last().copied().unwrap_or(0) + usize::from(v != 0.0));
            n1.push(n1.last().copied().unwrap_or(0) + usize::from(v != 1.0));
        }
        let kernel = std::mem::take(&mut self.mask_kernel);
        let conv = conv_same(&mut self.planner, &vs, &kernel);
        self.mask_kernel = kernel;
        let w = 2 * half as usize + 1;
        (0..(b - a) as usize)
            .map(|i| {
                let count = |p: &[usize]| p.get(i + w).copied().unwrap_or(0).saturating_sub(p.get(i).copied().unwrap_or(0));
                if count(&nz) == 0 {
                    0.0
                } else if count(&n1) == 0 {
                    1.0
                } else {
                    conv.get(i + half as usize).copied().unwrap_or(0.0).clamp(0.0, 1.0)
                }
            })
            .collect()
    }
}

/// numpy `convolve(x, k, mode="same")` (zero padding) by FFT overlap-add.
fn conv_same(planner: &mut FftPlanner<f64>, x: &[f64], k: &[f64]) -> Vec<f64> {
    if x.is_empty() || k.is_empty() {
        return vec![0.0; x.len()];
    }
    let m = (2 * k.len()).next_power_of_two().max(64);
    let block = m - k.len() + 1;
    let fwd = planner.plan_fft_forward(m);
    let inv = planner.plan_fft_inverse(m);
    let mut scratch = vec![Complex::default(); fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len())];
    let mut kf: Vec<Complex<f64>> = (0..m).map(|i| Complex::new(k.get(i).copied().unwrap_or(0.0), 0.0)).collect();
    fwd.process_with_scratch(&mut kf, &mut scratch);
    let full_len = x.len() + k.len() - 1;
    let mut full = vec![0.0f64; full_len];
    let mut buf = vec![Complex::default(); m];
    let scale = 1.0 / m as f64;
    for start in (0..x.len()).step_by(block) {
        for (i, z) in buf.iter_mut().enumerate() {
            let v = if i < block { x.get(start + i).copied().unwrap_or(0.0) } else { 0.0 };
            *z = Complex::new(v, 0.0);
        }
        fwd.process_with_scratch(&mut buf, &mut scratch);
        for (z, kz) in buf.iter_mut().zip(&kf) {
            *z *= kz;
        }
        inv.process_with_scratch(&mut buf, &mut scratch);
        for (i, z) in buf.iter().enumerate() {
            if let Some(d) = full.get_mut(start + i) {
                *d += z.re * scale;
            }
        }
    }
    let off = (k.len() - 1) / 2;
    full.into_iter().skip(off).take(x.len()).collect()
}
