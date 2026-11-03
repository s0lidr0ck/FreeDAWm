//! TapeEngineDSP -- Engine 9: a tape machine, not a tape echo.
//! Line-by-line translation of `Engine 9 - Tape/Source/TapeEngineDSP.h`.
//!
//! Saturation is perhaps 40 % of what tape does. The rest -- head bump, wow and flutter, the way
//! HF saturates before LF, hiss -- comes from the machine, and TAPE SPEED moves all of it at once:
//! every other constant is derived from it. Four machines: three studio reel-to-reels and a
//! cassette deck (1 7/8 ips only, so its Speed switch does nothing).
//!
//! Signal flow, per channel, at the oversampled rate:
//!
//! ```text
//!   in -> upsample x N --+-> dry delay (the reported latency) ---------------------(dry)--+
//!                        |                                                               |
//!                        +-> drive -> [valve record amp] -> record EQ -> OXIDE (J-A)     |
//!                            + hiss -> TRANSPORT (modulated delay) -> replay EQ ->       |
//!                            head losses + bump -> DC block -> auto gain --------(wet)-> mix
//!                                                                        -> downsample -> output
//! ```
//!
//! Record EQ and replay EQ are exact inverses. The oxide is Jiles-Atherton hysteresis solved with
//! adaptive-step RK2; bias is the reversible fraction c. The transport is a modulated delay, a
//! constant `K_LATENCY_MS` reported to the host whether the knob is up or not; the dry and bypass
//! paths are delayed by the same amount.
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - Float and double are kept exactly where the C++ has them (`float` knobs, output gain and
//!   auto-gain target; `double` everywhere else, the glides in `double`).
//! - `int (v + 0.5f)` on a choice value and the other `(int)` casts: Rust `as` truncates toward zero
//!   like the C++ cast for every finite in-range value, and saturates where the C++ would be
//!   undefined. Non-finite knob values never reach here (the plugin wrapper clamps first, as the
//!   APVTS does in the C++).
//! - `clampT` returns NaN for NaN like the C++ template (comparisons false); same for `std::max`.
//! - Ring indices are C++ `int`s masked with `& mask`: two's complement, so a negative index wraps
//!   into the ring. Reproduced with `i32` arithmetic and the same masks.
//! - The hiss and the transport use the C++'s own LCG, seeded identically in `prepare`, so two
//!   renders are sample-identical (to each other and to the C++).
//! - The delay lines are `Vec`s sized in `prepare` for the largest (8x) quality, as the C++ does;
//!   everything else is fixed-size, so a quality or voicing change on the audio thread never
//!   allocates. Reads and writes use checked indexing (a process before prepare reads zeros).

use std::f64::consts::PI;

use super::tape_gain_table as tapegain;
use super::tape_halfband::{self as tapehb, Coefs, Stage};

pub const M_TWO_TRACK: i32 = 0;
pub const M_MULTITRACK: i32 = 1;
pub const M_VALVE: i32 = 2;
pub const M_CASSETTE: i32 = 3;
pub const NUM_MACHINES: i32 = 4;
pub const NUM_FORMULAS: i32 = 3;
pub const NUM_SPEEDS: i32 = 3;

// ---------------------------------------------------------------- parameters (order == Parameters.h)
pub const P_MACHINE: usize = 0;
pub const P_FORMULA: usize = 1;
pub const P_SPEED: usize = 2;
pub const P_DRIVE: usize = 3;
pub const P_BIAS: usize = 4;
pub const P_FLUTTER: usize = 5;
pub const P_HISS: usize = 6;
pub const P_MIX: usize = 7;
pub const P_OUTPUT: usize = 8;
pub const P_AUTO_GAIN: usize = 9;
pub const P_QUALITY: usize = 10;
pub const P_BYPASS: usize = 11;
pub const NUM_PARAMS: usize = 12;

// ---------------------------------------------------------------- unit maps
/// -6 .. +24 dB re 0 VU (-18 dBFS).
pub fn knob_to_drive_db(v: f32) -> f32 {
    -6.0 + 30.0 * v
}
/// -12 .. +12 dB.
pub fn knob_to_out_db(v: f32) -> f32 {
    -12.0 + 24.0 * v
}
/// -1 under .. +1 over, 0.5 = aligned.
pub fn knob_to_bias(v: f32) -> f32 {
    2.0 * v - 1.0
}
/// Re the real machine's noise; 0 is off.
pub fn knob_to_hiss_db(v: f32) -> f32 {
    -24.0 + 42.0 * v
}

pub const K_LATENCY_MS: f64 = 2.0;

/// Tape speed in inches per second. Cassette ignores the Speed switch.
pub fn speed_ips(machine: i32, speed: i32) -> f64 {
    const IPS: [f64; 3] = [7.5, 15.0, 30.0];
    if machine == M_CASSETTE { 1.875 } else { IPS[clamp_t(speed, 0, 2) as usize] }
}

/// Peak speed deviation, in %, that the Wow & Flutter knob reaches at `knob` on `machine`: every
/// component of the transport at its peak, as the delay-domain sum in `transport_deviation`
/// produces it. A ceiling -- the components rarely peak together.
pub fn flutter_percent(machine: i32, knob: f32) -> f64 {
    let m = &MACHINE_SPECS[clamp_t(machine, 0, NUM_MACHINES - 1) as usize];
    let drift_pct = m.drift_ms * 1.0e-3 * 2.0 * PI * (0.5 * 0.071 + 0.3 * 0.13 + 0.2 * 0.21) * 100.0;
    f64::from(knob) * (1.6 * m.wow_pct + 1.4 * m.flut_pct + drift_pct) // wow: 1 + 0.25 wobble + 0.35 partner
}

// ---------------------------------------------------------------- small helpers
/// The C++ `clampT`: `v < lo ? lo : (v > hi ? hi : v)` (NaN passes through).
#[inline]
fn clamp_t<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `std::max` for floats: `a < b ? b : a`.
#[inline]
fn max_f(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

/// `std::max` for doubles.
#[inline]
fn max_d(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `std::lround` (half away from zero) to `int`; `as` saturates where the C++ would be undefined.
#[inline]
fn lround(x: f64) -> i32 {
    x.round() as i32
}

/// The C++ LCG: `s = s * 1664525u + 1013904223u; return (s >> 8) * (1.0 / 16777216.0);`
#[inline]
fn uniform(s: &mut u32) -> f64 {
    *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    f64::from(*s >> 8) * (1.0 / 16_777_216.0)
}

/// One-pole glide that LANDS, in double -- see SatEngineDSP::glide for why both matter.
#[inline]
fn glide(smooth: f64, v: &mut f64, target: f64) {
    *v += smooth * (target - *v);
    if (target - *v).abs() < 1.0e-9 {
        *v = target;
    }
}

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }
}

impl Biquad {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
    fn copy_coefs(&mut self, o: &Biquad) {
        self.b0 = o.b0;
        self.b1 = o.b1;
        self.b2 = o.b2;
        self.a1 = o.a1;
        self.a2 = o.a2;
    }
}

fn lowpass(fc: f64, fs: f64, q: f64) -> Biquad {
    let w = 2.0 * PI * fc.min(0.45 * fs) / fs;
    let cw = w.cos();
    let al = w.sin() / (2.0 * q);
    let a0 = 1.0 + al;
    let b0 = (1.0 - cw) * 0.5 / a0;
    Biquad { b0, b1: (1.0 - cw) / a0, b2: b0, a1: -2.0 * cw / a0, a2: (1.0 - al) / a0, ..Biquad::default() }
}

fn highpass(fc: f64, fs: f64, q: f64) -> Biquad {
    let w = 2.0 * PI * fc.min(0.45 * fs) / fs;
    let cw = w.cos();
    let al = w.sin() / (2.0 * q);
    let a0 = 1.0 + al;
    let b0 = (1.0 + cw) * 0.5 / a0;
    Biquad { b0, b1: -(1.0 + cw) / a0, b2: b0, a1: -2.0 * cw / a0, a2: (1.0 - al) / a0, ..Biquad::default() }
}

fn peaking(fc: f64, fs: f64, q: f64, gain_db: f64) -> Biquad {
    let a = 10.0f64.powf(gain_db / 40.0);
    let w = 2.0 * PI * fc.min(0.45 * fs) / fs;
    let cw = w.cos();
    let al = w.sin() / (2.0 * q);
    let a0 = 1.0 + al / a;
    Biquad { b0: (1.0 + al * a) / a0, b1: -2.0 * cw / a0, b2: (1.0 - al * a) / a0, a1: -2.0 * cw / a0, a2: (1.0 - al / a) / a0, ..Biquad::default() }
}

/// First order, one zero and one pole: H(s) = g (s + wz) / (s + wp). Unity at DC (or at the top,
/// with `norm_top`), and |H| = wp / wz at the other end. Both corners prewarped. `inverse` swaps
/// zero and pole, which is EXACTLY 1/H, so a record curve and its replay curve cancel.
#[derive(Debug, Clone, Copy)]
struct Tilt1 {
    b0: f64,
    b1: f64,
    a1: f64,
    z: f64,
}

impl Default for Tilt1 {
    fn default() -> Self {
        Tilt1 { b0: 1.0, b1: 0.0, a1: 0.0, z: 0.0 }
    }
}

impl Tilt1 {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z;
        self.z = self.b1 * x - self.a1 * y;
        y
    }
    fn reset(&mut self) {
        self.z = 0.0;
    }
    /// Sets the coefficients only; the state is kept, as in the C++.
    fn set(&mut self, fz: f64, fp: f64, fs: f64, norm_top: bool, inverse: bool) {
        let c = 2.0 * fs;
        let mut wz = c * (PI * fz.min(0.45 * fs) / fs).tan();
        let mut wp = c * (PI * fp.min(0.45 * fs) / fs).tan();
        let mut g = if norm_top { 1.0 } else { wp / wz }; // DC gain 1 means g * wz / wp = 1
        if inverse {
            std::mem::swap(&mut wz, &mut wp);
            g = 1.0 / g;
        }
        self.b0 = g * (c + wz) / (c + wp);
        self.b1 = g * (wz - c) / (c + wp);
        self.a1 = (wp - c) / (c + wp);
    }
}

