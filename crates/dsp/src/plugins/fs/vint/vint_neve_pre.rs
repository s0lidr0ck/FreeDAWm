//! VintNevePre -- the Neve 1073's amplifiers: the sensitivity switch and the fast model the
//! plugin runs. Translation of `Engine 11 - Vint/Source/VintNevePre.h` (its tables are
//! `tables/vint_neve_curves.rs`, generated from `VintNeveCurves.h`).
//!
//! What is translated: the switch (`PrePos`, `pre_position`, `wafer`, `position_db`,
//! `volts_per_unit` and their constants), `OutputTransformer`'s figures, and the fast model --
//! `Place`, `FastStage`, `FirstOrder`, `FastPre` -- which is what the engine runs.
//!
//! What is not, and why: `gainStage`, `outputStage` and the `NevePre` class are the transistor
//! circuits the fast model was fitted to, solved by `VintBjt.h` (Ebers-Moll, Newton). The engine
//! never instantiates them (its `Channel::pre` is a `FastPre`); they exist for `Tests/prefit.cpp`
//! and the comparison tests, so they cannot reach the output and are left to the C++, as is
//! `VintBjt.h` itself.

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

use super::tables::StageModel;
use super::tables::vint_neve_curves::{
    K_AXIS_POINTS, K_CURVE_POINTS, K_MAX_SLOW, K_TWO_POINTS, STAGE_OPEN, STAGE_OUTPUT, STAGE91, STAGE120, STAGE330,
};

/// C++ `std::clamp (v, lo, hi)`: `v < lo ? lo : (hi < v ? hi : v)`. Unlike `f64::clamp` it never
/// panics (a NaN passes through, as in C++).
#[inline]
pub fn std_clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

/// The LO1166 (linear): 1+1 : 1.7+1.7, windings 6+6 / 20+20 ohms, primary 0.58 H.
#[derive(Debug, Clone, Copy)]
pub struct OutputTransformer {
    pub n: f64,
    pub r_pri: f64,
    pub r_sec: f64,
    pub l_pri: f64,
}

impl Default for OutputTransformer {
    fn default() -> Self {
        OutputTransformer { n: 1.67, r_pri: 12.0, r_sec: 40.0, l_pri: 0.58 }
    }
}

// ---------------------------------------------------------------- the sensitivity switch (EK20033)
// 22 positions, in the switch's order round the dial (see the C++ for each position's wiring).
pub const K_FIRST_STAGE_T: f64 = 91.0;
pub const K_LINE_INPUT_DB: f64 = -3.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrePos {
    pub label: &'static str,
    pub mute: bool,
    pub mic: bool,
    pub two_stages: bool,
    pub pad: f64,
    pub tap: f64,
    pub r_b: f64,
}

pub const K_OPEN: f64 = 1.0e9;

pub const fn ratio(series: f64, shunt: f64) -> f64 {
    shunt / (series + shunt)
}

const LAD: f64 = 33.0 + 27.0 + 47.0 + 82.0 + 150.0 + 270.0 + 430.0;
const TOP: f64 = LAD + 2200.0;

const fn pp(label: &'static str, mute: bool, mic: bool, two_stages: bool, pad: f64, tap: f64, r_b: f64) -> PrePos {
    PrePos { label, mute, mic, two_stages, pad, tap, r_b }
}

static PRE_POSITIONS: [PrePos; 22] = [
    pp("-80", false, true, true, 1.0, 1.0, 120.0),
    pp("-75", false, true, true, 1.0, 1.0, 330.0),
    pp("-70", false, true, true, 1.0, 1.0, K_OPEN),
    pp("-65", false, true, true, 1.0, 0.562341, K_OPEN),
    pp("-60", false, true, true, 1.0, 0.316228, K_OPEN),
    pp("-55", false, true, true, 1.0, 0.177828, K_OPEN),
    pp("OFF", true, true, false, 0.0, 0.0, K_OPEN),
    pp("-50", false, true, false, 1.0, 1.0, 120.0),
    pp("-45", false, true, false, 1.0, 1.0, 330.0),
    pp("-40", false, true, false, 1.0, 1.0, K_OPEN),
    pp("-35", false, true, false, ratio(2200.0, 3900.0), 1.0, K_OPEN),
    pp("-30", false, true, false, ratio(3300.0, 1800.0), 1.0, K_OPEN),
    pp("-25", false, true, false, ratio(3900.0, 1000.0), 1.0, K_OPEN),
    pp("-20", false, true, false, ratio(4300.0, 510.0), 1.0, K_OPEN),
    pp("OFF", true, false, false, 0.0, 0.0, K_OPEN),
    pp("+10", false, false, false, (33.0) / TOP, 1.0, K_OPEN),
    pp("+5", false, false, false, (33.0 + 27.0) / TOP, 1.0, K_OPEN),
    pp("0", false, false, false, (33.0 + 27.0 + 47.0) / TOP, 1.0, K_OPEN),
    pp("-5", false, false, false, (33.0 + 27.0 + 47.0 + 82.0) / TOP, 1.0, K_OPEN),
    pp("-10", false, false, false, (33.0 + 27.0 + 47.0 + 82.0 + 150.0) / TOP, 1.0, K_OPEN),
    pp("-15", false, false, false, (33.0 + 27.0 + 47.0 + 82.0 + 150.0 + 270.0) / TOP, 1.0, K_OPEN),
    pp("-20", false, false, false, LAD / TOP, 1.0, K_OPEN),
];

