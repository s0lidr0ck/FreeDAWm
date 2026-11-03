//! DistEngineDSP -- Engine 2: twelve component-level drive circuits under one selector.
//! Line-by-line translation of `Engine 2 - Dist/Source/DistEngineDSP.h` (itself a port of
//! `DistEngine.cmajor`).
//!
//! - 0 Tube Screamer, 1 RAT, 2 Big Muff, 3 Fuzz Face, 4 DS-1, 5 Klon, 6 Blues Breaker,
//!   7 Guv'nor, 8 Tube Driver (12AX7), 9 Blues Driver (JFET + discrete gain block),
//!   10 MXR Distortion+ (741 + Ge to ground), 11 Fulltone OCD (MOSFET clippers).
//!
//! Every circuit is solved at the component level: transistor stages with Ebers-Moll BJTs and
//! Newton-Raphson nodal solves, op-amps with gain-bandwidth / slew / rail limits, diode clippers
//! solved on the Shockley equation. Only the selected circuit runs, 4x oversampled.
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - `float` and `double` are kept exactly where the C++ has them.
//! - `clampT` returns NaN for NaN (both comparisons false), unlike `f64::clamp` (which also
//!   panics on NaN bounds, as `groundClip`'s `±|vs|` would be): [`clamp_t`]. `std::max(a, b)` is
//!   `a < b ? b : a`: [`max_t`].
//! - `int (v + 0.5f)` on a choice value: Rust `as` truncates toward zero like the C++ cast for
//!   every finite value; non-finite values never reach here (the plugin wrapper clamps first, as
//!   the APVTS does in the C++).
//! - The C++ `processChannel` calls `resetStates()` (both channels) from inside its NaN guard;
//!   nothing after the guard reads the solver state it resets, so here the guard reports the NaN
//!   and `process` resets right after the call: the same values in the same order.
//! - State is split into the shared settings ([`Settings`]) and the two [`Channel`]s so a circuit
//!   core can read the one while writing the other; the arithmetic is unchanged.
//! - Everything is fixed-size: nothing here allocates.

// The C++ names, constants and argument lists are kept so the two can be diffed.
#![allow(clippy::too_many_arguments)]

// The C++ `pi` literal (3.14159265358979323846) is exactly `std::f64::consts::PI`.
const PI: f64 = std::f64::consts::PI;

// ---------------------------------------------------------------- parameter events (declaration order = param1..param54)
pub const P_DRIVE: usize = 0;
pub const P_TONE: usize = 1;
pub const P_LEVEL: usize = 2;
pub const P_CIRCUIT: usize = 3;
pub const P_BYPASS: usize = 4;
pub const P_TRIM: usize = 5;
pub const P_LINK: usize = 6;
pub const P_TS_LO: usize = 7;
pub const P_OCD_HI: usize = 30;
pub const P_TS_MODEL: usize = 31;
pub const P_TS_DIODE: usize = 32;
pub const P_RAT_MODEL: usize = 33;
pub const P_RAT_DIODE: usize = 34;
pub const P_RAT_CHIP: usize = 35;
pub const P_RAT_FAT: usize = 36;
pub const P_MUFF_VER: usize = 37;
pub const P_MUFF_DIODE: usize = 38;
pub const P_MUFF_MIDS: usize = 39;
pub const P_FF_TRANS: usize = 40;
pub const P_FF_BIAS: usize = 41;
pub const P_DS1_DIODE: usize = 42;
pub const P_DS1_VOICE: usize = 43;
pub const P_KLON_DIODE: usize = 44;
pub const P_BB_GAIN: usize = 45;
pub const P_GOV_CLIP: usize = 46;
pub const P_GOV_BASS: usize = 47;
pub const P_GOV_MID: usize = 48;
pub const P_TD_PLATE: usize = 49;
pub const P_TD_BIAS: usize = 50;
pub const P_BD_MODE: usize = 51;
pub const P_DP_DIODE: usize = 52;
pub const P_OCD_MODE: usize = 53;
pub const NUM_PARAMS: usize = 54;

