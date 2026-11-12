//! VintEngineDSP -- Engine 11: vintage hardware for flavour.
//! Line-by-line translation of `Engine 11 - Vint/Source/VintEngineDSP.h`, as the plugin builds it
//! (`VINT_MODELLED_PASSIVE = 0`).
//!
//! ```text
//!   0 Passive        the EQP-1A: its passive network solved as a circuit
//!   7 Passive Mid    the MEQ-5, its own unit: its passive network, solved as a circuit
//!   1 Console        the Neve 1073 EQ, as circuits, inside the 1073's preamp (its fast model)
//!   2 Proportional   API 550A(-1): three bands in 2 / 4 / 6 / 9 / 12 dB steps, as a circuit
//!   3 British        SSL 4000 E channel EQ, the brown (82E02) or black (82E242) card, as circuits
//!   4 Enhancer       exciter / maximizer / vitalizer family; the Maximizer is the BBE 882,
//!                    as a circuit (`vint_maximizer`: FreeDAWm's own, not in the C++)
//!   5 Air            the Dolby A encode trick: a four-band, level-dependent brightener
//!   6 Sampler        12-bit sampler converters and their filters
//! ```
//!
//! Signal flow, per channel, at the oversampled rate:
//!
//! ```text
//!   in -> upsample --+-------------------------------------------------------(dry)--+
//!                    +-> circuit (EQ sections, or the flavour process) ->            |
//!                        [output stage x drive, auto-gained] ----------------(wet)-> mix
//!                                                                   -> downsample -> output
//! ```
//!
//! C++ types kept: `values` and `smoothed` are `float` arrays (the gliding values are worked in
//! `double` and stored back as `float`, as the C++ does), the unit maps are `float`, and the
//! per-sample output arithmetic after the oversampler is `float`.
//!
//! Rust differences (no computed value changes):
//! - Function statics (`meqMakeup`, the 1073's pot tables, the half-band coefficients) are
//!   computed in the constructor, as the C++ constructor does for the pot tables; the audio thread
//!   never measures or allocates.
//! - The MNA solve matrices are an engine-owned scratch, not thread-locals (see `vint_mna`).
//! - The Passive's makeup amp (`pultecAmp`, `DkAmp`) is not translated: with the output stage
//!   `stNone` for the Passive in this build it is never run (see `vint_pultec_amp`).
//! - `processOversampled` is not called for channels the host does not have; the C++ would read a
//!   missing channel pointer.

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

use std::sync::OnceLock;

use super::tables::vint_gain_table as vintgain;
use super::vint_api as vintapi;
use super::vint_british as vintbri;
use super::vint_filters::{self as vintf, Coefs, Section};
use super::vint_halfband::{self as vinthb};
use super::vint_maximizer as vintmax;
use super::vint_mna::{self as vintmna, Kind, Netlist, Scratch, Solver};
use super::vint_neve as vintneve;
use super::vint_neve_pre::{self as vintpre, FastPre};
use super::vint_pultec_amp::{InputTransformer, Interstage};
use super::vint_stages::{self as vints, clamp_t};

// ---------------------------------------------------------------- circuits
pub const C_PASSIVE: i32 = 0;
pub const C_CONSOLE: i32 = 1;
pub const C_PROPORTIONAL: i32 = 2;
pub const C_BRITISH: i32 = 3;
pub const C_ENHANCER: i32 = 4;
pub const C_AIR: i32 = 5;
pub const C_SAMPLER: i32 = 6;
pub const C_PASSIVE_MID: i32 = 7;
pub const NUM_CIRCUITS: i32 = 8;

// ---------------------------------------------------------------- parameters (order == Parameters.h)
pub const P_CIRCUIT: usize = 0;
pub const P_STAGE: usize = 1;
pub const P_DRIVE: usize = 2;
pub const P_MIX: usize = 3;
pub const P_OUTPUT: usize = 4;
pub const P_QUALITY: usize = 5;
pub const P_BYPASS: usize = 6;
// Passive: EQP-1A, then MEQ-5
pub const P_PAS_LOW_FREQ: usize = 7;
pub const P_PAS_LOW_BOOST: usize = 8;
pub const P_PAS_LOW_ATTEN: usize = 9;
pub const P_PAS_HIGH_FREQ: usize = 10;
pub const P_PAS_HIGH_BOOST: usize = 11;
pub const P_PAS_BANDWIDTH: usize = 12;
pub const P_PAS_HIGH_ATTEN_FREQ: usize = 13;
pub const P_PAS_HIGH_ATTEN: usize = 14;
pub const P_PAS_MID_LOW_FREQ: usize = 15;
pub const P_PAS_MID_LOW_PEAK: usize = 16;
pub const P_PAS_MID_DIP_FREQ: usize = 17;
pub const P_PAS_MID_DIP: usize = 18;
pub const P_PAS_MID_HIGH_FREQ: usize = 19;
pub const P_PAS_MID_HIGH_PEAK: usize = 20;
// Console
pub const P_CON_HPF: usize = 21;
pub const P_CON_LOW_FREQ: usize = 22;
pub const P_CON_LOW_GAIN: usize = 23;
pub const P_CON_MID_FREQ: usize = 24;
pub const P_CON_MID_GAIN: usize = 25;
pub const P_CON_HIGH_GAIN: usize = 26;
// Proportional
pub const P_PRO_LOW_FREQ: usize = 27;
pub const P_PRO_LOW_GAIN: usize = 28;
pub const P_PRO_LOW_SHELF: usize = 29;
pub const P_PRO_MID_FREQ: usize = 30;
pub const P_PRO_MID_GAIN: usize = 31;
pub const P_PRO_HIGH_FREQ: usize = 32;
pub const P_PRO_HIGH_GAIN: usize = 33;
pub const P_PRO_HIGH_SHELF: usize = 34;
pub const P_PRO_FILTER: usize = 35;
// British
pub const P_BRI_HPF: usize = 36;
pub const P_BRI_LPF: usize = 37;
pub const P_BRI_LOW_GAIN: usize = 38;
pub const P_BRI_LOW_FREQ: usize = 39;
pub const P_BRI_LOW_BELL: usize = 40;
pub const P_BRI_LM_GAIN: usize = 41;
pub const P_BRI_LM_FREQ: usize = 42;
pub const P_BRI_LM_Q: usize = 43;
pub const P_BRI_HM_GAIN: usize = 44;
pub const P_BRI_HM_FREQ: usize = 45;
pub const P_BRI_HM_Q: usize = 46;
pub const P_BRI_HIGH_GAIN: usize = 47;
pub const P_BRI_HIGH_FREQ: usize = 48;
pub const P_BRI_HIGH_BELL: usize = 49;
pub const P_BRI_BLACK: usize = 50;
// Enhancer
pub const P_ENH_TYPE: usize = 51;
pub const P_ENH_AMOUNT: usize = 52;
pub const P_ENH_TUNE: usize = 53;
pub const P_ENH_LOW: usize = 54;
// Air
pub const P_AIR_AMOUNT: usize = 55;
pub const P_AIR_BANDS: usize = 56;
pub const P_AIR_REF: usize = 57;
// Sampler
pub const P_SMP_MACHINE: usize = 58;
pub const P_SMP_RATE: usize = 59;
pub const P_SMP_BITS: usize = 60;
pub const P_SMP_DRIVE: usize = 61;
pub const P_SMP_FILTER: usize = 62;
// Console, added later: the 1073's sensitivity switch (the dial's clockwise order)
pub const P_CON_PRE: usize = 63;
pub const NUM_PARAMS: usize = 64;

pub type Values = [f32; NUM_PARAMS];

// ---------------------------------------------------------------- unit maps (float, as the C++)
pub fn knob_to_drive_db(v: f32) -> f32 {
    -6.0 + 24.0 * v // output stage: -6 .. +18 dB, 0.25 = 0 dB
}
pub fn knob_to_out_db(v: f32) -> f32 {
    -12.0 + 24.0 * v // -12 .. +12 dB
}
pub fn knob_to_pultec(v: f32) -> f32 {
    10.0 * v // the passive EQ's 0 .. 10 dials
}
pub fn knob_to_console_db(v: f32) -> f32 {
    -16.0 + 32.0 * v // console shelves: -16 .. +16 dB
}
pub fn knob_to_console_mid_db(v: f32) -> f32 {
    -18.0 + 36.0 * v // console mid: -18 .. +18 dB
}
pub fn knob_to_bri_shelf_db(v: f32) -> f32 {
    -16.5 + 33.0 * v // British shelves: -16.5 .. +16.5 dB
}
pub fn knob_to_bri_bell_db(v: f32) -> f32 {
    -15.0 + 30.0 * v // British mids: -15 .. +15 dB
}
pub fn knob_to_bri_q(v: f32) -> f32 {
    0.5 + 2.0 * v // British mid Q: 0.5 .. 2.5
}
pub fn knob_to_air_ref_db(v: f32) -> f32 {
    -24.0 + 16.0 * v // Dolby level: -24 .. -8 dBFS
}
pub fn knob_to_smp_bits(v: f32) -> f32 {
    8.0 + 8.0 * v // sampler: 8 .. 16 bits, 0.5 = 12
}
pub fn knob_to_smp_drive_db(v: f32) -> f32 {
    -12.0 + 24.0 * v // sampler input: -12 .. +12 dB
}
pub fn knob_to_smp_rate_hz(v: f32) -> f32 {
    5000.0 * (48000.0f32 / 5000.0f32).powf(v) // variable rate: 5 .. 48 kHz
}