pub fn pre_position(i: i32) -> &'static PrePos {
    &PRE_POSITIONS[i.clamp(0, 21) as usize]
}

/// The spec's gain at each position (dB), the output into 10k.
pub fn pre_spec_db(i: i32) -> f64 {
    const G: [f64; 22] =
        [80.0, 75.0, 70.0, 65.0, 60.0, 55.0, -200.0, 50.0, 45.0, 40.0, 35.0, 30.0, 25.0, 20.0, -200.0, -10.0, -5.0, 0.0, 5.0, 10.0, 15.0, 20.0];
    G[i.clamp(0, 21) as usize]
}

/// The dial's clockwise position (the parameter) as the switch's wafer position.
pub fn wafer(dial: i32) -> i32 {
    21 - dial.clamp(0, 21)
}

// The stages' gains at 1 kHz (Tests/preswitch.cpp).
pub const K_STAGE_OPEN_DB: f64 = 17.84;
pub const K_STAGE330_DB: f64 = 23.16;
pub const K_STAGE120_DB: f64 = 28.25;
pub const K_STAGE91_DB: f64 = 29.98;
pub const K_OUTPUT_DB: f64 = 15.53;

pub fn stage_db(r_t: f64) -> f64 {
    if r_t > 1.0e8 {
        K_STAGE_OPEN_DB
    } else if r_t > 200.0 {
        K_STAGE330_DB
    } else if r_t > 100.0 {
        K_STAGE120_DB
    } else {
        K_STAGE91_DB
    }
}

/// A wafer position's whole gain, mic or line in to the secondary (dB; -200 when off).
pub fn position_db(w: i32) -> f64 {
    let p = pre_position(w);
    if p.mute {
        return -200.0;
    }
    let mut g = 20.0 * p.pad.log10() + stage_db(p.r_b) + K_OUTPUT_DB + if p.mic { 20.0 * 2.0f64.log10() } else { K_LINE_INPUT_DB };
    if p.two_stages {
        g += K_STAGE91_DB + 20.0 * p.tap.log10();
    }
    g
}

pub const K_LINE_REF_DBU: f64 = 0.0;
pub const K_MIC_REF_DBU: f64 = -50.0;

/// Volts at the input per plugin unit (a -18 dBFS sine, 0.126 peak, is ref dBu).
pub fn volts_per_unit(mic: bool) -> f64 {
    0.7746 * 2.0f64.sqrt() * 10.0f64.powf((if mic { K_MIC_REF_DBU } else { K_LINE_REF_DBU }) / 20.0) / 0.125893
}

// ---------------------------------------------------------------- the fast model

/// A place on a table of N points over the warped axis (u -1..1), read with cubic (Catmull-Rom)
/// interpolation; past its ends, its ends.
#[derive(Debug, Clone, Copy)]
struct Place {
    i0: usize,
    i1: usize,
    i2: usize,
    i3: usize,
    w0: f64,
    w1: f64,
    w2: f64,
    w3: f64,
}