// ---------------------------------------------------------------- the machines
// Emphasis time constants are the published standards: NAB 50 us + 3180 us at 7.5 and 15 ips,
// CCIR/IEC1 70 us at 7.5 and 35 us at 15, AES 17.5 us at 30 for everybody, and IEC 120 us
// (Type I) or 70 us (Type II, IV) with 3180 us on cassette.
//
//   bump        f0 per ips, peak and following dip in dB, the LF roll-off per ips
//   hfHz        what is left of the top after alignment, per speed (-3 dB, 2nd order)
//   lossDb/Hz   the HF loss the record amplifier compensates, per speed
//   wow, flut   peak speed deviation at the top of the knob: a tired machine, in %
//   drift       slow speed wander, as a delay excursion in ms
//   capstan, reel  diameters in inches: the flutter and wow RATES follow from speed
//   snr         unweighted signal-to-noise re 0 VU, per speed
//   mol         3 % THD at 1 kHz, dB re 0 VU, per formulation
struct MachineSpec {
    emph_us: [f64; 3],
    lf_us: [f64; 3],
    bump_hz_per_ips: f64,
    bump_db: f64,
    dip_db: f64,
    lf_hz_per_ips: f64,
    hf_hz: [f64; 3],
    loss_db: [f64; 3],
    loss_hz: [f64; 3],
    wow_pct: f64,
    flut_pct: f64,
    drift_ms: f64,
    capstan_in: f64,
    reel_in: f64,
    snr_db: [f64; 3],
    #[allow(dead_code)] // the C++ keeps it for documentation; molScale does the work
    mol_db: [f64; 3],
}

const MACHINE_SPECS: [MachineSpec; NUM_MACHINES as usize] = [
    // Two-Track: an American mastering deck, NAB, wide tracks: the quietest and cleanest
    MachineSpec {
        emph_us: [50.0, 50.0, 17.5],
        lf_us: [3180.0, 3180.0, 0.0],
        bump_hz_per_ips: 3.4,
        bump_db: 2.0,
        dip_db: -1.0,
        lf_hz_per_ips: 1.2,
        hf_hz: [15000.0, 21000.0, 27000.0],
        loss_db: [7.0, 4.0, 2.0],
        loss_hz: [6000.0, 8000.0, 10000.0],
        wow_pct: 0.06,
        flut_pct: 0.04,
        drift_ms: 0.15,
        capstan_in: 1.00,
        reel_in: 7.0,
        snr_db: [65.0, 68.0, 70.0],
        mol_db: [9.0, 12.0, 6.0],
    },
    // Multitrack: a European 24-track, CCIR, narrow tracks on 2-inch tape: the big bump
    MachineSpec {
        emph_us: [70.0, 35.0, 17.5],
        lf_us: [0.0, 0.0, 0.0],
        bump_hz_per_ips: 3.2,
        bump_db: 3.0,
        dip_db: -1.5,
        lf_hz_per_ips: 1.0,
        hf_hz: [14000.0, 20000.0, 26000.0],
        loss_db: [7.0, 4.0, 2.0],
        loss_hz: [6000.0, 8000.0, 10000.0],
        wow_pct: 0.08,
        flut_pct: 0.05,
        drift_ms: 0.20,
        capstan_in: 1.25,
        reel_in: 9.0,
        snr_db: [60.0, 63.0, 65.0],
        mol_db: [8.0, 11.0, 5.0],
    },
    // Valve: a 1950s deck with valve record electronics, NAB: soft top, a little looser
    MachineSpec {
        emph_us: [50.0, 50.0, 17.5],
        lf_us: [3180.0, 3180.0, 0.0],
        bump_hz_per_ips: 3.0,
        bump_db: 2.5,
        dip_db: -1.0,
        lf_hz_per_ips: 1.3,
        hf_hz: [12500.0, 17500.0, 23000.0],
        loss_db: [8.0, 5.0, 3.0],
        loss_hz: [5500.0, 7500.0, 9500.0],
        wow_pct: 0.12,
        flut_pct: 0.08,
        drift_ms: 0.30,
        capstan_in: 0.60,
        reel_in: 5.0,
        snr_db: [57.0, 60.0, 62.0],
        mol_db: [7.0, 10.0, 4.0],
    },
    // Cassette: 1 7/8 ips. The per-speed columns are unused (one speed); the per-formula
    // differences -- Type I / II / IV -- live in CASSETTE_VOICING below.
    MachineSpec {
        emph_us: [120.0, 120.0, 120.0],
        lf_us: [3180.0, 3180.0, 3180.0],
        bump_hz_per_ips: 29.3,
        bump_db: 1.5,
        dip_db: -0.5,
        lf_hz_per_ips: 11.7,
        hf_hz: [11000.0, 11000.0, 11000.0],
        loss_db: [10.0, 10.0, 10.0],
        loss_hz: [4500.0, 4500.0, 4500.0],
        wow_pct: 0.30,
        flut_pct: 0.12,
        drift_ms: 0.40,
        capstan_in: 0.094,
        reel_in: 1.3,
        snr_db: [50.0, 50.0, 50.0],
        mol_db: [4.0, 5.0, 8.0],
    },
];