/// Log sweeps: v in 0..1 across lo..hi.
pub fn sweep(v: f32, lo: f32, hi: f32) -> f32 {
    lo * (hi / lo).powf(v)
}

// The British frequency knobs turn as the E's dials read.
pub const BRI_LOW_DIAL: [f32; 6] = [30.0, 50.0, 100.0, 200.0, 300.0, 450.0];
pub const BRI_LM_DIAL: [f32; 7] = [200.0, 300.0, 800.0, 1000.0, 1500.0, 2000.0, 2500.0];
pub const BRI_HM_DIAL: [f32; 7] = [600.0, 800.0, 1500.0, 3000.0, 4500.0, 6000.0, 7000.0];
pub const BRI_HIGH_DIAL: [f32; 7] = [1500.0, 2000.0, 5000.0, 8000.0, 10000.0, 14000.0, 16000.0];
pub const BRI_HPF_DIAL: [f32; 7] = [16.0, 20.0, 70.0, 120.0, 200.0, 300.0, 350.0];
pub const BRI_LPF_DIAL: [f32; 7] = [22000.0, 12000.0, 8000.0, 5000.0, 4000.0, 3500.0, 3000.0];

/// A dial's frequency at v: between its printed figures, log.
pub fn dial(v: f32, figures: &[f32]) -> f32 {
    let n = figures.len() as i32;
    let x = (if v < 0.0 {
        0.0
    } else if 1.0 < v {
        1.0
    } else {
        v
    }) * (n - 1) as f32;
    // C++ `std::min ((int) x, n - 2)`; `as` saturates where the C++ cast is undefined.
    let i = (x as i32).min(n - 2).max(0);
    let at = |k: i32| figures.get(k as usize).copied().unwrap_or(1.0);
    at(i) * (at(i + 1) / at(i)).powf(x - i as f32)
}

// The passive EQs' switch positions (labels and tests).
pub const PAS_LOW_HZ: [f32; 4] = [20.0, 30.0, 60.0, 100.0];
pub const PAS_HIGH_HZ: [f32; 7] = [3000.0, 4000.0, 5000.0, 8000.0, 10000.0, 12000.0, 16000.0];
pub const PAS_ATTEN_HZ: [f32; 3] = [5000.0, 10000.0, 20000.0];
pub const PAS_MID_LOW_HZ: [f32; 5] = [200.0, 300.0, 500.0, 700.0, 1000.0];
pub const PAS_MID_DIP_HZ: [f32; 11] = [200.0, 300.0, 500.0, 700.0, 1000.0, 1500.0, 2000.0, 3000.0, 4000.0, 5000.0, 7000.0];
pub const PAS_MID_HIGH_HZ: [f32; 5] = [1500.0, 2000.0, 3000.0, 4000.0, 5000.0];

/// The MEQ-5's switch positions as parts: inductor tap (mH), capacitance (nF).
#[derive(Debug, Clone, Copy)]
pub struct MeqPos {
    pub mh: f64,
    pub nf: f64,
}
const fn mp(mh: f64, nf: f64) -> MeqPos {
    MeqPos { mh, nf }
}
pub const MEQ_LOW: [MeqPos; 5] = [mp(520.0, 1200.0), mp(420.0, 670.0), mp(520.0, 200.0), mp(120.0, 470.0), mp(120.0, 200.0)];
pub const MEQ_DIP: [MeqPos; 11] = [
    mp(420.0, 1515.0),
    mp(277.0, 1000.0),
    mp(145.0, 680.0),
    mp(145.0, 330.0),
    mp(108.0, 220.0),
    mp(61.0, 170.0),
    mp(34.0, 170.0),
    mp(34.0, 68.0),
    mp(61.0, 30.0),
    mp(61.0, 16.0),
    mp(61.0, 10.0),
];
pub const MEQ_HIGH: [MeqPos; 5] = [mp(625.0, 18.0), mp(625.0, 10.2), mp(477.0, 5.9), mp(477.0, 3.0), mp(477.0, 2.0)];

// The console EQ's steps (index 0 of the two filters is Off).
pub const CON_HPF_HZ: [f32; 5] = [0.0, 50.0, 80.0, 160.0, 300.0];
pub const CON_LOW_HZ: [f32; 5] = [0.0, 35.0, 60.0, 110.0, 220.0];
pub const CON_MID_HZ: [f32; 6] = [360.0, 700.0, 1600.0, 3200.0, 4800.0, 7200.0];

// The proportional-Q EQ's steps.
pub const PRO_LOW_HZ: [f32; 5] = [50.0, 100.0, 200.0, 300.0, 400.0];
pub const PRO_MID_HZ: [f32; 5] = [400.0, 800.0, 1500.0, 3000.0, 5000.0];
pub const PRO_HIGH_HZ: [f32; 5] = [5000.0, 7000.0, 10000.0, 12500.0, 15000.0];
pub const PRO_GAIN_DB: [f32; 11] = [-12.0, -9.0, -6.0, -4.0, -2.0, 0.0, 2.0, 4.0, 6.0, 9.0, 12.0];

/// Every parameter's default, in its own units (knob 0..1, or a choice index).
pub const DEFAULTS: Values = [
    0.0, 1.0, 0.25, 1.0, 0.5, 1.0, 0.0, // circuit Passive, stage on, drive 0 dB, mix, output, Normal, bypass
    2.0, 0.0, 0.0, 4.0, 0.0, 0.5, 2.0, 0.0, 1.0, 0.0, 4.0, 0.0, 2.0, 0.0, // Passive: 60 Hz, 10 kHz, 20 kHz, MEQ 300 / 1k / 3k
    0.0, 2.0, 0.5, 2.0, 0.5, 0.5, // Console: HPF off, 60 Hz, 1.6 kHz, all flat
    1.0, 5.0, 0.0, 2.0, 5.0, 2.0, 5.0, 0.0, 0.0, // Proportional: 100 / 1.5k / 10k, all 0 dB, bells, filter out
    0.0, 0.0, 0.5, 0.5, 0.0, 0.5, 0.5, 0.25, 0.5, 0.5, 0.25, 0.5, 0.5, 0.0, 0.0, // British: filters off, flat, Q 1, brown
    0.0, 0.0, 0.5, 0.0, // Enhancer: Exciter, off
    0.0, 1.0, 0.5, // Air: off, top two bands, Dolby level -16 dBFS
    0.0, 0.7, 0.5, 0.5, 0.0, // Sampler: 12-bit 26k, 12 bits, 0 dB, filter off
    4.0, // Console gain: line 0
];

pub fn default_of(i: usize) -> f32 {
    DEFAULTS.get(i).copied().unwrap_or(0.0)
}

// ---------------------------------------------------------------- the EQP-1A, as a circuit
pub const K_PASSIVE_MAKEUP: f64 = 15.4882;
pub const K_INPUT_CORE_NODE: i32 = 12;
pub const K_PASSIVE_OUT: i32 = 11;
/// The plugin build's switch (`VINT_MODELLED_PASSIVE`).
pub const VINT_MODELLED_PASSIVE: bool = false;

/// The EQP-1A network's makeup for these settings' source: the ideal source's in this build.
pub fn passive_makeup(_v: &Values) -> f64 {
    // `if (! VINT_MODELLED_PASSIVE || ...) return kPassiveMakeup;`
    K_PASSIVE_MAKEUP
}

pub const K_VOLTS_AT_FULL_SCALE: f64 = 8.705;
pub const K_PULTEC_AMP_GAIN: f64 = 16.8832;

/// `(int) (v + 0.5f)` clamped to 0..n-1: a switch's position (C++ float add, then int cast).
#[inline]
fn idx(v: &Values, p: usize, n: i32) -> i32 {
    ((v[p] + 0.5) as i32).clamp(0, n - 1)
}

