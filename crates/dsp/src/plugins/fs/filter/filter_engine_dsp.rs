//! FILTER ENGINE -- nine component-level filter circuits under one selector, one shared
//! modulation section. Line-by-line translation of `Engine 5 - Filter/Source/FilterEngineDSP.h`
//! (class `fs::filter::FilterEngineDSP`, in the header's order).
//!
//! ```text
//!   0 Q-Tron+      op-amp state-variable, opto (LDR) tuned, +18 V rail        (values from the New Sensor drawing)
//!   1 Mu-Tron III  the same topology on +/-9 V with a slower, rounder opto
//!   2 Cry Baby     inductor wah: Q1 gain stage (Ebers-Moll), 500 mH tank, 10n Miller cap through the pot, core saturation
//!   3 Vox          V846 / Clyde McCoy values in the same nodal solve
//!   4 Moog         4-pole transistor ladder, tanh differential pairs, implicit trapezoidal + Newton
//!   5 MS-20        Korg35 Sallen-Key (12 dB LP, 6 dB HP) with the diode limiter solved inside the feedback loop
//!   6 SEM          Oberheim 12 dB OTA state-variable
//!   7 TB-303       4-pole diode ladder (half-size first cap, coupled stages), AC-coupled feedback
//!   8 Wasp         CMOS-inverter state-variable: OTA inputs, asymmetric inverter saturation, dirty Q path
//! ```
//!
//! Mod sources: Manual, Envelope, LFO (free or tempo-synced), Step sequencer (tempo-synced,
//! 2..16 steps). Zero latency: IIR only, 4x oversampled cores, dry/wet mixed at the oversampled
//! rate so Mix never combs. Only the selected circuit runs.
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - `float` and `double` exactly where the C++ has them: the smoothers, knob maps, modulation
//!   and the up/down biquads are `f32` (`std::pow (10.0f, x)` is `powf`, `std::sin (float)` is
//!   `sinf`); the circuit solvers, coefficients and phases are `f64`. Casts sit where the C++
//!   casts. Expressions keep the C++ evaluation order (left to right), so `a - b + c` is
//!   `(a - b) + c` here too.
//! - `std::max` / `std::min` and the header's `clampT` are the C++ comparisons ([`cmax`],
//!   [`cmin`], [`clamp_t`]), so a NaN goes the way it goes there rather than being dropped like
//!   Rust's `max`.
//! - `int (v + 0.5f)` is `(v + 0.5) as i32` (truncation; the knob is always in range here).
//!   `nextRandom`'s `std::int64_t` LCG wraps on overflow instead of being undefined (its values
//!   stay below 2^62, so it never does).
//! - `isBadSample` tests the exponent bits, as the C++ does (it was written that way because
//!   `/fp:fast` would compile a comparison-based NaN test away).
//! - The `std::atomic<float>` UI outputs (`cutoffOut`, `modOut`, `bpmOut`, `stepOut`) are plain
//!   fields read through accessors; the host reads them between blocks.
//! - `processChannel (ci, ch[ci], ...)` / `coreSample` mutate one channel while reading the
//!   engine's scalars: here the channel is copied out, processed and written back (the same reads
//!   and writes, in the same order).

// Constants and literals are kept exactly as the C++ writes them.
#![allow(clippy::excessive_precision, clippy::approx_constant)]
// The C++ clamps and branches are kept as the C++ writes them.
#![allow(clippy::manual_clamp, clippy::collapsible_else_if, clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]

// ---------------------------------------------------------------- C++ comparison semantics
/// `std::max (a, b)`: `(a < b) ? b : a`.
#[inline]
fn cmax<T: PartialOrd>(a: T, b: T) -> T {
    if a < b { b } else { a }
}
/// `std::min (a, b)`: `(b < a) ? b : a`.
#[inline]
fn cmin<T: PartialOrd>(a: T, b: T) -> T {
    if b < a { b } else { a }
}
/// The header's `clampT (v, lo, hi)`: `v < lo ? lo : (v > hi ? hi : v)`.
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

// ------------------------------------------------------------------ parameters
// Declared in this order -> param1..param52. `Parameters.h` (and the module's table) must match.
pub const P_CUTOFF: usize = 0;
pub const P_RESO: usize = 1;
pub const P_DRIVE: usize = 2;
pub const P_MIX: usize = 3;
pub const P_LEVEL: usize = 4;
pub const P_CIRCUIT: usize = 5;
pub const P_BYPASS: usize = 6;
pub const P_TRIM: usize = 7;
pub const P_MOD_SRC: usize = 8;
pub const P_DEPTH: usize = 9;
pub const P_ENV_SENS: usize = 10;
pub const P_ENV_ATK: usize = 11;
pub const P_ENV_REL: usize = 12;
pub const P_LFO_SHAPE: usize = 13;
pub const P_LFO_SYNC: usize = 14;
pub const P_LFO_RATE: usize = 15;
pub const P_NOTE: usize = 16;
pub const P_LFO_STEREO: usize = 17;
pub const P_SEQ_STEPS: usize = 18;
pub const P_SEQ_GLIDE: usize = 19;
pub const P_TEMPO_SRC: usize = 20;
pub const P_BPM: usize = 21;
pub const P_STEP1: usize = 22;
pub const P_STEP16: usize = 37;
pub const P_QT_RANGE: usize = 38;
pub const P_QT_MODE: usize = 39;
pub const P_MU_RANGE: usize = 40;
pub const P_MU_MODE: usize = 41;
pub const P_CB_INDUCTOR: usize = 42;
pub const P_CB_VOICE: usize = 43;
pub const P_VOX_VOICE: usize = 44;
pub const P_MOOG_SLOPE: usize = 45;
pub const P_MOOG_COMP: usize = 46;
pub const P_MS_MODE: usize = 47;
pub const P_SEM_MODE: usize = 48;
pub const P_TB_MOD: usize = 49;
pub const P_WASP_MODE: usize = 50;
pub const P_ENV_SRC: usize = 51;
pub const NUM_PARAMS: usize = 52;

const PI: f64 = 3.14159265358979323846;
const TWO_PI: f64 = 6.28318530717958647692;

// ------------------------------------------------------------------ constants
const VT: f64 = 25.85e-3;

// Q-Tron+ (New Sensor drawing): integrators 12k + (LDR || 220k), 1.8n Hi / 1.8n + 2.2n Lo, +18 V rail, 9 V bias
const QT_RSER: f64 = 12000.0;
const QT_RSHUNT: f64 = 220000.0;
const QT_C_HI: f64 = 1.8e-9;
const QT_C_LO: f64 = 4.0e-9;
const QT_RAIL: f64 = 7.5;
const QT_LDR_BRIGHT: f64 = 8100.0; // bright value sets the 4.4 kHz factory alignment point (Hi, BP)
const QT_LDR_GAMMA: f64 = 0.80;
const QT_TAU_ON: f64 = 0.004;
const QT_TAU_OFF: f64 = 0.060;
const QT_Q_MIN: f64 = 0.6;
const QT_Q_MAX: f64 = 14.0;
// Mu-Tron III (values from recollection -- verify against a trusted drawing)
const MU_RSER: f64 = 10000.0;
const MU_RSHUNT: f64 = 470000.0;
const MU_C_HI: f64 = 2.2e-9;
const MU_C_LO: f64 = 6.9e-9;
const MU_RAIL: f64 = 6.5;
const MU_LDR_BRIGHT: f64 = 10000.0;
const MU_LDR_GAMMA: f64 = 0.75;
const MU_TAU_ON: f64 = 0.009;
const MU_TAU_OFF: f64 = 0.130;
const MU_Q_MIN: f64 = 0.7;
const MU_Q_MAX: f64 = 10.0;
// wah (shared topology)
const WAH_R1: f64 = 68000.0;
const WAH_CIN: f64 = 10.0e-9;
const WAH_R15: f64 = 1500.0;
const WAH_RC: f64 = 22000.0;
const WAH_RPOT: f64 = 100000.0;
const WAH_VCC: f64 = 9.0;
const WAH_IQ: f64 = 0.22e-3;
const WAH_AF: f64 = 0.98;

// diode pair of the MS-20 limiter, i = 2 Is sinh (v / nVt)
const MS_IS: f64 = 2.0e-9;
const MS_NVT: f64 = 1.9 * 25.85e-3;