/// Cassette's formulations change more than the oxide: the EQ standard (Type I is 120 us, the
/// rest 70 us), the top end and the noise.
struct CassetteVoicing {
    emph_us: f64,
    hf_hz: f64,
    snr_db: f64,
}
const CASSETTE_VOICING: [CassetteVoicing; NUM_FORMULAS as usize] = [
    CassetteVoicing { emph_us: 120.0, hf_hz: 11000.0, snr_db: 50.0 }, // Type I   ferric: dull, noisy, saturates early up top
    CassetteVoicing { emph_us: 70.0, hf_hz: 13000.0, snr_db: 53.0 },  // Type II  chrome: brighter and quieter
    CassetteVoicing { emph_us: 70.0, hf_hz: 14500.0, snr_db: 54.0 },  // Type IV  metal:  the most headroom a cassette ever had
];

/// The reel formulations' non-oxide differences: top end and noise.
const REEL_HF_SCALE: [f64; NUM_FORMULAS as usize] = [1.00, 1.05, 0.85];
const REEL_SNR_OFFSET: [f64; NUM_FORMULAS as usize] = [0.0, 2.0, -4.0];

/// The oxide, Jiles-Atherton in normalised units (Ms = 1): a anhysteretic shape, k pinning (loop
/// width), c reversible fraction AT ALIGNED BIAS, alpha inter-domain coupling.
#[derive(Debug, Clone, Copy)]
pub struct JaSpec {
    a: f64,
    k: f64,
    c: f64,
    alpha: f64,
}
const REEL_OXIDE: [JaSpec; NUM_FORMULAS as usize] = [
    JaSpec { a: 0.45, k: 0.40, c: 0.85, alpha: 0.012 }, // Standard     the classic mastering tape of the 70s
    JaSpec { a: 0.55, k: 0.35, c: 0.88, alpha: 0.012 }, // High Output  +3 dB of headroom, cleaner when pushed
    JaSpec { a: 0.40, k: 0.55, c: 0.78, alpha: 0.015 }, // Vintage      older, lower-output oxide: a wider loop
];
const CASSETTE_OXIDE: [JaSpec; NUM_FORMULAS as usize] = [
    JaSpec { a: 0.40, k: 0.50, c: 0.75, alpha: 0.015 }, // Type I
    JaSpec { a: 0.45, k: 0.42, c: 0.83, alpha: 0.012 }, // Type II
    JaSpec { a: 0.50, k: 0.35, c: 0.87, alpha: 0.012 }, // Type IV
];

/// First guess at the oxide's small-signal slope; the measured trim does the real work.
fn slope_guess(p: &JaSpec, c: f64) -> f64 {
    c / (3.0 * p.a) + (1.0 - c) / (p.k * (1.0 + 3.0 * p.a))
}

// ---------------------------------------------------------------- the valve record amp
fn knee_clip(v: f64, k_pos: f64, k_neg: f64, n: f64) -> f64 {
    let k = if v >= 0.0 { k_pos } else { k_neg };
    v / (1.0 + (v.abs() / k).powf(n)).powf(1.0 / n)
}

// ---------------------------------------------------------------- the oxide
/// The Langevin function L(x) = coth x - 1/x and its slope, together, from ONE exp. Below
/// |x| = 0.05 the closed forms cancel badly, so the series takes over.
fn langevin_both(x: f64) -> (f64, f64) {
    let ax = x.abs();
    if ax < 0.05 {
        let x2 = x * x;
        let l = x * (1.0 / 3.0 - x2 * (1.0 / 45.0 - x2 * (2.0 / 945.0)));
        let dl = 1.0 / 3.0 - x2 * (1.0 / 15.0 - x2 * (2.0 / 189.0));
        return (l, dl);
    }
    let e = (-2.0 * ax).exp();
    let coth = (1.0 + e) / (1.0 - e);
    let inv = 1.0 / ax;
    let l = (if x >= 0.0 { 1.0 } else { -1.0 }) * (coth - inv);
    let dl = inv * inv - (coth * coth - 1.0);
    (l, dl)
}

fn d_m_d_h(p: &JaSpec, h: f64, m: f64, dir: f64) -> f64 {
    let c = p.c;
    let he = h + p.alpha * m;
    let (man, mut d_man) = langevin_both(he / p.a);
    d_man /= p.a;

    let mut irr = 0.0;
    let gap = man - m;
    if dir * gap > 0.0 {
        // the standard fix: no negative susceptibility
        let den = dir * p.k - p.alpha * gap;
        if den.abs() > 1.0e-9 {
            irr = gap / den;
        }
    }

    let num = (1.0 - c) * irr + c * d_man;
    let den = 1.0 - p.alpha * c * d_man;
    num / den
}

/// One sample of the field, H0 -> H1: returns the magnetisation at the end of the sample, and in
/// `avg` its AVERAGE across the sample (the output). Midpoint RK2, one step for every `K_MAX_STEP`
/// of field travelled (fixed in FIELD units, so every Quality is the same tape), at most
/// `K_MAX_STEPS` steps. The average is a box filter whose nulls sit on multiples of the sample
/// rate, which is what folds to near DC.
fn oxide(p: &JaSpec, h0: f64, h1: f64, mut m: f64, avg: &mut f64) -> f64 {
    const K_MAX_STEP: f64 = 0.3;
    const K_MAX_STEPS: i32 = 32; // a hard ceiling on the cost of the worst sample
    let d_h = h1 - h0;
    if d_h == 0.0 {
        *avg = m;
        return m;
    }
    // C++ `(int) std::ceil(...)`: `as` saturates (NaN -> 0) where the C++ cast is undefined.
    let steps = clamp_t((d_h.abs() / K_MAX_STEP).ceil() as i32, 1, K_MAX_STEPS);
    let h = d_h / f64::from(steps);
    let dir = if d_h > 0.0 { 1.0 } else { -1.0 };

    let mut hh = h0;
    let mut acc = 0.0;
    for _ in 0..steps {
        let k1 = d_m_d_h(p, hh, m, dir) * h;
        let k2 = d_m_d_h(p, hh + 0.5 * h, m + 0.5 * k1, dir) * h;
        let next = clamp_t(m + k2, -1.2, 1.2);
        acc += 0.5 * (m + next);
        m = next;
        hh += h;
    }
    *avg = acc / f64::from(steps);
    m
}