pub fn passive_netlist(v: &Values) -> Netlist {
    let ohms = |x: f64| x.max(1.0e-3); // a pot at its end is a wire
    let pick = |p: usize, t: &[f32]| f64::from(t[idx(v, p, t.len() as i32) as usize]);
    const C2_OF: [f32; 4] = [2.2e-6, 1.1e-6, 560e-9, 330e-9];
    const HC_OF: [f32; 3] = [270e-9, 135e-9, 68e-9];
    let c2 = pick(P_PAS_LOW_FREQ, &C2_OF);
    let c1lo = c2 / 30.0;
    let chc = pick(P_PAS_HIGH_ATTEN_FREQ, &HC_OF);
    let fc = pick(P_PAS_HIGH_FREQ, &PAS_HIGH_HZ);
    let chb = (1.0 / (12.0e6 * (2.0 * 3.14159265358979323846 * fc).powf(2.0))).sqrt();
    let lhb = 12.0e6 * chb;
    let hi_b = f64::from(v[P_PAS_HIGH_BOOST]);
    let lo_b = f64::from(v[P_PAS_LOW_BOOST]);
    let lo_c = f64::from(v[P_PAS_LOW_ATTEN]);
    let hi_c = f64::from(v[P_PAS_HIGH_ATTEN]);
    let q = f64::from(v[P_PAS_BANDWIDTH]);

    // nodes: 1 the source, 2 boost wiper, 3 C/L, 4 L/Q pot, 5 Q pot/390, 6 top, 7 cut wiper,
    //        8 cut C/91, 9 bottom, 10 below Cut Lo, 11 out
    // (Without VINT_MODELLED_PASSIVE the input stage is left out altogether: node 1 drives the
    // network directly, and nodes 12 and 13 do not exist.)
    let mut n = Netlist::default();
    let inp = 1;
    n.add(Kind::R, inp, 2, ohms((1.0 - hi_b) * 12.0e3));
    n.add(Kind::R, 2, 6, ohms(hi_b * 12.0e3));
    n.add(Kind::C, 2, 3, chb);
    n.add(Kind::L, 3, 4, lhb);
    n.add(Kind::R, 4, 5, ohms(q * 3.9e3));
    n.add(Kind::R, 5, 6, 390.0);
    n.add(Kind::R, 6, 10, ohms(lo_c * 110.0e3));
    n.add(Kind::C, 6, 10, c1lo);
    n.add(Kind::R, 6, 7, ohms((1.0 - hi_c) * 1.0e3));
    n.add(Kind::R, 7, 9, ohms(hi_c * 1.0e3));
    n.add(Kind::C, 7, 8, chc);
    n.add(Kind::R, 8, 9, 91.0);
    n.add(Kind::R, 10, 11, 1.0e3);
    n.add(Kind::R, 11, 9, 10.0e3);
    n.add(Kind::R, 9, 0, ohms(lo_b * 8.2e3));
    n.add(Kind::C, 9, 0, c2);
    n.out = 11;
    n
}

/// The MEQ-5's passive network (Gyraf Audio's schematic of the MEQ-5 filter section).
pub fn meq_netlist(v: &Values) -> Netlist {
    let ohms = |x: f64| x.max(1.0e-3);
    let lo = MEQ_LOW[idx(v, P_PAS_MID_LOW_FREQ, 5) as usize];
    let dp = MEQ_DIP[idx(v, P_PAS_MID_DIP_FREQ, 11) as usize];
    let hi = MEQ_HIGH[idx(v, P_PAS_MID_HIGH_FREQ, 5) as usize];
    let lb = f64::from(v[P_PAS_MID_LOW_PEAK]);
    let hb = f64::from(v[P_PAS_MID_HIGH_PEAK]);
    // the dip pot is reverse-log ("C" taper): the wiper's share of the track above ground
    let x = f64::from(v[P_PAS_MID_DIP]);
    let w = 1.0 - (81.0f64.powf(1.0 - x) - 1.0) / 80.0;

    // nodes: 1 in, 2 low wiper, 3 after the low pot, 4 after 510, 5 dip wiper, 6 after 270,
    //        7 dip L-C, 8 high wiper, 9 out, 10 low L-C, 11 high L-C
    let mut n = Netlist::default();
    n.add(Kind::R, 1, 2, ohms((1.0 - lb) * 2500.0));
    n.add(Kind::R, 2, 3, ohms(lb * 2500.0));
    n.add(Kind::L, 2, 10, lo.mh * 1.0e-3);
    n.add(Kind::C, 10, 3, lo.nf * 1.0e-9);
    n.add(Kind::R, 3, 0, 1000.0);
    n.add(Kind::R, 3, 4, 510.0);
    n.add(Kind::R, 4, 5, ohms((1.0 - w) * 2500.0));
    n.add(Kind::R, 5, 0, ohms(w * 2500.0));
    n.add(Kind::R, 5, 6, 270.0);
    n.add(Kind::L, 6, 7, dp.mh * 1.0e-3);
    n.add(Kind::C, 7, 0, dp.nf * 1.0e-9);
    n.add(Kind::R, 4, 8, ohms((1.0 - hb) * 10000.0));
    n.add(Kind::R, 8, 9, ohms(hb * 10000.0));
    n.add(Kind::L, 8, 11, hi.mh * 1.0e-3);
    n.add(Kind::C, 11, 9, hi.nf * 1.0e-9);
    n.add(Kind::R, 9, 0, 5100.0);
    n.out = 9;
    n
}

/// The MEQ-5 network's loss with every control at zero, made up by its amp (1 kHz). A function
/// static in the C++; forced in the engine's constructor here.
pub fn meq_makeup() -> f64 {
    static K: OnceLock<f64> = OnceLock::new();
    *K.get_or_init(|| {
        let z: Values = [0.0; NUM_PARAMS];
        1.0 / vintmna::ac(&meq_netlist(&z), 1000.0).norm()
    })
}

/// Each switch position's resonance, from its tap and capacitor(s) (Hz).
pub fn meq_centre(m: &MeqPos) -> f64 {
    1.0 / (2.0 * 3.14159265358979323846 * (m.mh * 1.0e-3 * m.nf * 1.0e-9).sqrt())
}

/// The output stage each circuit has.
pub fn stage_for(circuit: i32) -> i32 {
    const S: [i32; NUM_CIRCUITS as usize] = [
        if VINT_MODELLED_PASSIVE { vints::ST_VALVE } else { vints::ST_NONE },
        vints::ST_CLASS_A,
        vints::ST_DISCRETE,
        vints::ST_OP_AMP,
        vints::ST_OP_AMP,
        vints::ST_OP_AMP,
        vints::ST_NONE,
        vints::ST_NONE,
    ];
    S[circuit.clamp(0, NUM_CIRCUITS - 1) as usize]
}

/// The British knobs as the SSL card's pots.
pub fn british_controls(v: &Values) -> vintbri::Controls {
    use vintbri::{B_HF, B_HMF, B_LF, B_LMF};
    let mut k = vintbri::Controls { black: v[P_BRI_BLACK] > 0.5, ..vintbri::Controls::default() };
    let hpv = v[P_BRI_HPF];
    let lpv = v[P_BRI_LPF];
    if hpv > 0.001 {
        k.hpf_r = vintbri::hpf_pot_for(k.black, f64::from(dial(hpv, &BRI_HPF_DIAL)));
    }
    if lpv > 0.001 {
        k.lpf_r = vintbri::lpf_pot_for(k.black, f64::from(dial(lpv, &BRI_LPF_DIAL)));
    }
    k.lf_bell = v[P_BRI_LOW_BELL] > 0.5;
    k.hf_bell = v[P_BRI_HIGH_BELL] > 0.5;
    k.freq_r[B_LF] = vintbri::freq_pot_for(k.black, B_LF, f64::from(dial(v[P_BRI_LOW_FREQ], &BRI_LOW_DIAL)));
    k.freq_r[B_LMF] = vintbri::freq_pot_for(k.black, B_LMF, f64::from(dial(v[P_BRI_LM_FREQ], &BRI_LM_DIAL)));
    k.freq_r[B_HMF] = vintbri::freq_pot_for(k.black, B_HMF, f64::from(dial(v[P_BRI_HM_FREQ], &BRI_HM_DIAL)));
    k.freq_r[B_HF] = vintbri::freq_pot_for(k.black, B_HF, f64::from(dial(v[P_BRI_HIGH_FREQ], &BRI_HIGH_DIAL)));
    k.boost[B_LF] = vintbri::boost_for(k.black, if k.lf_bell { 1 } else { 0 }, f64::from(knob_to_bri_shelf_db(v[P_BRI_LOW_GAIN])));
    k.boost[B_LMF] = vintbri::boost_for(k.black, 2, f64::from(knob_to_bri_bell_db(v[P_BRI_LM_GAIN])));
    k.boost[B_HMF] = vintbri::boost_for(k.black, 3, f64::from(knob_to_bri_bell_db(v[P_BRI_HM_GAIN])));
    k.boost[B_HF] = vintbri::boost_for(k.black, if k.hf_bell { 5 } else { 4 }, f64::from(knob_to_bri_shelf_db(v[P_BRI_HIGH_GAIN])));
    k.q_rot[0] = vintbri::q_rot_for(k.black, 0, f64::from(knob_to_bri_q(v[P_BRI_LM_Q])));
    k.q_rot[1] = vintbri::q_rot_for(k.black, 1, f64::from(knob_to_bri_q(v[P_BRI_HM_Q])));
    k
}

/// The 550A's switches as the circuit sees them.
pub fn api_controls(v: &Values) -> vintapi::Controls {
    vintapi::Controls {
        freq: [idx(v, P_PRO_LOW_FREQ, 5), idx(v, P_PRO_MID_FREQ, 5), idx(v, P_PRO_HIGH_FREQ, 5)],
        gain: [idx(v, P_PRO_LOW_GAIN, 11), idx(v, P_PRO_MID_GAIN, 11), idx(v, P_PRO_HIGH_GAIN, 11)],
        low_shelf: v[P_PRO_LOW_SHELF] > 0.5,
        high_shelf: v[P_PRO_HIGH_SHELF] > 0.5,
        filter: v[P_PRO_FILTER] > 0.5,
    }
}

/// The 1073's switches and pots as the circuit sees them.
pub fn neve_controls(v: &Values) -> vintneve::Controls {
    vintneve::Controls {
        hpf: idx(v, P_CON_HPF, 5),
        low: idx(v, P_CON_LOW_FREQ, 5),
        low_pot: vintneve::pot_for(0, f64::from(knob_to_console_db(v[P_CON_LOW_GAIN]))),
        mid: idx(v, P_CON_MID_FREQ, 6),
        mid_pot: vintneve::pot_for(2, f64::from(knob_to_console_mid_db(v[P_CON_MID_GAIN]))),
        high_pot: vintneve::pot_for(1, f64::from(knob_to_console_db(v[P_CON_HIGH_GAIN]))),
        ..vintneve::Controls::default()
    }
}

