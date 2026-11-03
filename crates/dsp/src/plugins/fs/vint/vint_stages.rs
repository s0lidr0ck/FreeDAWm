//! VintStages -- the output stages: what each box puts on the signal after its EQ.
//! Line-by-line translation of `Engine 11 - Vint/Source/VintStages.h` (adapted in the C++ from
//! SatEngine's circuits: a valve whose bias moves, flux-driven Jiles-Atherton iron, and an
//! amplifier's slew as its own differential equation).
//!
//! A stage is a chain of up to three of those, voiced for the box it follows:
//! - Valve makeup: triode line amp, then the output transformer (retired; the id stays);
//! - Class A: a lopsided transistor stage, then heavier iron (the inductor console);
//! - Discrete: a fast op-amp's slew, then the output transformer (the proportional-Q box);
//! - Op-amp: a modern op-amp's slew, no transformer (British, Enhancer, Air).
//!
//! All of them run at the oversampled rate and are unity gain for small signals.

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

use super::vint_filters::PI;

/// C++ `clampT`: `v < lo ? lo : (v > hi ? hi : v)` (a NaN passes through, as in C++).
#[inline]
pub fn clamp_t<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

// ---------------------------------------------------------------- iron
#[derive(Debug, Clone, Copy)]
pub struct IronSpec {
    pub a: f64,
    pub k: f64,
    pub c: f64,
    pub alpha: f64,
    pub hf_hz: f64,
}

fn langevin(x: f64) -> f64 {
    if x.abs() < 1.0e-4 {
        return x / 3.0;
    }
    1.0 / x.tanh() - 1.0 / x
}

fn langevin_slope(x: f64) -> f64 {
    if x.abs() < 1.0e-3 {
        return 1.0 / 3.0 - x * x / 15.0;
    }
    let s = x.sinh();
    1.0 / (x * x) - 1.0 / (s * s)
}

/// Transformer core: flux-driven Jiles-Atherton, with its HF resonance as a one-pole.
#[derive(Debug, Clone, Copy)]
pub struct Iron {
    pub p: IronSpec,
    /// measured small-signal trim (calibrate.cpp)
    pub trim: f64,
    h: f64,
    m: f64,
    h_prev: f64,
    b_prev: f64,
    lp: f64,
    k: f64,
    os_t: f64,
    leak: f64,
    norm: f64,
    lp_c: f64,
}

impl Default for Iron {
    fn default() -> Self {
        Iron {
            p: IronSpec { a: 0.6, k: 0.25, c: 0.35, alpha: 0.02, hf_hz: 40000.0 },
            trim: 1.0,
            h: 0.0,
            m: 0.0,
            h_prev: 0.0,
            b_prev: 0.0,
            lp: 0.0,
            k: 0.0,
            os_t: 1.0 / 192000.0,
            leak: 0.9999,
            norm: 1.0,
            lp_c: 0.5,
        }
    }
}

impl Iron {
    const AIR: f64 = 0.05;

    pub fn prepare(&mut self, os_fs: f64) {
        self.os_t = 1.0 / os_fs;
        self.k = 2.0 * PI * 100.0;
        self.leak = (-2.0 * PI * 8.0 / os_fs).exp();
        self.lp_c = 1.0 - (-2.0 * PI * self.p.hf_hz.min(0.45 * os_fs) / os_fs).exp();
        let p = &self.p;
        self.norm = (p.c / (3.0 * p.a) + Self::AIR + (1.0 - p.c) / (p.k * (1.0 + 3.0 * p.a))) * self.trim;
        self.h = 0.0;
        self.m = 0.0;
        self.h_prev = 0.0;
        self.b_prev = 0.0;
        self.lp = 0.0;
    }

    pub fn run(&mut self, x: f64) -> f64 {
        self.h = self.h * self.leak + x * self.k * self.os_t;
        let dh = self.h - self.h_prev;
        self.h_prev = self.h;
        let p = self.p;
        let he = self.h + p.alpha * self.m;
        let man = langevin(he / p.a);
        let d_man = langevin_slope(he / p.a) / p.a;
        let delta = if dh >= 0.0 { 1.0 } else { -1.0 };
        let mut irr = 0.0;
        let gap = man - self.m;
        if delta * gap > 0.0 {
            let den = delta * p.k - p.alpha * gap;
            if den.abs() > 1.0e-9 {
                irr = gap / den;
            }
        }
        let num = (1.0 - p.c) * irr + p.c * d_man;
        let den = 1.0 - p.alpha * p.c * d_man;
        self.m = clamp_t(self.m + (num / den) * dh, -1.2, 1.2);
        let b = self.m + Self::AIR * self.h;
        let y = (b - self.b_prev) / (self.k * self.os_t * self.norm);
        self.b_prev = b;
        self.lp += self.lp_c * (y - self.lp);
        self.lp
    }
}

