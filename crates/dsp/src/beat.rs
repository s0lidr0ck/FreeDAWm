//! Beat tracking for Detect Tempo: a spectral-flux onset envelope, an autocorrelation tempo
//! estimate and dynamic-programming beat tracking (after D. Ellis, "Beat Tracking by Dynamic
//! Programming", 2007), with downbeats from bar-level accent.
//!
//! Offline only (allocates); never panics. Positions are sample indices into the analysed audio.

use std::f64::consts::PI;

use rustfft::{FftPlanner, num_complex::Complex};

use crate::util::sane_sr;

/// Longest audio [`track_beats`] analyses (samples); the rest is ignored.
pub const MAX_BEAT_SAMPLES: usize = 1 << 27;

/// What to look for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeatOptions {
    /// Tempo range (beats per minute) the beat period is chosen from: fixes half/double tempo.
    pub min_bpm: f64,
    pub max_bpm: f64,
    /// Beats per bar, for downbeats.
    pub beats_per_bar: usize,
}

impl Default for BeatOptions {
    fn default() -> Self {
        BeatOptions { min_bpm: 60.0, max_bpm: 200.0, beats_per_bar: 4 }
    }
}

/// The result of [`track_beats`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeatTrack {
    /// Average beats per minute over the tracked beats (0 when fewer than two beats).
    pub bpm: f64,
    /// Beat positions (samples), ascending.
    pub beats: Vec<usize>,
    /// The beats that start bars (a subset of `beats`).
    pub downbeats: Vec<usize>,
    /// Index in `beats` of the first downbeat.
    pub first_downbeat: usize,
}

/// Average the channels into one.
pub fn downmix(ch: &[Vec<f32>]) -> Vec<f32> {
    let len = ch.iter().map(Vec::len).max().unwrap_or(0).min(MAX_BEAT_SAMPLES);
    let n = ch.len().max(1) as f32;
    let mut out = vec![0.0f32; len];
    for c in ch {
        for (o, v) in out.iter_mut().zip(c) {
            if v.is_finite() {
                *o += *v / n;
            }
        }
    }
    out
}

/// Onset envelopes of `x`: the full-band and the low-band (< 200 Hz) spectral flux, one value per
/// hop, each high-passed and normalised to unit deviation.
struct Envelope {
    full: Vec<f64>,
    low: Vec<f64>,
    hop: usize,
    n: usize,
}

fn envelope(x: &[f32], sr: f32) -> Envelope {
    let n = ((sr * 0.023) as usize).clamp(256, 8192).next_power_of_two();
    let hop = n / 4;
    let frames = if x.len() >= n { (x.len() - n) / hop + 1 } else { 0 };
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let window: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32).collect();
    let bins = n / 2 + 1;
    let low_bins = ((200.0 * n as f32 / sr) as usize).clamp(2, bins);
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut prev = vec![0.0f32; bins];
    let mut cur = vec![0.0f32; bins];
    let (mut full, mut low) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    for m in 0..frames {
        let seg = x.get(m * hop..m * hop + n).unwrap_or(&[]);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = Complex::new(seg.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0) * window.get(i).copied().unwrap_or(0.0), 0.0);
        }
        fft.process(&mut buf);
        for (c, b) in cur.iter_mut().zip(&buf) {
            *c = (1.0 + 1000.0 * b.norm()).ln();
        }
        let (mut f, mut l) = (0.0f64, 0.0f64);
        for (k, (c, p)) in cur.iter().zip(&prev).enumerate() {
            let d = f64::from((c - p).max(0.0));
            f += d;
            if k < low_bins {
                l += d;
            }
        }
        full.push(if m == 0 { 0.0 } else { f });
        low.push(if m == 0 { 0.0 } else { l });
        std::mem::swap(&mut prev, &mut cur);
    }
    let fps = f64::from(sr) / hop as f64;
    Envelope { full: normalise(&full, fps), low: normalise(&low, fps), hop, n }
}

/// Subtract a local mean (about 0.4 s), half-wave rectify, divide by the deviation.
fn normalise(e: &[f64], fps: f64) -> Vec<f64> {
    let w = ((fps * 0.2) as usize).max(1);
    let mut prefix = Vec::with_capacity(e.len() + 1);
    prefix.push(0.0f64);
    let mut acc = 0.0;
    for v in e {
        acc += v;
        prefix.push(acc);
    }
    let out: Vec<f64> = (0..e.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(w), (i + w + 1).min(e.len()));
            let mean = (prefix.get(b).copied().unwrap_or(0.0) - prefix.get(a).copied().unwrap_or(0.0)) / (b - a).max(1) as f64;
            (e.get(i).copied().unwrap_or(0.0) - mean).max(0.0)
        })
        .collect();
    let len = out.len().max(1) as f64;
    let m = out.iter().sum::<f64>() / len;
    let sd = (out.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / len).sqrt();
    if sd > 1e-12 { out.iter().map(|v| v / sd).collect() } else { out }
}