/// The circuits run as netlists, in series, each driven by the op-amp before it: how many
/// (0: not a netlist circuit), written to out.
pub const K_MAX_CIRCUIT_SECTIONS: usize = 3;
pub fn circuit_sections(circuit: i32, v: &Values, out: &mut [Netlist; K_MAX_CIRCUIT_SECTIONS]) -> usize {
    if circuit == C_BRITISH {
        let k = british_controls(v);
        out[0] = vintbri::filter_section(&k);
        out[1] = vintbri::high_section(&k);
        out[2] = vintbri::low_section(&k);
        return 3;
    }
    if circuit == C_PASSIVE_MID {
        // the MEQ-5's network, its loss made up by an ideal gain (its own amp is not modelled)
        out[0] = meq_netlist(v);
        let made = out[0].nodes;
        let o = out[0].out;
        out[0].add_vcvs(o, 0, made, meq_makeup());
        out[0].out = made;
        return 1;
    }
    if circuit == C_CONSOLE {
        let k = neve_controls(v);
        out[0] = vintneve::hpf_section(&k);
        out[1] = vintneve::low_high_section(&k);
        out[2] = vintneve::presence_section(&k);
        return 3;
    }
    if circuit == C_PROPORTIONAL {
        let k = api_controls(v);
        out[0] = vintapi::filter_section(&k);
        out[1] = vintapi::eq_section(&k);
        return 2;
    }
    0
}

// ---------------------------------------------------------------- curve design
pub const K_MAX_SECTIONS: usize = 10;

#[derive(Debug, Clone, Copy)]
pub struct Design {
    pub s: [Coefs; K_MAX_SECTIONS],
    pub n: usize,
}

/// Every EQ section a circuit's settings call for. Every EQ is a circuit now (passiveNetlist,
/// meqNetlist, circuitSections), so there are none; the flavour circuits have no static curve.
pub fn design(_circuit: i32, _v: &Values, _fs: f64) -> Design {
    Design { s: [Coefs::default(); K_MAX_SECTIONS], n: 0 }
}

/// The whole curve, in dB, at f -- the circuits' exact analog responses. UI thread (allocates).
pub fn response_db(circuit: i32, v: &Values, f: f64, fs: f64) -> f64 {
    let d = design(circuit, v, fs);
    let mut m = 1.0;
    for s in d.s.iter().take(d.n) {
        m *= vintf::magnitude(s, f, fs);
    }
    if circuit == C_PASSIVE {
        m *= vintmna::ac(&passive_netlist(v), f).norm() * passive_makeup(v);
    }
    let mut nets = [Netlist::default(); K_MAX_CIRCUIT_SECTIONS];
    let ns = circuit_sections(circuit, v, &mut nets);
    if ns > 0 {
        let mut h = rustfft::num_complex::Complex::new(1.0, 0.0);
        for net in nets.iter().take(ns) {
            h *= vintmna::ac(net, f);
        }
        m *= h.norm();
    }
    20.0 * m.max(1.0e-9).log10()
}

/// Finite and within the supply's reach, tested on the bits.
fn physical(v: f64) -> bool {
    let b = v.to_bits();
    ((b >> 52) & 0x7ff) != 0x7ff && v.abs() < 1000.0
}

fn enh_sat(v: f64, bias: f64) -> f64 {
    // lopsided soft clip: odd AND even harmonics, as the Exciter's generator makes
    (v + bias).tanh() - bias.tanh()
}

// Parameters that are switches or steps: they land at once, never glide.
const STEPPED_PARAMS: [usize; 30] = [
    P_CIRCUIT,
    P_STAGE,
    P_QUALITY,
    P_BYPASS,
    P_PAS_LOW_FREQ,
    P_PAS_HIGH_FREQ,
    P_PAS_HIGH_ATTEN_FREQ,
    P_PAS_MID_LOW_FREQ,
    P_PAS_MID_DIP_FREQ,
    P_PAS_MID_HIGH_FREQ,
    P_CON_HPF,
    P_CON_LOW_FREQ,
    P_CON_MID_FREQ,
    P_PRO_LOW_FREQ,
    P_PRO_LOW_GAIN,
    P_PRO_LOW_SHELF,
    P_PRO_MID_FREQ,
    P_PRO_MID_GAIN,
    P_PRO_HIGH_FREQ,
    P_PRO_HIGH_GAIN,
    P_PRO_HIGH_SHELF,
    P_PRO_FILTER,
    P_BRI_LOW_BELL,
    P_BRI_HIGH_BELL,
    P_BRI_BLACK,
    P_ENH_TYPE,
    P_AIR_BANDS,
    P_SMP_MACHINE,
    P_SMP_FILTER,
    P_CON_PRE,
];

struct Channel {
    os: [vinthb::Stage; 3],
    /// the Passive's wet signal, brought down to the amp's rate
    os_wet: [vinthb::Stage; 3],
    eq: [Section; K_MAX_SECTIONS],
    stage: vints::Stage,
    /// the EQP-1A network
    passive: Solver,
    /// the netlist circuits' sections (circuitSections)
    sec: [Solver; K_MAX_CIRCUIT_SECTIONS],
    /// the Console's EQ again, inside the 1073's preamp
    sec_pre: [Solver; K_MAX_CIRCUIT_SECTIONS],
    /// the 1073's amplifiers around its EQ (the fast model)
    pre: FastPre,
    inter: Interstage,
    input: InputTransformer,
    last_input_draw: f64,
    last_inter_draw: f64,
    /// not run since the stage was last off: restart it at rest
    amp_idle: bool,

    // Enhancer
    enh_hp1: Section,
    enh_hp2: Section,
    enh_low: Section,
    enh_scoop: Section,
    /// the Maximizer: the 882's filter (OA1 to OA4), its summing amp (OA5), its detector's level
    max_filter: Solver,
    max_sum: Solver,
    max_env: f64,

    // Air: the four Dolby A bands, and their level detectors
    air_b1: Section,
    air_b2a: Section,
    air_b2b: Section,
    air_b3: Section,
    air_b4: Section,
    air_env: [f64; 4],

    // Sampler: 8th-order Butterworth in and out
    smp_aa: [Section; 4],
    smp_rc: [Section; 4],
    smp_phase: f64,
    smp_held: f64,
    smp_prev: f64,
}

impl Channel {
    #[cfg(test)]
    fn solvers(&self) -> impl Iterator<Item = &Solver> {
        std::iter::once(&self.passive).chain(self.sec.iter()).chain(self.sec_pre.iter()).chain([&self.max_filter, &self.max_sum])
    }

    fn new() -> Channel {
        Channel::with_solvers(
            Solver::default(),
            [Solver::default(), Solver::default(), Solver::default()],
            [Solver::default(), Solver::default(), Solver::default()],
            [Solver::default(), Solver::default()],
        )
    }

    /// Back to `Channel::new()`, in place: the solvers keep their inverse blocks (zeroed), so
    /// nothing is allocated or freed.
    fn reset(&mut self) {
        let passive = self.passive.take_reset();
        let sec = self.sec.each_mut().map(Solver::take_reset);
        let sec_pre = self.sec_pre.each_mut().map(Solver::take_reset);
        let max = [self.max_filter.take_reset(), self.max_sum.take_reset()];
        *self = Channel::with_solvers(passive, sec, sec_pre, max);
    }

    fn with_solvers(passive: Solver, sec: [Solver; K_MAX_CIRCUIT_SECTIONS], sec_pre: [Solver; K_MAX_CIRCUIT_SECTIONS], max: [Solver; 2]) -> Channel {
        let [max_filter, max_sum] = max;
        Channel {
            os: [vinthb::Stage::default(); 3],
            os_wet: [vinthb::Stage::default(); 3],
            eq: [Section::default(); K_MAX_SECTIONS],
            stage: vints::Stage::default(),
            passive,
            sec,
            sec_pre,
            pre: FastPre::default(),
            inter: Interstage::default(),
            input: InputTransformer::default(),
            last_input_draw: 0.0,
            last_inter_draw: 0.0,
            amp_idle: true,
            enh_hp1: Section::default(),
            enh_hp2: Section::default(),
            enh_low: Section::default(),
            enh_scoop: Section::default(),
            max_filter,
            max_sum,
            max_env: 0.0,
            air_b1: Section::default(),
            air_b2a: Section::default(),
            air_b2b: Section::default(),
            air_b3: Section::default(),
            air_b4: Section::default(),
            air_env: [0.0; 4],
            smp_aa: [Section::default(); 4],
            smp_rc: [Section::default(); 4],
            smp_phase: 0.0,
            smp_held: 0.0,
            smp_prev: 0.0,
        }
    }
}