/// `std::clamp`-free `clampT`: `v < lo ? lo : (v > hi ? hi : v)` (NaN passes through).
#[inline(always)]
fn clamp_t<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `std::max (a, b)`: `a < b ? b : a`.
#[inline(always)]
fn max_t(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

// ---------------------------------------------------------------- shared constants
const VT: f64 = 25.85e-3;
const VCC: f64 = 9.0;
const INPUT_VOLTS: f64 = 0.15;

// Tube Screamer (unchanged from the standalone build)
const TS_R9: f64 = 1000.0;
const TS_C7: f64 = 0.22e-6;
const TS_R10: f64 = 220.0;
const TS_C8: f64 = 0.22e-6;
const TS_R11: f64 = 1000.0;
const TS_RPOT_TONE: f64 = 20000.0;
const TS_RPOT_DRIVE: f64 = 500000.0;
const TS_C5: f64 = 51.0e-12;
// RAT (unchanged)
const RAT_R_G1: f64 = 47.0;
const RAT_C_G1_STD: f64 = 4.7e-6;
const RAT_C_G1_FAT: f64 = 22.0e-6;
const RAT_R_G2: f64 = 560.0;
const RAT_C_G2: f64 = 2.2e-6;
const RAT_RPOT: f64 = 100000.0;
const RAT_C_COMP: f64 = 100.0e-12;
const RAT_C_COUPLE: f64 = 4.7e-6;
const RAT_R_CLIP: f64 = 1000.0;
const RAT_RPOT_FILT: f64 = 100000.0;
const RAT_C_FILT: f64 = 3.3e-9;
const RAT_V_RAIL: f64 = 3.6;
// Big Muff: input / clip / clip / output stages (2N5088-class NPN)
const MUFF_RC: f64 = 12000.0;
const MUFF_RFB: f64 = 470000.0;
const MUFF_RBG: f64 = 100000.0;
const MUFF_CFB: f64 = 470.0e-12;
const MUFF_RS_IN: f64 = 39000.0;
const MUFF_RS_CLIP: f64 = 10000.0;
const MUFF_RS_OUT: f64 = 100000.0;
const MUFF_RE_IN: f64 = 100.0;
const MUFF_RE_CLIP: f64 = 100.0;
const MUFF_RE_OUT: f64 = 2700.0;
#[allow(dead_code)] // declared in the C++, unused there too (the Sustain pot is `muffSus`)
const MUFF_RPOT_SUS: f64 = 100000.0;
const MUFF_CS_IN: f64 = 100.0e-9;
const MUFF_CS_OUT: f64 = 100.0e-9;
const MUFF_CCL: f64 = 1.0e-6;
// Fuzz Face (values as NPN-flipped from the PNP original)
const FF_RSRC: f64 = 8200.0;
const FF_RC1: f64 = 33000.0;
const FF_RFB: f64 = 100000.0;
const FF_RPOT: f64 = 1000.0;
const FF_CE: f64 = 20.0e-6;
// DS-1
const DS1_RG: f64 = 4700.0;
const DS1_CG: f64 = 47.0e-9;
const DS1_RPOT: f64 = 100000.0;
const DS1_RF_MIN: f64 = 4700.0;
const DS1_CF: f64 = 100.0e-12;
const DS1_R_CLIP: f64 = 2200.0;
const DS1_V_RAIL: f64 = 3.6;
// Klon (18 V rails from the charge pump)
const KLON_RG: f64 = 2200.0;
const KLON_CG: f64 = 1.0e-6;
const KLON_RF_MIN: f64 = 1000.0;
const KLON_RPOT: f64 = 100000.0;
const KLON_CF: f64 = 47.0e-12;
const KLON_R_CLIP: f64 = 1000.0;
const KLON_V_RAIL: f64 = 8.5;
const KLON_R_SUM: f64 = 20000.0;
const KLON_R_CLEAN_MIN: f64 = 5600.0;
const KLON_R_DIRT: f64 = 8200.0;
// Blues Breaker
const BB_RG_STD: f64 = 4700.0;
const BB_RG_HI: f64 = 2200.0;
const BB_CG: f64 = 4.7e-6;
const BB_RF_MIN: f64 = 10000.0;
const BB_RPOT: f64 = 100000.0;
const BB_CF: f64 = 100.0e-12;
// Tube Driver: op-amp gain stage into two cascaded 12AX7 triode stages (starved plate)
const TD_RG: f64 = 4700.0;
const TD_CG: f64 = 1.0e-6;
const TD_RF_MIN: f64 = 4700.0;
const TD_RPOT: f64 = 500000.0;
const TD_CF: f64 = 100.0e-12;
const TD_CC: f64 = 22.0e-9;
const TD_RGS: f64 = 68000.0;
const TD_RGL: f64 = 1000000.0;
const TD_RP: f64 = 100000.0;
const TD_CK: f64 = 22.0e-6;
// 12AX7 Koren parameters
const TU_MU: f64 = 100.0;
const TU_KP: f64 = 600.0;
const TU_KVB: f64 = 300.0;
const TU_KG1: f64 = 1060.0;
const TU_EX: f64 = 1.4;
// Blues Driver: JFET input stage, discrete gain block with 1N4148 pair in the loop, 100 k Gain pot
const BD_RD: f64 = 4700.0;
const BD_RS: f64 = 1000.0;
const BD_IDSS: f64 = 6.0e-3;
const BD_VP: f64 = -1.8;
const BD_CS: f64 = 1.0e-6;
const BD_RGATE: f64 = 1000000.0;
const BD_RG: f64 = 1000.0;
const BD_CG_STD: f64 = 2.2e-6;
const BD_CG_PHAT: f64 = 10.0e-6;
const BD_RF_MIN: f64 = 1000.0;
const BD_RPOT: f64 = 100000.0;
const BD_CF: f64 = 100.0e-12;
// MXR Distortion+: 741 non-inverting, 1 M Distortion pot, 10 k into 1N270 germanium pair + cap to ground
const DP_RG: f64 = 4700.0;
const DP_CG: f64 = 47.0e-9;
const DP_RF_MIN: f64 = 4700.0;
const DP_RPOT: f64 = 1000000.0;
const DP_CF: f64 = 47.0e-12;
const DP_R_CLIP: f64 = 10000.0;
// Fulltone OCD: TL082 non-inverting, 500 k Drive pot, 2N7000 diode-connected pair to ground
const OCD_RG_HP: f64 = 1000.0;
const OCD_RG_LP: f64 = 2200.0;
const OCD_CG: f64 = 4.7e-6;
const OCD_RF_MIN: f64 = 5000.0;
const OCD_RPOT: f64 = 500000.0;
const OCD_CF: f64 = 47.0e-12;
const OCD_R_CLIP: f64 = 1000.0;
const OCD_K: f64 = 0.18;
const OCD_VTH: f64 = 1.9;
// Guv'nor
const GOV_RG: f64 = 2200.0;
const GOV_CG: f64 = 4.7e-6;
const GOV_RF_MIN: f64 = 4700.0;
const GOV_RPOT: f64 = 1000000.0;
const GOV_CF: f64 = 47.0e-12;
const GOV_R_CLIP: f64 = 1000.0;

// ---------------------------------------------------------------- structs
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

/// TS trapezoidal feedback clipper.
#[derive(Debug, Clone, Copy, Default)]
struct ClipState {
    vc4: f64,
    vd: f64,
    vin_prev: f64,
    #[allow(dead_code)] // declared in the C++, never read there either
    g_prev: f64,
}

/// Generic op-amp feedback clipper (BB / Gov).
#[derive(Debug, Clone, Copy, Default)]
struct FbState {
    vcg: f64,
    vd: f64,
    #[allow(dead_code)] // declared in the C++, never read there either
    i_prev: f64,
    #[allow(dead_code)]
    g_prev: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct OpampState {
    v_gbw: f64,
    v_op: f64,
}

/// One CE stage with C-B feedback.
#[derive(Debug, Clone, Copy, Default)]
struct BjtStage {
    vcap: f64,
    vcd: f64,
    vb: f64,
    vc: f64,
    ve: f64,
    vfb_prev: f64,
}

/// One 12AX7 stage.
#[derive(Debug, Clone, Copy, Default)]
struct Triode {
    vcap: f64,
    vg: f64,
    vp: f64,
    vk: f64,
}

/// JFET common-source stage.
#[derive(Debug, Clone, Copy, Default)]
struct Jfet {
    vcap: f64,
    vs: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    up: [Biquad; 4],
    down: [Biquad; 4],
    in_hp_x1: f32,
    in_hp_y1: f32,
    out_hp_x1: f32,
    out_hp_y1: f32,
    tone_bq: Biquad,
    tone_bq2: Biquad,
    tone_bq3: Biquad,
    lp_y: f32,
    hp_x: f32,
    hp_y: f32,
    // TS
    clip: ClipState,
    lpf_y1: f32,
    // RAT
    r_vc1: f64,
    r_vc2: f64,
    r_vfb_lp: f64,
    r_vgbw: f64,
    r_vop: f64,
    r_cpl_x: f64,
    r_cpl_y: f64,
    r_vd: f64,
    // Muff
    muff: [BjtStage; 4],
    // FF
    f_hp_x: f64,
    f_hp_y: f64,
    f_vb1: f64,
    f_vc1: f64,
    f_ve2: f64,
    f_vw: f64,
    f_vc2: f64,
    // DS-1
    ds1q: BjtStage,
    d_hp_x: f64,
    d_hp_y: f64,
    d_vcg: f64,
    d_vfb_lp: f64,
    d_vd: f64,
    d_op: OpampState,
    // Klon
    k_vcg: f64,
    k_vfb_lp: f64,
    k_vd: f64,
    k_hp_x: f64,
    k_hp_y: f64,
    k_op: OpampState,
    // BB
    bb: FbState,
    b_op: OpampState,
    // Gov
    gov: FbState,
    g_op: OpampState,
    g_vd: f64,
    // Tube Driver
    td: FbState,
    t_op: OpampState,
    tube: [Triode; 2],
    // Blues Driver / Dist+ / OCD
    bd_j: Jfet,
    bd: FbState,
    bd_op: OpampState,
    bd_hp_x: f64,
    bd_hp_y: f64,
    dp: FbState,
    dp_op: OpampState,
    dp_vd: f64,
    ocd: FbState,
    ocd_op: OpampState,
    ocd_vd: f64,
}

/// Everything but the per-channel solver state: parameters, derived circuit values, coefficients.
#[derive(Debug, Clone, Copy)]
struct Settings {
    // ---- parameter state
    drive_knob: f32,
    tone_knob: f32,
    level_knob: f32,
    circuit_idx: i32,
    bypassed: bool,
    trim_db: f32,
    trim_lin: f32,
    trim_sm: f32,
    /// Stored as the C++ does; only the wrapper (Link) reads it.
    linked: bool,
    /// The per-circuit Level anchors, stored as the C++ does; only the wrapper (Link) reads them.
    lo: [f32; 12],
    hi: [f32; 12],

    ts_model_idx: i32,
    ts_diode_idx: i32,
    rat_model_idx: i32,
    rat_diode_idx: i32,
    rat_chip_idx: i32,
    rat_fat_on: bool,
    muff_ver_idx: i32,
    muff_diode_idx: i32,
    muff_mids_on: bool,
    ff_trans_idx: i32,
    ff_bias_idx: i32,
    ds1_diode_idx: i32,
    ds1_voice_idx: i32,
    klon_diode_idx: i32,
    bb_gain_idx: i32,
    gov_clip_idx: i32,
    gov_bass_idx: i32,
    gov_mid_idx: i32,
    td_plate_idx: i32,
    td_bias_idx: i32,
    bd_mode_idx: i32,
    dp_diode_idx: i32,
    ocd_mode_idx: i32,

    drive_sm: f32,
    tone_sm: f32,
    level_sm: f32,
    tone_counter: i32,
    in_hp_coef: f32,
    os_t: f64,
    fs_base: f64,

    // ---- derived: TS
    ts_rf_base: f64,
    ts_r5: f64,
    ts_c4: f64,
    ts_is: f64,
    ts_n_vt: f64,
    ts_k_f: f64,
    ts_k_r: f64,
    ts_clamp_v: f64,
    ts_rf: f64,
    ts_trap_a: f64,
    ts_late_chip: bool,
    ts_out_gain: f32,
    ts_lpf_coef: f32,
    // ---- derived: RAT
    rat_cg1: f64,
    rat_gbw: f64,
    rat_slew: f64,
    rat_is: f64,
    rat_n_vt: f64,
    rat_r_dist: f64,
    rat_k_comp: f64,
    rat_k_gbw: f64,
    rat_slew_step: f64,
    rat_cpl_coef: f64,
    // ---- derived: Muff
    muff_is: f64,
    muff_n_vt: f64,
    muff_sus: f64,
    muff_cs_clip: f64,
    muff_fc_lp: f64,
    muff_fc_hp: f64,
    muff_b_is: f64,
    muff_bf: f64,
    muff_br: f64,
    // ---- derived: FF
    ff_is: f64,
    ff_bf1: f64,
    ff_bf2: f64,
    ff_br: f64,
    ff_rc2: f64,
    ff_hp_in: f64,
    ff_ra: f64,
    ff_rb: f64,
    // ---- derived: DS-1
    ds1_is: f64,
    ds1_n_vt: f64,
    ds1_k_f: f64,
    ds1_k_r: f64,
    ds1_rf: f64,
    ds1_k_comp: f64,
    ds1_fc_lp: f64,
    ds1_fc_hp: f64,
    ds1_k_gbw: f64,
    ds1_slew_step: f64,
    ds1_hp_op: f64,
    // ---- derived: Klon
    klon_is: f64,
    klon_n_vt: f64,
    klon_rf: f64,
    klon_k_comp: f64,
    klon_r_clean: f64,
    klon_k_gbw: f64,
    klon_slew_step: f64,
    klon_hp_coef: f64,
    // ---- derived: BB
    bb_rg: f64,
    bb_rf: f64,
    bb_trap_a: f64,
    bb_k_gbw: f64,
    bb_slew_step: f64,
    bb_inv_c: f64,
    // ---- derived: Tube Driver
    td_rf: f64,
    td_k_gbw: f64,
    td_slew_step: f64,
    td_inv_c: f64,
    td_vpp: f64,
    td_rk: f64,
    // ---- derived: BD-2 / D+ / OCD
    bd_rf: f64,
    bd_cg: f64,
    bd_hp_coef: f64,
    bd_k_gbw: f64,
    bd_slew_step: f64,
    bd_inv_c: f64,
    dp_rf: f64,
    dp_k_gbw: f64,
    dp_slew_step: f64,
    dp_inv_c: f64,
    dp_is: f64,
    dp_n_vt: f64,
    dp_ccap: f64,
    ocd_rg: f64,
    ocd_rf: f64,
    ocd_k_gbw: f64,
    ocd_slew_step: f64,
    ocd_inv_c: f64,
    // ---- derived: Gov
    gov_rf: f64,
    #[allow(dead_code)] // declared in the C++, never read there either
    gov_trap_a: f64,
    gov_k_gbw: f64,
    gov_slew_step: f64,
    gov_inv_c: f64,
    gov_led_is: f64,
    gov_led_n_vt: f64,
    gov_si_diodes: bool,

    // shared derived
    out_scale: f64,
    out_hp_coef_d: f64,
    blend_lp_k: f32,
    blend_hp_coef: f32,
    blend_mix: f32,

    tone_coef: Biquad,
    tone_coef2: Biquad,
    tone_coef3: Biquad,
}

impl Default for Settings {
    /// The C++ default member initialisers.
    fn default() -> Self {
        Settings {
            drive_knob: 0.5,
            tone_knob: 0.5,
            level_knob: 0.5,
            circuit_idx: 0,
            bypassed: false,
            trim_db: 0.0,
            trim_lin: 1.0,
            trim_sm: 1.0,
            linked: true,
            lo: [0.0; 12],
            hi: [0.0; 12],
            ts_model_idx: 0,
            ts_diode_idx: 0,
            rat_model_idx: 0,
            rat_diode_idx: 0,
            rat_chip_idx: 0,
            rat_fat_on: false,
            muff_ver_idx: 1,
            muff_diode_idx: 0,
            muff_mids_on: false,
            ff_trans_idx: 0,
            ff_bias_idx: 1,
            ds1_diode_idx: 0,
            ds1_voice_idx: 0,
            klon_diode_idx: 0,
            bb_gain_idx: 0,
            gov_clip_idx: 0,
            gov_bass_idx: 1,
            gov_mid_idx: 1,
            td_plate_idx: 1,
            td_bias_idx: 1,
            bd_mode_idx: 0,
            dp_diode_idx: 0,
            ocd_mode_idx: 0,
            drive_sm: 0.5,
            tone_sm: 0.5,
            level_sm: 0.25,
            tone_counter: 0,
            in_hp_coef: 0.0,
            os_t: 1.0 / 192000.0,
            fs_base: 48000.0,
            ts_rf_base: 51000.0,
            ts_r5: 4700.0,
            ts_c4: 47.0e-9,
            ts_is: 2.52e-9,
            ts_n_vt: 1.752 * 25.85e-3,
            ts_k_f: 1.0,
            ts_k_r: 1.0,
            ts_clamp_v: 3.0,
            ts_rf: 176000.0,
            ts_trap_a: 0.0,
            ts_late_chip: false,
            ts_out_gain: 0.99,
            ts_lpf_coef: 0.0,
            rat_cg1: 4.7e-6,
            rat_gbw: 1.0e6,
            rat_slew: 0.3e6,
            rat_is: 2.52e-9,
            rat_n_vt: 1.752 * 25.85e-3,
            rat_r_dist: 25000.0,
            rat_k_comp: 0.1,
            rat_k_gbw: 0.1,
            rat_slew_step: 0.01,
            rat_cpl_coef: 0.999,
            muff_is: 2.52e-9,
            muff_n_vt: 1.752 * 25.85e-3,
            muff_sus: 0.5,
            muff_cs_clip: 100.0e-9,
            muff_fc_lp: 408.0,
            muff_fc_hp: 1855.0,
            muff_b_is: 1.0e-14,
            muff_bf: 400.0,
            muff_br: 4.0,
            ff_is: 1.5e-6,
            ff_bf1: 70.0,
            ff_bf2: 100.0,
            ff_br: 5.0,
            ff_rc2: 470.0,
            ff_hp_in: 0.99,
            ff_ra: 500.0,
            ff_rb: 500.0,
            ds1_is: 2.52e-9,
            ds1_n_vt: 1.752 * 25.85e-3,
            ds1_k_f: 1.0,
            ds1_k_r: 1.0,
            ds1_rf: 50000.0,
            ds1_k_comp: 0.1,
            ds1_fc_lp: 1064.0,
            ds1_fc_hp: 3300.0,
            ds1_k_gbw: 0.5,
            ds1_slew_step: 0.01,
            ds1_hp_op: 0.99,
            klon_is: 2.0e-7,
            klon_n_vt: 1.4 * 25.85e-3,
            klon_rf: 20000.0,
            klon_k_comp: 0.1,
            klon_r_clean: 50000.0,
            klon_k_gbw: 0.5,
            klon_slew_step: 0.1,
            klon_hp_coef: 0.99,
            bb_rg: 4700.0,
            bb_rf: 50000.0,
            bb_trap_a: 0.0,
            bb_k_gbw: 0.5,
            bb_slew_step: 0.1,
            bb_inv_c: 1.0 / 100.0e-12,
            td_rf: 50000.0,
            td_k_gbw: 0.5,
            td_slew_step: 0.1,
            td_inv_c: 1.0 / 100.0e-12,
            td_vpp: 12.0,
            td_rk: 1500.0,
            bd_rf: 50000.0,
            bd_cg: 2.2e-6,
            bd_hp_coef: 0.999,
            bd_k_gbw: 0.5,
            bd_slew_step: 0.1,
            bd_inv_c: 1.0 / 100.0e-12,
            dp_rf: 200000.0,
            dp_k_gbw: 0.5,
            dp_slew_step: 0.05,
            dp_inv_c: 1.0 / 47.0e-12,
            dp_is: 2.0e-7,
            dp_n_vt: 1.4 * 25.85e-3,
            dp_ccap: 1.0e-9,
            ocd_rg: 1000.0,
            ocd_rf: 100000.0,
            ocd_k_gbw: 0.5,
            ocd_slew_step: 0.1,
            ocd_inv_c: 1.0 / 47.0e-12,
            gov_rf: 50000.0,
            gov_trap_a: 0.0,
            gov_k_gbw: 0.5,
            gov_slew_step: 0.1,
            gov_inv_c: 1.0 / 47.0e-12,
            gov_led_is: 2.0e-13,
            gov_led_n_vt: 3.0 * 25.85e-3,
            gov_si_diodes: false,
            out_scale: 2.0,
            out_hp_coef_d: 0.999,
            blend_lp_k: 0.05,
            blend_hp_coef: 0.9,
            blend_mix: 0.5,
            tone_coef: Biquad::default(),
            tone_coef2: Biquad::default(),
            tone_coef3: Biquad::default(),
        }
    }
}

/// The DistEngine DSP.
#[derive(Debug, Clone)]
pub struct DistEngineDsp {
    s: Settings,
    ch: [Channel; 2],
}

impl Default for DistEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------- filter helpers
#[inline(always)]
fn filter(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

fn butter_lp(fc: f64, fs: f64, q_: f64) -> Biquad {
    let k = (PI * fc / fs).tan();
    let a0 = 1.0 + k / q_ + k * k;
    let b0 = ((k * k) / a0) as f32;
    Biquad {
        b0,
        b1: ((2.0 * k * k) / a0) as f32,
        b2: b0,
        a1: ((2.0 * (k * k - 1.0)) / a0) as f32,
        a2: ((1.0 - k / q_ + k * k) / a0) as f32,
        z1: 0.0,
        z2: 0.0,
    }
}

fn set_pass(q: &mut Biquad) {
    q.b0 = 1.0;
    q.b1 = 0.0;
    q.b2 = 0.0;
    q.a1 = 0.0;
    q.a2 = 0.0;
}

fn one_pole_lp(q: &mut Biquad, fc: f64, fs: f64) {
    let f = clamp_t(fc, 20.0, 0.45 * fs);
    let k = (PI * f / fs).tan();
    let a0 = 1.0 + k;
    q.b0 = (k / a0) as f32;
    q.b1 = (k / a0) as f32;
    q.b2 = 0.0;
    q.a1 = ((k - 1.0) / a0) as f32;
    q.a2 = 0.0;
}

fn high_shelf(q: &mut Biquad, fc: f64, fs: f64, gain_db: f64) {
    let g = 10.0f64.powf(gain_db / 20.0);
    let k = (PI * clamp_t(fc, 20.0, 0.45 * fs) / fs).tan();
    q.b0 = ((k + g) / (k + 1.0)) as f32;
    q.b1 = ((k - g) / (k + 1.0)) as f32;
    q.b2 = 0.0;
    q.a1 = ((k - 1.0) / (k + 1.0)) as f32;
    q.a2 = 0.0;
}

fn low_shelf(q: &mut Biquad, fc: f64, fs: f64, gain_db: f64) {
    let g = 10.0f64.powf(gain_db / 20.0);
    let k = (PI * clamp_t(fc, 20.0, 0.45 * fs) / fs).tan();
    q.b0 = ((g * k + 1.0) / (k + 1.0)) as f32;
    q.b1 = ((g * k - 1.0) / (k + 1.0)) as f32;
    q.b2 = 0.0;
    q.a1 = ((k - 1.0) / (k + 1.0)) as f32;
    q.a2 = 0.0;
}

fn peak_eq(q: &mut Biquad, fc: f64, fs: f64, q_: f64, gain_db: f64) {
    let a = 10.0f64.powf(gain_db / 40.0);
    let w = 2.0 * PI * clamp_t(fc, 20.0, 0.45 * fs) / fs;
    let al = w.sin() / (2.0 * q_);
    let a0 = 1.0 + al / a;
    q.b0 = ((1.0 + al * a) / a0) as f32;
    q.b1 = ((-2.0 * w.cos()) / a0) as f32;
    q.b2 = ((1.0 - al * a) / a0) as f32;
    q.a1 = ((-2.0 * w.cos()) / a0) as f32;
    q.a2 = ((1.0 - al / a) / a0) as f32;
}

fn one_pole_coef(fc: f64, fs: f64) -> f32 {
    (-2.0 * PI * fc / fs).exp() as f32
}

impl Settings {
    fn os_one_pole_k(&self, fc: f64) -> f64 {
        1.0 - (-2.0 * PI * clamp_t(fc, 1.0, 0.45 / self.os_t) * self.os_t).exp()
    }
    fn os_hp_coef(&self, fc: f64) -> f64 {
        (-2.0 * PI * clamp_t(fc, 0.5, 0.45 / self.os_t) * self.os_t).exp()
    }
}

// ---------------------------------------------------------------- device models
#[allow(dead_code)] // in the C++ too: every caller moved to `dEval`
fn d_current(vd: f64, is: f64, n_vt: f64, k_f: f64, k_r: f64) -> f64 {
    let ef = clamp_t(vd / (k_f * n_vt), -40.0, 40.0).exp();
    let er = clamp_t(-vd / (k_r * n_vt), -40.0, 40.0).exp();
    is * (ef - 1.0) - is * (er - 1.0)
}

fn d_slope(vd: f64, is: f64, n_vt: f64, k_f: f64, k_r: f64) -> f64 {
    let ef = clamp_t(vd / (k_f * n_vt), -40.0, 40.0).exp();
    let er = clamp_t(-vd / (k_r * n_vt), -40.0, 40.0).exp();
    is * ef / (k_f * n_vt) + is * er / (k_r * n_vt)
}

/// The diode's current AND its slope at the same voltage (arithmetically identical to
/// `dCurrent()` + `dSlope()`: same expressions, same order).
#[inline(always)]
fn d_eval(vd: f64, is: f64, n_vt: f64, k_f: f64, k_r: f64) -> (f64, f64) {
    let ef = clamp_t(vd / (k_f * n_vt), -40.0, 40.0).exp();
    let er = clamp_t(-vd / (k_r * n_vt), -40.0, 40.0).exp();
    let current = is * (ef - 1.0) - is * (er - 1.0);
    let slope = is * ef / (k_f * n_vt) + is * er / (k_r * n_vt);
    (current, slope)
}

/// Ebers-Moll NPN: currents and partials w.r.t. vbe and vbc: `(ic, ib, dIcE, dIcC, dIbE, dIbC)`.
#[inline(always)]
fn bjt(is: f64, bf: f64, br: f64, vbe: f64, vbc: f64) -> (f64, f64, f64, f64, f64, f64) {
    let ef = clamp_t(vbe / VT, -40.0, 30.0).exp();
    let er = clamp_t(vbc / VT, -40.0, 30.0).exp();
    let ic = is * (ef - er) - (is / br) * (er - 1.0);
    let ib = (is / bf) * (ef - 1.0) + (is / br) * (er - 1.0);
    let d_ic_e = is * ef / VT;
    let d_ic_c = -is * er / VT - (is / br) * er / VT;
    let d_ib_e = (is / bf) * ef / VT;
    let d_ib_c = (is / br) * er / VT;
    (ic, ib, d_ic_e, d_ic_c, d_ib_e, d_ib_c)
}

/// 3x3 solve by Cramer's rule: J·x = b.
#[inline(always)]
fn solve3(a11: f64, a12: f64, a13: f64, a21: f64, a22: f64, a23: f64, a31: f64, a32: f64, a33: f64, b1: f64, b2: f64, b3: f64) -> (f64, f64, f64) {
    let det = a11 * (a22 * a33 - a23 * a32) - a12 * (a21 * a33 - a23 * a31) + a13 * (a21 * a32 - a22 * a31);
    let d = if det.abs() < 1.0e-30 { 1.0e-30 } else { det };
    let x1 = (b1 * (a22 * a33 - a23 * a32) - a12 * (b2 * a33 - a23 * b3) + a13 * (b2 * a32 - a22 * b3)) / d;
    let x2 = (a11 * (b2 * a33 - a23 * b3) - b1 * (a21 * a33 - a23 * a31) + a13 * (a21 * b3 - b2 * a31)) / d;
    let x3 = (a11 * (a22 * b3 - b2 * a32) - a12 * (a21 * b3 - b2 * a31) + b1 * (a21 * a32 - a22 * a31)) / d;
    (x1, x2, x3)
}

/// Diode pair to ground through series R.
fn ground_clip(state: &mut f64, vs: f64, r: f64, is: f64, n_vt: f64, k_f: f64, k_r: f64) -> f64 {
    let mut vd = clamp_t(*state, -vs.abs(), vs.abs());
    for _ in 0..8 {
        let (idio, gdio) = d_eval(vd, is, n_vt, k_f, k_r);
        let f = (vs - vd) / r - idio;
        let fp = -1.0 / r - gdio;
        vd -= clamp_t(f / fp, -0.5, 0.5);
        vd = clamp_t(vd, -4.0, 4.0);
    }
    *state = vd;
    vd
}

/// Op-amp output limits: gain-bandwidth, slew rate, rails.
fn opamp(s: &mut OpampState, target: f64, k_gbw: f64, slew_step: f64, rail: f64) -> f64 {
    s.v_gbw += (target - s.v_gbw) * k_gbw;
    s.v_op += clamp_t(s.v_gbw - s.v_op, -slew_step, slew_step);
    clamp_t(s.v_op, -rail, rail)
}

/// Diode pair in series with a DC-blocking cap: solves id = Id(vfb - vcdPrev - id·dt/C)
/// implicitly (1-D Newton), cold-started at 0 with a fixed iteration count as in the C++.
fn series_diode_cap(vfb: f64, vcd_prev: f64, dt_c: f64, is: f64, n_vt: f64) -> (f64, f64) {
    let mut i = 0.0f64;
    for _ in 0..4 {
        let vd = vfb - vcd_prev - i * dt_c;
        let (idio, gdio) = d_eval(vd, is, n_vt, 1.0, 1.0);
        let g = i - idio;
        let gp = 1.0 + gdio * dt_c;
        i -= g / gp;
    }
    let vd_f = vfb - vcd_prev - i * dt_c;
    let sl = d_slope(vd_f, is, n_vt, 1.0, 1.0);
    (i, sl / (1.0 + sl * dt_c))
}

/// One common-emitter stage (see the C++ for the topology); unknowns Vb, Vc, Ve by 3-D Newton.
/// Returns the collector voltage.
fn bjt_stage(
    st: &mut BjtStage,
    xin: f64,
    dt: f64,
    rs: f64,
    cs: f64,
    rbg: f64,
    rfb: f64,
    cfb: f64,
    rc: f64,
    re: f64,
    diodes: bool,
    ccl: f64,
    is: f64,
    n_vt: f64,
    b_is: f64,
    bf: f64,
    br: f64,
) -> f64 {
    let (mut vb, mut vc, mut ve) = (st.vb, st.vc, st.ve);
    let zs = rs + dt / cs;
    let g_c = cfb / dt;
    for _ in 0..7 {
        let (ic, ib, d_ic_e, d_ic_c, d_ib_e, d_ib_c) = bjt(b_is, bf, br, vb - ve, vb - vc);
        let iin = (xin - vb - st.vcap) / zs;
        let vfb = vc - vb;
        let (mut id, mut gd) = (0.0f64, 0.0f64);
        if diodes {
            (id, gd) = series_diode_cap(vfb, st.vcd, dt / ccl, is, n_vt);
        }
        let ifb = vfb / rfb + id + g_c * (vfb - st.vfb_prev);
        let gfb = 1.0 / rfb + gd + g_c;

        let f1 = iin - vb / rbg + ifb - ib;
        let f2 = (VCC - vc) / rc - ifb - ic;
        let f3 = ic + ib - ve / re;

        let j11 = -1.0 / zs - 1.0 / rbg - gfb - (d_ib_e + d_ib_c);
        let j12 = gfb + d_ib_c;
        let j13 = d_ib_e;
        let j21 = gfb - (d_ic_e + d_ic_c);
        let j22 = -1.0 / rc - gfb + d_ic_c;
        let j23 = d_ic_e;
        let j31 = d_ic_e + d_ic_c + d_ib_e + d_ib_c;
        let j32 = -d_ic_c - d_ib_c;
        let j33 = -d_ic_e - d_ib_e - 1.0 / re;

        let (dx1, dx2, dx3) = solve3(j11, j12, j13, j21, j22, j23, j31, j32, j33, -f1, -f2, -f3);
        vb += clamp_t(dx1, -0.3, 0.3);
        vc += clamp_t(dx2, -1.0, 1.0);
        ve += clamp_t(dx3, -0.3, 0.3);
        vb = clamp_t(vb, -2.0, VCC + 1.0);
        vc = clamp_t(vc, -1.0, VCC + 1.0);
        ve = clamp_t(ve, -1.0, VCC);
    }
    let iin_f = (xin - vb - st.vcap) / zs;
    st.vcap += iin_f * dt / cs;
    if diodes {
        let (id_f, _gd_f) = series_diode_cap(vc - vb, st.vcd, dt / ccl, is, n_vt);
        st.vcd += id_f * dt / ccl;
    }
    st.vb = vb;
    st.vc = vc;
    st.ve = ve;
    st.vfb_prev = vc - vb;
    vc
}

/// Koren 12AX7: plate current with analytic partials `(ip, gm = dIp/dVgk, gp = dIp/dVpk)`.
fn triode_eval(vgk: f64, vpk: f64) -> (f64, f64, f64) {
    if vpk <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let s = (TU_KVB + vpk * vpk).sqrt();
    let a = TU_KP * (1.0 / TU_MU + vgk / s);
    let ac = clamp_t(a, -60.0, 60.0);
    let sp = if ac > 30.0 { ac } else { (1.0 + ac.exp()).ln() }; // softplus
    let sig = 1.0 / (1.0 + (-ac).exp()); // its derivative
    let e1 = (vpk / TU_KP) * sp;
    if e1 <= 1.0e-12 {
        return (0.0, 0.0, 0.0);
    }
    let e1p = e1.powf(TU_EX - 1.0);
    let ip = e1p * e1 / TU_KG1;
    let d_ip_d_e1 = TU_EX * e1p / TU_KG1;
    let d_e1_d_vgk = vpk * sig / s;
    let d_e1_d_vpk = sp / TU_KP - vpk * vpk * vgk * sig / (s * s * s);
    (ip, d_ip_d_e1 * d_e1_d_vgk, d_ip_d_e1 * d_e1_d_vpk)
}

/// Grid conduction, diode-like once the grid goes positive w.r.t. the cathode.
fn grid_ig(vgk: f64) -> f64 {
    1.0e-9 * (clamp_t(vgk / 0.06, -40.0, 40.0).exp() - 1.0)
}
fn grid_slope(vgk: f64) -> f64 {
    1.0e-9 * clamp_t(vgk / 0.06, -40.0, 40.0).exp() / 0.06
}

/// One triode gain stage; unknowns Vg, Vp, Vk by 3-D Newton.
fn triode_stage(st: &mut Triode, xin: f64, dt: f64, vpp: f64, rk: f64) -> f64 {
    let (mut vg, mut vp, mut vk) = (st.vg, st.vp, st.vk);
    let zs = TD_RGS + dt / TD_CC;
    let g_ck = TD_CK / dt;
    let vk_prev = st.vk;
    for _ in 0..5 {
        let (vgk, vpk) = (vg - vk, vp - vk);
        let (ip, gm, gp) = triode_eval(vgk, vpk);
        let (ig, gg) = (grid_ig(vgk), grid_slope(vgk));
        let iin = (xin - vg - st.vcap) / zs;

        let f1 = iin - vg / TD_RGL - ig;
        let f2 = (vpp - vp) / TD_RP - ip;
        let f3 = ip + ig - vk / rk - g_ck * (vk - vk_prev);

        let j11 = -1.0 / zs - 1.0 / TD_RGL - gg;
        let j12 = 0.0;
        let j13 = gg;
        let j21 = -gm;
        let j22 = -1.0 / TD_RP - gp;
        let j23 = gm + gp;
        let j31 = gm + gg;
        let j32 = gp;
        let j33 = -gm - gp - gg - 1.0 / rk - g_ck;

        let (dx1, dx2, dx3) = solve3(j11, j12, j13, j21, j22, j23, j31, j32, j33, -f1, -f2, -f3);
        vg += clamp_t(dx1, -1.0, 1.0);
        vp += clamp_t(dx2, -20.0, 20.0);
        vk += clamp_t(dx3, -0.5, 0.5);
        vg = clamp_t(vg, -20.0, 20.0);
        vp = clamp_t(vp, 0.0, vpp);
        vk = clamp_t(vk, 0.0, vpp);
    }
    if vp.is_nan() || vg.is_nan() || vk.is_nan() {
        vg = 0.0;
        vp = vpp * 0.6;
        vk = vpp * 0.01;
        st.vcap = 0.0;
    }
    let iin_f = (xin - vg - st.vcap) / zs;
    st.vcap += iin_f * dt / TD_CC;
    st.vg = vg;
    st.vp = vp;
    st.vk = vk;
    vp
}

/// JFET common-source stage (Shockley square law); the only unknown is Vs (1-D Newton).
/// Returns the drain voltage.
fn jfet_stage(st: &mut Jfet, xin: f64, dt: f64) -> f64 {
    // gate node: cap + 1 M leak -> plain HP with the cap's corner
    let zs = dt / BD_CS;
    let ig = (xin - st.vcap) / (BD_RGATE + zs);
    st.vcap += ig * zs;
    let vg = ig * BD_RGATE;
    let mut vs = st.vs;
    for _ in 0..8 {
        let vgs = clamp_t(vg - vs, BD_VP, 0.6);
        let x = 1.0 - vgs / BD_VP;
        let id = BD_IDSS * x * x;
        let gm = -2.0 * BD_IDSS * x / BD_VP;
        let f = id - vs / BD_RS;
        let fp = -gm - 1.0 / BD_RS;
        vs -= clamp_t(f / fp, -0.3, 0.3);
        vs = clamp_t(vs, 0.0, VCC);
    }
    st.vs = vs;
    let vgs_f = clamp_t(vg - vs, BD_VP, 0.6);
    let x_f = 1.0 - vgs_f / BD_VP;
    let id_f = BD_IDSS * x_f * x_f;
    clamp_t(VCC - id_f * BD_RD, 0.0, VCC)
}

/// Diode-connected 2N7000 (square law above Vth, smooth knee) in parallel with the body diode
/// of the opposite device: `(current, slope)`, the slope analytic.
fn mos_eval(v: f64) -> (f64, f64) {
    let u = v - OCD_VTH;
    let t = clamp_t(u / 0.08, -40.0, 75.0);
    let et = t.exp();
    let sp = if u > 6.0 { u } else { 0.08 * (1.0 + et).ln() };
    // d(softplus)/dv is the logistic sigmoid; written as et/(1+et) it reuses the exp above
    let dsp = if u > 6.0 { 1.0 } else { et / (1.0 + et) };

    let eb = clamp_t(-v / (1.5 * VT), -40.0, 40.0).exp();
    let bod = 1.0e-12 * (eb - 1.0);

    (OCD_K * sp * sp - bod, 2.0 * OCD_K * sp * dsp + 1.0e-12 * eb / (1.5 * VT))
}

/// Anti-parallel MOSFET pair to ground through series R.
fn mos_clip(state: &mut f64, vs: f64, r: f64) -> f64 {
    let mut vd = clamp_t(*state, -vs.abs(), vs.abs());
    for _ in 0..8 {
        let (i_p, g_p) = mos_eval(vd);
        let (i_n, g_n) = mos_eval(-vd);

        let f = (vs - vd) / r - (i_p - i_n);
        let fp = -1.0 / r - (g_p + g_n);
        vd -= clamp_t(f / fp, -0.5, 0.5);
        vd = clamp_t(vd, -6.0, 6.0);
    }
    *state = vd;
    vd
}

/// Non-inverting op-amp feedback clipper: ground leg Rg + Cg (backward Euler), feedback
/// Rf || Cf || diode pairs, backward Euler on the feedback cap, Newton on the diode voltage.
/// `k_series` = diodes in series each way. (`trapA` is passed and unused, as in the C++.)
fn fb_clip(
    os_t: f64,
    s: &mut FbState,
    vin: f64,
    rg: f64,
    cg: f64,
    rf: f64,
    inv_cf: f64,
    _trap_a: f64,
    has_diodes: bool,
    is: f64,
    n_vt: f64,
    k_series: f64,
) -> f64 {
    // ground-leg current through Rg + Cg
    let zg = rg + os_t / cg;
    let ig = (vin - s.vcg) / zg;
    s.vcg += ig * os_t / cg;
    // backward Euler on the feedback cap (L-stable, so a transient can't leave the solver ringing)
    let inv_rf = 1.0 / rf;
    let mut vd = s.vd;
    for _ in 0..10 {
        let (mut idio, mut gdio) = (0.0f64, 0.0f64);
        if has_diodes {
            (idio, gdio) = d_eval(vd, is, n_vt, k_series, k_series);
        }
        let g = (ig - vd * inv_rf - idio) * inv_cf;
        let f = vd - s.vd - os_t * g;
        let fp = 1.0 + os_t * (inv_rf + gdio) * inv_cf;
        vd -= clamp_t(f / fp, -0.5, 0.5);
        vd = clamp_t(vd, -20.0, 20.0);
    }
    s.vd = vd;
    vin + vd
}

impl DistEngineDsp {
    pub fn new() -> Self {
        DistEngineDsp { s: Settings::default(), ch: [Channel::default(); 2] }
    }

    pub fn set_parameter(&mut self, index: usize, v: f32) {
        let s = &mut self.s;
        match index {
            P_DRIVE => s.drive_knob = v,
            P_TONE => s.tone_knob = v,
            P_LEVEL => s.level_knob = v,
            P_CIRCUIT => {
                s.circuit_idx = (v + 0.5) as i32;
                self.apply_model();
                self.reset_states();
            }
            P_BYPASS => s.bypassed = v > 0.5,
            P_TRIM => {
                s.trim_db = -12.0 + 36.0 * clamp_t(v, 0.0f32, 1.0);
                s.trim_lin = 10.0f32.powf(s.trim_db / 20.0);
            }
            P_LINK => s.linked = v > 0.5,
            i if (P_TS_LO..=P_OCD_HI).contains(&i) => {
                // tsLo, tsHi, ratLo, ratHi, ... in pairs: lo[k] / hi[k]
                let k = (i - P_TS_LO) / 2;
                let arr = if (i - P_TS_LO).is_multiple_of(2) { &mut s.lo } else { &mut s.hi };
                if let Some(slot) = arr.get_mut(k) {
                    *slot = v;
                }
            }
            P_TS_MODEL..=P_OCD_MODE => {
                let idx = (v + 0.5) as i32;
                let on = v > 0.5;
                match index {
                    P_TS_MODEL => s.ts_model_idx = idx,
                    P_TS_DIODE => s.ts_diode_idx = idx,
                    P_RAT_MODEL => s.rat_model_idx = idx,
                    P_RAT_DIODE => s.rat_diode_idx = idx,
                    P_RAT_CHIP => s.rat_chip_idx = idx,
                    P_RAT_FAT => s.rat_fat_on = on,
                    P_MUFF_VER => s.muff_ver_idx = idx,
                    P_MUFF_DIODE => s.muff_diode_idx = idx,
                    P_MUFF_MIDS => s.muff_mids_on = on,
                    P_FF_TRANS => s.ff_trans_idx = idx,
                    P_FF_BIAS => s.ff_bias_idx = idx,
                    P_DS1_DIODE => s.ds1_diode_idx = idx,
                    P_DS1_VOICE => s.ds1_voice_idx = idx,
                    P_KLON_DIODE => s.klon_diode_idx = idx,
                    P_BB_GAIN => s.bb_gain_idx = idx,
                    P_GOV_CLIP => s.gov_clip_idx = idx,
                    P_GOV_BASS => s.gov_bass_idx = idx,
                    P_GOV_MID => s.gov_mid_idx = idx,
                    P_TD_PLATE => s.td_plate_idx = idx,
                    P_TD_BIAS => s.td_bias_idx = idx,
                    P_BD_MODE => s.bd_mode_idx = idx,
                    P_DP_DIODE => s.dp_diode_idx = idx,
                    _ => s.ocd_mode_idx = idx,
                }
                self.apply_model();
                if index == P_TD_PLATE {
                    self.reset_states();
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- init (cmajor init())
    pub fn prepare(&mut self, sample_rate: f64) {
        let s = &mut self.s;
        s.fs_base = sample_rate;
        let os_fs = s.fs_base * 4.0;
        s.os_t = 1.0 / os_fs;

        let fc = 0.4 * s.fs_base;
        for c in &mut self.ch {
            c.up[0] = butter_lp(fc, os_fs, 0.5098);
            c.up[1] = butter_lp(fc, os_fs, 0.6013);
            c.up[2] = butter_lp(fc, os_fs, 0.8999);
            c.up[3] = butter_lp(fc, os_fs, 2.5629);
            c.down[0] = butter_lp(fc, os_fs, 0.5098);
            c.down[1] = butter_lp(fc, os_fs, 0.6013);
            c.down[2] = butter_lp(fc, os_fs, 0.8999);
            c.down[3] = butter_lp(fc, os_fs, 2.5629);
        }

        s.in_hp_coef = one_pole_coef(16.0, s.fs_base);
        s.ts_lpf_coef = one_pole_coef(8000.0, s.fs_base);

        for i in 0..12 {
            s.lo[i] = 0.6;
            s.hi[i] = 0.4;
        }

        s.drive_sm = s.drive_knob;
        s.tone_sm = s.tone_knob;
        s.level_sm = s.level_knob * s.level_knob;
        s.trim_lin = 10.0f32.powf(s.trim_db / 20.0);
        s.trim_sm = s.trim_lin;

        self.reset_states();
        self.apply_model();
        self.update_blend();
    }

    /// Rust addition (the C++ has no reset): SoundCraft resets a plugin on seek to silence its
    /// tails. The engine becomes a fresh one prepared at the same rate (its own defaults, as a
    /// new C++ instance), in place: nothing is allocated. The caller pushes the parameters again.
    pub fn reset(&mut self) {
        let fs = self.s.fs_base;
        *self = DistEngineDsp::new();
        self.prepare(fs);
    }

    /// `prepare (sampleRate)` on the live engine, for the host's transport reset (Play / Stop /
    /// Locate / bounce start), keeping its own knobs. Rust additions so the result is the same
    /// whatever played before and nothing glides: every per-channel state is cleared first (the
    /// C++ `prepare` rebuilds the oversampling filters and `resetStates` the solvers, but leaves
    /// the input high-pass, tone and other filter memories) and the control counter starts from
    /// 0. The Drive- and Tone-dependent values need nothing extra: `prepare` seats the smoothers
    /// at the knobs before `applyModel`, which derives them there. The caller pushes every value
    /// again at the next block, as `prepareToPlay`'s `forcePush` does (which also restores the
    /// Level anchors `prepare` writes over). Plain data, no allocation.
    pub fn reset_live(&mut self, sample_rate: f64) {
        self.ch = [Channel::default(); 2];
        self.prepare(sample_rate);
        self.s.tone_counter = 0;
    }

    /// The smoothed Drive, Tone, Level and trim and the values they head for (tests).
    #[cfg(test)]
    pub(crate) fn smoothers(&self) -> ([f32; 4], [f32; 4]) {
        let s = &self.s;
        ([s.drive_sm, s.tone_sm, s.level_sm, s.trim_sm], [s.drive_knob, s.tone_knob, s.level_knob * s.level_knob, s.trim_lin])
    }

    // ---------------------------------------------------------------- main (cmajor main() loop body, per sample)
    /// Channels `0..min(num_channels, 2)` are processed in place; only channels 0 and 1 have
    /// solver state, so anything above stereo is left alone.
    pub fn process(&mut self, channels: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        let n2 = num_channels.min(2).min(channels.len());
        let num_samples = channels.iter().take(n2).map(Vec::len).fold(num_samples, usize::min);

        if self.s.bypassed {
            return; // in-place: bypass is a no-op on the buffer
        }

        for n in 0..num_samples {
            {
                let s = &mut self.s;
                s.trim_sm += (s.trim_lin - s.trim_sm) * 0.005;
                s.drive_sm += (s.drive_knob - s.drive_sm) * 0.005;
                s.tone_sm += (s.tone_knob - s.tone_sm) * 0.005;
                let lt = s.level_knob * s.level_knob;
                s.level_sm += (lt - s.level_sm) * 0.005;

                s.tone_counter += 1;
            }
            if self.s.tone_counter >= 32 {
                self.s.tone_counter = 0;
                self.update_drive(self.s.drive_sm);
                self.update_tone(self.s.tone_sm);
                self.update_blend();
            }

            for c in 0..n2 {
                let Some(x) = channels.get(c).and_then(|b| b.get(n)).copied() else { continue };
                let (y, nan) = match self.ch.get_mut(c) {
                    Some(chn) => process_channel(&self.s, chn, x),
                    None => continue,
                };
                if nan {
                    // NaN guard: a solver blow-up resets instead of silencing the plugin
                    self.reset_states();
                }
                if let Some(slot) = channels.get_mut(c).and_then(|b| b.get_mut(n)) {
                    *slot = y;
                }
            }
        }
    }

    /// The rate `prepare` settled on.
    pub fn sample_rate(&self) -> f64 {
        self.s.fs_base
    }
    pub fn is_bypassed(&self) -> bool {
        self.s.bypassed
    }
    /// The smoothed Drive knob (0..1).
    pub fn drive_smoothed(&self) -> f32 {
        self.s.drive_sm
    }
    /// The smoothed Level gain (`level²`).
    pub fn level_smoothed(&self) -> f32 {
        self.s.level_sm
    }
    pub fn circuit(&self) -> i32 {
        self.s.circuit_idx
    }

    // ---------------------------------------------------------------- model table
    fn apply_model(&mut self) {
        let s = &mut self.s;
        let fs = s.fs_base;

        // ================= Tube Screamer =================
        s.ts_rf_base = 51000.0;
        s.ts_r5 = 4700.0;
        s.ts_c4 = 47.0e-9;
        s.ts_clamp_v = 3.0;
        s.ts_late_chip = false;
        s.ts_out_gain = 0.99;
        let mut ts_out_hp = 1.6f64;
        s.ts_is = 2.52e-9;
        s.ts_n_vt = 1.752 * VT;
        s.ts_k_f = 1.0;
        s.ts_k_r = 1.0;
        if s.ts_model_idx == 1 {
            s.ts_out_gain = 0.995;
            ts_out_hp = 0.16;
        } else if s.ts_model_idx == 2 || s.ts_model_idx == 3 {
            s.ts_out_gain = 0.995;
            ts_out_hp = 0.16;
            s.ts_clamp_v = 2.8;
            s.ts_late_chip = true;
        } else if s.ts_model_idx == 4 {
            s.ts_r5 = 2200.0;
            s.ts_c4 = 100.0e-9;
            s.ts_out_gain = 0.995;
            ts_out_hp = 0.16;
            s.ts_k_f = 2.0;
            s.ts_k_r = 2.0;
        } else if s.ts_model_idx == 5 {
            s.ts_c4 = 100.0e-9;
            s.ts_k_f = 2.0;
            s.ts_k_r = 1.0;
        }
        if s.ts_diode_idx == 3 {
            s.ts_is = 3.0e-9;
            s.ts_n_vt = 1.70 * VT;
            s.ts_k_f = 1.0;
            s.ts_k_r = 1.0;
        } else if s.ts_diode_idx == 4 {
            s.ts_k_f = 2.0;
            s.ts_k_r = 1.0;
        } else if s.ts_diode_idx == 5 {
            s.ts_is = 1.0e-18;
            s.ts_n_vt = 2.0 * VT;
            s.ts_k_f = 1.0;
            s.ts_k_r = 1.0;
        } else if s.ts_diode_idx == 6 {
            s.ts_is = 1.0e-6;
            s.ts_n_vt = 1.3 * VT;
            s.ts_k_f = 1.0;
            s.ts_k_r = 1.0;
        } else if s.ts_diode_idx >= 1 {
            s.ts_is = 2.52e-9;
            s.ts_n_vt = 1.752 * VT;
            s.ts_k_f = 1.0;
            s.ts_k_r = 1.0;
        }
        s.ts_trap_a = s.os_t / (2.0 * s.ts_r5 * s.ts_c4);

        // ================= RAT =================
        let (mut s_chip, mut s_diode) = (1, 1);
        if s.rat_model_idx == 1 {
            s_chip = 2;
            s_diode = 1;
        } else if s.rat_model_idx == 2 {
            s_chip = 1;
            s_diode = 2;
        } else if s.rat_model_idx == 3 {
            s_chip = 1;
            s_diode = 3;
        } else if s.rat_model_idx == 4 {
            s_chip = 2;
            s_diode = 1;
        }
        let rc = if s.rat_chip_idx == 0 { s_chip } else { s.rat_chip_idx };
        let rd = if s.rat_diode_idx == 0 { s_diode } else { s.rat_diode_idx };
        s.rat_gbw = 1.0e6;
        s.rat_slew = 0.3e6;
        if rc == 2 {
            s.rat_gbw = 0.6e6;
            s.rat_slew = 0.3e6;
        } else if rc == 3 {
            s.rat_gbw = 1.0e6;
            s.rat_slew = 0.5e6;
        } else if rc == 4 {
            s.rat_gbw = 3.0e6;
            s.rat_slew = 13.0e6;
        }
        s.rat_is = 2.52e-9;
        s.rat_n_vt = 1.752 * VT;
        if rd == 2 {
            s.rat_is = 2.0e-13;
            s.rat_n_vt = 3.0 * VT;
        } else if rd == 3 {
            s.rat_is = 2.0e-7;
            s.rat_n_vt = 1.4 * VT;
        } else if rd == 4 {
            s.rat_is = 1.0e-18;
            s.rat_n_vt = 2.0 * VT;
        }
        s.rat_cg1 = if s.rat_fat_on { RAT_C_G1_FAT } else { RAT_C_G1_STD };

        // ================= Big Muff =================
        s.muff_cs_clip = 100.0e-9; // 100 n into 10 k
        s.muff_fc_lp = 408.0;
        s.muff_fc_hp = 1855.0;
        if s.muff_ver_idx == 0 {
            s.muff_fc_lp = 408.0;
            s.muff_fc_hp = 1855.0;
        } else if s.muff_ver_idx == 1 {
            s.muff_fc_lp = 482.0;
            s.muff_fc_hp = 1855.0;
        } else if s.muff_ver_idx == 2 {
            s.muff_fc_lp = 723.0;
            s.muff_fc_hp = 2040.0;
            s.muff_cs_clip = 1.0e-6; // Sovtek: 1 µ coupling
        } else {
            s.muff_fc_lp = 408.0;
            s.muff_fc_hp = 1720.0;
        }
        if s.muff_mids_on {
            s.muff_fc_lp *= 1.8;
            s.muff_fc_hp *= 0.55;
        }
        s.muff_is = 2.52e-9;
        s.muff_n_vt = 1.752 * VT;
        if s.muff_diode_idx == 2 {
            s.muff_is = 1.0e-18;
            s.muff_n_vt = 2.0 * VT;
        } else if s.muff_diode_idx == 3 {
            s.muff_is = 1.0e-6;
            s.muff_n_vt = 1.3 * VT;
        }
        s.muff_b_is = 1.0e-14;
        s.muff_bf = 400.0;
        s.muff_br = 4.0;

        // ================= Fuzz Face =================
        if s.ff_trans_idx == 0 {
            // NKT275-class germanium
            s.ff_is = 1.5e-6;
            s.ff_bf1 = 70.0;
            s.ff_bf2 = 100.0;
            s.ff_br = 5.0;
        } else {
            // BC108-class silicon
            s.ff_is = 5.0e-14;
            s.ff_bf1 = 250.0;
            s.ff_bf2 = 300.0;
            s.ff_br = 10.0;
        }
        s.ff_rc2 = 470.0;
        if s.ff_bias_idx == 0 {
            s.ff_rc2 = 820.0; // cold: collector sits low, gated
        } else if s.ff_bias_idx == 2 {
            s.ff_rc2 = 330.0; // hot: smoother
        }
        s.ff_hp_in = s.os_hp_coef(1.0 / (2.0 * PI * (FF_RSRC + 4000.0) * 2.2e-6));

        // ================= DS-1 =================
        s.ds1_is = 2.52e-9;
        s.ds1_n_vt = 1.752 * VT;
        s.ds1_k_f = 1.0;
        s.ds1_k_r = 1.0;
        if s.ds1_diode_idx == 1 {
            s.ds1_is = 1.0e-18;
            s.ds1_n_vt = 2.0 * VT;
        } else if s.ds1_diode_idx == 2 {
            s.ds1_k_f = 2.0;
            s.ds1_k_r = 1.0;
        }
        s.ds1_fc_lp = 1064.0;
        s.ds1_fc_hp = 3300.0;
        if s.ds1_voice_idx == 1 {
            s.ds1_fc_lp = 700.0;
            s.ds1_fc_hp = 2200.0;
        }
        s.ds1_hp_op = s.os_hp_coef(1.0 / (2.0 * PI * 100000.0 * 47.0e-9));
        s.ds1_k_gbw = s.os_one_pole_k(1.0e6 / 25.0);
        s.ds1_slew_step = 0.5e6 * s.os_t; // M5223AL-class

        // ================= Klon =================
        s.klon_is = 2.0e-7;
        s.klon_n_vt = 1.4 * VT;
        if s.klon_diode_idx == 1 {
            s.klon_is = 2.52e-9;
            s.klon_n_vt = 1.752 * VT;
        } else if s.klon_diode_idx == 2 {
            s.klon_is = 1.0e-18;
            s.klon_n_vt = 2.0 * VT;
        }
        s.klon_hp_coef = s.os_hp_coef(20.0);
        s.klon_slew_step = 13.0e6 * s.os_t; // TL072

        // ================= Blues Breaker =================
        s.bb_rg = if s.bb_gain_idx == 0 { BB_RG_STD } else { BB_RG_HI };
        s.bb_inv_c = 1.0 / BB_CF;
        s.bb_slew_step = 13.0e6 * s.os_t;

        // ================= Guv'nor =================
        s.gov_si_diodes = s.gov_clip_idx == 1;
        s.gov_inv_c = 1.0 / GOV_CF;
        s.gov_slew_step = 13.0e6 * s.os_t;
        s.gov_led_is = 2.0e-13;
        s.gov_led_n_vt = 3.0 * VT;

        // ================= Tube Driver =================
        s.td_vpp = if s.td_plate_idx == 0 {
            9.0
        } else if s.td_plate_idx == 1 {
            12.0
        } else {
            250.0
        };
        s.td_rk = if s.td_bias_idx == 0 {
            4700.0
        } else if s.td_bias_idx == 2 {
            680.0
        } else {
            1500.0
        };
        s.td_inv_c = 1.0 / TD_CF;
        s.td_slew_step = 13.0e6 * s.os_t;

        // ================= Blues Driver =================
        s.bd_cg = if s.bd_mode_idx == 0 { BD_CG_STD } else { BD_CG_PHAT }; // Keeley "Phat" mod: bigger gain-leg cap
        s.bd_inv_c = 1.0 / BD_CF;
        s.bd_slew_step = 4.0e6 * s.os_t; // discrete gain block: slower than a TL072
        s.bd_hp_coef = s.os_hp_coef(16.0);

        // ================= Distortion+ =================
        s.dp_is = 2.0e-7; // 1N270 germanium
        s.dp_n_vt = 1.4 * VT;
        if s.dp_diode_idx == 1 {
            s.dp_is = 2.52e-9;
            s.dp_n_vt = 1.752 * VT;
        } else if s.dp_diode_idx == 2 {
            s.dp_is = 1.0e-18;
            s.dp_n_vt = 2.0 * VT;
        }
        s.dp_inv_c = 1.0 / DP_CF;
        s.dp_slew_step = 0.5e6 * s.os_t; // LM741

        // ================= OCD =================
        s.ocd_rg = if s.ocd_mode_idx == 0 { OCD_RG_HP } else { OCD_RG_LP };
        s.ocd_inv_c = 1.0 / OCD_CF;
        s.ocd_slew_step = 13.0e6 * s.os_t; // TL082

        // ================= output scale per circuit =================
        s.out_scale = 2.0;
        let mut out_hp_fc = ts_out_hp;
        match s.circuit_idx {
            1 => {
                s.out_scale = 1.0 / INPUT_VOLTS;
                out_hp_fc = 1.6;
            }
            2 => {
                s.out_scale = 4.0;
                out_hp_fc = 1.6;
            }
            3 => {
                s.out_scale = 1.0;
                out_hp_fc = 1.6;
            }
            4 => {
                s.out_scale = 1.0 / INPUT_VOLTS;
                out_hp_fc = 1.6;
            }
            5 | 6 | 9 => {
                s.out_scale = 2.0;
                out_hp_fc = 1.6;
            }
            7 | 11 => {
                s.out_scale = 1.5;
                out_hp_fc = 1.6;
            }
            8 => {
                s.out_scale = if s.td_plate_idx == 2 { 0.03 } else { 0.6 };
                out_hp_fc = 1.6;
            }
            10 => {
                s.out_scale = 6.0;
                out_hp_fc = 1.6;
            }
            _ => {}
        }
        s.out_hp_coef_d = (-2.0 * PI * out_hp_fc / fs).exp();

        self.update_drive(self.s.drive_sm);
        self.update_tone(self.s.tone_sm);
    }

    fn reset_states(&mut self) {
        let td_vpp = self.s.td_vpp;
        for c in &mut self.ch {
            c.r_vc1 = 0.0;
            c.r_vc2 = 0.0;
            c.r_vfb_lp = 0.0;
            c.r_vgbw = 0.0;
            c.r_vop = 0.0;
            c.r_vd = 0.0;
            for m in &mut c.muff {
                m.vb = 0.6;
                m.vc = 4.5;
                m.ve = 0.05;
                m.vfb_prev = 3.9;
                m.vcap = 0.0;
                m.vcd = 3.9;
            }
            c.ds1q.vb = 0.6;
            c.ds1q.vc = 4.5;
            c.ds1q.ve = 0.1;
            c.ds1q.vfb_prev = 3.9;
            c.ds1q.vcap = 0.0;
            c.ds1q.vcd = 0.0;
            c.f_vb1 = 0.15;
            c.f_vc1 = 0.6;
            c.f_ve2 = 0.5;
            c.f_vw = 0.5;
            c.f_vc2 = 4.5;
            c.d_vd = 0.0;
            c.k_vd = 0.0;
            c.g_vd = 0.0;
            c.bb.vd = 0.0;
            c.gov.vd = 0.0;
            for t in &mut c.tube {
                t.vcap = 0.0;
                t.vg = 0.0;
                t.vp = td_vpp * 0.6;
                t.vk = td_vpp * 0.01;
            }
            c.td.vd = 0.0;
            c.bd_j.vs = 1.0;
            c.bd_j.vcap = 0.0;
            c.bd.vd = 0.0;
            c.bd_hp_x = VCC - 1.0e-3 * BD_RD;
            c.bd_hp_y = 0.0;
            c.dp.vd = 0.0;
            c.dp_vd = 0.0;
            c.ocd.vd = 0.0;
            c.ocd_vd = 0.0;
        }
    }

    // ---------------------------------------------------------------- drive-dependent (every 32 samples)
    fn update_drive(&mut self, dsm: f32) {
        let s = &mut self.s;
        let d = f64::from(clamp_t(dsm, 0.0f32, 1.0));
        s.ts_rf = s.ts_rf_base + d * d * TS_RPOT_DRIVE;

        s.rat_r_dist = max_t(1.0, RAT_RPOT * d * d);
        let rg_par = (RAT_R_G1 * RAT_R_G2) / (RAT_R_G1 + RAT_R_G2);
        s.rat_k_comp = s.os_one_pole_k(1.0 / (2.0 * PI * s.rat_r_dist * RAT_C_COMP));
        s.rat_k_gbw = s.os_one_pole_k(max_t(s.rat_gbw / (1.0 + s.rat_r_dist / rg_par), 200.0));
        s.rat_slew_step = s.rat_slew * s.os_t;

        s.muff_sus = 0.003 + 0.997 * d * d; // 100 k Sustain pot after the input stage

        s.ff_ra = (1.0 - d) * FF_RPOT + 1.0; // fuzz pot: wiper to emitter
        s.ff_rb = d * FF_RPOT + 1.0; // wiper to ground

        s.ds1_rf = DS1_RF_MIN + DS1_RPOT * d * d;
        s.ds1_k_comp = s.os_one_pole_k(1.0 / (2.0 * PI * s.ds1_rf * DS1_CF));

        s.klon_rf = KLON_RF_MIN + KLON_RPOT * d * d; // gang 1: drive gain
        s.klon_k_comp = s.os_one_pole_k(1.0 / (2.0 * PI * s.klon_rf * KLON_CF));
        s.klon_r_clean = KLON_R_CLEAN_MIN + KLON_RPOT * d; // gang 2: clean path attenuates as gain rises
        s.klon_k_gbw = s.os_one_pole_k(max_t(3.0e6 / (1.0 + s.klon_rf / KLON_RG), 1000.0));

        s.bb_rf = BB_RF_MIN + BB_RPOT * d * d;
        s.bb_trap_a = s.os_t / (2.0 * s.bb_rg * BB_CG);
        s.bb_k_gbw = s.os_one_pole_k(max_t(3.0e6 / (1.0 + s.bb_rf / s.bb_rg), 1000.0));

        s.gov_rf = GOV_RF_MIN + GOV_RPOT * d * d;

        s.td_rf = TD_RF_MIN + TD_RPOT * d * d;

        s.bd_rf = BD_RF_MIN + BD_RPOT * d * d;
        s.bd_k_gbw = s.os_one_pole_k(max_t(2.0e6 / (1.0 + s.bd_rf / BD_RG), 500.0));
        s.dp_rf = DP_RF_MIN + DP_RPOT * d * d;
        s.dp_k_gbw = s.os_one_pole_k(max_t(1.0e6 / (1.0 + s.dp_rf / DP_RG), 300.0));
        s.ocd_rf = OCD_RF_MIN + OCD_RPOT * d * d;
        s.ocd_k_gbw = s.os_one_pole_k(max_t(3.0e6 / (1.0 + s.ocd_rf / s.ocd_rg), 500.0));
        s.td_k_gbw = s.os_one_pole_k(max_t(3.0e6 / (1.0 + s.td_rf / TD_RG), 500.0));
        s.gov_k_gbw = s.os_one_pole_k(max_t(3.0e6 / (1.0 + s.gov_rf / GOV_RG), 500.0));
    }

    // ---------------------------------------------------------------- tone (base rate)
    fn update_tone(&mut self, t: f32) {
        let s = &mut self.s;
        let fs = s.fs_base;
        let tt = f64::from(clamp_t(t, 0.0f32, 1.0));
        set_pass(&mut s.tone_coef);
        set_pass(&mut s.tone_coef2);
        set_pass(&mut s.tone_coef3);

        match s.circuit_idx {
            0 => {
                // Tube Screamer active tone (exact from the TS build)
                let rp1 = max_t(tt * TS_RPOT_TONE, 10.0);
                let rp2 = max_t((1.0 - tt) * TS_RPOT_TONE, 10.0);
                let (g1, g2) = (1.0 / rp1, 1.0 / rp2);
                let gp = g1 + g2;
                let g9 = 1.0 / TS_R9;
                let a = TS_R10 * TS_C8;
                let b = 1.0 + TS_R11 * g2;
                let n0 = g9 * gp;
                let n1 = g9 * (gp * a + TS_C8 * b);
                let d0 = g9 * gp;
                let d1 = g9 * (gp * a + TS_C8) + TS_C7 * gp + g1 * TS_C8;
                let d2 = TS_C7 * (gp * a + TS_C8);
                let k = 2.0 * fs;
                let a0 = d0 + d1 * k + d2 * k * k;
                s.tone_coef.b0 = ((n0 + n1 * k) / a0) as f32;
                s.tone_coef.b1 = ((2.0 * n0) / a0) as f32;
                s.tone_coef.b2 = ((n0 - n1 * k) / a0) as f32;
                s.tone_coef.a1 = ((2.0 * d0 - 2.0 * d2 * k * k) / a0) as f32;
                s.tone_coef.a2 = ((d0 - d1 * k + d2 * k * k) / a0) as f32;
            }
            1 => {
                let r_f = RAT_R_CLIP + RAT_RPOT_FILT * tt;
                one_pole_lp(&mut s.tone_coef, 1.0 / (2.0 * PI * r_f * RAT_C_FILT), fs);
                s.rat_cpl_coef = s.os_hp_coef(1.0 / (2.0 * PI * r_f * RAT_C_COUPLE));
            }
            3 => {
                one_pole_lp(&mut s.tone_coef, 1500.0 * 8.0f64.powf(tt), fs); // output cap mod, FF has no tone
            }
            5 => {
                high_shelf(&mut s.tone_coef, 1000.0, fs, -10.0 + 20.0 * tt); // Klon treble
            }
            6 => {
                one_pole_lp(&mut s.tone_coef, 1200.0 * 15.0f64.powf(tt), fs); // BB passive tone
            }
            9 => {
                // BD-2 active tone: a tilt around 1 kHz
                high_shelf(&mut s.tone_coef, 1000.0, fs, -9.0 + 18.0 * tt);
                low_shelf(&mut s.tone_coef2, 1000.0, fs, 9.0 - 18.0 * tt);
            }
            10 => {
                // Distortion+ has no tone: the knob is the cap across the diodes (1 n .. 22 n) with the 10 k source
                s.dp_ccap = 1.0e-9 * 22.0f64.powf(tt);
                one_pole_lp(&mut s.tone_coef, 1.0 / (2.0 * PI * DP_R_CLIP * s.dp_ccap), fs);
            }
            11 => {
                // OCD tone: 10 k pot + 10 n, LP mode adds a fixed extra roll-off
                one_pole_lp(&mut s.tone_coef, 1600.0 * 12.0f64.powf(tt), fs);
                if s.ocd_mode_idx == 1 {
                    one_pole_lp(&mut s.tone_coef2, 6000.0, fs);
                }
            }
            8 => {
                high_shelf(&mut s.tone_coef, 2000.0, fs, -10.0 + 20.0 * tt); // Tube Driver "Hi"
                low_shelf(&mut s.tone_coef2, 200.0, fs, 2.0); // "Lo" fixed a touch up
            }
            7 => {
                // Guv'nor 3-band: Bass / Middle stepped, Treble on the Tone knob
                let bass = if s.gov_bass_idx == 0 {
                    -7.0
                } else if s.gov_bass_idx == 2 {
                    7.0
                } else {
                    0.0
                };
                low_shelf(&mut s.tone_coef2, 150.0, fs, bass);
                let mid = if s.gov_mid_idx == 0 {
                    -8.0
                } else if s.gov_mid_idx == 2 {
                    8.0
                } else {
                    0.0
                };
                peak_eq(&mut s.tone_coef3, 650.0, fs, 0.7, mid);
                high_shelf(&mut s.tone_coef, 2500.0, fs, -9.0 + 18.0 * tt);
            }
            _ => {}
        }

        // Muff / DS-1 use the scoop blend (updateBlend)
        let (c1, c2, c3) = (s.tone_coef, s.tone_coef2, s.tone_coef3);
        for c in &mut self.ch {
            copy_coefs(&mut c.tone_bq, &c1);
            copy_coefs(&mut c.tone_bq2, &c2);
            copy_coefs(&mut c.tone_bq3, &c3);
        }
    }

    fn update_blend(&mut self) {
        let s = &mut self.s;
        let fs = s.fs_base;
        let f_lp = if s.circuit_idx == 2 { s.muff_fc_lp } else { s.ds1_fc_lp };
        let f_hp = if s.circuit_idx == 2 { s.muff_fc_hp } else { s.ds1_fc_hp };
        s.blend_lp_k = (1.0 - (-2.0 * PI * f_lp / fs).exp()) as f32;
        s.blend_hp_coef = (-2.0 * PI * f_hp / fs).exp() as f32;
        s.blend_mix = s.tone_sm;
    }
}

/// The coefficient half of a biquad (the state is the channel's own).
fn copy_coefs(dst: &mut Biquad, src: &Biquad) {
    dst.b0 = src.b0;
    dst.b1 = src.b1;
    dst.b2 = src.b2;
    dst.a1 = src.a1;
    dst.a2 = src.a2;
}

// ---------------------------------------------------------------- circuit cores (oversampled rate, volts in / volts out)

fn ts_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let st = c.clip;
    let vc4 = (st.vc4 * (1.0 - s.ts_trap_a) + s.ts_trap_a * (vin + st.vin_prev)) / (1.0 + s.ts_trap_a);
    let ig = (vin - vc4) / s.ts_r5;
    // backward Euler on the 51 pF (L-stable): the trapezoidal version could get knocked into a
    // Nyquist-rate ringing state by a hard transient and stay there
    let inv_rf = 1.0 / s.ts_rf;
    let inv_c5 = 1.0 / TS_C5;
    let mut vd = st.vd;
    for _ in 0..8 {
        let (idio, gdio) = d_eval(vd, s.ts_is, s.ts_n_vt, s.ts_k_f, s.ts_k_r);
        let g = (ig - vd * inv_rf - idio) * inv_c5;
        let f = vd - st.vd - s.os_t * g;
        let fp = 1.0 + s.os_t * (inv_rf + gdio) * inv_c5;
        vd -= clamp_t(f / fp, -0.5, 0.5);
        vd = clamp_t(vd, -5.0, 5.0);
    }
    c.clip.vc4 = vc4;
    c.clip.vin_prev = vin;
    c.clip.vd = vd;
    clamp_t(vin + vd, -s.ts_clamp_v, s.ts_clamp_v)
}

fn rat_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let os_t = s.os_t;
    let i1 = (vin - c.r_vc1) / (RAT_R_G1 + os_t / s.rat_cg1);
    c.r_vc1 += i1 * os_t / s.rat_cg1;
    let i2 = (vin - c.r_vc2) / (RAT_R_G2 + os_t / RAT_C_G2);
    c.r_vc2 += i2 * os_t / RAT_C_G2;
    let vfb = (i1 + i2) * s.rat_r_dist;
    c.r_vfb_lp += (vfb - c.r_vfb_lp) * s.rat_k_comp;
    let v_target = vin + c.r_vfb_lp;
    c.r_vgbw += (v_target - c.r_vgbw) * s.rat_k_gbw;
    c.r_vop += clamp_t(c.r_vgbw - c.r_vop, -s.rat_slew_step, s.rat_slew_step);
    let v_out = clamp_t(c.r_vop, -RAT_V_RAIL, RAT_V_RAIL);
    let vs = v_out - c.r_cpl_x + s.rat_cpl_coef * c.r_cpl_y;
    c.r_cpl_x = v_out;
    c.r_cpl_y = vs;
    ground_clip(&mut c.r_vd, vs, RAT_R_CLIP, s.rat_is, s.rat_n_vt, 1.0, 1.0)
}

/// 2: Big Muff -- four CE transistor stages, the middle two with anti-parallel diodes + 470 pF
/// across the 470 k feedback.
fn muff_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let (os_t, is, nvt, bis, bf, br) = (s.os_t, s.muff_is, s.muff_n_vt, s.muff_b_is, s.muff_bf, s.muff_br);
    let v1 = bjt_stage(
        &mut c.muff[0],
        vin,
        os_t,
        MUFF_RS_IN,
        MUFF_CS_IN,
        MUFF_RBG,
        MUFF_RFB,
        MUFF_CFB,
        MUFF_RC,
        MUFF_RE_IN,
        false,
        MUFF_CCL,
        is,
        nvt,
        bis,
        bf,
        br,
    );
    let v2 = bjt_stage(
        &mut c.muff[1],
        v1 * s.muff_sus,
        os_t,
        MUFF_RS_CLIP,
        s.muff_cs_clip,
        MUFF_RBG,
        MUFF_RFB,
        MUFF_CFB,
        MUFF_RC,
        MUFF_RE_CLIP,
        true,
        MUFF_CCL,
        is,
        nvt,
        bis,
        bf,
        br,
    );
    // tone stack + recovery stage at base rate
    bjt_stage(
        &mut c.muff[2],
        v2,
        os_t,
        MUFF_RS_CLIP,
        s.muff_cs_clip,
        MUFF_RBG,
        MUFF_RFB,
        MUFF_CFB,
        MUFF_RC,
        MUFF_RE_CLIP,
        true,
        MUFF_CCL,
        is,
        nvt,
        bis,
        bf,
        br,
    )
}

/// 3: Fuzz Face -- Q1/Q2 pair with 100 k feedback and the fuzz pot's partially bypassed
/// emitter; 3-D Newton on Vb1, Vc1, Ve2.
fn ff_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let vs = vin - c.f_hp_x + s.ff_hp_in * c.f_hp_y;
    c.f_hp_x = vin;
    c.f_hp_y = vs;

    let (mut vb1, mut vc1, mut ve2, mut vc2) = (c.f_vb1, c.f_vc1, c.f_ve2, c.f_vc2);
    let g_ce = FF_CE / s.os_t;
    let g_w = 1.0 / s.ff_ra + 1.0 / s.ff_rb + g_ce; // wiper node conductance
    let d_vw_d_ve = (1.0 / s.ff_ra) / g_w;

    for _ in 0..12 {
        let (ic1, ib1, d_ic_e1, d_ic_c1, d_ib_e1, d_ib_c1) = bjt(s.ff_is, s.ff_bf1, s.ff_br, vb1, vb1 - vc1);
        let (ic2, ib2, d_ic_e2, d_ic_c2, d_ib_e2, d_ib_c2) = bjt(s.ff_is, s.ff_bf2, s.ff_br, vc1 - ve2, vc1 - vc2);
        let vw = (ve2 / s.ff_ra + g_ce * c.f_vw) / g_w;

        let f1 = (vs - vb1) / FF_RSRC + (ve2 - vb1) / FF_RFB - ib1;
        let f2 = (VCC - vc1) / FF_RC1 - ic1 - ib2;
        let f3 = ic2 + ib2 - (ve2 - vw) / s.ff_ra - (ve2 - vb1) / FF_RFB;

        let j11 = -1.0 / FF_RSRC - 1.0 / FF_RFB - (d_ib_e1 + d_ib_c1);
        let j12 = d_ib_c1;
        let j13 = 1.0 / FF_RFB;
        let j21 = -(d_ic_e1 + d_ic_c1);
        let j22 = -1.0 / FF_RC1 + d_ic_c1 - (d_ib_e2 + d_ib_c2);
        let j23 = d_ib_e2;
        let j31 = 1.0 / FF_RFB;
        let j32 = d_ic_e2 + d_ic_c2 + d_ib_e2 + d_ib_c2;
        let j33 = -d_ic_e2 - d_ib_e2 - (1.0 - d_vw_d_ve) / s.ff_ra - 1.0 / FF_RFB;

        let (dx1, dx2, dx3) = solve3(j11, j12, j13, j21, j22, j23, j31, j32, j33, -f1, -f2, -f3);
        vb1 += clamp_t(dx1, -0.2, 0.2);
        vc1 += clamp_t(dx2, -0.5, 0.5);
        ve2 += clamp_t(dx3, -0.5, 0.5);
        vb1 = clamp_t(vb1, -1.0, 2.0);
        vc1 = clamp_t(vc1, -0.5, VCC);
        ve2 = clamp_t(ve2, -0.5, VCC);
        vc2 = clamp_t(VCC - s.ff_rc2 * ic2, -0.5, VCC);
    }
    let vw = (ve2 / s.ff_ra + g_ce * c.f_vw) / g_w;
    c.f_vw = vw;
    c.f_vb1 = vb1;
    c.f_vc1 = vc1;
    c.f_ve2 = ve2;
    c.f_vc2 = vc2;
    vc2
}

/// 4: DS-1 -- CE transistor gain stage, non-inverting op-amp with the 4.7 k / 47 n leg and 100 k
/// Dist pot, rails, 2.2 k + diodes to ground.
fn ds1_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let os_t = s.os_t;
    let vq = bjt_stage(
        &mut c.ds1q,
        vin,
        os_t,
        10000.0,
        47.0e-9,
        100000.0,
        470000.0,
        100.0e-12,
        10000.0,
        390.0,
        false,
        1.0e-6,
        s.ds1_is,
        s.ds1_n_vt,
        1.0e-14,
        400.0,
        4.0,
    );
    let xin = vq - c.d_hp_x + s.ds1_hp_op * c.d_hp_y; // 47 n coupling into the op-amp
    c.d_hp_x = vq;
    c.d_hp_y = xin;
    let ig = (xin - c.d_vcg) / (DS1_RG + os_t / DS1_CG); // gain leg 4.7 k + 47 n
    c.d_vcg += ig * os_t / DS1_CG;
    c.d_vfb_lp += (ig * s.ds1_rf - c.d_vfb_lp) * s.ds1_k_comp; // Rf || 100 p
    let v_op = opamp(&mut c.d_op, xin + c.d_vfb_lp, s.ds1_k_gbw, s.ds1_slew_step, DS1_V_RAIL);
    ground_clip(&mut c.d_vd, v_op, DS1_R_CLIP, s.ds1_is, s.ds1_n_vt, s.ds1_k_f, s.ds1_k_r)
}

/// 5: Klon -- clean path + drive path (gain stage into 1 k + Ge diodes, ±18 V rails) summed
/// through the ganged pot.
fn klon_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let os_t = s.os_t;
    let xin = vin - c.k_hp_x + s.klon_hp_coef * c.k_hp_y;
    c.k_hp_x = vin;
    c.k_hp_y = xin;
    let ig = (xin - c.k_vcg) / (KLON_RG + os_t / KLON_CG);
    c.k_vcg += ig * os_t / KLON_CG;
    c.k_vfb_lp += (ig * s.klon_rf - c.k_vfb_lp) * s.klon_k_comp;
    let v_op = opamp(&mut c.k_op, xin + c.k_vfb_lp, s.klon_k_gbw, s.klon_slew_step, KLON_V_RAIL);
    let dirt = ground_clip(&mut c.k_vd, v_op, KLON_R_CLIP, s.klon_is, s.klon_n_vt, 1.0, 1.0);
    // inverting summing amp: -(clean·Rsum/Rclean + dirt·Rsum/Rdirt), sign restored downstream
    -(vin * (KLON_R_SUM / s.klon_r_clean) + dirt * (KLON_R_SUM / KLON_R_DIRT)) * -1.0
}

/// 6: Blues Breaker -- non-inverting op-amp, two series diodes each way across the 100 k gain
/// pot + 100 pF.
fn bb_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let v = fb_clip(s.os_t, &mut c.bb, vin, s.bb_rg, BB_CG, s.bb_rf, s.bb_inv_c, s.bb_trap_a, true, 2.52e-9, 1.752 * VT, 2.0);
    opamp(&mut c.b_op, v, s.bb_k_gbw, s.bb_slew_step, 3.6) * 2.0 // second stage fixed gain
}