// ------------------------------------------------------------------ knob -> unit maps
// The UI has to print these same numbers. They live here, and only here, so a readout
// cannot drift away from what the DSP actually does.
pub fn knob_to_level_db(v: f32) -> f32 {
    -24.0 + 36.0 * clamp_t(v, 0.0, 1.0)
}
pub fn knob_to_trim_db(v: f32) -> f32 {
    -12.0 + 36.0 * clamp_t(v, 0.0, 1.0)
}
pub fn knob_to_drive_db(v: f32) -> f32 {
    24.0 * clamp_t(v, 0.0, 1.0)
}
pub fn knob_to_env_sens_db(v: f32) -> f32 {
    -6.0 + 46.0 * clamp_t(v, 0.0, 1.0)
}
pub fn knob_to_env_atk_sec(v: f32) -> f32 {
    0.001 * 150.0f32.powf(clamp_t(v, 0.0, 1.0))
}
pub fn knob_to_env_rel_sec(v: f32) -> f32 {
    0.020 * 100.0f32.powf(clamp_t(v, 0.0, 1.0))
}
pub fn knob_to_lfo_hz(v: f32) -> f32 {
    0.05 * 400.0f32.powf(clamp_t(v, 0.0, 1.0))
}
pub fn knob_to_bpm(v: f32) -> f32 {
    40.0 + 200.0 * clamp_t(v, 0.0, 1.0)
}
pub fn knob_to_step_count(v: f32) -> i32 {
    clamp_t((v + 0.5) as i32 + 2, 2, 16)
}
/// Mod Depth is bipolar: 0.5 is none, below that the sweep runs downwards.
pub fn knob_to_depth_bipolar(v: f32) -> f32 {
    2.0 * clamp_t(v, 0.0, 1.0) - 1.0
}

/// Cycle / step length in quarter-note beats, for the Note parameter.
pub fn note_to_beats(i: i32) -> f32 {
    const BEATS: [f32; 18] =
        [16.0, 8.0, 6.0, 4.0, 3.0, 8.0 / 3.0, 2.0, 1.5, 4.0 / 3.0, 1.0, 0.75, 2.0 / 3.0, 0.5, 0.375, 1.0 / 3.0, 0.25, 1.0 / 6.0, 0.125];
    BEATS[clamp_t(i, 0, 17) as usize]
}

/// See [`FilterEngineDsp::linear_coefs`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LinearCoefs {
    pub svf_damp: f64,
    pub svf_in_comp: f64,
    pub lad_k: f64,
    pub lad_in_comp: f64,
    pub ms_k: f64,
    pub gain: f64,
}

// ------------------------------------------------------------------ structs
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Channel {
    up: [Biquad; 4],
    down: [Biquad; 4],
    in_hp_x1: f32,
    in_hp_y1: f32,
    out_hp_x1: f32,
    out_hp_y1: f32,
    // SVF (Q-Tron, Mu-Tron, SEM, Wasp)
    sb: f64,
    sl: f64,
    lp: f64,
    bp: f64,
    hp: f64,
    // ladders (Moog, 303)
    y1: f64,
    y2: f64,
    y3: f64,
    y4: f64,
    s1: f64,
    s2: f64,
    s3: f64,
    s4: f64,
    fb_z: f64,
    // MS-20: LP section (a) and HP section (b)
    a_sw: f64,
    a_s2: f64,
    a_vd: f64,
    b_sw: f64,
    b_sq: f64,
    b_vd: f64,
    // wah
    w_u: f64,
    w_il: f64,
    w_vl: f64,
    w_eq_cf: f64,
    w_eq_cin: f64,
}

// ------------------------------------------------------------------ helpers
#[inline]
fn filter(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

fn butter_lp(fc: f64, fs: f64, q_: f64) -> Biquad {
    let k = (PI * fc / fs).tan();
    let a0 = 1.0 + k / q_ + k * k;
    let mut q = Biquad { b0: ((k * k) / a0) as f32, b1: ((2.0 * k * k) / a0) as f32, ..Biquad::default() };
    q.b2 = q.b0;
    q.a1 = ((2.0 * (k * k - 1.0)) / a0) as f32;
    q.a2 = ((1.0 - k / q_ + k * k) / a0) as f32;
    q
}

/// Linear to 80 % of the limit, then an exponential approach to it (C1-continuous): op-amp
/// rails, CMOS rails, saturation.
#[inline]
fn knee(v: f64, lim: f64) -> f64 {
    let a = v.abs();
    let k0 = 0.8 * lim;
    let w = 0.2 * lim;
    if a <= k0 {
        return v;
    }
    let y = k0 + w * (1.0 - (-(a - k0) / w).exp());
    if v > 0.0 { y } else { -y }
}

#[inline]
fn knee_d(v: f64, lim: f64) -> f64 {
    let a = v.abs();
    let k0 = 0.8 * lim;
    let w = 0.2 * lim;
    if a <= k0 {
        return 1.0;
    }
    (-(a - k0) / w).exp()
}

/// NaN, infinity, or a runaway. Tested on the bits rather than with comparisons, as the C++
/// does (its `/fp:fast` build would compile `!(o > -100 && o < 100)` away for a NaN).
#[inline]
fn is_bad_sample(v: f32) -> bool {
    let bits = v.to_bits();
    if (bits & 0x7f80_0000) == 0x7f80_0000 {
        return true; // NaN or infinity
    }
    v <= -100.0 || v >= 100.0
}

// ---------------------------------------------------------------- circuit cores (oversampled rate)

/// State-variable filter, zero-delay feedback, scalar Newton on the highpass node.
///   nl 0: op-amp integrators (linear) whose outputs run into the supply rails        (Q-Tron, Mu-Tron)
///   nl 1: OTA integrators -- tanh at each OTA input                                  (SEM)
///   nl 2: OTA inputs + CMOS inverters that saturate asymmetrically + dirty Q path    (Wasp)
fn svf_core(c: &mut Channel, x: f64, h: f64, d: f64, nl: i32, rail: f64) {
    let vr = if nl == 2 { 1.2 } else { 1.0 };
    let mut hp = (x - (d + h) * c.sb - c.sl) / (1.0 + d * h + h * h);
    let mut bp = 0.0;
    let mut lp = 0.0;
    let mut vb = 0.0;
    let mut vl = 0.0;

    for _it in 0..3 {
        let mut nin = hp;
        let mut dnin = 1.0;
        if nl != 0 {
            let t = (hp / vr).tanh();
            nin = vr * t;
            dnin = 1.0 - t * t;
        }
        vb = h * nin;
        let bp_lin = c.sb + vb;
        let mut dbp = h * dnin;
        if nl == 0 {
            bp = knee(bp_lin, rail);
            dbp *= knee_d(bp_lin, rail);
        } else if nl == 2 {
            let lim = if bp_lin > 0.0 { 2.3 } else { 1.9 };
            bp = knee(bp_lin, lim);
            dbp *= knee_d(bp_lin, lim);
        } else {
            bp = bp_lin;
        }

        let mut nb = bp;
        let mut dnb = 1.0;
        if nl != 0 {
            let t = (bp / vr).tanh();
            nb = vr * t;
            dnb = 1.0 - t * t;
        }
        vl = h * nb;
        let lp_lin = c.sl + vl;
        let mut dlp = h * dnb * dbp;
        if nl == 0 {
            lp = knee(lp_lin, rail);
            dlp *= knee_d(lp_lin, rail);
        } else if nl == 2 {
            let lim = if lp_lin > 0.0 { 2.3 } else { 1.9 };
            lp = knee(lp_lin, lim);
            dlp *= knee_d(lp_lin, lim);
        } else {
            lp = lp_lin;
        }

        let mut qb = bp;
        let mut dqb = 1.0;
        if nl == 2 {
            let t = (bp / 1.6).tanh();
            qb = 1.6 * t;
            dqb = 1.0 - t * t;
        }

        let f = hp - x + d * qb + lp;
        let fp = 1.0 + d * dqb * dbp + dlp;
        hp -= clamp_t(f / fp, -4.0, 4.0);
    }

    c.sb = bp + vb;
    c.sl = lp + vl;
    c.hp = hp;
    c.bp = bp;
    c.lp = lp;
}

/// Moog transistor ladder: C dVi/dt = (Ictl/2) (tanh (V(i-1)/2Vt) - tanh (Vi/2Vt)), voltages in
/// units of 2 Vt. Implicit trapezoidal; the Jacobian is bidiagonal plus the feedback corner,
/// solved in closed form.
fn moog_core(c: &mut Channel, u: f64, h: f64, k: f64) {
    let mut y1 = c.y1;
    let mut y2 = c.y2;
    let mut y3 = c.y3;
    let mut y4 = c.y4;

    for _it in 0..2 {
        let t0 = (u - k * y4).tanh();
        let t1 = y1.tanh();
        let t2 = y2.tanh();
        let t3 = y3.tanh();
        let t4 = y4.tanh();
        let r1 = y1 - c.s1 - h * (t0 - t1);
        let r2 = y2 - c.s2 - h * (t1 - t2);
        let r3 = y3 - c.s3 - h * (t2 - t3);
        let r4 = y4 - c.s4 - h * (t3 - t4);
        let g1 = h * (1.0 - t1 * t1);
        let g2 = h * (1.0 - t2 * t2);
        let g3 = h * (1.0 - t3 * t3);
        let g4 = h * (1.0 - t4 * t4);
        let j14 = h * k * (1.0 - t0 * t0);
        let a1 = -r1 / (1.0 + g1);
        let b1 = -j14 / (1.0 + g1);
        let a2 = (-r2 + g1 * a1) / (1.0 + g2);
        let b2 = g1 * b1 / (1.0 + g2);
        let a3 = (-r3 + g2 * a2) / (1.0 + g3);
        let b3 = g2 * b2 / (1.0 + g3);
        let d4 = (-r4 + g3 * a3) / ((1.0 + g4) - g3 * b3);
        y4 += d4;
        y3 += a3 + b3 * d4;
        y2 += a2 + b2 * d4;
        y1 += a1 + b1 * d4;
    }

    c.s1 = 2.0 * y1 - c.s1;
    c.s2 = 2.0 * y2 - c.s2;
    c.s3 = 2.0 * y3 - c.s3;
    c.s4 = 2.0 * y4 - c.s4;
    c.y1 = y1;
    c.y2 = y2;
    c.y3 = y3;
    c.y4 = y4;
}

/// 4x4 Gaussian elimination (the diode ladder Jacobian is tridiagonal plus a corner; pivots
/// stay positive).
fn solve4(a: &mut [f64; 16], b: &mut [f64; 4]) {
    for k in 0..4 {
        let p = a[k * 4 + k];
        for i in 0..4 {
            if i > k {
                let f = a[i * 4 + k] / p;
                if f != 0.0 {
                    for j in 0..4 {
                        a[i * 4 + j] -= f * a[k * 4 + j];
                    }
                    b[i] -= f * b[k];
                }
            }
        }
    }
    b[3] /= a[15];
    b[2] = (b[2] - a[11] * b[3]) / a[10];
    b[1] = (b[1] - a[6] * b[2] - a[7] * b[3]) / a[5];
    b[0] = (b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3]) / a[0];
}

