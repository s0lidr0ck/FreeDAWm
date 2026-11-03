//! REVERB ENGINE -- one frame, six machines.
//! Line-by-line translation of `Engine 7 - Reverb/Source/ReverbEngineDSP.h`.
//!
//! Shared by every machine:
//!   dry path untouched (zero latency) -> wet: send gate (bypass / freeze / swell) -> pre-delay (free or
//!   note value) -> the machine -> Low Cut / High Cut -> width -> duck -> mix.
//!   Decay is a real RT60 in seconds. Damping and Low Decay work INSIDE the loop (they change how long
//!   highs and lows ring, not just how bright the output is). Low Cut / High Cut are plain EQ on the wet.
//!
//! Machines:
//! - Room: 16-line feedback delay network, short lines, early reflections. Character = Early.
//! - Hall: same network, long lines, slower build. Character = Early.
//! - Plate: EMT 140; short, tightly spaced lines, an allpass chain inside each line (dispersion that
//!   compounds on every circulation) and shelving damping. Character = Zing (dispersion).
//! - Spring: three dispersive spring lines (100 stretched allpasses each, 4.3 kHz transition), driven by a
//!   saturating tank driver. Character = Dwell (drive into the tank).
//! - Cloud: very long lines, heavy in-loop diffusion, deep modulation, input diffusers that stretch so
//!   the sound blooms in. Character = Bloom.
//! - Shimmer: hall network with a splice-aligned pitch shifter INSIDE the loop on two lines.
//!   Character = Shimmer amount.
//!
//! Color (every machine except Spring) is the era of the box: 1970s / 1980s / Now. Attack spreads what
//! goes into the tail over up to 250 ms. Changing Machine clears the tail (0.33 s flush).
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - `float` and `double` are kept exactly where the C++ has them, and every expression keeps its
//!   C++ evaluation order (float arithmetic is not associative).
//! - `clampT`, `std::min` and `std::max` keep their C++ definitions ([`clamp_t`], [`min_t`],
//!   [`max_t`]), including what they do with a NaN.
//! - `int (x)` on a float truncates toward zero; Rust `as i32` does the same for every value the
//!   engine can produce (the wrapper clamps every parameter into range first, as the APVTS does), and
//!   saturates (NaN reads 0) where the C++ would be undefined.
//! - Buffer reads and writes go through [`rd`] / [`wr`]: every index is in range by construction (the
//!   C++ folds them), and an impossible one reads 0 / writes nothing instead of panicking.
//! - The big buffers are `Vec`s, as in the C++ (`std::vector` sized in `prepare`). They are made once,
//!   in [`ReverbEngineDsp::new`] (the C++ constructor's `prepare (48000.0)`), always at their full,
//!   rate-independent size (the C++ sizes them for 192 kHz), and only zeroed after that, so `prepare`
//!   and [`ReverbEngineDsp::restore_initial`] never allocate.
//!
//! Rust additions: [`ReverbEngineDsp::restore_initial`] (a fresh engine in place) and the read-only
//! accessors at the end (the strip's readouts and graph).

// The C++ literals (0.7071f and friends) are kept as written, not swapped for the constants.
#![allow(clippy::approx_constant)]
#![allow(clippy::excessive_precision)]
// The C++ `if (x < lo) x = lo; if (x > hi) x = hi;` is kept as written.
#![allow(clippy::manual_clamp)]

// ---------------------------------------------------------------- parameters (order == Parameters.h)
pub const P_DECAY: usize = 0;
pub const P_MIX: usize = 1;
pub const P_MACHINE: usize = 2;
pub const P_SIZE: usize = 3;
pub const P_PRE_DELAY: usize = 4;
pub const P_BYPASS: usize = 5;
pub const P_LEVEL: usize = 6;
pub const P_LOW_CUT: usize = 7;
pub const P_HIGH_CUT: usize = 8;
pub const P_DAMPING: usize = 9;
pub const P_BASS: usize = 10;
pub const P_CHARACTER: usize = 11;
pub const P_DIFFUSION: usize = 12;
pub const P_MOD_RATE: usize = 13;
pub const P_MOD_DEPTH: usize = 14;
pub const P_WIDTH: usize = 15;
pub const P_DUCK: usize = 16;
pub const P_SWELL: usize = 17;
pub const P_FREEZE: usize = 18;
pub const P_TRAILS: usize = 19;
pub const P_INTERVAL: usize = 20;
pub const P_SYNC: usize = 21;
pub const P_DIVISION: usize = 22;
pub const P_TEMPO: usize = 23;
pub const P_TEMPO_SRC: usize = 24;
pub const P_COLOR: usize = 25;
pub const P_ATTACK: usize = 26;
pub const NUM_PARAMS: usize = 27;

// ---------------------------------------------------------------- constants
/// Network / spring lines: 170 ms at 192k.
pub const LBUF: i32 = 32768;
/// In-loop allpasses.
pub const ABUF: i32 = 16384;
/// Input diffusers.
pub const DBUF: i32 = 16384;
/// Pre-delay: 1.36 s at 192k.
pub const PBUF: i32 = 262144;
/// Pitch shifter.
pub const PSBUF: i32 = 32768;
/// Stretched allpasses per spring.
pub const SPR_M: i32 = 100;
/// Max stretch (samples) per allpass.
pub const SPR_K: i32 = 32;
/// Anti-denormal bias, 360 dB down.
const DN: f32 = 1.0e-18;
/// Target length of one pitch-shifter grain, whatever the interval.
const K_GRAIN_SECONDS: f32 = 0.10;
/// First-order allpasses per line in the plate's in-loop dispersion chain.
const K_PLATE_AP: usize = 8;

const PI: f64 = 3.14159265358979323846;
const TWO_PI: f64 = 6.28318530717958647692;

