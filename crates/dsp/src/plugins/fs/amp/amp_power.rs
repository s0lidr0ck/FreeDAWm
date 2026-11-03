//! `AmpPower.h`: phase inverter, power tubes, output transformer, feedback, sag. A long-tail pair
//! (or the Orange's cathodyne) driving a push-pull pair through coupling caps that grid current
//! charges ("blocking"), Koren's pentode law against the reflected load, the supply's sag, the
//! transformer's band and the speaker's impedance, and the negative feedback (with the JTM45's
//! Presence as a real network in that loop).
//!
//! Translated line by line. Rust additions: `prepare`'s exact solves of the phase inverter are
//! shared between instances (see [`super::amp_tube::shared_table`]); [`PowerAmp::reset_fresh`]
//! puts the dynamic state back where `prepare` leaves it without re-solving or allocating.

use super::amp_network::Network;
use super::amp_tube::{K12AT7, K12AX7, TriodeParams, cclamp, cmax, shared_table, triode_ip};
use std::sync::Arc;

// ------------------------------------------------------------------ pentodes
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PentodeParams {
    pub mu: f64,
    pub ex: f64,
    pub kg1: f64,
    pub kg2: f64,
    pub kp: f64,
    pub kvb: f64,
}

/// 6L6GC: Koren's published fit.
pub const K6L6GC: PentodeParams = PentodeParams { mu: 8.7, ex: 1.35, kg1: 1460.0, kg2: 4500.0, kp: 48.0, kvb: 12.0 };
/// KT66: the 6L6GC's fit with a slightly lower perveance (kg1 up ~10 %), an approximation.
pub const KKT66: PentodeParams = PentodeParams { mu: 8.7, ex: 1.35, kg1: 1600.0, kg2: 4500.0, kp: 48.0, kvb: 12.0 };
/// 6V6GT: Koren's form fitted to the RCA datasheet's class A1 point.
pub const K6V6GT: PentodeParams = PentodeParams { mu: 9.8, ex: 1.35, kg1: 2277.0, kg2: 7097.0, kp: 45.0, kvb: 12.0 };
/// EL84: Koren's form fitted to Philips' class A point.
pub const KEL84: PentodeParams = PentodeParams { mu: 19.0, ex: 1.35, kg1: 839.0, kg2: 1977.0, kp: 60.0, kvb: 12.0 };
/// EL34: Koren's published fit with kg2 refitted to the datasheet.
pub const KEL34: PentodeParams = PentodeParams { mu: 11.0, ex: 1.35, kg1: 650.0, kg2: 1353.0, kp: 60.0, kvb: 24.0 };

#[inline]
pub fn pentode_ip(t: &PentodeParams, vg1k: f64, vg2k: f64, vpk: f64) -> f64 {
    if vg2k <= 0.0 || vpk <= 0.0 {
        return 0.0;
    }
    let inner = t.kp * (1.0 / t.mu + vg1k / vg2k);
    let soft = if inner > 30.0 { inner } else { inner.exp().ln_1p() };
    let e1 = vg2k / t.kp * soft;
    if e1 <= 0.0 {
        return 0.0;
    }
    2.0 * e1.powf(t.ex) / t.kg1 * (vpk / t.kvb).atan()
}

#[inline]
pub fn pentode_ig2(t: &PentodeParams, vg1k: f64, vg2k: f64) -> f64 {
    let x = vg2k / t.mu + vg1k;
    if x > 0.0 { x.powf(t.ex) / t.kg2 } else { 0.0 }
}

// ------------------------------------------------------------------ the amp's power side
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerParts {
    // phase inverter
    pub pi_tube: TriodeParams,
    pub pi_supply: f64,
    pub pi_rp_a: f64,
    pub pi_rp_b: f64,
    pub pi_tail_current: f64,
    // coupling into the power grids
    pub c_couple: f64,
    pub r_grid_leak: f64,
    pub r_grid_stop: f64,
    /// The power grids' DC volts.
    pub bias: f64,
    // output stage
    pub tube: PentodeParams,
    /// 2 for four tubes, 1 for two.
    pub pairs: i32,
    pub ultralinear: bool,
    /// Fraction of the primary the screens see (UL).
    pub ul_tap: f64,
    /// Pentode: screens' DC supply (V) below B+.
    pub screen_supply: f64,
    /// Plate-to-plate load, per pair (ohms).
    pub raa: f64,
    /// The tap the speakers are on.
    pub r_speaker: f64,
    // supply
    pub b_plus_idle: f64,
    pub r_source: f64,
    pub c_reservoir: f64,
    // transformer and speaker
    pub ot_low_hz: f64,
    pub ot_high_hz: f64,
    pub spk_res_hz: f64,
    pub spk_res_q: f64,
    pub spk_res_gain: f64,
    pub spk_rise_hz: f64,
    // negative feedback, and (Marshall) presence
    pub nfb_beta: f64,
    pub has_presence: bool,
    pub presence_cap: f64,
    pub presence_pot: f64,
    pub fb_resistor: f64,
    /// How far the voltage at the speaker's terminals rises with its impedance.
    pub spk_rise_gain: f64,
    /// Resistance feeding each pentode's screen; 0 = screens held at their supply.
    pub r_screen: f64,
    /// The fraction of the tubes' power that reaches the speaker tap.
    pub ot_efficiency: f64,
    /// Self-bias: one bypassed resistor shared by all the power tubes' cathodes; 0 = fixed bias.
    pub r_cathode: f64,
    pub c_cathode: f64,
    /// The Vox Cut: a pot in series with a capacitor across the grid lines; 0 = none.
    pub cut_cap: f64,
    pub cut_pot: f64,
    /// 0 = long-tail pair, 1 = cathodyne.
    pub pi_type: i32,
    pub pi_rk_total: f64,
    pub pi_rk_bias: f64,
}