impl Place {
    #[inline]
    fn new(u: f64, n: usize) -> Place {
        let nf = n as f64;
        let p = std_clamp((u * 0.5 + 0.5) * (nf - 1.0), 0.0, nf - 1.0);
        // C++ `std::min ((int) p, N - 2)`; `as` saturates (a NaN reads 0) where C++ is undefined.
        let i = (p as usize).min(n - 2);
        let t = p - i as f64;
        let t2 = t * t;
        let t3 = t2 * t;
        Place {
            i0: i.saturating_sub(1),
            i1: i,
            i2: i + 1,
            i3: (i + 2).min(n - 1),
            w0: 0.5 * (-t + 2.0 * t2 - t3),
            w1: 0.5 * (2.0 - 5.0 * t2 + 3.0 * t3),
            w2: 0.5 * (t + 4.0 * t2 - 3.0 * t3),
            w3: 0.5 * (t3 - t2),
        }
    }

    /// `operator()` on the row starting at `base` in `t`.
    #[inline]
    fn at(&self, t: &[f64], base: usize) -> f64 {
        let g = |i: usize| t.get(base + i).copied().unwrap_or(0.0);
        self.w0 * g(self.i0) + self.w1 * g(self.i1) + self.w2 * g(self.i2) + self.w3 * g(self.i3)
    }
}

pub fn stage_model_for(r_t: f64) -> &'static StageModel {
    if r_t > 1.0e8 {
        return &STAGE_OPEN;
    }
    if r_t > 200.0 {
        return &STAGE330;
    }
    if r_t > 100.0 {
        return &STAGE120;
    }
    &STAGE91
}

/// One stage: its curves, and its slow states' offsets from rest.
#[derive(Debug, Clone, Copy)]
pub struct FastStage {
    m: &'static StageModel,
    d: [f64; K_MAX_SLOW],
    step: [f64; K_MAX_SLOW],
    out_lin: [f64; K_MAX_SLOW],
    flow_lin: [[f64; K_MAX_SLOW]; K_MAX_SLOW],
}

impl Default for FastStage {
    fn default() -> Self {
        FastStage {
            m: &STAGE_OPEN,
            d: [0.0; K_MAX_SLOW],
            step: [0.0; K_MAX_SLOW],
            out_lin: [0.0; K_MAX_SLOW],
            flow_lin: [[0.0; K_MAX_SLOW]; K_MAX_SLOW],
        }
    }
}

impl FastStage {
    pub fn set(&mut self, model: &'static StageModel, fs: f64) {
        self.m = model;
        let n = self.m.n.min(K_MAX_SLOW);
        for k in 0..n {
            self.step[k] = 1.0 / (fs * self.m.inertia[k]);
        }
        // the terms at rest (those that don't vary with the input): read once, here
        let mid = K_TWO_POINTS / 2;
        for j in 0..n {
            self.out_lin[j] = self.m.r_out.get(j * K_TWO_POINTS + mid).copied().unwrap_or(0.0);
        }
        for k in 0..n {
            for j in 0..n {
                self.flow_lin[k][j] = self.m.r_flow.get((k * self.m.n + j) * K_TWO_POINTS + mid).copied().unwrap_or(0.0);
            }
        }
    }
    pub fn reset(&mut self) {
        self.d = [0.0; K_MAX_SLOW];
    }
    pub fn offset(&self, k: usize) -> f64 {
        self.d.get(k).copied().unwrap_or(0.0)
    }
    pub fn nudge(&mut self, k: usize, by: f64) {
        if let Some(d) = self.d.get_mut(k) {
            *d += by;
        }
    }
    pub fn freeze(&mut self, mask: u32) {
        for k in 0..K_MAX_SLOW {
            if (mask >> k) & 1 != 0 {
                self.step[k] = 0.0;
            }
        }
    }

    /// The source's voltage in; the stage's DC-coupled output (from rest) out.
    #[inline]
    pub fn run(&mut self, x: f64) -> f64 {
        let mut q = [0.0f64; K_MAX_SLOW];
        let d = self.d;
        let y = self.eval(x, &d, &mut q);
        let n = self.m.n.min(K_MAX_SLOW);
        for k in 0..n {
            self.d[k] += q[k] * self.step[k];
        }
        y
    }