// ---------------------------------------------------------------- C++ helpers, same semantics
/// `clampT`: `v < lo ? lo : (v > hi ? hi : v)`.
pub(crate) fn clamp_t(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}
/// `std::min`: `(b < a) ? b : a`.
pub(crate) fn min_t(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}
/// `std::max`: `(a < b) ? b : a`.
fn max_t(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}
/// `std::min` on doubles.
fn min_d(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// The C++ wrapper runs the engine under `ScopedNoDenormals` (flush-to-zero and
/// denormals-are-zero): a subnormal operand or result is 0 there. Only the pitch shifter's splice
/// search needs it reproduced: during near-silence its correlation scores are built from
/// products far below `f32::MIN_POSITIVE` (the `DN` bias squared is already 1e-36), and which
/// candidate wins decides every later grain position.
#[inline(always)]
fn daz(x: f32) -> f32 {
    if x.abs() < f32::MIN_POSITIVE { 0.0 } else { x }
}

/// `buf[(size_t) i]` for a read: in range by construction; an impossible index reads 0.
#[inline(always)]
fn rd(buf: &[f32], i: i32) -> f32 {
    buf.get(i as usize).copied().unwrap_or(0.0)
}
/// `buf[(size_t) i] = v`: an impossible index writes nothing.
#[inline(always)]
fn wr(buf: &mut [f32], i: i32, v: f32) {
    if let Some(s) = buf.get_mut(i as usize) {
        *s = v;
    }
}

// ---------------------------------------------------------------- knob -> unit maps
// The UI has to print these same numbers. They live here, and only here, so a readout cannot drift
// away from what the DSP actually does.
/// 0.2 .. 30 s.
pub fn knob_to_decay_sec(v: f32) -> f32 {
    0.2 * 150.0f32.powf(clamp_t(v, 0.0, 1.0))
}
/// 0 .. 250 ms.
pub fn knob_to_pre_ms(v: f32) -> f32 {
    250.0 * v * v
}
/// 20 Hz .. 1 kHz.
pub fn knob_to_low_cut_hz(v: f32) -> f64 {
    20.0 * 50.0f64.powf(f64::from(v))
}
/// 1 k .. 20 k (also Damping).
pub fn knob_to_high_cut_hz(v: f32) -> f64 {
    1000.0 * 20.0f64.powf(f64::from(v))
}
/// 0.05 .. 8 Hz.
pub fn knob_to_mod_rate_hz(v: f32) -> f32 {
    0.05 * 160.0f32.powf(v)
}
/// 0.4x .. 2.5x.
pub fn knob_to_low_decay_x(v: f32) -> f32 {
    2.5f32.powf(2.0 * v - 1.0)
}
/// 50 ms .. 2 s.
pub fn knob_to_swell_sec(v: f32) -> f32 {
    0.05 * 40.0f32.powf(v)
}
pub fn knob_to_attack_ms(v: f32) -> f32 {
    250.0 * v
}
pub fn knob_to_bpm(v: f32) -> f32 {
    40.0 + 200.0 * v
}
/// 0.7071 = unity.
pub fn knob_to_level_gain(v: f32) -> f32 {
    2.0 * v * v
}

/// Pre-delay note values, in beats.
pub fn division_beats(i: i32) -> f32 {
    if i == 0 {
        return 0.0625; // 1/64
    }
    if i == 1 {
        return 0.125; // 1/32
    }
    if i == 2 {
        return 0.1666667; // 1/16T
    }
    if i == 3 {
        return 0.25; // 1/16
    }
    if i == 4 {
        return 0.3333333; // 1/8T
    }
    if i == 5 {
        return 0.375; // 1/16.
    }
    if i == 6 {
        return 0.5; // 1/8
    }
    if i == 7 {
        return 0.75; // 1/8.
    }
    1.0 // 1/4
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

#[derive(Debug, Clone, Default, PartialEq)]
struct Line {
    buf: Vec<f32>,
    pos: i32,
    /// Slewed length, samples.
    d_cur: f32,
    /// In-loop band split.
    lp_lo: f32,
    lp_hi: f32,
    /// Spring: in-loop DC block.
    dc_x: f32,
    dc_y: f32,
    /// Plate: in-loop dispersion.
    pdisp: [f32; K_PLATE_AP],
    /// Plate: the low shelf's own one-pole.
    lp_lo_b: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct LoopAp {
    b: Vec<f32>,
    pos: i32,
    len: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct PreDelay {
    buf: Vec<f32>,
    pos: i32,
    d_a: i32,
    d_b: i32,
    xf: f32,
    fading: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Shifter {
    ps: Vec<f32>,
    ps_pos: i32,
    ps_d0: f32,
    ps_d1: f32,
    ps_active: i32,
    ps_xf: f32,
    ps_fading: bool,
    /// pre + pre2 = 4th order when shifting up.
    pre: Biquad,
    pre2: Biquad,
    post: Biquad,
}

// ---------------------------------------------------------------- helpers
fn filter(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

fn set_lp(q: &mut Biquad, fc: f64, fsr: f64, qv: f64) {
    let k = (PI * fc / fsr).tan();
    let a0 = 1.0 + k / qv + k * k;
    q.b0 = ((k * k) / a0) as f32;
    q.b1 = ((2.0 * k * k) / a0) as f32;
    q.b2 = q.b0;
    q.a1 = ((2.0 * (k * k - 1.0)) / a0) as f32;
    q.a2 = ((1.0 - k / qv + k * k) / a0) as f32;
}

fn set_hp(q: &mut Biquad, fc: f64, fsr: f64, qv: f64) {
    let k = (PI * fc / fsr).tan();
    let a0 = 1.0 + k / qv + k * k;
    q.b0 = (1.0 / a0) as f32;
    q.b1 = (-2.0 / a0) as f32;
    q.b2 = q.b0;
    q.a1 = ((2.0 * (k * k - 1.0)) / a0) as f32;
    q.a2 = ((1.0 - k / qv + k * k) / a0) as f32;
}

fn one_pole_coef(fc: f64, fsr: f64) -> f32 {
    (-2.0 * PI * fc / fsr).exp() as f32
}

fn soft_limit(w: f32) -> f32 {
    let a = w.abs();
    if a <= 2.0 {
        return w;
    }
    let o = 2.0 + 2.0 * ((a - 2.0) / 2.0).tanh(); // safety only: nothing normal gets near this
    if w < 0.0 {
        return -o;
    }
    o
}

fn write_line(l: &mut Line, w: f32) {
    wr(&mut l.buf, l.pos, w);
    l.pos += 1;
    if l.pos >= LBUF {
        l.pos = 0;
    }
}

fn slew_line(l: &mut Line, target: f32) {
    let diff = target - l.d_cur;
    if diff.abs() < 0.01 {
        l.d_cur = target;
    } else {
        l.d_cur += diff * 0.0004; // Size bends the tail like a tape speed change instead of clicking
    }
}

/// Schroeder allpass whose length walks one sample at a time towards its target (no clicks when it
/// changes).
fn allpass(a: &mut LoopAp, x: f32, target: i32, buf_size: i32, g: f32) -> f32 {
    if a.len < target {
        a.len += 1;
    } else if a.len > target {
        a.len -= 1;
    }
    if a.len < 1 {
        return x;
    }
    let mut r = a.pos - a.len;
    if r < 0 {
        r += buf_size;
    }
    let v = rd(&a.b, r);
    let y = v - g * x;
    wr(&mut a.b, a.pos, x + g * y);
    a.pos += 1;
    if a.pos >= buf_size {
        a.pos = 0;
    }
    y
}

/// Pre-delay read. Fold rather than add PBUF twice: a long pre-delay plus a reflection tap can reach
/// further back than one wrap.
fn pre_read(p: &PreDelay, d: i32) -> f32 {
    let mut i = p.pos.wrapping_sub(1).wrapping_sub(d);
    i %= PBUF;
    if i < 0 {
        i += PBUF;
    }
    rd(&p.buf, i)
}

/// Hermite, the same interpolator `readLine` uses.
fn ps_read(l: &Shifter, delay_samples: f32) -> f32 {
    let d = clamp_t(delay_samples, 2.0, (PSBUF - 4) as f32);
    let di = d as i32;
    let fr = d - di as f32;

    let mut i1 = l.ps_pos - di;
    if i1 < 0 {
        i1 += PSBUF;
    }
    let mut i0 = i1 + 1;
    if i0 >= PSBUF {
        i0 -= PSBUF;
    }
    let mut i2 = i1 - 1;
    if i2 < 0 {
        i2 += PSBUF;
    }
    let mut i3 = i2 - 1;
    if i3 < 0 {
        i3 += PSBUF;
    }

    let y0 = rd(&l.ps, i0);
    let y1 = rd(&l.ps, i1);
    let y2 = rd(&l.ps, i2);
    let y3 = rd(&l.ps, i3);

    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * fr + c2) * fr + c1) * fr + y1
}

/// Cmajor's `.at()` wraps; a raw index here can go more than one buffer out on the correlation
/// search, so fold it properly rather than adding PSBUF once.
fn ps_at(l: &Shifter, delay_samples: i32) -> f32 {
    let mut i = l.ps_pos.wrapping_sub(delay_samples);
    i %= PSBUF;
    if i < 0 {
        i += PSBUF;
    }
    rd(&l.ps, i)
}

/// The C++ engine.
#[derive(Debug, Clone, PartialEq)]
pub struct ReverbEngineDsp {
    ln: [Line; 16],
    lap: [LoopAp; 16],
    /// Input diffusers: 0..3 left, 4..7 right.
    idf: [LoopAp; 8],
    pd: [PreDelay; 2],
    sh: [Shifter; 2],
    /// Spring loop delays.
    sp: [Line; 3],
    /// 3 springs x 100 allpasses x sprK samples.
    casc: Vec<f32>,
    out_lc: [Biquad; 2],
    out_hc: [Biquad; 2],
    /// Color: converter bandwidth on the way in (2 per channel)...
    col_in: [Biquad; 4],
    /// ...and the reconstruction filter on the way out.
    col_out: [Biquad; 2],
    spr_in_lp: [Biquad; 4],
    spr_out_lp: [Biquad; 3],
    spr_in_hp_x: f32,
    spr_in_hp_y: f32,
    sat_x1: f64,
    sat_f1: f64,

    // ---- parameter state
    decay_knob: f32,
    mix_knob: f32,
    size_knob: f32,
    pre_knob: f32,
    level_knob: f32,
    machine_idx: i32,
    pending_machine: i32,
    flush_len: i32,
    flush_swap_at: i32,
    bypassed: bool,
    frozen: bool,
    trails_on: bool,
    synced: bool,
    lc_knob: f32,
    hc_knob: f32,
    damp_knob: f32,
    bass_knob: f32,
    char_knob: f32,
    diff_knob: f32,
    rate_knob: f32,
    depth_knob: f32,
    width_knob: f32,
    duck_knob: f32,
    swell_knob: f32,
    interval_idx: i32,
    div_idx: i32,
    tempo_src_idx: i32,
    color_idx: i32,
    attack_knob: f32,
    attack_sm: f32,
    attack_mix: f32,
    use_linear: bool,
    use_color: bool,
    use_quant: bool,
    wander_amt: f32,
    wander_a: f32,
    wander_b: f32,
    wander_coef: f32,
    color_mod_scale: f32,
    color_damp_max: f32,
    rng: i64,
    coef_color: i32,
    col_mix_t: f32,
    col_mix: f32,
    tempo_knob: f32,
    host_bpm: f32,
    host_playing: bool,

    // ---- smoothed
    dry_g_sm: f32,
    wet_g_sm: f32,
    level_sm: f32,
    width_sm: f32,
    duck_sm: f32,
    char_sm: f32,
    gate_sm: f32,
    byp_sm: f32,
    freeze_sm: f32,
    wet_fade: f32,
    shim_sm: f32,
    er_gain_sm: f32,
    tail_gain_sm: f32,
    drive_sm: f32,
    plate_ap_a: f32,
    plate_ap_delay: f32,

    // ---- derived, control rate
    fs: f32,
    sr: f64,
    ctrl_counter: i32,
    flush_count: i32,
    lfo_phase_a: f32,
    lfo_phase_b: f32,
    lfo_inc_a: f32,
    lfo_inc_b: f32,
    mod_samples: f32,
    last_lc: f32,
    last_hc: f32,
    c_hi: f32,
    c_lo: f32,
    in_diff_g: f32,
    loop_ap_g: f32,
    duck_atk: f32,
    duck_rel: f32,
    duck_env: f32,
    env_fast: f32,
    env_slow: f32,
    fast_atk: f32,
    fast_rel: f32,
    slow_atk: f32,
    slow_rel: f32,
    swell_env: f32,
    swell_rise: f32,
    swell_fall_coef: f32,
    swell_fall: i32,
    swell_fall_len: i32,
    pre_target: f32,
    xf_inc: f32,

    // ---- per-machine table (set in applyMachine)
    line_ms: [f32; 16],
    in_diff_ms: [f32; 4],
    m_size_lo: f32,
    m_size_hi: f32,
    m_hf_ratio: f32,
    m_lo_xover: f32,
    m_ap_frac: f32,
    m_ap_gain: f32,
    m_mod_max_ms: f32,
    m_tail_trim: f32,
    m_diff_min: f32,
    /// Per-machine low-band decay ratio, multiplied into the user's Low Decay knob.
    m_bass_mult: f32,
    /// Scales the requested T60 for this machine.
    m_decay_scale: f32,
    /// Ceiling on the in-loop HF corner.
    m_damp_max: f32,
    use_er: bool,
    use_spring: bool,
    use_shimmer: bool,
    bloom_diff: bool,
    /// In-loop dispersion and shelving damping.
    use_plate_model: bool,

    // ---- per-line, control rate
    line_target: [f32; 16],
    line_gain: [f32; 16],
    r_low: [f32; 16],
    r_high: [f32; 16],
    ap_target: [i32; 16],
    /// 0..3 left, 4..7 right.
    in_diff_target: [i32; 8],

    // ---- early reflections: taps read straight out of the pre-delay buffers
    er_ms_l: [f32; 6],
    er_ms_r: [f32; 6],
    er_g: [f32; 6],
    er_tap_l: [i32; 6],
    er_tap_r: [i32; 6],

    // ---- attack ramp: eight taps off the pre-delay lines, getting louder
    atk_frac: [f32; 8],
    atk_g: [f32; 8],
    atk_target: [i32; 8],
    atk_cur: [i32; 8],

    // ---- spring
    spr_k: i32,
    spr_pos: i32,
    spr_a: f32,
    spr_damp_c: f32,
    spr_hp_c: f32,
    spr_makeup: f32,
    spr_ms: [f32; 3],
    spr_target: [f32; 3],
    spr_gain: [f32; 3],

    // ---- pitch shifter geometry
    pitch_ratio: f32,
    ps_window: f32,
    ps_xf_len: f32,
    ps_min_d: f32,
    /// See `updatePitchGuard()`.
    ps_guard_on: bool,
    ps_lag_step: i32,
    ps_seg_step: i32,
    ps_lag_count: i32,

    // ---- Rust additions: the last targets of the control-rate glides (for `reset_live`)
    tail_gain_target: f32,
    er_gain_target: f32,
    shim_target: f32,
}

impl Default for ReverbEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl ReverbEngineDsp {
    /// Every member at its declared initial value, with no buffer memory (`Vec::new` does not
    /// allocate).
    fn blank() -> Self {
        ReverbEngineDsp {
            ln: Default::default(),
            lap: Default::default(),
            idf: Default::default(),
            pd: Default::default(),
            sh: Default::default(),
            sp: Default::default(),
            casc: Vec::new(),
            out_lc: [Biquad::default(); 2],
            out_hc: [Biquad::default(); 2],
            col_in: [Biquad::default(); 4],
            col_out: [Biquad::default(); 2],
            spr_in_lp: [Biquad::default(); 4],
            spr_out_lp: [Biquad::default(); 3],
            spr_in_hp_x: 0.0,
            spr_in_hp_y: 0.0,
            sat_x1: 0.0,
            sat_f1: 0.0,
            decay_knob: 0.5041,
            mix_knob: 0.3,
            size_knob: 0.5,
            pre_knob: 0.2828,
            level_knob: 0.7071,
            machine_idx: 1,
            pending_machine: 1,
            flush_len: 14400,
            flush_swap_at: 12960,
            bypassed: false,
            frozen: false,
            trails_on: true,
            synced: false,
            lc_knob: 0.3544,
            hc_knob: 0.8295,
            damp_knob: 0.5981,
            bass_knob: 0.5,
            char_knob: 0.5,
            diff_knob: 0.7,
            rate_knob: 0.4537,
            depth_knob: 0.3,
            width_knob: 0.5,
            duck_knob: 0.0,
            swell_knob: 0.0,
            interval_idx: 0,
            div_idx: 3,
            tempo_src_idx: 0,
            color_idx: 2,
            attack_knob: 0.0,
            attack_sm: 0.0,
            attack_mix: 0.0,
            use_linear: false,
            use_color: false,
            use_quant: false,
            wander_amt: 0.0,
            wander_a: 0.0,
            wander_b: 0.0,
            wander_coef: 0.0,
            color_mod_scale: 1.0,
            color_damp_max: 20000.0,
            rng: 48271,
            coef_color: -1,
            col_mix_t: 0.0,
            col_mix: 0.0,
            tempo_knob: 0.4,
            host_bpm: 0.0,
            host_playing: false,
            dry_g_sm: 1.0,
            wet_g_sm: 0.0,
            level_sm: 1.0,
            width_sm: 1.0,
            duck_sm: 0.0,
            char_sm: 0.5,
            gate_sm: 1.0,
            byp_sm: 0.0,
            freeze_sm: 0.0,
            wet_fade: 1.0,
            shim_sm: 0.0,
            er_gain_sm: 0.0,
            tail_gain_sm: 1.0,
            drive_sm: 1.0,
            plate_ap_a: -0.55,
            plate_ap_delay: 0.0,
            fs: 48000.0,
            sr: 48000.0,
            ctrl_counter: 0,
            flush_count: 0,
            lfo_phase_a: 0.0,
            lfo_phase_b: 0.0,
            lfo_inc_a: 0.0,
            lfo_inc_b: 0.0,
            mod_samples: 0.0,
            last_lc: -1.0,
            last_hc: -1.0,
            c_hi: 0.5,
            c_lo: 0.03,
            in_diff_g: 0.5,
            loop_ap_g: 0.4,
            duck_atk: 0.0,
            duck_rel: 0.0,
            duck_env: 0.0,
            env_fast: 0.0,
            env_slow: 0.0,
            fast_atk: 0.0,
            fast_rel: 0.0,
            slow_atk: 0.0,
            slow_rel: 0.0,
            swell_env: 1.0,
            swell_rise: 0.0,
            swell_fall_coef: 0.99,
            swell_fall: 0,
            swell_fall_len: 240,
            pre_target: 960.0,
            xf_inc: 0.0,
            line_ms: [0.0; 16],
            in_diff_ms: [0.0; 4],
            m_size_lo: 0.6,
            m_size_hi: 1.5,
            m_hf_ratio: 0.35,
            m_lo_xover: 300.0,
            m_ap_frac: 0.27,
            m_ap_gain: 0.55,
            m_mod_max_ms: 1.2,
            m_tail_trim: 1.0,
            m_diff_min: 0.0,
            m_bass_mult: 1.0,
            m_decay_scale: 1.0,
            m_damp_max: 20000.0,
            use_er: true,
            use_spring: false,
            use_shimmer: false,
            bloom_diff: false,
            use_plate_model: false,
            line_target: [0.0; 16],
            line_gain: [0.0; 16],
            r_low: [0.0; 16],
            r_high: [0.0; 16],
            ap_target: [0; 16],
            in_diff_target: [0; 8],
            er_ms_l: [0.0; 6],
            er_ms_r: [0.0; 6],
            er_g: [0.0; 6],
            er_tap_l: [0; 6],
            er_tap_r: [0; 6],
            atk_frac: [0.0; 8],
            atk_g: [0.0; 8],
            atk_target: [0; 8],
            atk_cur: [0; 8],
            spr_k: 5,
            spr_pos: 0,
            spr_a: 0.62,
            spr_damp_c: 0.4,
            spr_hp_c: 0.992,
            spr_makeup: 1.0,
            spr_ms: [0.0; 3],
            spr_target: [0.0; 3],
            spr_gain: [0.0; 3],
            pitch_ratio: 2.0,
            ps_window: 4800.0,
            ps_xf_len: 960.0,
            ps_min_d: 600.0,
            ps_guard_on: false,
            ps_lag_step: 2,
            ps_seg_step: 4,
            ps_lag_count: 300,
            tail_gain_target: 0.0,
            er_gain_target: 0.0,
            shim_target: 0.0,
        }
    }

    /// `ReverbEngineDSP() { prepare (48000.0); }` -- the only place the buffers are allocated.
    pub fn new() -> Self {
        let mut d = Self::blank();
        d.prepare(48000.0);
        d
    }

    /// Puts the engine back exactly as [`ReverbEngineDsp::new`] makes it, in place and without
    /// allocating: every member at its declared initial value (the buffers are kept and zeroed),
    /// then the constructor's `prepare (48000.0)`. The host transport is the host's, not the
    /// engine's, and is kept.
    pub fn restore_initial(&mut self) {
        self.restore_members();
        self.prepare(48000.0);
    }

    /// [`ReverbEngineDsp::restore_initial`] followed by `prepare (sample_rate)`: a fresh engine as
    /// the C++ wrapper's `prepareToPlay` leaves it. The buffers are zeroed once (the constructor's
    /// `prepare` does not read them, and the second one zeroes them anyway).
    pub fn restore_and_prepare(&mut self, sample_rate: f64) {
        self.restore_members();
        self.prepare_impl(48000.0, false);
        self.prepare_impl(sample_rate, true);
    }

    /// Rust addition (the host's reset: Play, Stop, Locate, a bounce's start): `prepare` on the
    /// LIVE engine's own parameter state, as the C++ wrapper's `prepareToPlay` does, with every
    /// piece of audio state cleared (buffers, filters, envelopes, positions, the RNG) and the
    /// glides seeded at their targets, so the reset renders the same whatever came before it. A
    /// machine change still pending is completed now, and the current machine is kept: no flush,
    /// no fade-in. Allocation-free.
    ///
    /// Filter DESIGNS the C++ `prepare` never touches are kept, only their state cleared: the
    /// output EQ (`lastLc` / `lastHc` with `outLc` / `outHc`) and the colour converters
    /// (`coefColor` with `colIn` / `colOut`). So at 44.1 or 96 kHz the default cuts keep the
    /// constructor's 48 kHz designs through every reset, as a C++ instance keeps them through
    /// every `prepareToPlay`, until those knobs move.
    pub fn reset_live(&mut self) {
        let sr = self.sr;
        let designs = (self.last_lc, self.last_hc, self.out_lc, self.out_hc, self.coef_color, self.col_in, self.col_out);
        let k = (
            [
                self.decay_knob,
                self.mix_knob,
                self.size_knob,
                self.pre_knob,
                self.level_knob,
                self.lc_knob,
                self.hc_knob,
                self.damp_knob,
                self.bass_knob,
                self.char_knob,
                self.diff_knob,
                self.rate_knob,
                self.depth_knob,
                self.width_knob,
                self.duck_knob,
                self.swell_knob,
                self.tempo_knob,
                self.attack_knob,
            ],
            [self.pending_machine, self.interval_idx, self.div_idx, self.tempo_src_idx, self.color_idx],
            [self.bypassed, self.frozen, self.trails_on, self.synced],
        );
        self.restore_members();
        [
            self.decay_knob,
            self.mix_knob,
            self.size_knob,
            self.pre_knob,
            self.level_knob,
            self.lc_knob,
            self.hc_knob,
            self.damp_knob,
            self.bass_knob,
            self.char_knob,
            self.diff_knob,
            self.rate_knob,
            self.depth_knob,
            self.width_knob,
            self.duck_knob,
            self.swell_knob,
            self.tempo_knob,
            self.attack_knob,
        ] = k.0;
        let [machine, interval, div, tempo_src, color] = k.1;
        self.machine_idx = machine;
        self.pending_machine = machine;
        self.interval_idx = interval;
        self.div_idx = div;
        self.tempo_src_idx = tempo_src;
        self.color_idx = color;
        [self.bypassed, self.frozen, self.trails_on, self.synced] = k.2;
        (self.last_lc, self.last_hc, self.out_lc, self.out_hc, self.coef_color, self.col_in, self.col_out) = designs;
        for q in self.out_lc.iter_mut().chain(self.out_hc.iter_mut()).chain(self.col_in.iter_mut()).chain(self.col_out.iter_mut()) {
            q.z1 = 0.0;
            q.z2 = 0.0;
        }

        // The per-sample glides at their targets (the `process` smoothing, settled).
        self.char_sm = self.char_knob;
        self.width_sm = self.width_knob * 2.0;
        self.duck_sm = self.duck_knob;
        self.level_sm = knob_to_level_gain(self.level_knob);
        let wm = min_t(1.0, 2.0 * self.mix_knob);
        let dm = min_t(1.0, 2.0 * (1.0 - self.mix_knob));
        self.wet_g_sm = wm * wm.sqrt();
        self.dry_g_sm = dm;
        self.gate_sm = if self.bypassed || self.frozen { 0.0 } else { 1.0 };
        self.byp_sm = if self.bypassed { 1.0 } else { 0.0 };
        self.freeze_sm = if self.frozen { 1.0 } else { 0.0 };
        self.wet_fade = 1.0;
        self.attack_sm = self.attack_knob;

        // `prepare` (zeroes the buffers, applies the interval and machine, runs `updateControl`,
        // seeds the line lengths and the pre-delay), then the control-rate glides at their targets.
        self.prepare(sr);
        if self.use_color && self.coef_color == self.color_idx {
            self.col_mix = 1.0;
        }
        self.update_control();
        self.tail_gain_sm = self.tail_gain_target;
        self.er_gain_sm = self.er_gain_target;
        self.shim_sm = self.shim_target;
        self.col_mix_t = if self.use_color { 1.0 } else { 0.0 };
        self.atk_cur = self.atk_target;
        self.ctrl_counter = 0;
    }

    fn restore_members(&mut self) {
        let mut fresh = Self::blank();
        for (a, b) in fresh.ln.iter_mut().zip(self.ln.iter_mut()) {
            std::mem::swap(&mut a.buf, &mut b.buf);
        }
        for (a, b) in fresh.sp.iter_mut().zip(self.sp.iter_mut()) {
            std::mem::swap(&mut a.buf, &mut b.buf);
        }
        for (a, b) in fresh.lap.iter_mut().zip(self.lap.iter_mut()) {
            std::mem::swap(&mut a.b, &mut b.b);
        }
        for (a, b) in fresh.idf.iter_mut().zip(self.idf.iter_mut()) {
            std::mem::swap(&mut a.b, &mut b.b);
        }
        for (a, b) in fresh.pd.iter_mut().zip(self.pd.iter_mut()) {
            std::mem::swap(&mut a.buf, &mut b.buf);
        }
        for (a, b) in fresh.sh.iter_mut().zip(self.sh.iter_mut()) {
            std::mem::swap(&mut a.ps, &mut b.ps);
        }
        std::mem::swap(&mut fresh.casc, &mut self.casc);
        fresh.host_bpm = self.host_bpm;
        fresh.host_playing = self.host_playing;
        // The old members now hold only empty vectors: nothing is freed here.
        *self = fresh;
    }

    // ------------------------------------------------------------------ host transport
    /// Amorph fed a hidden 6-value packet; JUCE has AudioPlayHead, so the processor calls this.
    pub fn set_host_transport(&mut self, playing: bool, bpm: f32) {
        self.host_playing = playing;
        self.host_bpm = if (20.0..=999.0).contains(&bpm) { bpm } else { 0.0 };
    }

    pub fn get_host_bpm(&self) -> f32 {
        self.host_bpm
    }

    // ------------------------------------------------------------------ events
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        match index {
            P_DECAY => self.decay_knob = v,
            P_MIX => self.mix_knob = v,
            // fade out, swap, flush, fade in -- so one machine never bursts out of another
            P_MACHINE => {
                let m = (v + 0.5) as i32;
                if m != self.pending_machine {
                    self.pending_machine = m;
                    self.flush_count = self.flush_len;
                }
            }
            P_SIZE => self.size_knob = v,
            P_PRE_DELAY => self.pre_knob = v,
            P_BYPASS => self.bypassed = v > 0.5,
            P_LEVEL => self.level_knob = v,
            P_LOW_CUT => self.lc_knob = v,
            P_HIGH_CUT => self.hc_knob = v,
            P_DAMPING => self.damp_knob = v,
            P_BASS => self.bass_knob = v,
            P_CHARACTER => self.char_knob = v,
            P_DIFFUSION => self.diff_knob = v,
            P_MOD_RATE => self.rate_knob = v,
            P_MOD_DEPTH => self.depth_knob = v,
            P_WIDTH => self.width_knob = v,
            P_DUCK => self.duck_knob = v,
            P_SWELL => self.swell_knob = v,
            P_FREEZE => self.frozen = v > 0.5,
            P_TRAILS => self.trails_on = v > 0.5,
            P_INTERVAL => {
                self.interval_idx = (v + 0.5) as i32;
                self.apply_pitch();
            }
            P_SYNC => self.synced = v > 0.5,
            P_DIVISION => self.div_idx = (v + 0.5) as i32,
            P_TEMPO => self.tempo_knob = v,
            P_TEMPO_SRC => self.tempo_src_idx = (v + 0.5) as i32,
            P_COLOR => self.color_idx = (v + 0.5) as i32,
            P_ATTACK => self.attack_knob = v,
            _ => {}
        }
    }

    // ------------------------------------------------------------------ init
    pub fn prepare(&mut self, sample_rate: f64) {
        self.prepare_impl(sample_rate, true);
    }

    /// `l.buf.assign (N, 0.0f)`: allocates only while the buffer is not yet at its (fixed) size,
    /// which is only ever in `new`.
    fn assign(buf: &mut Vec<f32>, n: i32, clear: bool) {
        let n = n.max(0) as usize;
        if buf.len() != n {
            *buf = vec![0.0; n];
        } else if clear {
            buf.fill(0.0);
        }
    }

    fn prepare_impl(&mut self, sample_rate: f64, clear: bool) {
        // Rust addition: a rate the engine's `int` sample counts can hold (the C++ would overflow).
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate.min(768_000.0) } else { 48_000.0 };
        self.fs = sample_rate as f32;
        self.sr = sample_rate;

        for l in &mut self.ln {
            Self::assign(&mut l.buf, LBUF, clear);
        }
        for l in &mut self.sp {
            Self::assign(&mut l.buf, LBUF, clear);
        }
        for a in &mut self.lap {
            Self::assign(&mut a.b, ABUF, clear);
        }
        for a in &mut self.idf {
            Self::assign(&mut a.b, DBUF, clear);
        }
        for p in &mut self.pd {
            Self::assign(&mut p.buf, PBUF, clear);
        }
        for s in &mut self.sh {
            Self::assign(&mut s.ps, PSBUF, clear);
        }
        Self::assign(&mut self.casc, 3 * SPR_M * SPR_K, clear);

        self.er_ms_l = [3.1, 7.9, 13.3, 19.7, 27.1, 35.9];
        self.er_ms_r = [4.3, 9.7, 15.1, 22.3, 29.9, 39.1];
        self.er_g = [0.80, -0.66, 0.55, -0.45, 0.36, -0.28];

        // attack ramp: eight taps, louder towards the end, mixed signs so the lows don't pile up.
        self.atk_frac = [0.09, 0.23, 0.36, 0.50, 0.61, 0.74, 0.87, 1.00];
        self.atk_g = [0.070, -0.140, 0.210, 0.280, -0.350, 0.420, -0.490, 0.560];
        self.wander_coef = 1.0 - one_pole_coef(2.5, self.sr);

        self.spr_ms = [66.0, 82.0, 74.5];

        // spring stretch: transition frequency = fs / (2K), as close to 4.3 kHz as whole samples allow
        self.spr_k = (self.fs / 8600.0 + 0.5) as i32;
        if self.spr_k < 2 {
            self.spr_k = 2;
        }
        if self.spr_k > SPR_K {
            self.spr_k = SPR_K;
        }
        let f_trans = 0.5 * self.sr / f64::from(self.spr_k);
        let sr = self.sr;
        set_lp(&mut self.spr_in_lp[0], 0.9 * f_trans, sr, 0.5098); // 8th-order Butterworth
        set_lp(&mut self.spr_in_lp[1], 0.9 * f_trans, sr, 0.6013);
        set_lp(&mut self.spr_in_lp[2], 0.9 * f_trans, sr, 0.9000);
        set_lp(&mut self.spr_in_lp[3], 0.9 * f_trans, sr, 2.5629);
        for s in 0..3 {
            set_lp(&mut self.spr_out_lp[s], 0.97 * f_trans, sr, 0.7071);
        }
        self.spr_hp_c = one_pole_coef(70.0, sr);

        let fs = self.fs;
        for c in 0..2 {
            self.sh[c].ps_d0 = 0.05 * fs;
            self.sh[c].ps_d1 = 0.05 * fs;
            self.sh[c].ps_xf = 1.0;
            set_lp(&mut self.sh[c].post, 9000.0, sr, 0.7071); // the pre pair is set by updatePitchGuard()
        }

        self.ps_xf_len = 0.02 * fs;
        let mut m = (fs / 48000.0 + 0.5) as i32;
        if m < 1 {
            m = 1;
        }
        self.ps_lag_step = 2 * m;
        self.ps_seg_step = 4 * m;
        self.ps_lag_count = 300;
        self.ps_min_d = (128 * self.ps_seg_step) as f32 + 16.0;
        self.ps_window = min_t(0.10 * fs, PSBUF as f32 - 0.03 * fs - 64.0); // applyPitch() resizes it

        self.duck_atk = 1.0 - one_pole_coef(80.0, sr);
        self.duck_rel = 1.0 - one_pole_coef(0.6, sr);
        self.fast_atk = 1.0 - one_pole_coef(320.0, sr);
        self.fast_rel = 1.0 - one_pole_coef(8.0, sr);
        self.slow_atk = 1.0 - one_pole_coef(8.0, sr);
        self.slow_rel = 1.0 - one_pole_coef(1.1, sr);
        self.swell_fall_len = (0.006 * fs) as i32;
        self.swell_fall_coef = one_pole_coef(160.0, sr);
        self.xf_inc = 1.0 / (0.04 * fs);
        self.flush_len = (0.33 * fs) as i32;
        self.flush_swap_at = self.flush_len - (0.03 * fs) as i32; // the wet has faded out by then
        self.pending_machine = self.machine_idx;

        self.level_sm = knob_to_level_gain(self.level_knob);
        self.width_sm = self.width_knob * 2.0;
        self.char_sm = self.char_knob;

        self.apply_pitch();
        self.apply_machine();
        self.update_control();
        self.tail_gain_sm = 0.0;
        for i in 0..16 {
            self.ln[i].d_cur = self.line_target[i];
            self.lap[i].len = self.ap_target[i];
        }
        for i in 0..8 {
            self.idf[i].len = self.in_diff_target[i];
        }
        for s in 0..3 {
            self.sp[s].d_cur = self.spr_target[s];
        }
        for c in 0..2 {
            self.pd[c].d_a = self.pre_target as i32;
            self.pd[c].d_b = self.pd[c].d_a;
        }
    }

    // ------------------------------------------------------------------ main
    /// `process (float* const* channels, int numChannels, int numSamples)`: `io[0]` is left, `io[1]`
    /// (when `num_channels > 1`) right; `num_samples` must not exceed any used buffer's length (the
    /// wrapper guarantees it; the loop also stops at the shortest).
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        if num_channels < 1 || self.ln[0].buf.is_empty() {
            return;
        }
        let (left, rest) = match io.split_first_mut() {
            Some(x) => x,
            None => return,
        };
        let mut ch_r: Option<&mut Vec<f32>> = if num_channels > 1 { rest.first_mut() } else { None };
        let n_max = num_samples.min(left.len()).min(ch_r.as_ref().map_or(usize::MAX, |r| r.len()));

        for n in 0..n_max {
            let x0 = left[n];
            let x1 = match &ch_r {
                Some(r) => r[n],
                None => x0,
            };

            // ---- smoothing
            let k = 0.005f32;
            self.char_sm += (self.char_knob - self.char_sm) * k;
            self.width_sm += (self.width_knob * 2.0 - self.width_sm) * k;
            self.duck_sm += (self.duck_knob - self.duck_sm) * k;
            self.level_sm += (knob_to_level_gain(self.level_knob) - self.level_sm) * k;

            // Mix: dry stays at unity up to noon, wet comes up underneath it. 100 % = wet only.
            let wm = min_t(1.0, 2.0 * self.mix_knob);
            let dm = min_t(1.0, 2.0 * (1.0 - self.mix_knob));
            self.wet_g_sm += (wm * wm.sqrt() - self.wet_g_sm) * k;
            self.dry_g_sm += (dm - self.dry_g_sm) * k;

            let (mut gate_t, mut byp_t, mut frz_t, mut fade_t) = (1.0f32, 0.0f32, 0.0f32, 1.0f32);
            if self.bypassed {
                gate_t = 0.0;
                byp_t = 1.0;
            }
            if self.frozen {
                gate_t = 0.0;
                frz_t = 1.0;
            }
            if self.flush_count > 0 {
                self.flush_count -= 1;
                fade_t = 0.0;
                gate_t = 0.0;
                if self.flush_count == self.flush_swap_at && self.pending_machine != self.machine_idx {
                    self.machine_idx = self.pending_machine;
                    self.apply_machine();
                }
            }
            self.gate_sm += (gate_t - self.gate_sm) * 0.002 + DN; // DN: keeps decays out of denormal range
            self.byp_sm += (byp_t - self.byp_sm) * 0.002 + DN;
            self.freeze_sm += (frz_t - self.freeze_sm) * 0.002 + DN;
            self.wet_fade += (fade_t - self.wet_fade) * 0.004 + DN;

            self.ctrl_counter += 1;
            if self.ctrl_counter >= 32 {
                self.ctrl_counter = 0;
                self.update_control();
            }

            // ---- modulators: two rates, four phases each -> every line moves differently
            self.lfo_phase_a += self.lfo_inc_a;
            if self.lfo_phase_a >= 1.0 {
                self.lfo_phase_a -= 1.0;
            }
            self.lfo_phase_b += self.lfo_inc_b;
            if self.lfo_phase_b >= 1.0 {
                self.lfo_phase_b -= 1.0;
            }
            let tp = TWO_PI as f32;
            let (mut s_a, mut c_a, mut s_b, mut c_b) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            if self.mod_samples > 0.01 {
                s_a = self.mod_samples * (tp * self.lfo_phase_a).sin();
                c_a = self.mod_samples * (tp * self.lfo_phase_a).cos();
                s_b = self.mod_samples * (tp * self.lfo_phase_b).sin();
                c_b = self.mod_samples * (tp * self.lfo_phase_b).cos();

                if self.wander_amt > 0.0 {
                    // 1970s: the sweep isn't a clean sine, it drifts
                    let w = self.white();
                    self.wander_a += (w - self.wander_a) * self.wander_coef + DN;
                    let w = self.white();
                    self.wander_b += (w - self.wander_b) * self.wander_coef + DN;
                    let wa = self.wander_amt * self.mod_samples * 14.0 * self.wander_a;
                    let wb = self.wander_amt * self.mod_samples * 14.0 * self.wander_b;
                    s_a += wa;
                    c_a += wb;
                    s_b -= wb;
                    c_b += wa;
                }
            }

            // ---- duck + swell, both from the dry signal
            let in_m = 0.5 * (x0 + x1);
            let lvl = in_m.abs() + 1.0e-12;
            if lvl > self.duck_env {
                self.duck_env += (lvl - self.duck_env) * self.duck_atk;
            } else {
                self.duck_env += (lvl - self.duck_env) * self.duck_rel;
            }
            let duck_g = 1.0 - self.duck_sm * 0.95 * clamp_t(self.duck_env * 8.0, 0.0, 1.0);

            if lvl > self.env_fast {
                self.env_fast += (lvl - self.env_fast) * self.fast_atk;
            } else {
                self.env_fast += (lvl - self.env_fast) * self.fast_rel;
            }
            if lvl > self.env_slow {
                self.env_slow += (lvl - self.env_slow) * self.slow_atk;
            } else {
                self.env_slow += (lvl - self.env_slow) * self.slow_rel;
            }

            let mut swell_g = 1.0f32;
            if self.swell_rise > 0.0 {
                // a new pick attack shuts the send, then it fades back up: the reverb hears a bowed note
                if self.env_fast > self.env_slow * 1.35 && self.env_fast > 0.003 {
                    self.swell_fall = self.swell_fall_len;
                }
                if self.swell_fall > 0 {
                    self.swell_fall -= 1;
                    self.swell_env = self.swell_env * self.swell_fall_coef + DN;
                } else {
                    self.swell_env += (1.0 - self.swell_env) * self.swell_rise;
                }
                swell_g = self.swell_env;
            } else {
                self.swell_env = 1.0;
            }

            // ---- send -> pre-delay
            let send_g = self.gate_sm * swell_g;
            let p_t = (self.pre_target + 0.5) as i32;
            let xf_inc = self.xf_inc;
            let p_l = Self::pre_process(&mut self.pd[0], xf_inc, x0 * send_g, p_t) + DN;
            let p_r = Self::pre_process(&mut self.pd[1], xf_inc, x1 * send_g, p_t) + DN;

            // (`float tailL = 0, tailR = 0`: both branches set them)
            let (mut tail_l, mut tail_r): (f32, f32);
            let (mut er_l, mut er_r) = (0.0f32, 0.0f32);

            if self.use_spring {
                // tank driver: band-limit, lose the lows, saturate (Dwell)
                let mut m = 0.5 * (p_l + p_r);
                for q in 0..4 {
                    m = filter(&mut self.spr_in_lp[q], m);
                }
                let hp = m - self.spr_in_hp_x + self.spr_hp_c * self.spr_in_hp_y;
                self.spr_in_hp_x = m;
                self.spr_in_hp_y = hp;
                let dr = self.sat_adaa(hp * self.drive_sm * 2.0) * self.spr_makeup + DN;

                self.spr_pos += 1;
                if self.spr_pos >= self.spr_k {
                    self.spr_pos = 0;
                }

                let s0 = self.spring(0, dr, s_a);
                let s1 = self.spring(1, dr, c_a);
                let s2 = self.spring(2, dr, s_b);
                tail_l = 0.8 * s0 + 0.5 * s2;
                tail_r = 0.8 * s1 + 0.5 * s2;
            } else {
                // ---- early reflections, straight off the pre-delay lines
                if self.use_er {
                    // (they crossfade along with the pre-delay when its time changes)
                    let fa = 1.0 - self.pd[0].xf;
                    let fb = self.pd[0].xf;
                    for t in 0..6 {
                        er_l += self.er_g[t] * fa * pre_read(&self.pd[0], self.pd[0].d_a + self.er_tap_l[t]);
                        er_r += self.er_g[t] * fa * pre_read(&self.pd[1], self.pd[1].d_a + self.er_tap_r[t]);
                        if fb > 0.0 {
                            er_l += self.er_g[t] * fb * pre_read(&self.pd[0], self.pd[0].d_b + self.er_tap_l[t]);
                            er_r += self.er_g[t] * fb * pre_read(&self.pd[1], self.pd[1].d_b + self.er_tap_r[t]);
                        }
                    }
                }

                let (mut t_l, mut t_r) = (p_l, p_r);

                // Attack: the tail is fed from a rising ramp of taps instead of from the note itself
                if self.attack_mix > 0.001 {
                    let (mut a_l, mut a_r) = (0.0f32, 0.0f32);
                    for a in 0..8 {
                        if self.atk_cur[a] < self.atk_target[a] {
                            self.atk_cur[a] += 1;
                        } else if self.atk_cur[a] > self.atk_target[a] {
                            self.atk_cur[a] -= 1;
                        }
                        a_l += self.atk_g[a] * pre_read(&self.pd[0], self.pd[0].d_a + self.atk_cur[a]);
                        a_r += self.atk_g[a] * pre_read(&self.pd[1], self.pd[1].d_a + self.atk_cur[a] + 23);
                    }
                    t_l = t_l * (1.0 - self.attack_mix) + a_l * self.attack_mix;
                    t_r = t_r * (1.0 - self.attack_mix) + a_r * self.attack_mix;
                }

                // Color: the converters in front of the algorithm
                self.col_mix += (self.col_mix_t - self.col_mix) * 0.002 + DN;
                if self.col_mix > 0.0005 {
                    let f = filter(&mut self.col_in[0], t_l);
                    t_l += (filter(&mut self.col_in[1], f) - t_l) * self.col_mix;
                    let f = filter(&mut self.col_in[2], t_r);
                    t_r += (filter(&mut self.col_in[3], f) - t_r) * self.col_mix;
                    if self.use_quant {
                        // 12 bits with dither: hiss rather than crunch, and it reverberates
                        let d_l = (self.white() + self.white()) * 0.5;
                        let d_r = (self.white() + self.white()) * 0.5;
                        // (the grain rides on the signal and goes quiet when you do)
                        let q_amt = self.col_mix * self.gate_sm * clamp_t(self.env_slow * 40.0, 0.0, 1.0);
                        t_l += ((t_l * 2048.0 + 0.5 + d_l).floor() * 0.00048828125 - t_l) * q_amt;
                        t_r += ((t_r * 2048.0 + 0.5 + d_r).floor() * 0.00048828125 - t_r) * q_amt;
                    }
                }

                for d in 0..4 {
                    t_l = allpass(&mut self.idf[d], t_l, self.in_diff_target[d], DBUF, self.in_diff_g);
                    t_r = allpass(&mut self.idf[d + 4], t_r, self.in_diff_target[d + 4], DBUF, self.in_diff_g);
                }

                // ---- the network: read, damp, diffuse, (shift), mix, write
                let mut r = [0.0f32; 16];
                let mut v = [0.0f32; 16];
                for i in 0..16 {
                    slew_line(&mut self.ln[i], self.line_target[i]);

                    // two LFO rates x four phases, and every other pair moves a little less
                    let ph = i % 8;
                    let mut m = match ph {
                        0 => s_a,
                        1 => c_a,
                        2 => -s_a,
                        3 => -c_a,
                        4 => s_b,
                        5 => c_b,
                        6 => -s_b,
                        _ => -c_b,
                    };
                    if i >= 8 {
                        m *= -0.62;
                    }

                    let rdv = self.read_line(&self.ln[i], self.ln[i].d_cur + m);
                    r[i] = rdv;

                    let g = self.line_gain[i] + (1.0 - self.line_gain[i]) * self.freeze_sm;
                    let rl = self.r_low[i] + (1.0 - self.r_low[i]) * self.freeze_sm;
                    let rh = self.r_high[i] + (1.0 - self.r_high[i]) * self.freeze_sm;

                    let (c_lo, c_hi) = (self.c_lo, self.c_hi);
                    let l = &mut self.ln[i];
                    l.lp_lo += (rdv - l.lp_lo) * c_lo;
                    l.lp_hi += (rdv - l.lp_hi) * c_hi;

                    let dmp = if self.use_plate_model {
                        // Two first-order shelves rather than three summed bands (see the C++).
                        let lp = l.lp_hi;
                        let hfs = lp + rh * (rdv - lp);

                        l.lp_lo_b += (hfs - l.lp_lo_b) * c_lo;
                        let lo2 = l.lp_lo_b;
                        g * ((hfs - lo2) + rl * lo2)
                    } else {
                        // three bands, each with its own decay. Freeze pulls all three to unity.
                        let lo = l.lp_lo;
                        let mid = l.lp_hi - lo;
                        let hi = rdv - l.lp_hi;
                        g * (rl * lo + mid + rh * hi)
                    };

                    v[i] = allpass(&mut self.lap[i], dmp, self.ap_target[i], ABUF, self.loop_ap_g);
                    if self.use_plate_model {
                        v[i] = Self::plate_disperse(self.plate_ap_a, &mut self.ln[i], v[i]);
                    }
                }

                if self.use_shimmer {
                    let keep = max_t(0.0, 1.0 - self.shim_sm * self.shim_sm).sqrt();
                    let x = self.ps_pre_filter(0, 0.7071 * (v[0] + v[8]));
                    let y = self.pitch_shift(0, x);
                    let p0 = filter(&mut self.sh[0].post, y);
                    let x = self.ps_pre_filter(1, 0.7071 * (v[5] + v[13]));
                    let y = self.pitch_shift(1, x);
                    let p1 = filter(&mut self.sh[1].post, y);
                    v[0] = keep * v[0] + self.shim_sm * 0.7071 * p0;
                    v[8] = keep * v[8] + self.shim_sm * 0.7071 * p0;
                    v[5] = keep * v[5] + self.shim_sm * 0.7071 * p1;
                    v[13] = keep * v[13] + self.shim_sm * 0.7071 * p1;
                }

                // 16x16 Hadamard: every line feeds every other line, and no energy is made or lost
                let h0_0 = v[0] + v[1];
                let h0_1 = v[0] - v[1];
                let h0_2 = v[2] + v[3];
                let h0_3 = v[2] - v[3];
                let h0_4 = v[4] + v[5];
                let h0_5 = v[4] - v[5];
                let h0_6 = v[6] + v[7];
                let h0_7 = v[6] - v[7];
                let h0_8 = v[8] + v[9];
                let h0_9 = v[8] - v[9];
                let h0_10 = v[10] + v[11];
                let h0_11 = v[10] - v[11];
                let h0_12 = v[12] + v[13];
                let h0_13 = v[12] - v[13];
                let h0_14 = v[14] + v[15];
                let h0_15 = v[14] - v[15];
                let h1_0 = h0_0 + h0_2;
                let h1_2 = h0_0 - h0_2;
                let h1_1 = h0_1 + h0_3;
                let h1_3 = h0_1 - h0_3;
                let h1_4 = h0_4 + h0_6;
                let h1_6 = h0_4 - h0_6;
                let h1_5 = h0_5 + h0_7;
                let h1_7 = h0_5 - h0_7;
                let h1_8 = h0_8 + h0_10;
                let h1_10 = h0_8 - h0_10;
                let h1_9 = h0_9 + h0_11;
                let h1_11 = h0_9 - h0_11;
                let h1_12 = h0_12 + h0_14;
                let h1_14 = h0_12 - h0_14;
                let h1_13 = h0_13 + h0_15;
                let h1_15 = h0_13 - h0_15;
                let h2_0 = h1_0 + h1_4;
                let h2_4 = h1_0 - h1_4;
                let h2_1 = h1_1 + h1_5;
                let h2_5 = h1_1 - h1_5;
                let h2_2 = h1_2 + h1_6;
                let h2_6 = h1_2 - h1_6;
                let h2_3 = h1_3 + h1_7;
                let h2_7 = h1_3 - h1_7;
                let h2_8 = h1_8 + h1_12;
                let h2_12 = h1_8 - h1_12;
                let h2_9 = h1_9 + h1_13;
                let h2_13 = h1_9 - h1_13;
                let h2_10 = h1_10 + h1_14;
                let h2_14 = h1_10 - h1_14;
                let h2_11 = h1_11 + h1_15;
                let h2_15 = h1_11 - h1_15;
                let h3_0 = h2_0 + h2_8;
                let h3_8 = h2_0 - h2_8;
                let h3_1 = h2_1 + h2_9;
                let h3_9 = h2_1 - h2_9;
                let h3_2 = h2_2 + h2_10;
                let h3_10 = h2_2 - h2_10;
                let h3_3 = h2_3 + h2_11;
                let h3_11 = h2_3 - h2_11;
                let h3_4 = h2_4 + h2_12;
                let h3_12 = h2_4 - h2_12;
                let h3_5 = h2_5 + h2_13;
                let h3_13 = h2_5 - h2_13;
                let h3_6 = h2_6 + h2_14;
                let h3_14 = h2_6 - h2_14;
                let h3_7 = h2_7 + h2_15;
                let h3_15 = h2_7 - h2_15;
                let hs = 0.25f32;
                let i_l = 0.3536 * t_l;
                let i_r = 0.3536 * t_r;
                write_line(&mut self.ln[0], soft_limit(hs * h3_0 + i_l));
                write_line(&mut self.ln[1], soft_limit(hs * h3_1 + i_r));
                write_line(&mut self.ln[2], soft_limit(hs * h3_2 - i_l));
                write_line(&mut self.ln[3], soft_limit(hs * h3_3 - i_r));
                write_line(&mut self.ln[4], soft_limit(hs * h3_4 + i_l));
                write_line(&mut self.ln[5], soft_limit(hs * h3_5 + i_r));
                write_line(&mut self.ln[6], soft_limit(hs * h3_6 - i_l));
                write_line(&mut self.ln[7], soft_limit(hs * h3_7 - i_r));
                write_line(&mut self.ln[8], soft_limit(hs * h3_8 + i_l));
                write_line(&mut self.ln[9], soft_limit(hs * h3_9 + i_r));
                write_line(&mut self.ln[10], soft_limit(hs * h3_10 - i_l));
                write_line(&mut self.ln[11], soft_limit(hs * h3_11 - i_r));
                write_line(&mut self.ln[12], soft_limit(hs * h3_12 + i_l));
                write_line(&mut self.ln[13], soft_limit(hs * h3_13 + i_r));
                write_line(&mut self.ln[14], soft_limit(hs * h3_14 - i_l));
                write_line(&mut self.ln[15], soft_limit(hs * h3_15 - i_r));

                tail_l = 0.7071 * (r[0] + r[2] - r[4] - r[6] + r[8] + r[10] - r[12] - r[14]);
                tail_r = 0.7071 * (r[1] + r[3] - r[5] - r[7] + r[9] + r[11] - r[13] - r[15]);

                if self.col_mix > 0.0005 {
                    tail_l += (filter(&mut self.col_out[0], tail_l) - tail_l) * self.col_mix;
                    tail_r += (filter(&mut self.col_out[1], tail_r) - tail_r) * self.col_mix;
                }
            }

            // ---- wet: level, EQ, width, duck
            let mut wet_l = tail_l * self.tail_gain_sm + er_l * self.er_gain_sm;
            let mut wet_r = tail_r * self.tail_gain_sm + er_r * self.er_gain_sm;
            let f = filter(&mut self.out_lc[0], wet_l);
            wet_l = filter(&mut self.out_hc[0], f);
            let f = filter(&mut self.out_lc[1], wet_r);
            wet_r = filter(&mut self.out_hc[1], f);

            let mm = 0.5 * (wet_l + wet_r);
            let ss = 0.5 * (wet_l - wet_r) * self.width_sm;
            let w_g = self.wet_g_sm * duck_g * self.level_sm * self.wet_fade;
            let w_l = (mm + ss) * w_g;
            let w_r = (mm - ss) * w_g;

            // ---- output. Bypassed = the untouched input, plus the tail if Trails is on.
            let mut tail = 0.0f32;
            if self.trails_on {
                tail = 1.0;
            }

            let y0 = (1.0 - self.byp_sm) * (x0 * self.dry_g_sm * self.level_sm + w_l) + self.byp_sm * (x0 + w_l * tail);
            let y1 = (1.0 - self.byp_sm) * (x1 * self.dry_g_sm * self.level_sm + w_r) + self.byp_sm * (x1 + w_r * tail);

            left[n] = y0;
            if let Some(r) = ch_r.as_mut() {
                r[n] = y1;
            }
        }
    }

    // ------------------------------------------------------------------ helpers (members)
    fn white(&mut self) -> f32 {
        self.rng = (self.rng * 1103515245i64 + 12345i64) % 2147483647i64;
        self.rng as f32 / 1073741824.0 - 1.0
    }

    /// tanh with first-order antiderivative anti-aliasing (the spring tank driver). F(x) = ln cosh x.
    fn sat_adaa(&mut self, u: f32) -> f32 {
        let x1 = f64::from(u);
        let ax = x1.abs();
        let f1 = ax + (1.0 + (-2.0 * ax).exp()).ln() - 0.6931471805599453;
        let dx = x1 - self.sat_x1;
        let y = if dx.abs() < 1.0e-4 { (0.5 * (x1 + self.sat_x1)).tanh() } else { (f1 - self.sat_f1) / dx };
        self.sat_x1 = x1;
        self.sat_f1 = f1;
        y as f32
    }

    // ------------------------------------------------------------------ delay lines
    fn read_line(&self, l: &Line, delay_samples: f32) -> f32 {
        let d = clamp_t(delay_samples, 2.0, (LBUF - 8) as f32);
        let di = d as i32;
        let fr = d - di as f32;

        let mut i1 = l.pos - di;
        if i1 < 0 {
            i1 += LBUF;
        }
        let y1 = rd(&l.buf, i1);
        if fr < 0.0005 {
            return y1; // whole-sample read: exact, so Freeze never dulls
        }

        if self.use_linear {
            // what the old boxes did: two-point interpolation.
            let mut j = i1 - 1;
            if j < 0 {
                j += LBUF;
            }
            return y1 + fr * (rd(&l.buf, j) - y1);
        }

        let mut i0 = i1 + 1;
        if i0 >= LBUF {
            i0 -= LBUF;
        }
        let mut i2 = i1 - 1;
        if i2 < 0 {
            i2 += LBUF;
        }
        let mut i3 = i2 - 1;
        if i3 < 0 {
            i3 += LBUF;
        }

        let y0 = rd(&l.buf, i0);
        let y2 = rd(&l.buf, i2);
        let y3 = rd(&l.buf, i3);

        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * fr + c2) * fr + c1) * fr + y1
    }

    // ------------------------------------------------------------------ pre-delay (crossfades to a new time, no pitch bend)
    fn pre_process(p: &mut PreDelay, xf_inc: f32, x: f32, target: i32) -> f32 {
        wr(&mut p.buf, p.pos, x);
        p.pos += 1;
        if p.pos >= PBUF {
            p.pos = 0;
        }

        if !p.fading && target != p.d_a {
            p.d_b = target;
            p.fading = true;
            p.xf = 0.0;
        }

        let a = pre_read(p, p.d_a);
        if !p.fading {
            return a;
        }

        let b = pre_read(p, p.d_b);
        let y = a * (1.0 - p.xf) + b * p.xf;
        p.xf += xf_inc;
        if p.xf >= 1.0 {
            p.d_a = p.d_b;
            p.fading = false;
            p.xf = 0.0;
        }
        y
    }

    // ------------------------------------------------------------------ pitch shifter
    /// Where should the next grain start so its waveform lines up with the one that's ending?
    fn ps_best_start(&self, l: &Shifter, old_delay: i32, first_candidate: i32, direction: i32) -> i32 {
        let mut best = first_candidate;
        let mut best_score = -1.0e9f32;

        for lag in 0..self.ps_lag_count {
            let cand = first_candidate + direction * lag * self.ps_lag_step;
            let mut corr = 0.0f32;
            let mut energy = 1.0e-9f32;
            for j in 0..128 {
                let a = daz(ps_at(l, old_delay - j * self.ps_seg_step));
                let b = daz(ps_at(l, cand - j * self.ps_seg_step));
                corr = daz(corr + daz(a * b));
                energy = daz(energy + daz(b * b));
            }
            let score = daz(corr / energy.sqrt());
            if score > best_score {
                best_score = score;
                best = cand;
            }
        }
        best
    }

    fn pitch_shift(&mut self, c: usize, x: f32) -> f32 {
        let pitch_ratio = self.pitch_ratio;
        let ps_xf_len = self.ps_xf_len;
        let ps_window = self.ps_window;
        let ps_min_d = self.ps_min_d;
        let ps_seg_step = self.ps_seg_step;
        let l = &mut self.sh[c];
        wr(&mut l.ps, l.ps_pos, x);

        let rate = 1.0 - pitch_ratio;
        let span = (128 * ps_seg_step) as f32;

        let mut d_act = l.ps_d0;
        if l.ps_active == 1 {
            d_act = l.ps_d1;
        }

        if !l.ps_fading {
            let mut splice = false;
            let (mut first, mut dir) = (0i32, 1i32);

            if pitch_ratio > 1.0 {
                if d_act <= ps_xf_len * (pitch_ratio - 1.0) + span + 8.0 {
                    splice = true;
                    first = ps_window as i32;
                    dir = -1;
                }
            } else if pitch_ratio < 1.0 && d_act >= ps_window {
                splice = true;
                first = ps_min_d as i32;
                dir = 1;
            }

            if splice {
                let start = self.ps_best_start(&self.sh[c], d_act as i32, first, dir) as f32;
                let l = &mut self.sh[c];
                if l.ps_active == 0 {
                    l.ps_d1 = start;
                    l.ps_active = 1;
                } else {
                    l.ps_d0 = start;
                    l.ps_active = 0;
                }
                l.ps_xf = 0.0;
                l.ps_fading = true;
            }
        }

        let l = &mut self.sh[c];
        let y;
        if l.ps_fading {
            let (mut cur, mut old) = (l.ps_d0, l.ps_d1);
            if l.ps_active == 1 {
                cur = l.ps_d1;
                old = l.ps_d0;
            }
            y = ps_read(l, cur) * l.ps_xf + ps_read(l, old) * (1.0 - l.ps_xf);
            l.ps_xf += 1.0 / ps_xf_len;
            if l.ps_xf >= 1.0 {
                l.ps_xf = 1.0;
                l.ps_fading = false;
            }
            l.ps_d0 += rate;
            l.ps_d1 += rate;
        } else if l.ps_active == 0 {
            y = ps_read(l, l.ps_d0);
            l.ps_d0 += rate;
        } else {
            y = ps_read(l, l.ps_d1);
            l.ps_d1 += rate;
        }

        l.ps_pos += 1;
        if l.ps_pos >= PSBUF {
            l.ps_pos = 0;
        }
        y
    }

    fn apply_pitch(&mut self) {
        if self.interval_idx == 1 {
            self.pitch_ratio = 1.49831;
        } else if self.interval_idx == 2 {
            self.pitch_ratio = 0.5;
        } else {
            self.pitch_ratio = 2.0;
        }

        self.update_pitch_guard();
    }

    /// Shifting UP reads the line faster than it was written, so anything above fs / (2 * ratio)
    /// folds back down as aliasing; the guard is 4th order when shifting up (see the C++).
    fn update_pitch_guard(&mut self) {
        self.ps_guard_on = self.pitch_ratio > 1.0;

        let sr = self.sr;
        let voicing = min_d(7000.0, 0.2 * sr);
        let fc = if self.ps_guard_on { min_d(voicing, min_d(0.92 * sr / (2.0 * f64::from(self.pitch_ratio)), 0.45 * sr)) } else { voicing };

        for c in 0..2 {
            if self.ps_guard_on {
                set_lp(&mut self.sh[c].pre, fc, sr, 0.5412);
                set_lp(&mut self.sh[c].pre2, fc, sr, 1.3066);
            } else {
                set_lp(&mut self.sh[c].pre, fc, sr, 0.7071);
            }
        }

        // Size the grain window from the rate (|1 - ratio|), not a fixed length.
        let rate = (1.0 - self.pitch_ratio).abs();
        let max_window = PSBUF as f32 - 0.03 * self.fs - 64.0;

        if self.ps_guard_on {
            let margin = self.ps_xf_len * (self.pitch_ratio - 1.0) + (128 * self.ps_seg_step) as f32 + 8.0;
            self.ps_window = clamp_t(margin + K_GRAIN_SECONDS * self.fs * rate, 0.10 * self.fs, max_window);
        } else {
            self.ps_window = min_t(0.10 * self.fs, max_window);
        }
    }

    fn ps_pre_filter(&mut self, c: usize, x: f32) -> f32 {
        let guard = self.ps_guard_on;
        let l = &mut self.sh[c];
        let y = filter(&mut l.pre, x);
        if guard { filter(&mut l.pre2, y) } else { y }
    }

    // ------------------------------------------------------------------ machine table
    /// 16 lines, 29 .. 100 ms before Size, no two sharing a common factor worth hearing.
    fn set_lines(&mut self) {
        self.line_ms = [29.3, 33.1, 37.9, 41.3, 45.7, 49.9, 54.1, 58.9, 63.7, 68.3, 72.1, 77.3, 82.9, 87.7, 93.1, 99.7];
    }

    fn set_in_diff(&mut self, a: f32, b: f32, c: f32, d: f32) {
        self.in_diff_ms = [a, b, c, d];
    }

    fn apply_machine(&mut self) {
        self.use_er = false;
        self.use_spring = false;
        self.use_shimmer = false;
        self.bloom_diff = false;
        self.use_plate_model = false;
        self.m_diff_min = 0.0;
        self.m_tail_trim = 1.0;
        self.m_bass_mult = 1.0;
        self.m_damp_max = 20000.0;
        self.m_decay_scale = 1.0;
        self.set_lines();

        if self.machine_idx == 0 {
            // Room: 7 .. 45 ms lines, reflections up front, highs die quickly
            self.use_er = true;
            self.m_size_lo = 0.22;
            self.m_size_hi = 0.62;
            self.m_hf_ratio = 0.30;
            self.m_lo_xover = 250.0;
            self.m_ap_frac = 0.31;
            self.m_ap_gain = 0.5;
            self.m_mod_max_ms = 0.5;
            self.m_tail_trim = 0.9;
            self.set_in_diff(1.9, 3.1, 4.7, 6.7);
        } else if self.machine_idx == 1 {
            // Hall
            self.use_er = true;
            self.m_size_lo = 0.55;
            self.m_size_hi = 1.5;
            self.m_hf_ratio = 0.35;
            self.m_lo_xover = 300.0;
            self.m_ap_frac = 0.27;
            self.m_ap_gain = 0.55;
            self.m_mod_max_ms = 1.2;
            self.set_in_diff(4.7, 7.3, 11.9, 17.1);
        } else if self.machine_idx == 2 {
            // Plate: EMT 140 (measured against published decay curves; see the C++ notes)
            self.use_plate_model = true;
            self.m_size_lo = 0.18;
            self.m_size_hi = 0.42;
            self.m_hf_ratio = 0.33;
            self.m_lo_xover = 350.0;
            self.m_bass_mult = 0.50;
            self.m_damp_max = 2500.0;
            self.m_decay_scale = 1.25;
            self.m_ap_frac = 0.37;
            self.m_ap_gain = 0.62;
            self.m_mod_max_ms = 0.3;
            self.m_diff_min = 0.7;
            self.m_tail_trim = 1.05;
            self.set_in_diff(1.7, 2.6, 3.9, 5.7);
        } else if self.machine_idx == 3 {
            // Spring: its own engine
            self.use_spring = true;
            self.m_size_lo = 0.6;
            self.m_size_hi = 1.4;
            self.m_mod_max_ms = 0.35;
        } else if self.machine_idx == 4 {
            // Cloud
            self.bloom_diff = true;
            self.m_size_lo = 0.9;
            self.m_size_hi = 1.65;
            self.m_hf_ratio = 0.4;
            self.m_lo_xover = 300.0;
            self.m_ap_frac = 0.36;
            self.m_ap_gain = 0.68;
            self.m_mod_max_ms = 3.0;
            self.m_diff_min = 0.6;
            self.m_tail_trim = 1.15;
            self.set_in_diff(5.9, 9.7, 14.3, 19.1);
        } else {
            // Shimmer
            self.use_shimmer = true;
            self.m_size_lo = 0.7;
            self.m_size_hi = 1.5;
            self.m_hf_ratio = 0.45;
            self.m_lo_xover = 300.0;
            self.m_ap_frac = 0.30;
            self.m_ap_gain = 0.6;
            self.m_mod_max_ms = 1.5;
            self.m_diff_min = 0.4;
            self.m_tail_trim = 1.2;
            self.set_in_diff(4.7, 7.3, 11.9, 17.1);
        }
    }

    /// Plate dispersion, one line, inside the feedback path: a chain of first-order allpasses.
    fn plate_disperse(a: f32, l: &mut Line, x: f32) -> f32 {
        let mut v = x;
        for s in l.pdisp.iter_mut() {
            let st = *s;
            let w = v - a * st;
            v = a * w + st;
            *s = w;
        }
        v
    }

    // ------------------------------------------------------------------ one spring: y = C (x + g * D (y))
    fn spring(&mut self, s: usize, x: f32, m: f32) -> f32 {
        let target = self.spr_target[s];
        slew_line(&mut self.sp[s], target);
        let d = self.read_line(&self.sp[s], self.sp[s].d_cur + m);

        // in the loop: damping, a DC block, and a sign flip at the far end
        let (damp_c, hp_c) = (self.spr_damp_c, self.spr_hp_c);
        let line = &mut self.sp[s];
        line.lp_hi += (d - line.lp_hi) * damp_c;
        let lp = line.lp_hi;
        let hp = lp - line.dc_x + hp_c * line.dc_y;
        line.dc_x = lp;
        line.dc_y = hp;

        let mut v = x - self.spr_gain[s] * hp + DN;

        // 100 stretched allpasses: group delay climbs towards the transition frequency, which is the chirp
        let base = s as i32 * SPR_M * SPR_K;
        let a = self.spr_a;
        for k in 0..SPR_M {
            let idx = base + k * SPR_K + self.spr_pos;
            let st = rd(&self.casc, idx);
            let w = v - a * st;
            v = a * w + st;
            wr(&mut self.casc, idx, w);
        }

        write_line(&mut self.sp[s], soft_limit(v));

        // listen half-way round the loop, so the first arrival is late like a real tank
        let half = ((0.5 * self.sp[s].d_cur) as i32) as f32;
        let out = self.read_line(&self.sp[s], half);
        filter(&mut self.spr_out_lp[s], out)
    }

    // ------------------------------------------------------------------ control rate (every 32 samples)
    fn update_control(&mut self) {
        let fs = self.fs;
        let sr = self.sr;
        let t60 = knob_to_decay_sec(self.decay_knob) * self.m_decay_scale;
        let sc = self.m_size_lo * (self.m_size_hi / self.m_size_lo).powf(clamp_t(self.size_knob, 0.0, 1.0));
        let bass_mult = knob_to_low_decay_x(self.bass_knob) * self.m_bass_mult;

        // ---- Color
        self.use_color = (self.color_idx < 2) && !self.use_spring;
        self.use_linear = self.use_color;
        self.use_quant = self.use_color && self.color_idx == 0;
        self.color_mod_scale = 1.0;
        self.wander_amt = 0.0;
        self.color_damp_max = 20000.0;
        if self.use_color {
            if self.color_idx == 0 {
                self.color_mod_scale = 1.5;
                self.wander_amt = 0.6;
                self.color_damp_max = 6000.0;
            } else {
                self.color_damp_max = 10000.0;
            }
        }
        // the converter filters fade out, change, and fade back in, so switching Color never clicks
        self.col_mix_t = 0.0;
        if self.use_color && self.coef_color == self.color_idx {
            self.col_mix_t = 1.0;
        }
        if self.use_color && self.coef_color != self.color_idx && self.col_mix < 0.01 {
            self.coef_color = self.color_idx;
            let (mut f_in, mut f_out) = (13000.0f64, 14000.0f64);
            if self.color_idx == 0 {
                f_in = 8000.0;
                f_out = 8000.0;
            }
            f_in = min_d(f_in, 0.45 * sr);
            f_out = min_d(f_out, 0.45 * sr);
            set_lp(&mut self.col_in[0], f_in, sr, 0.5412);
            set_lp(&mut self.col_in[1], f_in, sr, 1.3066);
            set_lp(&mut self.col_in[2], f_in, sr, 0.5412);
            set_lp(&mut self.col_in[3], f_in, sr, 1.3066);
            set_lp(&mut self.col_out[0], f_out, sr, 0.7071);
            set_lp(&mut self.col_out[1], f_out, sr, 0.7071);
        }

        let f_damp = min_t(min_t(knob_to_high_cut_hz(self.damp_knob) as f32, self.color_damp_max), self.m_damp_max);
        self.c_hi = 1.0 - one_pole_coef(f64::from(min_t(f_damp, 0.45 * fs)), sr);
        self.c_lo = 1.0 - one_pole_coef(f64::from(self.m_lo_xover), sr);

        let dk = self.m_diff_min + (1.0 - self.m_diff_min) * self.diff_knob;
        self.in_diff_g = 0.75 * dk;
        self.loop_ap_g = self.m_ap_gain * dk;
        if self.flush_count > 0 {
            self.in_diff_g = 0.0;
            self.loop_ap_g = 0.0;
        }

        let mut g_sum = 0.0f32;
        for i in 0..16 {
            let l = (clamp_t(self.line_ms[i] * sc * fs * 0.001, 16.0, (LBUF - 1200) as f32) + 0.5) as i32 as f32;
            self.line_target[i] = l;
            let a = clamp_t(l * (self.m_ap_frac + 0.006 * i as f32), 8.0, (ABUF - 8) as f32) as i32;
            self.ap_target[i] = a;

            let mut g = 10.0f32.powf(-3.0 * (l + a as f32 + self.plate_ap_delay) / (t60 * fs));
            let rl = g.powf(1.0 / bass_mult - 1.0);
            let rh = g.powf(1.0 / self.m_hf_ratio - 1.0);
            if self.flush_count > 0 {
                g = 0.0;
            }
            self.line_gain[i] = g;
            self.r_low[i] = rl;
            self.r_high[i] = rh;
            g_sum += g * g;
        }

        // input diffusers. Cloud's stretch with Bloom, so the sound takes longer to arrive.
        let mut d_scale = 1.0f32;
        if self.bloom_diff {
            d_scale = 1.0 + 3.5 * self.char_sm;
        }
        for i in 0..4 {
            let base = self.in_diff_ms[i] * d_scale * fs * 0.001;
            self.in_diff_target[i] = clamp_t(base, 4.0, (DBUF - 8) as f32) as i32;
            self.in_diff_target[i + 4] = clamp_t(base * 1.093, 4.0, (DBUF - 8) as f32) as i32;
        }

        // early reflections scale with Size
        let er_scale = 0.5 + 1.1 * self.size_knob;
        let mut er_room = 1.0f32;
        if self.machine_idx == 0 {
            er_room = 0.55;
        }
        for i in 0..6 {
            self.er_tap_l[i] = (self.er_ms_l[i] * er_scale * er_room * fs * 0.001) as i32;
            self.er_tap_r[i] = (self.er_ms_r[i] * er_scale * er_room * fs * 0.001) as i32;
        }

        // level: a long decay stores more energy. Take back half of that (in dB).
        let g_avg = g_sum * 0.0625;
        let mut tg = self.m_tail_trim * 1.15 * max_t(1.0 - g_avg, 0.0005).powf(0.33);
        let mut eg = 0.0f32;
        if self.use_er {
            eg = 0.6 * self.char_sm;
        }

        // modulation
        let rate = knob_to_mod_rate_hz(self.rate_knob);
        self.lfo_inc_a = rate / fs;
        self.lfo_inc_b = 0.731 * rate / fs;
        self.mod_samples = self.depth_knob * self.depth_knob * self.m_mod_max_ms * self.color_mod_scale * fs * 0.001 * (1.0 - self.freeze_sm);

        // spring
        if self.use_spring {
            let casc_dc = (SPR_M * self.spr_k) as f32 * (1.0 - self.spr_a) / (1.0 + self.spr_a);
            let mut gs = 0.0f32;
            for s in 0..3 {
                let td = self.spr_ms[s] * sc * fs * 0.001;
                self.spr_target[s] = clamp_t(td - casc_dc, 64.0, (LBUF - 1200) as f32) as i32 as f32;
                let mut g = 10.0f32.powf(-3.0 * td / (t60 * fs));
                if self.flush_count > 0 {
                    g = 0.0;
                }
                self.spr_gain[s] = g;
                gs += g * g;
            }
            self.spr_damp_c = 1.0 - one_pole_coef(f64::from(min_t(f_damp, 0.45 * fs)), sr);
            let drive = 0.7 + 9.0 * self.char_sm * self.char_sm;
            self.drive_sm = drive;
            self.spr_makeup = 1.0 / drive.powf(0.55); // more Dwell = a bit more reverb, like the real knob
            tg = 0.4 * max_t(1.0 - gs * 0.3333, 0.0005).powf(0.25);
            eg = 0.0;
        }

        self.tail_gain_target = tg;
        self.er_gain_target = eg;
        self.tail_gain_sm += (tg - self.tail_gain_sm) * 0.05;
        self.er_gain_sm += (eg - self.er_gain_sm) * 0.05;

        // Zing: how hard the plate disperses.
        self.plate_ap_a = -(0.35 + 0.40 * self.char_sm);

        // The in-loop allpasses lengthen the loop, so fold their delay into the gain that sets T60.
        self.plate_ap_delay = if self.use_plate_model {
            K_PLATE_AP as f32 * (1.0 - self.plate_ap_a * self.plate_ap_a) / (1.0 + self.plate_ap_a * self.plate_ap_a)
        } else {
            0.0
        };

        // shimmer amount (equal-power against the unshifted signal on its two lines)
        let mut s_t = 0.0f32;
        if self.use_shimmer {
            s_t = 0.92 * self.char_sm * (1.0 - self.freeze_sm);
        }
        self.shim_target = s_t;
        self.shim_sm += (s_t - self.shim_sm) * 0.05;

        // attack: tap times along the ramp
        self.attack_sm += (self.attack_knob - self.attack_sm) * 0.2;
        self.attack_mix = min_t(1.0, self.attack_sm * 5.0);
        for a in 0..8 {
            self.atk_target[a] = (self.atk_frac[a] * self.attack_sm * 0.25 * fs) as i32;
        }

        // pre-delay
        let mut bpm = knob_to_bpm(self.tempo_knob);
        if self.tempo_src_idx == 0 && self.host_bpm >= 20.0 {
            bpm = self.host_bpm;
        }
        let mut pre_sec = 0.001 * knob_to_pre_ms(self.pre_knob);
        if self.synced {
            pre_sec = division_beats(self.div_idx) * 60.0 / bpm;
        }
        self.pre_target = clamp_t(pre_sec * fs, 0.0, PBUF as f32 - 0.42 * fs - 16.0); // room for reflections and the attack ramp

        // swell
        self.swell_rise = 0.0;
        if self.swell_knob > 0.01 {
            let sec = knob_to_swell_sec(self.swell_knob); // 50 ms .. 2 s
            self.swell_rise = 1.0 - (-1.0 / (f64::from(sec) * 0.4 * sr)).exp() as f32;
        }

        // output EQ
        if self.lc_knob != self.last_lc {
            self.last_lc = self.lc_knob;
            let f = knob_to_low_cut_hz(self.lc_knob);
            set_hp(&mut self.out_lc[0], f, sr, 0.7071);
            set_hp(&mut self.out_lc[1], f, sr, 0.7071);
        }
        if self.hc_knob != self.last_hc {
            self.last_hc = self.hc_knob;
            let f = min_d(knob_to_high_cut_hz(self.hc_knob), 0.45 * sr);
            set_lp(&mut self.out_hc[0], f, sr, 0.7071);
            set_lp(&mut self.out_hc[1], f, sr, 0.7071);
        }
    }

    // ------------------------------------------------------------------ Rust additions: read-only views
    pub fn sample_rate(&self) -> f64 {
        self.sr
    }
    /// The pre-delay the engine is heading for, in samples (`preTarget`; the lines read it rounded).
    pub fn pre_target_samples(&self) -> f32 {
        self.pre_target
    }
    /// The tempo the pre-delay sync uses right now: the host's unless Manual, else the Tempo knob
    /// (the same choice as `updateControl`).
    pub fn effective_bpm(&self) -> f32 {
        if self.tempo_src_idx == 0 && self.host_bpm >= 20.0 { self.host_bpm } else { knob_to_bpm(self.tempo_knob) }
    }
    /// The machine running now (it changes at the flush's swap point, not at the parameter).
    pub fn machine(&self) -> i32 {
        self.machine_idx
    }
    /// The smoothed freeze amount (0 running .. 1 frozen).
    pub fn freeze_amount(&self) -> f32 {
        self.freeze_sm
    }
    /// The machine's T60 scale, its low-band multiplier and its high-band ratio, as `applyMachine`
    /// set them (tests compare these with [`super::reverb_config::machine_decay`]).
    pub fn machine_decay_traits(&self) -> (f32, f32, f32, bool) {
        (self.m_decay_scale, self.m_bass_mult, self.m_hf_ratio, self.use_spring)
    }
    /// The output EQ's and colour converters' coefficients (tests: which designs are in force).
    #[cfg(test)]
    pub(crate) fn filter_designs(&self) -> Vec<[f32; 5]> {
        self.out_lc.iter().chain(&self.out_hc).chain(&self.col_in).chain(&self.col_out).map(|q| [q.b0, q.b1, q.b2, q.a1, q.a2]).collect()
    }

    /// Every buffer's address and capacity (tests: nothing is re-made).
    #[cfg(test)]
    pub(crate) fn buffer_ptrs(&self) -> Vec<(*const f32, usize)> {
        let mut v: Vec<(*const f32, usize)> = Vec::new();
        for l in self.ln.iter().chain(self.sp.iter()) {
            v.push((l.buf.as_ptr(), l.buf.capacity()));
        }
        for a in self.lap.iter().chain(self.idf.iter()) {
            v.push((a.b.as_ptr(), a.b.capacity()));
        }
        for p in &self.pd {
            v.push((p.buf.as_ptr(), p.buf.capacity()));
        }
        for s in &self.sh {
            v.push((s.ps.as_ptr(), s.ps.capacity()));
        }
        v.push((self.casc.as_ptr(), self.casc.capacity()));
        v
    }
    /// The in-loop damping corner limits: the colour's and the machine's (Hz).
    pub fn damp_limits(&self) -> (f32, f32) {
        (self.color_damp_max, self.m_damp_max)
    }
}