/// The aggregate's trailing defaults (`PowerParts`' member initialisers).
const TAIL: PowerParts = PowerParts {
    pi_tube: K12AX7,
    pi_supply: 0.0,
    pi_rp_a: 0.0,
    pi_rp_b: 0.0,
    pi_tail_current: 0.0,
    c_couple: 0.0,
    r_grid_leak: 0.0,
    r_grid_stop: 0.0,
    bias: 0.0,
    tube: K6L6GC,
    pairs: 0,
    ultralinear: false,
    ul_tap: 0.0,
    screen_supply: 0.0,
    raa: 0.0,
    r_speaker: 0.0,
    b_plus_idle: 0.0,
    r_source: 0.0,
    c_reservoir: 0.0,
    ot_low_hz: 0.0,
    ot_high_hz: 0.0,
    spk_res_hz: 0.0,
    spk_res_q: 0.0,
    spk_res_gain: 0.0,
    spk_rise_hz: 0.0,
    nfb_beta: 0.0,
    has_presence: false,
    presence_cap: 0.0,
    presence_pot: 0.0,
    fb_resistor: 0.0,
    spk_rise_gain: 2.0,
    r_screen: 0.0,
    ot_efficiency: 1.0,
    r_cathode: 0.0,
    c_cathode: 0.0,
    cut_cap: 0.0,
    cut_pot: 0.0,
    pi_type: 0,
    pi_rk_total: 0.0,
    pi_rk_bias: 0.0,
};

/// 1977 Twin Reverb 135 W (Fender 013694).
pub const K_TWIN_POWER: PowerParts = PowerParts {
    pi_tube: K12AT7,
    pi_supply: 400.0,
    pi_rp_a: 47e3,
    pi_rp_b: 47e3,
    pi_tail_current: 5.0e-3,
    c_couple: 0.1e-6,
    r_grid_leak: 47e3,
    r_grid_stop: 1500.0,
    bias: -60.0,
    tube: K6L6GC,
    pairs: 2,
    ultralinear: true,
    ul_tap: 0.43,
    screen_supply: 0.0,
    raa: 4000.0,
    r_speaker: 4.0,
    b_plus_idle: 500.0,
    r_source: 25.0,
    c_reservoir: 110e-6,
    ot_low_hz: 28.0,
    ot_high_hz: 16000.0,
    spk_res_hz: 85.0,
    spk_res_q: 2.2,
    spk_res_gain: 1.35,
    spk_rise_hz: 1500.0,
    nfb_beta: 47.0 / (820.0 + 47.0),
    has_presence: false,
    presence_cap: 0.0,
    presence_pot: 0.0,
    fb_resistor: 820.0,
    spk_rise_gain: 1.26, // +2 dB of speaker rise
    ..TAIL
};

/// Marshall JTM45 trem, 1962.
pub const K_MARSHALL_POWER: PowerParts = PowerParts {
    pi_tube: K12AX7,
    pi_supply: 400.0,
    pi_rp_a: 82e3,
    pi_rp_b: 100e3,
    pi_tail_current: 3.8e-3,
    c_couple: 0.1e-6,
    r_grid_leak: 220e3,
    r_grid_stop: 1500.0,
    bias: -38.0,
    tube: KKT66,
    pairs: 1,
    ultralinear: false,
    ul_tap: 0.0,
    screen_supply: 10.0,
    raa: 8000.0,
    r_speaker: 16.0,
    b_plus_idle: 450.0,
    r_source: 350.0,
    c_reservoir: 32e-6,
    ot_low_hz: 40.0,
    ot_high_hz: 14000.0,
    spk_res_hz: 75.0,
    spk_res_q: 2.5,
    spk_res_gain: 1.8,
    spk_rise_hz: 1200.0,
    nfb_beta: 0.0,
    has_presence: true,
    presence_cap: 0.1e-6,
    presence_pot: 5000.0,
    fb_resistor: 27e3,
    spk_rise_gain: 1.7, // +4.6 dB of speaker rise
    ..TAIL
};

/// 1965 Fender Deluxe Reverb (AB763).
pub const K_DELUXE_POWER: PowerParts = PowerParts {
    pi_tube: K12AT7,
    pi_supply: 400.0,
    pi_rp_a: 82e3,
    pi_rp_b: 100e3,
    pi_tail_current: 2.8e-3,
    c_couple: 0.1e-6,
    r_grid_leak: 220e3,
    r_grid_stop: 1500.0,
    bias: -34.0,
    tube: K6V6GT,
    pairs: 1,
    ultralinear: false,
    ul_tap: 0.0,
    screen_supply: 10.0,
    raa: 6600.0,
    r_speaker: 8.0,
    b_plus_idle: 420.0,
    r_source: 330.0,
    c_reservoir: 16e-6,
    ot_low_hz: 35.0,
    ot_high_hz: 13000.0,
    spk_res_hz: 90.0,
    spk_res_q: 2.2,
    spk_res_gain: 1.35,
    spk_rise_hz: 1500.0,
    nfb_beta: 47.0 / (820.0 + 47.0),
    has_presence: false,
    presence_cap: 0.0,
    presence_pot: 0.0,
    fb_resistor: 820.0,
    spk_rise_gain: 1.26,
    r_screen: 1000.0, // 470 ohm screen resistor + its share of the supply
    ..TAIL
};