/// The engine's members `processOversampled` and the circuits it calls read (the C++ `this`),
/// borrowed beside the channel they process: Rust cannot lend the engine whole while one of its
/// channels is lent mutably. Nothing is precomputed: `enhance`, `air` and `sample` read
/// `smoothed` themselves, as the C++ does.
struct Ctx<'a> {
    circuit: i32,
    mix_sm: f64,
    stage_sm: f64,
    sec_count: usize,
    active_sections: usize,
    passive_makeup_now: f64,
    pre_volts: f64,
    pre_gain: f64,
    con_rate: f64,
    os_factor: usize,
    num_stages: usize,
    amp_decimation: usize,
    con_decimation: usize,
    smoothed: &'a Values,
    smp_step: f64,
    max_att: f64,
    max_rel: f64,
    max_vca_open: bool,
    air_att: f64,
    air_rel: &'a [f64; 4],
    inter_at_rest: &'a Interstage,
    input_at_rest: &'a InputTransformer,
}

pub struct VintEngineDsp {
    ch: Vec<Channel>,
    /// the MNA solve scratch: always `Some` (an `Option` only so `reset` can move it into the
    /// re-initialised engine without allocating)
    scratch: Option<Box<Scratch>>,
    hb_coefs: [vinthb::Coefs; 3],

    values: Values,
    smoothed: Values,
    /// as last solved, and as now
    sec_net: [Netlist; K_MAX_CIRCUIT_SECTIONS],
    sec_next: [Netlist; K_MAX_CIRCUIT_SECTIONS],
    sec_solved: bool,
    sec_skip: bool,
    passive_solved: bool,
    /// the Maximizer's circuit: solved, and the capacitor scale it was solved for
    max_solved: bool,
    max_scale: f64,
    sec_count: usize,
    sec_circuit: i32,
    circuit: i32,
    quality: i32,
    design_dirty: bool,
    calibrating: bool,
    design_counter: i32,
    active_sections: usize,

    fs_base: f64,
    os_fs: f64,
    smooth: f64,
    smooth16: f64,
    os_factor: usize,
    num_stages: usize,
    mix_sm: f64,
    out_sm: f64,
    byp_sm: f64,
    drive_sm: f64,
    stage_sm: f64,
    iron_trim: [f64; vints::NUM_STAGE_TYPES],
    /// the interstage and input transformer at rest (never prepared in this build)
    inter_at_rest: Interstage,
    input_at_rest: InputTransformer,
    /// the EQP-1A network's makeup for the present source
    passive_makeup_now: f64,
    amp_decimation: usize,
    con_decimation: usize,
    con_rate: f64,
    pre_volts: f64,
    pre_gain: f64,

    smp_step: f64,
    max_att: f64,
    max_rel: f64,
    max_vca_open: bool,
    air_att: f64,
    air_rel: [f64; 4],

    in_peak: f32,
    out_peak: f32,
}

impl Default for VintEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl VintEngineDsp {
    pub fn new() -> Self {
        Self::with_buffers((0..2).map(|_| Channel::new()).collect(), Some(Scratch::new_boxed()))
    }

    /// Back to exactly what `new()` builds, in place: the channels, their solvers' inverse blocks
    /// and the solve scratch are reused (zeroed or re-defaulted), every other member re-defaulted.
    /// Nothing is allocated or freed, so a host reset may run it on the audio thread.
    pub fn reset(&mut self) {
        let mut ch = std::mem::take(&mut self.ch);
        for c in &mut ch {
            c.reset();
        }
        let mut scratch = self.scratch.take();
        if let Some(sc) = scratch.as_deref_mut() {
            sc.clear();
        }
        // The old engine left behind owns an empty Vec and no scratch: dropping it frees nothing.
        *self = Self::with_buffers(ch, scratch);
    }

    fn with_buffers(ch: Vec<Channel>, scratch: Option<Box<Scratch>>) -> Self {
        // The C++ constructor measures the 1073's knob calibration now, not on the audio thread;
        // the MEQ-5's makeup (a function static there) is measured here too (both are computed
        // once per process, so a reset only reads them).
        let _ = vintneve::pot_table(0);
        let _ = meq_makeup();
        let mut iron_trim = [0.0; vints::NUM_STAGE_TYPES];
        for (s, t) in iron_trim.iter_mut().enumerate() {
            *t = vintgain::IRON_TRIM[s];
        }
        VintEngineDsp {
            ch,
            scratch,
            hb_coefs: [vinthb::coefs_for_stage(0), vinthb::coefs_for_stage(1), vinthb::coefs_for_stage(2)],
            values: DEFAULTS,
            smoothed: [0.0; NUM_PARAMS],
            sec_net: [Netlist::default(); K_MAX_CIRCUIT_SECTIONS],
            sec_next: [Netlist::default(); K_MAX_CIRCUIT_SECTIONS],
            sec_solved: false,
            sec_skip: false,
            passive_solved: false,
            max_solved: false,
            max_scale: 1.0,
            sec_count: 0,
            sec_circuit: -1,
            circuit: C_PASSIVE,
            quality: 1,
            design_dirty: true,
            calibrating: false,
            design_counter: 0,
            active_sections: 0,
            fs_base: 0.0,
            os_fs: 0.0,
            smooth: 0.001,
            smooth16: 0.01,
            os_factor: 4,
            num_stages: 2,
            mix_sm: 1.0,
            out_sm: 1.0,
            byp_sm: 0.0,
            drive_sm: 0.25,
            stage_sm: 1.0,
            iron_trim,
            inter_at_rest: Interstage::default(),
            input_at_rest: InputTransformer::default(),
            passive_makeup_now: K_PASSIVE_MAKEUP,
            amp_decimation: 0,
            con_decimation: 0,
            con_rate: 96000.0,
            pre_volts: 8.7,
            pre_gain: 1.0,
            smp_step: 0.1,
            max_att: 0.01,
            max_rel: 0.001,
            max_vca_open: false,
            air_att: 0.01,
            air_rel: [0.0; 4],
            in_peak: 0.0,
            out_peak: 0.0,
        }
    }

    // ---------------------------------------------------------------- parameters in
    pub fn set_parameter(&mut self, index: usize, value: f32) {
        if index >= NUM_PARAMS {
            return;
        }
        if self.values[index] == value {
            return;
        }
        self.values[index] = value;

        if index == P_CIRCUIT {
            self.circuit = ((value + 0.5) as i32).clamp(0, NUM_CIRCUITS - 1);
            if self.os_fs > 0.0 {
                self.configure_stage();
            }
            self.design_counter = 16; // the new circuit's design at the very next sample
        } else if index == P_QUALITY {
            let q = ((value + 0.5) as i32).clamp(0, 2);
            if q != self.quality {
                self.quality = q;
                if self.fs_base > 0.0 {
                    self.rebuild_oversampling();
                }
            }
        }
        self.design_dirty = true;
    }

    pub fn value(&self, index: usize) -> f32 {
        self.values.get(index).copied().unwrap_or(0.0)
    }

