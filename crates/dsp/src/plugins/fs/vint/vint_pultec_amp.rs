//! VintPultecAmp -- the EQP-1A's makeup amplifier and its transformers' iron.
//! Translation of the parts of `Engine 11 - Vint/Source/VintPultecAmp.h` the plugin can reach:
//! `Core` (the Jiles-Atherton permalloy core, driven by flux), `Interstage` (the Triad HS-29) and
//! `InputTransformer` (the Triad HS-56).
//!
//! What is not translated, and why: `Koren`, `Koren4`, `gridCurrent`, `applyStep`, `Damper`,
//! `solveSample`, `TubeCaps`, `Amp`, `FastAmp` and `DkAmp` -- the tube amplifier's solvers. The
//! plugin is built with `VINT_MODELLED_PASSIVE = 0` (the Passive runs ideal; `stageFor(cPassive)`
//! is `stNone`), so `pultecAmp()` is never called and the amp's state is only ever copied, never
//! run: none of it can reach the output. They remain the C++'s, for its tests.
//!
//! The two transformers ARE reachable: while the output stage fades out after switching from a
//! circuit with a stage to the Passive (the 15 ms `stageSm` glide), `passiveNetwork` takes its
//! modelled path, which solves both cores against the network. In the plugin build their
//! `prepare` is never called (it sits behind the same switch), so they run in their
//! default-constructed state, exactly as here.

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

/// The permalloy core, driven by flux (see the C++ for the material fit and the numerics).
#[derive(Debug, Clone, Copy)]
pub struct Core {
    /// The winding the core is referred to, and the core's area and path: the S-217-D's by default.
    pub ns: f64,
    pub ae: f64,
    pub le: f64,
    /// An effective air gap (m) in the magnetic path.
    pub gap: f64,
    pub t: f64,
    h: f64,
    b: f64,
    s_prev: f64,
    s_prev2: f64,
    dir: f64,
    sample_dir: f64,
    sample_steps: i32,
    slope_c: f64,
    /// Newton's slope: the field's slope at the end of the step, or the exact derivative of the
    /// integration.
    pub exact_slope: bool,
    kept_s: f64,
    kept_h: f64,
    kept_slope: f64,
}

impl Default for Core {
    fn default() -> Self {
        Core {
            ns: 352.0,
            ae: 0.009525 * 0.020955 * 0.9,
            le: 0.0619,
            gap: 0.0,
            t: 1.0 / 192000.0,
            h: 0.0,
            b: 0.0,
            s_prev: 0.0,
            s_prev2: 0.0,
            dir: 1.0,
            sample_dir: 1.0,
            sample_steps: 1,
            slope_c: 0.0,
            exact_slope: false,
            kept_s: 0.0,
            kept_h: 0.0,
            kept_slope: 0.0,
        }
    }
}

impl Core {
    pub const MU0: f64 = 4.0e-7 * 3.14159265358979323846;
    pub const MS: f64 = 0.75 / Self::MU0;
    pub const A: f64 = 1.96172;
    pub const K: f64 = 1.81988;
    pub const C: f64 = 0.379934;
    pub const ALPHA: f64 = 1.09418e-5;

    pub fn prepare(&mut self, fs: f64) {
        self.t = 1.0 / fs;
        self.h = 0.0;
        self.b = 0.0;
        self.s_prev = 0.0;
        self.s_prev2 = 0.0;
        self.dir = 1.0;
        self.sample_dir = 1.0;
        self.sample_steps = 1;
        self.slope_c = 1.0 / (Self::MU0 * (1.0 + Self::chi(0.0, 0.0, 1.0)));
    }

    /// The core's tangent at its committed state (no integration -- for predicting).
    pub fn tangent(&self, s: f64) -> (f64, f64) {
        let db = 0.5 * self.t * (s + self.s_prev) / (self.ns * self.ae);
        let gg = self.gap / (Self::MU0 * self.le);
        let im = (self.h + self.slope_c * db + (self.b + db) * gg) * self.le / self.ns;
        let dimds = (self.slope_c + gg) * self.le / self.ns * 0.5 * self.t / (self.ns * self.ae);
        (im, dimds)
    }

    /// eval(), remembering the answer so commit_near() can finish the sample from it.
    pub fn eval_keep(&mut self, s: f64) -> (f64, f64) {
        let (im, dimds) = self.eval(s);
        // kept as the iron's own field and slope: the gap's share taken back out
        let bn = self.b + 0.5 * self.t * (s + self.s_prev) / (self.ns * self.ae);
        let gg = self.gap / (Self::MU0 * self.le);
        self.kept_s = s;
        self.kept_h = im * self.ns / self.le - bn * gg;
        self.kept_slope = dimds / (self.le / self.ns * 0.5 * self.t / (self.ns * self.ae)) - gg;
        (im, dimds)
    }

