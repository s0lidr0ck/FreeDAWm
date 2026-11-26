//! Live mode: the feasibility study's low-delay "splice" engine, for singing through Plumb.
//! The voice is read back at the corrected speed (a touch faster to go up, slower to go down);
//! when the read head has caught up with the input (going up) or fallen a period behind (going
//! down), it jumps by exactly one sung period, matched by correlation, with a short crossfade.
//! The delay so varies between about 1 ms and one sung period plus a little; the reported latency
//! is 1 ms plus half the longest period of the voice range (about 4 to 8 ms). Formants move a
//! little with the correction (about 6 % for a whole semitone).
//!
//! Where the voice is let through (unvoiced, unsure) the read head goes at the input's speed and,
//! once the correction has faded, steps back to the reported delay so the voice is in time again.
//!
//! Nothing allocates after [`Live::new`].

use super::detect::cents_to_hz;
use super::front::Front;
use std::f64::consts::PI;

/// The read head never comes closer to the input than this (samples).
const MARGIN: f64 = 4.0;

pub(super) struct Live {
    sr: f64,
    /// The longest period of the voice range (samples).
    tmax: f64,
    latency: usize,
    /// The read head (input samples, absolute).
    rp: f64,
    /// A crossfade from the old read position: (old position, step, length).
    fade: Option<(f64, usize, usize)>,
    /// Output samples emitted since the last reset.
    emitted: u64,
    /// Largest delay seen since the last reset (samples), for tests and the display.
    d_max: f64,
    d_sum: f64,
    d_n: f64,
}

impl Live {
    /// An engine for a voice whose lowest note is `floor_hz`.
    pub(super) fn new(sr: f64, floor_hz: f64) -> Live {
        let tmax = sr / floor_hz.max(20.0);
        let latency = (sr * 0.001 + tmax / 2.0).round() as usize;
        let mut l = Live { sr, tmax, latency, rp: 0.0, fade: None, emitted: 0, d_max: 0.0, d_sum: 0.0, d_n: 0.0 };
        l.reset();
        l
    }

    pub(super) fn latency(&self) -> usize {
        self.latency
    }

    /// Input history it reads back at most (samples).
    pub(super) fn history(&self) -> usize {
        (self.tmax * 4.0 + self.sr * 0.02) as usize
    }

    pub(super) fn reset(&mut self) {
        self.rp = -(self.latency as f64);
        self.fade = None;
        self.emitted = 0;
        self.d_max = 0.0;
        self.d_sum = 0.0;
        self.d_n = 0.0;
    }

    /// (mean, largest) delay since the last reset (samples).
    pub(super) fn delay_stats(&self) -> (f64, f64) {
        (self.d_sum / self.d_n.max(1.0), self.d_max)
    }

    /// Channel `c` at fractional input position `pos` (Catmull-Rom; silence before the start).
    fn read(f: &Front, c: usize, pos: f64) -> f64 {
        let i = pos.floor();
        let u = pos - i;
        let i = i as i64;
        let g = |k: i64| -> f64 { if k < 0 { 0.0 } else { f64::from(f.x(c, k as u64)) } };
        let (y0, y1, y2, y3) = (g(i - 1), g(i), g(i + 1), g(i + 2));
        y1 + 0.5 * u * (y2 - y0 + u * (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3 + u * (3.0 * (y1 - y2) + y3 - y0)))
    }