    // ---------------------------------------------------------------- lifecycle
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs_base = if sample_rate > 0.0 { sample_rate } else { 48000.0 };
        self.smooth = 1.0 - (-1.0 / (0.015 * self.fs_base)).exp();
        self.rebuild_oversampling();
        self.mix_sm = f64::from(self.values[P_MIX]);
        self.out_sm = self.out_gain_target();
        self.byp_sm = self.bypass_target();
        self.drive_sm = f64::from(self.values[P_DRIVE]);
        self.stage_sm = self.stage_target();
    }

    /// Holds the Maximizer's VCA open, as it is on programme: for measuring the response with an
    /// impulse, which is over before the detector has heard it.
    pub fn hold_vca_open(&mut self, on: bool) {
        self.max_vca_open = on;
    }

    /// The rate `prepare` was given (0 before the first).
    pub fn sample_rate(&self) -> f64 {
        self.fs_base
    }

    /// Every heap block the engine owns (for the allocation guard).
    #[cfg(test)]
    pub fn heap_blocks(&self) -> Vec<usize> {
        let scratch = self.scratch.as_deref().map_or(0, |s| s as *const Scratch as usize);
        let mut v = vec![self.ch.as_ptr() as usize, self.ch.capacity(), scratch];
        for c in &self.ch {
            v.extend(c.solvers().map(|s| s.heap_block()));
        }
        v
    }

    pub fn get_oversampling(&self) -> usize {
        self.os_factor
    }
    pub fn take_input_peak(&mut self) -> f32 {
        std::mem::take(&mut self.in_peak)
    }
    pub fn take_output_peak(&mut self) -> f32 {
        std::mem::take(&mut self.out_peak)
    }

    // ---- calibration hooks, for Tests/calibrate.cpp only
    pub fn set_calibrating(&mut self, b: bool) {
        self.calibrating = b;
    }

    // ---------------------------------------------------------------- audio
    /// `channels[c][n]`, `num_channels` of them (at most 2 are processed), `num_samples` each.
    pub fn process(&mut self, channels: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        let nc = num_channels.min(2).min(channels.len());
        if nc == 0 {
            return;
        }
        let len = channels.iter().take(nc).map(Vec::len).min().unwrap_or(0).min(num_samples);

        for n in 0..len {
            let (mix_t, drive_t) = (f64::from(self.values[P_MIX]), f64::from(self.values[P_DRIVE]));
            let (out_t, byp_t, stage_t) = (self.out_gain_target(), self.bypass_target(), self.stage_target());
            let s = self.smooth;
            glide(s, &mut self.mix_sm, mix_t);
            glide(s, &mut self.out_sm, out_t);
            glide(s, &mut self.byp_sm, byp_t);
            glide(s, &mut self.drive_sm, drive_t);
            glide(s, &mut self.stage_sm, stage_t);

            // The EQ follows its knobs every 16 samples.
            self.design_counter += 1;
            if self.design_counter >= 16 {
                self.design_counter = 0;
                if self.design_dirty || self.gliding() {
                    self.redesign();
                }
            }

            let inp = [channels[0][n], if nc > 1 { channels[1][n] } else { 0.0 }];
            self.in_peak = cpp_max(self.in_peak, cpp_max(inp[0].abs(), inp[1].abs()));

            let drive_gain = 10.0f64.powf(f64::from(knob_to_drive_db(self.drive_sm as f32)) / 20.0);
            let makeup = self.stage_makeup();
            let ctx = Ctx {
                circuit: self.circuit,
                mix_sm: self.mix_sm,
                stage_sm: self.stage_sm,
                sec_count: self.sec_count,
                active_sections: self.active_sections,
                passive_makeup_now: self.passive_makeup_now,
                pre_volts: self.pre_volts,
                pre_gain: self.pre_gain,
                con_rate: self.con_rate,
                os_factor: self.os_factor,
                num_stages: self.num_stages,
                amp_decimation: self.amp_decimation,
                con_decimation: self.con_decimation,
                smoothed: &self.smoothed,
                smp_step: self.smp_step,
                max_att: self.max_att,
                max_rel: self.max_rel,
                max_vca_open: self.max_vca_open,
                air_att: self.air_att,
                air_rel: &self.air_rel,
                inter_at_rest: &self.inter_at_rest,
                input_at_rest: &self.input_at_rest,
            };

            for c in 0..nc {
                let y = process_oversampled(&mut self.ch[c], &ctx, inp[c], drive_gain, makeup);
                let out = y * self.out_sm as f32;
                let o = if self.byp_sm >= 1.0 { inp[c] } else { out + self.byp_sm as f32 * (inp[c] - out) };
                channels[c][n] = o;
                self.out_peak = cpp_max(self.out_peak, o.abs());
            }
        }
    }

    fn smp_rate(&self) -> f64 {
        let machine = ((self.smoothed[P_SMP_MACHINE] + 0.5) as i32).clamp(0, 2);
        if machine == 0 {
            26040.0
        } else if machine == 1 {
            40000.0
        } else {
            f64::from(knob_to_smp_rate_hz(self.smoothed[P_SMP_RATE]))
        }
    }

    /// Filters and constants for the flavour circuits, from the (gliding) values.
    fn design_flavour(&mut self) {
        use vintf::{bell, bilinear, high_pass2, low_pass1, low_pass2};
        let os_fs = self.os_fs;
        let sm = self.smoothed;
        let tune = f64::from(sweep(sm[P_ENH_TUNE], 700.0, 7000.0));
        let hp = bilinear(&high_pass2(0.7071), tune, os_fs);
        let lp120 = bilinear(&low_pass1(), 120.0, os_fs);
        let scoop = bilinear(&bell(10.0f64.powf(-6.0 * f64::from(sm[P_ENH_AMOUNT]) / 20.0), 0.5), 1000.0, os_fs);

        let b1 = bilinear(&low_pass2(0.7071), 80.0, os_fs);
        let b2a = bilinear(&high_pass2(0.7071), 80.0, os_fs);
        let b2b = bilinear(&low_pass2(0.7071), 3000.0, os_fs);
        let b3 = bilinear(&high_pass2(0.7071), 3000.0, os_fs);
        let b4 = bilinear(&high_pass2(0.7071), 9000.0, os_fs);

        let rate = self.smp_rate();
        self.smp_step = rate / os_fs;
        // 8th-order Butterworth at 0.42 x the machine's rate, in and out: the four section Qs
        const BQ: [f64; 4] = [0.5098, 0.6013, 0.9000, 2.5629];
        let aa = BQ.map(|q| bilinear(&low_pass2(q), 0.42 * rate, os_fs));

        for c in &mut self.ch {
            c.enh_hp1.c = hp;
            c.enh_hp2.c = hp;
            c.enh_low.c = lp120;
            c.enh_scoop.c = scoop;
            c.air_b1.c = b1;
            c.air_b2a.c = b2a;
            c.air_b2b.c = b2b;
            c.air_b3.c = b3;
            c.air_b4.c = b4;
            for i in 0..4 {
                c.smp_aa[i].c = aa[i];
                c.smp_rc[i].c = aa[i];
            }
        }
        (self.max_att, self.max_rel) = vintmax::detector_steps(os_fs);
        self.air_att = 1.0 - (-1.0 / (0.002 * os_fs)).exp();
        const REL_MS: [f64; 4] = [150.0, 80.0, 60.0, 60.0];
        for i in 0..4 {
            self.air_rel[i] = 1.0 - (-1.0 / (REL_MS[i] * 0.001 * os_fs)).exp();
        }
    }

    // ---------------------------------------------------------------- the EQ, re-made from gliding values
    fn gliding(&self) -> bool {
        self.smoothed.iter().zip(&self.values).any(|(s, v)| s != v)
    }

    fn redesign(&mut self) {
        // Continuous knobs glide; switches and steps land at once.
        for i in 0..NUM_PARAMS {
            let t = f64::from(self.values[i]);
            let mut s = f64::from(self.smoothed[i]);
            s += self.smooth16 * (t - s);
            if (t - s).abs() < 1.0e-6 {
                s = t;
            }
            self.smoothed[i] = s as f32;
        }
        for p in STEPPED_PARAMS {
            self.smoothed[p] = self.values[p];
        }

        let d = design(self.circuit, &self.smoothed, self.os_fs);
        self.design_flavour();
        // The circuits solved as circuits are solved only while they are the one in use (and the
        // Passive once, at the start, so it is never without a solution).
        if self.circuit == C_PASSIVE || !self.passive_solved {
            let net = passive_netlist(&self.smoothed);
            self.passive_makeup_now = passive_makeup(&self.smoothed);
            if let Some(scratch) = self.scratch.as_deref_mut() {
                for c in &mut self.ch {
                    c.passive.set(&net, self.os_fs, scratch);
                }
            }
            self.passive_solved = true;
        }
        // The Maximizer's circuit: solved when the Enhancer comes into use, and again only when
        // Tune moves its capacitors (the pots are outside the filter: they need no solve).
        if self.circuit == C_ENHANCER {
            let scale = vintmax::cap_scale(self.smoothed[P_ENH_TUNE]);
            if (!self.max_solved || scale != self.max_scale)
                && let Some(scratch) = self.scratch.as_deref_mut()
            {
                let filter = vintmax::filter_netlist(scale);
                let sum = vintmax::summing_netlist();
                for c in &mut self.ch {
                    c.max_filter.set(&filter, self.os_fs, scratch);
                    if !self.max_solved {
                        c.max_sum.set_running(&sum, self.os_fs, scratch);
                    }
                }
                self.max_solved = true;
                self.max_scale = scale;
            }
        }
        // The netlist circuits: each section solved again only when one of its parts changed, and
        // once, for both channels. While a knob glides, every other time.
        self.sec_skip = !self.sec_skip;
        if self.sec_circuit != self.circuit {
            self.sec_solved = false;
            self.sec_circuit = self.circuit;
        }
        if !(self.sec_solved && self.sec_skip && self.gliding()) {
            let ns = circuit_sections(self.circuit, &self.smoothed, &mut self.sec_next);
            let (first, rest) = self.ch.split_at_mut(1);
            if let (Some(c0), Some(c1), Some(scratch)) = (first.first_mut(), rest.first_mut(), self.scratch.as_deref_mut()) {
                for s in 0..ns {
                    if self.sec_solved && vintbri::same(&self.sec_next[s], &self.sec_net[s]) {
                        continue;
                    }
                    c0.sec[s].set_running(&self.sec_next[s], self.os_fs, scratch);
                    c1.sec[s].adopt(&c0.sec[s]);
                    if self.circuit == C_CONSOLE {
                        c0.sec_pre[s].set_running(&self.sec_next[s], self.con_rate, scratch);
                        c1.sec_pre[s].adopt(&c0.sec_pre[s]);
                    }
                    self.sec_net[s] = self.sec_next[s];
                }
            }
            self.sec_count = ns;
            self.sec_solved = true;
        }
        for c in &mut self.ch {
            for i in 0..d.n {
                c.eq[i].c = d.s[i];
            }
            // a section that has just come into use starts from rest
            for i in self.active_sections..d.n {
                c.eq[i].reset();
            }
        }
        self.active_sections = d.n;
        // the 1073's switch: its position, and the level and gain around it
        {
            let w = vintpre::wafer((self.smoothed[P_CON_PRE] + 0.5) as i32);
            let pos = vintpre::pre_position(w);
            self.pre_volts = vintpre::volts_per_unit(pos.mic);
            self.pre_gain = if pos.mute { 0.0 } else { 10.0f64.powf(vintpre::position_db(w) / 20.0) };
            if self.circuit == C_CONSOLE {
                for c in &mut self.ch {
                    c.pre.set_position(w);
                }
            }
        }
        self.design_dirty = false;
    }

    // ---------------------------------------------------------------- the output stage
    fn stage_target(&self) -> f64 {
        if self.values[P_STAGE] > 0.5 && stage_for(self.circuit) != vints::ST_NONE { 1.0 } else { 0.0 }
    }

    fn stage_makeup(&self) -> f64 {
        if self.calibrating || !vintgain::K_CALIBRATED {
            return 1.0;
        }
        let s = stage_for(self.circuit) as usize;
        let pos = self.drive_sm * (vintgain::K_POINTS - 1) as f64;
        // C++ `clampT ((int) pos, 0, kPoints - 2)`
        let i = clamp_t(pos as i32, 0, vintgain::K_POINTS as i32 - 2) as usize;
        let t = pos - i as f64;
        let row = &vintgain::TABLE[s.min(vintgain::K_STAGES - 1)];
        // `table[s][i] + t * (table[s][i + 1] - table[s][i])`: the difference is a float one
        f64::from(row[i]) + t * f64::from(row[i + 1] - row[i])
    }

    fn configure_stage(&mut self) {
        // The makeup amp runs at 4x at most (no amp in this build, no need).
        self.amp_decimation = if VINT_MODELLED_PASSIVE { self.num_stages.saturating_sub(2) } else { 0 };
        // The Console runs at twice the session's rate at every quality.
        self.con_decimation = self.num_stages.saturating_sub(1);
        self.con_rate = self.os_fs / (1u32 << self.con_decimation) as f64;
        if self.circuit == C_CONSOLE {
            let w = vintpre::wafer((self.values[P_CON_PRE] + 0.5) as i32);
            for c in &mut self.ch {
                c.pre.prepare(self.con_rate, w);
            }
        }
        self.sec_solved = false;
        // (VINT_MODELLED_PASSIVE: ampAtRest / interAtRest / inputAtRest prepared here; not in this build.)
        let t = stage_for(self.circuit);
        for c in &mut self.ch {
            c.stage.configure(t, &self.iron_trim);
            c.stage.prepare(self.os_fs);
            // c.amp = ampAtRest: the amp is never run in this build (see the module notes).
            c.inter = self.inter_at_rest;
            c.input = self.input_at_rest;
            c.amp_idle = false;
        }
    }

    fn out_gain_target(&self) -> f64 {
        10.0f64.powf(f64::from(knob_to_out_db(self.values[P_OUTPUT])) / 20.0)
    }
    fn bypass_target(&self) -> f64 {
        if self.values[P_BYPASS] > 0.5 { 1.0 } else { 0.0 }
    }

    fn rebuild_oversampling(&mut self) {
        const FACTORS: [usize; 3] = [2, 4, 8];
        self.quality = ((self.values[P_QUALITY] + 0.5) as i32).clamp(0, 2);
        self.os_factor = FACTORS[self.quality as usize];
        self.os_fs = self.fs_base * self.os_factor as f64;
        self.num_stages = if self.os_factor == 2 {
            1
        } else if self.os_factor == 4 {
            2
        } else {
            3
        };
        self.smooth16 = 1.0 - (-16.0 / (0.015 * self.fs_base)).exp();
        let coefs = self.hb_coefs;
        for c in &mut self.ch {
            for st in 0..3 {
                c.os[st].set(&coefs[st]);
                c.os_wet[st].set(&coefs[st]);
            }
            for s in &mut c.eq {
                s.reset();
            }
            c.passive.reset();
            for b in &mut c.sec {
                b.reset();
            }
            c.max_filter.reset();
            c.max_sum.reset();
            c.max_env = 0.0;
        }
        self.sec_solved = false;
        self.passive_solved = false;
        self.max_solved = false;
        self.configure_stage();
        self.smoothed = self.values;
        self.active_sections = 0;
        self.redesign();
    }

    /// The drive knob's value (for the readout's knob map).
    pub fn drive_knob(&self) -> f32 {
        self.values[P_DRIVE]
    }
}