/// Korg35 section. lowpass: R - C(to feedback) - buffer - R - C(to ground). highpass: same parts,
/// input fed through the caps. The feedback amp (gain K) drives a resistor into the diode pair;
/// the unknown is the diode voltage, so the amp output needed to get there is explicit and one
/// scalar Newton solves filter and limiter together.
fn korg35(sw: &mut f64, s2: &mut f64, vd_state: &mut f64, x: f64, h: f64, k: f64, highpass: bool) -> f64 {
    const RS: f64 = 4700.0;
    let h1 = 1.0 + h;
    let v2;
    let mut vd = 0.0;

    if k < 0.03 {
        if highpass {
            let v1 = *sw / h1;
            v2 = (*s2 + x + h * v1) / h1;
        } else {
            let w = (*sw + h * x) / h1;
            v2 = (*s2 + h * w) / h1;
        }
    } else {
        vd = *vd_state;
        let mut id = 0.0;
        for _it in 0..3 {
            let e = clamp_t(vd / MS_NVT, -30.0, 30.0).exp();
            let ei = 1.0 / e;
            id = MS_IS * (e - ei);
            let gd = MS_IS * (e + ei) / MS_NVT;
            let v = (vd + RS * id) / k;
            let vd_ = (1.0 + RS * gd) / k;
            let f = if highpass {
                let v1 = (*sw + vd) / h1;
                v * h1 - *s2 - x - h * v1
            } else {
                let v1 = (*sw + h * (x - vd)) / h1 + vd;
                v * h1 - *s2 - h * v1
            };
            let fp = vd_ * h1 - h / h1;
            let stp = clamp_t(f / fp, -0.25, 0.25);
            vd -= stp;
            id -= gd * stp; // first-order update so the committed current matches the final vd
        }
        v2 = (vd + RS * id) / k;
    }

    if highpass {
        let v1 = (*sw + vd) / h1;
        *sw = 2.0 * (v1 - vd) - *sw;
        *s2 = 2.0 * (v2 - x) - *s2;
    } else {
        let w = (*sw + h * (x - vd)) / h1;
        *sw = 2.0 * w - *sw;
        *s2 = 2.0 * v2 - *s2;
    }
    *vd_state = vd;
    knee(v2, 7.0) // output buffer headroom
}

/// The filter engine. `new` is the C++ constructor (`prepare (48000.0)` included).
#[derive(Debug, Clone)]
pub struct FilterEngineDsp {
    // ---------------------------------------------------------------- outputs back to the UI
    /// Live cutoff of the left channel, Hz.
    cutoff_out: f32,
    /// Current mod value, -1..1.
    mod_out: f32,
    /// Tempo in use; negative = host gave none, running on the BPM knob.
    bpm_out: f32,
    /// Current step index (0-based), -1 when Steps is not the source.
    step_out: f32,

    ch: [Channel; 2],

    // ---------------------------------------------------------------- parameter state
    cutoff_knob: f32,
    reso_knob: f32,
    drive_knob: f32,
    mix_knob: f32,
    level_knob: f32,
    circuit_idx: i32,
    bypassed: bool,
    trim_lin: f32,
    trim_sm: f32,
    level_lin: f32,
    level_sm: f32,
    cutoff_sm: f32,
    reso_sm: f32,
    drive_sm: f32,
    mix_sm: f32,

    mod_src_idx: i32,
    depth_bip: f32,
    env_sens_lin: f32,
    env_atk_coef: f32,
    env_rel_coef: f32,
    lfo_shape_idx: i32,
    lfo_synced: bool,
    lfo_free_hz: f32,
    note_idx: i32,
    lfo_stereo_amt: f32,
    step_count: i32,
    glide_amt: f32,
    tempo_src_idx: i32,
    manual_bpm: f32,
    step_val: [f32; 16],
    env_low_cut: bool,

    qt_range_idx: i32,
    qt_mode_idx: i32,
    mu_range_idx: i32,
    mu_mode_idx: i32,
    cb_inductor_idx: i32,
    cb_voice_idx: i32,
    vox_voice_idx: i32,
    moog_slope_idx: i32,
    moog_comp_idx: i32,
    ms_mode_idx: i32,
    sem_mode_idx: i32,
    tb_mod_idx: i32,
    wasp_mode_idx: i32,

    // host transport
    host_bpm: f32,
    host_playing: bool,

    // ---------------------------------------------------------------- mod state
    lfo_phase: f64,
    seq_phase: f64,
    sh_prev: f32,
    sh_cur: f32,
    seq_val: f32,
    env_state: f32,
    env_hp_x: f32,
    env_hp_y: f32,
    env_hp_coef: f32,
    rng: i64,
    pos_sm: [f32; 2],
    pos_opto: [f32; 2],
    pos_sm_coef: f32,
    opto_on_coef: f32,
    opto_off_coef: f32,
    ui_counter: i32,
    ui_slot: i32,
    mod_now: f32,
    bpm_now: f32,
    /// Rust addition: a transport reset left the modulation to seat (`seat_live`): the next
    /// transport (after its grid lock) or the next block seats the position, the opto cell and
    /// the sequencer value from the phases actually in force.
    seat_pending: bool,

    // ---------------------------------------------------------------- derived per circuit
    fs_base: f64,
    fs_os: f64,
    os_t: f64,
    /// Volts (or normalised units) at 0 dBFS in, and back out.
    in_v: f64,
    out_v: f64,
    drive_g: f64,
    drive_comp: f64,
    /// SVF circuits.
    svf_damp: f64,
    svf_in_comp: f64,
    /// Ladders.
    lad_k: f64,
    lad_in_comp: f64,
    /// MS-20.
    ms_k: f64,
    wah_re: f64,
    wah_beta: f64,
    wah_l0: f64,
    wah_rl: f64,
    wah_isat: f64,
    wah_irem: f64,
    wah_cf: f64,
    wah_rq: f64,
    wah_rc_eff: f64,
    wah_a: f64,
    wah_vdn: f64,
    wah_gcin: f64,
    wah_gin: f64,
    wah_gcf: f64,
    h_c: [f64; 2],
    h_c2: [f64; 2],
    alpha_c: [f64; 2],
    fc_disp: [f64; 2],
}

