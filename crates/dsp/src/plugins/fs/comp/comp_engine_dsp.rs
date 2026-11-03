//! CompEngineDSP -- Engine 1: ten compressors in one zero-latency processor, three lanes of them.
//! Line-by-line translation of `Engine 1 - Comp/Source/CompEngineDSP.h` (`CompLane` and
//! `CompEngineDSP`, in the header's order).
//!
//! ```text
//!   0 Clean   1 1176   2 LA-2A   3 160   4 G Bus   5 Vari-Mu   6 Ross   7 EL8   8 2500   9 33609
//! ```
//!
//! The gain cell, the detector and the timing network are solved as circuits, in volts, for the
//! modelled designs; the rest are behavioural. The linear signal path has no delay at all.
//! Everything non-linear is generated as a 2x-oversampled residual and added back.
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - Everything is `float` as in the C++, except the crossover design (`butter`, `allpass`),
//!   which the C++ does in `double` and rounds to `float` coefficients.
//! - `std::max` / `std::min` / `clampT` are the C++ comparisons (`cmax`, `cmin`, `clamp_t`), so a
//!   NaN propagates the way it does there rather than being dropped like Rust's `f32::max`.
//! - `(int) (v + 0.5f)` on a knob: Rust `as` truncates toward zero like the C++ cast for every
//!   finite value; non-finite values never reach here (the plugin wrapper clamps first, as the
//!   APVTS does). `std::lround` is `f32::round` (half away from zero) then the cast.
//! - The C++ class splits a lane's state into its two `Chan`s and everything else; here the
//!   "everything else" is `LaneModel`, so a model method can read it while it mutates a `Chan`
//!   (the C++ does both through `this`). The order of every read and write is unchanged.
//! - The input scope (`scope`, `scopeWrite`, `copyScope`) feeds the editor's crossover spectrum
//!   only and is not translated (no analysis ring for this port). The 30 Hz meters are kept as
//!   plain fields; per-block meters for the Rust readouts are added beside them (marked).

// Constants and literals are kept exactly as the C++ writes them (`3.14159265358979` in the
// crossover is a different double from `f64::consts::PI`, and is what the C++ computes with).
#![allow(clippy::excessive_precision, clippy::approx_constant)]

