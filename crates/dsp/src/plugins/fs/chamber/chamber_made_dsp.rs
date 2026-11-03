//! `ChamberMadeDSP.h`, translated line by line: a reverb chamber you build yourself.
//!
//! A speaker plays into a room and two microphones pick it up. EARLY is the image-source solve
//! in [`chamber_geometry`](super::chamber_geometry) (one tap per reflection per microphone), TAIL
//! a 16-line feedback delay network with line lengths from the room's proportions and per-band
//! decay from Eyring's equation, FLUTTER two combs (the closest facing pair of parallel walls,
//! and floor against ceiling), TAPE an optional machine ahead of the speaker.
//!
//! Translation notes (C++ semantics reproduced deliberately):
//! - every `float` expression keeps the C++ order of operations; the filter designs are in
//!   `double` as in the C++; `(int) x` is `as i32` (truncation; every value is bounded, so the
//!   saturation of `as` never differs from the C++);
//! - the ring buffers are the C++ power-of-two sizes, independent of the sample rate, so they are
//!   allocated once (at the first `prepare`) and only zeroed afterwards: `process` and a
//!   re-`prepare` (the plugin's `reset`) never allocate;
//! - the C++ `std::atomic` outputs to the editor are plain fields (the Rust host reads them
//!   after `process` through `Plugin::readouts`);
//! - the room rebuild (`updateGeometry`) runs on the audio thread exactly as in the C++, on the
//!   same 64-sample control tick and four-tick cooldown; the geometry is fixed-size and is
//!   rebuilt in place, so it never allocates.

// The C++ constants and loops are kept verbatim for line-by-line review: 0.7071 is the C++
// literal (not 1/sqrt 2), `(i / 1) % 2` its sign pattern, indexed loops its loops.
#![allow(clippy::approx_constant, clippy::identity_op, clippy::needless_range_loop, clippy::manual_clamp, clippy::assign_op_pattern)]

use super::chamber_geometry::{
    self as geo, Band3, Chamber, Knobs, MAX_OBJECTS, NUM_TAPS, RES_BARREL, RES_NONE, SPEED_OF_SOUND, SURF_CEILING, SURF_FLOOR, TAPS_PER_MIC, clamp_f,
    fmax, fmin, len3,
};

// ------------------------------------------------------------------ parameters
pub const P_MIX: usize = 0;
pub const P_DECAY: usize = 1;
pub const P_PRE_DELAY: usize = 2;
pub const P_EARLY: usize = 3;
pub const P_TAIL: usize = 4;
pub const P_LOW_CUT: usize = 5;
pub const P_HIGH_CUT: usize = 6;
pub const P_WIDTH: usize = 7;
pub const P_LEVEL: usize = 8;
pub const P_BYPASS: usize = 9;
pub const P_DRIVE: usize = 10;
pub const P_DRIFT: usize = 11;
pub const P_SHAPE: usize = 12;
pub const P_ROOM_W: usize = 13;
pub const P_ROOM_L: usize = 14;
pub const P_ROOM_H: usize = 15;
pub const P_TAPER: usize = 16;
pub const P_WALL_MAT: usize = 17;
pub const P_FLOOR_MAT: usize = 18;
pub const P_CEIL_MAT: usize = 19;
pub const P_SPK_X: usize = 20;
pub const P_SPK_Y: usize = 21;
pub const P_SPK_Z: usize = 22;
pub const P_SPK_AIM: usize = 23;
pub const P_SPK_TYPE: usize = 24;
pub const P_MIC_AX: usize = 25;
pub const P_MIC_AY: usize = 26;
pub const P_MIC_BX: usize = 27;
pub const P_MIC_BY: usize = 28;
pub const P_MIC_Z: usize = 29;
pub const P_MIC_A_AIM: usize = 30;
pub const P_MIC_B_AIM: usize = 31;
pub const P_MIC_PAT: usize = 32;
pub const P_MIC_TYPE: usize = 33;
/// `pObj1Type`; slot `o` is `P_OBJ1_TYPE + o * 4` (type, x, y, size).
pub const P_OBJ1_TYPE: usize = 34;
pub const P_OBJ8_SIZE: usize = 65;
pub const P_CEIL_SLOPE: usize = 66;
pub const P_TAPE_SEND: usize = 67;
pub const P_TAPE_TIME: usize = 68;
pub const P_TAPE_REPEATS: usize = 69;
pub const P_TAPE_TONE: usize = 70;
/// `pObj1Rot`; slot `o` is `P_OBJ1_ROT + o`.
pub const P_OBJ1_ROT: usize = 71;
pub const P_OBJ8_ROT: usize = 78;
pub const NUM_PARAMS: usize = 79;

pub const NUM_OBJECTS: usize = MAX_OBJECTS;
pub const NUM_LINES: usize = 16;

// Buffer sizes, as powers of two so the wrap is a mask (the C++'s, independent of the rate).
pub const ER_SIZE: i32 = 131_072;
pub const ER_MASK: i32 = ER_SIZE - 1;
pub const PD_SIZE: i32 = 65_536;
pub const PD_MASK: i32 = PD_SIZE - 1;
pub const FL_SIZE: i32 = 65_536;
pub const FL_MASK: i32 = FL_SIZE - 1;
pub const AP_SIZE: i32 = 4096;
pub const AP_MASK: i32 = AP_SIZE - 1;
pub const TP_SIZE: i32 = 131_072;
pub const TP_MASK: i32 = TP_SIZE - 1;

/// `static constexpr double pi`.
const PI_D: f64 = std::f64::consts::PI;
/// `(float) pi`.
const PI_F: f32 = std::f32::consts::PI;

