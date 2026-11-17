//! Second-order IIR filters designed from the RBJ "Audio EQ Cookbook" formulas (bells and shelves
//! matched to their analog prototypes up to Nyquist instead, see [`matched`]), plus first-order
//! low/high-pass sections (bilinear transform) for 6 dB/oct slopes.

use std::f64::consts::PI;

use crate::util::{flush, sane_sr};

/// Which bell or shelf [`matched`] designs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matched {
    Peak,
    LowShelf,
    HighShelf,
}

/// A bell or shelf designed to follow its analog prototype all the way to Nyquist (M. Vicanek,
/// "Matched Second Order Digital Filters", 2016): poles matched to the analog ones (impulse
/// invariance), zeros chosen so the magnitude equals the analog one at DC, at the centre `w0`
/// (rad/sample) and at Nyquist. The bilinear transform (the RBJ cookbook) instead pins the gain
/// at Nyquist to the gain at analog infinity, so a bell or shelf high up at 44.1 or 48 kHz
/// narrows and loses gain on its top side ("cramping").
///
/// The analog prototypes are the cookbook's, with `a` = 10^(dB/40): bell
/// `(s² + s·a/q + 1)/(s² + s/(a·q) + 1)`, low shelf `a·(s² + s·√a/q + a)/(a·s² + s·√a/q + 1)`,
/// high shelf `a·(a·s² + s·√a/q + 1)/(s² + s·√a/q + a)`, so a design low down matches the
/// cookbook's. `(b0, b1, b2, a1, a2)` with `a0 = 1`; `None` where the matched poles would sit at
/// or past Nyquist (a high shelf near the top boosting): use the bilinear design there.
pub fn matched(kind: Matched, w0: f64, q: f64, a: f64) -> Option<[f64; 5]> {
    if !(w0.is_finite() && q.is_finite() && a.is_finite()) || w0 <= 0.0 || w0 >= PI || q <= 0.0 || a <= 0.0 {
        return None;
    }
    let ra = a.sqrt();
    // The analog poles (natural frequency, relative to w0, and damping) and |H|² at Ω (relative).
    let (wn, zeta) = match kind {
        Matched::Peak => (1.0, 1.0 / (2.0 * a * q)),
        Matched::LowShelf => (1.0 / ra, 1.0 / (2.0 * q)),
        Matched::HighShelf => (ra, 1.0 / (2.0 * q)),
    };
    let mag2 = |w: f64| {
        let o = w / w0;
        let o2 = o * o;
        let x = a * o2 / (q * q);
        match kind {
            Matched::Peak => {
                let d = (1.0 - o2) * (1.0 - o2);
                (d + (a * o / q).powi(2)) / (d + (o / (a * q)).powi(2))
            }
            Matched::LowShelf => a * a * ((a - o2).powi(2) + x) / ((1.0 - a * o2).powi(2) + x),
            Matched::HighShelf => a * a * ((1.0 - a * o2).powi(2) + x) / ((a - o2).powi(2) + x),
        }
    };
    let wp = wn * w0;
    if wp >= 0.95 * PI {
        return None;
    }
    let r = (-zeta * wp).exp();
    let a1 = if zeta < 1.0 { -2.0 * r * (wp * (1.0 - zeta * zeta).sqrt()).cos() } else { -2.0 * r * (wp * (zeta * zeta - 1.0).sqrt()).cosh() };
    let a2 = r * r;
    // |B(e^jw)|² = B0·φ0 + B1·φ1 + B2·φ2 with φ1 = sin²(w/2), φ0 = 1 - φ1, φ2 = 4·φ0·φ1, and the same
    // for the poles (A0 = (1 + a1 + a2)², A1 = (1 - a1 + a2)², A2 = -4·a2).
    let (big_a0, big_a1, big_a2) = ((1.0 + a1 + a2).powi(2), (1.0 - a1 + a2).powi(2), -4.0 * a2);
    let p1 = (w0 / 2.0).sin().powi(2);
    let p0 = 1.0 - p1;
    let p2 = 4.0 * p0 * p1;
    let b0_ = big_a0 * mag2(0.0);
    let b1_ = big_a1 * mag2(PI);
    let b2_ = (mag2(w0) * (big_a0 * p0 + big_a1 * p1 + big_a2 * p2) - b0_ * p0 - b1_ * p1) / p2;
    let (s0, s1) = (b0_.max(0.0).sqrt(), b1_.max(0.0).sqrt());
    let w = 0.5 * (s0 + s1);
    let b0 = 0.5 * (w + (w * w + b2_).max(0.0).sqrt());
    if b0.abs() < 1e-12 {
        return None;
    }
    let out = [b0, 0.5 * (s0 - s1), -b2_ / (4.0 * b0), a1, a2];
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// Filter response shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FilterType {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peak,
    LowShelf,
    HighShelf,
    AllPass,
    /// First-order (6 dB/oct) low-pass; `q` and `gain_db` are ignored.
    LowPass1,
    /// First-order (6 dB/oct) high-pass; `q` and `gain_db` are ignored.
    HighPass1,
}

