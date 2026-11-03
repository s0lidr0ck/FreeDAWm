//! SatEngineDSP -- Engine 8: clean colour at unity gain.
//! Line-by-line translation of `Engine 8 - Sat/Source/SatEngineDSP.h`.
//!
//! Five circuits, chosen by MECHANISM rather than by name:
//!
//! - 0 Iron: transformer core. Jiles-Atherton hysteresis driven by the INTEGRAL of the signal, so
//!   iron distorts more as frequency falls.
//! - 1 Valve: triode / pentode whose operating point MOVES (grid-leak bias and cathode shift).
//! - 2 Desk: console line amp. Slew-rate limiting is the audible mechanism, not clipping.
//! - 3 Bridge: diode bridge on the Shockley equation, solved exactly through the Wright omega
//!   function, with the iron around it approximated as a low-frequency emphasis.
//! - 4 FET: square-law JFET. The one deliberately static model.
//!
//! Signal flow, per channel:
//!
//! ```text
//!   in -> [M/S encode] -> upsample x N --+--------------------------------(dry)--+
//!                                        |                                       |
//!                                        +-> drive -> focus pre -> circuit ->    |
//!                                            focus de -> DC block -> auto gain   |
//!                                            (+ air band) -----------(wet)-> mix +-> downsample
//!                                                                                  -> output -> [M/S decode]
//! ```
//!
//! Dry and wet are mixed AT THE OVERSAMPLED RATE, before the one downsampling filter.
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - Float and double are kept exactly where the C++ has them (`float` maths in Valve, Desk and
//!   FET, `double` elsewhere, the glides in `double`).
//! - `int (v + 0.5f)` on a choice value: Rust `as` truncates toward zero like the C++ cast for
//!   every finite value; non-finite values never reach here (the plugin wrapper clamps first, as
//!   the APVTS does in the C++).
//! - `clampT` returns NaN for NaN like the C++ template (comparisons false); same for `std::max`.
//! - All state is fixed-size, so `rebuild_oversampling` (called on the audio thread when the
//!   circuit, variant or quality changes) never allocates.

use std::f64::consts::PI;

use super::sat_gain_table as satgain;
use super::sat_halfband::{self as sathb, Coefs, Stage};

pub const C_IRON: i32 = 0;
pub const C_VALVE: i32 = 1;
pub const C_DESK: i32 = 2;
pub const C_BRIDGE: i32 = 3;
pub const C_FET: i32 = 4;
pub const NUM_CIRCUITS: i32 = 5;
pub const NUM_VARIANTS: i32 = 3;
const NV: usize = NUM_VARIANTS as usize;

// ---------------------------------------------------------------- parameters (order == Parameters.h)
pub const P_CIRCUIT: usize = 0;
pub const P_VARIANT: usize = 1;
pub const P_DRIVE: usize = 2;
pub const P_BIAS: usize = 3;
pub const P_FOCUS: usize = 4;
pub const P_AIR: usize = 5;
pub const P_MIX: usize = 6;
pub const P_OUTPUT: usize = 7;
pub const P_AUTO_GAIN: usize = 8;
pub const P_CHAN_MODE: usize = 9;
pub const P_QUALITY: usize = 10;
pub const P_BYPASS: usize = 11;
pub const NUM_PARAMS: usize = 12;

// ---------------------------------------------------------------- unit maps
/// -6 .. +24 dB into the circuit.
pub fn knob_to_drive_db(v: f32) -> f32 {
    -6.0 + 30.0 * v
}
/// -12 .. +12 dB.
pub fn knob_to_out_db(v: f32) -> f32 {
    -12.0 + 24.0 * v
}
/// -1 .. +1, 0.5 = neutral.
pub fn knob_to_bias(v: f32) -> f32 {
    2.0 * v - 1.0
}
/// Focus: +12 dB lows .. +12 dB highs, 0.5 = flat.
pub fn knob_to_tilt_db(v: f32) -> f32 {
    -12.0 * (2.0 * v - 1.0)
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
}

fn lowpass(fc: f64, fs: f64, q: f64) -> Biquad {
    let w = 2.0 * PI * fc.min(0.49 * fs) / fs;
    let cw = w.cos();
    let al = w.sin() / (2.0 * q);
    let a0 = 1.0 + al;
    let mut b = Biquad::default();
    b.b0 = (1.0 - cw) * 0.5 / a0;
    b.b1 = (1.0 - cw) / a0;
    b.b2 = b.b0;
    b.a1 = -2.0 * cw / a0;
    b.a2 = (1.0 - al) / a0;
    b
}

fn highpass(fc: f64, fs: f64, q: f64) -> Biquad {
    let w = 2.0 * PI * fc.min(0.49 * fs) / fs;
    let cw = w.cos();
    let al = w.sin() / (2.0 * q);
    let a0 = 1.0 + al;
    let mut b = Biquad::default();
    b.b0 = (1.0 + cw) * 0.5 / a0;
    b.b1 = -(1.0 + cw) / a0;
    b.b2 = b.b0;
    b.a1 = -2.0 * cw / a0;
    b.a2 = (1.0 - al) / a0;
    b
}

/// A first-order shelf and its exact inverse. H(s) = (s + G w) / (s + w): DC gain G, unity at the
/// top. Swapping numerator and denominator gives 1/H.
#[derive(Debug, Clone, Copy)]
struct Shelf1 {
    b0: f64,
    b1: f64,
    a1: f64,
    z: f64,
}

impl Default for Shelf1 {
    fn default() -> Self {
        Shelf1 { b0: 1.0, b1: 0.0, a1: 0.0, z: 0.0 }
    }
}

impl Shelf1 {
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
    fn set(&mut self, fc: f64, fs: f64, g: f64, inverse: bool) {
        let k = (PI * fc.min(0.45 * fs) / fs).tan();
        let (mut n0, mut n1, mut d0, mut d1) = (1.0 + g * k, g * k - 1.0, 1.0 + k, k - 1.0);
        if inverse {
            std::mem::swap(&mut n0, &mut d0);
            std::mem::swap(&mut n1, &mut d1);
        }
        self.b0 = n0 / d0;
        self.b1 = n1 / d0;
        self.a1 = d1 / d0;
    }
}

// ---------------------------------------------------------------- per-circuit constants
/// Iron: Jiles-Atherton, in normalised units (Ms = 1). a = anhysteretic shape, k = pinning (the
/// width of the loop), c = reversible fraction, alpha = inter-domain coupling.
struct IronSpec {
    a: f64,
    k: f64,
    c: f64,
    alpha: f64,
    hf_hz: f64,
    hf_q: f64,
}
const IRON_SPECS: [IronSpec; NV] = [
    IronSpec { a: 0.60, k: 0.25, c: 0.35, alpha: 0.020, hf_hz: 35000.0, hf_q: 0.90 }, // Nickel  saturates early and softly
    IronSpec { a: 0.90, k: 0.45, c: 0.20, alpha: 0.030, hf_hz: 45000.0, hf_q: 0.70 }, // Steel   wider loop: more odd-order, more grit
    IronSpec { a: 1.60, k: 0.10, c: 0.60, alpha: 0.010, hf_hz: 60000.0, hf_q: 0.60 }, // Ferrite narrow loop, the cleanest of the three
];

/// Valve: asymmetric soft clip, knees kPos/kNeg and hardness n, around a bias point pushed by the
/// cathode (tauMs, shiftGain) and the grid leak (leakMs, leakCharge).
struct ValveSpec {
    k_pos: f32,
    k_neg: f32,
    n: f32,
    tau_ms: f32,
    shift_gain: f32,
    leak_ms: f32,
    leak_charge: f32,
}
const VALVE_SPECS: [ValveSpec; NV] = [
    ValveSpec { k_pos: 0.70, k_neg: 1.60, n: 2.0, tau_ms: 80.0, shift_gain: 1.2, leak_ms: 60.0, leak_charge: 0.35 }, // Triode
    ValveSpec { k_pos: 1.00, k_neg: 1.15, n: 4.0, tau_ms: 40.0, shift_gain: 0.6, leak_ms: 30.0, leak_charge: 0.15 }, // Pentode
    ValveSpec { k_pos: 0.45, k_neg: 0.90, n: 2.5, tau_ms: 150.0, shift_gain: 2.0, leak_ms: 120.0, leak_charge: 0.50 }, // Starved
];

/// Desk: output-stage softness, slew rate in V/us, and the amplifier's small-signal time constant,
/// with 1.0 normalised = 10 V peak.
struct DeskSpec {
    soft: f32,
    slew_vus: f32,
    tau_us: f32,
}
const DESK_SPECS: [DeskSpec; NV] = [
    DeskSpec { soft: 0.02, slew_vus: 2.00, tau_us: 1.0 }, // Clean    5534-class
    DeskSpec { soft: 0.08, slew_vus: 0.50, tau_us: 1.6 }, // Vintage  741-class
    DeskSpec { soft: 0.25, slew_vus: 0.25, tau_us: 3.0 }, // Driven   slow and pushed
];

/// Bridge: two diode legs, forward and reverse. Is in amps, nVt = n * 25.85 mV.
struct BridgeSpec {
    is_f: f64,
    n_vt_f: f64,
    is_r: f64,
    n_vt_r: f64,
}
const BRIDGE_SPECS: [BridgeSpec; NV] = [
    BridgeSpec { is_f: 1.0e-6, n_vt_f: 0.0336, is_r: 1.0e-6, n_vt_r: 0.0336 }, // Germanium
    BridgeSpec { is_f: 2.5e-9, n_vt_f: 0.0452, is_r: 2.5e-9, n_vt_r: 0.0452 }, // Silicon
    BridgeSpec { is_f: 1.0e-6, n_vt_f: 0.0336, is_r: 2.5e-9, n_vt_r: 0.0452 }, // Mixed
];
const BRIDGE_R: f64 = 2200.0;

/// Brings the three bridge variants into the same THD region at the same drive.
const BRIDGE_VARIANT_SCALE: [f64; NV] = [0.30, 2.00, 0.30]; // Mixed's forward leg IS germanium

/// FET: quiescent gate-source voltage with Vp = -2 V, Idss = 1 (normalised).
const FET_VGS_Q: [f32; NV] = [-1.4, -1.0, -0.5]; // Cold / Stock / Hot
const FET_VP: f32 = -2.0;

/// Where each circuit's operating level sits, relative to full scale.
const IN_SCALE: [f32; NUM_CIRCUITS as usize] = [0.30, 0.30, 1.00, 0.30, 0.12];

const IRON_K: f64 = 2.0 * PI * 100.0; // 100 Hz, amplitude 1 -> field amplitude 1
const IRON_AIR: f64 = 0.05;

// ---------------------------------------------------------------- per-channel state
#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    os: [Stage; 3], // os[0] sits next to the base rate
    pre: Shelf1,
    de: Shelf1,
    bridge_pre: Shelf1,
    bridge_de: Shelf1,
    air: Biquad,
    iron_hf: Biquad,

    // Iron
    h: f64,
    m: f64,
    h_prev: f64,
    b_prev: f64,
    // Valve
    shift: f32,
    grid_charge: f32,
    // Desk
    slew_y: f32,
    desk_target: f32,
    // Bridge
    v_bridge: f64,
    // DC blocker on the wet path
    dc_x: f64,
    dc_y: f64,
}