/// 1965 Fender Super Reverb (AB763).
pub const K_SUPER_POWER: PowerParts = PowerParts {
    pi_tube: K12AT7,
    pi_supply: 400.0,
    pi_rp_a: 82e3,
    pi_rp_b: 100e3,
    pi_tail_current: 2.8e-3,
    c_couple: 0.1e-6,
    r_grid_leak: 220e3,
    r_grid_stop: 1500.0,
    bias: -50.0,
    tube: K6L6GC,
    pairs: 1,
    ultralinear: false,
    ul_tap: 0.0,
    screen_supply: 10.0,
    raa: 4200.0,
    r_speaker: 2.0,
    b_plus_idle: 460.0,
    r_source: 250.0,
    c_reservoir: 35e-6,
    ot_low_hz: 30.0,
    ot_high_hz: 14000.0,
    spk_res_hz: 100.0,
    spk_res_q: 2.0,
    spk_res_gain: 1.35,
    spk_rise_hz: 1800.0,
    nfb_beta: 47.0 / (820.0 + 47.0),
    has_presence: false,
    presence_cap: 0.0,
    presence_pot: 0.0,
    fb_resistor: 820.0,
    spk_rise_gain: 1.26,
    r_screen: 1000.0,
    ..TAIL
};

/// 1965 Fender Princeton Reverb (AB1270).
pub const K_PRINCETON_POWER: PowerParts = PowerParts {
    pi_tube: K12AT7,
    pi_supply: 400.0,
    pi_rp_a: 82e3,
    pi_rp_b: 100e3,
    pi_tail_current: 2.5e-3,
    c_couple: 0.1e-6,
    r_grid_leak: 220e3,
    r_grid_stop: 1500.0,
    bias: -35.0,
    tube: K6V6GT,
    pairs: 1,
    ultralinear: false,
    ul_tap: 0.0,
    screen_supply: 10.0,
    raa: 8000.0,
    r_speaker: 8.0,
    b_plus_idle: 410.0,
    r_source: 550.0,
    c_reservoir: 16e-6,
    ot_low_hz: 45.0,
    ot_high_hz: 12000.0,
    spk_res_hz: 110.0,
    spk_res_q: 2.0,
    spk_res_gain: 1.35,
    spk_rise_hz: 1800.0,
    nfb_beta: 47.0 / (820.0 + 47.0),
    has_presence: false,
    presence_cap: 0.0,
    presence_pot: 0.0,
    fb_resistor: 820.0,
    spk_rise_gain: 1.26,
    r_screen: 1000.0,
    ot_efficiency: 0.8, // the small output transformer's losses
    ..TAIL
};

/// 1964 Vox AC30/6.
pub const K_AC30_POWER: PowerParts = PowerParts {
    pi_tube: K12AX7,
    pi_supply: 280.0,
    pi_rp_a: 100e3,
    pi_rp_b: 100e3,
    pi_tail_current: 1.0e-3,
    c_couple: 0.15e-6,
    r_grid_leak: 220e3,
    r_grid_stop: 1500.0,
    bias: 0.0,
    tube: KEL84,
    pairs: 2,
    ultralinear: false,
    ul_tap: 0.0,
    screen_supply: 2.0,
    raa: 8000.0,
    r_speaker: 8.0,
    b_plus_idle: 320.0,
    r_source: 200.0,
    c_reservoir: 16e-6,
    ot_low_hz: 40.0,
    ot_high_hz: 12000.0,
    spk_res_hz: 80.0,
    spk_res_q: 2.5,
    spk_res_gain: 1.9,
    spk_rise_hz: 1200.0,
    nfb_beta: 0.0,
    has_presence: false,
    presence_cap: 0.0,
    presence_pot: 0.0,
    fb_resistor: 0.0,
    spk_rise_gain: 2.0, // no feedback: the speaker's full rise at the terminals
    r_screen: 100.0,    // screen stoppers
    ot_efficiency: 1.0,
    r_cathode: 50.0, // the shared cathode resistor and its bypass
    c_cathode: 250e-6,
    cut_cap: 0.0047e-6, // the Cut
    cut_pot: 250e3,
    ..TAIL
};

/// Orange OR120 Graphic, early 1970s.
pub const K_OR120_POWER: PowerParts = PowerParts {
    pi_tube: K12AX7,
    pi_supply: 450.0,
    pi_rp_a: 100e3,
    pi_rp_b: 0.0,
    pi_tail_current: 0.0,
    c_couple: 0.068e-6,
    r_grid_leak: 220e3,
    r_grid_stop: 2400.0,
    bias: -55.0,
    tube: KEL34,
    pairs: 2,
    ultralinear: false,
    ul_tap: 0.0,
    screen_supply: 4.0,
    raa: 7200.0,
    r_speaker: 16.0,
    b_plus_idle: 545.0,
    r_source: 40.0,
    c_reservoir: 50e-6,
    ot_low_hz: 25.0,
    ot_high_hz: 18000.0,
    spk_res_hz: 75.0,
    spk_res_q: 2.5,
    spk_res_gain: 1.6,
    spk_rise_hz: 1200.0,
    nfb_beta: 0.0,
    has_presence: false,
    presence_cap: 0.0,
    presence_pot: 0.0,
    fb_resistor: 0.0,
    spk_rise_gain: 1.6,
    r_screen: 1000.0,
    ot_efficiency: 1.0,
    r_cathode: 0.0,
    c_cathode: 0.0,
    cut_cap: 0.0,
    cut_pot: 0.0,
    pi_type: 1,
    pi_rk_total: 101.5e3,
    pi_rk_bias: 1.5e3,
};