// ------------------------------------------------------------------ knob -> unit maps
fn clamp01(v: f32) -> f32 {
    if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

/// 1.5..30 m.
pub fn knob_to_room_w(v: f32) -> f32 {
    1.5 * 20.0f32.powf(clamp01(v))
}
/// 1.5..15 m.
pub fn knob_to_room_h(v: f32) -> f32 {
    1.5 * 10.0f32.powf(clamp01(v))
}
pub fn knob_to_pre_delay_ms(v: f32) -> f32 {
    200.0 * v * v
}
pub fn knob_to_low_cut_hz(v: f32) -> f32 {
    20.0 * 50.0f32.powf(clamp01(v))
}
pub fn knob_to_high_cut_hz(v: f32) -> f32 {
    1000.0 * 20.0f32.powf(clamp01(v))
}
pub fn knob_to_tape_ms(v: f32) -> f32 {
    30.0 * 16.6667f32.powf(clamp01(v))
}
pub fn knob_to_tape_tone_hz(v: f32) -> f32 {
    1500.0 * 8.0f32.powf(clamp01(v))
}
pub fn knob_to_decay_mul(v: f32) -> f32 {
    4.0f32.powf(2.0 * clamp01(v) - 1.0)
}
/// Early and Tail are fourth-power faders where 0.7071 is unity and 1.0 is +12 dB.
pub fn knob_to_early_gain(v: f32) -> f32 {
    let q = v * v;
    4.0 * q * q
}
pub fn knob_to_level_gain(v: f32) -> f32 {
    2.0 * v * v
}

fn is_object_param(index: usize) -> bool {
    (P_OBJ1_TYPE..=P_OBJ8_SIZE).contains(&index) || (P_OBJ1_ROT..=P_OBJ8_ROT).contains(&index)
}

/// `std::lround` on a parameter value (every value reaching here is finite and bounded).
fn lround(v: f32) -> i32 {
    v.round() as i32
}

/// `defaultKnobs()`.
pub fn default_knobs() -> [f32; NUM_PARAMS] {
    let mut k = [0.0f32; NUM_PARAMS];
    k[P_MIX] = 0.35;
    k[P_DECAY] = 0.5;
    k[P_PRE_DELAY] = 0.0;
    k[P_EARLY] = 0.7071;
    k[P_TAIL] = 0.7071;
    k[P_LOW_CUT] = 0.1772;
    k[P_HIGH_CUT] = 1.0;
    k[P_WIDTH] = 0.5;
    k[P_LEVEL] = 0.7071;
    k[P_BYPASS] = 0.0;
    k[P_DRIVE] = 0.0;
    k[P_DRIFT] = 0.2;

    k[P_SHAPE] = 5.0;
    k[P_ROOM_W] = 0.3059;
    k[P_ROOM_L] = 0.4683;
    k[P_ROOM_H] = 0.3153;
    k[P_TAPER] = 0.7;

    k[P_SPK_X] = 0.25;
    k[P_SPK_Y] = 0.15;
    k[P_SPK_Z] = 0.3;
    k[P_SPK_AIM] = 0.625;
    k[P_SPK_TYPE] = 1.0;

    k[P_MIC_AX] = 0.3;
    k[P_MIC_AY] = 0.8;
    k[P_MIC_BX] = 0.7;
    k[P_MIC_BY] = 0.8;
    k[P_MIC_Z] = 0.45;
    k[P_MIC_A_AIM] = 0.375;
    k[P_MIC_B_AIM] = 0.125;

    for o in 0..NUM_OBJECTS {
        let b = P_OBJ1_TYPE + o * 4;
        k[b + 1] = 0.5;
        k[b + 2] = 0.5;
        k[b + 3] = 0.5;
    }

    k[P_CEIL_SLOPE] = 0.4;
    k[P_TAPE_SEND] = 0.0;
    k[P_TAPE_TIME] = 0.4927;
    k[P_TAPE_REPEATS] = 0.3;
    k[P_TAPE_TONE] = 0.6;
    k
}

/// `knobsToGeometry`. `k` shorter than [`NUM_PARAMS`] reads the defaults for what is missing.
pub fn knobs_to_geometry(k: &[f32]) -> Knobs {
    let defaults = default_knobs();
    let at = |i: usize| k.get(i).copied().unwrap_or(defaults[i]);
    let mut g = Knobs {
        shape: lround(at(P_SHAPE)),
        room_w: at(P_ROOM_W),
        room_l: at(P_ROOM_L),
        room_h: at(P_ROOM_H),
        taper: at(P_TAPER),
        ceil_slope: at(P_CEIL_SLOPE),
        wall_mat: lround(at(P_WALL_MAT)),
        floor_mat: lround(at(P_FLOOR_MAT)),
        ceil_mat: lround(at(P_CEIL_MAT)),
        spk_x: at(P_SPK_X),
        spk_y: at(P_SPK_Y),
        spk_z: at(P_SPK_Z),
        spk_aim: at(P_SPK_AIM),
        spk_type: lround(at(P_SPK_TYPE)),
        mic_ax: at(P_MIC_AX),
        mic_ay: at(P_MIC_AY),
        mic_bx: at(P_MIC_BX),
        mic_by: at(P_MIC_BY),
        mic_z: at(P_MIC_Z),
        mic_a_aim: at(P_MIC_A_AIM),
        mic_b_aim: at(P_MIC_B_AIM),
        mic_pattern: lround(at(P_MIC_PAT)),
        decay: at(P_DECAY),
        ..Knobs::default()
    };
    for o in 0..NUM_OBJECTS {
        let b = P_OBJ1_TYPE + o * 4;
        g.obj_type[o] = lround(at(b));
        g.obj_x[o] = at(b + 1);
        g.obj_y[o] = at(b + 2);
        g.obj_size[o] = at(b + 3);
        g.obj_rotate[o] = at(P_OBJ1_ROT + o);
    }
    g
}

/// `buildChamberFor`: the room a set of knobs WOULD make, without touching a DSP. The strip's
/// room plan draws from this, so the picture is the room the audio is in.
pub fn build_chamber_for(knobs: &[f32]) -> Chamber {
    geo::build(&knobs_to_geometry(knobs))
}

// ------------------------------------------------------------------ structures
#[derive(Debug, Clone, Copy, PartialEq)]
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

#[inline]
fn filter(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DspTap {
    g: Band3,
    g_t: Band3,
    d: f32,
    d_t: f32,
    live: bool,
}

impl Default for DspTap {
    fn default() -> Self {
        DspTap { g: Band3::default(), g_t: Band3::default(), d: 2.0, d_t: 2.0, live: false }
    }
}

#[derive(Debug, Clone)]
struct FLine {
    buf: Vec<f32>,
    d: f32,
    d_t: f32,
    lp1: f32,
    lp2: f32,
    k_l: f32,
    k_h: f32,
    g_m: f32,
    k_lt: f32,
    k_ht: f32,
    g_mt: f32,
    notch: Biquad,
    notch_on: bool,
}

impl Default for FLine {
    fn default() -> Self {
        FLine {
            buf: Vec::new(),
            d: 0.0,
            d_t: 0.0,
            lp1: 0.0,
            lp2: 0.0,
            k_l: 1.0,
            k_h: 1.0,
            g_m: 0.5,
            k_lt: 1.0,
            k_ht: 1.0,
            g_mt: 0.5,
            notch: Biquad::default(),
            notch_on: false,
        }
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buf: Vec<f32>,
    pos: i32,
    len: f32,
    len_t: f32,
}

impl Default for Allpass {
    fn default() -> Self {
        Allpass { buf: Vec::new(), pos: 0, len: 8.0, len_t: 8.0 }
    }
}

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    len: f32,
    len_t: f32,
    fb: f32,
    fb_t: f32,
    damp: f32,
    lp: f32,
    lvl: f32,
    lvl_t: f32,
}

impl Default for Comb {
    fn default() -> Self {
        Comb { buf: Vec::new(), len: 16.0, len_t: 16.0, fb: 0.0, fb_t: 0.0, damp: 0.5, lp: 0.0, lvl: 0.0, lvl_t: 0.0 }
    }
}

/// `readRing`.
#[inline]
fn read_ring(buf: &[f32], w: i32, d: f32, mask: i32) -> f32 {
    let di = d as i32;
    let fr = d - di as f32;
    let i1 = w.wrapping_sub(di) & mask;
    let i2 = (i1 - 1) & mask;
    buf[i1 as usize] * (1.0 - fr) + buf[i2 as usize] * fr
}

/// The spread the FDN line lengths are mixed with. Sixteen values from 0.55 to 1.58.
const SPREAD: [f32; 16] = [0.55, 0.61, 0.68, 0.75, 0.82, 0.89, 0.95, 1.00, 1.06, 1.12, 1.19, 1.26, 1.33, 1.41, 1.49, 1.58];

/// The ChamberMade engine (`ChamberMadeDSP`).
#[derive(Debug, Clone)]
pub struct ChamberMadeDsp {
    // ---- outputs to the editor
    pub rt_low_out: f32,
    pub rt_mid_out: f32,
    pub rt_high_out: f32,
    pub volume_out: f32,
    pub surface_out: f32,
    pub scatter_out: f32,
    pub taps_a_out: i32,
    pub taps_b_out: i32,
    pub flutter_out: f32,
    pub wet_out: f32,

    fs: f32,
    knob: [f32; NUM_PARAMS],

    chamber: Chamber,
    dirty_geo: bool,
    dirty_tone: bool,
    geo_cooldown: i32,

    // pre-delay and tape send
    pd_buf: Vec<f32>,
    tp_buf: Vec<f32>,
    pd_w: i32,
    tp_w: i32,
    pd_cur: f32,
    pd_target: f32,
    tp_cur: f32,
    tp_target: f32,
    tp_amt_sm: f32,
    tp_fb_sm: f32,
    tp_lp: f32,
    tp_hp_x: f32,
    tp_hp_y: f32,
    tp_lp_c: f32,
    tp_hp_c: f32,
    tp_wow: f32,
    tp_wow_t: f32,

    // the chamber amp
    sat_x1: f64,
    sat_f1: f64,
    sat_drive: f32,
    sat_makeup: f32,
    sat_on: bool,
    spk_hp: Biquad,
    spk_pk: Biquad,
    spk_lp: Biquad,
    bs1: f32,
    bs2: f32,
    bs_c1: f32,
    bs_c2: f32,

    // early reflections
    er_buf: Vec<Band3>,
    er_w: i32,
    taps: [DspTap; NUM_TAPS],
    g_slew: f32,
    d_slew: f32,

    // diffusion and the late field
    ap: [Allpass; 4],
    ap_g: f32,
    ap_gt: f32,
    mix_delay: f32,
    mix_delay_t: f32,

    fl: [FLine; NUM_LINES],
    f_w: i32,
    fv: [f32; NUM_LINES],
    in_sign: [f32; NUM_LINES],
    out_a: [f32; NUM_LINES],
    out_b: [f32; NUM_LINES],
    tail_gain: f32,
    tail_gain_t: f32,
    wet_norm: f32,
    wet_norm_t: f32,
    lfo_ph: f32,
    drift_samples: f32,
    drift_t: [f32; 4],
    drift_cur: [f32; 4],

    // flutter
    comb: [Comb; 2],
    comb_w: i32,
    comb_off_b: i32,

    // microphone coherence and the barrels
    side_hp_c: f32,
    side_lp: f32,
    ring: [Biquad; NUM_OBJECTS * 2],
    ring_on: [bool; NUM_OBJECTS],
    ring_gain: [f32; NUM_OBJECTS],

    // voicing and the user's filters
    mic_eq1: [Biquad; 2],
    mic_eq2: [Biquad; 2],
    user_hp: [Biquad; 2],
    user_lp: [Biquad; 2],
    last_lc: f32,
    last_hc: f32,

    // smoothed controls
    wet_g_sm: f32,
    dry_g_sm: f32,
    level_sm: f32,
    width_sm: f32,
    early_sm: f32,
    tail_sm: f32,
    gate_sm: f32,
    byp_sm: f32,
    ctrl_counter: i32,
}

impl Default for ChamberMadeDsp {
    fn default() -> Self {
        Self::unallocated()
    }
}

impl ChamberMadeDsp {
    /// Every member at its C++ in-class initial value, with no buffers yet (nothing allocated).
    /// The C++ constructor then runs `prepare (48000.0)`: see [`ChamberMadeDsp::new`].
    pub fn unallocated() -> Self {
        ChamberMadeDsp {
            rt_low_out: 1.0,
            rt_mid_out: 1.0,
            rt_high_out: 1.0,
            volume_out: 90.0,
            surface_out: 126.0,
            scatter_out: 0.1,
            taps_a_out: 0,
            taps_b_out: 0,
            flutter_out: 0.0,
            wet_out: 0.0,
            fs: 48_000.0,
            knob: default_knobs(),
            chamber: Chamber::default(),
            dirty_geo: true,
            dirty_tone: true,
            geo_cooldown: 0,
            pd_buf: Vec::new(),
            tp_buf: Vec::new(),
            pd_w: 0,
            tp_w: 0,
            pd_cur: 0.0,
            pd_target: 0.0,
            tp_cur: 5760.0,
            tp_target: 5760.0,
            tp_amt_sm: 0.0,
            tp_fb_sm: 0.0,
            tp_lp: 0.0,
            tp_hp_x: 0.0,
            tp_hp_y: 0.0,
            tp_lp_c: 0.5,
            tp_hp_c: 0.99,
            tp_wow: 0.0,
            tp_wow_t: 0.0,
            sat_x1: 0.0,
            sat_f1: 0.0,
            sat_drive: 1.0,
            sat_makeup: 1.0,
            sat_on: false,
            spk_hp: Biquad::default(),
            spk_pk: Biquad::default(),
            spk_lp: Biquad::default(),
            bs1: 0.0,
            bs2: 0.0,
            bs_c1: 0.04,
            bs_c2: 0.3,
            er_buf: Vec::new(),
            er_w: 0,
            taps: [DspTap::default(); NUM_TAPS],
            g_slew: 0.001,
            d_slew: 0.0003,
            ap: std::array::from_fn(|_| Allpass::default()),
            ap_g: 0.4,
            ap_gt: 0.4,
            mix_delay: 400.0,
            mix_delay_t: 400.0,
            fl: std::array::from_fn(|_| FLine::default()),
            f_w: 0,
            fv: [0.0; NUM_LINES],
            in_sign: [0.0; NUM_LINES],
            out_a: [0.0; NUM_LINES],
            out_b: [0.0; NUM_LINES],
            tail_gain: 0.5,
            tail_gain_t: 0.5,
            wet_norm: 0.5,
            wet_norm_t: 0.5,
            lfo_ph: 0.0,
            drift_samples: 0.0,
            drift_t: [0.0; 4],
            drift_cur: [0.0; 4],
            comb: std::array::from_fn(|_| Comb::default()),
            comb_w: 0,
            comb_off_b: 14,
            side_hp_c: 0.01,
            side_lp: 0.0,
            ring: [Biquad::default(); NUM_OBJECTS * 2],
            ring_on: [false; NUM_OBJECTS],
            ring_gain: [0.0; NUM_OBJECTS],
            mic_eq1: [Biquad::default(); 2],
            mic_eq2: [Biquad::default(); 2],
            user_hp: [Biquad::default(); 2],
            user_lp: [Biquad::default(); 2],
            last_lc: -1.0,
            last_hc: -1.0,
            wet_g_sm: 0.0,
            dry_g_sm: 1.0,
            level_sm: 1.0,
            width_sm: 1.0,
            early_sm: 1.0,
            tail_sm: 1.0,
            gate_sm: 1.0,
            byp_sm: 0.0,
            ctrl_counter: 0,
        }
    }

    /// `ChamberMadeDSP() { prepare (48000.0); }`: allocates the buffers.
    pub fn new() -> Self {
        let mut d = Self::unallocated();
        d.allocate();
        d.prepare(48_000.0);
        d
    }

    /// Whether the buffers exist (`process` and `prepare` do nothing without them).
    pub fn is_allocated(&self) -> bool {
        self.pd_buf.len() == PD_SIZE as usize
            && self.tp_buf.len() == TP_SIZE as usize
            && self.er_buf.len() == ER_SIZE as usize
            && self.fl.iter().all(|l| l.buf.len() == FL_SIZE as usize)
            && self.ap.iter().all(|a| a.buf.len() == AP_SIZE as usize)
            && self.comb.iter().all(|c| c.buf.len() == FL_SIZE as usize)
    }

    /// The `assign` half of the C++ `prepare`: the buffers at their fixed sizes (they do not
    /// depend on the rate). Allocates only when they are not there yet.
    pub fn allocate(&mut self) {
        if self.is_allocated() {
            return;
        }
        self.pd_buf = vec![0.0; PD_SIZE as usize];
        self.tp_buf = vec![0.0; TP_SIZE as usize];
        self.er_buf = vec![Band3::default(); ER_SIZE as usize];
        for l in &mut self.fl {
            l.buf = vec![0.0; FL_SIZE as usize];
        }
        for a in &mut self.ap {
            a.buf = vec![0.0; AP_SIZE as usize];
        }
        for c in &mut self.comb {
            c.buf = vec![0.0; FL_SIZE as usize];
        }
    }

    /// The rate this engine was last prepared at.
    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    // ------------------------------------------------------------------ lifecycle
    /// `prepare (newSampleRate)`.
    pub fn prepare(&mut self, new_sample_rate: f64) {
        self.prepare_inner(new_sample_rate, true);
    }

    /// `prepare`, optionally without zeroing the buffers (the constructor's `prepare (48000)`
    /// when another `prepare` follows at once: nothing reads a buffer in between, so zeroing
    /// them twice is the same as once).
    pub(crate) fn prepare_inner(&mut self, new_sample_rate: f64, zero_buffers: bool) {
        self.fs = (if new_sample_rate > 0.0 { new_sample_rate } else { 48_000.0 }) as f32;

        // pdBuf / tpBuf / erBuf / fl / ap / comb `assign`: zeroed here (and by `reset`).

        // FDN in and out sign patterns: three different rows of a 16x16 Hadamard matrix.
        for i in 0..NUM_LINES {
            let mut s_in = 1.0f32;
            let mut s_a = 1.0f32;
            let mut s_b = 1.0f32;
            if (i / 1) % 2 == 1 {
                s_in = -s_in;
            }
            if (i / 4) % 2 == 1 {
                s_in = -s_in;
            }
            if (i / 2) % 2 == 1 {
                s_a = -s_a;
            }
            if (i / 8) % 2 == 1 {
                s_a = -s_a;
            }
            if (i / 1) % 2 == 1 {
                s_b = -s_b;
            }
            if (i / 2) % 2 == 1 {
                s_b = -s_b;
            }
            if (i / 8) % 2 == 1 {
                s_b = -s_b;
            }
            self.in_sign[i] = s_in * 0.25;
            self.out_a[i] = s_a;
            self.out_b[i] = s_b;
            self.fl[i].d_t = 0.0;
        }

        self.bs_c1 = self.one_pole(320.0);
        self.bs_c2 = self.one_pole(2800.0);
        self.g_slew = 1.0 - (-1.0 / (0.030 * self.fs)).exp();
        self.d_slew = 1.0 - (-1.0 / (0.080 * self.fs)).exp();
        self.comb_off_b = (0.0003 * self.fs) as i32;

        self.tp_target = knob_to_tape_ms(self.knob[P_TAPE_TIME]) * 0.001 * self.fs;
        self.tp_cur = self.tp_target;

        self.reset_inner(zero_buffers);

        self.update_tone();
        self.dirty_tone = false;
        self.update_geometry();
        self.dirty_geo = false;

        // Start settled, so loading a preset does not swoop every delay into place.
        for l in &mut self.fl {
            l.d = l.d_t;
            l.g_m = l.g_mt;
            l.k_l = l.k_lt;
            l.k_h = l.k_ht;
        }
        for a in &mut self.ap {
            a.len = a.len_t;
        }
        for c in &mut self.comb {
            c.len = c.len_t;
            c.fb = c.fb_t;
            c.lvl = c.lvl_t;
        }
        self.mix_delay = self.mix_delay_t;
        for t in &mut self.taps {
            t.g = t.g_t;
            t.d = t.d_t;
        }
        self.ap_g = self.ap_gt;
        self.tail_gain = self.tail_gain_t;
        self.wet_norm = self.wet_norm_t;
        self.level_sm = knob_to_level_gain(self.knob[P_LEVEL]);
        self.early_sm = knob_to_early_gain(self.knob[P_EARLY]);
        self.tail_sm = knob_to_early_gain(self.knob[P_TAIL]);
        self.width_sm = 2.0 * self.knob[P_WIDTH];
    }

    /// Rust-only (controller ruling for the host's reset; never on the fresh prepare, so
    /// fidelity is untouched): after a `prepare` on a live instance, compute the control-tick
    /// values deterministically and seed every remaining glide at its target, so a reset renders
    /// the same whatever was played before it and nothing swoops. The C++ prepare leaves these
    /// to the next 64-sample control tick and the smoothers (pre-delay, tape amount and
    /// repeats, wet / dry / gate / bypass, wow, drift) where they were.
    ///
    /// Each value is what `updateControl` computes, at `lfoPh = 0` (prepare's `reset` set it).
    /// This engine has no random generator; its only time-varying targets are the LFO's.
    pub fn seed_after_reset(&mut self) {
        let fs = self.fs;

        // The user's filters, built now (as the first control tick would) at the current knobs.
        self.last_lc = self.knob[P_LOW_CUT];
        let f = f64::from(knob_to_low_cut_hz(self.last_lc));
        for c in 0..2 {
            let mut q = self.user_hp[c];
            self.set_hp(&mut q, f, 0.7071);
            self.user_hp[c] = q;
        }
        self.last_hc = self.knob[P_HIGH_CUT];
        let f = f64::from(knob_to_high_cut_hz(self.last_hc));
        for c in 0..2 {
            let mut q = self.user_lp[c];
            self.set_lp(&mut q, f, 0.7071);
            self.user_lp[c] = q;
        }

        // updateControl's targets.
        self.pd_target = clamp_f(0.001 * knob_to_pre_delay_ms(self.knob[P_PRE_DELAY]) * fs, 0.0, (PD_SIZE - 8) as f32);
        self.tp_target = clamp_f(0.001 * knob_to_tape_ms(self.knob[P_TAPE_TIME]) * fs, 16.0, (TP_SIZE - 64) as f32);
        self.tp_lp_c = self.one_pole(knob_to_tape_tone_hz(self.knob[P_TAPE_TONE]));
        self.tp_hp_c = (-2.0 * PI_F * 90.0 / fs).exp();
        self.drift_samples = self.knob[P_DRIFT] * self.knob[P_DRIFT] * 0.0004 * fs;
        self.lfo_ph = 0.0;
        self.tp_wow_t = 0.0007 * self.tp_target * (2.0 * PI_F * (2.0 * self.lfo_ph + 0.1)).sin();
        for i in 0..4 {
            self.drift_t[i] = self.drift_samples * (1.0 + (2.0 * PI_F * (self.lfo_ph + 0.25 * i as f32)).sin());
        }

        // Every glide at its target.
        let bypassed = self.knob[P_BYPASS] > 0.5;
        let wm = fmin(1.0, 2.0 * self.knob[P_MIX]);
        let dm = fmin(1.0, 2.0 * (1.0 - self.knob[P_MIX]));
        self.pd_cur = self.pd_target;
        self.tp_cur = self.tp_target;
        self.tp_amt_sm = self.knob[P_TAPE_SEND];
        self.tp_fb_sm = 0.9 * self.knob[P_TAPE_REPEATS];
        self.wet_g_sm = wm * wm;
        self.dry_g_sm = dm;
        self.gate_sm = if bypassed { 0.0 } else { 1.0 };
        self.byp_sm = if bypassed { 1.0 } else { 0.0 };
        self.tp_wow = self.tp_wow_t;
        self.drift_cur = self.drift_t;
        self.geo_cooldown = 0;
        self.ctrl_counter = 0;
    }

    /// `reset()`.
    pub fn reset(&mut self) {
        self.reset_inner(true);
    }

    fn reset_inner(&mut self, zero_buffers: bool) {
        if zero_buffers {
            self.pd_buf.fill(0.0);
            self.tp_buf.fill(0.0);
            self.er_buf.fill(Band3::default());
        }

        for l in &mut self.fl {
            if zero_buffers {
                l.buf.fill(0.0);
            }
            l.lp1 = 0.0;
            l.lp2 = 0.0;
            l.notch = Biquad::default();
        }
        for a in &mut self.ap {
            if zero_buffers {
                a.buf.fill(0.0);
            }
            a.pos = 0;
        }
        for c in &mut self.comb {
            if zero_buffers {
                c.buf.fill(0.0);
            }
            c.lp = 0.0;
        }

        self.pd_w = 0;
        self.tp_w = 0;
        self.er_w = 0;
        self.f_w = 0;
        self.comb_w = 0;
        self.pd_cur = 0.0;
        self.tp_lp = 0.0;
        self.tp_hp_x = 0.0;
        self.tp_hp_y = 0.0;
        self.tp_wow = 0.0;
        self.tp_amt_sm = 0.0;
        self.tp_fb_sm = 0.0;
        self.sat_x1 = 0.0;
        self.sat_f1 = 0.0;
        self.spk_hp = Biquad::default();
        self.spk_pk = Biquad::default();
        self.spk_lp = Biquad::default();
        self.mic_eq1 = [Biquad::default(); 2];
        self.mic_eq2 = [Biquad::default(); 2];
        self.user_hp = [Biquad::default(); 2];
        self.user_lp = [Biquad::default(); 2];
        self.ring = [Biquad::default(); NUM_OBJECTS * 2];
        self.bs1 = 0.0;
        self.bs2 = 0.0;
        self.side_lp = 0.0;
        self.drift_cur = [0.0; 4];
        self.lfo_ph = 0.0;
        self.ctrl_counter = 0;
        self.last_lc = -1.0;
        self.last_hc = -1.0;
        self.wet_out = 0.0;
    }

    /// Zero. The dry path is never filtered, delayed or trimmed.
    pub fn latency_samples(&self) -> i32 {
        0
    }

    // ------------------------------------------------------------------ events
    /// `setParameter (index, v)`.
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        if index >= NUM_PARAMS {
            return;
        }

        let old = self.knob[index];
        self.knob[index] = v;
        if old == v {
            return;
        }

        match index {
            // Anything that moves a wall, a surface, a source or an object rebuilds the room.
            P_DECAY | P_SHAPE | P_ROOM_W | P_ROOM_L | P_ROOM_H | P_TAPER | P_WALL_MAT | P_FLOOR_MAT | P_CEIL_MAT | P_CEIL_SLOPE | P_SPK_X
            | P_SPK_Y | P_SPK_Z | P_SPK_AIM | P_MIC_AX | P_MIC_AY | P_MIC_BX | P_MIC_BY | P_MIC_Z | P_MIC_A_AIM | P_MIC_B_AIM | P_MIC_PAT => {
                self.dirty_geo = true
            }

            // The speaker changes both the room's directivity and its own voicing.
            P_SPK_TYPE => {
                self.dirty_geo = true;
                self.dirty_tone = true;
            }

            P_DRIVE | P_MIC_TYPE => self.dirty_tone = true,

            _ => {
                if is_object_param(index) {
                    self.dirty_geo = true;
                }
            }
        }
    }

    pub fn get_parameter(&self, index: usize) -> f32 {
        self.knob.get(index).copied().unwrap_or(0.0)
    }

    /// The room as the DSP currently hears it.
    pub fn chamber(&self) -> &Chamber {
        &self.chamber
    }

    // ------------------------------------------------------------------ audio
    /// `process (channels, numChannels, numSamples)`: in place on the first one (mono) or two
    /// channels of `io`, `frames` samples (fewer if a channel is shorter).
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, frames: usize) {
        let nc = num_channels.min(io.len()).min(2);
        if nc == 0 || frames == 0 || !self.is_allocated() {
            return;
        }
        let n = io.iter().take(nc).map(Vec::len).min().unwrap_or(0).min(frames);

        let stereo = nc >= 2;
        let bypassed = self.knob[P_BYPASS] > 0.5;
        let mut wet_peak = 0.0f32;

        let (first, rest) = io.split_at_mut(1);
        let l_ch = &mut first[0];
        for i in 0..n {
            let x_l = l_ch[i];
            let x_r = if stereo { rest[0][i] } else { x_l };
            let (y_l, y_r) = self.tick(x_l, x_r, bypassed, &mut wet_peak);
            if stereo {
                l_ch[i] = y_l;
                rest[0][i] = y_r;
            } else {
                // `L[n] = yL;` then, mono, `L[n] = 0.5f * (yL + yR)`.
                l_ch[i] = 0.5 * (y_l + y_r);
            }
        }

        self.wet_out = wet_peak;
    }

    /// One sample of the C++ per-sample loop.
    #[inline]
    fn tick(&mut self, x_l: f32, x_r: f32, bypassed: bool, wet_peak: &mut f32) -> (f32, f32) {
        // ---- smoothing
        const K: f32 = 0.004;
        self.level_sm += (knob_to_level_gain(self.knob[P_LEVEL]) - self.level_sm) * K;
        self.early_sm += (knob_to_early_gain(self.knob[P_EARLY]) - self.early_sm) * K;
        self.tail_sm += (knob_to_early_gain(self.knob[P_TAIL]) - self.tail_sm) * K;
        self.width_sm += (2.0 * self.knob[P_WIDTH] - self.width_sm) * K;
        self.tail_gain += (self.tail_gain_t - self.tail_gain) * 0.001;
        self.wet_norm += (self.wet_norm_t - self.wet_norm) * 0.001;
        self.ap_g += (self.ap_gt - self.ap_g) * 0.001;

        let wm = fmin(1.0, 2.0 * self.knob[P_MIX]);
        let dm = fmin(1.0, 2.0 * (1.0 - self.knob[P_MIX]));
        self.wet_g_sm += (wm * wm - self.wet_g_sm) * K;
        self.dry_g_sm += (dm - self.dry_g_sm) * K;

        self.gate_sm += ((if bypassed { 0.0 } else { 1.0 }) - self.gate_sm) * 0.002;
        self.byp_sm += ((if bypassed { 1.0 } else { 0.0 }) - self.byp_sm) * 0.002;

        self.ctrl_counter += 1;
        if self.ctrl_counter >= 64 {
            self.ctrl_counter = 0;
            self.update_control();
        }

        // ---- the send: pre-delay, then the tape machine, then the chamber amp
        self.pd_buf[self.pd_w as usize] = 0.5 * (x_l + x_r) * self.gate_sm;
        self.pd_cur += (self.pd_target - self.pd_cur) * self.d_slew;
        if (self.pd_target - self.pd_cur).abs() < 0.01 {
            self.pd_cur = self.pd_target;
        }
        let mut s = read_ring(&self.pd_buf, self.pd_w, self.pd_cur, PD_MASK);
        self.pd_w = (self.pd_w + 1) & PD_MASK;

        // The machine's repeats -- and at full, only them -- feed the speaker.
        self.tp_amt_sm += (self.knob[P_TAPE_SEND] - self.tp_amt_sm) * K;
        self.tp_fb_sm += (0.9 * self.knob[P_TAPE_REPEATS] - self.tp_fb_sm) * K;

        if self.tp_amt_sm > 0.0005 || self.tp_lp.abs() > 1.0e-7 {
            self.tp_cur += (self.tp_target - self.tp_cur) * (self.d_slew * 0.5); // motor inertia
            self.tp_wow += (self.tp_wow_t - self.tp_wow) * 0.002;
            let rd = read_ring(&self.tp_buf, self.tp_w, self.tp_cur + self.tp_wow, TP_MASK);

            self.tp_lp += (rd - self.tp_lp) * self.tp_lp_c;
            let hp = self.tp_lp - self.tp_hp_x + self.tp_hp_c * self.tp_hp_y;
            self.tp_hp_x = self.tp_lp;
            self.tp_hp_y = hp;

            self.tp_buf[self.tp_w as usize] = (s + hp * self.tp_fb_sm).tanh();
            s = s * (1.0 - self.tp_amt_sm) + hp * self.tp_amt_sm;
        } else {
            self.tp_buf[self.tp_w as usize] = s;
        }
        self.tp_w = (self.tp_w + 1) & TP_MASK;

        if self.sat_on {
            s = self.sat_adaa(s * self.sat_drive) * self.sat_makeup;
        }
        s = filter(&mut self.spk_hp, s);
        s = filter(&mut self.spk_pk, s);
        s = filter(&mut self.spk_lp, s);

        // ---- three bands into the room. They add back up to s exactly.
        self.bs1 += (s - self.bs1) * self.bs_c1;
        self.bs2 += (s - self.bs2) * self.bs_c2;
        self.er_buf[self.er_w as usize] = Band3::new(self.bs1, self.bs2 - self.bs1, s - self.bs2);

        // ---- early reflections
        let mut er_a = 0.0f32;
        let mut er_b = 0.0f32;

        let er_w = self.er_w;
        let g_slew = self.g_slew;
        let d_slew = self.d_slew;
        for (ti, t) in self.taps.iter_mut().enumerate() {
            if !t.live {
                continue;
            }

            t.g = t.g + (t.g_t - t.g).scale(g_slew);
            t.d += (t.d_t - t.d) * d_slew;

            let di = t.d as i32;
            let fr = t.d - di as f32;
            let i1 = er_w.wrapping_sub(di) & ER_MASK;
            let i2 = (i1 - 1) & ER_MASK;
            let v = (self.er_buf[i1 as usize].scale(1.0 - fr) + self.er_buf[i2 as usize].scale(fr)) * t.g;
            let sum = v.lo + v.mid + v.hi;

            if ti < TAPS_PER_MIC {
                er_a += sum;
            } else {
                er_b += sum;
            }
        }

        // ---- flutter
        let comb_w = self.comb_w;
        let comb_off_b = self.comb_off_b;
        for cb in &mut self.comb {
            cb.len += (cb.len_t - cb.len) * d_slew;
            if (cb.len_t - cb.len).abs() < 0.01 {
                cb.len = cb.len_t;
            }
            cb.fb += (cb.fb_t - cb.fb) * 0.001;
            cb.lvl += (cb.lvl_t - cb.lvl) * 0.001;

            if cb.lvl > 1.0e-6 || cb.lp.abs() > 1.0e-9 {
                let ci = cb.len as i32;
                let cf = cb.len - ci as f32;
                let c1 = comb_w.wrapping_sub(ci) & FL_MASK;
                let r = cb.buf[c1 as usize] * (1.0 - cf) + cb.buf[((c1 - 1) & FL_MASK) as usize] * cf;
                // The second microphone taps the same comb a fraction of a millisecond later.
                let rb = cb.buf[((c1 - comb_off_b) & FL_MASK) as usize];

                cb.lp += (r - cb.lp) * cb.damp;
                cb.buf[comb_w as usize] = s + cb.fb * cb.lp;
                er_a += r * cb.lvl;
                er_b += rb * cb.lvl;
            } else {
                cb.buf[comb_w as usize] = s;
            }
        }
        self.comb_w = (self.comb_w + 1) & FL_MASK;

        // ---- the late field
        self.mix_delay += (self.mix_delay_t - self.mix_delay) * d_slew;
        if (self.mix_delay_t - self.mix_delay).abs() < 0.01 {
            self.mix_delay = self.mix_delay_t;
        }

        let mi = self.mix_delay as i32;
        let mf = self.mix_delay - mi as f32;
        let m1 = er_w.wrapping_sub(mi) & ER_MASK;
        let mv = self.er_buf[m1 as usize].scale(1.0 - mf) + self.er_buf[((m1 - 1) & ER_MASK) as usize].scale(mf);
        let mut u = mv.lo + mv.mid + mv.hi;

        let ap_g = self.ap_g;
        for a in &mut self.ap {
            u = allpass(a, u, d_slew, ap_g);
        }

        let mut t_a = 0.0f32;
        let mut t_b = 0.0f32;

        let f_w = self.f_w;
        let bs_c1 = self.bs_c1;
        let bs_c2 = self.bs_c2;
        let drift_samples = self.drift_samples;
        for i in 0..NUM_LINES {
            let l = &mut self.fl[i];

            l.d += (l.d_t - l.d) * d_slew;
            if (l.d_t - l.d).abs() < 0.01 {
                l.d = l.d_t;
            }

            l.g_m += (l.g_mt - l.g_m) * 0.001;
            l.k_l += (l.k_lt - l.k_l) * 0.001;
            l.k_h += (l.k_ht - l.k_h) * 0.001;

            let mut d = l.d;
            if i < 4 {
                // Air movement: four slow, out-of-phase wanders on the first four lines only.
                self.drift_cur[i] += (self.drift_t[i] - self.drift_cur[i]) * 0.002;
                if drift_samples > 0.001 || self.drift_cur[i] > 0.001 {
                    d += self.drift_cur[i];
                }
            }

            let mut r = read_frac(&l.buf, f_w, d);
            t_a += r * self.out_a[i];
            t_b += r * self.out_b[i];

            // Decay: a low shelf and a high shelf around a mid gain, then the resonant
            // absorber if a pot or a barrel is carried on this line.
            l.lp1 += (r - l.lp1) * bs_c1;
            r = r + (l.k_l - 1.0) * l.lp1;
            l.lp2 += (r - l.lp2) * bs_c2;
            r = r + (l.k_h - 1.0) * (r - l.lp2);
            r *= l.g_m;
            if l.notch_on {
                r = filter(&mut l.notch, r);
            }
            self.fv[i] = r;
        }

        self.hadamard16();

        for i in 0..NUM_LINES {
            let v = self.fv[i] + u * self.in_sign[i];
            self.fl[i].buf[f_w as usize] = v;
        }

        self.f_w = (self.f_w + 1) & FL_MASK;
        self.er_w = (self.er_w + 1) & ER_MASK;

        // Two microphones in one diffuse field are coherent below c / (2 * spacing).
        let t_mid = 0.5 * (t_a + t_b);
        let mut t_side = 0.5 * (t_a - t_b);
        self.side_lp += (t_side - self.side_lp) * self.side_hp_c;
        t_side -= self.side_lp;
        t_a = t_mid + t_side;
        t_b = t_mid - t_side;

        // Barrels ring.
        for o in 0..NUM_OBJECTS {
            if !self.ring_on[o] {
                continue;
            }
            let r1 = filter(&mut self.ring[o * 2], t_mid);
            let r2 = filter(&mut self.ring[o * 2 + 1], t_mid);
            t_a += self.ring_gain[o] * (r1 + 0.6 * r2);
            t_b += self.ring_gain[o] * (r1 - 0.6 * r2);
        }

        // ---- at the microphones
        let mut w_a = (er_a * self.early_sm + t_a * self.tail_gain * self.tail_sm) * self.wet_norm;
        let mut w_b = (er_b * self.early_sm + t_b * self.tail_gain * self.tail_sm) * self.wet_norm;

        w_a = filter(&mut self.mic_eq1[0], w_a);
        w_a = filter(&mut self.mic_eq2[0], w_a);
        w_b = filter(&mut self.mic_eq1[1], w_b);
        w_b = filter(&mut self.mic_eq2[1], w_b);
        w_a = filter(&mut self.user_hp[0], w_a);
        w_a = filter(&mut self.user_lp[0], w_a);
        w_b = filter(&mut self.user_hp[1], w_b);
        w_b = filter(&mut self.user_lp[1], w_b);

        let m = 0.5 * (w_a + w_b);
        let sd = 0.5 * (w_a - w_b) * self.width_sm;
        let w_l = (m + sd) * self.wet_g_sm * self.level_sm;
        let w_r = (m - sd) * self.wet_g_sm * self.level_sm;

        *wet_peak = fmax(*wet_peak, w_l.abs());

        // Bypassed is the untouched input plus whatever is still ringing.
        let y_l = (1.0 - self.byp_sm) * (x_l * self.dry_g_sm * self.level_sm + w_l) + self.byp_sm * (x_l + w_l);
        let y_r = (1.0 - self.byp_sm) * (x_r * self.dry_g_sm * self.level_sm + w_r) + self.byp_sm * (x_r + w_r);
        (y_l, y_r)
    }

    // ------------------------------------------------------------------ helpers
    fn safe_hz(&self, fc: f64) -> f64 {
        let top = 0.45 * f64::from(self.fs);
        if fc > top {
            return top;
        }
        if fc < 5.0 {
            return 5.0;
        }
        fc
    }

    fn set_lp(&self, q: &mut Biquad, fc_in: f64, qv: f64) {
        let k = (PI_D * self.safe_hz(fc_in) / f64::from(self.fs)).tan();
        let a0 = 1.0 + k / qv + k * k;
        q.b0 = ((k * k) / a0) as f32;
        q.b1 = ((2.0 * k * k) / a0) as f32;
        q.b2 = q.b0;
        q.a1 = ((2.0 * (k * k - 1.0)) / a0) as f32;
        q.a2 = ((1.0 - k / qv + k * k) / a0) as f32;
    }

    fn set_hp(&self, q: &mut Biquad, fc_in: f64, qv: f64) {
        let k = (PI_D * self.safe_hz(fc_in) / f64::from(self.fs)).tan();
        let a0 = 1.0 + k / qv + k * k;
        q.b0 = (1.0 / a0) as f32;
        q.b1 = (-2.0 / a0) as f32;
        q.b2 = q.b0;
        q.a1 = ((2.0 * (k * k - 1.0)) / a0) as f32;
        q.a2 = ((1.0 - k / qv + k * k) / a0) as f32;
    }

    fn set_peak(&self, q: &mut Biquad, fc_in: f64, qv: f64, db: f64) {
        let a = 10.0f64.powf(db / 40.0);
        let w = 2.0 * PI_D * self.safe_hz(fc_in) / f64::from(self.fs);
        let al = w.sin() / (2.0 * qv);
        let a0 = 1.0 + al / a;
        q.b0 = ((1.0 + al * a) / a0) as f32;
        q.b1 = ((-2.0 * w.cos()) / a0) as f32;
        q.b2 = ((1.0 - al * a) / a0) as f32;
        q.a1 = q.b1;
        q.a2 = ((1.0 - al / a) / a0) as f32;
    }

    fn set_band(&self, q: &mut Biquad, fc_in: f64, qv: f64) {
        let w = 2.0 * PI_D * self.safe_hz(fc_in) / f64::from(self.fs);
        let al = w.sin() / (2.0 * qv);
        let a0 = 1.0 + al;
        q.b0 = (al / a0) as f32;
        q.b1 = 0.0;
        q.b2 = (-al / a0) as f32;
        q.a1 = ((-2.0 * w.cos()) / a0) as f32;
        q.a2 = ((1.0 - al) / a0) as f32;
    }

    fn one_pole(&self, fc: f32) -> f32 {
        1.0 - (-2.0 * PI_F * fmin(fc, 0.45 * self.fs) / self.fs).exp()
    }

    /// tanh with first-order antiderivative anti-aliasing, F(x) = ln cosh x.
    fn sat_adaa(&mut self, u: f32) -> f32 {
        let x1 = f64::from(u);
        let ax = x1.abs();
        let f1 = ax + (1.0 + (-2.0 * ax).exp()).ln() - 0.693_147_180_559_945_3;
        let dx = x1 - self.sat_x1;
        let y = if dx.abs() < 1.0e-4 { (0.5 * (x1 + self.sat_x1)).tanh() } else { (f1 - self.sat_f1) / dx };
        self.sat_x1 = x1;
        self.sat_f1 = f1;
        y as f32
    }

    /// A 16-point fast Hadamard transform in place, scaled so it is orthonormal.
    fn hadamard16(&mut self) {
        let fv = &mut self.fv;
        for k in 0..8 {
            let i = k * 2;
            let (a, b) = (fv[i], fv[i + 1]);
            fv[i] = a + b;
            fv[i + 1] = a - b;
        }
        for k in 0..8 {
            let i = (k / 2) * 4 + (k % 2);
            let (a, b) = (fv[i], fv[i + 2]);
            fv[i] = a + b;
            fv[i + 2] = a - b;
        }
        for k in 0..8 {
            let i = (k / 4) * 8 + (k % 4);
            let (a, b) = (fv[i], fv[i + 4]);
            fv[i] = a + b;
            fv[i + 4] = a - b;
        }
        for k in 0..8 {
            let (a, b) = (fv[k], fv[k + 8]);
            fv[k] = (a + b) * 0.25;
            fv[k + 8] = (a - b) * 0.25;
        }
    }

    // ------------------------------------------------------------------ the whole room
    fn update_geometry(&mut self) {
        geo::build_into(&mut self.chamber, &knobs_to_geometry(&self.knob));

        let fs = self.fs;
        let room = self.chamber.room;
        let ac = self.chamber.acoustics;

        self.rt_low_out = ac.rt60.lo;
        self.rt_mid_out = ac.rt60.mid;
        self.rt_high_out = ac.rt60.hi;
        self.volume_out = ac.volume;
        self.surface_out = ac.surface_area;
        self.scatter_out = ac.scatter;
        self.taps_a_out = self.chamber.live_taps(0);
        self.taps_b_out = self.chamber.live_taps(1);

        // ---- the taps: distances become delays in samples
        for i in 0..NUM_TAPS {
            let dst = &mut self.taps[i];
            let src = &self.chamber.taps[i];

            if !src.live {
                // Fade it out rather than cutting it. updateControl() retires it.
                dst.g_t = Band3::new(0.0, 0.0, 0.0);
                continue;
            }

            let d_s = clamp_f(src.distance / SPEED_OF_SOUND * fs, 2.0, (ER_SIZE - 8) as f32);

            // A reflection that has just appeared arrives IN PLACE.
            if dst.g.sum_abs() < 1.0e-6 {
                dst.d = d_s;
            }

            dst.d_t = d_s;
            dst.g_t = src.gain;
            dst.live = true;
        }

        // ---- late field: line lengths from the room's own proportions
        let h = room.height_m;
        let mut pm = 0.0f32;
        for p in ac.line_proportions {
            pm += p;
        }
        pm /= 16.0;

        let mut d_mean = 0.0f32;
        for i in 0..NUM_LINES {
            let r = clamp_f(0.6 * ac.line_proportions[i] / pm + 0.4 * SPREAD[i], 0.35, 2.4);
            let mut d = (r * ac.mean_line_seconds * fs).floor();

            // Two lines of the same length are one line with twice the gain. Nudge them apart.
            for j in 0..i {
                if (d - self.fl[j].d_t).abs() < 7.0 {
                    d += 11.0;
                }
            }

            d = clamp_f(d, 32.0, (FL_SIZE - 64) as f32);
            self.fl[i].d_t = d;
            d_mean += d;
        }
        d_mean /= 16.0;

        // ---- resonant absorbers: which lines carry which pot or barrel
        let objects = self.chamber.objects;
        let mut n_res = 0usize;
        let mut res_obj = [0usize; NUM_OBJECTS];

        for o in 0..NUM_OBJECTS {
            self.ring_on[o] = false;
            if !objects[o].on {
                continue;
            }

            let sp = &geo::OBJECT_SPECS[objects[o].ty as usize];
            if sp.resonator > RES_NONE {
                res_obj[n_res] = o;
                n_res += 1;
            }

            if sp.resonator == RES_BARREL {
                // A barrel is a Helmholtz absorber that also rings.
                let f0 = sp.res_f0_times_radius / objects[o].radius;
                let mut q = self.ring[o * 2];
                self.set_band(&mut q, f64::from(f0), 30.0);
                self.ring[o * 2] = q;
                let mut q = self.ring[o * 2 + 1];
                self.set_band(&mut q, f64::from(f0 * 2.76), 22.0);
                self.ring[o * 2 + 1] = q;
                self.ring_on[o] = true;
                self.ring_gain[o] = 0.05 * objects[o].radius / 0.3;
            }
        }

        // ---- per-line decay filters
        let decay_mul = knob_to_decay_mul(self.knob[P_DECAY]);

        for i in 0..NUM_LINES {
            let secs = self.fl[i].d_t / fs;

            let g_l = 10.0f32.powf(-3.0 * secs / ac.rt60.lo);
            let g_m = 10.0f32.powf(-3.0 * secs / ac.rt60.mid);
            let g_h = 10.0f32.powf(-3.0 * secs / ac.rt60.hi);

            self.fl[i].g_mt = g_m;
            self.fl[i].k_lt = g_l / g_m;
            self.fl[i].k_ht = g_h / g_m;

            self.fl[i].notch_on = false;
            if n_res > 0 {
                let o = res_obj[i % n_res];
                let sp = &geo::OBJECT_SPECS[objects[o].ty as usize];
                let a = objects[o].radius;
                let f0 = sp.res_f0_times_radius / a;
                let a_res = sp.res_area_per_radius_sq * a * a;

                // The extra decay rate at f0 from this absorber, concentrated on the lines
                // that carry it.
                let cut = clamp_f(-60.0 * secs * (a_res / (0.161 * ac.volume)) * n_res as f32 / decay_mul, -9.0, 0.0);
                let mut q = self.fl[i].notch;
                self.set_peak(&mut q, f64::from(f0), if sp.resonator == RES_BARREL { 12.0 } else { 6.0 }, f64::from(cut));
                self.fl[i].notch = q;
                self.fl[i].notch_on = true;
            }
        }

        // ---- diffusion
        self.ap_gt = clamp_f(0.2 + 0.55 * ac.scatter, 0.2, 0.72);
        let ap_base = clamp_f(ac.mean_free_path / SPEED_OF_SOUND, 0.004, 0.03) * 0.5;
        let ap_max = (AP_SIZE - 8) as f32;
        self.ap[0].len_t = clamp_f((ap_base * 0.31 * fs).floor(), 8.0, ap_max);
        self.ap[1].len_t = clamp_f((ap_base * 0.47 * fs).floor(), 8.0, ap_max);
        self.ap[2].len_t = clamp_f((ap_base * 0.71 * fs).floor(), 8.0, ap_max);
        self.ap[3].len_t = clamp_f((ap_base * 1.00 * fs).floor(), 8.0, ap_max);
        self.mix_delay_t = clamp_f((ac.mean_free_path / SPEED_OF_SOUND * fs).floor(), 8.0, (ER_SIZE - 8) as f32);

        // ---- flutter: the closest FACING pair of parallel walls, and floor against ceiling
        let mut best_d = 1.0e9f32;
        let mut best_pf = 0.0f32;

        let nw = room.num_walls;
        for i in 0..nw {
            for j in (i + 1)..nw {
                let dp = room.nx[i] * room.nx[j] + room.ny[i] * room.ny[j];
                if dp >= -0.985 {
                    continue; // not parallel enough to flutter
                }

                let j2 = if j + 1 >= nw { 0 } else { j + 1 };
                let mx = 0.5 * (room.vx[j] + room.vx[j2]);
                let my = 0.5 * (room.vy[j] + room.vy[j2]);
                let dd = (mx - room.vx[i]) * room.nx[i] + (my - room.vy[i]) * room.ny[i];

                if dd > 0.3 && dd < best_d {
                    best_d = dd;
                    best_pf = clamp_f((-dp - 0.985) / 0.015, 0.0, 1.0);
                }
            }
        }

        let r_w = ac.surface_reflection[0];
        let r_f = ac.surface_reflection[SURF_FLOOR];
        let r_c = ac.surface_reflection[SURF_CEILING];
        let smooth = 1.0 - ac.scatter;

        if best_pf > 0.0 {
            let t = 2.0 * best_d / SPEED_OF_SOUND;
            self.comb[0].len_t = clamp_f((t * fs).floor(), 16.0, (FL_SIZE - 64) as f32);
            let g_max = 10.0f32.powf(-3.0 * t / (1.5 * ac.rt60.mid));
            self.comb[0].fb_t = fmin(fmin(r_w.mid * r_w.mid * smooth * smooth.sqrt() * best_pf, g_max), 0.96);
            self.comb[0].damp = self.one_pole(2000.0 + 12000.0 * r_w.hi * r_w.hi * smooth);
            self.comb[0].lvl_t = 0.5 * best_pf * smooth * smooth / fmax(best_d * 0.5, 1.0);
        } else {
            self.comb[0].fb_t = 0.0;
            self.comb[0].lvl_t = 0.0;
        }

        {
            let t = 2.0 * h / SPEED_OF_SOUND;
            let pf_c = clamp_f((1.0 / room.ceil_norm - 0.985) / 0.015, 0.0, 1.0);
            let mat_scatter = ac.scatter; // close enough for the flutter weighting
            let sm2 = (1.0 - mat_scatter) * (1.0 - 0.5 * ac.object_scatter) * pf_c;
            self.comb[1].len_t = clamp_f((t * fs).floor(), 16.0, (FL_SIZE - 64) as f32);
            let g_max = 10.0f32.powf(-3.0 * t / (1.5 * ac.rt60.mid));
            self.comb[1].fb_t = fmin(fmin(r_f.mid * r_c.mid * sm2 * fmax(sm2, 0.0).sqrt(), g_max), 0.96);
            self.comb[1].damp = self.one_pole(2000.0 + 12000.0 * r_f.hi * r_c.hi * sm2);
            self.comb[1].lvl_t = 0.35 * sm2 * sm2 / fmax(h * 0.5, 1.0);
        }

        self.flutter_out = fmax(self.comb[0].lvl_t, self.comb[1].lvl_t);

        // ---- two microphones in a diffuse field are coherent below c / (2 * spacing)
        let spacing = fmax(len3(self.chamber.placement.mic_a - self.chamber.placement.mic_b), 0.05);
        self.side_hp_c = self.one_pole(clamp_f(SPEED_OF_SOUND / (2.0 * spacing), 30.0, 1500.0));

        // ---- levels (the geometric mean between the measured tap energy and the physical
        // tail, which keeps placement audible).
        let g2 = 10.0f32.powf(-6.0 * (d_mean / fs) / ac.rt60.mid);

        let mut ree = 1.0f32; // random-energy efficiency of the polar pattern
        let pat = lround(self.knob[P_MIC_PAT]);
        if pat == 1 {
            ree = 1.0 / 3.0;
        } else if pat == 2 {
            ree = 0.25;
        } else if pat == 3 {
            ree = 1.0 / 3.0;
        }

        let mut e_taps = 0.0f32;
        for t in &self.taps {
            e_taps += t.g_t.mid * t.g_t.mid;
        }
        e_taps = fmax(0.5 * e_taps, 1.0e-4);

        let e_tail_phys = 50.265 / ac.total_absorption.mid * ree;
        let e_tail = e_taps * (e_tail_phys / e_taps).sqrt();

        self.tail_gain_t = 1.4 * (e_tail * fmax(1.0 - g2, 1.0e-5)).sqrt();
        self.wet_norm_t = clamp_f(1.0 / (e_taps + e_tail).sqrt(), 0.02, 6.0);
    }

    // ------------------------------------------------------------------ voicing
    fn update_tone(&mut self) {
        let spk = lround(self.knob[P_SPK_TYPE]);
        let mic = lround(self.knob[P_MIC_TYPE]);

        let (mut hp, mut pk, mut lp) = (self.spk_hp, self.spk_pk, self.spk_lp);
        // The speaker's own voicing: a high-pass, a presence peak, a low-pass.
        if spk == 0 {
            self.set_hp(&mut hp, 45.0, 0.7071);
            self.set_peak(&mut pk, 2500.0, 0.8, 0.0);
            self.set_lp(&mut lp, 18000.0, 0.7071);
        } else if spk == 1 {
            self.set_hp(&mut hp, 70.0, 0.7071);
            self.set_peak(&mut pk, 2600.0, 0.9, 3.0);
            self.set_lp(&mut lp, 12000.0, 0.7071);
        } else if spk == 2 {
            self.set_hp(&mut hp, 55.0, 0.7071);
            self.set_peak(&mut pk, 450.0, 0.7, 1.5);
            self.set_lp(&mut lp, 14000.0, 0.7071);
        } else if spk == 3 {
            self.set_hp(&mut hp, 120.0, 0.7071);
            self.set_peak(&mut pk, 1800.0, 1.0, 4.0);
            self.set_lp(&mut lp, 6500.0, 0.7071);
        } else {
            self.set_hp(&mut hp, 150.0, 0.7071);
            self.set_peak(&mut pk, 900.0, 1.2, 3.0);
            self.set_lp(&mut lp, 8000.0, 0.7071);
        }
        (self.spk_hp, self.spk_pk, self.spk_lp) = (hp, pk, lp);

        for c in 0..2 {
            let (mut e1, mut e2) = (self.mic_eq1[c], self.mic_eq2[c]);
            if mic == 0 {
                self.set_peak(&mut e1, 10000.0, 0.7, 0.0);
                self.set_lp(&mut e2, 20000.0, 0.7071);
            } else if mic == 1 {
                self.set_peak(&mut e1, 10000.0, 0.7, 2.5);
                self.set_lp(&mut e2, 18000.0, 0.7071);
            } else if mic == 2 {
                self.set_peak(&mut e1, 150.0, 0.7, 2.0);
                self.set_lp(&mut e2, 7500.0, 0.6);
            } else {
                self.set_peak(&mut e1, 4500.0, 1.0, 3.5);
                self.set_lp(&mut e2, 11000.0, 0.7071);
            }
            (self.mic_eq1[c], self.mic_eq2[c]) = (e1, e2);
        }

        let drive = self.knob[P_DRIVE];
        self.sat_on = drive > 0.01;
        self.sat_drive = 1.0 + 9.0 * drive * drive;
        self.sat_makeup = 1.0 / self.sat_drive.powf(0.8);
    }

    fn update_control(&mut self) {
        if self.dirty_tone {
            self.dirty_tone = false;
            self.update_tone();
        }

        // Rebuilding the room is the expensive call: a cooldown keeps a dragged control from
        // asking for it on every control tick. Four ticks is about 5 ms at 48 kHz.
        if self.geo_cooldown > 0 {
            self.geo_cooldown -= 1;
        }
        if self.dirty_geo && self.geo_cooldown <= 0 {
            self.dirty_geo = false;
            self.geo_cooldown = 4;
            self.update_geometry();
        }

        if (self.knob[P_LOW_CUT] - self.last_lc).abs() > 0.0005 {
            self.last_lc = self.knob[P_LOW_CUT];
            let f = f64::from(knob_to_low_cut_hz(self.last_lc));
            for c in 0..2 {
                let mut q = self.user_hp[c];
                self.set_hp(&mut q, f, 0.7071);
                self.user_hp[c] = q;
            }
        }
        if (self.knob[P_HIGH_CUT] - self.last_hc).abs() > 0.0005 {
            self.last_hc = self.knob[P_HIGH_CUT];
            let f = f64::from(knob_to_high_cut_hz(self.last_hc));
            for c in 0..2 {
                let mut q = self.user_lp[c];
                self.set_lp(&mut q, f, 0.7071);
                self.user_lp[c] = q;
            }
        }

        self.pd_target = clamp_f(0.001 * knob_to_pre_delay_ms(self.knob[P_PRE_DELAY]) * self.fs, 0.0, (PD_SIZE - 8) as f32);

        self.tp_target = clamp_f(0.001 * knob_to_tape_ms(self.knob[P_TAPE_TIME]) * self.fs, 16.0, (TP_SIZE - 64) as f32);
        self.tp_lp_c = self.one_pole(knob_to_tape_tone_hz(self.knob[P_TAPE_TONE]));
        self.tp_hp_c = (-2.0 * PI_F * 90.0 / self.fs).exp();

        self.drift_samples = self.knob[P_DRIFT] * self.knob[P_DRIFT] * 0.0004 * self.fs; // up to 0.4 ms
        self.lfo_ph += 0.31 * 64.0 / self.fs;
        if self.lfo_ph >= 1.0 {
            self.lfo_ph -= 1.0;
        }

        // A little wow on the tape send, slower than you would notice as pitch.
        self.tp_wow_t = 0.0007 * self.tp_target * (2.0 * PI_F * (2.0 * self.lfo_ph + 0.1)).sin();

        for i in 0..4 {
            self.drift_t[i] = self.drift_samples * (1.0 + (2.0 * PI_F * (self.lfo_ph + 0.25 * i as f32)).sin());
        }

        // Retire reflections that have finished fading, so the per-sample loop can skip them.
        for t in &mut self.taps {
            if !t.live {
                continue;
            }
            if t.g_t.sum_abs() == 0.0 && t.g.sum_abs() < 1.0e-6 {
                t.live = false;
                t.g = Band3::new(0.0, 0.0, 0.0);
            }
        }
    }

    // ------------------------------------------------------------------ tests
    #[cfg(test)]
    pub(crate) fn heap_blocks(&self) -> Vec<(usize, usize)> {
        let mut v = vec![
            (self.pd_buf.as_ptr() as usize, self.pd_buf.capacity()),
            (self.tp_buf.as_ptr() as usize, self.tp_buf.capacity()),
            (self.er_buf.as_ptr() as usize, self.er_buf.capacity()),
        ];
        v.extend(self.fl.iter().map(|l| (l.buf.as_ptr() as usize, l.buf.capacity())));
        v.extend(self.ap.iter().map(|a| (a.buf.as_ptr() as usize, a.buf.capacity())));
        v.extend(self.comb.iter().map(|c| (c.buf.as_ptr() as usize, c.buf.capacity())));
        v
    }

    #[cfg(test)]
    pub(crate) fn knobs(&self) -> &[f32; NUM_PARAMS] {
        &self.knob
    }

    #[cfg(test)]
    pub(crate) fn line_lengths(&self) -> [f32; NUM_LINES] {
        std::array::from_fn(|i| self.fl[i].d_t)
    }
}

/// `readFrac` (on the FDN write position `fW`).
#[inline]
fn read_frac(buf: &[f32], f_w: i32, d: f32) -> f32 {
    let di = d as i32;
    let fr = d - di as f32;
    let i1 = f_w.wrapping_sub(di) & FL_MASK;
    let i2 = (i1 - 1) & FL_MASK;
    buf[i1 as usize] * (1.0 - fr) + buf[i2 as usize] * fr
}

/// `allpass (Allpass&, float x)` (with the engine's `dSlew` and `apG`).
#[inline]
fn allpass(a: &mut Allpass, x: f32, d_slew: f32, ap_g: f32) -> f32 {
    a.len += (a.len_t - a.len) * d_slew;
    if (a.len_t - a.len).abs() < 0.01 {
        a.len = a.len_t;
    }

    let li = a.len as i32;
    let lf = a.len - li as f32;
    let r = a.pos.wrapping_sub(li) & AP_MASK;
    let v = a.buf[r as usize] * (1.0 - lf) + a.buf[((r - 1) & AP_MASK) as usize] * lf;
    let y = v - ap_g * x;
    a.buf[a.pos as usize] = x + ap_g * y;
    a.pos = (a.pos + 1) & AP_MASK;
    y
}
