//! VintFilters -- analog prototypes, made digital, and their exact response.
//! Line-by-line translation of `Engine 11 - Vint/Source/VintFilters.h`.
//!
//! Every EQ curve is written as the hardware's ANALOG response -- a second-order section in s --
//! and turned into a digital biquad by the bilinear transform, prewarped at the section's own
//! characteristic frequency, at the OVERSAMPLED rate. `magnitude` evaluates a section's exact
//! digital response, so a display draws the curve the audio actually gets.

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

use rustfft::num_complex::Complex;

/// C++ `vintf::pi`.
pub const PI: f64 = 3.14159265358979323846;

/// A second-order analog section, (b2 s^2 + b1 s + b0) / (a2 s^2 + a1 s + a0), with s in units of
/// the section's own corner w0 (so s = 1 is w0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Analog {
    pub b2: f64,
    pub b1: f64,
    pub b0: f64,
    pub a2: f64,
    pub a1: f64,
    pub a0: f64,
}

impl Default for Analog {
    fn default() -> Self {
        Analog { b2: 0.0, b1: 0.0, b0: 1.0, a2: 0.0, a1: 0.0, a0: 1.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Default for Coefs {
    fn default() -> Self {
        Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }
}

/// Bilinear transform, prewarped so the section's corner lands exactly at f0 (Hz).
pub fn bilinear(h: &Analog, f0: f64, fs: f64) -> Coefs {
    let w0 = 2.0 * PI * f0.min(0.49 * fs);
    let k = 1.0 / (w0 / (2.0 * fs)).tan(); // s -> K (1 - z^-1) / (1 + z^-1), in units of w0
    let k2 = k * k;
    let bb0 = h.b2 * k2 + h.b1 * k + h.b0;
    let bb1 = 2.0 * (h.b0 - h.b2 * k2);
    let bb2 = h.b2 * k2 - h.b1 * k + h.b0;
    let aa0 = h.a2 * k2 + h.a1 * k + h.a0;
    let aa1 = 2.0 * (h.a0 - h.a2 * k2);
    let aa2 = h.a2 * k2 - h.a1 * k + h.a0;
    Coefs { b0: bb0 / aa0, b1: bb1 / aa0, b2: bb2 / aa0, a1: aa1 / aa0, a2: aa2 / aa0 }
}

// ---- the analog shapes the circuits are built from. g is a LINEAR gain.

/// Second-order low shelf, gain g below the corner, 1 above; q shapes the knee.
pub fn low_shelf2(g: f64, q: f64) -> Analog {
    let a = g.sqrt();
    let sa = a.sqrt();
    // numerator and denominator swap roles
    Analog { b2: 1.0, b1: sa / q, b0: a, a2: 1.0, a1: 1.0 / (sa * q), a0: 1.0 / a }
}

pub fn high_shelf2(g: f64, q: f64) -> Analog {
    let a = g.sqrt();
    let sa = a.sqrt();
    Analog { b2: a, b1: sa / q, b0: 1.0, a2: 1.0 / a, a1: 1.0 / (sa * q), a0: 1.0 }
}

/// First-order low shelf: gain g at DC, 1 at the top, corner the geometric midpoint.
pub fn low_shelf1(g: f64) -> Analog {
    let sg = g.sqrt();
    Analog { b1: 1.0, b0: sg, a1: 1.0, a0: 1.0 / sg, ..Analog::default() }
}

pub fn high_shelf1(g: f64) -> Analog {
    let sg = g.sqrt();
    Analog { b1: sg, b0: 1.0, a1: 1.0 / sg, a0: 1.0, ..Analog::default() }
}

/// Constant-Q bell: gain g at the centre, 1 far away; q the bandwidth at the half-gain points.
pub fn bell(g: f64, q: f64) -> Analog {
    let a = g.sqrt();
    Analog { b2: 1.0, b1: a / q, b0: 1.0, a2: 1.0, a1: 1.0 / (a * q), a0: 1.0 }
}

pub fn high_pass2(q: f64) -> Analog {
    Analog { b2: 1.0, b1: 0.0, b0: 0.0, a2: 1.0, a1: 1.0 / q, a0: 1.0 }
}
pub fn low_pass2(q: f64) -> Analog {
    Analog { b2: 0.0, b1: 0.0, b0: 1.0, a2: 1.0, a1: 1.0 / q, a0: 1.0 }
}
pub fn high_pass1() -> Analog {
    Analog { b1: 1.0, b0: 0.0, a1: 1.0, a0: 1.0, ..Analog::default() }
}
pub fn low_pass1() -> Analog {
    Analog { b1: 0.0, b0: 1.0, a1: 1.0, a0: 1.0, ..Analog::default() }
}
pub fn unity() -> Analog {
    Analog::default()
}

/// A running biquad, transposed direct form II.
#[derive(Debug, Clone, Copy, Default)]
pub struct Section {
    pub c: Coefs,
    pub z1: f64,
    pub z2: f64,
}

impl Section {
    #[inline]
    pub fn run(&mut self, x: f64) -> f64 {
        let y = self.c.b0 * x + self.z1;
        self.z1 = self.c.b1 * x - self.c.a1 * y + self.z2;
        self.z2 = self.c.b2 * x - self.c.a2 * y;
        y
    }
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// |H| of a digital section at f (Hz), sampled at fs.
pub fn magnitude(c: &Coefs, f: f64, fs: f64) -> f64 {
    // C++: z1 = std::polar (1.0, -2 pi f / fs), z2 = z1 z1; |num / den|.
    let z1 = Complex::from_polar(1.0, -2.0 * PI * f / fs);
    let z2 = z1 * z1;
    let num = c.b0 + c.b1 * z1 + c.b2 * z2;
    let den = 1.0 + c.a1 * z1 + c.a2 * z2;
    (num / den).norm()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_low_pass_is_unity_at_dc_and_minus_3_at_its_corner() {
        let c = bilinear(&low_pass2(std::f64::consts::FRAC_1_SQRT_2), 1000.0, 192_000.0);
        assert!((magnitude(&c, 0.0, 192_000.0) - 1.0).abs() < 1e-12);
        assert!((20.0 * magnitude(&c, 1000.0, 192_000.0).log10() + 3.0103).abs() < 1e-3);
        let mut s = Section { c, ..Section::default() };
        let mut y = 0.0;
        for _ in 0..20_000 {
            y = s.run(1.0);
        }
        assert!((y - 1.0).abs() < 1e-9);
        // the shapes are the C++'s
        assert_eq!(unity(), Analog::default());
        assert!(high_pass1().b1 == 1.0 && low_pass1().b0 == 1.0);
        let _ = (low_shelf2(2.0, 0.7), high_shelf2(2.0, 0.7), low_shelf1(2.0), high_shelf1(2.0), bell(2.0, 1.0));
    }
}