fn autocorr(e: &[f64], lag: usize) -> f64 {
    e.iter().zip(e.iter().skip(lag)).map(|(a, b)| a * b).sum::<f64>()
}

/// The beat period in frames (fractional) inside the tempo range.
fn beat_period(e: &[f64], fps: f64, opts: &BeatOptions) -> Option<f64> {
    let (lo_bpm, hi_bpm) = sane_range(opts);
    let lag_min = ((60.0 * fps / hi_bpm).floor() as usize).max(1);
    let lag_max = ((60.0 * fps / lo_bpm).ceil() as usize).max(lag_min + 1);
    if e.len() < lag_max * 2 + 4 {
        return None;
    }
    let ac: Vec<f64> = (0..=lag_max * 2 + 2).map(|l| autocorr(e, l)).collect();
    let a = |l: usize| ac.get(l).copied().unwrap_or(0.0);
    // Prefer tempi near the middle of the range (log-Gaussian, one octave wide).
    let centre = (lo_bpm * hi_bpm).sqrt();
    let score = |l: usize| {
        let bpm = 60.0 * fps / l as f64;
        let w = (-0.5 * (bpm / centre).log2().powi(2)).exp();
        w * (a(l) + 0.5 * a(2 * l) + 0.25 * a(2 * l - 1) + 0.25 * a(2 * l + 1))
    };
    let mut best = lag_min;
    let mut best_s = f64::NEG_INFINITY;
    for l in lag_min..=lag_max {
        let bpm = 60.0 * fps / l as f64;
        if bpm < lo_bpm * 0.97 || bpm > hi_bpm * 1.03 {
            continue;
        }
        let s = score(l);
        if s > best_s {
            best_s = s;
            best = l;
        }
    }
    // Parabolic refinement on the raw autocorrelation.
    let (y0, y1, y2) = (a(best - 1), a(best), a(best + 1));
    let den = y0 - 2.0 * y1 + y2;
    let off = if den.abs() > 1e-12 { (0.5 * (y0 - y2) / den).clamp(-0.5, 0.5) } else { 0.0 };
    Some(best as f64 + off)
}

fn sane_range(opts: &BeatOptions) -> (f64, f64) {
    let lo = if opts.min_bpm.is_finite() { opts.min_bpm.clamp(20.0, 400.0) } else { 60.0 };
    let hi = if opts.max_bpm.is_finite() { opts.max_bpm.clamp(20.0, 400.0) } else { 200.0 };
    if hi > lo * 1.05 { (lo, hi) } else { (lo, lo * 1.05) }
}

/// Dynamic-programming beat tracking on the onset envelope: beat frames.
fn dp_beats(e: &[f64], period: f64) -> Vec<usize> {
    const TIGHTNESS: f64 = 400.0;
    let len = e.len();
    if len == 0 || period < 1.0 {
        return Vec::new();
    }
    let (lo, hi) = ((period * 0.5).round().max(1.0) as usize, (period * 2.0).round().max(2.0) as usize);
    let mut score = vec![0.0f64; len];
    let mut back: Vec<Option<usize>> = vec![None; len];
    for t in 0..len {
        let mut best: Option<(f64, usize)> = None;
        if t >= lo {
            let from = t.saturating_sub(hi);
            for tau in from..=t - lo {
                let d = (t - tau) as f64 / period;
                let s = score.get(tau).copied().unwrap_or(0.0) - TIGHTNESS * d.ln().powi(2);
                if best.is_none_or(|(b, _)| s > b) {
                    best = Some((s, tau));
                }
            }
        }
        let own = e.get(t).copied().unwrap_or(0.0);
        match best {
            // A chain only links when that pays: otherwise a new chain starts here.
            Some((s, tau)) if s > 0.0 => {
                if let Some(x) = score.get_mut(t) {
                    *x = own + s;
                }
                if let Some(x) = back.get_mut(t) {
                    *x = Some(tau);
                }
            }
            _ => {
                if let Some(x) = score.get_mut(t) {
                    *x = own;
                }
            }
        }
    }
    // End on the best-scoring frame within the last period.
    let tail = (period.ceil() as usize).min(len);
    let mut end = len - 1;
    let mut best = f64::NEG_INFINITY;
    for (t, s) in score.iter().enumerate().skip(len - tail) {
        if *s > best {
            best = *s;
            end = t;
        }
    }
    let mut beats = vec![end];
    let mut t = end;
    while let Some(Some(p)) = back.get(t) {
        beats.push(*p);
        t = *p;
    }
    beats.reverse();
    beats
}