impl Channel {
    fn reset_circuit(&mut self) {
        self.h = 0.0;
        self.m = 0.0;
        self.h_prev = 0.0;
        self.b_prev = 0.0;
        self.shift = 0.0;
        self.grid_charge = 0.0;
        self.slew_y = 0.0;
        self.desk_target = 0.0;
        self.v_bridge = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.iron_hf.reset();
    }

    /// Rust addition for `Plugin::reset`: every piece of filter state, not only the circuit's.
    fn reset_all(&mut self) {
        for s in &mut self.os {
            s.reset();
        }
        self.pre.reset();
        self.de.reset();
        self.bridge_pre.reset();
        self.bridge_de.reset();
        self.air.reset();
        self.reset_circuit();
    }
}

/// The engine. Defaults match the C++ member initialisers.
#[derive(Debug, Clone)]
pub struct SatEngineDsp {
    ch: [Channel; 2],
    stage_coefs: [Coefs; 3],

    circuit: i32,
    variant: i32,
    quality: i32,
    drive_knob: f32,
    bias_knob: f32,
    focus_knob: f32,
    air_knob: f32,
    mix_knob: f32,
    out_knob: f32,
    auto_gain: bool,
    ms_mode: bool,
    bypassed: bool,
    calibrating: bool,

    drive_sm: f64,
    bias_sm: f64,
    focus_sm: f64,
    air_sm: f64,
    mix_sm: f64,
    out_sm: f64,
    ag_sm: f64,
    byp_sm: f64,
    smooth: f64,
    coef_counter: i32,