/// Cubic Hermite read, `d` samples behind the newest (d >= 2). At integer d it returns the stored
/// sample exactly, so with the transport still the wet path is a pure delay.
#[inline]
fn read_line(line: &[f64], line_mask: i32, newest: i32, d: f64) -> f64 {
    let pos = f64::from(newest) - d;
    let fl = pos.floor();
    let i = fl as i32;
    let f = pos - fl;
    let at = |j: i32| line.get((j & line_mask) as usize).copied().unwrap_or(0.0);
    let xm1 = at(i.wrapping_sub(1));
    let x0 = at(i);
    let x1 = at(i.wrapping_add(1));
    let x2 = at(i.wrapping_add(2));
    if f == 0.0 {
        return x0;
    }
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * f + c2) * f + c1) * f + x0
}

// ---------------------------------------------------------------- per-channel state
#[derive(Debug, Clone, Default)]
struct Channel {
    os: [Stage; 3], // os[0] sits next to the base rate
    pre: [Tilt1; 3],
    de: [Tilt1; 3],
    noise_pre: [Tilt1; 3], // LF (NAB), HF emphasis, loss compensation
    bias_shelf: Tilt1,
    hf_loss: Biquad,
    bump: Biquad,
    dip: Biquad,
    lf_cut: Biquad,
    valve_dc_x: f64,
    valve_dc_y: f64,
    h: f64,
    m: f64,
    dc_x: f64,
    dc_y: f64,
    env: f64, // modulation noise follows the signal on tape
    rng: u32,

    wet_line: Vec<f64>, // at the oversampled rate
    dry_line: Vec<f64>,
    byp_line: Vec<f32>, // at the base rate
    w: i32,
    byp_w: i32,
}

impl Channel {
    fn reset_state(&mut self) {
        for f in &mut self.pre {
            f.reset();
        }
        for f in &mut self.de {
            f.reset();
        }
        for f in &mut self.noise_pre {
            f.reset();
        }
        self.bias_shelf.reset();
        self.hf_loss.reset();
        self.bump.reset();
        self.dip.reset();
        self.lf_cut.reset();
        self.valve_dc_x = 0.0;
        self.valve_dc_y = 0.0;
        self.h = 0.0;
        self.m = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.env = 0.0;
        self.wet_line.fill(0.0);
        self.dry_line.fill(0.0);
        self.w = 0;
    }
}

/// Modulation noise sits this far below the signal it rides on, at the Real hiss setting.
const MOD_NOISE_RATIO: f64 = 0.006; // about -45 dB

type MolScale = [[f64; tapegain::K_FORMULAS]; tapegain::K_MACHINES];
type Trim = [[[f64; tapegain::K_BIAS_POINTS]; tapegain::K_FORMULAS]; tapegain::K_MACHINES];
type NoiseTrim = [[[f64; tapegain::K_SPEEDS]; tapegain::K_FORMULAS]; tapegain::K_MACHINES];

/// The engine. Field names and initial values follow the C++ member list.
pub struct TapeEngineDsp {
    ch: [Channel; 2],
    /// `tapehb::coefsForStage` (function statics in the C++), computed once here.
    stage_coefs: [Coefs; 3],

    // ---------------------------------------------------------------- state
    machine: i32,
    formula: i32,
    speed: i32,
    quality: i32,
    drive_knob: f32,
    bias_knob: f32,
    flutter_knob: f32,
    hiss_knob: f32,
    mix_knob: f32,
    out_knob: f32,
    auto_gain: bool,
    bypassed: bool,
    calibrating: bool,

    drive_sm: f64,
    bias_sm: f64,
    flut_sm: f64,
    hiss_sm: f64,
    mix_sm: f64,
    out_sm: f64,
    ag_sm: f64,
    byp_sm: f64,
    smooth: f64,
    coef_counter: i32,

    fs_base: f64,
    os_fs: f64,
    dc_r: f64,
    valve_dc_r: f64,
    env_coef: f64,
    os_factor: i32,
    num_stages: i32,
    latency: i32,
    line_mask: i32,
    byp_mask: i32,
    base_delay_os: i32,
    hb_delay: [f64; 3],
    max_dev_sec: f64,

    transport_phase: [f64; 7],
    wow_wobble: f64,
    dev_prev: f64,

    noise_ref: f64,

    mol_scale: MolScale,
    trim: Trim,
    noise_trim: NoiseTrim,

    in_peak: f32,
    out_peak: f32,
}

impl Default for TapeEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl TapeEngineDsp {
    pub fn new() -> Self {
        // The C++ constructor copies the generated tables into the (calibration-writable) members.
        TapeEngineDsp {
            ch: [Channel { rng: 1, ..Channel::default() }, Channel { rng: 1, ..Channel::default() }],
            stage_coefs: [tapehb::coefs_for_stage(0), tapehb::coefs_for_stage(1), tapehb::coefs_for_stage(2)],
            machine: M_MULTITRACK,
            formula: 0,
            speed: 1,
            quality: 1,
            drive_knob: 0.3,
            bias_knob: 0.5,
            flutter_knob: 0.0,
            hiss_knob: 0.0,
            mix_knob: 1.0,
            out_knob: 0.5,
            auto_gain: true,
            bypassed: false,
            calibrating: false,
            drive_sm: 0.3,
            bias_sm: 0.5,
            flut_sm: 0.0,
            hiss_sm: 0.0,
            mix_sm: 1.0,
            out_sm: 1.0,
            ag_sm: 1.0,
            byp_sm: 0.0,
            smooth: 0.001,
            coef_counter: 0,
            fs_base: 0.0,
            os_fs: 192000.0,
            dc_r: 0.9999,
            valve_dc_r: 0.9999,
            env_coef: 0.001,
            os_factor: 4,
            num_stages: 2,
            latency: 96,
            line_mask: 0,
            byp_mask: 0,
            base_delay_os: 384,
            hb_delay: [0.0; 3],
            max_dev_sec: 0.0019,
            transport_phase: [0.0; 7],
            wow_wobble: 0.0,
            dev_prev: 0.0,
            noise_ref: 1.0e-4,
            mol_scale: tapegain::MOL_SCALE,
            trim: tapegain::TRIM,
            noise_trim: tapegain::NOISE_TRIM,
            in_peak: 0.0,
            out_peak: 0.0,
        }
    }

    pub fn set_parameter(&mut self, index: usize, v: f32) {
        match index {
            P_MACHINE => {
                let m = clamp_t((v + 0.5) as i32, 0, NUM_MACHINES - 1);
                if m != self.machine {
                    self.machine = m;
                    self.voicing_changed();
                }
            }
            P_FORMULA => {
                let f = clamp_t((v + 0.5) as i32, 0, NUM_FORMULAS - 1);
                if f != self.formula {
                    self.formula = f;
                    self.voicing_changed();
                }
            }
            P_SPEED => {
                let s = clamp_t((v + 0.5) as i32, 0, NUM_SPEEDS - 1);
                if s != self.speed {
                    self.speed = s;
                    self.voicing_changed();
                }
            }
            P_DRIVE => self.drive_knob = clamp_t(v, 0.0, 1.0),
            P_BIAS => self.bias_knob = clamp_t(v, 0.0, 1.0),
            P_FLUTTER => self.flutter_knob = clamp_t(v, 0.0, 1.0),
            P_HISS => self.hiss_knob = clamp_t(v, 0.0, 1.0),
            P_MIX => self.mix_knob = clamp_t(v, 0.0, 1.0),
            P_OUTPUT => self.out_knob = clamp_t(v, 0.0, 1.0),
            P_AUTO_GAIN => self.auto_gain = v > 0.5,
            P_QUALITY => {
                let q = clamp_t((v + 0.5) as i32, 0, 2);
                if q != self.quality {
                    self.quality = q;
                    if self.fs_base > 0.0 {
                        self.rebuild_oversampling();
                    }
                }
            }
            P_BYPASS => self.bypassed = v > 0.5,
            _ => {}
        }
    }