// ---------------------------------------------------------------- valve
pub fn knee_clip(v: f64, k_pos: f64, k_neg: f64, n: f64) -> f64 {
    let kk = if v >= 0.0 { k_pos } else { k_neg };
    v / (1.0 + (v.abs() / kk).powf(n)).powf(1.0 / n)
}

/// A triode line stage whose bias moves: grid-leak charge and the cathode following the average
/// current -- SatEngine's Valve, Triode variant.
#[derive(Debug, Clone, Copy)]
pub struct Valve {
    k_pos: f64,
    k_neg: f64,
    n: f64,
    tau_ms: f64,
    shift_gain: f64,
    leak_ms: f64,
    leak_charge: f64,
    shift: f64,
    grid: f64,
    cath_c: f64,
    leak_c: f64,
    attack_c: f64,
}

impl Default for Valve {
    fn default() -> Self {
        Valve {
            k_pos: 0.70,
            k_neg: 1.60,
            n: 2.0,
            tau_ms: 80.0,
            shift_gain: 1.2,
            leak_ms: 60.0,
            leak_charge: 0.35,
            shift: 0.0,
            grid: 0.0,
            cath_c: 0.001,
            leak_c: 0.001,
            attack_c: 0.01,
        }
    }
}

impl Valve {
    pub fn prepare(&mut self, os_fs: f64) {
        self.cath_c = 1.0 - (-1.0 / (self.tau_ms * 0.001 * os_fs)).exp();
        self.leak_c = 1.0 - (-1.0 / (self.leak_ms * 0.001 * os_fs)).exp();
        self.attack_c = 1.0 - (-1.0 / (0.001 * os_fs)).exp();
        self.shift = 0.0;
        self.grid = 0.0;
    }

    pub fn run(&mut self, x: f64) -> f64 {
        let vg = x - self.shift_gain * self.shift - self.grid;
        let y = knee_clip(vg, self.k_pos, self.k_neg, self.n);
        let over = vg - self.k_pos;
        if over > 0.0 {
            self.grid += self.attack_c * self.leak_charge * over;
        }
        self.grid -= self.leak_c * self.grid;
        self.shift += self.cath_c * (y - self.shift);
        y
    }
}

// ---------------------------------------------------------------- slew-limited amplifier
/// An amplifier as dy/dt = S tanh((t - y) / (S tau)), sub-stepped only while it slews, with the
/// exact linear solution the rest of the time -- SatEngine's Desk. Normalised 1.0 = 10 V.
#[derive(Debug, Clone, Copy)]
pub struct Amp {
    pub soft: f64,
    pub slew_vus: f64,
    pub tau_us: f64,
    y: f64,
    target: f64,
    s: f64,
    s_tau: f64,
    h: f64,
    q: f64,
    e: f64,
    k: i32,
}

impl Default for Amp {
    fn default() -> Self {
        Amp { soft: 0.02, slew_vus: 2.0, tau_us: 1.0, y: 0.0, target: 0.0, s: 1.0, s_tau: 1.0, h: 1e-6, q: 1.0, e: 0.5, k: 1 }
    }
}

impl Amp {
    fn soft_sat(x: f64) -> f64 {
        let x = clamp_t(x, -3.0, 3.0);
        let x2 = x * x;
        x * (27.0 + x2) / (27.0 + 9.0 * x2)
    }

    pub fn prepare(&mut self, os_fs: f64) {
        let os_t = 1.0 / os_fs;
        self.s = self.slew_vus * 1.0e6 / 10.0;
        self.s_tau = self.s * self.tau_us * 1.0e-6;
        // C++ `(int) std::ceil(...)`: finite and small here; `as` saturates where C++ is undefined.
        self.k = clamp_t((2.0 * os_t / (self.tau_us * 1.0e-6)).ceil() as i32, 1, 16);
        self.h = os_t / f64::from(self.k);
        self.q = self.tau_us * 1.0e-6 / os_t;
        self.e = (-os_t / (self.tau_us * 1.0e-6)).exp();
        self.y = 0.0;
        self.target = 0.0;
    }