// -------------------------------------------------------------- linear filters
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
    pub z1: f64,
    pub z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }
}

impl Biquad {
    #[inline]
    pub fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
    pub fn clear(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

fn highpass(hz: f64, q: f64, fs: f64) -> Biquad {
    let w = 2.0 * 3.14159265358979 * hz / fs;
    let (c, a) = (w.cos(), w.sin() / (2.0 * q));
    let n = 1.0 + a;
    Biquad { b0: (1.0 + c) / 2.0 / n, b1: -(1.0 + c) / n, b2: (1.0 + c) / 2.0 / n, a1: -2.0 * c / n, a2: (1.0 - a) / n, z1: 0.0, z2: 0.0 }
}
fn lowpass(hz: f64, q: f64, fs: f64) -> Biquad {
    let w = 2.0 * 3.14159265358979 * hz / fs;
    let (c, a) = (w.cos(), w.sin() / (2.0 * q));
    let n = 1.0 + a;
    Biquad { b0: (1.0 - c) / 2.0 / n, b1: (1.0 - c) / n, b2: (1.0 - c) / 2.0 / n, a1: -2.0 * c / n, a2: (1.0 - a) / n, z1: 0.0, z2: 0.0 }
}
fn peak(hz: f64, q: f64, gain: f64, fs: f64) -> Biquad {
    let a_ = gain.sqrt();
    let w = 2.0 * 3.14159265358979 * hz / fs;
    let (c, a) = (w.cos(), w.sin() / (2.0 * q));
    let n = 1.0 + a / a_;
    Biquad { b0: (1.0 + a * a_) / n, b1: -2.0 * c / n, b2: (1.0 - a * a_) / n, a1: -2.0 * c / n, a2: (1.0 - a / a_) / n, z1: 0.0, z2: 0.0 }
}
fn high_shelf1(hz: f64, gain_high: f64, fs: f64) -> Biquad {
    // first-order: 1 at DC rising to gainHigh, the speaker's voice-coil inductance
    let k = (3.14159265358979 * hz / fs).tan();
    let n = 1.0 + k;
    Biquad { b0: (gain_high + k) / n, b1: (k - gain_high) / n, b2: 0.0, a1: (k - 1.0) / n, a2: 0.0, z1: 0.0, z2: 0.0 }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OnePole {
    b0: f64,
    b1: f64,
    a1: f64,
    x1: f64,
    y1: f64,
}

impl Default for OnePole {
    fn default() -> Self {
        OnePole { b0: 1.0, b1: 0.0, a1: 0.0, x1: 0.0, y1: 0.0 }
    }
}

impl OnePole {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 - self.a1 * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

const PI_N: usize = 2048;
const PI_LO: f64 = -80.0;
const PI_HI: f64 = 80.0;

/// What `prepare` solves for the phase inverter (the parts alone decide it).
#[derive(Debug, Clone, Default)]
struct PiSolve {
    table: Vec<f64>,
    idle_a: f64,
    idle_b: f64,
    cathodyne_grid_idle: f64,
}

/// The power side: inverter, coupling, push-pull pair, supply, transformer, speaker, feedback.
#[derive(Debug, Clone)]
pub struct PowerAmp {
    /// A resistive dummy load in place of the speaker.
    pub dummy_load: bool,
    /// Per-tube idle plate current to bias to (A); 0 = use the schematic's bias as is.
    pub idle_target: f64,
    /// The feedback divider's bottom resistor on the Marshall (~6 dB of feedback).
    pub marshall_fb_bottom: f64,

    parts: PowerParts,
    fs: f64,
    dt: f64,
    b_plus: f64,
    v_open: f64,
    i_idle: f64,
    vc_a: f64,
    vc_b: f64,
    fb: f64,
    last_out: f64,
    d_prev: f64,
    load_volts: f64,
    ig2_prev_a: f64,
    ig2_prev_b: f64,
    bias_offset: f64,
    vk_power: f64,
    cathodyne_grid_idle: f64,
    cut_a: OnePole,
    cut_b: OnePole,
    pi_idle_a: f64,
    pi_idle_b: f64,
    pi_idle_set: bool,
    idle_plate_a: f64,
    idle_plate_b: f64,
    ot_hp: Biquad,
    ot_lp: Biquad,
    spk_res: Biquad,
    spk_rise: Biquad,
    pi_table: Arc<PiSolve>,
    fb_net: Network,
    fb_node: usize,
    fb_src: usize,
    r_presence: usize,
    /// (Rust) The solver state `prepare` leaves (its bias searches run the output stage, which
    /// keeps its Newton seed and screen currents, and the self-bias cathode volts it found), for
    /// [`PowerAmp::reset_fresh`].
    prepared: [f64; 4],
}

impl Default for PowerAmp {
    fn default() -> Self {
        PowerAmp {
            dummy_load: false,
            idle_target: 0.035,
            marshall_fb_bottom: 500.0,
            parts: K_TWIN_POWER,
            fs: 192_000.0,
            dt: 1.0 / 192_000.0,
            b_plus: 450.0,
            v_open: 450.0,
            i_idle: 0.0,
            vc_a: 0.0,
            vc_b: 0.0,
            fb: 0.0,
            last_out: 0.0,
            d_prev: 0.0,
            load_volts: 0.0,
            ig2_prev_a: 0.0,
            ig2_prev_b: 0.0,
            bias_offset: 0.0,
            vk_power: 0.0,
            cathodyne_grid_idle: 0.0,
            cut_a: OnePole::default(),
            cut_b: OnePole::default(),
            pi_idle_a: 0.0,
            pi_idle_b: 0.0,
            pi_idle_set: false,
            idle_plate_a: 0.0,
            idle_plate_b: 0.0,
            ot_hp: Biquad::default(),
            ot_lp: Biquad::default(),
            spk_res: Biquad::default(),
            spk_rise: Biquad::default(),
            pi_table: Arc::new(PiSolve::default()),
            fb_net: Network::new(1),
            fb_node: 0,
            fb_src: 0,
            r_presence: 0,
            prepared: [0.0; 4],
        }
    }
}

impl PowerAmp {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        out.push(Arc::as_ptr(&self.pi_table) as usize);
        self.fb_net.heap_blocks(out);
    }

    pub fn prepare(&mut self, p: &PowerParts, sample_rate: f64) {
        self.parts = *p;
        self.fs = sample_rate;
        self.dt = 1.0 / self.fs;
        self.b_plus = p.b_plus_idle;
        self.vc_a = 0.0;
        self.vc_b = 0.0;

        // the inverter's idle: settle its plates with no signal, then tabulate it. (`useTables`,
        // the C++ tests' switch for exact per-sample solves, is always on in the plugin.)
        let mut key: Vec<u64> = p.pi_tube.key().to_vec();
        key.extend([p.pi_supply, p.pi_rp_a, p.pi_rp_b, p.pi_tail_current, p.pi_rk_total, p.pi_rk_bias].map(f64::to_bits));
        key.extend([p.pi_type as u64, 0x5049]);
        let parts = self.parts;
        self.pi_table = shared_table(&key, || {
            let mut s = PiSolver { parts, plate_a: parts.pi_supply * 0.6, plate_b: parts.pi_supply * 0.6, ..PiSolver::default() };
            for _ in 0..4 {
                s.exact(0.0, 0.0);
                s.idle_set = false;
            }
            s.exact(0.0, 0.0); // this one records the idle
            let mut table = vec![0.0; PI_N * 2];
            for i in 0..PI_N {
                let d = PI_LO + (PI_HI - PI_LO) * i as f64 / (PI_N - 1) as f64;
                let (a, b) = s.exact(0.5 * d, -0.5 * d);
                table[i * 2] = a;
                table[i * 2 + 1] = b;
            }
            PiSolve { table, idle_a: s.idle_a, idle_b: s.idle_b, cathodyne_grid_idle: s.cathodyne_grid_idle }
        });
        self.pi_idle_a = self.pi_table.idle_a;
        self.pi_idle_b = self.pi_table.idle_b;
        self.cathodyne_grid_idle = self.pi_table.cathodyne_grid_idle;
        self.pi_idle_set = true;
        self.d_prev = 0.0;
        self.ig2_prev_a = 0.0;
        self.ig2_prev_b = 0.0;

        // Bias it as a technician would.
        self.vk_power = 0.0;
        if p.r_cathode > 0.0 {
            // self-bias: the grids sit at 0 V and the cathode finds its own level
            self.parts.bias = 0.0;
            let (mut lo, mut hi) = (0.0, 60.0);
            for _ in 0..50 {
                self.vk_power = 0.5 * (lo + hi);
                let (itot, _ia, _ib) = self.supply_current(0.0, 0.0, p.b_plus_idle);
                if itot * p.r_cathode > self.vk_power {
                    lo = self.vk_power;
                } else {
                    hi = self.vk_power;
                }
            }
            self.vk_power = 0.5 * (lo + hi);
        } else if self.idle_target > 0.0 {
            let (mut lo, mut hi) = (-120.0, -5.0);
            for _ in 0..50 {
                self.parts.bias = 0.5 * (lo + hi);
                let (_itot, ia, _ib) = self.supply_current(0.0, 0.0, p.b_plus_idle);
                if ia > self.idle_target {
                    hi = self.parts.bias;
                } else {
                    lo = self.parts.bias;
                }
            }
        }
        self.fb = 0.0;
        self.last_out = 0.0;
        self.filters_prepare();
        self.cut_a = OnePole::default();
        self.cut_b = OnePole::default();
        if p.cut_cap > 0.0 {
            self.set_cut(0.0); // the network is always there
        }

        // the open-circuit supply voltage that gives the schematic's B+ at the idle current
        let (i_idle, ia, ib) = self.supply_current(0.0, 0.0, p.b_plus_idle);
        self.i_idle = i_idle;
        self.idle_plate_a = ia;
        self.idle_plate_b = ib;
        self.v_open = p.b_plus_idle + self.i_idle * p.r_source;

        // Marshall's presence / feedback network
        if p.has_presence {
            self.fb_net = Network::new(1);
            let f = self.fb_net.add_node();
            let pc = self.fb_net.add_node();
            self.fb_node = f;
            self.fb_src = self.fb_net.add_source(f, p.fb_resistor);
            self.fb_net.add_resistor(f, Network::GROUND, self.marshall_fb_bottom);
            self.fb_net.add_capacitor(f, pc, p.presence_cap);
            self.r_presence = self.fb_net.add_resistor(pc, Network::GROUND, p.presence_pot);
            self.fb_net.prepare(self.fs);
        }
        self.prepared = [self.d_prev, self.ig2_prev_a, self.ig2_prev_b, self.vk_power];
    }

    /// (Rust) The dynamic state as `prepare` leaves it, in place: what `prepareToPlay` (or the
    /// wrapper's oversampling re-prepare) gives a live instance. The solved bias, idle and tables
    /// are the parts' own and stay. No allocation.
    pub fn reset_fresh(&mut self) {
        let p = self.parts;
        self.b_plus = p.b_plus_idle;
        self.vc_a = 0.0;
        self.vc_b = 0.0;
        [self.d_prev, self.ig2_prev_a, self.ig2_prev_b, self.vk_power] = self.prepared;
        self.fb = 0.0;
        self.last_out = 0.0;
        for b in [&mut self.ot_hp, &mut self.ot_lp, &mut self.spk_res, &mut self.spk_rise] {
            b.clear();
        }
        self.cut_a.x1 = 0.0;
        self.cut_a.y1 = 0.0;
        self.cut_b.x1 = 0.0;
        self.cut_b.y1 = 0.0;
        if p.has_presence {
            self.fb_net.reset();
            self.fb_net.set_input(self.fb_src, 0.0);
        }
    }

    /// Presence, 0..1 (Marshall). Full presence = pot at 0 ohms.
    pub fn set_presence(&mut self, v: f64) {
        if !self.parts.has_presence {
            return;
        }
        self.fb_net.set_resistor(self.r_presence, cmax(10.0, self.parts.presence_pot * (1.0 - cclamp(v, 0.0, 1.0))));
    }

    /// The inverter's input (volts at its grid) in, the speaker voltage out.
    pub fn process(&mut self, vin: f64) -> f64 {
        let p = self.parts;

        // ---- the phase inverter: the input on one grid, the feedback on the other
        let (mut va, mut vb) = self.phase_inverter(vin, self.fb);
        if p.cut_cap > 0.0 {
            va = self.cut_a.run(va);
            vb = self.cut_b.run(vb);
        }

        // ---- coupling caps into the power grids, with grid conduction charging them
        let mut vc_a = self.vc_a;
        let g_a = self.grid_side(va, &mut vc_a);
        self.vc_a = vc_a;
        let mut vc_b = self.vc_b;
        let g_b = self.grid_side(vb, &mut vc_b);
        self.vc_b = vc_b;

        // ---- the output stage against the transformer, at the present B+
        let (vpp, i_a, i_b, ig2) = self.output_stage(g_a, g_b, self.b_plus);

        // ---- self-bias
        if p.r_cathode > 0.0 {
            self.vk_power += ((i_a + i_b + ig2) * f64::from(p.pairs) - self.vk_power / p.r_cathode) * self.dt / p.c_cathode;
        }

        // ---- sag
        let i_load = (i_a + i_b + ig2) * f64::from(p.pairs);
        let i_charge = cmax(0.0, (self.v_open - self.b_plus) / p.r_source); // a rectifier only charges
        self.b_plus += (i_charge - i_load) * self.dt / p.c_reservoir;
        self.b_plus = cclamp(self.b_plus, 0.4 * self.v_open, self.v_open);

        // ---- transformer to the speaker tap
        let mut v = vpp * (p.r_speaker / p.raa * p.ot_efficiency).sqrt();
        v = self.ot_hp.run(v);
        v = self.ot_lp.run(v);
        self.load_volts = v;
        if !self.dummy_load {
            v = self.spk_res.run(v);
            v = self.spk_rise.run(v);
        }

        // ---- feedback for the next sample
        if p.has_presence {
            self.fb_net.set_input(self.fb_src, v);
            self.fb_net.step();
            self.fb = self.fb_net.voltage(self.fb_node);
        } else {
            self.fb = p.nfb_beta * v;
        }

        self.last_out = v;
        v
    }

    pub fn supply_now(&self) -> f64 {
        self.b_plus
    }

    /// The power tubes' cathode volts (self-bias only; 0 with fixed bias).
    pub fn cathode_now(&self) -> f64 {
        self.vk_power
    }

    /// The Vox Cut, 0..1: a shelf, H = (1 + s C R) / (1 + s C (R + 2 Rs)).
    pub fn set_cut(&mut self, v: f64) {
        let p = self.parts;
        if p.cut_cap <= 0.0 {
            return;
        }
        let r = cmax(10.0, p.cut_pot * Network::audio_taper(1.0 - cclamp(v, 0.0, 1.0)));
        let rs = 1.0 / (1.0 / p.pi_rp_a + 1.0 / 62e3 + 1.0 / p.r_grid_leak);
        let tz = p.cut_cap * r;
        let tp = p.cut_cap * (r + 2.0 * rs);
        let k = 2.0 * self.fs; // bilinear
        let (b0, b1, a0, a1) = (1.0 + k * tz, 1.0 - k * tz, 1.0 + k * tp, 1.0 - k * tp);
        for f in [&mut self.cut_a, &mut self.cut_b] {
            f.b0 = b0 / a0;
            f.b1 = b1 / a0;
            f.a1 = a1 / a0;
        }
    }

    /// (Rust) The Cut's response at `hz`, as `set_cut` builds it before the bilinear map:
    /// `(1 + s tz) / (1 + s tp)` with the knob at `v` (the tone curve's view of the Normal channel).
    /// 1 where the amp has no Cut.
    pub fn cut_response(p: &PowerParts, v: f64, hz: f64) -> rustfft::num_complex::Complex<f64> {
        type C = rustfft::num_complex::Complex<f64>;
        if p.cut_cap <= 0.0 {
            return C::new(1.0, 0.0);
        }
        let r = cmax(10.0, p.cut_pot * Network::audio_taper(1.0 - cclamp(v, 0.0, 1.0)));
        let rs = 1.0 / (1.0 / p.pi_rp_a + 1.0 / 62e3 + 1.0 / p.r_grid_leak);
        let s = C::new(0.0, 2.0 * std::f64::consts::PI * hz);
        (1.0 + s * (p.cut_cap * r)) / (1.0 + s * (p.cut_cap * (r + 2.0 * rs)))
    }

    /// Volts added to the power grids' bias, this sample (the Princeton's tremolo).
    pub fn set_bias_offset(&mut self, v: f64) {
        self.bias_offset = v;
    }

    /// The last sample's speaker-tap voltage into a resistor of the rated impedance.
    pub fn load_volts_now(&self) -> f64 {
        self.load_volts
    }

    pub fn idle_current_per_tube(&self) -> f64 {
        self.idle_plate_a
    }

    pub fn bias_volts(&self) -> f64 {
        self.parts.bias
    }

    pub fn parts(&self) -> &PowerParts {
        &self.parts
    }

    // -------------------------------------------------------------- phase inverter
    fn phase_inverter(&self, vg1: f64, vg2: f64) -> (f64, f64) {
        let t = &self.pi_table.table;
        let x = cclamp((vg1 - vg2 - PI_LO) / (PI_HI - PI_LO) * (PI_N - 1) as f64, 0.0, PI_N as f64 - 1.001);
        let i = x as usize;
        let f = x - i as f64;
        let get = |k: usize| t.get(k).copied().unwrap_or(0.0);
        let out_a = get(i * 2) + (get(i * 2 + 2) - get(i * 2)) * f;
        let out_b = get(i * 2 + 1) + (get(i * 2 + 3) - get(i * 2 + 1)) * f;
        (out_a, out_b)
    }

    // -------------------------------------------------------------- coupling + grid current
    /// One side's power grid. `vc` is the coupling cap's charge. Returns the grid volts.
    #[inline]
    fn grid_side(&self, vpi: f64, vc: &mut f64) -> f64 {
        let p = &self.parts;
        let mut vg = p.bias + self.bias_offset + (vpi - *vc); // what the grid would be with no grid current
        let mut ig = 0.0;
        let vgk = vg - self.vk_power; // (the cathode is at 0 V with fixed bias)
        if vgk > 0.0 {
            // the grid conducts (~1k): the stopper and the grid form a divider
            let rg = 1000.0;
            let vgk_clamped = vgk * rg / (rg + p.r_grid_stop);
            ig = (vgk - vgk_clamped) / p.r_grid_stop;
            vg = self.vk_power + vgk_clamped;
        }
        // the cap carries the grid-leak current and any grid current
        let i_cap = (vg - p.bias) / p.r_grid_leak + ig;
        *vc += i_cap * self.dt / p.c_couple;
        vg
    }

    // -------------------------------------------------------------- the push-pull pair
    /// Returns (plate-to-plate volts, iA, iB, ig2 total).
    fn output_stage(&mut self, g_a: f64, g_b: f64, bp: f64) -> (f64, f64, f64, f64) {
        let p = self.parts;
        let vk = self.vk_power;
        let (ig2a, ig2b) = (self.ig2_prev_a, self.ig2_prev_b);
        // unknown: d = iA - iB. Plates: vpA = B+ - d raa/4, vpB = B+ + d raa/4.
        let screens = |vp_a: f64, vp_b: f64| -> (f64, f64) {
            if p.ultralinear {
                (bp + p.ul_tap * (vp_a - bp), bp + p.ul_tap * (vp_b - bp))
            } else {
                // last sample's screen current through the screen resistor
                (bp - p.screen_supply - ig2a * p.r_screen, bp - p.screen_supply - ig2b * p.r_screen)
            }
        };
        let d_max = 4.0 * bp / p.raa;
        let resid = |d: f64| -> f64 {
            let vp_a = bp - d * p.raa / 4.0;
            let vp_b = bp + d * p.raa / 4.0;
            let (s_a, s_b) = screens(vp_a, vp_b);
            pentode_ip(&p.tube, g_a - vk, s_a - vk, vp_a - vk) - pentode_ip(&p.tube, g_b - vk, s_b - vk, vp_b - vk) - d
        };
        // Newton from last sample's answer; bisection as the safety net.
        let mut d = cclamp(self.d_prev, -d_max, d_max);
        let mut ok = false;
        for _ in 0..8 {
            let f = resid(d);
            if f.abs() < 1.0e-7 {
                ok = true;
                break;
            }
            let h = 1.0e-6;
            let df = (resid(d + h) - f) / h;
            if df >= 0.0 {
                break;
            }
            let step = -f / df;
            d = cclamp(d + step, -d_max, d_max);
            if step.abs() < 1.0e-7 {
                ok = true;
                break;
            }
        }
        if !ok {
            let (mut lo, mut hi) = (-d_max, d_max);
            for _ in 0..34 {
                let m = 0.5 * (lo + hi);
                if resid(m) > 0.0 {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            d = 0.5 * (lo + hi);
        }
        self.d_prev = d;
        let vp_a = bp - d * p.raa / 4.0;
        let vp_b = bp + d * p.raa / 4.0;
        let (s_a, s_b) = screens(vp_a, vp_b);
        let i_a = pentode_ip(&p.tube, g_a - vk, s_a - vk, vp_a - vk);
        let i_b = pentode_ip(&p.tube, g_b - vk, s_b - vk, vp_b - vk);
        self.ig2_prev_a = pentode_ig2(&p.tube, g_a - vk, s_a - vk);
        self.ig2_prev_b = pentode_ig2(&p.tube, g_b - vk, s_b - vk);
        let ig2_total = self.ig2_prev_a + self.ig2_prev_b;
        // plate-to-plate volts, the way round that makes the whole amp non-inverting
        (vp_a - vp_b, i_a, i_b, ig2_total)
    }

    /// Returns (total supply current, ia, ib).
    fn supply_current(&mut self, g_a: f64, g_b: f64, bp: f64) -> (f64, f64, f64) {
        let bias = self.parts.bias;
        let (_vpp, ia, ib, ig2) = self.output_stage(bias + g_a, bias + g_b, bp);
        ((ia + ib + ig2) * f64::from(self.parts.pairs), ia, ib)
    }

    fn filters_prepare(&mut self) {
        let p = self.parts;
        self.ot_hp = highpass(p.ot_low_hz, 0.6, self.fs);
        self.ot_lp = lowpass(p.ot_high_hz, 0.7, self.fs);
        self.spk_res = peak(p.spk_res_hz, p.spk_res_q, p.spk_res_gain, self.fs);
        self.spk_rise = high_shelf1(p.spk_rise_hz, p.spk_rise_gain, self.fs);
    }
}

/// The phase inverter's exact solves (`phaseInverterExact`, `cathodyneExact`, `plateSolve`) with
/// the state they keep while `prepare` settles the idle and tabulates the transfer.
#[derive(Debug, Clone, Default)]
struct PiSolver {
    parts: PowerParts,
    plate_a: f64,
    plate_b: f64,
    idle_a: f64,
    idle_b: f64,
    idle_set: bool,
    cathodyne_grid_idle: f64,
}

impl Default for PowerParts {
    fn default() -> Self {
        TAIL
    }
}

impl PiSolver {
    /// A triode's plate current with its plate resistor, solved together (bisection).
    fn plate_solve(t: &TriodeParams, vgk: f64, supply: f64, rp: f64, vk: f64) -> f64 {
        let mut lo = 0.0;
        let mut hi = cmax(0.0, (supply - vk) / rp);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if triode_ip(t, vgk, supply - mid * rp - vk) > mid {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    /// The cathodyne: one current through the plate load and the whole cathode load.
    fn cathodyne(&mut self, vin: f64) -> (f64, f64) {
        let p = self.parts;
        let solve = |vg: f64| -> f64 {
            let mut lo = 0.0;
            let mut hi = p.pi_supply / (p.pi_rp_a + p.pi_rk_total);
            for _ in 0..50 {
                let i = 0.5 * (lo + hi);
                let vk = i * p.pi_rk_total;
                let vp = p.pi_supply - i * p.pi_rp_a;
                let mut vgk = vg - vk;
                if vgk > 0.0 {
                    vgk *= 0.3; // grid conduction
                }
                if triode_ip(&p.pi_tube, vgk, vp - vk) > i {
                    lo = i;
                } else {
                    hi = i;
                }
            }
            0.5 * (lo + hi)
        };
        if !self.idle_set {
            // the grid's own bias: 1M from the tap between the bias resistor and the rest
            let mut vg = 0.0;
            for _ in 0..60 {
                vg = solve(vg) * (p.pi_rk_total - p.pi_rk_bias);
            }
            self.cathodyne_grid_idle = vg;
            let i = solve(vg);
            self.idle_a = p.pi_supply - i * p.pi_rp_a;
            self.idle_b = i * p.pi_rk_total;
            self.idle_set = true;
        }
        let i = solve(self.cathodyne_grid_idle + vin);
        ((p.pi_supply - i * p.pi_rp_a) - self.idle_a, i * p.pi_rk_total - self.idle_b)
    }

    fn exact(&mut self, vg1: f64, vg2: f64) -> (f64, f64) {
        if self.parts.pi_type == 1 {
            return self.cathodyne(vg1 - vg2);
        }
        let p = self.parts;
        // Shared cathode: find vk such that the two plate currents add up to the tail's.
        let (mut lo, mut hi) = (-20.0, 150.0);
        let (mut ia, mut ib) = (0.0, 0.0);
        for _ in 0..30 {
            let vk = 0.5 * (lo + hi);
            ia = Self::plate_solve(&p.pi_tube, vg1 - vk, p.pi_supply, p.pi_rp_a, vk);
            ib = Self::plate_solve(&p.pi_tube, vg2 - vk, p.pi_supply, p.pi_rp_b, vk);
            if ia + ib > p.pi_tail_current {
                lo = vk;
            } else {
                hi = vk;
            }
        }
        self.plate_a = p.pi_supply - ia * p.pi_rp_a;
        self.plate_b = p.pi_supply - ib * p.pi_rp_b;
        // AC out: the plates' swing around their idle
        if !self.idle_set {
            self.idle_a = self.plate_a;
            self.idle_b = self.plate_b;
            self.idle_set = true;
        }
        (self.plate_a - self.idle_a, self.plate_b - self.idle_b)
    }
}