    // ---------------------------------------------------------------- lifecycle
    /// Allocates the delay lines (the only allocation, for the largest quality). The caller keeps
    /// `sample_rate` sane (the plugin wrapper bounds it); a non-positive one means 48 kHz, as in C++.
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs_base = if sample_rate > 0.0 { sample_rate } else { 48000.0 };
        self.smooth = 1.0 - (-1.0 / (0.015 * self.fs_base)).exp(); // 15 ms
        self.latency = lround(K_LATENCY_MS * 0.001 * self.fs_base);

        // Every buffer at its largest (8x) size, once, here -- a Quality switch arrives on the
        // audio thread and must not allocate.
        let os_max = self.latency.saturating_mul(8 * 2).saturating_add(64);
        let mut size: i32 = 1;
        while size < os_max && size < (1 << 30) {
            size <<= 1;
        }
        for c in &mut self.ch {
            // clear + resize keeps the capacity, so a re-prepare at the same rate reuses it.
            c.wet_line.clear();
            c.wet_line.resize(size as usize, 0.0);
            c.dry_line.clear();
            c.dry_line.resize(size as usize, 0.0);
        }
        self.line_mask = size - 1;

        self.measure_halfband_delays();

        let mut bsize: i32 = 1;
        while bsize < self.latency.saturating_add(4) && bsize < (1 << 30) {
            bsize <<= 1;
        }
        for c in &mut self.ch {
            c.byp_line.clear();
            c.byp_line.resize(bsize as usize, 0.0);
        }
        self.byp_mask = bsize - 1;

        self.rebuild_oversampling();

        // Every smoothed value starts AT its target, so a session loads without a ramp.
        self.drive_sm = f64::from(self.drive_knob);
        self.bias_sm = f64::from(self.bias_knob);
        self.flut_sm = f64::from(self.flutter_knob);
        self.hiss_sm = self.hiss_gain_target();
        self.mix_sm = f64::from(self.mix_knob);
        self.out_sm = f64::from(self.out_gain_target());
        self.ag_sm = f64::from(self.auto_gain_target());
        self.byp_sm = if self.bypassed { 1.0 } else { 0.0 };
        self.update_bias_shelf(self.bias_sm as f32);