/// C++ `std::max (a, b)` on floats: `a < b ? b : a`.
#[inline]
fn cpp_max(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

#[inline]
fn glide(smooth: f64, v: &mut f64, target: f64) {
    *v += smooth * (target - *v);
    if (target - *v).abs() < 1.0e-9 {
        *v = target;
    }
}

fn process_oversampled(c: &mut Channel, f: &Ctx, x_in: f32, drive_gain: f64, makeup: f64) -> f32 {
    let nn = f.os_factor.min(8);
    let num_stages = f.num_stages.min(3);
    let mut buf = [0.0f64; 8];
    let mut tmp = [0.0f64; 8];
    buf[0] = f64::from(x_in);
    let mut count = 1usize;
    for st in 0..num_stages {
        for i in 0..count {
            let (a, b) = c.os[st].up(buf[i]);
            tmp[2 * i] = a;
            tmp[2 * i + 1] = b;
        }
        count *= 2;
        buf[..count].copy_from_slice(&tmp[..count]);
    }

    // The Passive's amp runs at 4x at most: at Insane the wet signal is brought down a stage
    // first, and the dry through an identical filter. The Console runs its preamp at 2x.
    let dec = if f.circuit == C_PASSIVE {
        f.amp_decimation
    } else if f.circuit == C_CONSOLE {
        f.con_decimation
    } else {
        0
    };
    let mut wet = [0.0f64; 8];
    for k in 0..nn {
        let dry = buf[k];
        let mut w = dry;
        if f.circuit == C_PASSIVE {
            w = passive_network(c, f, dry, drive_gain) * f.passive_makeup_now;
        } else {
            for s in 0..f.sec_count.min(K_MAX_CIRCUIT_SECTIONS) {
                w = c.sec[s].run(w);
            }
        }
        for i in 0..f.active_sections.min(K_MAX_SECTIONS) {
            w = c.eq[i].run(w);
        }
        if f.circuit == C_ENHANCER {
            w = enhance(c, f, w);
        } else if f.circuit == C_AIR {
            w = air(c, f, w);
        } else if f.circuit == C_SAMPLER {
            w = sample(c, f, w);
        }

        if dec == 0 {
            buf[k] = finish(c, f, dry, w, drive_gain, makeup);
        } else {
            wet[k] = w;
        }
    }

    let mut st = num_stages as i32 - 1;
    if dec > 0 {
        for _ in 0..dec {
            count /= 2;
            let s = st.max(0) as usize;
            for i in 0..count {
                buf[i] = c.os[s].down(buf[2 * i], buf[2 * i + 1]);
                wet[i] = c.os_wet[s].down(wet[2 * i], wet[2 * i + 1]);
            }
            st -= 1;
        }
        for i in 0..count {
            buf[i] = finish(c, f, buf[i], wet[i], drive_gain, makeup);
        }
    }
    while st >= 0 {
        count /= 2;
        for i in 0..count {
            buf[i] = c.os[st as usize].down(buf[2 * i], buf[2 * i + 1]);
        }
        st -= 1;
    }
    buf[0] as f32
}

/// The output stage, crossfaded in and out so its switch never clicks; then the mix.
fn finish(c: &mut Channel, f: &Ctx, dry: f64, mut w: f64, drive_gain: f64, makeup: f64) -> f64 {
    if f.circuit == C_CONSOLE {
        w = console(c, f, dry, w);
        return dry + f.mix_sm * (w - dry);
    }
    if f.stage_sm > 0.0 && c.stage.type_ != vints::ST_NONE {
        // C++: `(circuit == cPassive ? pultecAmp (c, w, driveGain) : c.stage.run (w * driveGain) /
        // driveGain) * makeup`. The Passive's stage is stNone in this build (stageFor), so with a
        // stage here the circuit is never the Passive: the stage's path is the one that runs.
        let driven = c.stage.run(w * drive_gain) / drive_gain * makeup;
        w += f.stage_sm * (driven - w);
    } else {
        c.amp_idle = true;
    }
    dry + f.mix_sm * (w - dry)
}

/// The EQP-1A's network, and -- while the output stage is fading -- the interstage and input
/// transformers on it (see `vint_pultec_amp` for when that happens in this build).
fn passive_network(c: &mut Channel, f: &Ctx, x: f64, drive_gain: f64) -> f64 {
    if f.stage_sm <= 0.0 {
        return c.passive.run(x); // nothing loads it: the fast path
    }
    c.passive.open_circuit(x);
    restart_if_idle(c, f);
    let volts = K_VOLTS_AT_FULL_SCALE * drive_gain;
    let (xn, on) = (K_INPUT_CORE_NODE, K_PASSIVE_OUT);
    let v_x = c.passive.node_open(xn) * volts;
    let v_o = c.passive.node_open(on) * volts;
    let r_xx = c.passive.transfer(xn, xn);
    let r_xo = c.passive.transfer(xn, on);
    let r_ox = c.passive.transfer(on, xn);
    let r_oo = c.passive.transfer(on, on);
    // Two cores on one network, solved in turn from last sample's currents.
    let mut i_x = c.last_input_draw;
    let mut i_o = c.last_inter_draw;
    let mut x_in = v_x;
    let mut grid = v_o;
    for pass in 0..6 {
        let (xi, d_x) = c.input.solve(v_x - r_xo * i_o, r_xx, pass == 0);
        let (g, d_o) = c.inter.solve(v_o - r_ox * d_x, r_oo, pass == 0);
        x_in = xi;
        grid = g;
        let settled = (r_xo * (d_o - i_o)).abs() < 1.0e-7;
        i_x = d_x;
        i_o = d_o;
        if settled {
            break;
        }
    }
    c.last_input_draw = i_x;
    c.last_inter_draw = i_o;
    c.input.commit(x_in);
    c.inter.commit(grid);
    c.passive.commit2(xn, i_x / volts, on, i_o / volts);
    grid / volts
}

/// The stage was out: its amp and transformers come back in at rest.
fn restart_if_idle(c: &mut Channel, f: &Ctx) {
    if c.amp_idle {
        // c.amp = ampAtRest (never run in this build)
        c.inter = *f.inter_at_rest;
        c.input = *f.input_at_rest;
        c.last_input_draw = 0.0;
        c.last_inter_draw = 0.0;
        c.amp_idle = false;
    }
}

/// The Console's output stage: the 1073's preamp around its EQ, at the preamp's rate,
/// crossfaded with the ideal EQ.
fn console(c: &mut Channel, f: &Ctx, x: f64, ideal: f64) -> f64 {
    let mut modelled = 0.0;
    if f.stage_sm > 0.0 {
        if f.pre_gain <= 0.0 {
            modelled = 0.0; // an OFF position
        } else {
            let mut v = c.pre.front(x * f.pre_volts);
            for s in 0..f.sec_count.min(K_MAX_CIRCUIT_SECTIONS) {
                v = c.sec_pre[s].run(v);
            }
            modelled = c.pre.back(v) / (f.pre_volts * f.pre_gain);
            if !physical(modelled) {
                let p = c.pre.get_position();
                c.pre.prepare(f.con_rate, p);
                modelled = 0.0;
            }
        }
    }
    ideal + f.stage_sm * (modelled - ideal)
}

// ---------------------------------------------------------------- 4: Enhancer
fn enhance(c: &mut Channel, fl: &Ctx, x: f64) -> f64 {
    let sm = fl.smoothed;
    let ty = ((sm[P_ENH_TYPE] + 0.5) as i32).clamp(0, 2);
    let amount = f64::from(sm[P_ENH_AMOUNT]);
    let low = f64::from(sm[P_ENH_LOW]);

    // The Exciter's and the Vitalizer's filters run all the time, whichever box is selected.
    let band = c.enh_hp1.run(x);
    let scooped = c.enh_scoop.run(x);

    if ty == 1 {
        // The BBE 882, as a circuit (`vint_maximizer`). Amount is its PROCESS pot, Low its
        // LO CONTOUR pot. It runs with both at minimum too, so that it is settled when one rises.
        c.enh_low.run(x);
        c.enh_hp2.run(0.0);
        let lp = c.max_filter.open_circuit(x);
        let bp = c.max_filter.node_open(vintmax::N_BP);
        let hp = c.max_filter.node_open(vintmax::N_HP);
        let buffer = c.max_filter.node_open(vintmax::N_BUFFER);
        c.max_filter.commit(0.0);
        c.max_env = vintmax::detect(c.max_env, buffer * K_VOLTS_AT_FULL_SCALE, fl.max_att, fl.max_rel);
        let vca = if fl.max_vca_open { 1.0 } else { vintmax::vca_gain(c.max_env) };
        let y = c.max_sum.run(vintmax::summing_drive(lp, bp, hp, vca, low, amount));
        if amount <= 0.0 && low <= 0.0 {
            return x; // off is the input itself
        }
        return y;
    }

    let low_add = low_shelf_add(c, x, low, ty == 2);

    if amount <= 0.0 && low <= 0.0 {
        c.enh_hp2.run(0.0);
        return x; // off is the input itself
    }

    if ty == 0 {
        let top = c.enh_hp2.run(enh_sat(band * 4.0, 0.25));
        return x + amount * 0.35 * top + low_add;
    }
    let top = c.enh_hp2.run((band * 3.0).tanh());
    scooped + amount * 0.25 * top + low_add
}

fn low_shelf_add(c: &mut Channel, x: f64, low: f64, saturate: bool) -> f64 {
    // The low contour is added as a band, x + (g - 1) * lowpass(x). Up to +8 dB, below ~120 Hz.
    let l = c.enh_low.run(x);
    if low <= 0.0 {
        return 0.0;
    }
    let g = 10.0f64.powf(8.0 * low / 20.0) - 1.0;
    let add = g * l;
    if saturate { (add * 2.0).tanh() * 0.5 } else { add }
}

// ---------------------------------------------------------------- 5: Air (the Dolby A encode trick)
fn air(c: &mut Channel, fl: &Ctx, x: f64) -> f64 {
    let sm = fl.smoothed;
    let amount = f64::from(sm[P_AIR_AMOUNT]);
    let bands = ((sm[P_AIR_BANDS] + 0.5) as i32).clamp(0, 2);
    let ref_db = f64::from(knob_to_air_ref_db(sm[P_AIR_REF]));

    let b2 = c.air_b2a.run(x);
    let b = [c.air_b1.run(x), c.air_b2b.run(b2), c.air_b3.run(x), c.air_b4.run(x)];
    const MAX_DB: [f64; 4] = [10.0, 10.0, 10.0, 15.0];
    let first = if bands == 0 {
        0
    } else if bands == 1 {
        2
    } else {
        3
    };

    let mut y = x;
    for i in 0..4 {
        let e = b[i].abs();
        c.air_env[i] += (if e > c.air_env[i] { fl.air_att } else { fl.air_rel[i] }) * (e - c.air_env[i]);
        if i < first || amount <= 0.0 {
            continue;
        }
        // Full lift 40 dB and more below Dolby level, none at Dolby level.
        let level_db = 20.0 * (c.air_env[i] * 1.4142 + 1e-9).log10() - ref_db;
        let lift = MAX_DB[i] * clamp_t(-level_db / 40.0, 0.0, 1.0) * amount;
        y += (10.0f64.powf(lift / 20.0) - 1.0) * b[i];
    }
    y
}

// ---------------------------------------------------------------- 6: Sampler
fn sample(c: &mut Channel, fl: &Ctx, x: f64) -> f64 {
    let sm = fl.smoothed;
    let machine = ((sm[P_SMP_MACHINE] + 0.5) as i32).clamp(0, 2);
    let filt = (sm[P_SMP_FILTER] > 0.5) || machine == 1;
    let drive = 10.0f64.powf(f64::from(knob_to_smp_drive_db(sm[P_SMP_DRIVE])) / 20.0);
    let levels = 2.0f64.powf(f64::from(knob_to_smp_bits(sm[P_SMP_BITS])) - 1.0);

    let mut inp = x;
    {
        let mut f = x;
        for s in &mut c.smp_aa {
            f = s.run(f);
        }
        if filt {
            inp = f;
        }
    }

    c.smp_phase += fl.smp_step;
    if c.smp_phase >= 1.0 {
        c.smp_phase -= 1.0;
        let frac = c.smp_phase / fl.smp_step; // how far past the true instant we are
        let at = inp + frac * (c.smp_prev - inp); // the input AT the sampling instant
        let v = clamp_t(at * drive, -1.0, 1.0);
        c.smp_held = (v * levels).round() / levels / drive;
    }
    c.smp_prev = inp;

    let out = c.smp_held;
    let mut f = out;
    for s in &mut c.smp_rc {
        f = s.run(f);
    }
    if filt { f } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_maps_match_the_cpp() {
        assert_eq!(DEFAULTS.len(), NUM_PARAMS);
        assert_eq!(knob_to_drive_db(0.25), 0.0);
        assert_eq!(knob_to_out_db(0.5), 0.0);
        assert!((knob_to_smp_rate_hz(1.0) - 48000.0).abs() < 0.1);
        assert!((dial(0.5, &BRI_HM_DIAL) - 3000.0).abs() < 1.0);
        assert_eq!(stage_for(C_PASSIVE), vints::ST_NONE);
        assert_eq!(stage_for(C_CONSOLE), vints::ST_CLASS_A);
        assert!(meq_makeup() > 1.0);
        assert!(physical(1.0) && !physical(f64::NAN) && !physical(f64::INFINITY) && !physical(2000.0));
        assert!((meq_centre(&MEQ_LOW[1]) - 300.0).abs() < 30.0);
        assert_eq!(knob_to_pultec(0.5), 5.0);
    }

    #[test]
    fn the_passive_network_at_zero_loses_its_makeup() {
        let v = DEFAULTS;
        let db = response_db(C_PASSIVE, &v, 1000.0, 192_000.0);
        assert!(db.abs() < 0.05, "{db}");
        let db = response_db(C_PASSIVE_MID, &v, 1000.0, 192_000.0);
        assert!(db.abs() < 0.05, "{db}");
        for c in [C_CONSOLE, C_PROPORTIONAL, C_BRITISH] {
            let db = response_db(c, &v, 1000.0, 192_000.0);
            assert!(db.abs() < 0.2, "circuit {c}: {db}");
        }
        // README: both cards' HMF at +12 dB read +11.98 / +11.96 at the dial.
        for black in [0.0, 1.0] {
            let mut v = DEFAULTS;
            v[P_BRI_BLACK] = black;
            v[P_BRI_HM_GAIN] = 0.9; // +12 dB
            let db = response_db(C_BRITISH, &v, f64::from(dial(0.5, &BRI_HM_DIAL)), 192_000.0);
            assert!((db - 12.0).abs() < 0.1, "black {black}: {db}");
        }
    }
}