    /// Commits S from the last eval_keep() at a nearby S: the field extended along its slope.
    pub fn commit_near(&mut self, s: f64) {
        let bn = self.b + 0.5 * self.t * (s + self.s_prev) / (self.ns * self.ae);
        let h = self.kept_h + self.kept_slope * 0.5 * self.t * (s - self.kept_s) / (self.ns * self.ae);
        self.dir = self.sample_dir;
        self.h = h;
        self.b = bn;
        self.slope_c = self.kept_slope;
        self.s_prev2 = self.s_prev;
        self.s_prev = s;
    }

    /// Call before each sample's solve: the branch and the step count are fixed for the sample.
    pub fn begin_sample(&mut self) {
        let s_guess = 2.0 * self.s_prev - self.s_prev2;
        let db = 0.5 * self.t * (s_guess + self.s_prev) / (self.ns * self.ae);
        self.sample_dir = if db > 0.0 {
            1.0
        } else if db < 0.0 {
            -1.0
        } else {
            self.dir
        };
        // C++ `std::min (32, 1 + (int) (...))`: `as` saturates where the C++ cast is undefined.
        self.sample_steps = 32.min(1i32.saturating_add((2.0 * db.abs() / 2.0e-3) as i32));
    }

    /// Magnetizing current (secondary-referred) and its slope in S, for secondary voltage S.
    pub fn eval(&self, s: f64) -> (f64, f64) {
        let bn = self.b + 0.5 * self.t * (s + self.s_prev) / (self.ns * self.ae);
        let (mut h, mut slope) = self.integrate(bn);
        h += bn * self.gap / (Self::MU0 * self.le); // the gap's share, as core-length field
        slope += self.gap / (Self::MU0 * self.le);
        if self.exact_slope {
            const D: f64 = 1.0e-7;
            let (hp, _) = self.integrate(bn + D);
            let (hm, _) = self.integrate(bn - D);
            slope = (hp - hm) / (2.0 * D) + self.gap / (Self::MU0 * self.le);
        }
        let im = h * self.le / self.ns;
        let dimds = slope * self.le / self.ns * 0.5 * self.t / (self.ns * self.ae);
        (im, dimds)
    }

    pub fn commit(&mut self, s: f64) {
        let bn = self.b + 0.5 * self.t * (s + self.s_prev) / (self.ns * self.ae);
        let (h, slope) = self.integrate(bn);
        self.dir = self.sample_dir;
        self.h = h;
        self.b = bn;
        self.slope_c = slope;
        self.s_prev2 = self.s_prev;
        self.s_prev = s;
    }

    /// dM/dH at (h, M) moving in direction d: the Jiles-Atherton differential.
    pub fn chi(h: f64, m: f64, d: f64) -> f64 {
        let he = h + Self::ALPHA * m;
        let x = he / Self::A;
        let (l, dl) = if x.abs() < 1.0e-4 {
            (x / 3.0, 1.0 / 3.0 - x * x / 15.0)
        } else {
            let ct = 1.0 / x.tanh();
            (ct - 1.0 / x, 1.0 - ct * ct + 1.0 / (x * x))
        };
        let man = Self::MS * l;
        let dman = Self::MS * dl / Self::A;
        let mirr = (m - Self::C * man) / (1.0 - Self::C);
        let diff = man - mirr;
        // the irreversible part moves only toward the anhysteretic, switched smoothly
        let z = diff * d;
        let dirr = 0.5 * (z + (z * z + 1.0).sqrt()) / Self::K;
        let num = (1.0 - Self::C) * dirr + Self::C * dman;
        num / (1.0 - Self::ALPHA * num).max(1.0e-9)
    }

    /// H at flux density Bn from the committed state, and dH/dB there: RK2 on the sample's branch.
    fn integrate(&self, bn: f64) -> (f64, f64) {
        let db = bn - self.b;
        let d = self.sample_dir;
        let n = self.sample_steps;
        let step = db / f64::from(n);
        let mut b = self.b;
        let mut h = self.h;
        let dhdb = |bb: f64, hh: f64| 1.0 / (Self::MU0 * (1.0 + Self::chi(hh, bb / Self::MU0 - hh, d)));
        for _ in 0..n {
            let k1 = dhdb(b, h);
            let k2 = dhdb(b + 0.5 * step, h + 0.5 * step * k1);
            h += step * k2;
            b += step;
        }
        (h, dhdb(bn, h))
    }
}

/// The interstage transformer, Triad HS-29, between the EQ network and the 12AX7 grids.
#[derive(Debug, Clone, Copy)]
pub struct Interstage {
    pub core: Core,
    /// the primary's DCR, both halves
    pub rp: f64,
    /// a load on the core node (the datasheet rig)
    pub g_load: f64,
    pub last_iterations: i32,
}

impl Default for Interstage {
    fn default() -> Self {
        Interstage { core: Core::default(), rp: 935.0 + 1020.0, g_load: 0.0, last_iterations: 0 }
    }
}