/// Move a beat to the strongest energy rise near it (sample accuracy for sharp attacks).
fn refine(x: &[f32], lo: usize, hi: usize, sr: f32) -> Option<usize> {
    let w = ((sr * 0.0015) as usize).max(8);
    let step = (w / 4).max(1);
    let energy = |a: usize, b: usize| -> f64 { x.get(a..b.min(x.len())).map_or(0.0, |s| s.iter().map(|v| f64::from(*v) * f64::from(*v)).sum()) };
    let mut best: Option<(f64, usize)> = None;
    let mut j = lo.max(w);
    while j < hi.min(x.len()) {
        let rise = energy(j, j + w) - energy(j - w, j);
        if best.is_none_or(|(b, _)| rise > b) {
            best = Some((rise, j));
        }
        j += step;
    }
    // Fine pass around the coarse best.
    let (_, c) = best?;
    let mut fine: Option<(f64, usize)> = None;
    for j in c.saturating_sub(step).max(w)..(c + step).min(x.len()) {
        let rise = energy(j, j + w) - energy(j - w, j);
        if fine.is_none_or(|(b, _)| rise > b) {
            fine = Some((rise, j));
        }
    }
    let (rise, at) = fine?;
    // Only a real attack counts: the rise must be a clear share of the energy after it.
    (rise > 0.0 && rise > 0.3 * energy(at, at + w)).then_some(at)
}

/// Find beats and downbeats in mono audio `x` at `sample_rate`.
pub fn track_beats(x: &[f32], sample_rate: f32, opts: &BeatOptions) -> BeatTrack {
    let sr = sane_sr(sample_rate);
    let x = x.get(..x.len().min(MAX_BEAT_SAMPLES)).unwrap_or(&[]);
    let env = envelope(x, sr);
    let fps = f64::from(sr) / env.hop as f64;
    let Some(period) = beat_period(&env.full, fps, opts) else { return BeatTrack::default() };
    let frames = dp_beats(&env.full, period);
    // Trim beats outside the music (before the first and after the last clear onset).
    let strong: Vec<usize> = env.full.iter().enumerate().filter(|(_, v)| **v > 1.5).map(|(i, _)| i).collect();
    let (first, last) = (strong.first().copied().unwrap_or(0), strong.last().copied().unwrap_or(0));
    let margin = (period * 0.5) as usize;
    let frames: Vec<usize> = frames.into_iter().filter(|f| *f + margin >= first && *f <= last + margin).collect();
    let mut beats: Vec<usize> = Vec::with_capacity(frames.len());
    let mut accent: Vec<f64> = Vec::with_capacity(frames.len());
    for f in &frames {
        let lo = f * env.hop;
        let nominal = lo + env.n / 2;
        let at = refine(x, lo, lo + env.n + env.hop, sr).unwrap_or(nominal);
        if beats.last().is_none_or(|b| at > *b) {
            beats.push(at);
            let near = |e: &[f64]| (f.saturating_sub(1)..=f + 1).filter_map(|i| e.get(i)).copied().fold(0.0, f64::max);
            accent.push(near(&env.full) + 1.5 * near(&env.low));
        }
    }
    let bpm = match (beats.first(), beats.last()) {
        (Some(a), Some(b)) if beats.len() >= 2 && b > a => 60.0 * f64::from(sr) * (beats.len() - 1) as f64 / (b - a) as f64,
        _ => 0.0,
    };
    let bpb = opts.beats_per_bar.clamp(1, 64);
    let phase = (0..bpb.min(beats.len().max(1)))
        .map(|k| {
            let v: Vec<f64> = accent.iter().skip(k).step_by(bpb).copied().collect();
            (k, v.iter().sum::<f64>() / v.len().max(1) as f64)
        })
        .fold((0usize, f64::NEG_INFINITY), |acc, (k, m)| if m > acc.1 + 1e-9 { (k, m) } else { acc })
        .0;
    let downbeats = beats.iter().skip(phase).step_by(bpb).copied().collect();
    BeatTrack { bpm, beats, downbeats, first_downbeat: phase }
}