    pub fn run(&mut self, x: f64) -> f64 {
        let t1 = x / (1.0 + self.soft * x * x).sqrt();
        let t0 = self.target;
        let e0 = t0 - self.y;
        let rq = (t1 - t0) * self.q;
        if e0.abs() < 0.15 * self.s_tau && rq.abs() < 0.15 * self.s_tau {
            let transient = -e0 + rq;
            self.y = t1 - rq + transient * self.e;
            self.target = t1;
            return 0.5 * (t0 + t1) - rq + transient * self.q * (1.0 - self.e);
        }
        let mut yy = self.y;
        let mut acc = 0.0;
        let kf = f64::from(self.k);
        for i in 0..self.k {
            let t = t0 + (t1 - t0) * (f64::from(i) + 0.5) / kf;
            let yn = yy + self.h * self.s * Self::soft_sat((t - yy) / self.s_tau);
            acc += 0.5 * (yy + yn);
            yy = yn;
        }
        self.y = yy;
        self.target = t1;
        acc / kf
    }
}

// ---------------------------------------------------------------- the stages
pub const ST_NONE: i32 = 0;
pub const ST_VALVE: i32 = 1;
pub const ST_CLASS_A: i32 = 2;
pub const ST_DISCRETE: i32 = 3;
pub const ST_OP_AMP: i32 = 4;
pub const NUM_STAGE_TYPES: usize = 5;

/// Where each stage's operating level sits: how hard 0 dB of Drive hits it.
pub const STAGE_SCALE: [f64; NUM_STAGE_TYPES] = [1.0, 0.30, 0.35, 0.60, 0.80];

#[derive(Debug, Clone, Copy)]
pub struct Stage {
    pub type_: i32,
    valve: Valve,
    iron: Iron,
    amp: Amp,
    dc_x: f64,
    dc_y: f64,
    dc_r: f64,
}

impl Default for Stage {
    fn default() -> Self {
        Stage { type_: ST_NONE, valve: Valve::default(), iron: Iron::default(), amp: Amp::default(), dc_x: 0.0, dc_y: 0.0, dc_r: 0.9999 }
    }
}

impl Stage {
    pub fn configure(&mut self, t: i32, iron_trims: &[f64; NUM_STAGE_TYPES]) {
        self.type_ = t;
        // C++ indexes `ironTrims[t]` directly; t is always one of the stage ids here.
        self.iron.trim = iron_trims.get(usize::try_from(t).unwrap_or(0)).copied().unwrap_or(1.0);
        if t == ST_VALVE {
            self.iron.p = IronSpec { a: 0.60, k: 0.25, c: 0.35, alpha: 0.020, hf_hz: 35000.0 }; // light iron after the tube
        }
        if t == ST_CLASS_A {
            self.iron.p = IronSpec { a: 0.90, k: 0.45, c: 0.20, alpha: 0.030, hf_hz: 45000.0 }; // heavier: two transformers
        }
        if t == ST_DISCRETE {
            self.iron.p = IronSpec { a: 0.75, k: 0.35, c: 0.28, alpha: 0.025, hf_hz: 50000.0 };
            self.amp.slew_vus = 1.2;
            self.amp.soft = 0.06;
            self.amp.tau_us = 1.2;
        }
        if t == ST_OP_AMP {
            self.amp.slew_vus = 6.0;
            self.amp.soft = 0.02;
            self.amp.tau_us = 0.6;
        }
    }

    pub fn prepare(&mut self, os_fs: f64) {
        self.valve.prepare(os_fs);
        self.iron.prepare(os_fs);
        self.amp.prepare(os_fs);
        self.dc_r = (-2.0 * PI * 5.0 / os_fs).exp();
        self.dc_x = 0.0;
        self.dc_y = 0.0;
    }

    /// One oversampled sample through the stage, at unity small-signal gain.
    pub fn run(&mut self, x: f64) -> f64 {
        let sc = STAGE_SCALE.get(usize::try_from(self.type_).unwrap_or(0)).copied().unwrap_or(1.0);
        let mut v = x * sc;
        match self.type_ {
            ST_VALVE => v = self.iron.run(self.valve.run(v)),
            ST_CLASS_A => v = self.iron.run(knee_clip(v, 1.1, 1.7, 2.0)),
            ST_DISCRETE => v = self.iron.run(self.amp.run(v)),
            ST_OP_AMP => v = self.amp.run(v),
            _ => return x,
        }
        v /= sc;
        let dc = v - self.dc_x + self.dc_r * self.dc_y; // every lopsided stage makes DC
        self.dc_x = v;
        self.dc_y = dc;
        dc
    }
}