/// 7: Guv'nor -- TL072 gain stage with the 1 M pot (optionally 1N914s in the loop), then 1 k +
/// LEDs to ground.
fn gov_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let v = fb_clip(s.os_t, &mut c.gov, vin, GOV_RG, GOV_CG, s.gov_rf, s.gov_inv_c, 0.0, s.gov_si_diodes, 2.52e-9, 1.752 * VT, 1.0);
    let v_op = opamp(&mut c.g_op, v, s.gov_k_gbw, s.gov_slew_step, 3.8);
    ground_clip(&mut c.g_vd, v_op, GOV_R_CLIP, s.gov_led_is, s.gov_led_n_vt, 1.0, 1.0)
}

/// 8: Tube Driver -- TL072 gain stage (500 k Drive pot) into two 12AX7 stages on a starved
/// 9/12 V plate supply (or 250 V).
fn td_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let v = fb_clip(s.os_t, &mut c.td, vin, TD_RG, TD_CG, s.td_rf, s.td_inv_c, 0.0, false, 2.52e-9, 1.752 * VT, 1.0);
    let v_op = opamp(&mut c.t_op, v, s.td_k_gbw, s.td_slew_step, 3.8);
    let [t0, t1] = &mut c.tube;
    let v1 = triode_stage(t0, v_op * 0.35, s.os_t, s.td_vpp, s.td_rk); // 1 M / 470 k divider into the first grid
    triode_stage(t1, v1, s.os_t, s.td_vpp, s.td_rk)
}

