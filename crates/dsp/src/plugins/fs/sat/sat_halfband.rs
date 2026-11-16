//! SatHalfband -- polyphase half-band IIR, for 2x / 4x / 8x oversampling.
//! Line-by-line translation of `Engine 8 - Sat/Source/SatHalfband.h`.
//!
//! Two parallel chains of first-order allpasses, one of them delayed a sample:
//!
//! ```text
//!     H(z) = 0.5 * [ A0(z^2) + z^-1 A1(z^2) ]
//! ```
//!
//! The C++ reports no latency for them; FreeDAWm does ([`round_trip_latency`]: a few samples, the
//! same at every rate). The coefficients have a closed form and depend only on the transition
//! width, never on the sample rate. Measured in the C++: stage 1 (next to the base rate) has 10
//! coefficients, stages 2 and 3 have 5.
//!
//! Rust difference: the C++ keeps coefficients and state in `std::vector`s, assigned on every
//! oversampling rebuild (which runs on the audio thread when the circuit, variant or quality
//! changes). Here they are fixed-capacity arrays, so a rebuild never allocates. The arithmetic is
//! identical.

use std::f64::consts::PI;

/// Room for every coefficient list `design` produces here (10 for stage 0, 5 for stages 1 and 2).
pub const MAX_COEFS: usize = 16;

/// A coefficient list: `len` values in `c`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefs {
    pub c: [f64; MAX_COEFS],
    pub len: usize,
}

impl Coefs {
    const EMPTY: Coefs = Coefs { c: [0.0; MAX_COEFS], len: 0 };

    fn push(&mut self, v: f64) {
        if let Some(slot) = self.c.get_mut(self.len) {
            *slot = v;
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[f64] {
        self.c.get(..self.len).unwrap_or(&[])
    }
}

fn acc_num(q: f64, order: i32, c: i32) -> f64 {
    let mut r = 0.0;
    let mut i: i32 = 0;
    let mut j: i32 = 1;
    loop {
        let cur = q.powf(f64::from(i * (i + 1))) * (f64::from((i * 2 + 1) * c) * PI / f64::from(order)).sin() * f64::from(j);
        r += cur;
        j = -j;
        i += 1;
        if cur.abs() < 1.0e-30 || i > 100 {
            return r;
        }
    }
}

fn acc_den(q: f64, order: i32, c: i32) -> f64 {
    let mut r = 0.0;
    let mut i: i32 = 1;
    let mut j: i32 = -1;
    loop {
        let cur = q.powf(f64::from(i * i)) * (f64::from(i * 2 * c) * PI / f64::from(order)).cos() * f64::from(j);
        r += cur;
        j = -j;
        i += 1;
        if cur.abs() < 1.0e-30 || i > 100 {
            return r;
        }
    }
}

/// Coefficients for `atten_db` of stopband rejection with transition half-width `tbw`
/// (normalised to the HIGH rate: passband ends at 0.25 - tbw, stopband starts 0.25 + tbw).
pub fn design(atten_db: f64, tbw: f64) -> Coefs {
    let mut k = ((1.0 - tbw * 2.0) * PI / 4.0).tan();
    k *= k;
    let kk = (1.0 - k * k).powf(0.25);
    let e = 0.5 * (1.0 - kk) / (1.0 + kk);
    let e2 = e * e;
    let e4 = e2 * e2;
    let q = e * (1.0 + e4 * (2.0 + e4 * (15.0 + 150.0 * e4)));

    let p = 10.0f64.powf(-atten_db / 10.0);
    let a = p / (1.0 - p);
    // C++ `(int) std::ceil(...)`; `as` saturates where the C++ cast would be undefined.
    let mut order = ((a * a / 16.0).ln() / q.ln()).ceil() as i32;
    if (order & 1) == 0 {
        order += 1;
    }
    if order < 3 {
        order = 3;
    }

    let mut out = Coefs::EMPTY;
    for idx in 0..(order - 1) / 2 {
        let c = idx + 1;
        let ww = acc_num(q, order, c) * q.powf(0.25) / (acc_den(q, order, c) + 0.5);
        let w2 = ww * ww;
        let x = ((1.0 - w2 * k) * (1.0 - w2 / k)).sqrt() / (1.0 + w2);
        out.push((1.0 - x) / (1.0 + x));
    }
    out
}

/// One chain of first-order allpasses, (a + z^-1) / (1 + a z^-1), run at the LOW rate.
#[derive(Debug, Clone, Copy)]
pub struct Chain {
    a: Coefs,
    xp: [f64; MAX_COEFS],
    yp: [f64; MAX_COEFS],
}

impl Default for Chain {
    fn default() -> Self {
        Chain { a: Coefs::EMPTY, xp: [0.0; MAX_COEFS], yp: [0.0; MAX_COEFS] }
    }
}

impl Chain {
    pub fn set(&mut self, coefs: &Coefs) {
        self.a = *coefs;
        self.xp = [0.0; MAX_COEFS];
        self.yp = [0.0; MAX_COEFS];
    }

    pub fn reset(&mut self) {
        self.xp = [0.0; MAX_COEFS];
        self.yp = [0.0; MAX_COEFS];
    }

    #[inline]
    pub fn run(&mut self, mut x: f64) -> f64 {
        for ((a, xp), yp) in self.a.as_slice().iter().zip(self.xp.iter_mut()).zip(self.yp.iter_mut()) {
            let y = a * x + *xp - a * *yp;
            *xp = x;
            *yp = y;
            x = y;
        }
        x
    }
}

/// One 2x stage: an interpolator and a decimator, each with its own state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stage {
    up0: Chain,
    up1: Chain,
    dn0: Chain,
    dn1: Chain,
    pending_odd: f64,
}

impl Stage {
    pub fn set(&mut self, coefs: &Coefs) {
        let mut even = Coefs::EMPTY;
        let mut odd = Coefs::EMPTY;
        for (i, &c) in coefs.as_slice().iter().enumerate() {
            if i & 1 != 0 { odd.push(c) } else { even.push(c) }
        }
        self.up0.set(&even);
        self.up1.set(&odd);
        self.dn0.set(&even);
        self.dn1.set(&odd);
        self.pending_odd = 0.0;
    }