impl Default for FilterEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl FilterEngineDsp {
    /// The C++ constructor: the member initialisers, then `prepare (48000.0)`.
    pub fn new() -> Self {
        let mut s = FilterEngineDsp {
            cutoff_out: 1000.0,
            mod_out: 0.0,
            bpm_out: 120.0,
            step_out: -1.0,
            ch: [Channel::default(); 2],

            cutoff_knob: 0.5,
            reso_knob: 0.5,
            drive_knob: 0.0,
            mix_knob: 1.0,
            level_knob: 0.6667,
            circuit_idx: 0,
            bypassed: false,
            trim_lin: 1.0,
            trim_sm: 1.0,
            level_lin: 1.0,
            level_sm: 1.0,
            cutoff_sm: 0.5,
            reso_sm: 0.5,
            drive_sm: 0.0,
            mix_sm: 1.0,

            mod_src_idx: 1,
            depth_bip: 0.7,
            env_sens_lin: 4.17,
            env_atk_coef: 0.01,
            env_rel_coef: 0.0005,
            lfo_shape_idx: 0,
            lfo_synced: true,
            lfo_free_hz: 1.0,
            note_idx: 9,
            lfo_stereo_amt: 0.0,
            step_count: 8,
            glide_amt: 0.1,
            tempo_src_idx: 0,
            manual_bpm: 120.0,
            step_val: [0.0; 16],
            env_low_cut: false,

            qt_range_idx: 1,
            qt_mode_idx: 0,
            mu_range_idx: 1,
            mu_mode_idx: 0,
            cb_inductor_idx: 0,
            cb_voice_idx: 0,
            vox_voice_idx: 0,
            moog_slope_idx: 0,
            moog_comp_idx: 0,
            ms_mode_idx: 0,
            sem_mode_idx: 0,
            tb_mod_idx: 0,
            wasp_mode_idx: 0,

            host_bpm: 0.0,
            host_playing: false,

            lfo_phase: 0.0,
            seq_phase: 0.0,
            sh_prev: 0.0,
            sh_cur: 0.3,
            seq_val: 0.5,
            env_state: 0.0,
            env_hp_x: 0.0,
            env_hp_y: 0.0,
            env_hp_coef: 0.98,
            rng: 12345,
            pos_sm: [0.0; 2],
            pos_opto: [0.0; 2],
            pos_sm_coef: 0.02,
            opto_on_coef: 0.005,
            opto_off_coef: 0.0003,
            ui_counter: 0,
            ui_slot: 0,
            mod_now: 0.0,
            bpm_now: 120.0,
            seat_pending: false,

            fs_base: 48000.0,
            fs_os: 192000.0,
            os_t: 1.0 / 192000.0,
            in_v: 1.0,
            out_v: 1.0,
            drive_g: 1.0,
            drive_comp: 1.0,
            svf_damp: 1.0,
            svf_in_comp: 1.0,
            lad_k: 0.0,
            lad_in_comp: 1.0,
            ms_k: 0.0,
            wah_re: 390.0,
            wah_beta: 400.0,
            wah_l0: 0.5,
            wah_rl: 15.0,
            wah_isat: 1.5e-3,
            wah_irem: 0.0,
            wah_cf: 10.0e-9,
            wah_rq: 33000.0,
            wah_rc_eff: 18000.0,
            wah_a: 35.0,
            wah_vdn: 3.85,
            wah_gcin: 1.0,
            wah_gin: 1.0,
            wah_gcf: 1.0,
            h_c: [0.0; 2],
            h_c2: [0.0; 2],
            alpha_c: [0.0; 2],
            fc_disp: [0.0; 2],
        };
        s.prepare(48000.0);
        s
    }

    // ------------------------------------------------------------------ host transport
    /// Amorph fed a hidden 6-value packet; JUCE has AudioPlayHead, so the processor calls this
    /// instead. Unlike the other engines this one also needs the bar position: the LFO and the
    /// step sequencer are pulled back onto the host's grid, not just run at its tempo.
    pub fn set_host_transport(&mut self, playing: bool, bpm: f32, ppq: f64) {
        self.host_playing = playing;
        self.host_bpm = clamp_t(bpm, 0.0, 999.0);
        if self.seat_pending {
            // After a transport reset: snap onto the host's grid exactly (the phases were zeroed,
            // so the C++ tolerance would leave a small offset), then seat from there.
            self.lock_to_host_with(ppq, true);
            self.seat_modulation();
        } else {
            self.lock_to_host(ppq);
        }
    }

    pub fn get_host_bpm(&self) -> f32 {
        self.host_bpm
    }

    /// `cutoffOut`: the live cutoff of the left channel, Hz.
    pub fn cutoff_out(&self) -> f32 {
        self.cutoff_out
    }
    /// `modOut`: the current mod value, -1..1.
    pub fn mod_out(&self) -> f32 {
        self.mod_out
    }
    /// `bpmOut`: the tempo in use; negative = host gave none, running on the BPM knob.
    pub fn bpm_out(&self) -> f32 {
        self.bpm_out
    }
    /// `stepOut`: the current step index (0-based), -1 when Steps is not the source.
    pub fn step_out(&self) -> f32 {
        self.step_out
    }
    /// The sample rate given to `prepare`.
    pub fn sample_rate(&self) -> f64 {
        self.fs_base
    }
    /// The LFO phase, 0..1 (cycles).
    pub fn lfo_phase(&self) -> f64 {
        self.lfo_phase
    }
    /// The step sequencer phase, 0..step count (steps).
    pub fn seq_phase(&self) -> f64 {
        self.seq_phase
    }

    /// The cutoff in Hz the engine's own `updateChannel` gives pedal position `p` (0..1) for the
    /// circuit and switches set on this instance: the editor-side map from a knob to Hz (for a
    /// display), with no re-derivation. Use a scratch instance: it overwrites channel 0's
    /// coefficients.
    pub fn position_to_hz(&mut self, p: f64) -> f64 {
        self.update_channel(0, p);
        self.fc_disp[0]
    }

    /// The small-signal (linear) constants the engine's `updateShared` derives from Reso and
    /// Drive for the circuit set on this instance, for an editor-side response display: the
    /// SVF damping and input compensation, the ladder feedback and input compensation, the
    /// MS-20's K, and the linear gain `inV * driveG * outV / driveComp` the core sees in and
    /// out. Use a scratch instance: it overwrites the smoothed Reso and Drive.
    pub fn linear_coefs(&mut self, reso: f32, drive: f32) -> LinearCoefs {
        self.reso_sm = clamp_t(reso, 0.0, 1.0);
        self.drive_sm = clamp_t(drive, 0.0, 1.0);
        self.update_shared();
        LinearCoefs {
            svf_damp: self.svf_damp,
            svf_in_comp: self.svf_in_comp,
            lad_k: self.lad_k,
            lad_in_comp: self.lad_in_comp,
            ms_k: self.ms_k,
            gain: self.in_v * self.drive_g * self.out_v / self.drive_comp,
        }
    }

    // ------------------------------------------------------------------ events
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        if (P_STEP1..=P_STEP16).contains(&index) {
            self.step_val[index - P_STEP1] = v;
            return;
        }