/// Normalized biquad coefficients (`a0 = 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Default for Coeffs {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Coeffs {
    /// Passes the signal unchanged.
    pub const IDENTITY: Coeffs = Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// Designs a filter. All inputs are sanitized: frequency is clamped to 1 Hz .. 0.49·sr,
    /// Q to 0.025 .. 100 and gain to ±48 dB.
    pub fn design(kind: FilterType, freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> Coeffs {
        let sr = f64::from(sane_sr(sample_rate));
        let f = if freq.is_finite() { f64::from(freq).clamp(1.0, sr * 0.49) } else { 1000.0 };
        let q = if q.is_finite() { f64::from(q).clamp(0.025, 100.0) } else { std::f64::consts::FRAC_1_SQRT_2 };
        let g = if gain_db.is_finite() { f64::from(gain_db).clamp(-48.0, 48.0) } else { 0.0 };
        let w0 = 2.0 * PI * f / sr;
        let (sw, cw) = w0.sin_cos();
        let alpha = sw / (2.0 * q);
        let a = 10f64.powf(g / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterType::LowPass => ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::HighPass => ((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::Notch => (1.0, -2.0 * cw, 1.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::AllPass => (1.0 - alpha, -2.0 * cw, 1.0 + alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::Peak | FilterType::LowShelf | FilterType::HighShelf
                if let Some([b0, b1, b2, a1, a2]) = matched(
                    match kind {
                        FilterType::Peak => Matched::Peak,
                        FilterType::LowShelf => Matched::LowShelf,
                        _ => Matched::HighShelf,
                    },
                    w0,
                    q,
                    a,
                ) =>
            {
                (b0, b1, b2, 1.0, a1, a2)
            }
            FilterType::Peak => (1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a),
            FilterType::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cw + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                    a * ((a + 1.0) - (a - 1.0) * cw - s),
                    (a + 1.0) + (a - 1.0) * cw + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                    (a + 1.0) + (a - 1.0) * cw - s,
                )
            }
            FilterType::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cw + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                    a * ((a + 1.0) + (a - 1.0) * cw - s),
                    (a + 1.0) - (a - 1.0) * cw + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cw),
                    (a + 1.0) - (a - 1.0) * cw - s,
                )
            }
            FilterType::LowPass1 | FilterType::HighPass1 => {
                // Bilinear transform of 1/(s+1) with prewarped cutoff.
                let k = (w0 / 2.0).tan();
                let norm = 1.0 / (1.0 + k);
                let a1 = (k - 1.0) * norm;
                return if kind == FilterType::LowPass1 {
                    Coeffs { b0: (k * norm) as f32, b1: (k * norm) as f32, b2: 0.0, a1: a1 as f32, a2: 0.0 }
                } else {
                    Coeffs { b0: norm as f32, b1: -norm as f32, b2: 0.0, a1: a1 as f32, a2: 0.0 }
                };
            }
        };
        let c = Coeffs { b0: (b0 / a0) as f32, b1: (b1 / a0) as f32, b2: (b2 / a0) as f32, a1: (a1 / a0) as f32, a2: (a2 / a0) as f32 };
        if [c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()) { c } else { Coeffs::IDENTITY }
    }

    /// Magnitude response in dB at `freq` Hz.
    pub fn magnitude_db(&self, freq: f32, sample_rate: f32) -> f32 {
        let sr = f64::from(sane_sr(sample_rate));
        let f = if freq.is_finite() { f64::from(freq).clamp(0.0, sr * 0.5) } else { 0.0 };
        let w = 2.0 * PI * f / sr;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let (b0, b1, b2, a1, a2) = (f64::from(self.b0), f64::from(self.b1), f64::from(self.b2), f64::from(self.a1), f64::from(self.a2));
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = -(b1 * s1 + b2 * s2);
        let dr = 1.0 + a1 * c1 + a2 * c2;
        let di = -(a1 * s1 + a2 * s2);
        let num = nr * nr + ni * ni;
        let den = (dr * dr + di * di).max(1e-30);
        let db = 10.0 * (num / den).max(1e-30).log10();
        db.max(-300.0) as f32
    }
}

