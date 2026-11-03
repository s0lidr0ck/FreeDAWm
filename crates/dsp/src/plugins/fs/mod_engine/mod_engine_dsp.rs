//! MOD ENGINE -- 14 component-level modulation circuits.
//! Line-by-line translation of `Engine 4 - Mod/Source/ModEngineDSP.h`.
//!
//! Families and what is actually modelled:
//!
//! - BBD (0-7): the Dimension D signal path, generalised.
//!   `in -> HPF -> anti-alias LPF -> NE570 compressor -> BBD line -> NE570 expander -> reconstruction LPF`.
//!   Delay is set as stages / (2 * fclk); the LFO sweeps the clock, and the BBD bandwidth
//!   (aa/recon corner) follows the clock, so pitch, bandwidth and noise floor move together.
//!   Charge-transfer loss is eta^stages as a one-pole droop. Clock feedthrough is a filtered noise
//!   floor scaled by the Noise knob and pushed down by the expander.
//!   0 Dimension D, 1 CE-1 (MN3002 512), 2 CE-2 (MN3007 1024), 3 Small Clone (MN3007),
//!   4 JC-120 (MN3002 pair), 5 Brigade (generic MN3007, dark/wide), 6 Electric Mistress (SAD1024
//!   flanger), 7 A/DA (SAD1024).
//! - Phase (8-10): 8 Phase 90 (4 JFET allpass stages, audio modulates Rds), 9 Small Stone (4 OTA
//!   allpass stages, pole linear in bias current), 10 Uni-Vibe (one lamp into 4 LDRs).
//! - Trem / vibrato (11-13): 11 Opto Trem (neon strike + LDR shunt), 12 Harmonic (3-section
//!   phase-shift network, bands driven counter-phase), 13 Magnatone (6 phase-shift sections swept
//!   by a varistor bridge: real FM, no delay line).
//!
//! Frame conventions: drive-role knob = Depth, tone-role knob = Rate, Level 0.7071 = unity, Input
//! trim 0..1 -> -12..+24 dB, Link off by default. Zero latency: IIR only, no lookahead.
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - `float` and `double` are kept exactly where the C++ has them (the BBD filter design and the
//!   one-pole coefficients in `double`, everything per sample in `float`).
//! - `clampT`, `std::min` and `std::max` keep their C++ definitions (`clamp_t`, `min_t`, `max_t`),
//!   including what they do with a NaN.
//! - `int (v + 0.5f)` on a choice value: Rust `as` truncates toward zero like the C++ cast for
//!   every finite value; the plugin wrapper clamps first, as the APVTS does in the C++.
//! - `nextNoise` multiplies a signed 32-bit integer and overflows; MSVC wraps (two's complement),
//!   so the Rust uses `wrapping_mul` / `wrapping_add`.
//! - All state is fixed-size (the two 8192-sample BBD lines live inside the struct), so nothing
//!   here allocates.
//!
//! Rust addition (plan A3): a host tempo, given by [`ModEngineDsp::set_host_transport`] as the
//! FireStation Delay and Filter engines take theirs, stands in for the Tempo knob while Sync is on.
//! With no host tempo (0, as the C++ render tool gives), the C++ path is unchanged.

// The C++ literals (6.2831853f, 0.7071f) are kept as written, not swapped for the constants.
#![allow(clippy::approx_constant)]

use std::f64::consts::PI as PI64;

// ---------------------------------------------------------------- parameters (order == Parameters.h)
pub const P_DEPTH: usize = 0;
pub const P_RATE: usize = 1;
pub const P_LEVEL: usize = 2;
pub const P_CIRCUIT: usize = 3;
pub const P_VARIANT: usize = 4;
pub const P_BYPASS: usize = 5;
pub const P_TRIM: usize = 6;
pub const P_LINK: usize = 7;
pub const P_LINK_LO: usize = 8;
pub const P_LINK_HI: usize = 9;
pub const P_MIX: usize = 10;
pub const P_WIDTH: usize = 11;
pub const P_REGEN: usize = 12;
pub const P_DRIVE: usize = 13;
pub const P_NOISE: usize = 14;
pub const P_BIAS: usize = 15;
pub const P_TONE: usize = 16;
pub const P_STEREO: usize = 17;
pub const P_SHAPE: usize = 18;
pub const P_SYNC: usize = 19;
pub const P_DIVISION: usize = 20;
pub const P_TEMPO: usize = 21;
pub const NUM_PARAMS: usize = 22;

// ---------------------------------------------------------------- constants
const INPUT_VOLTS: f64 = 0.15;
const OUTPUT_SCALE: f64 = 0.725 / 0.15;
const COMP_REF: f32 = 0.10;
const BBD_CLIP: f32 = 0.55;
/// Dimension D stock wet (UAD A/B).
const WET_STOCK: f32 = 0.42;
/// Dimension D stock cross-feed.
const CROSS_STOCK: f32 = 0.67;
const LINE_B_OFFSET: f32 = 1.05;
const BUF: usize = 8192;
const BUF_I: i32 = BUF as i32;

// ---------------------------------------------------------------- C++ helpers, same semantics
fn clamp_t(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}
/// `std::min`: `(b < a) ? b : a`.
fn min_t(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}
/// `std::max`: `(a < b) ? b : a`.
fn max_t(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

// ---------------------------------------------------------------- knob -> unit maps
/// The UI prints these same numbers.
pub fn knob_to_trim_db(v: f32) -> f32 {
    -12.0 + 36.0 * clamp_t(v, 0.0, 1.0)
}
/// 0.7071 = unity.
pub fn knob_to_level_gain(v: f32) -> f32 {
    2.0 * v * v
}
pub fn knob_to_bpm(v: f32) -> f32 {
    40.0 + 200.0 * v
}

/// LFO cycle length in quarter-note beats, for the Division parameter.
pub fn division_beats(d: i32) -> f32 {
    if d == 0 {
        return 8.0;
    }
    if d == 1 {
        return 4.0;
    }
    if d == 2 {
        return 3.0;
    }
    if d == 3 {
        return 2.0;
    }
    if d == 4 {
        return 1.5;
    }
    if d == 5 {
        return 1.0;
    }
    if d == 6 {
        return 0.5;
    }
    0.25
}

/// What the Rate knob is worth on a given circuit: the stock rate at knob 5, and the two ends it
/// sweeps to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateSpec {
    pub stock: f32,
    pub lo: f32,
    pub hi: f32,
}

pub fn rate_spec_for(circuit: i32, variant: i32) -> RateSpec {
    let r = |stock, lo, hi| RateSpec { stock, lo, hi };
    match circuit {
        0 => {
            // Dimension D: the mode buttons each have their own rate
            let stock = if variant == 0 {
                0.30
            } else if variant == 1 {
                0.78
            } else if variant == 2 {
                0.90
            } else if variant == 3 {
                1.20
            } else if variant == 4 {
                0.70
            } else {
                0.85
            };
            r(stock, 0.15, 2.4)
        }
        1 => r(0.9, 0.10, 5.0),
        2 => r(1.0, 0.12, 6.5),
        3 => r(0.8, 0.15, 4.5),
        4 => r(0.9, 0.10, 7.0),
        5 => r(0.7, 0.08, 6.0),
        6 => r(0.35, 0.05, 5.0),
        7 => r(0.35, 0.05, 6.0),
        8 => r(0.9, 0.10, 8.0),
        9 => r(0.8, 0.08, 7.0),
        10 => r(1.0, 0.10, 9.0),
        11 => r(4.0, 0.50, 12.0),
        12 => r(3.5, 0.40, 10.0),
        _ => r(5.0, 0.50, 9.0),
    }
}

/// The LFO frequency those settings produce (`lfoRateHz`).
pub fn lfo_rate_hz(circuit: i32, variant: i32, rate_knob: f32, sync_idx: i32, div_idx: i32, tempo_knob: f32) -> f32 {
    if sync_idx == 1 {
        return (knob_to_bpm(tempo_knob) / 60.0) / division_beats(div_idx);
    }
    let s = rate_spec_for(circuit, variant);
    let r = clamp_t(rate_knob, 0.0, 1.0);
    if r < 0.5 { s.lo * (s.stock / s.lo).powf(r * 2.0) } else { s.stock * (s.hi / s.stock).powf((r - 0.5) * 2.0) }
}

/// [`lfo_rate_hz`] with a host tempo standing in for the Tempo knob when it is valid (Rust
/// addition, plan A3; `host_bpm` 0 = none, the C++ path).
pub fn lfo_rate_hz_host(circuit: i32, variant: i32, rate_knob: f32, sync_idx: i32, div_idx: i32, tempo_knob: f32, host_bpm: f32) -> f32 {
    if sync_idx == 1 && host_bpm >= 20.0 {
        return (host_bpm / 60.0) / division_beats(div_idx);
    }
    lfo_rate_hz(circuit, variant, rate_knob, sync_idx, div_idx, tempo_knob)
}

/// The LFO shape factor `applyModel` derives (`lfoShape`): 0 = sine, 1 = triangle, > 1 =
/// trapezoid (the triangle scaled and clipped). The circuit's own unless the Shape switch overrides
/// it. For the UI; the engine keeps its own copy of the table in `apply_model`.
pub fn lfo_shape_for(circuit: i32, variant: i32, shape_idx: i32) -> f32 {
    let stock = match circuit {
        0 => match variant {
            0 | 1 => 1.4,
            2 => 1.3,
            3 => 1.0,
            4 => 1.2,
            _ => 1.3,
        },
        3 | 6 | 7 => 1.0,
        _ => 0.0,
    };
    match shape_idx {
        1 => 0.0,
        2 => 1.0,
        3 => 1.4,
        4 => 8.0,
        _ => stock,
    }
}

/// One LFO value at `phase` (0..1) for a shape factor, as the engine's sample loop computes it.
pub fn lfo_value(phase: f32, lfo_shape: f32) -> f32 {
    if lfo_shape <= 0.0 {
        (phase * 6.283_185_3).sin()
    } else {
        let tri = 4.0 * (phase - 0.5).abs() - 1.0;
        clamp_t(tri * lfo_shape, -1.0, 1.0)
    }
}

// ---------------------------------------------------------------- structs
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

#[derive(Debug, Clone, PartialEq)]
struct Channel {
    in_hp_x1: f32,
    in_hp_y1: f32,
    out_hp_x1: f32,
    out_hp_y1: f32,
    // BBD
    aa: [Biquad; 2],
    recon: [Biquad; 2],
    env_in: f32,
    env_out: f32,
    buf: [f32; BUF],
    write_pos: i32,
    /// charge-transfer droop state
    ct_loss: f32,
    /// clock-tracking bandwidth one-pole
    bw_state: f32,
    noise_lp: f32,
    /// flanger regen (post-expander)
    fb_state: f32,
    // phase
    ap: [f32; 6],
    ph_fb: f32,
    colour: f32,
    lamp: f32,
    lamp_slow: f32,
    // trem
    ldr: f32,
    hts: [f32; 3],
    h_bass: f32,
    vs: [f32; 6],
    out_lp: f32,
}

impl Default for Channel {
    fn default() -> Self {
        Channel {
            in_hp_x1: 0.0,
            in_hp_y1: 0.0,
            out_hp_x1: 0.0,
            out_hp_y1: 0.0,
            aa: [Biquad::default(); 2],
            recon: [Biquad::default(); 2],
            env_in: 0.0,
            env_out: 0.0,
            buf: [0.0; BUF],
            write_pos: 0,
            ct_loss: 0.0,
            bw_state: 0.0,
            noise_lp: 0.0,
            fb_state: 0.0,
            ap: [0.0; 6],
            ph_fb: 0.0,
            colour: 0.0,
            lamp: 0.3,
            lamp_slow: 0.3,
            ldr: 1.0,
            hts: [0.0; 3],
            h_bass: 0.0,
            vs: [0.0; 6],
            out_lp: 0.0,
        }
    }
}

fn filter(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

fn butter_lp(fc: f64, fsr: f64, q_: f64) -> Biquad {
    let k = (PI64 * fc / fsr).tan();
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

fn one_pole_coef(fc: f64, fsr: f64) -> f32 {
    (-2.0 * PI64 * fc / fsr).exp() as f32
}

fn soft_sat(x: f32, amt: f32) -> f32 {
    let y = x * (1.0 + amt * 3.0);
    y / (1.0 + y.abs() * 0.8)
}

/// The engine (`ModEngineDSP`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModEngineDsp {
    ch: [Channel; 2],

    // ---- parameter state
    depth_knob: f32,
    rate_knob: f32,
    level_knob: f32,
    circuit_idx: i32,
    variant_idx: i32,
    stereo_idx: i32,
    shape_idx: i32,
    sync_idx: i32,
    div_idx: i32,
    bypassed: bool,
    trim_db: f32,
    trim_lin: f32,
    trim_sm: f32,
    linked: bool,
    link_lo_val: f32,
    link_hi_val: f32,
    mix_knob: f32,
    width_knob: f32,
    regen_knob: f32,
    drive_knob: f32,
    noise_knob: f32,
    bias_knob: f32,
    tone_knob: f32,
    tempo_knob: f32,

    depth_sm: f32,
    rate_sm: f32,
    level_sm: f32,
    mix_sm: f32,
    width_sm: f32,
    regen_sm: f32,
    tone_counter: i32,

    // ---- circuit model (set by apply_model)
    /// 0 BBD, 1 phase, 2 trem
    family: i32,
    stages: f32,
    base_ms: f32,
    depth_ms: f32,
    aa_hz: f32,
    rec_hz: f32,
    eta: f32,
    /// 1 - eta^stages, hoisted out of the sample loop
    ct_loss_g: f32,
    noise_floor: f32,
    /// LFO: 1 = triangle, > 1 = trapezoid, 0 = sine
    stock_shape: f32,
    dual_line: bool,
    flanger: bool,
    wet_only: bool,
    fb_max: f32,
    cross_feed: f32,
    sat_amt: f32,

    // ---- effective (after knobs)
    lfo_inc: f32,
    lfo_phase: f32,
    lfo_shape: f32,
    base_samples: f32,
    depth_samples: f32,

    in_hp_coef: f32,
    out_hp_coef: f32,
    env_coef: f32,
    fs: f32,
    sr: f64,
    rng_state: i32,

    /// Rust addition: the host tempo (0 until the host reports a valid one).
    host_bpm: f32,
}

impl Default for ModEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl ModEngineDsp {
    /// `ModEngineDSP() { prepare (48000.0); }`, with every member at its declared initial value.
    pub fn new() -> Self {
        let mut d = ModEngineDsp {
            ch: [Channel::default(), Channel::default()],
            depth_knob: 0.5,
            rate_knob: 0.5,
            level_knob: 0.7071,
            circuit_idx: 0,
            variant_idx: 1,
            stereo_idx: 1,
            shape_idx: 0,
            sync_idx: 0,
            div_idx: 5,
            bypassed: false,
            trim_db: 0.0,
            trim_lin: 1.0,
            trim_sm: 1.0,
            linked: false,
            link_lo_val: 0.7071,
            link_hi_val: 0.7071,
            mix_knob: 0.5,
            width_knob: 0.5,
            regen_knob: 0.0,
            drive_knob: 0.0,
            noise_knob: 0.35,
            bias_knob: 0.5,
            tone_knob: 0.5,
            tempo_knob: 0.4,
            depth_sm: 0.5,
            rate_sm: 0.5,
            level_sm: 1.0,
            mix_sm: 0.5,
            width_sm: 0.5,
            regen_sm: 0.0,
            tone_counter: 0,
            family: 0,
            stages: 1024.0,
            base_ms: 7.5,
            depth_ms: 1.4,
            aa_hz: 9000.0,
            rec_hz: 9000.0,
            eta: 0.99965,
            ct_loss_g: 0.0,
            noise_floor: 0.00025,
            stock_shape: 1.4,
            dual_line: false,
            flanger: false,
            wet_only: false,
            fb_max: 0.0,
            cross_feed: 0.0,
            sat_amt: 0.15,
            lfo_inc: 0.0,
            lfo_phase: 0.0,
            lfo_shape: 1.4,
            base_samples: 0.0,
            depth_samples: 0.0,
            in_hp_coef: 0.0,
            out_hp_coef: 0.0,
            env_coef: 0.0,
            fs: 48000.0,
            sr: 48000.0,
            rng_state: 90210,
            host_bpm: 0.0,
        };
        d.prepare(48000.0);
        d
    }

    /// Puts the engine back exactly as [`ModEngineDsp::new`] makes it, in place (no allocation,
    /// no large temporary): every member at its declared initial value, then the constructor's
    /// `prepare (48000.0)`. The host tempo is the host's, not the engine's, and is kept.
    pub fn restore_initial(&mut self) {
        let host_bpm = self.host_bpm;
        self.depth_knob = 0.5;
        self.rate_knob = 0.5;
        self.level_knob = 0.7071;
        self.circuit_idx = 0;
        self.variant_idx = 1;
        self.stereo_idx = 1;
        self.shape_idx = 0;
        self.sync_idx = 0;
        self.div_idx = 5;
        self.bypassed = false;
        self.trim_db = 0.0;
        self.trim_lin = 1.0;
        self.trim_sm = 1.0;
        self.linked = false;
        self.link_lo_val = 0.7071;
        self.link_hi_val = 0.7071;
        self.mix_knob = 0.5;
        self.width_knob = 0.5;
        self.regen_knob = 0.0;
        self.drive_knob = 0.0;
        self.noise_knob = 0.35;
        self.bias_knob = 0.5;
        self.tone_knob = 0.5;
        self.tempo_knob = 0.4;
        self.depth_sm = 0.5;
        self.rate_sm = 0.5;
        self.level_sm = 1.0;
        self.mix_sm = 0.5;
        self.width_sm = 0.5;
        self.regen_sm = 0.0;
        self.tone_counter = 0;
        self.family = 0;
        self.stages = 1024.0;
        self.base_ms = 7.5;
        self.depth_ms = 1.4;
        self.aa_hz = 9000.0;
        self.rec_hz = 9000.0;
        self.eta = 0.99965;
        self.ct_loss_g = 0.0;
        self.noise_floor = 0.00025;
        self.stock_shape = 1.4;
        self.dual_line = false;
        self.flanger = false;
        self.wet_only = false;
        self.fb_max = 0.0;
        self.cross_feed = 0.0;
        self.sat_amt = 0.15;
        self.lfo_inc = 0.0;
        self.lfo_phase = 0.0;
        self.lfo_shape = 1.4;
        self.base_samples = 0.0;
        self.depth_samples = 0.0;
        self.in_hp_coef = 0.0;
        self.out_hp_coef = 0.0;
        self.env_coef = 0.0;
        self.fs = 48000.0;
        self.sr = 48000.0;
        self.rng_state = 90210;
        for k in &mut self.ch {
            // Every member `prepare` leaves alone is a biquad, which `apply_model` rebuilds.
            k.aa = [Biquad::default(); 2];
            k.recon = [Biquad::default(); 2];
        }
        self.prepare(48000.0);
        self.host_bpm = host_bpm;
    }

    /// The smoothers and the values they head for (tests): trim, depth, rate, mix, width, regen,
    /// level.
    #[cfg(test)]
    pub(crate) fn smoothers(&self) -> ([f32; 7], [f32; 7]) {
        (
            [self.trim_sm, self.depth_sm, self.rate_sm, self.mix_sm, self.width_sm, self.regen_sm, self.level_sm],
            [self.trim_lin, self.depth_knob, self.rate_knob, self.mix_knob, self.width_knob, self.regen_knob, knob_to_level_gain(self.level_knob)],
        )
    }

    /// What this instance's LFO is actually running at (`getRateHz`).
    pub fn get_rate_hz(&self) -> f32 {
        self.lfo_inc * self.fs
    }

    /// The LFO phase (0..1) the next sample starts from (left side; the right is offset by Width).
    pub fn lfo_phase(&self) -> f32 {
        self.lfo_phase
    }

    /// The LFO shape factor in use (`lfoShape`).
    pub fn lfo_shape(&self) -> f32 {
        self.lfo_shape
    }

    pub fn sample_rate(&self) -> f64 {
        self.sr
    }

    /// Rust addition (plan A3), as `DelayEngineDSP::setHostTransport`: a tempo outside 20..999
    /// BPM (or none) reads as 0, "no host tempo".
    pub fn set_host_transport(&mut self, _playing: bool, bpm: f32) {
        self.host_bpm = if (20.0..=999.0).contains(&bpm) { bpm } else { 0.0 };
    }

    pub fn host_bpm(&self) -> f32 {
        self.host_bpm
    }

    // ------------------------------------------------------------------ events
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        match index {
            P_DEPTH => self.depth_knob = v,
            P_RATE => self.rate_knob = v,
            P_LEVEL => self.level_knob = v,
            P_CIRCUIT => {
                self.circuit_idx = (v + 0.5) as i32;
                self.apply_model();
            }
            P_VARIANT => {
                self.variant_idx = (v + 0.5) as i32;
                self.apply_model();
            }
            P_BYPASS => self.bypassed = v > 0.5,
            P_TRIM => {
                self.trim_db = knob_to_trim_db(v);
                self.trim_lin = 10.0f32.powf(self.trim_db / 20.0);
            }
            P_LINK => self.linked = v > 0.5,
            P_LINK_LO => self.link_lo_val = v,
            P_LINK_HI => self.link_hi_val = v,
            P_MIX => self.mix_knob = v,
            P_WIDTH => self.width_knob = v,
            P_REGEN => self.regen_knob = v,
            P_DRIVE => self.drive_knob = v,
            P_NOISE => self.noise_knob = v,
            P_BIAS => self.bias_knob = v,
            P_TONE => self.tone_knob = v,
            P_STEREO => self.stereo_idx = (v + 0.5) as i32,
            P_SHAPE => {
                self.shape_idx = (v + 0.5) as i32;
                self.apply_model();
            }
            P_SYNC => self.sync_idx = (v + 0.5) as i32,
            P_DIVISION => self.div_idx = (v + 0.5) as i32,
            P_TEMPO => self.tempo_knob = v,
            _ => {}
        }
    }

    // ------------------------------------------------------------------ init
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = sample_rate as f32;
        self.sr = sample_rate;

        for k in &mut self.ch {
            k.buf.fill(0.0);
            k.write_pos = 0;
            k.env_in = 0.0;
            k.env_out = 0.0;
            k.ct_loss = 0.0;
            k.bw_state = 0.0;
            k.noise_lp = 0.0;
            k.fb_state = 0.0;
            k.ph_fb = 0.0;
            k.colour = 0.0;
            k.lamp = 0.3;
            k.lamp_slow = 0.3;
            k.ldr = 1.0;
            k.h_bass = 0.0;
            k.out_lp = 0.0;
            k.in_hp_x1 = 0.0;
            k.in_hp_y1 = 0.0;
            k.out_hp_x1 = 0.0;
            k.out_hp_y1 = 0.0;
            k.ap.fill(0.0);
            k.hts.fill(0.0);
            k.vs.fill(0.0);
        }

        // The LFO and the noise source are state, not settings: a host that re-prepares must get
        // the same audio as a fresh instance.
        self.lfo_phase = 0.0;
        self.tone_counter = 0;
        self.rng_state = 90210;

        self.in_hp_coef = one_pole_coef(16.0, self.sr);
        self.out_hp_coef = one_pole_coef(1.6, self.sr);
        self.env_coef = 1.0 - one_pole_coef(12.0, self.sr);

        self.depth_sm = self.depth_knob;
        self.rate_sm = self.rate_knob;
        self.mix_sm = self.mix_knob;
        self.width_sm = self.width_knob;
        self.regen_sm = self.regen_knob;
        self.level_sm = knob_to_level_gain(self.level_knob);
        self.trim_lin = 10.0f32.powf(self.trim_db / 20.0);
        self.trim_sm = self.trim_lin;
        self.apply_model();
    }

    // ------------------------------------------------------------------ main
    /// Processes `num_samples` frames of the first `num_channels` (at most 2) buffers in place.
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        if num_channels < 1 || io.is_empty() {
            return;
        }
        let (left, rest) = io.split_at_mut(1);
        let ch_l = &mut left[0];
        let mut ch_r = if num_channels > 1 { rest.first_mut() } else { None };
        let n_max = match &ch_r {
            Some(r) => num_samples.min(ch_l.len()).min(r.len()),
            None => num_samples.min(ch_l.len()),
        };

        for n in 0..n_max {
            let x0 = ch_l[n];
            let x1 = match &ch_r {
                Some(r) => r[n],
                None => x0,
            };

            if self.bypassed {
                ch_l[n] = x0;
                if let Some(r) = ch_r.as_deref_mut() {
                    r[n] = x1;
                }
                continue;
            }

            self.trim_sm += (self.trim_lin - self.trim_sm) * 0.005;
            self.depth_sm += (self.depth_knob - self.depth_sm) * 0.005;
            self.rate_sm += (self.rate_knob - self.rate_sm) * 0.005;
            self.mix_sm += (self.mix_knob - self.mix_sm) * 0.005;
            self.width_sm += (self.width_knob - self.width_sm) * 0.005;
            self.regen_sm += (self.regen_knob - self.regen_sm) * 0.005;
            let lt = knob_to_level_gain(self.level_knob);
            self.level_sm += (lt - self.level_sm) * 0.005;

            self.tone_counter += 1;
            if self.tone_counter >= 32 {
                self.tone_counter = 0;
                self.update_tone(self.rate_sm);
            }

            // one LFO; Width offsets the right side's phase (0 = mono mod, 5 = quadrature, 10 = anti-phase)
            self.lfo_phase += self.lfo_inc;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }

            let lfo_l;
            let lfo_r;
            let ph_r = self.lfo_phase + self.width_sm * 0.5;
            let p_r = ph_r - ph_r.floor();
            if self.lfo_shape <= 0.0 {
                lfo_l = (self.lfo_phase * 6.283_185_3).sin();
                lfo_r = (p_r * 6.283_185_3).sin();
            } else {
                let tri_l = 4.0 * (self.lfo_phase - 0.5).abs() - 1.0;
                let tri_r = 4.0 * (p_r - 0.5).abs() - 1.0;
                lfo_l = clamp_t(tri_l * self.lfo_shape, -1.0, 1.0);
                lfo_r = clamp_t(tri_r * self.lfo_shape, -1.0, 1.0);
            }

            let v_l = self.input_stage(0, x0);
            let v_r = self.input_stage(1, x1);

            let mut dry_l = v_l;
            let mut dry_r = v_r;
            if self.stereo_idx == 0 {
                let m = 0.5 * (v_l + v_r);
                dry_l = m;
                dry_r = m;
            }

            let y_l;
            let y_r;

            if self.family == 0 {
                if self.dual_line {
                    // Dimension D: anti-phase lines, own wet in phase, other side inverted and cross-fed
                    let wet_a = self.bbd_line(0, dry_l, lfo_l, 1.0);
                    let wet_b = self.bbd_line(1, dry_r, -lfo_l, LINE_B_OFFSET);
                    let m = self.mix_sm * 2.0 * WET_STOCK;
                    let w = self.width_sm * 2.0 * CROSS_STOCK;
                    y_l = dry_l + m * (wet_a - w * wet_b);
                    y_r = dry_r + m * (wet_b - w * wet_a);
                } else {
                    let wet_a = self.bbd_line(0, dry_l, lfo_l, 1.0);
                    let wet_b = self.bbd_line(1, dry_r, lfo_r, 1.0);
                    // cross-feed (JC-120 / Brigade) scales with Width
                    let xf = self.cross_feed * self.width_sm * 2.0;
                    let w_a = wet_a + (wet_b - wet_a) * min_t(xf, 1.0) * 0.5;
                    let w_b = wet_b + (wet_a - wet_b) * min_t(xf, 1.0) * 0.5;
                    let m = self.mix_sm * 2.0;
                    if self.wet_only {
                        y_l = w_a * m;
                        y_r = w_b * m;
                    } else {
                        y_l = dry_l + w_a * m;
                        y_r = dry_r + w_b * m;
                    }
                }
            } else if self.family == 1 {
                // phaser sums sit ~5 dB under the BBD paths; make-up so Level reads the same across circuits
                let w_l = self.phase_sample(0, dry_l, lfo_l) * 1.8;
                let w_r = self.phase_sample(1, dry_r, lfo_r) * 1.8;
                let m = self.mix_sm * 2.0;
                y_l = dry_l + (w_l - dry_l) * min_t(m, 1.0);
                y_r = dry_r + (w_r - dry_r) * min_t(m, 1.0);
            } else {
                let w_l = self.trem_sample(0, dry_l, lfo_l) * 1.5;
                let w_r = self.trem_sample(1, dry_r, lfo_r) * 1.5;
                let m = self.mix_sm * 2.0;
                y_l = dry_l + (w_l - dry_l) * min_t(m, 1.0);
                y_r = dry_r + (w_r - dry_r) * min_t(m, 1.0);
            }

            ch_l[n] = self.output_stage(0, y_l);
            if let Some(r) = ch_r.as_deref_mut() {
                r[n] = self.output_stage(1, y_r);
            }
        }
    }

    // ------------------------------------------------------------------ shared helpers
    fn allpass1(&self, x: f32, s: &mut f32, f_hz: f32) -> f32 {
        let f = clamp_t(f_hz, 20.0, self.fs * 0.45);
        let g = (PI64 as f32 * f / self.fs).tan();
        let a = (g - 1.0) / (g + 1.0);
        let v = a * x + *s;
        *s = x - a * v;
        v
    }

    fn next_noise(&mut self) -> f32 {
        // Signed 32-bit overflow: MSVC wraps.
        self.rng_state = self.rng_state.wrapping_mul(1_103_515_245).wrapping_add(12345) & 0x7fff_ffff;
        ((self.rng_state % 20000) as f32 / 10000.0) - 1.0
    }

    // ------------------------------------------------------------------ circuit table
    fn apply_model(&mut self) {
        let c = self.circuit_idx;
        self.family = if c <= 7 {
            0
        } else if c <= 10 {
            1
        } else {
            2
        };
        self.dual_line = false;
        self.flanger = false;
        self.wet_only = false;
        self.fb_max = 0.0;
        self.cross_feed = 0.0;
        self.sat_amt = 0.15;
        self.stock_shape = 0.0;

        if c == 0 {
            // Dimension D -- validated mode table (SDD-320 service notes + UAD A/B), variant = button
            self.stages = 1024.0;
            self.aa_hz = 9000.0;
            self.rec_hz = 9000.0;
            self.eta = 0.99965;
            self.noise_floor = 0.00025;
            self.dual_line = true;
            self.cross_feed = CROSS_STOCK;
            self.sat_amt = 0.10;
            let (b, d, s) = if self.variant_idx == 0 {
                (10.0, 0.8, 1.4)
            } else if self.variant_idx == 1 {
                (7.5, 1.4, 1.4)
            } else if self.variant_idx == 2 {
                (7.5, 1.0, 1.3)
            } else if self.variant_idx == 3 {
                (7.5, 1.5, 1.0)
            } else if self.variant_idx == 4 {
                (8.5, 1.6, 1.2)
            } else {
                (7.5, 1.4, 1.3)
            };
            self.base_ms = b;
            self.depth_ms = d;
            self.stock_shape = s;
        } else if c == 1 {
            // CE-1: MN3002, preamp, its own noise floor; variant 1 = vibrato
            self.stages = 512.0;
            self.aa_hz = 6200.0;
            self.rec_hz = 5600.0;
            self.eta = 0.9992;
            self.noise_floor = 0.0009;
            self.base_ms = 6.0;
            self.depth_ms = 2.6;
            self.stock_shape = 0.0;
            self.sat_amt = 0.4;
            self.wet_only = self.variant_idx == 1;
        } else if c == 2 {
            // CE-2: MN3007
            self.stages = 1024.0;
            self.aa_hz = 7800.0;
            self.rec_hz = 7200.0;
            self.eta = 0.99955;
            self.noise_floor = 0.0004;
            self.base_ms = 5.5;
            self.depth_ms = 2.4;
            self.stock_shape = 0.0;
            self.sat_amt = 0.18;
        } else if c == 3 {
            // Small Clone: wide fixed depth, triangle
            self.stages = 1024.0;
            self.aa_hz = 6800.0;
            self.rec_hz = 6200.0;
            self.eta = 0.9994;
            self.noise_floor = 0.00055;
            self.base_ms = 7.0;
            self.depth_ms = 3.8;
            self.stock_shape = 1.0;
            self.sat_amt = 0.22;
        } else if c == 4 {
            // JC-120: MN3002 pair, cross-fed; variant 1 = vibrato
            self.stages = 512.0;
            self.aa_hz = 7200.0;
            self.rec_hz = 6800.0;
            self.eta = 0.99935;
            self.noise_floor = 0.0005;
            self.base_ms = 5.5;
            self.depth_ms = 2.2;
            self.stock_shape = 0.0;
            self.sat_amt = 0.2;
            self.cross_feed = 0.4;
            self.wet_only = self.variant_idx == 1;
        } else if c == 5 {
            // Brigade: generic MN3007, dark, wide, noisy
            self.stages = 1024.0;
            self.aa_hz = 5400.0;
            self.rec_hz = 4800.0;
            self.eta = 0.9990;
            self.noise_floor = 0.0007;
            self.base_ms = 8.5;
            self.depth_ms = 3.2;
            self.stock_shape = 0.0;
            self.sat_amt = 0.3;
            self.cross_feed = 0.55;
        } else if c == 6 {
            // Electric Mistress: SAD1024 flanger
            self.stages = 512.0;
            self.aa_hz = 9500.0;
            self.rec_hz = 9000.0;
            self.eta = 0.9988;
            self.noise_floor = 0.0008;
            self.base_ms = 3.2;
            self.depth_ms = 2.8;
            self.stock_shape = 1.0;
            self.sat_amt = 0.25;
            self.flanger = true;
            self.fb_max = 0.88;
        } else if c == 7 {
            // A/DA: SAD1024, huge sweep, high regen
            self.stages = 512.0;
            self.aa_hz = 11000.0;
            self.rec_hz = 10000.0;
            self.eta = 0.9986;
            self.noise_floor = 0.0009;
            self.base_ms = 5.5;
            self.depth_ms = 5.2;
            self.stock_shape = 1.0;
            self.sat_amt = 0.2;
            self.flanger = true;
            self.fb_max = 0.95;
        }
        // Circuits 8..13 (the phase and tremolo families) have nothing to set here.

        // LFO shape: Stock uses the circuit's own; the switch overrides
        self.lfo_shape = self.stock_shape;
        if self.shape_idx == 1 {
            self.lfo_shape = 0.0;
        } else if self.shape_idx == 2 {
            self.lfo_shape = 1.0;
        } else if self.shape_idx == 3 {
            self.lfo_shape = 1.4;
        } else if self.shape_idx == 4 {
            self.lfo_shape = 8.0;
        }

        // eta^stages is fixed by the circuit, so it is hoisted out of the per-sample path.
        self.ct_loss_g = 1.0 - self.eta.powf(self.stages);

        // BBD filters: fixed per-circuit corner (the clock tracking is on top, in the loop)
        for k in &mut self.ch {
            k.aa[0] = butter_lp(f64::from(self.aa_hz), self.sr, 0.5412);
            k.aa[1] = butter_lp(f64::from(self.aa_hz), self.sr, 1.3066);
            k.recon[0] = butter_lp(f64::from(self.rec_hz), self.sr, 0.5412);
            k.recon[1] = butter_lp(f64::from(self.rec_hz), self.sr, 1.3066);
        }

        self.update_tone(self.rate_sm);
    }

    // ------------------------------------------------------------------ rate / depth scaling
    // Rate knob: 5 = stock rate, sweeping exponentially to the circuit's two ends.
    // Depth knob: 0 -> 0x, 5 -> 1x, 10 -> 2x of the circuit's stock depth (BBD); 0..1 sweep elsewhere.
    fn update_tone(&mut self, r: f32) {
        self.lfo_inc = lfo_rate_hz_host(self.circuit_idx, self.variant_idx, r, self.sync_idx, self.div_idx, self.tempo_knob, self.host_bpm) / self.fs;

        self.depth_samples = self.depth_ms * self.depth_sm * 2.0 * self.fs * 0.001;
        self.base_samples = self.base_ms * self.fs * 0.001;

        let max_depth = self.base_samples - 4.0;
        let max_buf = BUF as f32 - self.base_samples * LINE_B_OFFSET - 8.0;
        if self.depth_samples > max_depth {
            self.depth_samples = max_depth;
        }
        if self.depth_samples > max_buf {
            self.depth_samples = max_buf;
        }
    }

    // ------------------------------------------------------------------ BBD read: 4-point Hermite
    fn read_delay(c: &Channel, delay_samples: f32) -> f32 {
        let d = clamp_t(delay_samples, 2.0, BUF as f32 - 4.0);
        let di = d as i32;
        let fr = d - di as f32;
        let mut i1 = c.write_pos - di;
        if i1 < 0 {
            i1 += BUF_I;
        }
        let mut i0 = i1 + 1;
        if i0 >= BUF_I {
            i0 -= BUF_I;
        }
        let mut i2 = i1 - 1;
        if i2 < 0 {
            i2 += BUF_I;
        }
        let mut i3 = i2 - 1;
        if i3 < 0 {
            i3 += BUF_I;
        }
        let at = |i: i32| c.buf.get(i as usize).copied().unwrap_or(0.0);
        let y0 = at(i0);
        let y1 = at(i1);
        let y2 = at(i2);
        let y3 = at(i3);
        let c0 = y1;
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * fr + c2) * fr + c1) * fr + c0
    }

    // ------------------------------------------------------------------ one BBD line (volts in / volts out)
    fn bbd_line(&mut self, ci: usize, x: f32, lfo: f32, base_mul: f32) -> f32 {
        // delay for this sample, and the clock it implies
        let d = self.base_samples * base_mul + self.depth_samples * lfo;
        let fclk = self.stages * self.fs / (2.0 * max_t(d, 2.0));
        // BBD bandwidth follows the clock: extra one-pole at ~fclk/4 on top of the fixed filters
        let bw_hz = clamp_t(fclk * 0.25, 800.0, self.fs * 0.45);
        let bw_g = 1.0 - (-6.283_185_3 * bw_hz / self.fs).exp();
        let (regen_sm, fb_max, env_coef, ct_loss_g, noise_floor, noise_knob) =
            (self.regen_sm, self.fb_max, self.env_coef, self.ct_loss_g, self.noise_floor, self.noise_knob);
        // `nextNoise()` is drawn here rather than after the delay read only for the borrow:
        // nothing in between touches the generator, so the sequence is the C++'s.
        let noise = self.next_noise();

        let c = &mut self.ch[ci];
        let mut u = x + c.fb_state * regen_sm * fb_max;
        for q in &mut c.aa {
            u = filter(q, u);
        }
        c.bw_state += (u - c.bw_state) * bw_g;
        u = c.bw_state;

        // NE570 compressor: 2:1 in dB around COMP_REF
        c.env_in += (u.abs() - c.env_in) * env_coef;
        let g_comp = min_t((COMP_REF / max_t(c.env_in, 1.0e-4)).sqrt(), 20.0);
        let mut v = u * g_comp;

        // BBD input: limited charge well
        v = BBD_CLIP * (v / BBD_CLIP).tanh();
        if let Some(slot) = c.buf.get_mut(c.write_pos as usize) {
            *slot = v;
        }
        let mut delayed = Self::read_delay(c, d);

        // charge transfer inefficiency: eta^N droop
        c.ct_loss += (delayed - c.ct_loss) * ct_loss_g;
        delayed = c.ct_loss;

        // clock feedthrough / BBD noise, before the expander so it breathes like the real thing
        let nf = noise_floor * (0.2 + noise_knob * 2.5) * COMP_REF;
        c.noise_lp += (noise * nf - c.noise_lp) * 0.35;
        delayed += c.noise_lp;

        // NE570 expander
        c.env_out += (delayed.abs() - c.env_out) * env_coef;
        let g_exp = min_t(c.env_out / COMP_REF, 20.0);
        let mut w = delayed * g_exp;

        for q in &mut c.recon {
            w = filter(q, w);
        }

        c.write_pos += 1;
        if c.write_pos >= BUF_I {
            c.write_pos = 0;
        }
        c.fb_state = w;
        w
    }

    // ------------------------------------------------------------------ phase family (volts in / volts out)
    fn phase_sample(&mut self, ci: usize, x: f32, lfo: f32) -> f32 {
        let c_idx = self.circuit_idx;
        let dep = self.depth_sm;
        let mut ap = self.ch[ci].ap;

        let out = if c_idx == 8 {
            // Phase 90: Script (variant 0) = no R28; Block (variant 1) = R28 in, hotter stages
            let block = self.variant_idx == 1;
            let fb_amt = if block { 0.28 + self.regen_sm * 0.45 } else { 0.0 };
            // control voltage: Bias sets the centre of the sweep, Depth the swing (Vgs/Vp, 0..1)
            let ctrl = 0.3 + self.bias_knob * 0.4 + 0.5 * lfo * dep;
            let mut y = x + self.ch[ci].ph_fb * fb_amt;
            for s in ap.iter_mut().take(4) {
                // JFET as VCR: Rds = Ron / (1 - Vgs/Vp); the audio riding on the channel modulates it
                let vgs = clamp_t(ctrl + y * 3.0, 0.03, 0.97);
                let rds = 1.0 / (1.0 - vgs);
                y = self.allpass1(y, s, 2600.0 / rds);
                if block {
                    y = y - 0.06 * y * y;
                }
            }
            self.ch[ci].ph_fb = y;
            (x + y) * 0.5
        } else if c_idx == 9 {
            // Small Stone: OTA stages, pole linear in bias current; Colour (variant 1) = resonator round last two
            let colour_on = self.variant_idx == 1;
            let ctrl = clamp_t(0.15 + self.bias_knob * 0.4 + 0.5 * lfo * dep, 0.0, 1.0);
            let fp = 150.0 + 2250.0 * ctrl;
            let fb_amt = if colour_on { 0.45 + self.regen_sm * 0.3 } else { self.regen_sm * 0.35 };
            let mut y = x + self.ch[ci].ph_fb * fb_amt;
            for (s, st) in ap.iter_mut().take(4).enumerate() {
                y = self.allpass1(y, st, fp);
                if colour_on && s == 1 {
                    let c = &mut self.ch[ci];
                    c.colour += (y - c.colour) * 0.25;
                    y += c.colour * 0.3;
                }
            }
            self.ch[ci].ph_fb = y;
            (x + y) * 0.5
        } else {
            // Uni-Vibe: lamp (filament + thermal tail) -> LDR power law -> four stages, four different caps
            let drive_v = 0.5 + 0.5 * lfo * dep;
            let bright;
            {
                let c = &mut self.ch[ci];
                c.lamp += (drive_v - c.lamp) * 0.020;
                c.lamp_slow += (c.lamp - c.lamp_slow) * 0.0035;
                bright = c.lamp * 0.4 + c.lamp_slow * 0.6;
            }
            let r = max_t(0.02, bright).powf(-1.6) * (0.6 + self.bias_knob * 0.8);

            let mut y = x;
            for (s, st) in ap.iter_mut().take(4).enumerate() {
                let cap = if s == 0 {
                    0.015
                } else if s == 1 {
                    0.22
                } else if s == 2 {
                    0.047
                } else {
                    0.0047
                };
                y = self.allpass1(y, st, 1.0 / (6.283_185_3 * r * cap * 0.004));
            }
            // Chorus (variant 0): unmatched blend; Vibrato (variant 1): wet only
            let w = if self.variant_idx == 1 { y } else { x * 0.45 + y * 0.55 };
            let c = &mut self.ch[ci];
            c.out_lp += (w - c.out_lp) * 0.45;
            c.out_lp
        };
        self.ch[ci].ap = ap;
        out
    }

    // ------------------------------------------------------------------ trem / vibrato family
    fn trem_sample(&mut self, ci: usize, x: f32, lfo: f32) -> f32 {
        let c_idx = self.circuit_idx;
        let dep = self.depth_sm;

        if c_idx == 11 {
            // Opto trem: neon strike point moves with Intensity; LDR lights fast, darkens slow; shunt divider
            let strike = 0.75 - dep * 0.62;
            let dv = (0.5 + 0.5 * lfo) - strike;
            let mut lamp = 0.0f32;
            if dv > 0.0 {
                let n = dv / max_t(0.05, 1.0 - strike);
                lamp = min_t(1.0, n * n * 1.4);
            }
            let c = &mut self.ch[ci];
            c.ldr += (lamp - c.ldr) * if lamp > c.ldr { 0.020 } else { 0.0035 };
            let r_ldr = max_t(0.004, c.ldr).powf(-0.85);
            x * (r_ldr / (1.0 + r_ldr))
        } else if c_idx == 12 {
            // Harmonic trem: 3-section phase network, sum/difference bands, triodes driven counter-phase
            let centre = 260.0 * 4.0f32.powf(self.bias_knob);
            let mut hts = self.ch[ci].hts;
            let mut ap = x;
            for (s, st) in hts.iter_mut().enumerate() {
                ap = self.allpass1(ap, st, centre * (0.6 + 0.55 * s as f32));
            }
            self.ch[ci].hts = hts;
            let bass = (x + ap) * 0.5;
            let treb = (x - ap) * 0.5;
            let g_a = 1.0 - dep * (0.5 - 0.5 * lfo);
            let g_b = 1.0 - dep * (0.5 + 0.5 * lfo);
            let w = bass * g_a + treb * g_b;
            let t = 0.4 + self.tone_knob * 0.6;
            let c = &mut self.ch[ci];
            c.h_bass += (w - c.h_bass) * 0.55;
            c.h_bass + (w - c.h_bass) * t
        } else {
            // Magnatone: varistor bridge sweeping six phase sections -- true FM, no delay line
            let v = (self.bias_knob * 0.5 + 0.5 * lfo * dep).abs();
            let r = 1.0 / (0.12 + v * v * 2.6 + v * 0.5);
            let mut vs = self.ch[ci].vs;
            let mut y = x;
            for (s, st) in vs.iter_mut().enumerate() {
                let cap = if s == 0 {
                    0.022
                } else if s == 1 {
                    0.015
                } else if s == 2 {
                    0.010
                } else if s == 3 {
                    0.0068
                } else if s == 4 {
                    0.0047
                } else {
                    0.0033
                };
                y = self.allpass1(y, st, 1.0 / (6.283_185_3 * r * cap * 0.02));
            }
            self.ch[ci].vs = vs;
            let w = if self.variant_idx == 1 { y } else { x * 0.4 + y * 0.6 };
            let tone = self.tone_knob;
            let c = &mut self.ch[ci];
            c.out_lp += (w - c.out_lp) * (0.25 + tone * 0.6);
            c.out_lp
        }
    }

    // ------------------------------------------------------------------ input / output buffers (shared)
    fn input_stage(&mut self, ci: usize, x: f32) -> f32 {
        let (in_hp_coef, trim_sm, drive_knob, sat_amt) = (self.in_hp_coef, self.trim_sm, self.drive_knob, self.sat_amt);
        let c = &mut self.ch[ci];
        let hp_in = x - c.in_hp_x1 + in_hp_coef * c.in_hp_y1;
        c.in_hp_x1 = x;
        c.in_hp_y1 = hp_in;
        let mut v = hp_in * trim_sm * INPUT_VOLTS as f32;
        if drive_knob > 0.001 {
            v = soft_sat(v * (1.0 + drive_knob * 4.0) / INPUT_VOLTS as f32, sat_amt) * INPUT_VOLTS as f32 / (1.0 + drive_knob * 1.8);
        }
        v
    }

    fn output_stage(&mut self, ci: usize, y: f32) -> f32 {
        let y = y * self.level_sm;
        let out_hp_coef = self.out_hp_coef;
        let c = &mut self.ch[ci];
        let o = y - c.out_hp_x1 + out_hp_coef * c.out_hp_y1;
        c.out_hp_x1 = y;
        c.out_hp_y1 = o;
        o * OUTPUT_SCALE as f32
    }
}