        match index {
            P_CUTOFF => self.cutoff_knob = clamp_t(v, 0.0, 1.0),
            P_RESO => self.reso_knob = clamp_t(v, 0.0, 1.0),
            P_DRIVE => self.drive_knob = clamp_t(v, 0.0, 1.0),
            P_MIX => self.mix_knob = clamp_t(v, 0.0, 1.0),
            P_LEVEL => {
                self.level_knob = clamp_t(v, 0.0, 1.0);
                self.level_lin = 10.0f32.powf(knob_to_level_db(self.level_knob) / 20.0);
            }
            P_CIRCUIT => {
                self.circuit_idx = clamp_t((v + 0.5) as i32, 0, 8);
                self.apply_model();
                self.reset_states();
            }
            P_BYPASS => self.bypassed = v > 0.5,
            P_TRIM => self.trim_lin = 10.0f32.powf(knob_to_trim_db(v) / 20.0),

            P_MOD_SRC => self.mod_src_idx = clamp_t((v + 0.5) as i32, 0, 3),
            P_DEPTH => self.depth_bip = knob_to_depth_bipolar(v),
            P_ENV_SENS => self.env_sens_lin = 10.0f32.powf(knob_to_env_sens_db(v) / 20.0),
            P_ENV_ATK => self.env_atk_coef = self.time_coef(knob_to_env_atk_sec(v)),
            P_ENV_REL => self.env_rel_coef = self.time_coef(knob_to_env_rel_sec(v)),
            P_LFO_SHAPE => self.lfo_shape_idx = clamp_t((v + 0.5) as i32, 0, 6),
            P_LFO_SYNC => self.lfo_synced = v > 0.5,
            P_LFO_RATE => self.lfo_free_hz = knob_to_lfo_hz(v),
            P_NOTE => self.note_idx = clamp_t((v + 0.5) as i32, 0, 17),
            P_LFO_STEREO => self.lfo_stereo_amt = clamp_t(v, 0.0, 1.0),
            P_SEQ_STEPS => self.step_count = knob_to_step_count(v),
            P_SEQ_GLIDE => self.glide_amt = clamp_t(v, 0.0, 1.0),
            P_TEMPO_SRC => self.tempo_src_idx = clamp_t((v + 0.5) as i32, 0, 1),
            P_BPM => self.manual_bpm = knob_to_bpm(v),

            P_QT_RANGE => self.qt_range_idx = (v + 0.5) as i32,
            P_QT_MODE => self.qt_mode_idx = (v + 0.5) as i32,
            P_MU_RANGE => self.mu_range_idx = (v + 0.5) as i32,
            P_MU_MODE => self.mu_mode_idx = (v + 0.5) as i32,
            P_CB_INDUCTOR => {
                self.cb_inductor_idx = (v + 0.5) as i32;
                self.apply_model();
            }
            P_CB_VOICE => {
                self.cb_voice_idx = (v + 0.5) as i32;
                self.apply_model();
            }
            P_VOX_VOICE => {
                self.vox_voice_idx = (v + 0.5) as i32;
                self.apply_model();
            }
            P_MOOG_SLOPE => self.moog_slope_idx = (v + 0.5) as i32,
            P_MOOG_COMP => self.moog_comp_idx = (v + 0.5) as i32,
            P_MS_MODE => self.ms_mode_idx = (v + 0.5) as i32,
            P_SEM_MODE => self.sem_mode_idx = (v + 0.5) as i32,
            P_TB_MOD => self.tb_mod_idx = (v + 0.5) as i32,
            P_WASP_MODE => self.wasp_mode_idx = (v + 0.5) as i32,
            P_ENV_SRC => self.env_low_cut = v > 0.5,
            _ => {}
        }
    }

    // ------------------------------------------------------------------ init
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs_base = sample_rate;
        self.fs_os = self.fs_base * 4.0;
        self.os_t = 1.0 / self.fs_os;

        let fc = 0.4 * self.fs_base;
        for c in 0..2 {
            let fs_os = self.fs_os;
            let k = &mut self.ch[c];
            k.up[0] = butter_lp(fc, fs_os, 0.5098);
            k.up[1] = butter_lp(fc, fs_os, 0.6013);
            k.up[2] = butter_lp(fc, fs_os, 0.8999);
            k.up[3] = butter_lp(fc, fs_os, 2.5629);
            k.down[0] = butter_lp(fc, fs_os, 0.5098);
            k.down[1] = butter_lp(fc, fs_os, 0.6013);
            k.down[2] = butter_lp(fc, fs_os, 0.8999);
            k.down[3] = butter_lp(fc, fs_os, 2.5629);
            self.pos_sm[c] = 0.5;
            self.pos_opto[c] = 0.5;
        }

        self.step_val = [0.20, 0.80, 0.45, 0.65, 0.10, 0.90, 0.35, 0.55, 0.20, 0.80, 0.45, 0.65, 0.10, 0.90, 0.35, 0.55];

        self.pos_sm_coef = self.time_coef(0.0015);
        self.env_atk_coef = self.time_coef(knob_to_env_atk_sec(0.2));
        self.env_rel_coef = self.time_coef(knob_to_env_rel_sec(0.4));
        self.env_hp_coef = (-2.0 * PI * 250.0 / self.fs_base).exp() as f32;
        self.level_lin = 10.0f32.powf(knob_to_level_db(self.level_knob) / 20.0);
        self.level_sm = self.level_lin;
        self.trim_sm = self.trim_lin;

        self.apply_model();
        self.reset_states();
        self.update_shared();
    }

    /// Rust addition for the host's transport reset: every per-channel audio state (filter
    /// memories, the oversampling filters, the input and output high-passes) back to zero, before
    /// `prepare` (which rebuilds the oversampling designs and clears the solvers, but keeps the
    /// rest). Plain data, no allocation.
    pub fn clear_audio(&mut self) {
        self.ch = [Channel::default(); 2];
    }

    /// Rust addition for the host's transport reset, after `prepare` and the full push: what the
    /// C++ `prepare` leaves to history is put where a reset needs it, so two histories render the
    /// same and nothing glides. Cutoff, Reso, Drive and Mix sit at their knobs (the C++ `prepare`
    /// leaves them mid-glide); the modulation starts from rest (LFO and sequencer at phase 0, the
    /// random source reseeded, the envelope at 0, the sequencer value at its first step); the
    /// pedal position and the opto cell sit where that modulation puts them (the C++ `prepare`
    /// parks them at 0.5, from where they would sweep, the opto for about a second). The host
    /// tempo and play state are kept. Because a playing host's next `setHostTransport` moves the
    /// phases onto its grid, the seating is done again there, after an exact snap to the grid (or
    /// at the next block if no transport comes), so nothing travels from the phase-0 seat.
    pub fn seat_live(&mut self) {
        self.cutoff_sm = self.cutoff_knob;
        self.reso_sm = self.reso_knob;
        self.drive_sm = self.drive_knob;
        self.mix_sm = self.mix_knob;
        self.trim_sm = self.trim_lin;
        self.level_sm = self.level_lin;

        self.lfo_phase = 0.0;
        self.seq_phase = 0.0;
        self.sh_prev = 0.0;
        self.sh_cur = 0.3;
        self.rng = 12345;
        self.env_state = 0.0;
        self.env_hp_x = 0.0;
        self.env_hp_y = 0.0;
        self.seq_val = self.step_val[0];
        self.ui_counter = 0;
        self.ui_slot = 0;
        self.mod_now = 0.0;

        self.seat_modulation();
        self.seat_pending = true;
        self.update_shared();
    }

    /// Seats what the modulation drives at the phases in force (a transport reset's last step):
    /// the sequencer value at its step's value, and the pedal position and the opto cell where
    /// that modulation puts them, so nothing glides or sweeps towards them.
    fn seat_modulation(&mut self) {
        self.seat_pending = false;
        let idx = clamp_t(self.seq_phase as i32, 0, 15);
        self.seq_val = self.step_val[idx as usize];
        for c in 0..2 {
            let m = match self.mod_src_idx {
                2 => self.lfo_value(self.lfo_phase + if c == 1 { 0.5 * f64::from(self.lfo_stereo_amt) } else { 0.0 }),
                3 => 2.0 * self.seq_val - 1.0,
                _ => 0.0, // Manual, or the envelope at rest
            };
            let p = clamp_t(self.cutoff_sm + self.depth_bip * m, 0.0, 1.0);
            self.pos_sm[c] = p;
            self.pos_opto[c] = p;
        }
    }

    /// The sequencer value and the value of the step it is on (tests).
    #[cfg(test)]
    pub(crate) fn seq_value(&self) -> (f32, f32) {
        (self.seq_val, self.step_val[clamp_t(self.seq_phase as i32, 0, 15) as usize])
    }

    /// The smoothers and the values they head for (tests): trim, level, cutoff, reso, drive, mix.
    #[cfg(test)]
    pub(crate) fn smoothers(&self) -> ([f32; 6], [f32; 6]) {
        (
            [self.trim_sm, self.level_sm, self.cutoff_sm, self.reso_sm, self.drive_sm, self.mix_sm],
            [self.trim_lin, self.level_lin, self.cutoff_knob, self.reso_knob, self.drive_knob, self.mix_knob],
        )
    }

    /// The host transport the engine holds (tests): playing, bpm.
    #[cfg(test)]
    pub(crate) fn host_transport(&self) -> (bool, f32) {
        (self.host_playing, self.host_bpm)
    }

    /// The pedal position and opto cell per channel (tests).
    #[cfg(test)]
    pub(crate) fn positions(&self) -> ([f32; 2], [f32; 2]) {
        (self.pos_sm, self.pos_opto)
    }

    /// Back to exactly what `new` makes, in place, keeping the host transport (the host's, not
    /// the engine's): the fresh-instance path of a reset before any audio.
    pub fn restore_initial(&mut self) {
        let (playing, bpm) = (self.host_playing, self.host_bpm);
        *self = FilterEngineDsp::new();
        self.host_playing = playing;
        self.host_bpm = bpm;
    }

    // ------------------------------------------------------------------ main
    /// `process (channels, numChannels, numSamples)`: `io[0]` is left, `io[1]` (when
    /// `num_channels > 1`) right. With one channel the right channel runs on a copy of the left
    /// and only the left is written. At most `min(frames, len)` samples of each channel.
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        if num_channels < 1 || io.is_empty() {
            return;
        }
        if self.seat_pending {
            // No transport came after the reset: seat from the phases as they are.
            self.seat_modulation();
        }
        let stereo = num_channels > 1 && io.len() > 1;
        let mut n_max = num_samples.min(io[0].len());
        if stereo {
            n_max = n_max.min(io[1].len());
        }

        for n in 0..n_max {
            let x0 = io[0][n];
            let x1 = if stereo { io[1][n] } else { x0 };

            self.trim_sm += (self.trim_lin - self.trim_sm) * 0.005;
            self.level_sm += (self.level_lin - self.level_sm) * 0.005;
            self.cutoff_sm += (self.cutoff_knob - self.cutoff_sm) * 0.004;
            self.reso_sm += (self.reso_knob - self.reso_sm) * 0.004;
            self.drive_sm += (self.drive_knob - self.drive_sm) * 0.004;
            self.mix_sm += (self.mix_knob - self.mix_sm) * 0.004;

            self.update_shared();
            self.run_mod(0.5 * (x0 + x1) * self.trim_sm);

            let mut y0 = x0;
            let mut y1 = x1;
            if !self.bypassed {
                let nz = self.next_random() * 1.0e-7;
                let mut c0 = self.ch[0];
                y0 = self.process_channel(0, &mut c0, x0, nz);
                self.ch[0] = c0;
                let mut c1 = self.ch[1];
                y1 = self.process_channel(1, &mut c1, x1, nz);
                self.ch[1] = c1;
            }

            io[0][n] = y0;
            if stereo {
                io[1][n] = y1;
            }

            // UI feedback, one value at a time so nothing coalesces
            self.ui_counter += 1;
            if self.ui_counter >= 480 {
                self.ui_counter = 0;
                if self.ui_slot == 0 {
                    self.cutoff_out = self.fc_disp[0] as f32;
                } else if self.ui_slot == 1 {
                    self.mod_out = self.mod_now;
                } else if self.ui_slot == 2 {
                    self.bpm_out = if self.tempo_src_idx == 0 && self.host_bpm < 20.0 { -self.bpm_now } else { self.bpm_now };
                } else {
                    self.step_out = if self.mod_src_idx == 3 { clamp_t(self.seq_phase as i32, 0, 15) as f32 } else { -1.0 };
                }
                self.ui_slot = (self.ui_slot + 1) % 4;
            }
        }
    }

    // keep the LFO and the step sequencer on the host's grid while it plays
    fn lock_to_host(&mut self, ppq: f64) {
        self.lock_to_host_with(ppq, false);
    }

    /// `lockToHost`; `snap` (Rust addition, after a transport reset only) moves the phases onto the
    /// grid whatever the tolerance.
    fn lock_to_host_with(&mut self, ppq: f64, snap: bool) {
        if !self.host_playing || self.tempo_src_idx != 0 || self.host_bpm < 20.0 || ppq < 0.0 {
            return;
        }
        let beats = f64::from(note_to_beats(self.note_idx));

        if self.lfo_synced {
            let target = ppq / beats - (ppq / beats).floor();
            let mut diff = target - self.lfo_phase;
            diff -= (diff + 0.5).floor();
            if snap || diff.abs() > 0.02 {
                self.lfo_phase = target;
            }
        }

        let n = f64::from(self.step_count);
        let sp = ppq / beats;
        let target2 = sp - (sp / n).floor() * n;
        let mut diff2 = target2 - self.seq_phase;
        diff2 -= (diff2 / n + 0.5).floor() * n;
        if snap || diff2.abs() > 0.05 {
            self.seq_phase = target2;
        }
    }

    // ------------------------------------------------------------------ helpers
    fn time_coef(&self, seconds: f32) -> f32 {
        1.0 - (-1.0 / (cmax(seconds, 0.0002) * self.fs_base as f32)).exp()
    }

    fn next_random(&mut self) -> f32 {
        self.rng = (self.rng.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7fff_ffff;
        self.rng as f32 / 1073741824.0 - 1.0
    }

    // ------------------------------------------------------------------ model table
    fn apply_model(&mut self) {
        // volts at 0 dBFS going in (before Drive) and the scale back out; outV is trimmed so the passband sits at unity
        self.in_v = 1.0;
        self.out_v = 1.0;
        let ci = self.circuit_idx;
        if ci == 0 {
            self.in_v = 1.0;
            self.out_v = 1.4;
        } else if ci == 1 {
            self.in_v = 1.6;
            self.out_v = 1.4 / 1.6;
        } else if ci == 2 || ci == 3 {
            self.in_v = 0.25;
            self.out_v = 1.0;
        } else if ci == 4 {
            self.in_v = 0.5;
            self.out_v = 2.8;
        } else if ci == 5 {
            self.in_v = 1.5;
            self.out_v = 1.0 / 1.5;
        } else if ci == 6 {
            self.in_v = 0.4;
            self.out_v = 2.5;
        } else if ci == 7 {
            self.in_v = 0.4;
            self.out_v = 4.0;
        } else {
            self.in_v = 0.8;
            self.out_v = 1.25;
        }

        // ---- wah component sets
        self.wah_re = 390.0;
        self.wah_beta = 450.0;
        self.wah_l0 = 0.50;
        self.wah_rl = 15.0;
        self.wah_isat = 1.5e-3;
        self.wah_irem = 0.0;
        self.wah_cf = 10.0e-9;
        if ci == 2 {
            if self.cb_inductor_idx == 1 {
                // Yellow Fasel
                self.wah_l0 = 0.55;
                self.wah_rl = 17.0;
                self.wah_isat = 1.0e-3;
            } else if self.cb_inductor_idx == 2 {
                // Halo: remanence = asymmetry
                self.wah_l0 = 0.50;
                self.wah_rl = 33.0;
                self.wah_isat = 0.8e-3;
                self.wah_irem = 60.0e-6;
            } else if self.cb_inductor_idx == 3 {
                // stock TDK can: lossier, cleaner
                self.wah_l0 = 0.53;
                self.wah_rl = 80.0;
                self.wah_isat = 3.0e-3;
            }
            if self.cb_voice_idx == 1 {
                self.wah_cf = 22.0e-9;
            } else if self.cb_voice_idx == 2 {
                self.wah_cf = 6.8e-9;
            }
            self.out_v = 1.4;
        } else if ci == 3 {
            self.wah_re = 510.0;
            self.wah_beta = 250.0;
            self.wah_l0 = 0.50;
            self.wah_rl = 35.0;
            self.wah_isat = 1.0e-3;
            if self.vox_voice_idx == 1 {
                self.wah_re = 470.0;
                self.wah_beta = 200.0;
                self.wah_rl = 33.0;
                self.wah_isat = 0.8e-3;
                self.wah_irem = 60.0e-6;
            }
            self.out_v = 1.65;
        }
        self.wah_rc_eff = 1.0 / (1.0 / WAH_RC + 1.0 / WAH_RPOT);
        self.wah_a = self.wah_rc_eff / (VT / WAH_IQ + self.wah_re);
        self.wah_vdn = (WAH_VCC - WAH_RC * WAH_IQ) - WAH_IQ * self.wah_re - 0.2;
        self.wah_gcin = 2.0 * WAH_CIN / self.os_t;
        self.wah_gin = 1.0 / (WAH_R1 + 1.0 / self.wah_gcin);
        self.wah_gcf = 2.0 * self.wah_cf / self.os_t;

        // opto time constants
        if ci == 1 {
            self.opto_on_coef = self.time_coef(MU_TAU_ON as f32);
            self.opto_off_coef = self.time_coef(MU_TAU_OFF as f32);
        } else {
            self.opto_on_coef = self.time_coef(QT_TAU_ON as f32);
            self.opto_off_coef = self.time_coef(QT_TAU_OFF as f32);
        }
    }

    fn reset_states(&mut self) {
        for k in &mut self.ch {
            k.sb = 0.0;
            k.sl = 0.0;
            k.lp = 0.0;
            k.bp = 0.0;
            k.hp = 0.0;
            k.y1 = 0.0;
            k.y2 = 0.0;
            k.y3 = 0.0;
            k.y4 = 0.0;
            k.s1 = 0.0;
            k.s2 = 0.0;
            k.s3 = 0.0;
            k.s4 = 0.0;
            k.fb_z = 0.0;
            k.a_sw = 0.0;
            k.a_s2 = 0.0;
            k.a_vd = 0.0;
            k.b_sw = 0.0;
            k.b_sq = 0.0;
            k.b_vd = 0.0;
            k.w_u = 0.0;
            k.w_il = 0.0;
            k.w_vl = 0.0;
            k.w_eq_cf = 0.0;
            k.w_eq_cin = 0.0;
        }
    }

    // ---------------------------------------------------------------- knob -> circuit values (per base sample)
    fn update_shared(&mut self) {
        let r = f64::from(self.reso_sm);
        self.drive_g = 10.0f64.powf(f64::from(knob_to_drive_db(self.drive_sm)) / 20.0);
        self.drive_comp = self.drive_g.powf(0.9);

        let ci = self.circuit_idx;
        if ci == 0 {
            let q = QT_Q_MIN * (QT_Q_MAX / QT_Q_MIN).powf(r);
            self.svf_damp = 1.0 / q;
            self.svf_in_comp = 1.0 / cmax(q, 1.0).sqrt();
        } else if ci == 1 {
            let q = MU_Q_MIN * (MU_Q_MAX / MU_Q_MIN).powf(r);
            self.svf_damp = 1.0 / q;
            self.svf_in_comp = 1.0 / cmax(q, 1.0).sqrt();
        } else if ci == 2 || ci == 3 {
            // the "Q resistor" across the inductor
            self.wah_rq = 33000.0 * 16.0f64.powf(r - 0.5);
        } else if ci == 4 {
            self.lad_k = 4.3 * r;
            self.lad_in_comp = if self.moog_comp_idx == 1 { 1.0 + 0.6 * self.lad_k } else { 1.0 };
        } else if ci == 5 {
            self.ms_k = 2.15 * r;
        } else if ci == 6 {
            let q = 0.5 * 24.0f64.powf(r);
            self.svf_damp = 1.0 / q;
            self.svf_in_comp = 1.0 / cmax(q, 1.0).sqrt();
        } else if ci == 7 {
            self.lad_k = (if self.tb_mod_idx == 1 { 20.5 } else { 17.2 }) * r;
            self.lad_in_comp = 1.0 + 0.3 * self.lad_k;
        } else {
            let q = 0.5 * 60.0f64.powf(r);
            self.svf_damp = 1.0 / q;
            self.svf_in_comp = 1.0 / cmax(q, 1.0).sqrt();
        }
    }

    // pos 0..1 -> coefficients for channel c
    fn update_channel(&mut self, c: usize, p: f64) {
        let mut fc;
        let ci = self.circuit_idx;

        if ci == 0 || ci == 1 {
            let qt = ci == 0;
            let rb = if qt { QT_LDR_BRIGHT } else { MU_LDR_BRIGHT };
            let ga = if qt { QT_LDR_GAMMA } else { MU_LDR_GAMMA };
            let rs = if qt { QT_RSHUNT } else { MU_RSHUNT };
            let rldr = rb * 10.0f64.powf(3.0 * ga * (1.0 - p));
            let r = (if qt { QT_RSER } else { MU_RSER }) + (rldr * rs) / (rldr + rs);
            let cap =
                if qt { if self.qt_range_idx == 1 { QT_C_HI } else { QT_C_LO } } else { if self.mu_range_idx == 1 { MU_C_HI } else { MU_C_LO } };
            fc = 1.0 / (2.0 * PI * r * cap);
        } else if ci == 2 || ci == 3 {
            // pot taper chosen so the sweep is even in octaves (what a good wah pot is for); heel = full Miller feedback
            let a = ((1.0 + self.wah_a).powf(1.0 - p) - 1.0) / self.wah_a;
            self.alpha_c[c] = clamp_t(a, 0.004, 1.0);
            fc = 1.0 / (2.0 * PI * (self.wah_l0 * self.wah_cf * (1.0 + self.alpha_c[c] * self.wah_a)).sqrt());
        } else if ci == 4 {
            fc = 30.0 * 600.0f64.powf(p);
        } else if ci == 5 {
            fc = 40.0 * 400.0f64.powf(p);
        } else if ci == 6 {
            fc = 30.0 * 530.0f64.powf(p);
        } else if ci == 7 {
            fc = if self.tb_mod_idx == 1 { 40.0 * 300.0f64.powf(p) } else { 70.0 * 90.0f64.powf(p) };
        } else {
            fc = 40.0 * 350.0f64.powf(p);
        }

        fc = clamp_t(fc, 10.0, 22000.0);
        self.fc_disp[c] = fc;

        // the 303's diode ladder resonates well above its nominal corner (measured 1.48x) -- tune it so Cutoff reads where the resonance is
        let tune = if ci == 7 { 0.68 } else { 1.0 };
        self.h_c[c] = (PI * cmin(fc * tune, 0.3 * self.fs_os) / self.fs_os).tan();
        self.h_c2[c] = (PI * cmin(fc * 0.25, 0.3 * self.fs_os) / self.fs_os).tan(); // MS-20 HP section in HP+LP mode
    }

    // ---------------------------------------------------------------- circuit cores (oversampled rate)

    /// TB-303 diode ladder. Each rung is a diode pair carrying (I0/2) tanh (dV / 2Vt); the
    /// stages load each other, the first cap is half size, and the resonance feedback is
    /// AC-coupled (it thins out in the bass, like the real one).
    fn diode_core(&self, c: &mut Channel, u: f64, h: f64, k: f64) {
        let mut y1 = c.y1;
        let mut y2 = c.y2;
        let mut y3 = c.y3;
        let mut y4 = c.y4;
        let mut jm = [0.0f64; 16];
        let mut r = [0.0f64; 4];

        for _it in 0..2 {
            let d0 = (u - k * (y4 - c.fb_z)).tanh();
            let d1 = (y1 - y2).tanh();
            let d2 = (y2 - y3).tanh();
            let d3 = (y3 - y4).tanh();
            let d4 = y4.tanh();
            r[0] = -(y1 - c.s1 - 2.0 * h * (d0 - d1));
            r[1] = -(y2 - c.s2 - h * (d1 - d2));
            r[2] = -(y3 - c.s3 - h * (d2 - d3));
            r[3] = -(y4 - c.s4 - h * (d3 - d4));
            let g0 = h * (1.0 - d0 * d0);
            let g1 = h * (1.0 - d1 * d1);
            let g2 = h * (1.0 - d2 * d2);
            let g3 = h * (1.0 - d3 * d3);
            let g4 = h * (1.0 - d4 * d4);
            jm[0] = 1.0 + 2.0 * g1;
            jm[1] = -2.0 * g1;
            jm[2] = 0.0;
            jm[3] = 2.0 * k * g0;
            jm[4] = -g1;
            jm[5] = 1.0 + g1 + g2;
            jm[6] = -g2;
            jm[7] = 0.0;
            jm[8] = 0.0;
            jm[9] = -g2;
            jm[10] = 1.0 + g2 + g3;
            jm[11] = -g3;
            jm[12] = 0.0;
            jm[13] = 0.0;
            jm[14] = -g3;
            jm[15] = 1.0 + g3 + g4;
            solve4(&mut jm, &mut r);
            y1 += clamp_t(r[0], -2.0, 2.0);
            y2 += clamp_t(r[1], -2.0, 2.0);
            y3 += clamp_t(r[2], -2.0, 2.0);
            y4 += clamp_t(r[3], -2.0, 2.0);
        }

        c.s1 = 2.0 * y1 - c.s1;
        c.s2 = 2.0 * y2 - c.s2;
        c.s3 = 2.0 * y3 - c.s3;
        c.s4 = 2.0 * y4 - c.s4;
        c.y1 = y1;
        c.y2 = y2;
        c.y3 = y3;
        c.y4 = y4;
        c.fb_z += (y4 - c.fb_z) * (2.0 * PI * 90.0 * self.os_t); // feedback coupling corner
    }

    /// Inductor wah. Unknown u = ln (Ic / Iq) of Q1, so the base voltage is explicit (Ebers-Moll +
    /// emitter degeneration) and one Newton solve covers: 68k + 10n input leg, 1.5k, inductor
    /// (series R, parallel Q resistor, core saturation), the 10n feedback cap returned to the pot
    /// wiper via the Q2 follower, and Q1's collector swing between cutoff and saturation.
    fn wah_core(&self, c: &mut Channel, vin: f64, alpha: f64) -> f64 {
        // core saturation: inductance falls as the core fills; a Halo's remanence offsets the curve
        let ix = (c.w_il + self.wah_irem) / self.wah_isat;
        let l = self.wah_l0 / (1.0 + ix * ix);
        let tl = self.os_t / (2.0 * l);
        let den = 1.0 + tl * self.wah_rl;
        let gl = tl / den;
        let hist_l = (c.w_il + tl * c.w_vl) / den;
        let b_inv = 1.0 / self.wah_beta;

        let mut u = c.w_u;
        let mut v_j = 0.0;
        let mut vc_s = 0.0;
        let mut iin = 0.0;
        let mut vfb = 0.0;

        for _it in 0..4 {
            let e = clamp_t(u, -14.0, 8.0).exp();
            let di = WAH_IQ * (e - 1.0);
            let vb = VT * u + self.wah_re * di * (1.0 + b_inv);
            let dvb = VT + self.wah_re * WAH_IQ * e * (1.0 + b_inv);
            iin = (vin - vb - c.w_eq_cin / self.wah_gcin) * self.wah_gin;
            let i15 = iin - di * b_inv;
            let di15 = -self.wah_gin * dvb - WAH_IQ * e * b_inv;
            v_j = vb - WAH_R15 * i15;
            let dv_j = dvb - WAH_R15 * di15;
            let vc = -self.wah_rc_eff * di;
            let mut dvc = -self.wah_rc_eff * WAH_IQ * e;
            vc_s = vc;
            if vc < -0.8 * self.wah_vdn {
                vc_s = knee(vc, self.wah_vdn);
                dvc *= knee_d(vc, self.wah_vdn);
            }
            vfb = alpha * WAH_AF * vc_s;
            let f = i15 - (gl * v_j + hist_l) - v_j / self.wah_rq - (self.wah_gcf * (v_j - vfb) - c.w_eq_cf);
            let fp = di15 - gl * dv_j - dv_j / self.wah_rq - self.wah_gcf * (dv_j - alpha * WAH_AF * dvc);
            u -= clamp_t(f / fp, -1.0, 1.0);
        }

        // commit companion states
        let i_cf = self.wah_gcf * (v_j - vfb) - c.w_eq_cf;
        c.w_eq_cf = self.wah_gcf * (v_j - vfb) + i_cf;
        let v_cin = (iin + c.w_eq_cin) / self.wah_gcin;
        c.w_eq_cin = self.wah_gcin * v_cin + iin;
        let i_l = gl * v_j + hist_l;
        c.w_il = i_l;
        c.w_vl = v_j - self.wah_rl * i_l;
        c.w_u = clamp_t(u, -14.0, 8.0);
        -vc_s // Q1 inverts; flip it so Mix adds in phase
    }

    fn core_sample(&self, ci: usize, c: &mut Channel, v: f64) -> f64 {
        let h = self.h_c[ci];
        let circuit = self.circuit_idx;

        if circuit == 0 {
            svf_core(c, v * self.svf_in_comp, h, self.svf_damp, 0, QT_RAIL);
            if self.qt_mode_idx == 0 {
                return c.lp;
            }
            if self.qt_mode_idx == 1 {
                return c.bp;
            }
            if self.qt_mode_idx == 2 {
                return c.hp;
            }
            return 0.5 * (c.lp + c.bp); // Mix: passive 100k / 100k sum
        }
        if circuit == 1 {
            svf_core(c, v * self.svf_in_comp, h, self.svf_damp, 0, MU_RAIL);
            if self.mu_mode_idx == 0 {
                return c.lp;
            }
            if self.mu_mode_idx == 1 {
                return c.bp;
            }
            return c.hp;
        }
        if circuit == 2 || circuit == 3 {
            return self.wah_core(c, v, self.alpha_c[ci]);
        }
        if circuit == 4 {
            moog_core(c, v * self.lad_in_comp, h, self.lad_k);
            if self.moog_slope_idx == 0 {
                return c.y4;
            }
            if self.moog_slope_idx == 1 {
                return c.y3;
            }
            if self.moog_slope_idx == 2 {
                return c.y2;
            }
            return c.y1;
        }
        if circuit == 5 {
            if self.ms_mode_idx == 0 {
                return korg35(&mut c.a_sw, &mut c.a_s2, &mut c.a_vd, v, h, self.ms_k, false);
            }
            if self.ms_mode_idx == 1 {
                return korg35(&mut c.b_sw, &mut c.b_sq, &mut c.b_vd, v, h, self.ms_k, true);
            }
            let hp_out = korg35(&mut c.b_sw, &mut c.b_sq, &mut c.b_vd, v, self.h_c2[ci], self.ms_k * 0.8, true);
            return korg35(&mut c.a_sw, &mut c.a_s2, &mut c.a_vd, hp_out, h, self.ms_k, false);
        }
        if circuit == 6 || circuit == 8 {
            let wasp = circuit == 8;
            svf_core(c, v * self.svf_in_comp, h, self.svf_damp, if wasp { 2 } else { 1 }, 0.0);
            let m = if wasp { self.wasp_mode_idx } else { self.sem_mode_idx };
            if m == 0 {
                return c.lp;
            }
            if m == 1 {
                return c.bp;
            }
            if m == 2 {
                return c.hp;
            }
            return c.lp + c.hp;
        }
        self.diode_core(c, v * self.lad_in_comp, h, self.lad_k);
        c.y4
    }

    // ---------------------------------------------------------------- modulation (per base sample)
    fn lfo_value(&self, ph: f64) -> f32 {
        let p = (ph - ph.floor()) as f32;
        let shape = self.lfo_shape_idx;
        if shape == 0 {
            return (TWO_PI as f32 * p).sin();
        }
        if shape == 1 {
            let q = p + 0.75;
            return 4.0 * ((q - q.floor()) - 0.5).abs() - 1.0;
        }
        if shape == 2 {
            return 2.0 * p - 1.0;
        }
        if shape == 3 {
            return 1.0 - 2.0 * p;
        }
        if shape == 4 {
            return if p < 0.5 { 1.0 } else { -1.0 };
        }
        if shape == 5 {
            return self.sh_cur;
        }
        let w = 0.5 - 0.5 * (PI as f32 * p).cos(); // Drift: smooth random walk between held values
        self.sh_prev + (self.sh_cur - self.sh_prev) * w
    }

    fn run_mod(&mut self, mono: f32) {
        self.bpm_now = if self.tempo_src_idx == 0 && self.host_bpm >= 20.0 { self.host_bpm } else { self.manual_bpm };
        let beats = note_to_beats(self.note_idx);
        let cycle_hz = self.bpm_now / (60.0 * beats);

        // envelope follower (shared detector, optional low cut so bass notes don't pin it)
        let mut d = mono;
        if self.env_low_cut {
            let hpv = mono - self.env_hp_x + self.env_hp_coef * self.env_hp_y;
            self.env_hp_x = mono;
            self.env_hp_y = hpv;
            d = hpv;
        }
        let rect = cmin(d.abs() * self.env_sens_lin, 4.0);
        self.env_state += (rect - self.env_state) * (if rect > self.env_state { self.env_atk_coef } else { self.env_rel_coef });
        let env_n = 1.0 - (-1.2 * self.env_state).exp(); // compressive, like an LED + photocell: touch-sensitive without pinning

        // LFO
        let lfo_hz = if self.lfo_synced { cycle_hz } else { self.lfo_free_hz };
        self.lfo_phase += f64::from(lfo_hz) / self.fs_base;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= self.lfo_phase.floor();
            self.sh_prev = self.sh_cur;
            self.sh_cur = self.next_random();
        }

        // step sequencer: Note is the length of one step
        let n = f64::from(self.step_count);
        self.seq_phase += f64::from(cycle_hz) / self.fs_base;
        if self.seq_phase >= n {
            self.seq_phase -= (self.seq_phase / n).floor() * n;
        }
        let idx = clamp_t(self.seq_phase as i32, 0, 15);
        let target = self.step_val[idx as usize];
        let glide_sec = 0.0005 + self.glide_amt * self.glide_amt * 0.6 / cmax(cycle_hz, 0.05);
        self.seq_val += (target - self.seq_val) * self.time_coef(glide_sec);

        for c in 0..2 {
            let mut m = 0.0f32;
            if self.mod_src_idx == 1 {
                m = env_n;
            } else if self.mod_src_idx == 2 {
                m = self.lfo_value(self.lfo_phase + if c == 1 { 0.5 * f64::from(self.lfo_stereo_amt) } else { 0.0 });
            } else if self.mod_src_idx == 3 {
                m = 2.0 * self.seq_val - 1.0;
            }
            if c == 0 {
                self.mod_now = m;
            }

            let p = clamp_t(self.cutoff_sm + self.depth_bip * m, 0.0, 1.0);
            self.pos_sm[c] += (p - self.pos_sm[c]) * self.pos_sm_coef;

            let mut p_use = self.pos_sm[c];
            if self.circuit_idx <= 1 {
                // the LDR: quick to light up, slow to go dark
                self.pos_opto[c] +=
                    (self.pos_sm[c] - self.pos_opto[c]) * (if self.pos_sm[c] > self.pos_opto[c] { self.opto_on_coef } else { self.opto_off_coef });
                p_use = self.pos_opto[c];
            }
            self.update_channel(c, f64::from(p_use));
        }
    }

    // ---------------------------------------------------------------- per-channel audio
    fn process_channel(&self, ci: usize, c: &mut Channel, x: f32, noise: f32) -> f32 {
        let hp_in = x - c.in_hp_x1 + 0.9993 * c.in_hp_y1;
        c.in_hp_x1 = x;
        c.in_hp_y1 = hp_in;
        let xin = hp_in * self.trim_sm;

        let g_in = self.in_v * self.drive_g;
        let g_out = self.out_v / self.drive_comp;
        let mut y = 0.0f32;

        for k in 0..4 {
            let mut u = 0.0f32;
            if k == 0 {
                u = xin * 4.0;
            }
            for i in 0..4 {
                u = filter(&mut c.up[i], u);
            }

            let w = self.core_sample(ci, c, f64::from(u) * g_in + f64::from(noise)) * g_out;
            let mut d = u * (1.0 - self.mix_sm) + w as f32 * self.mix_sm; // mix here so dry and wet share the same filters
            for i in 0..4 {
                d = filter(&mut c.down[i], d);
            }
            y = d;
        }

        y *= self.level_sm;
        let o = y - c.out_hp_x1 + 0.9990 * c.out_hp_y1;
        c.out_hp_x1 = y;
        c.out_hp_y1 = o;

        if is_bad_sample(o) {
            // NaN / runaway guard: reset this channel, don't go silent
            c.sb = 0.0;
            c.sl = 0.0;
            c.y1 = 0.0;
            c.y2 = 0.0;
            c.y3 = 0.0;
            c.y4 = 0.0;
            c.s1 = 0.0;
            c.s2 = 0.0;
            c.s3 = 0.0;
            c.s4 = 0.0;
            c.fb_z = 0.0;
            c.a_sw = 0.0;
            c.a_s2 = 0.0;
            c.a_vd = 0.0;
            c.b_sw = 0.0;
            c.b_sq = 0.0;
            c.b_vd = 0.0;
            c.w_u = 0.0;
            c.w_il = 0.0;
            c.w_vl = 0.0;
            c.w_eq_cf = 0.0;
            c.w_eq_cin = 0.0;
            c.out_hp_x1 = 0.0;
            c.out_hp_y1 = 0.0;
            for i in 0..4 {
                c.down[i].z1 = 0.0;
                c.down[i].z2 = 0.0;
                c.up[i].z1 = 0.0;
                c.up[i].z2 = 0.0;
            }
            return 0.0;
        }
        knee(f64::from(o), 1.5) as f32 // transparent below +1.6 dBFS; catches self-oscillation peaks
    }
}