/// A biquad section with its own state (transposed direct form II).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Biquad {
    pub coeffs: Coeffs,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn new(coeffs: Coeffs) -> Self {
        Self { coeffs, z1: 0.0, z2: 0.0 }
    }

    /// Designs a new filter (see [`Coeffs::design`]).
    pub fn design(kind: FilterType, freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> Self {
        Self::new(Coeffs::design(kind, freq, q, gain_db, sample_rate))
    }

    /// Re-designs the coefficients, keeping the state (for smooth sweeps).
    pub fn set(&mut self, kind: FilterType, freq: f32, q: f32, gain_db: f32, sample_rate: f32) {
        self.coeffs = Coeffs::design(kind, freq, q, gain_db, sample_rate);
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let c = &self.coeffs;
        let y = c.b0 * x + self.z1;
        self.z1 = flush(c.b1 * x - c.a1 * y + self.z2);
        self.z2 = flush(c.b2 * x - c.a2 * y);
        if y.is_finite() {
            y
        } else {
            self.reset();
            0.0
        }
    }

    pub fn process_block(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s = self.process_sample(*s);
        }
    }

    /// Magnitude response in dB at `freq` Hz, for drawing curves.
    pub fn magnitude_db(&self, freq: f32, sample_rate: f32) -> f32 {
        self.coeffs.magnitude_db(freq, sample_rate)
    }
}