    /// The core alone: the output (from rest) at input x with the states at offsets d, and each
    /// state's flow into q.
    pub fn eval(&self, x: f64, d: &[f64; K_MAX_SLOW], q: &mut [f64; K_MAX_SLOW]) -> f64 {
        let m = self.m;
        let n = m.n.min(K_MAX_SLOW);
        let mut eff = x;
        for k in 0..n {
            eff += m.in_ref[k] * d[k];
        }
        // where on the (warped) input axis: once, for every table
        let u = (eff / m.x0).asinh() * m.inv_a;
        let f = Place::new(u, K_CURVE_POINTS);
        let c = Place::new(u, K_TWO_POINTS);
        let ca = Place::new(u, K_AXIS_POINTS);
        // the states' terms: read input by input where they vary (m.varies), else their rest's
        let v = m.varies;
        let bit = |s: usize| s < 64 && (v >> s) & 1 != 0;
        let mut y = f.at(m.out, 0);
        for j in 0..n {
            y += (if bit(j) { c.at(m.r_out, j * K_TWO_POINTS) } else { self.out_lin[j] }) * d[j];
        }
        for k in 0..n {
            q[k] = m.flow[k].map(|t| f.at(t, 0)).unwrap_or(0.0);
            let r = k * n * K_TWO_POINTS;
            for j in 0..n {
                q[k] += (if bit(n + k * n + j) { c.at(m.r_flow, r + j * K_TWO_POINTS) } else { self.flow_lin[k][j] }) * d[j];
            }
        }
        if m.two >= 0 {
            // the second axis (and the third): the tables at this input, between the offsets either
            // side -- linearly in each
            let n3 = m.three_n;
            let two = usize::try_from(m.two).unwrap_or(0).min(K_MAX_SLOW - 1);
            let g = std_clamp((d[two] - m.two_lo) / m.two_step, 0.0, f64::from(m.two_n) - 1.000001);
            // C++ `(int) g`: g is in [0, twoN - 1) or NaN (which `as` reads as 0).
            let gi = g as i32;
            let gf = g - f64::from(gi);
            let mut hi = 0i32;
            let mut hf = 0.0;
            if m.three >= 0 {
                let three = usize::try_from(m.three).unwrap_or(0).min(K_MAX_SLOW - 1);
                let v3 = if m.three_w > 0.0 { ((d[three] - m.three_c) / m.three_w).asinh() } else { d[three] };
                let h = std_clamp((v3 - m.three_lo) / m.three_step, 0.0, f64::from(n3) - 1.000001);
                hi = h as i32;
                hf = h - f64::from(hi);
            }
            let axis = K_AXIS_POINTS as i32;
            let r00 = usize::try_from((gi * n3 + hi) * axis).unwrap_or(0);
            let r10 = r00 + usize::try_from(n3 * axis).unwrap_or(0);
            let s = if m.three >= 0 { K_AXIS_POINTS } else { 0 }; // the next row along the third axis
            let w00 = (1.0 - gf) * (1.0 - hf);
            let w10 = gf * (1.0 - hf);
            let w01 = (1.0 - gf) * hf;
            let w11 = gf * hf;
            let blend = |t: &[f64], base: usize| {
                w00 * ca.at(t, base + r00)
                    + w10 * ca.at(t, base + r10)
                    + if s != 0 { w01 * ca.at(t, base + r00 + s) + w11 * ca.at(t, base + r10 + s) } else { 0.0 }
            };
            y += blend(m.out2, 0);
            let per = usize::try_from(m.two_n * n3).unwrap_or(0) * K_AXIS_POINTS; // each flow's block
            for k in 0..n {
                q[k] += blend(m.flow2, k * per);
            }
        }
        y
    }
}

/// A first-order section from analog (b0 + b1 s) / (a0 + a1 s), bilinear (no prewarp).
#[derive(Debug, Clone, Copy)]
pub struct FirstOrder {
    b0: f64,
    b1: f64,
    a1: f64,
    z: f64,
}

impl Default for FirstOrder {
    fn default() -> Self {
        FirstOrder { b0: 1.0, b1: 0.0, a1: 0.0, z: 0.0 }
    }
}

impl FirstOrder {
    pub fn set(&mut self, bb0: f64, bb1: f64, aa0: f64, aa1: f64, fs: f64) {
        let k = 2.0 * fs;
        let d = aa0 + aa1 * k;
        self.b0 = (bb0 + bb1 * k) / d;
        self.b1 = (bb0 - bb1 * k) / d;
        self.a1 = (aa0 - aa1 * k) / d;
    }
    #[inline]
    pub fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z;
        self.z = self.b1 * x - self.a1 * y;
        y
    }
    pub fn reset(&mut self) {
        self.z = 0.0;
    }
}