// ---------------------------------------------------------------- C++ comparison semantics
/// `std::max (a, b)`: `(a < b) ? b : a`.
#[inline]
fn cmax(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}
/// `std::min (a, b)`: `(b < a) ? b : a`.
#[inline]
fn cmin(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}
/// `clampT`: `v < lo ? lo : (v > hi ? hi : v)`.
#[inline]
fn clamp_t(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

const PI_F: f32 = 3.14159265358979323846;

// ================================================================ CompLane
/// `CompLane::ParamIndex` (declaration order = param1..param16).
pub mod lane_param {
    pub const P_STYLE: usize = 0;
    pub const P_AMOUNT: usize = 1;
    pub const P_RATIO: usize = 2;
    pub const P_ATTACK: usize = 3;
    pub const P_RELEASE: usize = 4;
    pub const P_MAKEUP: usize = 5;
    pub const P_MIX: usize = 6;
    pub const P_KNEE: usize = 7;
    pub const P_SC_HPF: usize = 8;
    pub const P_COLOR: usize = 9;
    pub const P_LINK: usize = 10;
    pub const P_AUTO_GAIN: usize = 11;
    pub const P_BYPASS: usize = 12;
    pub const P_VARIANT: usize = 13;
    pub const P_DET_MODE: usize = 14;
    pub const P_CHAN_MODE: usize = 15;
}

#[derive(Debug, Clone, Copy, Default)]
struct Hb {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    x3: f32,
    y3: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Chan {
    // dB-domain envelopes (<= 0), gain reduction in dB, applied linear gain
    g1: f32,
    g2: f32,
    g3: f32,
    g_eff: f32,
    g_lin: f32,
    pw: f32, // RMS detector (mean square)
    fb: f32, // last compressed sample (linear path), for the feedback designs
    q: f32,  // what the detector measured this sample (dB level, volts or light)
    h1: f32, // sidechain high-pass
    h2: f32,
    bx1: f32, // EL8 band-emphasis biquad
    bx2: f32,
    by1: f32,
    by2: f32,
    t1x: f32, // 2500 Thrust shelves
    t1y: f32,
    t2x: f32,
    t2y: f32,
    t3x: f32,
    t3y: f32,
    u: f32,  // 1176: timing-cap voltage above the Q-bias point (V)
    vb: f32, // 1176: DC shift on the rectifier's coupling caps (V)
    f1: f32, // LA-2A: fast and slow (memory) states of the CdS cell, in light units
    f2: f32,
    m1: f32, // 670: control line and the two memory caps (dB = scaled volts, >= 0)
    m2: f32,
    m3: f32,
    vc: f32, // Ross: envelope cap voltage (V)
    ca: f32, // Ross: charge state of the two rectifier coupling caps (V)
    cb: f32,
    px: f32, // Ross: pre-emphasis / de-emphasis (2x rate) and the 9 kHz output pole
    py: f32,
    dy: f32,
    lp: f32,
    v1: f32, // G Bus: timing cap voltages (V); v2 is the big Auto cap
    v2: f32,
    dc_x: f32, // DC blocker on the colour residual
    dc_y: f32,
    up: Hb,
    dn: Hb,
}

const DN: f32 = 1.0e-18;
const HC0: f32 = 0.0798664262;
const HC1: f32 = 0.2838293449;
const HC2: f32 = 0.5453236511;
const HC3: f32 = 0.8344118915;

// 1176: series resistor x JFET transconductance parameter (27k x ~0.4 mS/V, a low-pinch-off selected part)
const FET_C: f32 = 17.5;
const FET_VD: f32 = 0.047; // FD333: n Vt
const FET_IS: f32 = 5.0e-12; // FD333: saturation current (a low-leakage part)
const FET_CT: f32 = 0.22e-6; // timing capacitor
// LA-2A: EL panel light ~ V^3, CdS conductance ~ light^0.9
const OPT_LOOP: f32 = 2.7;
// Ross: 9 V supply, two junction drops before Iabc flows, 10 uF envelope cap
const ROSS_VS: f32 = 9.0;
const ROSS_V0: f32 = 1.25;
const ROSS_C: f32 = 10.0e-6;
const ROSS_VFS: f32 = 1.5; // volts at the pedal input for a full-scale sample
const ROSS_CC: f32 = 0.01e-6; // C13 / C14, rectifier coupling caps
const ROSS_VBE: f32 = 0.55; // rectifier transistors and clamp diodes turn on

// G Bus: volts on the timing cap per dB (both VCAs: 6.98 dB/V), the matching exponent, diode softness,
// how far the TL074 can swing behind its diode, and the bleed resistor across the timing network
const GB_S: f32 = 0.14327;
const GB_K: f32 = 0.80360;
const GB_VD: f32 = 0.045;
const GB_VMAX: f32 = 9.9;
const GB_R3: f32 = 3300000.0;

// 33609: how far the other side of zero the recovery reference sits, in dB of gain reduction
const NV_REF: f32 = 14.2;

// 670: dB of gain reduction per 100 % of output overshoot past the DC threshold, rectifier softness,
// grid signal relative to the tubes' exponential constant, push-pull imbalance
const VARI_K: f32 = 25.0;
const VARI_SOFT: f32 = 0.08;
const VARI_Z: f32 = 0.4;
const VARI_EPS: f32 = 0.06;

#[inline]
fn clamp01(v: f32) -> f32 {
    // `v < 0 ? 0 : (v > 1 ? 1 : v)`: `f32::clamp` is the same comparisons (NaN stays NaN).
    v.clamp(0.0, 1.0)
}
#[inline]
fn db_to_lin(d: f32) -> f32 {
    (d * 0.11512925).exp()
}
#[inline]
fn softplus(z: f32) -> f32 {
    if z > 18.0 {
        z
    } else if z < -14.0 {
        0.0
    } else {
        (1.0 + z.exp()).ln()
    }
}

fn knee_fn(o: f32, w: f32) -> f32 {
    if w < 0.01 {
        return cmax(o, 0.0);
    }
    if o <= -0.5 * w {
        return 0.0;
    }
    if o >= 0.5 * w {
        return o;
    }
    let t = o + 0.5 * w;
    t * t / (2.0 * w)
}

fn knee_slope(o: f32, w: f32) -> f32 {
    if w < 0.01 {
        return if o > 0.0 { 1.0 } else { 0.0 };
    }
    if o <= -0.5 * w {
        return 0.0;
    }
    if o >= 0.5 * w {
        return 1.0;
    }
    (o + 0.5 * w) / w
}

fn ratio_from_knob(t: f32) -> f32 {
    if t > 0.965 {
        return 1000.0;
    }
    let u = t / 0.965;
    1.0 + 19.0 * u * u
}

/// `ap`: one first-order allpass section of the half-band pair.
#[inline]
fn ap(x1: &mut f32, y1: &mut f32, c: f32, v: f32) -> f32 {
    let y = *x1 + c * (v - *y1) + DN;
    *x1 = v;
    *y1 = y;
    y
}

/// Everything in a `CompLane` except its two channels: raw parameters and the derived model.
#[derive(Debug, Clone)]
struct LaneModel {
    // raw parameter values
    p_style_v: f32,
    p_amount: f32,
    p_ratio: f32,
    p_attack: f32,
    p_release: f32,
    p_makeup: f32,
    /// `pMix_`: written by `setParameter`, never read (the engine's dry is global).
    #[allow(dead_code)]
    p_mix: f32,
    p_knee: f32,
    p_hpf: f32,
    p_color: f32,
    p_link: f32,
    p_variant: f32,
    p_det: f32,
    p_chan: f32,
    /// `chMode`: derived in `updateParams`, never read (the wrapper passes its own).
    #[allow(dead_code)]
    ch_mode: i32,
    p_auto: bool,
    /// `pBypass_`: written by `setParameter`, never read (the engine's bypass is global).
    #[allow(dead_code)]
    p_bypass: bool,
    dirty: bool,

    // derived, dB-domain engine
    sty: i32,
    is_fb: bool,
    is_rms: bool,
    is_vari: bool,
    direct_gain: bool,
    thr_t: f32,
    thr: f32,
    slope: f32, // feed-forward: 1 - 1/R    feedback: k = R - 1
    knee_w: f32,
    thr_lin: f32,
    vm_ka: f32,
    vm_dis: f32,
    vm_b1: f32,
    vm_b1f: f32,
    vm_b2: f32,
    vm_b2f: f32,
    a_att: f32,
    a_rel: f32,
    a_rms: f32,
    a_c2: f32,
    a_s2: f32,
    w2: f32,
    a_c3: f32,
    a_s3: f32,
    w3: f32,

    // derived, circuit models
    in_g: f32, // 1176 input gain / LA-2A peak-reduction gain / Ross rest gain, smoothed
    in_gt: f32,
    fet_a: f32,
    fet_vth: f32,
    fet_u0: f32,
    fet_lam: f32,
    fet_s: f32,
    fet_ra: f32,
    fet_al: f32,
    fet_be: f32,
    fet_clamp_a: f32,
    fet_clamp_b: f32,
    opt_ff: f32,
    opt_w: f32,
    opt_emph: bool,
    ross_ratt: f32,
    ross_rrel: f32,
    ross_beta: f32,
    ross_ka: f32,
    ross_kb: f32,
    ross_pb0: f32,
    ross_pb1: f32,
    ross_pa1: f32,
    ross_de: f32,
    ross_lp: f32,
    br_s: f32,
    gb_c: f32,
    gb_g: f32,
    gb_ra: f32,
    gb_ag: f32,
    gb_al1: f32,
    gb_be1: f32,
    gb_al2: f32,
    gb_be2: f32,
    gb_two: bool,

    // colour
    sat_a: f32,
    sat_b0: f32,
    sat_b1: f32,
    sat_res: f32,
    sat_bias: f32,
    sat_tb: f32,
    sat_norm: f32,
    sat_m: f32,
    sat_on: bool,

    // sidechain filters
    hpf_a1: f32,
    hpf_a2: f32,
    hpf_b: f32,
    hpf_on: bool,
    band_on: bool,
    bd_b0: f32,
    bd_b1: f32,
    bd_b2: f32,
    bd_a1: f32,
    bd_a2: f32,
    thrust: i32,
    th_b0: [f32; 3],
    th_b1: [f32; 3],
    th_a1: [f32; 3],
    th_norm: f32,

    mk_lin: f32,
    ag_db: f32,
    ag_num: f32,
    ag_den: f32,
    a_ag: f32,
    out_gain_t: f32,
    out_gain: f32,
    fs: f32,
    smooth: f32,
    dc_r: f32,
}

impl Default for LaneModel {
    fn default() -> Self {
        // Initial values exactly as the C++ member initialisers.
        LaneModel {
            p_style_v: 0.0,
            p_amount: 0.4,
            p_ratio: 0.4,
            p_attack: 0.5,
            p_release: 0.5,
            p_makeup: 0.3333,
            p_mix: 1.0,
            p_knee: 0.25,
            p_hpf: 0.0,
            p_color: 0.5,
            p_link: 1.0,
            p_variant: 0.0,
            p_det: 0.0,
            p_chan: 0.0,
            ch_mode: 0,
            p_auto: false,
            p_bypass: false,
            dirty: true,

            sty: 0,
            is_fb: false,
            is_rms: false,
            is_vari: false,
            direct_gain: false,
            thr_t: 0.0,
            thr: 0.0,
            slope: 0.0,
            knee_w: 6.0,
            thr_lin: 1.0,
            vm_ka: 0.1,
            vm_dis: 0.0,
            vm_b1: 0.0,
            vm_b1f: 0.0,
            vm_b2: 0.0,
            vm_b2f: 0.0,
            a_att: 0.1,
            a_rel: 0.001,
            a_rms: 0.001,
            a_c2: 0.0,
            a_s2: 0.0,
            w2: 0.0,
            a_c3: 0.0,
            a_s3: 0.0,
            w3: 0.0,

            in_g: 1.0,
            in_gt: 1.0,
            fet_a: 0.2,
            fet_vth: 0.2,
            fet_u0: -0.015,
            fet_lam: 0.9,
            fet_s: 0.006,
            fet_ra: 570.0,
            fet_al: 1.0,
            fet_be: 0.0,
            fet_clamp_a: 0.0,
            fet_clamp_b: 0.0,
            opt_ff: 0.0,
            opt_w: 0.7,
            opt_emph: false,
            ross_ratt: 0.0,
            ross_rrel: 150000.0,
            ross_beta: 500.0,
            ross_ka: 0.2,
            ross_kb: 1.0,
            ross_pb0: 1.0,
            ross_pb1: 0.0,
            ross_pa1: 0.0,
            ross_de: 0.1,
            ross_lp: 1.0,
            br_s: 0.3,
            gb_c: 2.252,
            gb_g: 16.89,
            gb_ra: 820.0,
            gb_ag: 1.0,
            gb_al1: 1.0,
            gb_be1: 0.0,
            gb_al2: 0.0,
            gb_be2: 0.0,
            gb_two: false,

            sat_a: 0.0,
            sat_b0: 0.0,
            sat_b1: 0.0,
            sat_res: 0.0,
            sat_bias: 0.0,
            sat_tb: 0.0,
            sat_norm: 1.0,
            sat_m: 1.0,
            sat_on: false,

            hpf_a1: 0.0,
            hpf_a2: 0.0,
            hpf_b: 1.0,
            hpf_on: false,
            band_on: false,
            bd_b0: 1.0,
            bd_b1: 0.0,
            bd_b2: 0.0,
            bd_a1: 0.0,
            bd_a2: 0.0,
            thrust: 0,
            th_b0: [0.0; 3],
            th_b1: [0.0; 3],
            th_a1: [0.0; 3],
            th_norm: 1.0,

            mk_lin: 1.0,
            ag_db: 0.0,
            ag_num: 0.0,
            ag_den: 0.0,
            a_ag: 0.0,
            out_gain_t: 1.0,
            out_gain: 1.0,
            fs: 48000.0,
            smooth: 0.002,
            dc_r: 0.999,
        }
    }
}

impl LaneModel {
    fn coef_for(&self, ms: f32) -> f32 {
        1.0 - (-1000.0 / (cmax(ms, 0.001) * self.fs)).exp()
    }

    // gain reduction the dB-domain sidechain asks for (dB, <= 0) when it sees level "lev"
    fn wanted(&self, lev: f32) -> f32 {
        if self.is_vari {
            return -lev; // the 670's rectifier already produced the control voltage
        }
        -self.slope * knee_fn(lev - self.thr, self.knee_w)
    }

    fn loop_gain(&self, lev: f32) -> f32 {
        if !self.is_fb {
            return 0.0;
        }
        if self.is_vari {
            return if lev > 0.01 { 0.11512925 * (VARI_K + lev) } else { 0.0 };
        }
        self.slope * knee_slope(lev - self.thr, self.knee_w)
    }

    fn set_shelf(&mut self, i: usize, fc: f32, g: f32) {
        let k = 2.0 * self.fs;
        let wz = 2.0 * PI_F * fc / g.sqrt();
        let wp = 2.0 * PI_F * fc * g.sqrt();
        let n = 1.0 / (1.0 + k / wp);
        self.th_b0[i] = (1.0 + k / wz) * n;
        self.th_b1[i] = (1.0 - k / wz) * n;
        self.th_a1[i] = (1.0 - k / wp) * n;

        let w = 2.0 * PI_F * 1000.0;
        self.th_norm *= ((1.0 + (w / wp) * (w / wp)) / (1.0 + (w / wz) * (w / wz))).sqrt();
    }

    //==============================================================================
    fn update_params(&mut self) {
        self.sty = ((self.p_style_v + 0.5) as i32).clamp(0, 9);
        let sty = self.sty;

        let vari = (self.p_variant + 0.5) as i32;
        let dmod = (self.p_det + 0.5) as i32;
        self.ch_mode = (self.p_chan + 0.5) as i32;

        self.is_fb = sty == 5 || sty == 9 || (sty == 8 && dmod >= 1);
        self.is_rms = sty == 3 || sty == 8;
        self.is_vari = sty == 5;
        self.direct_gain = sty == 3;

        let mut att_ms = 10.0f32;
        let mut rel_ms = 200.0f32;
        let mut r = 4.0f32;
        let mut rms_ms = 36.0f32;
        let mut t_c2 = 0.0f32;
        let mut t_s2 = 0.0f32;
        let t_c3 = 0.0f32;
        let t_s3 = 0.0f32;
        let mut s_a = 0.0f32;
        let mut hpf_min = 0.0f32;

        self.w2 = 0.0;
        self.w3 = 0.0;
        self.sat_b0 = 0.0;
        self.sat_b1 = 0.0;
        self.band_on = false;
        self.thrust = 0;
        self.opt_emph = false;
        self.thr_t = -50.0 * self.p_amount;
        self.in_gt = 1.0;

        if sty == 0 {
            // Clean
            r = ratio_from_knob(self.p_ratio);
            self.knee_w = 24.0 * self.p_knee;
            att_ms = 0.05 * 5000.0f32.powf(self.p_attack);
            rel_ms = 10.0 * 250.0f32.powf(self.p_release);
        } else if sty == 1 {
            // 1176
            let idx = (self.p_ratio * 4.0 + 0.5) as i32;
            let blue = vari == 0;
            let dt = 1.0 / self.fs;

            // Input knob: gain into the limiter (the output is divided back down by the same amount,
            // so the knob behaves like a threshold but the FET and the amplifiers really are driven harder)
            self.in_gt = db_to_lin(50.0 * self.p_amount);

            // Ratio switch, sidechain side: tap on the divider between the preamp output and the GR amplifier.
            // Ratio switch, bias side: the voltage the rectifier diodes' anodes rest at (tapped chain to -10 V).
            // "All" is every switch closed at once, worked out from the same resistors.
            let (tap, v_a, v_q, g_s);
            if blue {
                // Rev A: series 242k / 186k / 130k / 0 into the GR amp (its input impedance taken as 150k);
                // bias chain 470 - 470 - 2k7 - 1k2 from ground to -10 V
                tap = if idx == 0 {
                    0.383
                } else if idx == 1 {
                    0.446
                } else if idx == 2 {
                    0.536
                } else {
                    1.0
                };
                v_a = if idx == 0 {
                    0.0
                } else if idx == 1 {
                    -0.971
                } else if idx == 2 {
                    -1.942
                } else if idx == 3 {
                    -7.521
                } else {
                    0.0
                };
                v_q = -0.19f32;
                g_s = 1.54f32;
            } else {
                // LN: 47k - 56k - 56k - 68k - 56k divider; bias chain 150 - 470 - 560 - 1k5 - 2k2 from ground to -10 V
                tap = if idx == 0 {
                    0.166
                } else if idx == 1 {
                    0.364
                } else if idx == 2 {
                    0.562
                } else if idx == 3 {
                    0.802
                } else {
                    0.456
                };
                v_a = if idx == 0 {
                    -0.307
                } else if idx == 1 {
                    -1.270
                } else if idx == 2 {
                    -2.418
                } else if idx == 3 {
                    -5.492
                } else {
                    -0.638
                };
                v_q = -0.19f32;
                g_s = 5.42f32;
            }
            let tap: f32 = tap;
            let v_a: f32 = v_a;

            // Attack: 470R + 25k pot + the rectifier's own ~100R.  Release: 270k + 5M pot.  Both into 0.22 uF.
            self.fet_ra = 570.0 * 45.0f32.powf(self.p_attack);

            // A diode in series with a resistor behaves like a soft switch whose knee sits where the diode's own
            // exponential meets the resistor's straight line: n Vt ln (n Vt / (R Is)), 0.78 V at 570R, 0.60 V at 25k.
            let v_knee = FET_VD * (FET_VD / (self.fet_ra * FET_IS)).ln();

            self.fet_a = g_s * tap; // volts at the rectifier per unit of internal signal
            self.fet_vth = v_q - v_a + v_knee; // what a peak has to exceed before the diodes charge the cap
            self.fet_u0 = -0.03; // Q-bias trimmed so the few nA the diodes leak at rest
            // (worst at 4:1 with a long release) leave the FET off
            let r_rel = 270000.0 * 19.5f32.powf(self.p_release);
            self.fet_al = 1.0 / (1.0 + dt / (r_rel * FET_CT));
            self.fet_be = dt / FET_CT;

            // coupling-cap clamp: 6.8 uF charged by the diode current, restored through 47k
            self.fet_clamp_a = 1.0 / (1.0 + dt / (47000.0 * 6.8e-6));
            self.fet_clamp_b = dt / 6.8e-6;

            // Both revisions feed half the drain signal to the gate. Rev A does it with fixed 560k / 560k, the LN with
            // a trimmed null, so the LN cancels a little better. The rest of the colour is the amplifiers: Rev A has a
            // FET-input class-A preamp, the LN a three-transistor preamp with more feedback.
            self.fet_lam = if blue { 0.85 } else { 0.95 };
            self.fet_s = if blue { 0.011 } else { 0.0056 };
            s_a = if blue { 0.26 } else { 0.19 };
            self.sat_b0 = if blue { -0.09 } else { -0.06 };
        } else if sty == 2 {
            // LA-2A
            let limit = self.p_ratio >= 0.5;
            att_ms = 12.0;
            rel_ms = 170.0;
            t_c2 = 600.0;
            t_s2 = 4000.0;
            self.opt_ff = if limit { 0.045 } else { 0.0 }; // R7 (2k) over the divider's 44k Thevenin resistance
            self.in_gt = db_to_lin(50.0 * self.p_amount) * 0.62; // peak-reduction pot: gain into the panel driver
            s_a = 0.28;
            self.sat_b0 = 0.14;
            self.opt_emph = dmod >= 1;
        } else if sty == 3 {
            // 160
            r = ratio_from_knob(self.p_ratio);
            self.knee_w = if self.p_knee >= 0.5 { 14.0 } else { 0.0 };
            s_a = 0.12;
        } else if sty == 4 {
            // G Bus
            let ri = (self.p_ratio * 2.0 + 0.5) as i32;
            let ai = (self.p_attack * 5.0 + 0.5) as i32;
            let ei = (self.p_release * 4.0 + 0.5) as i32;
            let dt = 1.0 / self.fs;

            // ratio stage: gain = (1M || switch resistor) / 20k, offset = that feedback resistance x 12 V / bias resistor
            self.gb_two = ri == 0;
            self.gb_g = if ri == 0 {
                10.630
            } else if ri == 1 {
                16.887
            } else {
                50.0
            };
            self.gb_c = if ri == 0 {
                0.6542
            } else if ri == 1 {
                2.2517
            } else {
                10.0
            };

            // attack switch: series resistor into the timing cap (the feedback loop is what turns 0.39 ms of RC
            // into the 0.1 ms on the panel)
            self.gb_ra = if ai == 0 {
                820.0
            } else if ai == 1 {
                2700.0
            } else if ai == 2 {
                8200.0
            } else if ai == 3 {
                27000.0
            } else if ai == 4 {
                82000.0
            } else {
                270000.0
            };

            // release switch: 0.47 uF with its own discharge resistor, or the two-cap Auto network
            let c1 = 0.47e-6f32;
            let r1: f32 = if ei == 0 {
                180000.0
            } else if ei == 1 {
                270000.0
            } else if ei == 2 {
                560000.0
            } else if ei == 3 {
                1200000.0
            } else {
                91000.0
            };
            self.gb_al1 = 1.0 / (1.0 + dt / (r1 * c1));
            self.gb_be1 = dt / c1;

            if ei >= 4 {
                self.gb_al2 = 1.0 / (1.0 + dt / (750000.0 * 6.8e-6));
                self.gb_be2 = dt / 6.8e-6;
            } else {
                self.gb_al2 = 0.0;
                self.gb_be2 = 0.0;
            }

            s_a = 0.1;
            self.sat_b0 = 0.02; // VCA: about -75 dB, mostly second harmonic
        } else if sty == 5 {
            // Vari-Mu (670)
            let ti = (self.p_release * 5.0 + 0.5) as i32;
            let dt = 1.0 / self.fs;
            self.thr_t = -45.0 * self.p_amount;

            // time-constant switch: what hangs on the control line besides the permanent 220k || 0.1 uF
            let (mut c_f, mut r_d, mut c1, mut c2) = (2.1e-6f32, 149900.0f32, 0.0f32, 0.0f32); // position 1: 2 uF, 470k across the 220k
            if ti == 1 {
                c_f = 4.1e-6;
                r_d = 220000.0;
            } else if ti == 2 {
                c_f = 8.1e-6;
                r_d = 220000.0;
            } else if ti == 3 {
                c_f = 20.1e-6;
                r_d = 220000.0;
            } else if ti == 4 {
                c_f = 8.1e-6;
                r_d = 220000.0;
                c1 = 20.0e-6;
            } else if ti == 5 {
                c_f = 2.1e-6;
                r_d = 149900.0;
                c1 = 8.0e-6;
                c2 = 20.0e-6;
            }

            let r_src = 470.0f32; // rectifier + T104 + output tubes, as seen by the caps
            self.vm_ka = 1.0 - (-dt / (r_src * c_f)).exp();
            self.vm_dis = dt / (r_d * c_f);
            self.vm_b1 = if c1 > 0.0 { dt / (100000.0 * c1) } else { 0.0 }; // memory branches: 100k in series with the cap
            self.vm_b1f = if c1 > 0.0 { dt / (100000.0 * c_f) } else { 0.0 };
            self.vm_b2 = if c2 > 0.0 { dt / (100000.0 * c2) } else { 0.0 };
            self.vm_b2f = if c2 > 0.0 { dt / (100000.0 * c_f) } else { 0.0 };

            r = 6.0;
            s_a = 0.22;
            self.sat_b0 = 0.04; // output transformer / line stage, after the tubes
        } else if sty == 6 {
            // Ross
            // Sustain: 500k linear pot + 27k feeding Iabc from the envelope (less two junction drops). OTA gain at low
            // frequencies = (2k / 16.8k) x Iabc / (2 Vt) x 150k: 97 with the pot at zero, 5 with all 500k in.
            let a = clamp01(self.p_amount);
            let r_pot = 500000.0 * (1.0 - a);
            let dyna = vari >= 1;
            let dt = 1.0 / self.fs;
            self.in_gt = 97.2 * 27000.0 / (27000.0 + r_pot);
            self.ross_beta = if dyna { 300.0 } else { 500.0 };
            self.ross_ratt = 20000.0 * self.p_attack * self.p_attack * self.p_attack; // mod: 0 (stock) .. 20k in the rectifier collectors
            self.ross_rrel = 15000.0 * 100.0f32.powf(self.p_release); // mod: 15k .. 1.5M instead of the 150k (centre = stock)

            // coupling caps charge through the phase splitter: 10k from its collector, ~400R from its emitter
            self.ross_ka = 1.0 - (-dt / (10000.0 * ROSS_CC)).exp();
            self.ross_kb = 1.0 - (-dt / (400.0 * ROSS_CC)).exp();

            // pre-emphasis ahead of the OTA (zero 1061 Hz, pole 9043 Hz), de-emphasis after it (pole 1061 Hz),
            // both running at the oversampled rate; and the 9 kHz pole that is left over, on the output
            let k2 = 4.0 * self.fs;
            let wz = 2.0 * PI_F * 1061.0;
            let wp = 2.0 * PI_F * 9043.0;
            let n = 1.0 / (1.0 + k2 / wp);
            self.ross_pb0 = (1.0 + k2 / wz) * n;
            self.ross_pb1 = (1.0 - k2 / wz) * n;
            self.ross_pa1 = (1.0 - k2 / wp) * n;
            self.ross_de = 1.0 - (-wz / (2.0 * self.fs)).exp();
            self.ross_lp = 1.0 - (-wp / self.fs).exp();

            s_a = 3.43; // 1.5 V x (2k / 16.8k) / (2 Vt): the OTA's input pair
            self.sat_b0 = 0.03;
        } else if sty == 7 {
            // EL8
            let idx = (self.p_ratio * 7.0 + 0.5) as i32;
            att_ms = 0.05 * 600.0f32.powf(self.p_attack);
            rel_ms = 50.0 * 70.0f32.powf(self.p_release);
            t_c2 = 500.0;
            t_s2 = rel_ms * 4.0;
            self.w2 = 0.25;

            if idx == 0 {
                r = 1.0;
                self.knee_w = 0.0;
            } else if idx == 1 {
                r = 2.0;
                self.knee_w = 30.0;
            } else if idx == 2 {
                r = 3.0;
                self.knee_w = 20.0;
            } else if idx == 3 {
                r = 4.0;
                self.knee_w = 15.0;
            } else if idx == 4 {
                r = 6.0;
                self.knee_w = 10.0;
            } else if idx == 5 {
                r = 10.0;
                self.knee_w = 20.0;
                t_c2 = 400.0;
                t_s2 = cmax(2000.0, rel_ms * 6.0);
                self.w2 = 0.5;
            } else if idx == 6 {
                r = 20.0;
                self.knee_w = 6.0;
            } else {
                r = 100.0;
                self.knee_w = 2.0;
            }

            if vari == 0 {
                s_a = 0.1;
            } else if vari == 1 {
                // Dist 2: even-order, tube-like
                s_a = 0.38;
                self.sat_b0 = 0.3;
                self.sat_b1 = 0.15;
            } else {
                // Dist 3: odd-order, tape-like
                s_a = 0.6;
                self.sat_b1 = 0.0;
            }

            if dmod == 1 || dmod == 3 {
                hpf_min = 80.0;
            }
            self.band_on = dmod >= 2;
        } else if sty == 8 {
            // 2500
            let ri = (self.p_ratio * 6.0 + 0.5) as i32;
            let ai = (self.p_attack * 6.0 + 0.5) as i32;
            let ki = (self.p_knee * 2.0 + 0.5) as i32;

            r = if ri == 0 {
                1.5
            } else if ri == 1 {
                2.0
            } else if ri == 2 {
                3.0
            } else if ri == 3 {
                4.0
            } else if ri == 4 {
                6.0
            } else if ri == 5 {
                10.0
            } else {
                1000.0
            };
            att_ms = if ai == 0 {
                0.03
            } else if ai == 1 {
                0.1
            } else if ai == 2 {
                0.3
            } else if ai == 3 {
                1.0
            } else if ai == 4 {
                3.0
            } else if ai == 5 {
                10.0
            } else {
                30.0
            };
            rel_ms = 50.0 * 60.0f32.powf(self.p_release);
            self.knee_w = if ki == 0 {
                0.0
            } else if ki == 1 {
                6.0
            } else {
                14.0
            };
            rms_ms = 4.0;
            self.thrust = vari;
            s_a = 0.16;
            self.sat_b0 = 0.02;
        } else {
            // 33609
            let ri = (self.p_ratio * 5.0 + 0.5) as i32;
            let ei = (self.p_release * 5.0 + 0.5) as i32;
            let slow = self.p_attack >= 0.5;
            let lim = ri == 5;

            r = if ri == 0 {
                1.5
            } else if ri == 1 {
                2.0
            } else if ri == 2 {
                3.0
            } else if ri == 3 {
                4.0
            } else if ri == 4 {
                6.0
            } else {
                121.0
            };
            self.knee_w = if lim { 2.0 } else { 6.0 };

            // attack: the timing cap's charging resistor x 10 uF, no tricks - the feedback loop is what makes it quick.
            // Compressor 680R (6.8 ms). Limiter: R28, with R30 across it on Fast - the scan reads 3k3 or 5k6; 5k6 is
            // what reproduces the handbook's 4 ms / 2 ms, so 5k6 it is.
            att_ms = if lim {
                if slow { 56.0 } else { 28.0 }
            } else if slow {
                6.8
            } else {
                3.4
            };

            // recovery switch resistors (in parallel with ~1.2M of buffer loading) x 10 uF
            if ei == 0 {
                rel_ms = 235.0;
            } else if ei == 1 {
                rel_ms = 915.0;
            } else if ei == 2 {
                rel_ms = 2060.0;
            } else if ei == 3 {
                rel_ms = 3900.0;
            } else if ei == 4 {
                // Auto 1: 18k, 47 uF via 120k
                rel_ms = 180.0;
                t_c2 = 5640.0;
                t_s2 = 5640.0;
                self.w2 = 0.8;
            } else {
                // Auto 2: 7k5, 47 uF via 240k
                rel_ms = 75.0;
                t_c2 = 5640.0;
                t_s2 = 11280.0;
                self.w2 = 0.8;
            }

            self.br_s = 2.0; // per-leg bridge signal / nVt for a full-scale sample: +22 dBu through the
            // -31 dB pad gives 4; halved so that limiting a +22 dBu input lands on the
            // handbook's 0.45% (each bridge arm is a two-diode package)
            s_a = 0.2;
            self.sat_b0 = 0.03; // transformers and the class-A output amp
        }

        self.slope = if self.is_fb { cmin(r - 1.0, if sty == 9 { 120.0 } else { 60.0 }) } else { 1.0 - 1.0 / r };

        self.a_att = self.coef_for(att_ms);
        self.a_rel = self.coef_for(rel_ms);
        self.a_rms = self.coef_for(rms_ms);
        self.a_c2 = if t_c2 > 0.0 { self.coef_for(t_c2) } else { 0.0 };
        self.a_s2 = if t_s2 > 0.0 { self.coef_for(t_s2) } else { 0.0 };
        self.a_c3 = if t_c3 > 0.0 { self.coef_for(t_c3) } else { 0.0 };
        self.a_s3 = if t_s3 > 0.0 { self.coef_for(t_s3) } else { 0.0 };

        // colour: 5 is the stock circuit level, 0 is clean, 10 drives every non-linear element about 3.4x harder
        let c = clamp01(self.p_color);
        self.sat_m = 0.2 + 3.2 * c * c;
        self.sat_a = s_a * self.sat_m;
        self.sat_res = clamp01(c * 20.0);
        self.sat_on = s_a > 0.0 && c > 0.001;

        // sidechain high-pass, 12 dB/oct Butterworth, 20..500 Hz (the EL8 detector HP forces at least 80 Hz)
        let mut fc = if self.p_hpf > 0.02 { 20.0 * 25.0f32.powf(self.p_hpf) } else { 0.0 };
        fc = cmax(fc, hpf_min);
        self.hpf_on = fc > 1.0;

        if self.hpf_on {
            let kk = (PI_F * fc / self.fs).tan();
            let q = 0.70710678f32;
            let n = 1.0 / (1.0 + kk / q + kk * kk);
            self.hpf_b = n;
            self.hpf_a1 = 2.0 * (kk * kk - 1.0) * n;
            self.hpf_a2 = (1.0 - kk / q + kk * kk) * n;
        }

        if self.band_on {
            // EL8 band emphasis: +9 dB around 6 kHz in the detector
            let a = 10.0f32.powf(9.0f32 / 40.0f32);
            let w0 = 2.0 * PI_F * cmin(6000.0, 0.4 * self.fs) / self.fs;
            let al = w0.sin() / (2.0 * 1.0);
            let n = 1.0 / (1.0 + al / a);
            self.bd_b0 = (1.0 + al * a) * n;
            self.bd_b1 = -2.0 * w0.cos() * n;
            self.bd_b2 = (1.0 - al * a) * n;
            self.bd_a1 = self.bd_b1;
            self.bd_a2 = (1.0 - al / a) * n;
        }

        if self.opt_emph {
            // LA-2A R37 (set to 250k) with C12 1n into the 100k pot: a +11 dB
            // shelf between 640 Hz and 2.2 kHz, referred to 1 kHz
            self.thrust = 3;
            self.th_norm = 1.0;
            self.set_shelf(0, 1191.0, 3.5);
            self.th_b0[1] = 1.0;
            self.th_b1[1] = 0.0;
            self.th_a1[1] = 0.0;
            self.th_b0[2] = 1.0;
            self.th_b1[2] = 0.0;
            self.th_a1[2] = 0.0;
        } else if self.thrust > 0 {
            // three first-order shelves a decade apart = a tilt
            self.th_norm = 1.0;

            if self.thrust == 1 {
                // Med, as API's manual words it: a slight cut of the lows, a slight lift of the highs, flat mid-range
                self.set_shelf(0, 100.0, 1.68);
                self.th_b0[1] = 1.0;
                self.th_b1[1] = 0.0;
                self.th_a1[1] = 0.0;
                self.set_shelf(2, cmin(6300.0, 0.4 * self.fs), 1.68);
            } else {
                // Loud: 10 dB per decade right across the band (three 10 dB shelves a decade apart)
                self.set_shelf(0, 63.0, 3.1623);
                self.set_shelf(1, 630.0, 3.1623);
                self.set_shelf(2, cmin(6300.0, 0.4 * self.fs), 3.1623);
            }

            // level-match the tilt on a pink (music-like) spectrum, so switching Thrust changes what drives
            // the compressor rather than how much of it you get
            self.th_norm *= if self.thrust == 1 { 0.97 } else { 0.624 }; // (a * b == b * a exactly)
        }

        let mut mk_db = -12.0 + 36.0 * self.p_makeup;
        if mk_db.abs() < 0.05 {
            mk_db = 0.0;
        }
        self.mk_lin = 10.0f32.powf(mk_db / 20.0);

        self.dirty = false;
    }

    //==============================================================================
    fn shape(&self, v: f32) -> f32 {
        ((self.sat_a * v + self.sat_bias).tanh() - self.sat_tb) * self.sat_norm
    }

    // 1176 gain cell: series resistor into a JFET to ground. "xi" is the voltage in front of the resistor,
    // "uc" is how far the gate sits above pinch-off. Triode region: Id = K (uc vd - (1 - lam) vd^2 / 2);
    // for positive swings beyond uc the FET saturates. Negative swings never do (drain and source swap roles).
    fn fet_node(&self, xi: f32, uc: f32) -> f32 {
        if uc < 0.0 {
            // gate parked beyond pinch-off: nothing flows until a negative swing drags the drain far enough
            // below the gate to open the channel from the other end (square-law from there)
            if xi >= uc {
                return xi;
            }
            let w = ((1.0 + 2.0 * FET_C * (uc - xi)).sqrt() - 1.0) / FET_C;
            return uc - w;
        }

        let a2 = 0.5 * FET_C * (1.0 - self.fet_lam);
        let b = 1.0 + FET_C * uc;
        let ub = uc / (1.0 - 0.5 * self.fet_lam);
        let d = b * b - 4.0 * a2 * xi;

        if d > 0.0 {
            let vd = 2.0 * xi / (b + d.sqrt());
            if vd <= ub {
                return vd;
            }
        }

        xi - 0.5 * FET_C * ub * ub // saturated: constant current
    }

    // everything non-linear the style does to a sample, given the current control state
    fn nonlinear(&self, c: &Chan, v: f32) -> f32 {
        if self.sty == 1 {
            let k = self.in_g * self.fet_s * self.sat_m;
            let vd = self.fet_node(v * k, c.u + self.fet_u0);
            return self.shape(vd / (self.fet_s * self.sat_m)) / self.in_g;
        }

        if self.sty == 5 {
            // push-pull pair of exponential-law tubes: the difference of the two plate currents is a sinh of the
            // grid signal, and a slightly mismatched pair leaks a little even-order through as well
            let k = VARI_Z * self.sat_m;
            let z = clamp_t(v * k, -4.0, 4.0);
            let e = z.exp();
            let t = 0.5 * (e - 1.0 / e) + VARI_EPS * (0.5 * (e + 1.0 / e) - 1.0);
            return self.shape(t * c.g_lin / k);
        }

        if self.sty == 9 {
            // diode bridge, voltage-driven: per leg (x - v) / 47k = v / 2k8 + 2 Id sinh (v / nVt). With z = v / nVt and
            // k = what sets the small-signal gain 1 / (1 + k): solve z + k sinh z = X. Newton from the linear answer
            // (always from above, so it cannot overshoot).
            let m = self.br_s * self.sat_m;
            let k = 1.0 / c.g_lin - 1.0;
            let x = clamp_t(v * m, -12.0, 12.0);
            let mut z = x * c.g_lin;
            for _ in 0..4 {
                let e = z.exp();
                let sh = 0.5 * (e - 1.0 / e);
                let ch = 0.5 * (e + 1.0 / e);
                z -= (z + k * sh - x) / (1.0 + k * ch);
            }
            return self.shape(z / m);
        }

        self.shape(v * c.g_lin)
    }

    // colour residual: non-linear output minus the clean path, computed at 2x
    fn residual(&self, c: &mut Chan, v: f32) -> f32 {
        let t0 = ap(&mut c.up.x0, &mut c.up.y0, HC0, v);
        let u0 = ap(&mut c.up.x2, &mut c.up.y2, HC2, t0);
        let t1 = ap(&mut c.up.x1, &mut c.up.y1, HC1, v);
        let u1 = ap(&mut c.up.x3, &mut c.up.y3, HC3, t1);

        let r0;
        let r1;

        if self.sty == 6 {
            // pre-emphasis -> OTA input pair -> de-emphasis, keeping only what the OTA added
            let p0 = self.ross_pb0 * u0 + self.ross_pb1 * c.px - self.ross_pa1 * c.py + DN;
            c.px = u0;
            c.py = p0;
            c.dy += self.ross_de * ((self.shape(p0) - p0) - c.dy);
            r0 = c.dy * c.g_lin;
            let p1 = self.ross_pb0 * u1 + self.ross_pb1 * c.px - self.ross_pa1 * c.py + DN;
            c.px = u1;
            c.py = p1;
            c.dy += self.ross_de * ((self.shape(p1) - p1) - c.dy);
            r1 = c.dy * c.g_lin;
        } else {
            r0 = self.nonlinear(c, u0) - u0 * c.g_lin;
            r1 = self.nonlinear(c, u1) - u1 * c.g_lin;
        }

        let ta = ap(&mut c.dn.x0, &mut c.dn.y0, HC0, r1);
        let a = ap(&mut c.dn.x2, &mut c.dn.y2, HC2, ta);
        let tb = ap(&mut c.dn.x1, &mut c.dn.y1, HC1, r0);
        let b = ap(&mut c.dn.x3, &mut c.dn.y3, HC3, tb);
        let r = 0.5 * (a + b);

        let y = r - c.dc_x + self.dc_r * c.dc_y + DN;
        c.dc_x = r;
        c.dc_y = y;
        y
    }

    //==============================================================================
    // sidechain: pick the source, filter it, and measure it the way the style's detector does
    fn detect(&self, c: &mut Chan, x: f32) {
        let mut s = x;

        if self.sty == 6 || self.is_fb {
            s = c.fb;
        } else if self.sty == 2 {
            s = c.fb + self.opt_ff * x;
        }

        if self.hpf_on {
            let w = s - self.hpf_a1 * c.h1 - self.hpf_a2 * c.h2 + DN;
            s = self.hpf_b * (w - 2.0 * c.h1 + c.h2);
            c.h2 = c.h1;
            c.h1 = w;
        }

        if self.band_on {
            let y = self.bd_b0 * s + self.bd_b1 * c.bx1 + self.bd_b2 * c.bx2 - self.bd_a1 * c.by1 - self.bd_a2 * c.by2 + DN;
            c.bx2 = c.bx1;
            c.bx1 = s;
            c.by2 = c.by1;
            c.by1 = y;
            s = y;
        }

        if self.thrust > 0 {
            let y1 = self.th_b0[0] * s + self.th_b1[0] * c.t1x - self.th_a1[0] * c.t1y + DN;
            c.t1x = s;
            c.t1y = y1;
            let y2 = self.th_b0[1] * y1 + self.th_b1[1] * c.t2x - self.th_a1[1] * c.t2y + DN;
            c.t2x = y1;
            c.t2y = y2;
            let y3 = self.th_b0[2] * y2 + self.th_b1[2] * c.t3x - self.th_a1[2] * c.t3y + DN;
            c.t3x = y2;
            c.t3y = y3;
            s = y3 * self.th_norm;
        }

        if self.sty == 4 {
            // what reaches the rectifier before the sidechain VCA's gain reduction is applied (volts)
            c.q = self.gb_ag * s.abs();
        } else if self.sty == 1 {
            // what would reach the rectifier with no gain reduction (volts); the loop applies the FET's gain to it
            c.q = self.fet_a * s.abs() * self.in_g;
        } else if self.sty == 2 {
            // drive on the EL panel, both polarities (the light law is applied after the cell's own lag)
            c.q = cmax(s.abs() * self.in_g - 0.1, 0.0); // the panel needs some voltage before it lights at all
        } else if self.sty == 6 {
            // Phase splitter output in volts, then the two rectifiers. Each base sits behind a 0.01 uF cap: above
            // +0.55 V the base conducts and the cap takes the overshoot as charge (that charge x beta is what the
            // collector pulls off the envelope cap); below -0.55 V the clamp diode gives the charge back.
            let v = clamp_t(s * self.in_g * ROSS_VFS, -3.5, 3.5); // the OTA stage runs out of 9 V supply at about +/-3.5 V
            let mut q = 0.0f32;

            let oa = -v - c.ca;
            if oa > ROSS_VBE {
                let d = self.ross_ka * (oa - ROSS_VBE);
                c.ca += d;
                q += d;
            } else if oa < -ROSS_VBE {
                c.ca -= self.ross_ka * (-ROSS_VBE - oa);
            }

            let ob = v - c.cb;
            if ob > ROSS_VBE {
                let d = self.ross_kb * (ob - ROSS_VBE);
                c.cb += d;
                q += d;
            } else if ob < -ROSS_VBE {
                c.cb -= self.ross_kb * (-ROSS_VBE - ob);
            }

            c.q = q * ROSS_CC * self.ross_beta; // coulombs asked of the envelope cap this sample
        } else if self.is_vari {
            // linear full-wave rectifier working against the DC threshold: the control voltage it asks for grows
            // with output volts, not output dB, which is where the ever-steepening ratio comes from
            let zz = (s.abs() / self.thr_lin - 1.0) / VARI_SOFT;
            c.q = if zz < -6.0 { 0.0 } else { VARI_K * VARI_SOFT * softplus(zz) };
        } else if self.is_rms {
            c.pw += self.a_rms * (s * s - c.pw);
            c.q = 10.0 * (c.pw + 1.0e-20).log10();
        } else {
            c.q = 20.0 * (s.abs() + 1.0e-10).log10();
        }
    }

    //==============================================================================
    // charging current into the 1176's timing cap when it sits at u, plus how fast that current falls as u rises
    fn fet_current(&self, c: &Chan, a: f32, u: f32, slope_out: &mut f32) -> f32 {
        let uc = cmax(u + self.fet_u0, 0.0);
        let g = 1.0 / (1.0 + FET_C * uc);
        let sp = softplus((a * g + c.vb - self.fet_vth - u) / FET_VD);
        let sg = 1.0 - (-sp).exp();
        *slope_out = (sg / self.fet_ra) * ((if uc > 0.0 { a * FET_C * g * g } else { 0.0 }) + 1.0);
        (FET_VD / self.fet_ra) * sp
    }

    fn gain1176(&self, c: &mut Chan, a: f32) {
        // Timing cap after this sample, backward Euler: u = al (u_old + be i(u)). The charging current depends on u
        // directly (the diodes see the cap) and through the FET, whose gain sets how big the sidechain signal is.
        // i(u) only ever falls as u rises, so the answer is bracketed: Newton steps, bisection whenever one leaves
        // the bracket (the FET's pinch-off corner would otherwise make plain Newton bounce for ever).
        let u_old = c.u;
        let mut slope_out = 0.0f32;
        let mut lo = self.fet_al * u_old;
        let mut i = self.fet_current(c, a, lo, &mut slope_out);
        let mut hi = self.fet_al * (u_old + self.fet_be * i);
        let mut u = lo;

        if hi - lo > 1.0e-7 {
            for _ in 0..8 {
                let f = u - self.fet_al * (u_old + self.fet_be * i);
                if f > 0.0 {
                    hi = u;
                } else {
                    lo = u;
                }
                let mut next = u - f / (1.0 + self.fet_al * self.fet_be * slope_out);
                if next <= lo || next >= hi {
                    next = 0.5 * (lo + hi);
                }
                u = next;
                i = self.fet_current(c, a, u, &mut slope_out);
            }
        }

        c.u = self.fet_al * (u_old + self.fet_be * i);
        c.vb = self.fet_clamp_a * (c.vb - self.fet_clamp_b * i);

        c.g_lin = 1.0 / (1.0 + FET_C * cmax(c.u + self.fet_u0, 0.0));
        c.g_eff = 8.6858896 * c.g_lin.ln();
    }

    fn gain_la2a(&self, c: &mut Chan, drive: f32) {
        // f1 / f2 are kept in drive-voltage units; light ~ drive^3 and conductance ~ light^0.9 come afterwards,
        // which is why big transients are grabbed faster than small ones, like the real cell
        let lg = OPT_LOOP * (1.0 - c.g_lin);
        if drive > c.f1 {
            c.f1 += (self.a_att / (1.0 + lg)) * (drive - c.f1);
        } else {
            c.f1 += self.a_rel * (drive - c.f1);
        }

        // memory: the longer and brighter the cell has been lit, the slower the last part of the recovery
        if c.f1 > c.f2 {
            c.f2 += self.a_c2 * (c.f1 - c.f2);
        } else {
            c.f2 += (self.a_s2 / (1.0 - 0.25 * c.g_eff)) * (c.f1 - c.f2);
        }

        let s = cmax(c.f1, (1.0 - self.opt_w) * c.f1 + self.opt_w * c.f2);

        // cell conductance against the divider's source resistance: (drive^3)^0.9
        let gph = (s + 1.0e-12).powf(OPT_LOOP);
        c.g_lin = 1.0 / (1.0 + gph);
        c.g_eff = 8.6858896 * c.g_lin.ln();
    }

    fn gain_ross(&self, c: &mut Chan, charge: f32) {
        // rectifier collectors pull charge off the envelope cap (an Attack-mod resistor limits how fast);
        // 150k (Release) puts it back
        let dt = 1.0 / self.fs;
        let q_max = cmax(c.vc - 0.2, 0.0) / (self.ross_ratt + 30.0) * dt;
        c.vc += (ROSS_VS - c.vc) * dt / (self.ross_rrel * ROSS_C) - cmin(charge, q_max) / ROSS_C;
        c.vc = clamp_t(c.vc, ROSS_V0 + 0.08, ROSS_VS); // (floor = 40 dB of reduction)

        // Iabc, and with it the OTA's gain, follows the cap voltage linearly
        c.g_lin = (c.vc - ROSS_V0) / (ROSS_VS - ROSS_V0);
        c.g_eff = 8.6858896 * c.g_lin.ln();
    }

    // charging current into the G Bus timing network when it sits at V, plus how fast that current falls as V rises
    fn gb_current(&self, a: f32, v: f32, slope_out: &mut f32) -> f32 {
        let raw = a * (-GB_K * v).exp(); // rectified sidechain x ratio-stage gain
        let mut vr = raw - self.gb_c;
        let mut dv = raw * GB_K; // -d(vr)/dV

        if self.gb_two {
            // 2:1 only: the diode / 68k / 620k network eases in and takes the stage gain from 10.6 down to
            // about 2.6. Soft minimum of the two straight lines, fitted to the exact 1N4148 solution (within 0.03 V).
            let l2 = 0.255 * raw + 1.06;
            let d = (vr - l2) / 0.08;
            let t = (-d.abs()).exp();
            let w_small = 1.0 / (1.0 + t);
            let w1 = if d < 0.0 { w_small } else { 1.0 - w_small };
            vr = cmin(vr, l2) - 0.08 * (1.0 + t).ln();
            dv = raw * GB_K * (w1 + (1.0 - w1) * 0.255);
        }

        if vr > GB_VMAX {
            // op-amp out of swing
            vr = GB_VMAX;
            dv = 0.0;
        }

        let z = (vr - v) / GB_VD;
        let sp = if z < -4.0 { 0.0 } else { softplus(z) };
        let sg = 1.0 - (-sp).exp();
        *slope_out = (sg / self.gb_ra) * (dv + 1.0);
        (GB_VD / self.gb_ra) * sp
    }

    fn gain_gbus(&self, c: &mut Chan, a: f32) {
        // Cap voltage V = v1 + v2 after this sample, backward Euler: V = A0 + B0 i(V), where the charging current
        // i depends on V twice over - through the diode, and through the sidechain VCA whose gain is exp (-K V).
        // The 3M3 across the whole network takes V / 3M3 away from the charging current.
        // i(V) only falls as V rises, so the root is bracketed: Newton steps, bisection if one leaves the bracket.
        let b1 = self.gb_al1 * self.gb_be1 + self.gb_al2 * self.gb_be2;
        let a0 = (self.gb_al1 * c.v1 + self.gb_al2 * c.v2) / (1.0 + b1 / GB_R3);
        let b0 = b1 / (1.0 + b1 / GB_R3);

        let mut slope_out = 0.0f32;
        let mut lo = a0;
        let mut i = self.gb_current(a, lo, &mut slope_out);
        let mut hi = a0 + b0 * i;
        let mut v = lo;

        if hi - lo > 1.0e-7 {
            for _ in 0..8 {
                let f = v - a0 - b0 * i;
                if f > 0.0 {
                    hi = v;
                } else {
                    lo = v;
                }
                let mut next = v - f / (1.0 + b0 * slope_out);
                if next <= lo || next >= hi {
                    next = 0.5 * (lo + hi);
                }
                v = next;
                i = self.gb_current(a, v, &mut slope_out);
            }
        }

        let inet = i - v / GB_R3;
        c.v1 = self.gb_al1 * (c.v1 + self.gb_be1 * inet);
        c.v2 = self.gb_al2 * (c.v2 + self.gb_be2 * inet);

        c.g_eff = -(c.v1 + c.v2) / GB_S;
        c.g_lin = (c.g_eff * 0.11512925).exp();
    }

    fn gain670(&self, c: &mut Chan, want: f32) {
        // m1 = the control line (in dB, which for these tubes is just scaled volts), m2 / m3 = the memory caps.
        // The bridge can only charge the line; the 220k (and 470k) bleed it; the memory caps trade charge with it
        // through their 100k resistors, so after sustained limiting they hold the line up long after the fast cap lets go.
        if want > c.m1 {
            c.m1 += self.vm_ka * (want - c.m1);
        }
        c.m1 -= self.vm_dis * c.m1;

        if self.vm_b1 > 0.0 {
            let d = c.m1 - c.m2;
            c.m2 += self.vm_b1 * d;
            c.m1 -= self.vm_b1f * d;
        } else {
            c.m2 = c.m1;
        }
        if self.vm_b2 > 0.0 {
            let d = c.m1 - c.m3;
            c.m3 += self.vm_b2 * d;
            c.m1 -= self.vm_b2f * d;
        } else {
            c.m3 = c.m1;
        }

        c.m1 = clamp_t(c.m1, 0.0, 60.0);
        c.g_eff = -c.m1;
        c.g_lin = (c.g_eff * 0.11512925).exp();
    }

    fn gain_db(&self, c: &mut Chan, lev: f32) {
        let target = self.wanted(lev);

        if self.direct_gain {
            c.g1 = target;
            c.g_eff = target;
        } else {
            if self.sty == 9 {
                // 33609: plain RC attack; recovery heads for a reference 14 dB the other side of zero and is stopped
                // when it gets back to what the detector is asking for
                if target < c.g1 {
                    c.g1 += self.a_att * (target - c.g1);
                } else {
                    c.g1 = cmin(c.g1 + self.a_rel * (NV_REF - c.g1), target);
                }
            } else if target < c.g1 {
                c.g1 += (self.a_att / (1.0 + self.loop_gain(lev))) * (target - c.g1);
            } else {
                c.g1 += self.a_rel * (target - c.g1);
            }

            let mut g = c.g1;

            if self.w2 > 0.0 {
                if c.g1 < c.g2 {
                    c.g2 += self.a_c2 * (c.g1 - c.g2);
                } else if self.sty == 9 {
                    c.g2 = cmin(c.g2 + self.a_s2 * (NV_REF - c.g2), c.g1);
                } else {
                    c.g2 += self.a_s2 * (c.g1 - c.g2);
                }
                g = cmin(g, self.w2 * c.g2);
            } else {
                c.g2 = c.g1;
            }

            if self.w3 > 0.0 {
                if c.g1 < c.g3 {
                    c.g3 += self.a_c3 * (c.g1 - c.g3);
                } else {
                    c.g3 += self.a_s3 * (c.g1 - c.g3);
                }
                g = cmin(g, self.w3 * c.g3);
            } else {
                c.g3 = c.g1;
            }

            c.g_eff = g;
        }

        c.g_lin = (c.g_eff * 0.11512925).exp();
    }

    fn follow(&self, c: &mut Chan, q: f32) {
        if self.sty == 1 {
            self.gain1176(c, q);
        } else if self.sty == 2 {
            self.gain_la2a(c, q);
        } else if self.sty == 6 {
            self.gain_ross(c, q);
        } else if self.sty == 4 {
            self.gain_gbus(c, q);
        } else if self.sty == 5 {
            self.gain670(c, q);
        } else {
            self.gain_db(c, q);
        }
    }

    fn render(&self, c: &mut Chan, x: f32) -> f32 {
        let mut u = x * c.g_lin;

        if self.sty == 6 {
            // what is left of the pre/de-emphasis pair: one pole at 9 kHz
            c.lp += self.ross_lp * (u - c.lp) + DN;
            u = c.lp;
        }

        c.fb = u;
        if !self.sat_on {
            return u;
        }
        u + self.residual(c, x) * self.sat_res
    }
}

/// One compressor lane (`CompLane`).
#[derive(Debug, Clone, Default)]
pub struct CompLane {
    c_l: Chan,
    c_r: Chan,
    m: LaneModel,
}

impl CompLane {
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        use lane_param::*;
        let m = &mut self.m;
        match index {
            P_STYLE => {
                m.p_style_v = v;
                m.dirty = true;
            }
            P_AMOUNT => {
                m.p_amount = v;
                m.dirty = true;
            }
            P_RATIO => {
                m.p_ratio = v;
                m.dirty = true;
            }
            P_ATTACK => {
                m.p_attack = v;
                m.dirty = true;
            }
            P_RELEASE => {
                m.p_release = v;
                m.dirty = true;
            }
            P_MAKEUP => {
                m.p_makeup = v;
                m.dirty = true;
            }
            P_MIX => {
                m.p_mix = v;
                m.dirty = true;
            }
            P_KNEE => {
                m.p_knee = v;
                m.dirty = true;
            }
            P_SC_HPF => {
                m.p_hpf = v;
                m.dirty = true;
            }
            P_COLOR => {
                m.p_color = v;
                m.dirty = true;
            }
            P_LINK => {
                m.p_link = v;
                m.dirty = true;
            }
            P_AUTO_GAIN => {
                m.p_auto = v > 0.5;
                m.dirty = true;
            }
            P_BYPASS => m.p_bypass = v > 0.5,
            P_VARIANT => {
                m.p_variant = v;
                m.dirty = true;
            }
            P_DET_MODE => {
                m.p_det = v;
                m.dirty = true;
            }
            P_CHAN_MODE => {
                m.p_chan = v;
                m.dirty = true;
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- init (cmajor init())
    pub fn prepare(&mut self, sample_rate: f64) {
        let m = &mut self.m;
        m.fs = sample_rate as f32;
        m.smooth = 1.0 - (-1.0 / (0.008 * m.fs)).exp();
        m.dc_r = 1.0 - (2.0 * PI_F * 8.0 / m.fs);
        m.a_ag = 1.0 - (-1.0 / (3.0 * m.fs)).exp();

        self.c_l = Chan::default();
        self.c_r = Chan::default();
        self.c_l.g_lin = 1.0;
        self.c_r.g_lin = 1.0;
        self.c_l.vc = ROSS_VS;
        self.c_r.vc = ROSS_VS;

        m.dirty = true;
    }

    /// Once every 16 samples: re-derive the model from the knobs.
    pub fn control_tick(&mut self) {
        if self.m.dirty {
            self.m.update_params();
        }
        self.update_shaper();
        let m = &mut self.m;
        m.thr_lin = db_to_lin(m.thr);
        m.gb_ag = (m.gb_c + GB_S) * 1.1220185 / m.thr_lin; // Threshold = level that gives 1 dB of reduction
        m.ag_db = m.ag_num / (m.ag_den + 1.0e-9);
        m.out_gain_t = m.mk_lin * (0.11512925 * (if m.p_auto { cmin(0.9 * m.ag_db, 15.0) } else { 0.0 })).exp();
    }

    pub fn smooth_tick(&mut self) {
        let m = &mut self.m;
        m.thr += m.smooth * (m.thr_t - m.thr);
        m.in_g += m.smooth * (m.in_gt - m.in_g);
        m.out_gain += m.smooth * (m.out_gain_t - m.out_gain);
    }

    /// The compressed signal for this sample. `pwr` is the whole plugin's input power, which
    /// is what weights the auto-gain average -- it is the same number for every lane.
    ///
    /// The mid-only / side-only PASS-THROUGH is deliberately not done here: with three lanes
    /// summing, doing it per lane would add the untouched channel three times. The wrapper
    /// does it once. What is done here is zeroing the gain on the channel this mode ignores,
    /// because that is a property of this lane's detector.
    pub fn process_sample(&mut self, p0: f32, p1: f32, ch_mode: i32, pwr: f32) -> (f32, f32) {
        let m = &self.m;
        let (c_l, c_r) = (&mut self.c_l, &mut self.c_r);
        m.detect(c_l, p0);
        m.detect(c_r, p1);

        let lk = if ch_mode >= 2 { 0.0 } else { m.p_link };
        // link: the louder channel's detector reading drives both ...
        let lkq = if m.sty == 8 { 0.0 } else { lk };
        let top = cmax(c_l.q, c_r.q);
        m.follow(c_l, c_l.q + lkq * (top - c_l.q));
        m.follow(c_r, c_r.q + lkq * (top - c_r.q));

        // ... except on the 2500, where each channel keeps its own detector and the two CONTROL VOLTAGES are mixed,
        // so 100 % gives both channels the average of the two gain reductions
        if m.sty == 8 && lk > 0.0 {
            let avg = 0.5 * (c_l.g_eff + c_r.g_eff);
            c_l.g_eff += lk * (avg - c_l.g_eff);
            c_l.g_lin = (c_l.g_eff * 0.11512925).exp();
            c_r.g_eff += lk * (avg - c_r.g_eff);
            c_r.g_lin = (c_r.g_eff * 0.11512925).exp();
        }

        if ch_mode == 2 {
            c_r.g_eff = 0.0;
            c_r.g_lin = 1.0;
        }
        if ch_mode == 3 {
            c_l.g_eff = 0.0;
            c_l.g_lin = 1.0;
        }

        // Auto Gain: loudness-weighted average of the gain reduction over a few seconds
        // (weighting by signal power means silence neither resets it nor drags it around)
        let (a_ag, ag_num, ag_den) = (m.a_ag, m.ag_num, m.ag_den);
        let new_num = ag_num + a_ag * (pwr * -0.5 * (c_l.g_eff + c_r.g_eff) - ag_num);
        let new_den = ag_den + a_ag * (pwr - ag_den);

        let w0 = m.render(c_l, p0) * m.out_gain;
        let w1 = m.render(c_r, p1) * m.out_gain;
        self.m.ag_num = new_num;
        self.m.ag_den = new_den;
        (w0, w1)
    }

    /// The more negative of the two channels, for the gain-reduction meter.
    pub fn min_gain_reduction_db(&self) -> f32 {
        cmin(self.c_l.g_eff, self.c_r.g_eff)
    }

    fn update_shaper(&mut self) {
        let gr = clamp01(-0.5 * (self.c_l.g_eff + self.c_r.g_eff) / 20.0);
        let m = &mut self.m;
        m.sat_bias = m.sat_b0 + m.sat_b1 * gr;
        m.sat_tb = m.sat_bias.tanh();
        let a = cmax(m.sat_a, 0.001);
        m.sat_norm = 1.0 / (a * (1.0 - m.sat_tb * m.sat_tb));
    }
}

// ================================================================ CompEngineDSP
pub const NUM_LANES: usize = 3;

// ---------------------------------------------------------------- parameters
// Lane A keeps the ORIGINAL ids and the original order; lanes B and C are appended.
pub const P_STYLE: usize = 0;
pub const P_AMOUNT: usize = 1;
pub const P_RATIO: usize = 2;
pub const P_ATTACK: usize = 3;
pub const P_RELEASE: usize = 4;
pub const P_MAKEUP: usize = 5;
pub const P_DRY: usize = 6;
pub const P_KNEE: usize = 7;
pub const P_SC_HPF: usize = 8;
pub const P_COLOR: usize = 9;
pub const P_LINK: usize = 10;
pub const P_AUTO_GAIN: usize = 11;
pub const P_BYPASS: usize = 12;
pub const P_VARIANT: usize = 13;
pub const P_DET_MODE: usize = 14;
pub const P_CHAN_MODE: usize = 15;
pub const P_A_LEVEL: usize = 16;
pub const P_B_ON: usize = 17;
pub const P_B_LEVEL: usize = 18;
pub const P_C_ON: usize = 19;
pub const P_C_LEVEL: usize = 20;
pub const P_B_STYLE: usize = 21;
pub const P_B_DET_MODE: usize = 33;
pub const P_C_STYLE: usize = 34;
pub const P_C_DET_MODE: usize = 46;
pub const P_A_ON: usize = 47;
pub const P_DRY_ON: usize = 48;
pub const P_SPLIT: usize = 49;
pub const P_XOVER_LO: usize = 50;
pub const P_XOVER_HI: usize = 51;
pub const NUM_PARAMS: usize = 52;

// enum Split
pub const SPLIT_PARALLEL: i64 = 0;
pub const SPLIT_MULTIBAND: i64 = 1;
pub const SPLIT_SERIAL: i64 = 2;

pub fn knob_to_xover_lo(v: f32) -> f32 {
    40.0 * 25.0f32.powf(clamp01(v)) //  40 Hz ..  1 kHz
}
pub fn knob_to_xover_hi(v: f32) -> f32 {
    800.0 * 15.0f32.powf(clamp01(v)) // 800 Hz .. 12 kHz
}

/// The lane knob -> gain in dB. 0.75 is unity, so a lane can be pushed 10 dB up as well
/// as pulled all the way down, and 0 is silence rather than a very small number.
pub fn knob_to_lane_db(v: f32) -> f32 {
    if v <= 0.0 {
        return -60.0;
    }
    (v - 0.75) * 40.0
}

/// Lanes B and C list their fields in the same order as lane A's enum, minus the four
/// that are global. This maps one to the other so the lane code stays single-copy.
fn lane_field_for(offset: usize) -> usize {
    use lane_param::*;
    const MAP: [usize; 13] =
        [P_STYLE, P_AMOUNT, P_RATIO, P_ATTACK, P_RELEASE, P_MAKEUP, P_KNEE, P_SC_HPF, P_COLOR, P_LINK, P_AUTO_GAIN, P_VARIANT, P_DET_MODE];
    MAP[offset.min(12)]
}

/// Shared by the lane faders and the dry one, so they behave identically.
fn gain_of(v: f32) -> f32 {
    // Exactly 1.0f at the default, not approximately.
    if (0.7499..=0.7501).contains(&v) {
        return 1.0;
    }
    if v <= 0.0 { 0.0 } else { (0.11512925 * knob_to_lane_db(v)).exp() }
}

// ---- the crossover, one per channel
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }
}

impl Biquad {
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
    fn set_coefs(&mut self, c: &Biquad) {
        self.b0 = c.b0;
        self.b1 = c.b1;
        self.b2 = c.b2;
        self.a1 = c.a1;
        self.a2 = c.a2;
    }
}

/// 2nd-order Butterworth (Q = 1/sqrt 2) sections; two in a row make an LR4. Bilinear,
/// prewarped, so the crossover is where the knob says at any sample rate.
fn butter(hz: f32, fs: f32, high: bool) -> Biquad {
    // `fs * 0.45` is float x double in the C++: a double product.
    let w = (3.14159265358979f64 * f64::from(hz).min_cpp(f64::from(fs) * 0.45) / f64::from(fs)).tan();
    let k = 1.0 / 2.0f64.sqrt();
    let n = 1.0 / (1.0 + w / k + w * w);
    let mut q = Biquad::default();
    if high {
        q.b0 = n as f32;
        q.b1 = (-2.0 * n) as f32;
        q.b2 = n as f32;
    } else {
        q.b0 = (w * w * n) as f32;
        q.b1 = (2.0 * w * w * n) as f32;
        q.b2 = (w * w * n) as f32;
    }
    q.a1 = (2.0 * (w * w - 1.0) * n) as f32;
    q.a2 = ((1.0 - w / k + w * w) * n) as f32;
    q
}

/// The allpass an LR4 pair at this frequency sums to.
fn allpass(hz: f32, fs: f32) -> Biquad {
    let w = (3.14159265358979f64 * f64::from(hz).min_cpp(f64::from(fs) * 0.45) / f64::from(fs)).tan();
    let k = 1.0 / 2.0f64.sqrt();
    let n = 1.0 / (1.0 + w / k + w * w);
    let mut q = Biquad::default();
    q.b0 = ((1.0 - w / k + w * w) * n) as f32;
    q.b1 = (2.0 * (w * w - 1.0) * n) as f32;
    q.b2 = 1.0;
    q.a1 = q.b1;
    q.a2 = q.b0;
    q
}

/// `std::min` on doubles, with the C++ comparison.
trait MinCpp {
    fn min_cpp(self, b: Self) -> Self;
}
impl MinCpp for f64 {
    #[inline]
    fn min_cpp(self, b: f64) -> f64 {
        if b < self { b } else { self }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Crossover {
    lo1a: Biquad, // LR4 at the lower frequency
    lo1b: Biquad,
    hi1a: Biquad,
    hi1b: Biquad,
    lo2a: Biquad, // LR4 at the upper frequency
    lo2b: Biquad,
    hi2a: Biquad,
    hi2b: Biquad,
    ap2: Biquad, // the low band's phase match to the upper split
}

impl Crossover {
    fn split(&mut self, x: f32, out: &mut [f32; NUM_LANES]) {
        let low = self.ap2.run(self.lo1b.run(self.lo1a.run(x)));
        let rest = self.hi1b.run(self.hi1a.run(x));
        out[0] = low;
        out[1] = self.lo2b.run(self.lo2a.run(rest));
        out[2] = self.hi2b.run(self.hi2a.run(rest));
    }
}

/// Three `CompLane`s in PARALLEL (or multiband, or serial), summed, then blended with the dry
/// signal (`CompEngineDSP`).
#[derive(Debug, Clone)]
pub struct CompEngineDsp {
    lanes: [CompLane; NUM_LANES],
    xover: [Crossover; 2],
    multiband: bool,
    serial: bool,
    x_lo_knob: f32,
    x_hi_knob: f32,
    x_lo: f32,
    x_hi: f32,

    lane_on: [bool; NUM_LANES],
    level: [f32; NUM_LANES], // 0.75 = unity

    // 0 = silent, which is where the old "mix 100 % wet" default lands.
    p_dry: f32,
    dry_on: bool,
    p_bypass: bool,
    ch_mode: i32,
    ms_on: bool,
    ms_init: bool,
    dry_s: f32,
    ms_fade: f32,
    byp_s: f32,
    smooth_init: bool,
    fs: f32,
    smooth: f32,

    ctl: i32,
    meter_count: i32,
    meter_len: i32,
    gr_min: f32,
    in_pk: f32,
    out_pk: f32,
    lane_gr_min: [f32; NUM_LANES],

    meter_gr: f32,
    meter_in: f32,
    meter_out: f32,
    meter_lane_gr: [f32; NUM_LANES],

    // Rust-only, for the per-block readouts (not in the C++): peaks and deepest lane GR since
    // the last `take_block_meters`.
    blk_in_pk: f32,
    blk_out_pk: f32,
    blk_lane_gr: [f32; NUM_LANES],
}

impl Default for CompEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-block meter readings (Rust-only; see `CompEngineDsp::take_block_meters`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockMeters {
    /// Input peak (linear) over the block.
    pub in_peak: f32,
    /// Output peak (linear) over the block.
    pub out_peak: f32,
    /// Deepest gain reduction per lane over the block, in dB (<= 0); 0 for a lane that is off.
    pub lane_gr_db: [f32; NUM_LANES],
}

impl CompEngineDsp {
    pub fn new() -> Self {
        CompEngineDsp {
            // Default-constructed as in the C++; lane B and C's own defaults (1176, LA-2A) come
            // from the plugin parameters when they are pushed.
            lanes: Default::default(),
            xover: [Crossover::default(); 2],
            multiband: false,
            serial: false,
            x_lo_knob: 0.467,
            x_hi_knob: 0.42, // ~180 Hz and ~2.5 kHz
            x_lo: 180.0,
            x_hi: 2500.0,
            lane_on: [true, false, false],
            level: [0.75, 0.75, 0.75],
            p_dry: 0.0,
            dry_on: true,
            p_bypass: false,
            ch_mode: 0,
            ms_on: false,
            ms_init: false,
            dry_s: 0.0,
            ms_fade: 1.0,
            byp_s: 0.0,
            smooth_init: false,
            fs: 48000.0,
            smooth: 0.002,
            ctl: 0,
            meter_count: 0,
            meter_len: 1600,
            gr_min: 0.0,
            in_pk: 0.0,
            out_pk: 0.0,
            lane_gr_min: [0.0; NUM_LANES],
            meter_gr: 0.0,
            meter_in: -100.0,
            meter_out: -100.0,
            meter_lane_gr: [0.0; NUM_LANES],
            blk_in_pk: 0.0,
            blk_out_pk: 0.0,
            blk_lane_gr: [0.0; NUM_LANES],
        }
    }

    pub fn set_parameter(&mut self, index: usize, v: f32) {
        // ---- global
        match index {
            P_DRY => {
                self.p_dry = v;
                return;
            }
            P_DRY_ON => {
                self.dry_on = v > 0.5;
                return;
            }
            P_SPLIT => {
                // `std::lround`: half away from zero.
                let m = v.round() as i64;
                self.multiband = m == SPLIT_MULTIBAND;
                self.serial = m == SPLIT_SERIAL;
                return;
            }
            P_XOVER_LO => {
                self.x_lo_knob = v;
                self.update_crossover();
                return;
            }
            P_XOVER_HI => {
                self.x_hi_knob = v;
                self.update_crossover();
                return;
            }
            P_BYPASS => {
                self.p_bypass = v > 0.5;
                return;
            }
            P_CHAN_MODE => {
                self.ch_mode = (v + 0.5) as i32;
                return;
            }
            P_A_LEVEL => {
                self.level[0] = v;
                return;
            }
            P_B_LEVEL => {
                self.level[1] = v;
                return;
            }
            P_C_LEVEL => {
                self.level[2] = v;
                return;
            }
            P_A_ON => {
                self.lane_on[0] = v > 0.5;
                return;
            }
            P_B_ON => {
                self.lane_on[1] = v > 0.5;
                return;
            }
            P_C_ON => {
                self.lane_on[2] = v > 0.5;
                return;
            }
            _ => {}
        }

        // ---- per lane. Lane A's block is the original enum, unchanged.
        if index <= P_DET_MODE {
            self.lanes[0].set_parameter(index, v);
            return;
        }
        if (P_B_STYLE..=P_B_DET_MODE).contains(&index) {
            self.lanes[1].set_parameter(lane_field_for(index - P_B_STYLE), v);
            return;
        }
        if (P_C_STYLE..=P_C_DET_MODE).contains(&index) {
            self.lanes[2].set_parameter(lane_field_for(index - P_C_STYLE), v);
        }
    }

    // ---------------------------------------------------------------- metering
    pub fn gain_reduction_db(&self) -> f32 {
        self.meter_gr
    }
    pub fn input_peak_db(&self) -> f32 {
        self.meter_in
    }
    pub fn output_peak_db(&self) -> f32 {
        self.meter_out
    }
    /// Per-lane gain reduction, for the mixer strip's little meters.
    pub fn lane_gain_reduction_db(&self, lane: i32) -> f32 {
        self.meter_lane_gr[juce_jlimit_lane(lane)]
    }
    pub fn is_lane_on(&self, lane: i32) -> bool {
        self.lane_on[juce_jlimit_lane(lane)]
    }
    pub fn xover_lo_hz(&self) -> f32 {
        self.x_lo
    }
    pub fn xover_hi_hz(&self) -> f32 {
        self.x_hi
    }
    pub fn is_multiband(&self) -> bool {
        self.multiband
    }
    pub fn is_serial(&self) -> bool {
        self.serial
    }
    pub fn sample_rate(&self) -> f32 {
        self.fs
    }
    pub fn is_bypassed(&self) -> bool {
        self.p_bypass
    }

    /// Rust-only: the meters since the last call, then restarts them.
    pub fn take_block_meters(&mut self) -> BlockMeters {
        let mut lane_gr_db = self.blk_lane_gr;
        for (i, g) in lane_gr_db.iter_mut().enumerate() {
            // As the C++ lane meters: an off lane, or a bypassed engine, shows none.
            if self.p_bypass || !self.lane_on[i] {
                *g = 0.0;
            }
        }
        let m = BlockMeters { in_peak: self.blk_in_pk, out_peak: self.blk_out_pk, lane_gr_db };
        self.blk_in_pk = 0.0;
        self.blk_out_pk = 0.0;
        self.blk_lane_gr = [0.0; NUM_LANES];
        m
    }

    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = sample_rate as f32;
        self.smooth = 1.0 - (-1.0 / (0.008 * self.fs)).exp();
        self.meter_len = (self.fs / 30.0) as i32;

        for l in &mut self.lanes {
            l.prepare(sample_rate);
        }

        for x in &mut self.xover {
            *x = Crossover::default();
        }
        self.update_crossover();

        self.ctl = 0;
        self.meter_count = 0;
        self.gr_min = 0.0;
        self.in_pk = 0.0;
        self.out_pk = 0.0;
        self.ms_init = false;
        self.smooth_init = false;
    }

    // ---------------------------------------------------------------- main
    /// Processes `num_samples` frames of the first `num_channels` buffers (the caller passes at
    /// most 2 and buffers at least `num_samples` long).
    #[allow(clippy::needless_range_loop)] // `n` indexes both channels, as the C++ loop does
    pub fn process(&mut self, channels: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        if num_channels == 0 || channels.is_empty() {
            return;
        }
        let stereo = num_channels >= 2 && channels.len() >= 2;

        for n in 0..num_samples {
            if self.ctl == 0 {
                for i in 0..NUM_LANES {
                    if self.lane_on[i] {
                        self.lanes[i].control_tick();
                    }
                }
            }

            self.ctl += 1;
            if self.ctl >= 16 {
                self.ctl = 0;
            }

            for i in 0..NUM_LANES {
                if self.lane_on[i] {
                    self.lanes[i].smooth_tick();
                }
            }

            let ms_want = self.ch_mode > 0;
            if !self.ms_init {
                self.ms_on = ms_want;
                self.ms_init = true;
            }

            // The wet path is faded out while the channel mode changes underneath it, so the
            // mid/side switch cannot click.
            self.ms_fade += self.smooth * ((if ms_want != self.ms_on { 0.0 } else { 1.0 }) - self.ms_fade);
            if ms_want != self.ms_on && self.ms_fade < 0.003 {
                self.ms_on = ms_want;
            }

            let dry_t = if self.dry_on { gain_of(self.p_dry) } else { 0.0 };
            let byp_t = if self.p_bypass { 1.0 } else { 0.0 };

            // Snap on the very first sample rather than ramping to it.
            if !self.smooth_init {
                self.dry_s = dry_t;
                self.byp_s = byp_t;
                self.smooth_init = true;
            }

            self.dry_s += self.smooth * (dry_t - self.dry_s);
            self.byp_s += self.smooth * (byp_t - self.byp_s);

            let x0 = channels[0][n];
            let x1 = if stereo { channels[1][n] } else { x0 };

            // channel mode: work on left/right, or on mid/side (optionally leaving one of the two untouched)
            let ms_on = self.ms_on;
            let p0 = if ms_on { 0.5 * (x0 + x1) } else { x0 };
            let p1 = if ms_on { 0.5 * (x0 - x1) } else { x1 };

            let pwr = x0 * x0 + x1 * x1;

            // ---- the parallel sum. Every lane sees the same input; none sees another's output.
            let mut s0 = 0.0f32;
            let mut s1 = 0.0f32;
            let mut deepest = 0.0f32;

            // Multiband: each lane gets its own band. A band whose lane is switched off passes
            // through uncompressed at its fader level.
            let mut band0 = [p0, p0, p0];
            let mut band1 = [p1, p1, p1];
            if self.multiband {
                self.xover[0].split(p0, &mut band0);
                self.xover[1].split(p1, &mut band1);
            }

            if self.serial {
                // A -> B -> C. The first lane sees the same detector power as in Parallel;
                // the ones after it measure what they are actually given.
                let mut c0 = p0;
                let mut c1 = p1;
                let mut first = true;
                for i in 0..NUM_LANES {
                    if !self.lane_on[i] {
                        continue;
                    }
                    let lp = if first { pwr } else { (if ms_on { 2.0 } else { 1.0 }) * (c0 * c0 + c1 * c1) };
                    first = false;

                    let (w0, w1) = self.lanes[i].process_sample(c0, c1, self.ch_mode, lp);
                    let g = self.level_gain(i);
                    c0 = g * w0;
                    c1 = g * w1;

                    let gr = self.lanes[i].min_gain_reduction_db();
                    deepest = cmin(deepest, gr);
                    self.lane_gr_min[i] = cmin(self.lane_gr_min[i], gr);
                    self.blk_lane_gr[i] = cmin(self.blk_lane_gr[i], gr);
                }
                s0 = c0;
                s1 = c1;
            }

            let mut i = 0;
            while i < NUM_LANES && !self.serial {
                if self.multiband && !self.lane_on[i] {
                    let g = self.level_gain(i);
                    s0 += g * band0[i];
                    s1 += g * band1[i];
                    i += 1;
                    continue;
                }

                if !self.lane_on[i] {
                    i += 1;
                    continue;
                }

                let lane_pwr = if self.multiband { band0[i] * band0[i] + band1[i] * band1[i] } else { pwr };
                let (w0, w1) = self.lanes[i].process_sample(band0[i], band1[i], self.ch_mode, lane_pwr);

                // Lane A at unity multiplies by exactly 1.0f.
                let g = self.level_gain(i);
                s0 += g * w0;
                s1 += g * w1;

                let gr = self.lanes[i].min_gain_reduction_db();
                deepest = cmin(deepest, gr);
                self.lane_gr_min[i] = cmin(self.lane_gr_min[i], gr);
                self.blk_lane_gr[i] = cmin(self.blk_lane_gr[i], gr);
                i += 1;
            }

            // mid only / side only: the untouched channel passes once, not once per lane
            if self.ch_mode == 2 {
                s1 = p1;
            }
            if self.ch_mode == 3 {
                s0 = p0;
            }

            let wl = if ms_on { s0 + s1 } else { s0 };
            let wr = if ms_on { s0 - s1 } else { s1 };

            // Dry is its own fader beside the lanes, and the result is their SUM.
            let pl = x0 * self.dry_s + wl * self.ms_fade;
            let pr = x1 * self.dry_s + wr * self.ms_fade;

            let y0 = pl + self.byp_s * (x0 - pl);
            let y1 = pr + self.byp_s * (x1 - pr);

            channels[0][n] = y0;
            if stereo {
                channels[1][n] = y1;
            }

            // (the C++ writes the input scope here: editor only, not translated)

            // meters, ~30 per second, the three sends spread across the window
            self.gr_min = cmin(self.gr_min, deepest);
            self.in_pk = cmax(self.in_pk, cmax(x0.abs(), x1.abs()));
            self.out_pk = cmax(self.out_pk, cmax(y0.abs(), y1.abs()));
            // Rust-only block meters.
            self.blk_in_pk = cmax(self.blk_in_pk, cmax(x0.abs(), x1.abs()));
            self.blk_out_pk = cmax(self.blk_out_pk, cmax(y0.abs(), y1.abs()));

            self.meter_count += 1;

            if self.meter_count == self.meter_len / 3 {
                self.meter_in = 20.0 * (self.in_pk + 1.0e-6).log10();
                self.in_pk = 0.0;
            } else if self.meter_count == (2 * self.meter_len) / 3 {
                self.meter_out = 20.0 * (self.out_pk + 1.0e-6).log10();
                self.out_pk = 0.0;
            } else if self.meter_count >= self.meter_len {
                self.meter_gr = if self.p_bypass { 0.0 } else { -self.gr_min };
                self.gr_min = 0.0;

                for i in 0..NUM_LANES {
                    self.meter_lane_gr[i] = if self.p_bypass || !self.lane_on[i] { 0.0 } else { -self.lane_gr_min[i] };
                    self.lane_gr_min[i] = 0.0;
                }

                self.meter_count = 0;
            }
        }
    }

    fn level_gain(&self, i: usize) -> f32 {
        gain_of(self.level[i])
    }

    fn update_crossover(&mut self) {
        self.x_lo = knob_to_xover_lo(self.x_lo_knob);
        self.x_hi = cmax(knob_to_xover_hi(self.x_hi_knob), self.x_lo * 2.0);
        let lp1 = butter(self.x_lo, self.fs, false);
        let hp1 = butter(self.x_lo, self.fs, true);
        let lp2 = butter(self.x_hi, self.fs, false);
        let hp2 = butter(self.x_hi, self.fs, true);
        let apc = allpass(self.x_hi, self.fs);
        for x in &mut self.xover {
            x.lo1a.set_coefs(&lp1);
            x.lo1b.set_coefs(&lp1);
            x.hi1a.set_coefs(&hp1);
            x.hi1b.set_coefs(&hp1);
            x.lo2a.set_coefs(&lp2);
            x.lo2b.set_coefs(&lp2);
            x.hi2a.set_coefs(&hp2);
            x.hi2b.set_coefs(&hp2);
            x.ap2.set_coefs(&apc);
        }
    }
}

/// `juce_jlimit_lane`: the C++ helper's own name (it is not a JUCE call).
fn juce_jlimit_lane(i: i32) -> usize {
    (if i < 0 {
        0
    } else if i >= NUM_LANES as i32 {
        NUM_LANES as i32 - 1
    } else {
        i
    }) as usize
}