/// Butterworth Q values for cascaded sections making an `order`-th order filter (2, 4).
pub(crate) fn butterworth_qs(order: usize) -> &'static [f32] {
    match order {
        4 => &[0.541_196_1, 1.306_563],
        _ => &[std::f32::consts::FRAC_1_SQRT_2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_gain_at_center() {
        let c = Coeffs::design(FilterType::Peak, 1000.0, 1.0, 6.0, 48_000.0);
        assert!((c.magnitude_db(1000.0, 48_000.0) - 6.0).abs() < 0.01);
        assert!(c.magnitude_db(20.0, 48_000.0).abs() < 0.1);
    }

    #[test]
    fn lowpass_minus3_at_cutoff() {
        let c = Coeffs::design(FilterType::LowPass, 1000.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, 48_000.0);
        assert!((c.magnitude_db(1000.0, 48_000.0) + 3.01).abs() < 0.05);
        let c1 = Coeffs::design(FilterType::HighPass1, 1000.0, 0.0, 0.0, 48_000.0);
        assert!((c1.magnitude_db(1000.0, 48_000.0) + 3.01).abs() < 0.05);
    }

    /// The analog prototype's level in dB at `f` (the cookbook's bell and shelves, `a` as there).
    fn analog_db(kind: FilterType, f0: f64, q: f64, db: f64, f: f64) -> f64 {
        let a = 10f64.powf(db / 40.0);
        let o = f / f0;
        let (o2, x) = (o * o, a * o * o / (q * q));
        let m2 = match kind {
            FilterType::Peak => ((1.0 - o2).powi(2) + (a * o / q).powi(2)) / ((1.0 - o2).powi(2) + (o / (a * q)).powi(2)),
            FilterType::LowShelf => a * a * ((a - o2).powi(2) + x) / ((1.0 - a * o2).powi(2) + x),
            _ => a * a * ((1.0 - a * o2).powi(2) + x) / ((a - o2).powi(2) + x),
        };
        10.0 * m2.log10()
    }

    /// The cookbook's (bilinear) design, as it was before the matched one.
    fn rbj(kind: FilterType, f0: f64, q: f64, db: f64, sr: f64) -> Coeffs {
        let w0 = 2.0 * PI * f0 / sr;
        let (sw, cw) = w0.sin_cos();
        let (alpha, a) = (sw / (2.0 * q), 10f64.powf(db / 40.0));
        let s = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterType::Peak => (1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a),
            FilterType::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cw + s),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                a * ((a + 1.0) - (a - 1.0) * cw - s),
                (a + 1.0) + (a - 1.0) * cw + s,
                -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                (a + 1.0) + (a - 1.0) * cw - s,
            ),
            _ => (
                a * ((a + 1.0) + (a - 1.0) * cw + s),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                a * ((a + 1.0) + (a - 1.0) * cw - s),
                (a + 1.0) - (a - 1.0) * cw + s,
                2.0 * ((a - 1.0) - (a + 1.0) * cw),
                (a + 1.0) - (a - 1.0) * cw - s,
            ),
        };
        let n = |x: f64| (x / a0) as f32;
        Coeffs { b0: n(b0), b1: n(b1), b2: n(b2), a1: n(a1), a2: n(a2) }
    }

    /// Built-in effects parity, bug 16: bells and shelves near the top of the range at 44.1 and
    /// 48 kHz narrowed and lost gain (the bilinear transform's cramping). The matched design keeps
    /// to the analog shape, at most half as far from it as the bilinear one and under 2 dB, with
    /// the centre exact.
    #[test]
    fn bells_and_shelves_follow_the_analog_shape_up_high() {
        for sr in [44_100.0f32, 48_000.0] {
            for (kind, f0, q, db) in [
                (FilterType::Peak, 10_000.0, 0.7, 12.0),
                (FilterType::Peak, 14_000.0, 2.0, -9.0),
                (FilterType::Peak, 16_000.0, 1.0, 6.0),
                (FilterType::Peak, 12_000.0, 8.0, 12.0),
                (FilterType::LowShelf, 8_000.0, 0.7, 6.0),
                (FilterType::HighShelf, 6_000.0, 0.7, -8.0),
                (FilterType::HighShelf, 4_000.0, 0.7, 6.0),
            ] {
                let c = Coeffs::design(kind, f0 as f32, q as f32, db as f32, sr);
                let old = rbj(kind, f0, q, db, f64::from(sr));
                let (mut worst, mut was) = (0.0f64, 0.0f64);
                let mut f = 20.0;
                while f < 20_000.0 {
                    let want = analog_db(kind, f0, q, db, f);
                    worst = worst.max((f64::from(c.magnitude_db(f as f32, sr)) - want).abs());
                    was = was.max((f64::from(old.magnitude_db(f as f32, sr)) - want).abs());
                    f *= 1.05;
                }
                let what = format!("{kind:?} {f0} Hz q {q} {db} dB at {sr}");
                assert!(worst < 2.0 && worst <= 0.5 * was, "{what}: off by {worst:.2} dB (bilinear {was:.2})");
                assert!((f64::from(c.magnitude_db(f0 as f32, sr)) - analog_db(kind, f0, q, db, f0)).abs() < 0.05, "{what}: at its centre");
            }
        }
        // Low down the matched design and the cookbook's agree.
        let c = Coeffs::design(FilterType::Peak, 200.0, 1.0, 6.0, 48_000.0);
        for f in [50.0, 150.0, 200.0, 300.0, 1000.0] {
            assert!((f64::from(c.magnitude_db(f, 48_000.0)) - analog_db(FilterType::Peak, 200.0, 1.0, 6.0, f64::from(f))).abs() < 0.05);
        }
        // 0 dB is the identity.
        let c = Coeffs::design(FilterType::Peak, 12_000.0, 1.0, 0.0, 48_000.0);
        assert!((c.b0 - 1.0).abs() < 1e-5 && (c.b1 - c.a1).abs() < 1e-5 && (c.b2 - c.a2).abs() < 1e-5);
    }

    #[test]
    fn hostile_inputs() {
        let c = Coeffs::design(FilterType::Peak, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0);
        assert!(c.b0.is_finite());
        let mut b = Biquad::new(c);
        assert!(b.process_sample(f32::NAN) == 0.0);
    }
}