        // The transport starts at the same place on every prepare, so two bounces of one session
        // are identical. Phases are spread so the components never line up at t = 0.
        let mut seed: u32 = 0x5eed_1234;
        for ph in &mut self.transport_phase {
            *ph = 2.0 * PI * uniform(&mut seed);
        }
        for (c, ch) in self.ch.iter_mut().enumerate() {
            ch.rng = 0x9e37_79b9u32.wrapping_mul(c as u32 + 1).wrapping_add(17);
        }
        self.dev_prev = 0.0;
        // As in the C++, `wow_wobble`, `coef_counter`, the bypass write index and the meters
        // carry over a prepare.
    }

    /// Rust addition for `Plugin::reset`: clears what a prepare carries over (the C++ has no
    /// reset; JUCE's default `reset()` does nothing), then prepares again at the same rate.
    /// SoundCraft resets plugins on seek to kill tails, so a reset must leave nothing of the old
    /// material in the delay lines, filters or oxide; re-preparing (which only reuses the lines'
    /// capacity at the same rate) is the faithful way to land in the C++'s fresh-prepare state.
    pub fn reset(&mut self) {
        for c in &mut self.ch {
            for s in &mut c.os {
                s.reset();
            }
            c.reset_state();
            c.byp_w = 0;
        }
        self.wow_wobble = 0.0;
        self.coef_counter = 0;
        self.in_peak = 0.0;
        self.out_peak = 0.0;
        if self.fs_base > 0.0 {
            self.prepare(self.fs_base);
        }
    }

    pub fn get_oversampling(&self) -> i32 {
        self.os_factor
    }
    pub fn get_latency_samples(&self) -> i32 {
        self.latency
    }

    /// Peak levels for the meters, since the last call.
    pub fn take_input_peak(&mut self) -> f32 {
        std::mem::take(&mut self.in_peak)
    }
    pub fn take_output_peak(&mut self) -> f32 {
        std::mem::take(&mut self.out_peak)
    }

    /// The current drive knob value (for the drive readout).
    pub fn drive_knob(&self) -> f32 {
        self.drive_knob
    }
    /// The current Wow & Flutter knob value.
    pub fn flutter_knob(&self) -> f32 {
        self.flutter_knob
    }
    /// The current machine index.
    pub fn machine(&self) -> i32 {
        self.machine
    }

    /// Test hook: (pointer, capacity) of every delay line, to prove `process` never reallocates.
    #[cfg(test)]
    pub(crate) fn buffer_ptrs(&self) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        for c in &self.ch {
            v.push((c.wet_line.as_ptr() as usize, c.wet_line.capacity()));
            v.push((c.dry_line.as_ptr() as usize, c.dry_line.capacity()));
            v.push((c.byp_line.as_ptr() as usize, c.byp_line.capacity()));
        }
        v
    }

    // ---------------------------------------------------------------- audio
    /// `num_channels` planar buffers of at least `num_samples` frames (the caller guarantees it).
    pub fn process(&mut self, channels: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        let nc = num_channels.min(2).min(channels.len());
        if nc == 0 {
            return; // the C++ would read channels[0] regardless; a host never passes none
        }
        let num_samples = channels.iter().take(nc).map(Vec::len).fold(num_samples, usize::min);

        for n in 0..num_samples {
            // ---- smoothing, once per base-rate sample
            let s = self.smooth;
            glide(s, &mut self.drive_sm, f64::from(self.drive_knob));
            glide(s, &mut self.bias_sm, f64::from(self.bias_knob));
            glide(s, &mut self.flut_sm, f64::from(self.flutter_knob));
            let hiss_target = self.hiss_gain_target();
            glide(s, &mut self.hiss_sm, hiss_target);
            glide(s, &mut self.mix_sm, f64::from(self.mix_knob));
            let (out_target, ag_target) = (self.out_gain_target(), self.auto_gain_target());
            glide(s, &mut self.out_sm, f64::from(out_target));
            glide(s, &mut self.ag_sm, f64::from(ag_target));
            glide(s, &mut self.byp_sm, if self.bypassed { 1.0 } else { 0.0 });

            self.coef_counter += 1;
            if self.coef_counter >= 16 {
                self.coef_counter = 0;
                self.update_bias_shelf(self.bias_sm as f32);
            }

            let drive_gain = 10.0f64.powf(f64::from(knob_to_drive_db(self.drive_sm as f32)) / 20.0);
            let bias = f64::from(knob_to_bias(self.bias_sm as f32));

            // ---- the transport: one speed error, shared by both channels -- they are on the same tape
            let dev = self.transport_deviation(self.flut_sm);

            // ---- bias: the reversible fraction, and the trim that keeps small signals at unity
            let mut ja = self.ja_spec();
            ja.c = if bias >= 0.0 { ja.c + bias * (0.97 - ja.c) } else { ja.c + bias * (ja.c - 0.45) };
            ja.a *= if bias >= 0.0 { 1.0 + 0.35 * bias } else { 1.0 + 0.10 * bias };
            let out_norm = 1.0 / (slope_guess(&ja, ja.c) * self.bias_trim(bias));

            let sample = |c: usize, channels: &[Vec<f32>]| channels.get(c).and_then(|b| b.get(n)).copied().unwrap_or(0.0);
            let input = [sample(0, channels), if nc > 1 { sample(1, channels) } else { 0.0 }];
            self.in_peak = max_f(self.in_peak, max_f(input[0].abs(), input[1].abs()));

            for (c, &x_in) in input.iter().enumerate().take(nc) {
                let y = self.process_oversampled(c, x_in, drive_gain, &ja, out_norm, dev);

                // The bypass path is delayed by the reported latency too: the host has already
                // moved this track earlier by that much, so an undelayed bypass would jump.
                let (latency, byp_mask) = (self.latency, self.byp_mask);
                let s = self.ch_mut(c);
                if let Some(slot) = s.byp_line.get_mut((s.byp_w & byp_mask) as usize) {
                    *slot = x_in;
                }
                let late = s.byp_line.get((s.byp_w.wrapping_sub(latency) & byp_mask) as usize).copied().unwrap_or(0.0);
                s.byp_w = (s.byp_w + 1) & byp_mask; // wrapped: an int counter would overflow in hours

                let out = y * self.out_sm as f32;
                // Bypass crossfades rather than clicking, and once it has landed it is the
                // delayed input itself, exactly.
                let o = if self.byp_sm >= 1.0 { late } else { out + self.byp_sm as f32 * (late - out) };
                if let Some(slot) = channels.get_mut(c).and_then(|b| b.get_mut(n)) {
                    *slot = o;
                }
                self.out_peak = max_f(self.out_peak, o.abs());
            }

            self.dev_prev = dev;
        }
    }

    // ---------------------------------------------------------------- private helpers
    /// Machine and formula as table indices (always in range: `set_parameter` clamps them).
    #[inline]
    fn mi(&self) -> usize {
        self.machine.clamp(0, NUM_MACHINES - 1) as usize
    }
    #[inline]
    fn fi(&self) -> usize {
        self.formula.clamp(0, NUM_FORMULAS - 1) as usize
    }

    fn ja_spec(&self) -> JaSpec {
        if self.machine == M_CASSETTE { CASSETTE_OXIDE[self.fi()] } else { REEL_OXIDE[self.fi()] }
    }

    fn bias_trim(&self, bias: f64) -> f64 {
        let pos = (bias + 1.0) * 0.5 * (tapegain::K_BIAS_POINTS as f64 - 1.0);
        let i = clamp_t(pos as i32, 0, tapegain::K_BIAS_POINTS as i32 - 2) as usize;
        let t = pos - i as f64;
        let row = &self.trim[self.mi()][self.fi()];
        row[i] + t * (row[i + 1] - row[i])
    }

    /// Channel `ci` (always 0 or 1; anything else falls back to channel 0).
    #[inline]
    fn ch_mut(&mut self, ci: usize) -> &mut Channel {
        let [c0, c1] = &mut self.ch;
        if ci == 1 { c1 } else { c0 }
    }

    // ---------------------------------------------------------------- the oversampled path
    fn process_oversampled(&mut self, ci: usize, x_in: f32, drive_gain: f64, ja: &JaSpec, out_norm: f64, dev: f64) -> f32 {
        let n_os = self.os_factor.clamp(1, 8) as usize;
        let num_stages = self.num_stages.clamp(0, 3) as usize;
        let mix = self.mix_sm;
        let wet_gain = if self.auto_gain && !self.calibrating && tapegain::K_CALIBRATED { self.ag_sm } else { 1.0 };
        let hiss = self.hiss_sm;
        let in_scale = self.mol_scale[self.mi()][self.fi()];
        let valve_amp = self.machine == M_VALVE;
        let base_d = f64::from(self.base_delay_os);
        let base_delay_os = self.base_delay_os;
        let (valve_dc_r, dc_r, env_coef, noise_ref) = (self.valve_dc_r, self.dc_r, self.env_coef, self.noise_ref);
        let (dev_prev, os_fs, line_mask) = (self.dev_prev, self.os_fs, self.line_mask);
        let c = self.ch_mut(ci);

        // ---- up: one base-rate sample becomes N, stage 0 first
        let mut buf = [0.0f64; 8];
        let mut tmp = [0.0f64; 8];
        buf[0] = f64::from(x_in);
        let mut count = 1usize;
        for stage in c.os.iter_mut().take(num_stages) {
            for i in 0..count {
                let (a, b) = stage.up(buf[i]);
                tmp[2 * i] = a;
                tmp[2 * i + 1] = b;
            }
            count *= 2;
            buf[..count].copy_from_slice(&tmp[..count]);
        }

        for (k, slot) in buf.iter_mut().enumerate().take(n_os.min(count)) {
            let x = *slot;

            // ---- record side
            let mut u = x * drive_gain;
            if valve_amp {
                // A triode line stage ahead of the head: lopsided, so second harmonic, and
                // coupled through a capacitor, so its DC never reaches the oxide.
                const S_V: f64 = 0.35;
                let v = knee_clip(u * S_V, 0.9, 1.8, 2.0) / S_V;
                let hp = v - c.valve_dc_x + valve_dc_r * c.valve_dc_y;
                c.valve_dc_x = v;
                c.valve_dc_y = hp;
                u = hp;
            }
            for f in &mut c.pre {
                u = f.run(u);
            }

            let h = u * in_scale;
            let mut avg = 0.0;
            c.m = oxide(ja, c.h, h, c.m, &mut avg);
            c.h = h;
            let mut t = avg * out_norm / in_scale;

            // ---- on the tape: hiss, and modulation noise riding the signal. Shaped by the
            // record curve so the replay curve leaves it white, as tape hiss is after replay EQ.
            if hiss > 0.0 {
                c.env += env_coef * (t.abs() - c.env);
                // The C++ leaves the order of the two calls in a sum unspecified; a + b == b + a.
                let wn = (uniform(&mut c.rng) + uniform(&mut c.rng) - 1.0) * 2.449; // unit RMS, triangular
                let mn = (uniform(&mut c.rng) + uniform(&mut c.rng) - 1.0) * 2.449;
                let mut nz = hiss * wn + hiss * MOD_NOISE_RATIO * c.env * mn / noise_ref;
                for f in &mut c.noise_pre {
                    nz = f.run(nz);
                }
                t += nz;
            }

            // ---- the transport
            if let Some(s) = c.wet_line.get_mut((c.w & line_mask) as usize) {
                *s = t;
            }
            if let Some(s) = c.dry_line.get_mut((c.w & line_mask) as usize) {
                *s = x;
            }
            let d = base_d + (dev_prev + (dev - dev_prev) * (k as f64 + 1.0) / n_os as f64) * os_fs;
            let mut y = read_line(&c.wet_line, line_mask, c.w, d);
            let dry = c.dry_line.get((c.w.wrapping_sub(base_delay_os) & line_mask) as usize).copied().unwrap_or(0.0);
            c.w = (c.w + 1) & line_mask;

            // ---- replay side
            for f in c.de.iter_mut().rev() {
                y = f.run(y);
            }
            y = c.hf_loss.run(y);
            y = c.bias_shelf.run(y);
            y = c.bump.run(y);
            y = c.dip.run(y);
            y = c.lf_cut.run(y);

            let dc = y - c.dc_x + dc_r * c.dc_y;
            c.dc_x = y;
            c.dc_y = dc;
            let wet = dc / drive_gain * wet_gain;

            *slot = dry + mix * (wet - dry);
        }

        // ---- down: pairs, innermost stage first
        for stage in c.os.iter_mut().take(num_stages).rev() {
            count /= 2;
            for i in 0..count {
                buf[i] = stage.down(buf[2 * i], buf[2 * i + 1]);
            }
        }

        buf[0] as f32
    }

    // ---------------------------------------------------------------- the transport
    /// Delay excursion in seconds, built in the DELAY domain: each component is a speed deviation
    /// of A (fraction) at rate f, which is a delay of A / (2 pi f).
    fn transport_deviation(&mut self, knob: f64) -> f64 {
        let m = &MACHINE_SPECS[self.mi()];
        let ips = speed_ips(self.machine, self.speed);
        let fw = ips / (PI * m.reel_in); // one turn of the reel
        let ff = ips / (PI * m.capstan_in); // one turn of the capstan

        const RATE_MUL: [f64; 7] = [1.0, 1.63, 1.0, 2.0, 0.071, 0.13, 0.21];
        let rates = [fw, fw * RATE_MUL[1], ff, ff * RATE_MUL[3], RATE_MUL[4], RATE_MUL[5], RATE_MUL[6]];
        for (ph, rate) in self.transport_phase.iter_mut().zip(rates) {
            *ph += 2.0 * PI * rate / self.fs_base;
            if *ph > 2.0 * PI {
                *ph -= 2.0 * PI;
            }
        }
        self.wow_wobble += 2.0 * PI * 0.09 / self.fs_base;
        if self.wow_wobble > 2.0 * PI {
            self.wow_wobble -= 2.0 * PI;
        }

        if knob <= 0.0 {
            return 0.0;
        }

        let wow_a = m.wow_pct * 0.01;
        let fl_a = m.flut_pct * 0.01;
        let ph = &self.transport_phase;
        let mut d = wow_a / (2.0 * PI * fw) * (1.0 + 0.25 * self.wow_wobble.sin()) * ph[0].sin()
            + 0.35 * wow_a / (2.0 * PI * rates[1]) * ph[1].sin()
            + fl_a / (2.0 * PI * ff) * ph[2].sin()
            + 0.4 * fl_a / (2.0 * PI * rates[3]) * ph[3].sin()
            + m.drift_ms * 0.001 * (0.5 * ph[4].sin() + 0.3 * ph[5].sin() + 0.2 * ph[6].sin());
        d *= knob;

        // The line holds K_LATENCY_MS either side of centre; a soft limit keeps the read point
        // inside it, with room for the interpolator's four taps.
        let lim = self.max_dev_sec;
        lim * (d / lim).tanh()
    }

    // ---------------------------------------------------------------- targets
    fn out_gain_target(&self) -> f32 {
        10.0f32.powf(knob_to_out_db(self.out_knob) / 20.0)
    }

    /// Linear hiss level to inject, RMS, for the knob and the machine.
    fn hiss_gain_target(&self) -> f64 {
        if self.hiss_knob < 0.001 {
            return 0.0;
        }
        10.0f64.powf(f64::from(knob_to_hiss_db(self.hiss_knob)) / 20.0) * self.noise_ref * self.noise_trim[self.mi()][self.fi()][self.speed_slot()]
    }

    fn speed_slot(&self) -> usize {
        if self.machine == M_CASSETTE { 1 } else { self.speed.clamp(0, NUM_SPEEDS - 1) as usize }
    }

    fn snr_db(&self) -> f64 {
        if self.machine == M_CASSETTE {
            return CASSETTE_VOICING[self.fi()].snr_db;
        }
        // C++ `machineSpecs[machine].snrDb[speed]` (speed, not speedSlot; the same on the reels).
        MACHINE_SPECS[self.mi()].snr_db[self.speed.clamp(0, NUM_SPEEDS - 1) as usize] + REEL_SNR_OFFSET[self.fi()]
    }

    fn auto_gain_target(&self) -> f32 {
        if !tapegain::K_CALIBRATED {
            return 1.0;
        }
        let plane = &tapegain::TABLE[self.mi()][self.fi()][self.speed_slot()];
        let bp = self.bias_knob * (tapegain::K_BIAS_POINTS as f32 - 1.0);
        let bi = clamp_t(bp as i32, 0, tapegain::K_BIAS_POINTS as i32 - 2) as usize;
        let bt = bp - bi as f32;
        let dp = self.drive_knob * (tapegain::K_POINTS as f32 - 1.0);
        let di = clamp_t(dp as i32, 0, tapegain::K_POINTS as i32 - 2) as usize;
        let dt = dp - di as f32;
        let at = |b: usize| plane[b][di] + dt * (plane[b][di + 1] - plane[b][di]);
        at(bi) + bt * (at(bi + 1) - at(bi))
    }

    // ---------------------------------------------------------------- rebuilds
    /// How late the oversampling filters make everything, in base-rate samples, per Quality: the
    /// centroid of the up-and-down impulse response. The transport gives that much back, so wet,
    /// dry and bypass all land on exactly K_LATENCY_MS.
    fn measure_halfband_delays(&mut self) {
        for q in 0..3usize {
            let stages = q + 1;
            let mut st = [Stage::default(); 3];
            for (i, s) in st.iter_mut().enumerate().take(stages) {
                s.set(&self.stage_coefs[i]);
            }

            let mut num = 0.0;
            let mut den = 0.0;
            for n in 0..512i32 {
                let mut buf = [0.0f64; 8];
                let mut tmp = [0.0f64; 8];
                buf[0] = if n == 0 { 1.0 } else { 0.0 };
                let mut count = 1usize;
                for s in st.iter_mut().take(stages) {
                    for j in 0..count {
                        let (a, b) = s.up(buf[j]);
                        tmp[2 * j] = a;
                        tmp[2 * j + 1] = b;
                    }
                    count *= 2;
                    buf[..count].copy_from_slice(&tmp[..count]);
                }
                for s in st.iter_mut().take(stages).rev() {
                    count /= 2;
                    for j in 0..count {
                        buf[j] = s.down(buf[2 * j], buf[2 * j + 1]);
                    }
                }
                num += f64::from(n) * buf[0];
                den += buf[0];
            }
            self.hb_delay[q] = if den != 0.0 { num / den } else { 0.0 };
        }
    }

    fn update_bias_shelf(&mut self, knob: f32) {
        // Over-bias erases the shortest wavelengths as it records them; under-bias leaves the top
        // a little hotter. Worse the slower the tape, where those wavelengths are shorter.
        let b = f64::from(knob_to_bias(knob));
        let ips = speed_ips(self.machine, self.speed);
        let slow = clamp_t(15.0 / ips, 0.5, 8.0);
        let fc = clamp_t(8000.0 / slow.sqrt(), 3000.0, 14000.0);
        let over_db = -4.0 * slow.sqrt();
        let under_db = 1.5;
        let g_db = if b >= 0.0 { b * over_db } else { -b * under_db };
        let g = 10.0f64.powf(g_db / 20.0);
        let os_fs = self.os_fs;
        // A high shelf: DC 1, top g. As zero over pole: fp / fz = g.
        for c in &mut self.ch {
            if g_db.abs() < 1.0e-4 {
                c.bias_shelf.b0 = 1.0;
                c.bias_shelf.b1 = 0.0;
                c.bias_shelf.a1 = 0.0;
                continue;
            }
            if g >= 1.0 {
                c.bias_shelf.set(fc, fc * g, os_fs, false, false);
            } else {
                c.bias_shelf.set(fc, fc / g, os_fs, false, true); // a cut: the inverse of a 1/g boost
            }
        }
    }

    fn update_voicing(&mut self) {
        let m = &MACHINE_SPECS[self.mi()];
        let ips = speed_ips(self.machine, self.speed);
        let s = self.speed_slot();
        let cass = self.machine == M_CASSETTE;
        let fi = self.fi();
        let os_fs = self.os_fs;

        let emph_us = if cass { CASSETTE_VOICING[fi].emph_us } else { m.emph_us[s] };
        let lf_us = m.lf_us[s];
        let hf_hz = if cass { CASSETTE_VOICING[fi].hf_hz } else { m.hf_hz[s] * REEL_HF_SCALE[fi] };

        for c in &mut self.ch {
            // Record: LF cut below the 3180 us corner (NAB and cassette only), capped 20 dB down;
            // HF emphasis from the standard's time constant, capped at 40 kHz; and the loss
            // compensation. Replay is each one's exact inverse.
            let f_lf = if lf_us > 0.0 { 1.0e6 / (2.0 * PI * lf_us) } else { 0.0 };
            let f_hf = 1.0e6 / (2.0 * PI * emph_us);
            let f_loss = m.loss_hz[s];
            let g_loss = 10.0f64.powf(m.loss_db[s] / 20.0);

            let set_pair = |pre: &mut Tilt1, de: &mut Tilt1, np: &mut Tilt1, fz: f64, fp: f64, norm_top: bool| {
                pre.set(fz, fp, os_fs, norm_top, false);
                np.set(fz, fp, os_fs, norm_top, false);
                de.set(fz, fp, os_fs, norm_top, true);
            };

            if f_lf > 0.0 {
                set_pair(&mut c.pre[0], &mut c.de[0], &mut c.noise_pre[0], f_lf * 0.1, f_lf, true);
            } else {
                c.pre[0] = Tilt1::default();
                c.de[0] = Tilt1::default();
                c.noise_pre[0] = Tilt1::default();
            }
            set_pair(&mut c.pre[1], &mut c.de[1], &mut c.noise_pre[1], f_hf, max_d(40000.0, f_hf * 1.5), false);
            set_pair(&mut c.pre[2], &mut c.de[2], &mut c.noise_pre[2], f_loss, f_loss * g_loss, false);

            // Replay losses and the head.
            c.hf_loss.copy_coefs(&lowpass(hf_hz, os_fs, 0.65));
            let f0 = m.bump_hz_per_ips * ips;
            c.bump.copy_coefs(&peaking(f0, os_fs, 1.2, m.bump_db));
            c.dip.copy_coefs(&peaking(f0 * 2.2, os_fs, 1.4, m.dip_db));
            c.lf_cut.copy_coefs(&highpass(m.lf_hz_per_ips * ips, os_fs, 0.6));
        }

        self.noise_ref = 10.0f64.powf(-(18.0 + self.snr_db()) / 20.0);
        self.update_bias_shelf(self.bias_sm as f32);
    }

    fn voicing_changed(&mut self) {
        // A new machine, tape or speed only changes coefficients. The state -- the oxide's
        // magnetisation, what is on the tape between the heads -- carries on.
        if self.fs_base > 0.0 {
            self.update_voicing();
        }
    }

    fn rebuild_oversampling(&mut self) {
        const FACTORS: [i32; 3] = [2, 4, 8];
        let q = self.quality.clamp(0, 2) as usize;
        self.os_factor = FACTORS[q];
        self.os_fs = self.fs_base * f64::from(self.os_factor);
        self.num_stages = if self.os_factor == 2 {
            1
        } else if self.os_factor == 4 {
            2
        } else {
            3
        };

        let coefs = self.stage_coefs;
        for c in &mut self.ch {
            for (st, stage) in c.os.iter_mut().enumerate() {
                stage.set(&coefs[st]);
            }
            c.reset_state();
        }

        self.dc_r = (-2.0 * PI * 5.0 / self.os_fs).exp();
        self.valve_dc_r = (-2.0 * PI * 8.0 / self.os_fs).exp();
        self.env_coef = 1.0 - (-1.0 / (0.005 * self.os_fs)).exp();
        // The transport's centre, in oversampled samples: the reported latency less what the
        // oversampling filters already take. Whole samples, so the still transport is still an
        // exact delay.
        let os = f64::from(self.os_factor);
        self.base_delay_os = 8.max(lround(f64::from(self.latency) * os - self.hb_delay[q] * os));
        self.max_dev_sec = (f64::from(self.base_delay_os) - 4.0) / self.os_fs;
        self.update_voicing();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_is_two_ms_and_the_line_holds_it_at_every_quality() {
        let mut e = TapeEngineDsp::new();
        e.prepare(48_000.0);
        assert_eq!(e.get_latency_samples(), 96);
        for q in 0..3 {
            e.set_parameter(P_QUALITY, q as f32);
            assert_eq!(e.get_oversampling(), [2, 4, 8][q]);
            assert!(e.base_delay_os >= 8 && e.base_delay_os < e.line_mask, "{}", e.base_delay_os);
        }
    }

    #[test]
    fn still_transport_dry_mix_is_a_pure_delay_of_the_latency() {
        // mix 0: the dry path, delayed by exactly the reported latency (to within the half-band
        // filters, which are not linear phase; a low tone shows the alignment).
        let mut e = TapeEngineDsp::new();
        e.set_parameter(P_MIX, 0.0);
        e.prepare(48_000.0);
        let len = 4800;
        let x: Vec<f32> = (0..len).map(|i| (std::f32::consts::TAU * 100.0 * i as f32 / 48_000.0).sin() * 0.5).collect();
        let mut io = vec![x.clone(), x.clone()];
        e.process(&mut io, 2, len);
        let lat = e.get_latency_samples() as usize;
        let err: f32 = (2400..len).map(|i| (io[0][i] - x[i - lat]).abs()).fold(0.0, f32::max);
        assert!(err < 2e-3, "{err}");
    }

    #[test]
    fn flutter_percent_matches_the_formula() {
        // Multitrack at full knob: 1.6 * 0.08 + 1.4 * 0.05 + drift
        let drift = 0.20 * 1.0e-3 * 2.0 * PI * (0.5 * 0.071 + 0.3 * 0.13 + 0.2 * 0.21) * 100.0;
        assert!((flutter_percent(M_MULTITRACK, 1.0) - (0.128 + 0.07 + drift)).abs() < 1e-12);
        assert_eq!(flutter_percent(M_CASSETTE, 0.0), 0.0);
    }
}