    fs_base: f64,
    os_t: f64,
    dc_r: f64,
    os_factor: i32,
    num_stages: i32,

    iron_leak: f64,
    iron_norm: [f64; NV],
    iron_trim: [f64; NV],
    valve_coef: [f32; NV],
    valve_leak_coef: [f32; NV],
    grid_attack: f32,
    desk_slew: [f32; NV],
    desk_s_tau: [f32; NV],
    desk_h: [f32; NV],
    desk_substeps: [i32; NV],
    desk_q: [f32; NV],
    desk_e: [f32; NV],

    in_peak: f32,
    out_peak: f32,
}

impl Default for SatEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl SatEngineDsp {
    pub fn new() -> Self {
        SatEngineDsp {
            ch: [Channel::default(); 2],
            stage_coefs: [sathb::coefs_for_stage(0), sathb::coefs_for_stage(1), sathb::coefs_for_stage(2)],
            circuit: C_VALVE,
            variant: 0,
            quality: 1,
            drive_knob: 0.4,
            bias_knob: 0.5,
            focus_knob: 0.5,
            air_knob: 0.0,
            mix_knob: 1.0,
            out_knob: 0.5,
            auto_gain: true,
            ms_mode: false,
            bypassed: false,
            calibrating: false,
            drive_sm: 0.4,
            bias_sm: 0.5,
            focus_sm: 0.5,
            air_sm: 0.0,
            mix_sm: 1.0,
            out_sm: 1.0,
            ag_sm: 1.0,
            byp_sm: 0.0,
            smooth: 0.001,
            coef_counter: 0,
            fs_base: 0.0,
            os_t: 1.0 / 192000.0,
            dc_r: 0.9999,
            os_factor: 4,
            num_stages: 2,
            iron_leak: 0.9999,
            iron_norm: [1.0, 1.0, 1.0],
            iron_trim: [satgain::IRON_TRIM[0], satgain::IRON_TRIM[1], satgain::IRON_TRIM[2]],
            valve_coef: [0.001, 0.001, 0.001],
            valve_leak_coef: [0.001, 0.001, 0.001],
            grid_attack: 0.01,
            desk_slew: [1.0, 1.0, 1.0],
            desk_s_tau: [1.0, 1.0, 1.0],
            desk_h: [1.0e-6, 1.0e-6, 1.0e-6],
            desk_substeps: [1, 1, 1],
            desk_q: [1.0, 1.0, 1.0],
            desk_e: [0.5, 0.5, 0.5],
            in_peak: 0.0,
            out_peak: 0.0,
        }
    }