    /// The period near `t` (samples) that best matches the last stretch before `pos` when the
    /// read head jumps by it (`dir` -1 back, +1 forward), within an eighth of a period.
    fn period(&self, f: &Front, pos: f64, t: f64, dir: i64) -> f64 {
        let w = t.min(self.sr * 0.01) as i64;
        let r = (t / 8.0) as i64;
        let p0 = pos.round() as i64;
        let newest = f.n_in as i64 - 1;
        let mut best = (t, f64::NEG_INFINITY);
        for dt in -r..=r {
            let tt = t.round() as i64 + dt;
            let q = p0 + dir * tt;
            if q - w < 0 || p0 - w < 0 || q > newest || p0 > newest {
                continue;
            }
            let (mut xy, mut yy) = (0.0, 0.0);
            let mut k = 0;
            while k < w {
                let a = f64::from(f.mono_at((p0 - k) as u64));
                let b = f64::from(f.mono_at((q - k) as u64));
                xy += a * b;
                yy += b * b;
                k += 2;
            }
            let sc = xy / yy.sqrt().max(1e-9);
            if sc > best.1 {
                best = (tt as f64, sc);
            }
        }
        best.0
    }

    /// Writes the output for the input samples just taken (`take`, the last ones) into `io` from
    /// `pos`, mixed with the input delayed by the reported latency by `mix`.
    pub(super) fn emit(&mut self, f: &Front, io: &mut [Vec<f32>], pos: usize, take: usize, mut mix: impl FnMut() -> f32) {
        let known = f.known_until();
        let lat = self.latency as f64;
        for i in 0..take {
            let o = self.emitted;
            self.emitted += 1;
            let m = mix();
            let (v, cents, corr) = f.at(self.rp.min(known));
            let r = 2f64.powf(corr.clamp(-1200.0, 1200.0) / 1200.0);
            for c in 0..f.channels {
                let a = Self::read(f, c, self.rp);
                let y = match self.fade {
                    Some((old, k, len)) => {
                        let g = (k as f64 + 0.5) / len as f64;
                        let g = 0.5 - 0.5 * (PI * g).cos();
                        g * a + (1.0 - g) * Self::read(f, c, old)
                    }
                    None => a,
                };
                let dry = o.checked_sub(self.latency as u64).map_or(0.0, |s| f.x(c, s));
                let moved = if y.is_finite() { y as f32 } else { dry };
                let out = if m <= 0.0 { dry } else { dry + m * (moved - dry) };
                if let Some(slot) = io.get_mut(c).and_then(|b| b.get_mut(pos + i)) {
                    *slot = out;
                }
            }
            let d = o as f64 - self.rp;
            self.d_sum += d;
            self.d_n += 1.0;
            self.d_max = self.d_max.max(d);
            self.fade = match self.fade {
                Some((old, k, len)) if k + 1 < len => Some((old + r, k + 1, len)),
                _ => None,
            };
            self.rp += r;
            if self.fade.is_some() {
                continue;
            }
            if v && cents.is_finite() && cents > 0.0 {
                let t = self.sr / cents_to_hz(cents);
                let len = (t * 0.5).clamp(16.0, self.sr * 0.005) as usize;
                let need_back = r > 1.0 && d - 1.0 < MARGIN + len as f64 * (r - 1.0) + 2.0;
                let need_fwd = r < 1.0 && d > t + MARGIN + len as f64 * 2.0 && d < self.tmax * 2.5;
                if need_back || need_fwd {
                    let dir = if need_back { -1 } else { 1 };
                    let tt = self.period(f, self.rp, t, dir);
                    let new = self.rp + dir as f64 * tt;
                    if (o as f64 + 1.0) - new >= MARGIN {
                        self.fade = Some((self.rp, 0, len));
                        self.rp = new;
                    }
                }
            } else if !v && corr == 0.0 && ((o as f64 + 1.0 - self.rp) - lat).abs() > 1.0 {
                // Let through: back to the reported delay (a short crossfade on breath or noise).
                let len = (self.sr * 0.005) as usize;
                self.fade = Some((self.rp, 0, len.max(16)));
                self.rp = o as f64 + 1.0 - lat;
            }
            // A runaway (it cannot, but a hostile input must not leave it reading the far past).
            let d = o as f64 + 1.0 - self.rp;
            if !(d >= 1.0 && d <= self.tmax * 3.0 + self.sr * 0.02) {
                self.rp = o as f64 + 1.0 - lat;
                self.fade = None;
            }
        }
    }
}