    pub fn reset(&mut self) {
        self.up0.reset();
        self.up1.reset();
        self.dn0.reset();
        self.dn1.reset();
        self.pending_odd = 0.0;
    }

    /// One low-rate sample in, two high-rate samples out, passband gain 1.
    #[inline]
    pub fn up(&mut self, x: f64) -> (f64, f64) {
        (self.up0.run(x), self.up1.run(x))
    }

    /// Two high-rate samples in (x0 first), one low-rate sample out.
    #[inline]
    pub fn down(&mut self, x0: f64, x1: f64) -> f64 {
        let r = 0.5 * (self.dn0.run(x0) + self.dn1.run(self.pending_odd));
        self.pending_odd = x1;
        r
    }
}

/// Coefficients for stage s (0 = next to the base rate), passband to 0.45 x base rate.
/// The C++ caches these in function statics; here the engine computes them once at construction.
pub fn coefs_for_stage(s: usize) -> Coefs {
    // stage 0 has to hold 0.45 fs against 0.55 fs; each later stage only has to keep
    // images and aliases out of 0..0.45 fs, which leaves it a far wider transition
    match s {
        0 => design(100.0, 0.25 - 0.225),
        1 => design(100.0, 0.25 - 0.1125),
        _ => design(100.0, 0.25 - 0.05625),
    }
}

/// How late `stages` stages (stage 0 first), up and back down again, deliver a low-frequency
/// signal, in base-rate samples: the group delay at DC. (The half-bands hold it to a small
/// fraction of a sample through the low and middle of the band.)
///
/// Each stage's interpolator and decimator is `0.5 * [A0(z^2) + z^-1 A1(z^2)]` at its high rate,
/// both branches unit gain and in phase at DC, so each delays DC by half of (its allpasses'
/// delays plus the one sample between the branches). A first-order allpass
/// `(a + z^-1) / (1 + a z^-1)` run in `z^2` delays DC by `2 (1 - a) / (1 + a)` high-rate samples;
/// stage `s` runs at `2^(s + 1)` times the base rate.
pub fn round_trip_delay(coefs: &[Coefs], stages: usize) -> f64 {
    coefs
        .iter()
        .take(stages)
        .enumerate()
        .map(|(s, c)| {
            let allpasses: f64 = c.as_slice().iter().map(|a| 2.0 * (1.0 - a) / (1.0 + a)).sum();
            (allpasses + 1.0) / 2.0f64.powi(s as i32 + 1)
        })
        .sum()
}

/// [`round_trip_delay`] to the nearest whole sample: the latency an engine built on these stages
/// reports (Sat and Vint: 4, 5 and 6 samples at 2x, 4x and 8x, at every rate).
pub fn round_trip_latency(coefs: &[Coefs], stages: usize) -> usize {
    let d = round_trip_delay(coefs, stages).round();
    if d.is_finite() && d > 0.0 { d as usize } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_latency_is_the_measured_low_frequency_delay() {
        // Up through `stages` stages and back down: a 500 Hz tone (at 48 kHz) comes out late by
        // the computed delay (its phase delay, to a twentieth of a sample), and the rounded
        // latency is that to the nearest sample.
        let coefs = [coefs_for_stage(0), coefs_for_stage(1), coefs_for_stage(2)];
        for (stages, want) in [(1, 4), (2, 5), (3, 6)] {
            let mut st = [Stage::default(); 3];
            for (s, c) in st.iter_mut().zip(&coefs) {
                s.set(c);
            }
            let w = std::f64::consts::TAU * 500.0 / 48_000.0;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for n in 0..48_000usize {
                let mut buf = [0.0f64; 8];
                let mut tmp = [0.0f64; 8];
                buf[0] = (w * n as f64).sin();
                let mut count = 1;
                for s in st.iter_mut().take(stages) {
                    for i in 0..count {
                        (tmp[2 * i], tmp[2 * i + 1]) = s.up(buf[i]);
                    }
                    count *= 2;
                    buf[..count].copy_from_slice(&tmp[..count]);
                }
                for s in st.iter_mut().take(stages).rev() {
                    count /= 2;
                    for i in 0..count {
                        buf[i] = s.down(buf[2 * i], buf[2 * i + 1]);
                    }
                }
                if n >= 24_000 {
                    // correlate with the input's sine and cosine: the output's phase
                    re += buf[0] * (w * n as f64).sin();
                    im += buf[0] * (w * n as f64).cos();
                }
            }
            let measured = -im.atan2(re) / w;
            let computed = round_trip_delay(&coefs, stages);
            assert!((measured - computed).abs() < 0.05, "{stages} stages: measured {measured:.3}, computed {computed:.3}");
            assert_eq!(round_trip_latency(&coefs, stages), want, "{stages} stages ({computed:.3})");
        }
        assert_eq!(round_trip_latency(&coefs, 0), 0);
    }

    #[test]
    fn coefficient_counts_match_the_cpp_measurements() {
        assert_eq!(coefs_for_stage(0).len, 10);
        assert_eq!(coefs_for_stage(1).len, 5);
        assert_eq!(coefs_for_stage(2).len, 5);
        for s in 0..3 {
            assert!(coefs_for_stage(s).as_slice().iter().all(|a| a.is_finite() && *a > 0.0 && *a < 1.0));
        }
    }

    #[test]
    fn up_then_down_passes_a_low_tone_at_unity() {
        let mut st = Stage::default();
        st.set(&coefs_for_stage(0));
        let mut peak = 0.0f64;
        for n in 0..4800 {
            let x = (std::f64::consts::TAU * 1000.0 * n as f64 / 48_000.0).sin();
            let (a, b) = st.up(x);
            let y = st.down(a, b);
            if n > 2400 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - 1.0).abs() < 1e-3, "{peak}");
    }
}