    pub fn set_parameter(&mut self, index: usize, v: f32) {
        match index {
            P_CIRCUIT => {
                let c = clamp_t((v + 0.5) as i32, 0, NUM_CIRCUITS - 1);
                if c != self.circuit {
                    self.circuit = c;
                    self.reset_circuit_state();
                }
            }
            P_VARIANT => {
                let x = clamp_t((v + 0.5) as i32, 0, NUM_VARIANTS - 1);
                if x != self.variant {
                    self.variant = x;
                    self.reset_circuit_state();
                }
            }
            P_DRIVE => self.drive_knob = clamp_t(v, 0.0, 1.0),
            P_BIAS => self.bias_knob = clamp_t(v, 0.0, 1.0),
            P_FOCUS => self.focus_knob = clamp_t(v, 0.0, 1.0),
            P_AIR => self.air_knob = clamp_t(v, 0.0, 1.0),
            P_MIX => self.mix_knob = clamp_t(v, 0.0, 1.0),
            P_OUTPUT => self.out_knob = clamp_t(v, 0.0, 1.0),
            P_AUTO_GAIN => self.auto_gain = v > 0.5,
            P_CHAN_MODE => self.ms_mode = v > 0.5,
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
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs_base = if sample_rate > 0.0 { sample_rate } else { 48000.0 };
        self.smooth = 1.0 - (-1.0 / (0.015 * self.fs_base)).exp(); // 15 ms
        self.rebuild_oversampling();

        // Every smoothed value starts AT its target, so a session loads without a ramp.
        self.drive_sm = f64::from(self.drive_knob);
        self.bias_sm = f64::from(self.bias_knob);
        self.focus_sm = f64::from(self.focus_knob);
        self.air_sm = f64::from(self.air_knob);
        self.mix_sm = f64::from(self.mix_knob);
        self.out_sm = f64::from(self.out_gain_target());
        self.ag_sm = f64::from(self.auto_gain_target());
        self.byp_sm = if self.bypassed { 1.0 } else { 0.0 };
        self.update_focus(self.focus_sm as f32);
    }

    /// Rust addition for `Plugin::reset`: clears all filter and circuit state (the C++ has no
    /// reset; JUCE's default `reset()` does nothing), then prepares again at the same rate.
    pub fn reset(&mut self) {
        for c in &mut self.ch {
            c.reset_all();
        }
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

    // ---------------------------------------------------------------- audio
    /// `num_channels` planar buffers of at least `num_samples` frames (the caller guarantees it).
    pub fn process(&mut self, channels: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        let nc = num_channels.min(2).min(channels.len());
        if nc == 0 {
            return; // the C++ would read channels[0] regardless; a host never passes none
        }
        let num_samples = channels.iter().take(nc).map(Vec::len).fold(num_samples, usize::min);
        let ms = self.ms_mode && nc == 2;

        for n in 0..num_samples {
            // ---- smoothing, once per base-rate sample
            let s = self.smooth;
            glide(s, &mut self.drive_sm, f64::from(self.drive_knob));
            glide(s, &mut self.bias_sm, f64::from(self.bias_knob));
            glide(s, &mut self.focus_sm, f64::from(self.focus_knob));
            glide(s, &mut self.air_sm, f64::from(self.air_knob));
            glide(s, &mut self.mix_sm, f64::from(self.mix_knob));
            let (out_target, ag_target) = (self.out_gain_target(), self.auto_gain_target());
            glide(s, &mut self.out_sm, f64::from(out_target));
            glide(s, &mut self.ag_sm, f64::from(ag_target));
            glide(s, &mut self.byp_sm, if self.bypassed { 1.0 } else { 0.0 });

            self.coef_counter += 1;
            if self.coef_counter >= 16 {
                self.coef_counter = 0;
                self.update_focus(self.focus_sm as f32);
            }

            let drive_gain = 10.0f32.powf(knob_to_drive_db(self.drive_sm as f32) / 20.0);
            let bias = knob_to_bias(self.bias_sm as f32);

            let sample = |c: usize, channels: &[Vec<f32>]| channels.get(c).and_then(|b| b.get(n)).copied().unwrap_or(0.0);
            let input = [sample(0, channels), if nc > 1 { sample(1, channels) } else { 0.0 }];
            self.in_peak = max_f(self.in_peak, max_f(input[0].abs(), input[1].abs()));

            let mut x = input;
            if ms {
                let m = 0.5 * (x[0] + x[1]);
                let s = 0.5 * (x[0] - x[1]);
                x[0] = m;
                x[1] = s;
            }

            let mut y = [0.0f32; 2];
            for c in 0..nc {
                y[c] = self.process_oversampled(c, x[c], drive_gain, bias);
            }

            if ms {
                let l = y[0] + y[1];
                let r = y[0] - y[1];
                y[0] = l;
                y[1] = r;
            }

            for (c, (&yc, &inc)) in y.iter().zip(&input).enumerate().take(nc) {
                let out = yc * self.out_sm as f32;
                // Bypass crossfades rather than clicking, and once it has landed it is the input
                // itself -- not out + 1 * (in - out), which need not round back to exactly in.
                let o = if self.byp_sm >= 1.0 { inc } else { out + self.byp_sm as f32 * (inc - out) };
                if let Some(slot) = channels.get_mut(c).and_then(|b| b.get_mut(n)) {
                    *slot = o;
                }
                self.out_peak = max_f(self.out_peak, o.abs());
            }
        }
    }

    // ---------------------------------------------------------------- the oversampled path
    fn process_oversampled(&mut self, ci: usize, x_in: f32, drive_gain: f32, bias: f32) -> f32 {
        let n_os = self.os_factor.clamp(1, 8) as usize;
        let num_stages = self.num_stages.clamp(0, 3) as usize;
        let mix = self.mix_sm;
        let wet_gain = if self.auto_gain && !self.calibrating { self.ag_sm } else { 1.0 };
        let air = self.air_sm;
        let drive_gain_d = f64::from(drive_gain);
        let dc_r = self.dc_r;

        // ---- up: one base-rate sample becomes N, stage 0 first
        let mut buf = [0.0f64; 8];
        let mut tmp = [0.0f64; 8];
        buf[0] = f64::from(x_in);
        let mut count = 1usize;
        for st in 0..num_stages {
            if let Some(stage) = self.ch.get_mut(ci).and_then(|c| c.os.get_mut(st)) {
                for i in 0..count {
                    let (a, b) = stage.up(buf[i]);
                    tmp[2 * i] = a;
                    tmp[2 * i + 1] = b;
                }
            }
            count *= 2;
            buf[..count].copy_from_slice(&tmp[..count]);
        }

        // ---- the circuit, at the oversampled rate
        for slot in buf.iter_mut().take(n_os.min(count)) {
            let dry = *slot;

            let mut w = self.ch_mut(ci).pre.run(dry * drive_gain_d);
            w = self.run_circuit(ci, w, bias);
            let c = self.ch_mut(ci);
            w = c.de.run(w);

            // DC blocker: every asymmetric circuit makes DC, and bias adds more on purpose
            let dc = w - c.dc_x + dc_r * c.dc_y;
            c.dc_x = w;
            c.dc_y = dc;
            w = dc / drive_gain_d * wet_gain;

            // Air: the input's top band, saturated, added on top.
            if air > 0.0005 {
                let h = self.ch_mut(ci).air.run(dry);
                w += air * 0.25 * (4.0 * h).tanh();
            }

            *slot = dry + mix * (w - dry);
        }

        // ---- down: pairs, innermost stage first
        for st in (0..num_stages).rev() {
            count /= 2;
            if let Some(stage) = self.ch.get_mut(ci).and_then(|c| c.os.get_mut(st)) {
                for i in 0..count {
                    buf[i] = stage.down(buf[2 * i], buf[2 * i + 1]);
                }
            }
        }

        buf[0] as f32
    }

    /// Channel `ci` (always 0 or 1; anything else falls back to channel 0).
    #[inline]
    fn ch_mut(&mut self, ci: usize) -> &mut Channel {
        let [c0, c1] = &mut self.ch;
        if ci == 1 { c1 } else { c0 }
    }

    fn run_circuit(&mut self, ci: usize, x: f64, bias: f32) -> f64 {
        let k = f64::from(IN_SCALE.get(self.circuit as usize).copied().unwrap_or(1.0));
        let x = x * k;
        let y = match self.circuit {
            C_IRON => self.iron(ci, x, bias),
            C_VALVE => f64::from(self.valve(ci, x as f32, bias)),
            C_DESK => f64::from(self.desk(ci, x as f32, bias)),
            C_BRIDGE => self.bridge(ci, x, bias),
            C_FET => f64::from(self.fet_stage(x as f32, bias)),
            _ => x,
        };
        y / k
    }

    fn v(&self) -> usize {
        (self.variant.clamp(0, NUM_VARIANTS - 1)) as usize
    }

    // ---------------------------------------------------------------- 0: Iron
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

    fn iron(&mut self, ci: usize, x: f64, bias: f32) -> f64 {
        let v = self.v();
        let p = &IRON_SPECS[v];
        let (iron_leak, os_t, iron_norm) = (self.iron_leak, self.os_t, self.iron_norm[v]);
        let c = self.ch_mut(ci);

        // Flux. A leaky integrator: the leak is the primary inductance.
        c.h = c.h * iron_leak + x * IRON_K * os_t;

        // Bias here is DC through the winding: onto the field, not the input.
        let h = c.h + 0.6 * f64::from(bias);
        let dh = h - c.h_prev;
        c.h_prev = h;

        let he = h + p.alpha * c.m;
        let man = Self::langevin(he / p.a);
        let d_man_d_he = Self::langevin_slope(he / p.a) / p.a;

        let delta = if dh >= 0.0 { 1.0 } else { -1.0 };
        let mut irr = 0.0;
        let gap = man - c.m;
        if delta * gap > 0.0 {
            // the standard fix: no negative susceptibility
            let den = delta * p.k - p.alpha * gap;
            if den.abs() > 1.0e-9 {
                irr = gap / den;
            }
        }

        let num = (1.0 - p.c) * irr + p.c * d_man_d_he;
        let den = 1.0 - p.alpha * p.c * d_man_d_he;
        c.m = clamp_t(c.m + (num / den) * dh, -1.2, 1.2);

        // Output is the rate of change of flux density: differentiate. A small air-core term
        // keeps some gain even with the core fully saturated.
        let b = c.m + IRON_AIR * h;
        let y = (b - c.b_prev) / (IRON_K * os_t * iron_norm);
        c.b_prev = b;

        c.iron_hf.run(y) // leakage inductance against winding capacitance
    }

    // ---------------------------------------------------------------- 1: Valve
    fn knee_clip(v: f32, k_pos: f32, k_neg: f32, n: f32) -> f32 {
        let k = if v >= 0.0 { k_pos } else { k_neg };
        v / (1.0f32 + (v.abs() / k).powf(n)).powf(1.0 / n)
    }

    fn valve(&mut self, ci: usize, x: f32, bias: f32) -> f32 {
        let v = self.v();
        let p = &VALVE_SPECS[v];
        let (grid_attack, leak_coef, coef) = (self.grid_attack, self.valve_leak_coef[v], self.valve_coef[v]);
        let c = self.ch_mut(ci);

        // The grid sits at its bias point, moved by Bias, the cathode and the coupling cap.
        let vg = x + 0.5 * bias - p.shift_gain * c.shift - c.grid_charge;
        let y = Self::knee_clip(vg, p.k_pos, p.k_neg, p.n);

        // Grid-leak bias: past the positive knee the grid conducts and charges the coupling cap
        // fast; it only bleeds off slowly through the leak resistor.
        let over = vg - p.k_pos;
        if over > 0.0 {
            c.grid_charge += grid_attack * p.leak_charge * over;
        }
        c.grid_charge -= leak_coef * c.grid_charge;

        // Cathode bias shift: a DC servo following the average current.
        c.shift += coef * (y - c.shift);
        y
    }

    // ---------------------------------------------------------------- 2: Desk
    /// tanh as a rational, clamped where it meets 1 with zero slope.
    fn soft_sat(x: f32) -> f32 {
        let x = clamp_t(x, -3.0, 3.0);
        let x2 = x * x;
        x * (27.0 + x2) / (27.0 + 9.0 * x2)
    }

    fn desk(&mut self, ci: usize, x: f32, bias: f32) -> f32 {
        let v = self.v();
        let p = &DESK_SPECS[v];
        let (s_tau, q, e_coef, substeps, h, slew) =
            (self.desk_s_tau[v], self.desk_q[v], self.desk_e[v], self.desk_substeps[v], self.desk_h[v], self.desk_slew[v]);
        let c = self.ch_mut(ci);

        let u = x + 0.3 * bias;
        let target = u / (1.0f32 + p.soft * u * u).sqrt(); // output stage, symmetric

        // The amplifier as an ODE: dy/dt = S tanh((target - y) / (S tau)), integrated in sub-steps
        // with the sample's AVERAGE output (a first-order anti-alias on the slewing corner).
        let t0 = c.desk_target;
        let e0 = t0 - c.slew_y;

        // Fast path: while the error stays in the input stage's linear range the ODE is linear and
        // a target ramping through a one-pole has an exact solution.
        //   y(s) = t(s) - r tau + (e0 + r tau) e^(-s / tau),   r = (t1 - t0) / dt
        let rq = (target - t0) * q; // r tau
        if e0.abs() < 0.15 * s_tau && rq.abs() < 0.15 * s_tau {
            let transient = -e0 + rq; // y0 - t0 + r tau
            c.slew_y = target - rq + transient * e_coef;
            c.desk_target = target;
            return 0.5 * (t0 + target) - rq + transient * q * (1.0 - e_coef);
        }

        let k = substeps;
        let mut y = c.slew_y;
        let mut acc = 0.0f32;
        for i in 0..k {
            let t = c.desk_target + (target - c.desk_target) * (i as f32 + 0.5) / k as f32;
            let yn = y + h * slew * Self::soft_sat((t - y) / s_tau);
            acc += 0.5 * (y + yn);
            y = yn;
        }
        c.slew_y = y;
        c.desk_target = target;
        acc / k as f32
    }

    // ---------------------------------------------------------------- 3: Bridge
    /// Wright omega, w(x) = W(e^x): D'Angelo, Gabrielli & Turchet's initial estimate (DAFx 2019),
    /// one corrective step, then two Newton steps on w + ln w = x.
    // `!(y > 0.0)` as the C++ (it also catches NaN); the constants are the C++ literals verbatim.
    #[allow(clippy::neg_cmp_op_on_partial_ord, clippy::excessive_precision)]
    fn wright_omega(x: f64) -> f64 {
        let mut y = if x < -3.341459552768620 {
            0.0
        } else if x < 8.0 {
            0.6313183464296682 + x * (0.3631952663804445 + x * (4.775931364975583e-2 + x * -1.314293149877800e-3))
        } else {
            x - x.ln()
        };
        y = y - (y - (x - y).exp()) / (y + 1.0);
        if !(y > 0.0) {
            return 0.0; // exp underflowed: the diode is off, and log(0) must not happen
        }
        for _ in 0..2 {
            y -= y * (y + y.ln() - x) / (y + 1.0);
        }
        y
    }

    /// One diode leg through a series resistor, exactly (Lambert W).
    fn diode_leg(u: f64, is: f64, n_vt: f64) -> f64 {
        let isr = is * BRIDGE_R;
        u + isr - n_vt * Self::wright_omega((isr / n_vt).ln() + (u + isr) / n_vt)
    }

    fn bridge(&mut self, ci: usize, x: f64, bias: f32) -> f64 {
        let v = self.v();
        let p = &BRIDGE_SPECS[v];
        let scale = BRIDGE_VARIANT_SCALE[v];
        let c = self.ch_mut(ci);

        // Series resistor into two diode legs, one each way, solved in closed form.
        let u = c.bridge_pre.run(x * scale) + 0.15 * f64::from(bias);
        let vb = if u >= 0.0 { Self::diode_leg(u, p.is_f, p.n_vt_f) } else { -Self::diode_leg(-u, p.is_r, p.n_vt_r) };
        c.v_bridge = vb;
        c.bridge_de.run(vb) / scale
    }

    // ---------------------------------------------------------------- 4: FET
    /// Square law between pinch-off and gate conduction.
    fn fet_stage(&self, x: f32, bias: f32) -> f32 {
        let vq = FET_VGS_Q[self.v()] + 0.4 * bias;
        let vgs = clamp_t(vq + 0.8 * x, FET_VP, 0.35);
        let t = 1.0 - vgs / FET_VP;
        let tq = 1.0 - vq / FET_VP;
        let gm = 2.0 * tq / -FET_VP; // small-signal slope at the bias point
        (t * t - tq * tq) / (0.8 * gm) // unity gain for small signals
    }

    // ---------------------------------------------------------------- targets
    fn out_gain_target(&self) -> f32 {
        10.0f32.powf(knob_to_out_db(self.out_knob) / 20.0)
    }

    fn auto_gain_target(&self) -> f32 {
        let pos = self.drive_knob * (satgain::K_POINTS - 1) as f32;
        let i = clamp_t(pos as i32, 0, satgain::K_POINTS as i32 - 2) as usize;
        let t = pos - i as f32;
        let row = &satgain::TABLE[self.circuit.clamp(0, NUM_CIRCUITS - 1) as usize][self.v()];
        row[i] + t * (row[i + 1] - row[i])
    }

    // ---------------------------------------------------------------- rebuilds
    fn update_focus(&mut self, knob: f32) {
        let g = 10.0f64.powf(f64::from(knob_to_tilt_db(knob)) / 20.0);
        let os_fs = self.fs_base * f64::from(self.os_factor);
        for c in &mut self.ch {
            c.pre.set(700.0, os_fs, g, false);
            c.de.set(700.0, os_fs, g, true);
        }
    }

    fn rebuild_oversampling(&mut self) {
        // Eco 2x, Normal 4x, Insane 8x -- except Iron, which never runs below 4x.
        const FACTORS: [i32; 3] = [2, 4, 8];
        self.os_factor = FACTORS[self.quality.clamp(0, 2) as usize];
        if self.circuit == C_IRON {
            self.os_factor = self.os_factor.max(4);
        }

        let os_fs = self.fs_base * f64::from(self.os_factor);
        self.os_t = 1.0 / os_fs;
        self.num_stages = if self.os_factor == 2 {
            1
        } else if self.os_factor == 4 {
            2
        } else {
            3
        };

        let v = self.v();
        for c in &mut self.ch {
            for (stage, coefs) in c.os.iter_mut().zip(&self.stage_coefs) {
                stage.set(coefs);
            }
            c.air = highpass(5000.0, os_fs, 0.707);
            c.iron_hf = lowpass(IRON_SPECS[v].hf_hz, os_fs, IRON_SPECS[v].hf_q);
            c.bridge_pre.set(150.0, os_fs, 10.0f64.powf(3.0 / 20.0), false); // the iron around the bridge
            c.bridge_de.set(150.0, os_fs, 10.0f64.powf(3.0 / 20.0), true);
            c.reset_circuit();
        }

        self.dc_r = (-2.0 * PI * 5.0 / os_fs).exp();
        self.grid_attack = (1.0 - (-1.0 / (0.001 * os_fs)).exp()) as f32; // ~1 ms: grid current is fast
        self.iron_leak = (-2.0 * PI * 8.0 / os_fs).exp(); // primary inductance: -3 dB near 8 Hz

        for v in 0..NV {
            let vs = &VALVE_SPECS[v];
            let ds = &DESK_SPECS[v];
            let is = &IRON_SPECS[v];
            // float maths, as the C++ (std::exp on a float argument)
            self.valve_coef[v] = 1.0f32 - (-1.0f32 / (vs.tau_ms * 0.001 * os_fs as f32)).exp();
            self.valve_leak_coef[v] = 1.0f32 - (-1.0f32 / (vs.leak_ms * 0.001 * os_fs as f32)).exp();
            // Enough sub-steps that each is at most half of tau; capped.
            self.desk_slew[v] = (f64::from(ds.slew_vus) * 1.0e6 / 10.0) as f32; // normalised units per second
            self.desk_s_tau[v] = self.desk_slew[v] * ds.tau_us * 1.0e-6f32;
            // C++ `(int) std::ceil(...)`: `as` saturates where the C++ cast would be undefined.
            self.desk_substeps[v] = clamp_t((2.0 * self.os_t / (f64::from(ds.tau_us) * 1.0e-6)).ceil() as i32, 1, 16);
            self.desk_h[v] = (self.os_t / f64::from(self.desk_substeps[v])) as f32;
            self.desk_q[v] = (f64::from(ds.tau_us) * 1.0e-6 / self.os_t) as f32; // tau / dt
            self.desk_e[v] = (-self.os_t / (f64::from(ds.tau_us) * 1.0e-6)).exp() as f32;
            // Small-signal permeability of each core, times the MEASURED trim.
            self.iron_norm[v] = (is.c / (3.0 * is.a) + IRON_AIR + (1.0 - is.c) / (is.k * (1.0 + 3.0 * is.a))) * self.iron_trim[v];
        }

        self.update_focus(self.focus_sm as f32);
    }

    fn reset_circuit_state(&mut self) {
        // A new circuit or variant can change the HF filter and the oversampling floor.
        if self.fs_base > 0.0 {
            self.rebuild_oversampling();
        }
    }
}

/// One-pole glide that LANDS, in double (a float one-pole stalls short of its target).
#[inline]
fn glide(smooth: f64, v: &mut f64, target: f64) {
    *v += smooth * (target - *v);
    if (target - *v).abs() < 1.0e-9 {
        *v = target;
    }
}