/// 9: Blues Driver -- JFET input stage into a discrete non-inverting gain block with 1N4148s in
/// the loop.
fn bd_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let vj = jfet_stage(&mut c.bd_j, vin, s.os_t);
    let xj = vj - c.bd_hp_x + s.bd_hp_coef * c.bd_hp_y; // 1 µ coupling out of the JFET stage
    c.bd_hp_x = vj;
    c.bd_hp_y = xj;
    let v = fb_clip(s.os_t, &mut c.bd, xj, BD_RG, s.bd_cg, s.bd_rf, s.bd_inv_c, 0.0, true, 2.52e-9, 1.752 * VT, 1.0);
    opamp(&mut c.bd_op, v, s.bd_k_gbw, s.bd_slew_step, 3.3)
}

/// 10: Distortion+ -- 741 non-inverting with the 1 M pot, then 10 k into germanium diodes and a
/// cap to ground.
fn dp_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let v = fb_clip(s.os_t, &mut c.dp, vin, DP_RG, DP_CG, s.dp_rf, s.dp_inv_c, 0.0, false, 2.52e-9, 1.752 * VT, 1.0);
    let v_op = opamp(&mut c.dp_op, v, s.dp_k_gbw, s.dp_slew_step, 3.6);
    ground_clip(&mut c.dp_vd, v_op, DP_R_CLIP, s.dp_is, s.dp_n_vt, 1.0, 1.0)
}