impl Interstage {
    pub const K_AE: f64 = 5.958e-5;
    pub const K_GAP: f64 = 2.46e-6;
    pub const K_NP: f64 = 3600.0;

    pub fn prepare(&mut self, fs: f64) {
        self.core.ns = Self::K_NP;
        self.core.ae = Self::K_AE;
        self.core.le = 6.5 * (Self::K_AE / 0.9).sqrt();
        self.core.gap = Self::K_GAP;
        self.core.prepare(fs);
    }

    /// voc: the network's output with nothing drawn (V); rth: its output resistance (ohms).
    /// Returns the voltage across the core and the current taken from the network. Solves and
    /// commits the sample.
    pub fn run(&mut self, voc: f64, rth: f64) -> (f64, f64) {
        let (x, drawn) = self.solve(voc, rth, true);
        self.commit(x);
        (x, drawn)
    }

    /// The same solve without finishing the sample; returns (voltage, drawn current).
    pub fn solve(&mut self, voc: f64, rth: f64, start: bool) -> (f64, f64) {
        if start {
            self.core.begin_sample();
        }
        let rt = rth + self.rp;
        // start from the core's tangent: F(X) = (X - voc) / Rt + im(X) + gLoad X, linearised at voc
        let (im0, g0) = self.core.tangent(voc);
        let mut x = voc - (im0 + self.g_load * voc) / (1.0 / rt + g0 + self.g_load);
        // Newton, bracketed (F rises with X, so a step leaving the bracket bisects instead)
        let mut lo = -1.0e30f64;
        let mut hi = 1.0e30f64;
        for it in 0..40 {
            let (im, g) = self.core.eval_keep(x);
            let f = (x - voc) / rt + im + self.g_load * x;
            if f > 0.0 {
                hi = hi.min(x);
            } else {
                lo = lo.max(x);
            }
            let df = 1.0 / rt + g + self.g_load;
            let mut xn = x - f / df;
            if !(xn > lo && xn < hi) {
                xn = if lo > -1.0e29 && hi < 1.0e29 { 0.5 * (lo + hi) } else { xn };
            }
            let step = (xn - x).abs();
            x = xn;
            self.last_iterations = it + 1;
            if step < 1.0e-7 {
                break;
            }
        }
        let drawn = (voc - x) / rt;
        (x, drawn)
    }

    /// Takes the solved voltage into the core's state.
    pub fn commit(&mut self, x: f64) {
        self.core.commit_near(x);
    }
}

/// The input transformer, Triad HS-56: 1:1 (its winding resistances are in the network's
/// netlist, so here the core stands alone once prepared).
#[derive(Debug, Clone, Copy, Default)]
pub struct InputTransformer {
    pub base: Interstage,
}

impl InputTransformer {
    pub const K_NP56: f64 = 2107.0;
    pub const K_GAP56: f64 = 11.46e-6;
    pub const K_RS: f64 = 600.0;
    pub const K_RP: f64 = 9.46 + 16.60 + 17.80 + 10.71;
    pub const K_RSEC: f64 = 8.40 + 15.10 + 19.70 + 11.80;

    pub fn prepare(&mut self, fs: f64) {
        let c = &mut self.base.core;
        c.ns = Self::K_NP56;
        c.ae = Interstage::K_AE;
        c.le = 6.5 * (Interstage::K_AE / 0.9).sqrt();
        c.gap = Self::K_GAP56;
        c.prepare(fs);
        self.base.rp = 0.0;
    }

    pub fn solve(&mut self, voc: f64, rth: f64, start: bool) -> (f64, f64) {
        self.base.solve(voc, rth, start)
    }

    pub fn commit(&mut self, x: f64) {
        self.base.commit(x);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_interstage_is_nearly_transparent_and_the_default_one_is_finite() {
        let mut t = Interstage::default();
        t.prepare(192_000.0);
        let mut worst = 0.0f64;
        for n in 0..19_200 {
            let v = 0.5 * (std::f64::consts::TAU * 1000.0 * n as f64 / 192_000.0).sin();
            let (x, _) = t.run(v, 5600.0);
            if n > 9600 {
                worst = worst.max((x - v).abs());
            }
        }
        assert!(worst < 0.02, "{worst}");
        // The plugin build runs them unprepared (their prepare is behind VINT_MODELLED_PASSIVE).
        let mut i = InputTransformer::default();
        for n in 0..1000 {
            let (x, d) = i.solve((n as f64 * 0.01).sin(), 0.0, true);
            i.commit(x);
            assert!(x.is_finite() && d.is_finite());
        }
        let mut p = InputTransformer::default();
        p.prepare(96_000.0);
        assert_eq!(p.base.rp, 0.0);
        assert!(Core::chi(0.0, 0.0, 1.0) > 0.0);
        let _ = (InputTransformer::K_RS, InputTransformer::K_RP, InputTransformer::K_RSEC);
    }
}
