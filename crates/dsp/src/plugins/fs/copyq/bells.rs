//! A CopyQ curve as a handful of bells, for handing the match to an ordinary equaliser (Send to
//! CleanQ): the largest remaining error gets a bell, its width read off the curve, a few widths
//! and gains tried around that, and its response taken out; then every bell's gain is touched
//! up against what the others left. Off the audio thread: it allocates.

use super::profile::{Curve, GRID_N, PER_OCTAVE, grid_hz};

/// One bell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bell {
    pub hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// The narrowest and widest bell fitted.
pub const MIN_Q: f32 = 0.3;
pub const MAX_Q: f32 = 8.0;
/// An error under this is left alone, dB.
pub const DONE_DB: f32 = 0.35;

/// The textbook peaking bell's response at `hz`, dB (the analogue prototype: the same at every
/// sample rate).
pub fn bell_db(bell: &Bell, hz: f32) -> f32 {
    if !(bell.hz > 0.0 && bell.q > 0.0 && hz > 0.0) {
        return 0.0;
    }
    let a = 10f32.powf(bell.gain_db / 40.0);
    let x = hz / bell.hz;
    let d = (1.0 - x * x).powi(2);
    let num = d + (a * x / bell.q).powi(2);
    let den = d + (x / (a * bell.q)).powi(2);
    if den > 1.0e-20 && num > 1.0e-20 { 10.0 * (num / den).log10() } else { 0.0 }
}

/// What the bells add up to on the grid, dB.
pub fn response(bells: &[Bell]) -> Curve {
    let mut out = [0.0f32; GRID_N];
    for (i, o) in out.iter_mut().enumerate() {
        let hz = grid_hz(i);
        *o = bells.iter().map(|b| bell_db(b, hz)).sum();
    }
    out
}

fn rms(c: &Curve, weight: &Curve) -> f32 {
    let (mut s, mut w) = (0.0f32, 0.0f32);
    for (v, k) in c.iter().zip(weight) {
        s += v * v * k;
        w += k;
    }
    if w > 0.0 { (s / w).sqrt() } else { 0.0 }
}

/// At most `max` bells (capped at 24) that follow `curve` (dB on the grid). Non-finite points
/// count as 0. Bells are returned from low to high.
pub fn fit_bells(curve: &Curve, max: usize) -> Vec<Bell> {
    let mut want = [0.0f32; GRID_N];
    for (w, v) in want.iter_mut().zip(curve) {
        *w = if v.is_finite() { v.clamp(-36.0, 36.0) } else { 0.0 };
    }
    let even = [1.0f32; GRID_N];
    let mut bells: Vec<Bell> = Vec::new();
    let mut residual = want;
    for _ in 0..max.min(24) {
        let Some((at, peak)) =
            residual.iter().copied().enumerate().max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap_or(std::cmp::Ordering::Equal))
        else {
            break;
        };
        if peak.abs() < DONE_DB {
            break;
        }
        // The width: how far the error stays over half its peak, each way.
        let over = |i: usize| residual.get(i).is_some_and(|v| v * peak.signum() > peak.abs() * 0.5);
        let mut lo = at;
        while lo > 0 && over(lo - 1) {
            lo -= 1;
        }
        let mut hi = at;
        while over(hi + 1) {
            hi += 1;
        }
        let octaves = ((hi - lo + 1) as f32 / PER_OCTAVE as f32).clamp(0.12, 5.0);
        let ratio = 2f32.powf(octaves);
        let q0 = ratio.sqrt() / (ratio - 1.0);
        let centre = grid_hz((lo + hi) / 2);
        let mut best: Option<(f32, Bell)> = None;
        for hz in [grid_hz(at), centre] {
            for qs in [0.5f32, 0.7, 1.0, 1.4, 2.0] {
                for gs in [0.8f32, 1.0, 1.15] {
                    let b = Bell { hz, gain_db: peak * gs, q: (q0 * qs).clamp(MIN_Q, MAX_Q) };
                    let mut left = residual;
                    for (i, v) in left.iter_mut().enumerate() {
                        *v -= bell_db(&b, grid_hz(i));
                    }
                    let e = rms(&left, &even);
                    if best.as_ref().is_none_or(|(be, _)| e < *be) {
                        best = Some((e, b));
                    }
                }
            }
        }
        let Some((_, bell)) = best else { break };
        for (i, v) in residual.iter_mut().enumerate() {
            *v -= bell_db(&bell, grid_hz(i));
        }
        bells.push(bell);
    }
    // Touch up each gain against what the others leave.
    for _ in 0..3 {
        for j in 0..bells.len() {
            let total = response(&bells);
            let Some(b) = bells.get(j).copied() else { continue };
            let unit = Bell { gain_db: 1.0, ..b };
            let (mut num, mut den) = (0.0f32, 0.0f32);
            for (i, (w, t)) in want.iter().zip(&total).enumerate() {
                let shape = bell_db(&unit, grid_hz(i));
                num += (w - t) * shape;
                den += shape * shape;
            }
            if den > 1.0e-6
                && let Some(slot) = bells.get_mut(j)
            {
                slot.gain_db = (slot.gain_db + num / den).clamp(-24.0, 24.0);
            }
        }
    }
    bells.retain(|b| b.gain_db.abs() >= 0.1);
    bells.sort_by(|a, b| a.hz.partial_cmp(&b.hz).unwrap_or(std::cmp::Ordering::Equal));
    bells
}

/// How far the bells are from the curve, dB RMS over the grid.
pub fn fit_error(curve: &Curve, bells: &[Bell]) -> f32 {
    let r = response(bells);
    let mut d = [0.0f32; GRID_N];
    for ((o, c), v) in d.iter_mut().zip(curve).zip(&r) {
        *o = if c.is_finite() { c - v } else { 0.0 };
    }
    rms(&d, &[1.0; GRID_N])
}