/// 11: OCD -- TL082 gain stage into diode-connected 2N7000s (with their body diodes) to ground.
fn ocd_core(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    let v = fb_clip(s.os_t, &mut c.ocd, vin, s.ocd_rg, OCD_CG, s.ocd_rf, s.ocd_inv_c, 0.0, false, 2.52e-9, 1.752 * VT, 1.0);
    let v_op = opamp(&mut c.ocd_op, v, s.ocd_k_gbw, s.ocd_slew_step, 4.2);
    mos_clip(&mut c.ocd_vd, v_op, OCD_R_CLIP)
}

fn core_sample(s: &Settings, c: &mut Channel, vin: f64) -> f64 {
    match s.circuit_idx {
        0 => ts_core(s, c, vin),
        1 => rat_core(s, c, vin),
        2 => muff_core(s, c, vin),
        3 => ff_core(s, c, vin),
        4 => ds1_core(s, c, vin),
        5 => klon_core(s, c, vin),
        6 => bb_core(s, c, vin),
        7 => gov_core(s, c, vin),
        8 => td_core(s, c, vin),
        9 => bd_core(s, c, vin),
        10 => dp_core(s, c, vin),
        _ => ocd_core(s, c, vin),
    }
}

// ---------------------------------------------------------------- per-channel processing
/// One base-rate sample of one channel: `(output, the NaN guard fired)`. On `true` the caller
/// resets the solver states of both channels, as the C++ guard does in place.
fn process_channel(s: &Settings, c: &mut Channel, x: f32) -> (f32, bool) {
    let hp_in = x - c.in_hp_x1 + s.in_hp_coef * c.in_hp_y1;
    c.in_hp_x1 = x;
    c.in_hp_y1 = hp_in;

    let vin = hp_in * s.trim_sm * (INPUT_VOLTS as f32);

    let mut y = 0.0f32;
    for k in 0..4 {
        let mut u = 0.0f32;
        if k == 0 {
            u = vin * 4.0;
        }
        for q in &mut c.up {
            u = filter(q, u);
        }
        let mut d = core_sample(s, c, f64::from(u)) as f32;
        for q in &mut c.down {
            d = filter(q, d);
        }
        y = d;
    }

    if s.circuit_idx == 2 || s.circuit_idx == 4 {
        // Muff / DS-1 passive tone: LP branch and HP branch blended by the pot
        c.lp_y += (y - c.lp_y) * s.blend_lp_k;
        let hp = y - c.hp_x + s.blend_hp_coef * c.hp_y;
        c.hp_x = y;
        c.hp_y = hp;
        y = c.lp_y * (1.0 - s.blend_mix) + hp * s.blend_mix;
        if s.circuit_idx == 2 {
            // Muff recovery stage: one more CE stage after the tone pot (base rate is fine, it's linear at these levels)
            y = bjt_stage(
                &mut c.muff[3],
                f64::from(y),
                1.0 / s.fs_base,
                MUFF_RS_OUT,
                MUFF_CS_OUT,
                MUFF_RBG,
                MUFF_RFB,
                0.0,
                MUFF_RC,
                MUFF_RE_OUT,
                false,
                MUFF_CCL,
                s.muff_is,
                s.muff_n_vt,
                s.muff_b_is,
                s.muff_bf,
                s.muff_br,
            ) as f32;
        }
    } else {
        if s.circuit_idx == 0 && s.ts_late_chip {
            y = y + (c.lpf_y1 - y) * s.ts_lpf_coef;
            c.lpf_y1 = y;
        }
        y = filter(&mut c.tone_bq, y);
        if s.circuit_idx == 7 {
            y = filter(&mut c.tone_bq2, y);
            y = filter(&mut c.tone_bq3, y);
        }
        if s.circuit_idx == 8 || s.circuit_idx == 9 || s.circuit_idx == 11 {
            y = filter(&mut c.tone_bq2, y);
        }
    }

    let nan = y.is_nan();
    if nan {
        y = 0.0;
    }

    if s.circuit_idx == 1 {
        y = (y * 0.9).tanh();
    }

    y *= s.level_sm;

    let o = y - c.out_hp_x1 + (s.out_hp_coef_d as f32) * c.out_hp_y1;
    c.out_hp_x1 = y;
    c.out_hp_y1 = o;

    let g = if s.circuit_idx == 0 { s.ts_out_gain } else { 1.0 };
    (o * g * (s.out_scale as f32), nan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_t_passes_nan_like_the_cpp_template() {
        assert!(clamp_t(f64::NAN, -1.0, 1.0).is_nan());
        assert_eq!(clamp_t(2.0, -1.0, 1.0), 1.0);
        // NaN bounds (groundClip's ±|vs| with vs NaN) do not panic.
        assert_eq!(clamp_t(0.5, f64::NAN, f64::NAN), 0.5);
        assert!(max_t(f64::NAN, 1.0).is_nan());
        assert_eq!(max_t(1.0, f64::NAN), 1.0);
        assert_eq!(max_t(1.0, 2.0), 2.0);
    }

    #[test]
    fn lo_hi_pairs_land_in_their_slots() {
        let mut d = DistEngineDsp::new();
        d.set_parameter(P_TS_LO, 0.11);
        d.set_parameter(P_TS_LO + 1, 0.22);
        d.set_parameter(P_OCD_HI - 1, 0.33);
        d.set_parameter(P_OCD_HI, 0.44);
        assert_eq!((d.s.lo[0], d.s.hi[0], d.s.lo[11], d.s.hi[11]), (0.11, 0.22, 0.33, 0.44));
    }
}