/// The 1073's amplifiers around its EQ, the fast model: front() takes the source's voltage and
/// returns the second stage's output; back() takes the EQ's output and returns the secondary's.
#[derive(Debug, Clone, Copy)]
pub struct FastPre {
    stage_a: FastStage,
    stage_b: FastStage,
    out_stage: FastStage,
    coup_a: FirstOrder,
    coup_b: FirstOrder,
    hf_out: FirstOrder,
    rate: f64,
    in_scale: f64,
    position: i32,
}

impl Default for FastPre {
    fn default() -> Self {
        FastPre {
            stage_a: FastStage::default(),
            stage_b: FastStage::default(),
            out_stage: FastStage::default(),
            coup_a: FirstOrder::default(),
            coup_b: FirstOrder::default(),
            hf_out: FirstOrder::default(),
            rate: 96000.0,
            in_scale: 0.0,
            position: -1,
        }
    }
}

impl FastPre {
    pub fn prepare(&mut self, fs: f64, position: i32) {
        self.rate = fs;
        self.stage_a.set(stage_model_for(K_FIRST_STAGE_T), fs);
        self.out_stage.set(&STAGE_OUTPUT, fs);
        // each gain stage's output coupling: C15 22u into R20 51k || the 10k after it
        let wc = 1.0 / (22e-6 * (51e3 * 10e3 / 61e3));
        self.coup_a.set(0.0, 1.0, wc, 1.0, fs);
        self.coup_b.set(0.0, 1.0, wc, 1.0, fs);
        // what the output stage's core leaves out: its pF caps and the Zobel, a pole at 88 kHz
        self.hf_out.set(1.0, 0.0, 1.0, 1.0 / (2.0 * 3.14159265358979323846 * 88.0e3), fs);
        self.position = -1;
        self.set_position(position);
        self.stage_a.reset();
        self.stage_b.reset();
        self.out_stage.reset();
        self.coup_a.reset();
        self.coup_b.reset();
        self.hf_out.reset();
    }

    /// A switch position (0..21). A change of the second stage's T resistor keeps its capacitors
    /// where they are, as the real switch does.
    pub fn set_position(&mut self, p: i32) {
        let p = p.clamp(0, 21);
        if p == self.position {
            return;
        }
        self.position = p;
        let s = pre_position(p);
        self.stage_b.set(stage_model_for(s.r_b), self.rate);
        self.in_scale = if s.mute { 0.0 } else { (if s.mic { 2.0 } else { 10.0f64.powf(K_LINE_INPUT_DB / 20.0) }) * s.pad };
    }

    pub fn get_position(&self) -> i32 {
        self.position
    }

    #[inline]
    pub fn front(&mut self, v: f64) -> f64 {
        let s = pre_position(self.position);
        let mut x = v * self.in_scale;
        if s.two_stages {
            x = self.coup_a.run(self.stage_a.run(x)) * s.tap;
        }
        self.coup_b.run(self.stage_b.run(x))
    }

    #[inline]
    pub fn back(&mut self, v: f64) -> f64 {
        let t = OutputTransformer::default();
        self.hf_out.run(self.out_stage.run(v)) * t.n * 10.0e3 / (10.0e3 + t.r_sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_positions_and_levels() {
        assert_eq!(wafer(4), 17);
        assert_eq!(pre_position(wafer(4)).label, "0");
        assert!((position_db(wafer(4)) - 0.0).abs() < 1.0, "{}", position_db(wafer(4)));
        assert_eq!(position_db(6), -200.0);
        assert_eq!(pre_spec_db(0), 80.0);
        assert!((volts_per_unit(false) - 8.7).abs() < 0.05);
        assert_eq!(std_clamp(f64::NAN, 0.0, 1.0).to_bits(), f64::NAN.to_bits());
        assert_eq!(std_clamp(5.0, 0.0, -1.0), -1.0, "an inverted range never panics");
    }

    #[test]
    fn line_zero_is_clean_unity() {
        let mut p = FastPre::default();
        p.prepare(96_000.0, wafer(4));
        let volts = volts_per_unit(false);
        let gain = 10f64.powf(position_db(wafer(4)) / 20.0);
        let mut peak = 0.0f64;
        for n in 0..96_000 {
            let x = 0.125 * (std::f64::consts::TAU * 1000.0 * n as f64 / 96_000.0).sin();
            let front = p.front(x * volts);
            let y = p.back(front) / (volts * gain);
            if n > 48_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((20.0 * (peak / 0.125).log10()).abs() < 0.5, "{peak}");
        assert_eq!(p.get_position(), wafer(4));
    }
}