/// A click track for tests and previews: a short decaying burst at each position, `accent`
/// louder at every `accent_every`-th click (0 = none).
pub fn click_track(len: usize, positions: &[usize], sample_rate: f32, accent_every: usize) -> Vec<f32> {
    let sr = sane_sr(sample_rate);
    let n = ((sr * 0.02) as usize).max(1);
    let mut out = vec![0.0f32; len.min(MAX_BEAT_SAMPLES)];
    for (i, p) in positions.iter().enumerate() {
        let amp = if accent_every > 0 && i % accent_every == 0 { 0.9 } else { 0.4 };
        for k in 0..n {
            if let Some(o) = out.get_mut(p + k) {
                let t = k as f32 / sr;
                *o += amp * (-t * 300.0).exp() * (2.0 * std::f32::consts::PI * 1500.0 * t).sin();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Click positions for a tempo going linearly from `a` to `b` BPM over `beats` beats.
    fn ramp(a: f64, b: f64, beats: usize, start: f64) -> Vec<usize> {
        let mut t = start;
        let mut out = Vec::new();
        for i in 0..beats {
            out.push((t * f64::from(SR)).round() as usize);
            let bpm = a + (b - a) * i as f64 / (beats.max(2) - 1) as f64;
            t += 60.0 / bpm;
        }
        out
    }

    fn track(pos: &[usize], accent: usize, opts: &BeatOptions) -> BeatTrack {
        let len = pos.last().copied().unwrap_or(0) + SR as usize;
        track_beats(&click_track(len, pos, SR, accent), SR, opts)
    }

    #[test]
    fn steady_click_is_found_at_its_tempo() {
        let pos = ramp(120.0, 120.0, 64, 0.5);
        let t = track(&pos, 4, &BeatOptions::default());
        assert!((t.bpm - 120.0).abs() < 0.5, "bpm {}", t.bpm);
        assert!(t.beats.len() >= 60, "{} beats", t.beats.len());
        for b in &t.beats {
            let near = pos.iter().map(|p| (*p as i64 - *b as i64).abs()).min().unwrap_or(i64::MAX);
            assert!(near < 96, "beat {b} is {near} samples from a click");
        }
    }

    #[test]
    fn range_resolves_half_and_double_tempo() {
        let pos = ramp(150.0, 150.0, 64, 0.25);
        let slow = track(&pos, 0, &BeatOptions { min_bpm: 60.0, max_bpm: 90.0, beats_per_bar: 4 });
        assert!((slow.bpm - 75.0).abs() < 1.0, "half: {}", slow.bpm);
        let fast = track(&pos, 0, &BeatOptions { min_bpm: 140.0, max_bpm: 200.0, beats_per_bar: 4 });
        assert!((fast.bpm - 150.0).abs() < 1.0, "as played: {}", fast.bpm);
    }

    #[test]
    fn downbeat_is_on_the_accented_click() {
        // The accent falls on the third click: downbeats are clicks 2, 6, 10, ...
        let pos = ramp(110.0, 110.0, 66, 0.3);
        let mut sig = click_track(pos.last().copied().unwrap_or(0) + 48_000, &pos, SR, 0);
        let accented = click_track(sig.len(), &pos.iter().skip(2).step_by(4).copied().collect::<Vec<_>>(), SR, 1);
        for (s, a) in sig.iter_mut().zip(&accented) {
            *s += *a;
        }
        let t = track_beats(&sig, SR, &BeatOptions::default());
        let d0 = t.downbeats.first().copied().unwrap_or(0) as i64;
        let want = pos.iter().skip(2).step_by(4).map(|p| *p as i64).find(|p| (*p - d0).abs() < 2_000);
        assert!(want.is_some(), "first downbeat {d0} is not an accented click");
    }

    #[test]
    fn drifting_click_is_followed() {
        let pos = ramp(100.0, 110.0, 80, 0.4);
        let t = track(&pos, 4, &BeatOptions::default());
        assert!((t.bpm - 105.0).abs() < 1.5, "avg {}", t.bpm);
        let hits = t.beats.iter().filter(|b| pos.iter().any(|p| (*p as i64 - **b as i64).abs() < 48)).count();
        assert_eq!(hits, t.beats.len(), "every beat is on a click (within 1 ms)");
        assert!(hits + 1 >= pos.len(), "{hits} of {} clicks tracked", pos.len());
    }

    #[test]
    fn silence_and_garbage_do_not_panic() {
        assert_eq!(track_beats(&[], SR, &BeatOptions::default()).beats.len(), 0);
        let t = track_beats(&vec![0.0; 100_000], SR, &BeatOptions::default());
        assert!(t.bpm.is_finite());
        let nan = vec![f32::NAN; 50_000];
        let _ = track_beats(&downmix(&[nan]), f32::NAN, &BeatOptions { min_bpm: f64::NAN, max_bpm: -3.0, beats_per_bar: 0 });
    }
}
