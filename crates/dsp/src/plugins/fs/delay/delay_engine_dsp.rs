//! A faithful translation of `Engine 6 - Delay/Source/DelayEngineDSP.h`: one delay core, six
//! voicings (Digital, Rack, BBD, Tape, Drum, Oil Can), four routings, host or manual tempo sync,
//! tap tempo, freeze, reverse, a pitch shifter inside the loop, multi-head and pattern taps.
//!
//! Same state, same order of operations, same constants. Where Rust differs from the C++:
//! - `std::max(a, b)` / `std::min(a, b)` / `clampT` are [`cmax`] / [`cmin`] / [`clamp_t`] (the
//!   C++ comparison forms, not `f32::max`, which treats NaN differently).
//! - Float -> int casts (`int (x)`) are Rust `as i32`: the same for every finite in-range value;
//!   where the C++ is undefined (NaN, out of range) Rust saturates, so an index never leaves its
//!   buffer. Index arithmetic that the C++ does in plain `int` uses wrapping ops, so nothing can
//!   panic on overflow (the C++ would be undefined there).
//! - The UI atomics (`gateOut`, `tapBpmOut`, `tapSeq`) are plain fields read by `readouts`.
//! - The buffers are `Vec`s allocated once in [`DelayEngineDsp::new`] (the C++ re-`assign`s them in
//!   `prepare`; here `prepare` zeroes them in place: same contents, no allocation).

// The C++ literals (0.7071, 3.14159...) are kept as written.
#![allow(clippy::approx_constant)]
#![allow(clippy::excessive_precision)]

// ------------------------------------------------------------------ parameters (param1..param35)
pub const P_TIME: usize = 0;
pub const P_FEEDBACK: usize = 1;
pub const P_MIX: usize = 2;
pub const P_STYLE: usize = 3;
pub const P_ROUTING: usize = 4;
pub const P_BYPASS: usize = 5;
pub const P_TRIM: usize = 6;
pub const P_LEVEL: usize = 7;
pub const P_SYNC: usize = 8;
pub const P_DIVISION: usize = 9;
pub const P_LOW_CUT: usize = 10;
pub const P_HIGH_CUT: usize = 11;
pub const P_CHARACTER: usize = 12;
pub const P_MOD_RATE: usize = 13;
pub const P_MOD_DEPTH: usize = 14;
pub const P_OFFSET: usize = 15;
pub const P_HEADS: usize = 16;
pub const P_DIFFUSION: usize = 17;
pub const P_DUCK: usize = 18;
pub const P_SWING: usize = 19;
pub const P_PITCH: usize = 20;
pub const P_FREEZE: usize = 21;
pub const P_WIDTH: usize = 22;
pub const P_TRAILS: usize = 23;
pub const P_TEMPO: usize = 24;
pub const P_TEMPO_SRC: usize = 25;
pub const P_THRESHOLD: usize = 26;
pub const P_FOCUS: usize = 27;
pub const P_TAP: usize = 28;
pub const P_REVERSE: usize = 29;
pub const P_NOISE: usize = 30;
pub const P_FILT_MOD: usize = 31;
pub const P_PATTERN: usize = 32;
pub const P_MOD_SYNC: usize = 33;
pub const P_DIVISION_R: usize = 34;
pub const NUM_PARAMS: usize = 35;

// ------------------------------------------------------------------ constants
/// Per line: 10.9 s at 48k, 5.4 s at 96k (longer settings clamp).
pub const BUF: i32 = 524_288;
pub const APBUF: i32 = 4096;
pub const PSBUF: i32 = 32_768;
/// Compander reference level.
const COMP_REF: f32 = 0.10;
/// Diffusion allpass gain.
const AP_G: f32 = 0.62;
/// Target length of one pitch-shifter grain, whatever the interval.
const K_GRAIN_SECONDS: f32 = 0.10;

const PI: f64 = 3.14159265358979323846;
const TWO_PI: f64 = 6.28318530717958647692;

// ------------------------------------------------------------------ C++ comparison helpers
/// `std::max (a, b)`: `(a < b) ? b : a`.
#[inline]
pub fn cmax(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

/// `std::min (a, b)`: `(b < a) ? b : a`.
#[inline]
pub fn cmin(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}

/// `clampT`.
#[inline]
pub fn clamp_t(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

// ------------------------------------------------------------------ knob -> unit maps
// The UI prints these same numbers; they live here, and only here (as in the C++).

/// 20 ms .. 2 s.
pub fn knob_to_ms(v: f32) -> f32 {
    20.0 * 100.0f32.powf(v)
}

/// 20 Hz .. 1 kHz.
pub fn knob_to_low_cut_hz(v: f32) -> f64 {
    20.0 * 50.0f64.powf(f64::from(v))
}

/// 1 k .. 20 k.
pub fn knob_to_high_cut_hz(v: f32) -> f64 {
    1000.0 * 20.0f64.powf(f64::from(v))
}

/// 0.05 .. 8 Hz.
pub fn knob_to_mod_rate_hz(v: f32) -> f32 {
    0.05 * 160.0f32.powf(v)
}

/// 40 .. 240 BPM.
pub fn knob_to_bpm(v: f32) -> f32 {
    40.0 + 200.0 * v
}

pub fn knob_to_trim_db(v: f32) -> f32 {
    -12.0 + 36.0 * clamp_t(v, 0.0, 1.0)
}

/// 0.7071 = unity.
pub fn knob_to_level_gain(v: f32) -> f32 {
    2.0 * v * v
}

/// 0 = off.
pub fn knob_to_threshold_db(v: f32) -> f32 {
    -60.0 + 54.0 * v
}

/// Note values, short -> long. The UI prints these; the sync maths uses them.
pub fn division_beats(i: i32) -> f32 {
    match i {
        0 => 0.1666667,  // 1/16T
        1 => 0.25,       // 1/16
        2 => 0.3333333,  // 1/8T
        3 => 0.375,      // 1/16.
        4 => 0.5,        // 1/8
        5 => 0.6666667,  // 1/4T
        6 => 0.75,       // 1/8.
        7 => 1.0,        // 1/4
        8 => 1.3333333,  // 1/2T
        9 => 1.5,        // 1/4.
        10 => 2.0,       // 1/2
        11 => 2.6666667, // 1/1T
        12 => 3.0,       // 1/2.
        13 => 4.0,       // 1/1
        14 => 6.0,       // 1/1.
        _ => 8.0,        // 2/1
    }
}

/// The playback heads `applyHeads` switches on: the wet gain of each head, the feedback
/// normalisation and the longest head's multiple of the delay time. A pure function of the style's
/// flags and the Heads choice, so the UI's tap display reads the very table the engine uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadTable {
    pub gain: [f32; 4],
    pub fb_norm: f32,
    pub max_ratio: f32,
}

/// `applyHeads`: 1 | 2 | 3 | 4 | 1+2 | 1+3 | 2+4 | All. Tape has three heads, so its "4" is head 3.
pub fn head_table(multi_head: bool, style_idx: i32, heads_idx: i32) -> HeadTable {
    let mut gain = [0.0f32; 4];
    if !multi_head {
        gain[0] = 1.0;
        return HeadTable { gain, fb_norm: 1.0, max_ratio: 1.0 };
    }
    let (mut h0, mut h1, mut h2, mut h3) = (false, false, false, false);
    match heads_idx {
        0 => h0 = true,
        1 => h1 = true,
        2 => h2 = true,
        3 => h3 = true,
        4 => {
            h0 = true;
            h1 = true;
        }
        5 => {
            h0 = true;
            h2 = true;
        }
        6 => {
            h1 = true;
            h3 = true;
        }
        _ => {
            h0 = true;
            h1 = true;
            h2 = true;
            h3 = true;
        }
    }
    if style_idx == 3 && h3 {
        // tape: no head 4
        h3 = false;
        h2 = true;
    }
    let mut n = 0.0f32;
    for h in [h0, h1, h2, h3] {
        if h {
            n += 1.0;
        }
    }
    if n < 1.0 {
        h0 = true;
        n = 1.0;
    }
    // wet: equal power. feedback gets another 1/sqrt(n) so the loop gain stays 1/n per head
    let g = 1.0 / n.sqrt();
    if h0 {
        gain[0] = g;
    }
    if h1 {
        gain[1] = g;
    }
    if h2 {
        gain[2] = g;
    }
    if h3 {
        gain[3] = g;
    }
    // the time limit only has to make room for the longest head that's actually switched on
    let mut max_ratio = 1.0;
    if h1 {
        max_ratio = 2.0;
    }
    if h2 {
        max_ratio = 3.0;
    }
    if h3 {
        max_ratio = 4.0;
    }
    HeadTable { gain, fb_norm: g, max_ratio }
}

/// Whether a style runs several playback heads (`applyStyle`: Tape and Drum).
pub fn style_is_multi_head(style_idx: i32) -> bool {
    style_idx == 3 || style_idx == 4
}

/// `applyPattern`: extra output-only taps at fractions of the delay time, `(count, fractions,
/// gains)`. Only the full-time tap feeds back, so the figure repeats cleanly.
pub fn pattern_table(pattern_idx: i32) -> (i32, [f32; 4], [f32; 4]) {
    let mut f = [0.0f32; 4];
    let mut g = [0.0f32; 4];
    let n = match pattern_idx {
        1 => {
            // Gallop: x . . . x . x .  -> taps at 1/2, 3/4
            f[0] = 0.5;
            g[0] = 0.75;
            f[1] = 0.75;
            g[1] = 0.85;
            2
        }
        2 => {
            // Tresillo 3+3+2
            f[0] = 0.375;
            g[0] = 0.8;
            f[1] = 0.75;
            g[1] = 0.8;
            2
        }
        3 => {
            // Triplet
            f[0] = 0.3333333;
            g[0] = 0.8;
            f[1] = 0.6666667;
            g[1] = 0.8;
            2
        }
        4 => {
            // Son clave over 16 steps: 0 3 6 10 12
            f[0] = 0.1875;
            g[0] = 0.8;
            f[1] = 0.375;
            g[1] = 0.75;
            f[2] = 0.625;
            g[2] = 0.8;
            f[3] = 0.75;
            g[3] = 0.75;
            4
        }
        5 => {
            // Cascade: four even taps, getting louder into the downbeat
            f[0] = 0.25;
            g[0] = 0.45;
            f[1] = 0.5;
            g[1] = 0.6;
            f[2] = 0.75;
            g[2] = 0.8;
            3
        }
        _ => 0,
    };
    (n, f, g)
}

/// The diffusion allpasses' longest delays, ms (`prepare`'s `apMaxMs`).
const AP_MAX_MS: [f32; 4] = [3.1, 5.3, 8.9, 13.7];

/// What `updateControl` derives for the loop's own delay: the allpass lengths, the shifter's
/// geometry, and `loopComp` (their delay, taken off the line so a repeat still comes back on time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopGeometry {
    pub ap_delay: [i32; 4],
    pub ps_xf_len: f32,
    pub ps_lag_step: i32,
    pub ps_seg_step: i32,
    pub ps_lag_count: i32,
    pub ps_min_d: f32,
    pub ps_window: f32,
    /// Samples.
    pub loop_comp: f32,
}

/// `updateControl`'s diffusion and pitch-shifter block, verbatim: `diff` is Diffusion plus the
/// style's own (`diffKnob + styleDiff`, clamped here as there).
pub fn loop_geometry(fs: f32, diff: f32, pitch_ratio: f32, pitch_idx: i32) -> LoopGeometry {
    // ---- diffusion: allpass lengths scale with the knob, and their total comes off the delay time
    let d = clamp_t(diff, 0.0, 1.0);
    let mut ap_delay = [0i32; 4];
    let mut ap_total = 0.0f32;
    for i in 0..4 {
        let mut dd = (d * AP_MAX_MS[i] * 0.001 * fs) as i32;
        if dd > APBUF - 2 {
            dd = APBUF - 2;
        }
        ap_delay[i] = dd;
        ap_total += dd as f32;
    }

    // pitch shifter geometry, scaled so the matcher always covers ~12 ms and a 128-point window
    let ps_xf_len = 0.02 * fs;
    let mut m = (fs / 48_000.0 + 0.5) as i32;
    if m < 1 {
        m = 1;
    }
    let ps_lag_step = 2 * m;
    let ps_seg_step = 4 * m;
    let ps_lag_count = 300;
    let ps_min_d = (128 * ps_seg_step) as f32 + 16.0;

    // Size the window from the rate, so a grain lasts about the same time whatever the interval.
    let rate = (1.0 - pitch_ratio).abs();
    let max_window = PSBUF as f32 - 0.03 * fs - 64.0;

    let ps_window = if pitch_ratio > 1.0 {
        let margin = ps_xf_len * (pitch_ratio - 1.0) + (128 * ps_seg_step) as f32 + 8.0;
        clamp_t(margin + K_GRAIN_SECONDS * fs * rate, 0.10 * fs, max_window)
    } else {
        cmin(0.10 * fs, max_window)
    };
    let mut loop_comp = ap_total;
    if pitch_idx > 0 {
        loop_comp += 0.5 * ps_window;
    }
    LoopGeometry { ap_delay, ps_xf_len, ps_lag_step, ps_seg_step, ps_lag_count, ps_min_d, ps_window, loop_comp }
}

/// Oil Can's own diffusion (`updateControl`'s `styleDiff`), from the smoothed Character; 0 for
/// every other style.
pub fn style_diff(style_idx: i32, ch: f32) -> f32 {
    if (0..=4).contains(&style_idx) { 0.0 } else { 0.25 + 0.35 * ch }
}

/// `applyPitch`: the shifter's ratio per Pitch choice.
pub fn pitch_ratio_for(pitch_idx: i32) -> f32 {
    match pitch_idx {
        1 => 0.5,
        2 => 1.33484,
        3 => 1.49831,
        4 => 2.0,
        _ => 1.0,
    }
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

#[derive(Debug, Clone, PartialEq)]
struct Allpass {
    b: Vec<f32>,
    pos: i32,
}

impl Allpass {
    fn new() -> Self {
        Allpass { b: vec![0.0; APBUF as usize], pos: 0 }
    }

    fn clear(&mut self) {
        self.b.fill(0.0);
        self.pos = 0;
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Line {
    buf: Vec<f32>,
    write_pos: i32,
    /// glide styles: the slewed delay, in samples
    d_cur: f32,
    /// crossfade styles
    d_a: f32,
    d_b: f32,
    xf: f32,
    fading: bool,
    /// user filters
    lc: Biquad,
    hc: Biquad,
    /// BBD anti-alias (2nd order)
    aa: Biquad,
    /// BBD reconstruction (4th order)
    recon: [Biquad; 2],
    /// style one-poles
    lp_z: f32,
    hp_x1: f32,
    hp_y1: f32,
    /// tape head bump
    bump: Biquad,
    /// pattern taps get their own copy of High Cut
    hc_x: Biquad,
    lp_zx: f32,
    /// clocked sample-and-hold
    hold_phase: f32,
    held: f32,
    loss_z: f32,
    /// reverse grain position
    rev_c: f32,
    rng: i64,
    /// compander
    env_in: f32,
    env_out: f32,
    /// ADAA state
    sat_x1: f64,
    sat_f1: f64,
    ap: [Allpass; 4],
    /// pitch shifter
    ps: Vec<f32>,
    /// anti-alias pair, used only when shifting up
    ps_guard: [Biquad; 2],
    ps_pos: i32,
    /// the two read heads' delays (samples)
    ps_d0: f32,
    ps_d1: f32,
    /// which head is the current one
    ps_active: i32,
    /// 0..1 crossfade into the current head
    ps_xf: f32,
    ps_fading: bool,
    /// what goes back round the loop (the full-time tap only)
    fb_sample: f32,
}

impl Line {
    fn new() -> Self {
        Line {
            buf: vec![0.0; BUF as usize],
            write_pos: 0,
            d_cur: 0.0,
            d_a: 0.0,
            d_b: 0.0,
            xf: 0.0,
            fading: false,
            lc: Biquad::default(),
            hc: Biquad::default(),
            aa: Biquad::default(),
            recon: [Biquad::default(); 2],
            lp_z: 0.0,
            hp_x1: 0.0,
            hp_y1: 0.0,
            bump: Biquad::default(),
            hc_x: Biquad::default(),
            lp_zx: 0.0,
            hold_phase: 0.0,
            held: 0.0,
            loss_z: 0.0,
            rev_c: 0.0,
            rng: 0,
            env_in: 0.0,
            env_out: 0.0,
            sat_x1: 0.0,
            sat_f1: 0.0,
            ap: [Allpass::new(), Allpass::new(), Allpass::new(), Allpass::new()],
            ps: vec![0.0; PSBUF as usize],
            ps_guard: [Biquad::default(); 2],
            ps_pos: 0,
            ps_d0: 0.0,
            ps_d1: 0.0,
            ps_active: 0,
            ps_xf: 0.0,
            ps_fading: false,
            fb_sample: 0.0,
        }
    }

    /// A default-constructed `Line`, in place (the buffers are zeroed, never re-made).
    fn restore_initial(&mut self) {
        self.buf.fill(0.0);
        self.write_pos = 0;
        self.d_cur = 0.0;
        self.d_a = 0.0;
        self.d_b = 0.0;
        self.xf = 0.0;
        self.fading = false;
        self.lc = Biquad::default();
        self.hc = Biquad::default();
        self.aa = Biquad::default();
        self.recon = [Biquad::default(); 2];
        self.lp_z = 0.0;
        self.hp_x1 = 0.0;
        self.hp_y1 = 0.0;
        self.bump = Biquad::default();
        self.hc_x = Biquad::default();
        self.lp_zx = 0.0;
        self.hold_phase = 0.0;
        self.held = 0.0;
        self.loss_z = 0.0;
        self.rev_c = 0.0;
        self.rng = 0;
        self.env_in = 0.0;
        self.env_out = 0.0;
        self.sat_x1 = 0.0;
        self.sat_f1 = 0.0;
        for a in &mut self.ap {
            a.clear();
        }
        self.ps.fill(0.0);
        self.ps_guard = [Biquad::default(); 2];
        self.ps_pos = 0;
        self.ps_d0 = 0.0;
        self.ps_d1 = 0.0;
        self.ps_active = 0;
        self.ps_xf = 0.0;
        self.ps_fading = false;
        self.fb_sample = 0.0;
    }
}

/// Everything but the two lines: parameter state, smoothers and control-rate values.
#[derive(Debug, Clone, PartialEq)]
struct State {
    // ---- outputs back to the UI (the C++ atomics)
    gate_out: f32,
    tap_bpm_out: f32,
    tap_seq: i32,

    // ---- parameter state
    time_knob: f32,
    fb_knob: f32,
    mix_knob: f32,
    style_idx: i32,
    route_idx: i32,
    bypassed: bool,
    trim_db: f32,
    trim_lin: f32,
    trim_sm: f32,
    level_knob: f32,
    synced: bool,
    div_idx: i32,
    lc_knob: f32,
    hc_knob: f32,
    char_knob: f32,
    rate_knob: f32,
    depth_knob: f32,
    offset_knob: f32,
    heads_idx: i32,
    diff_knob: f32,
    duck_knob: f32,
    swing_knob: f32,
    pitch_idx: i32,
    frozen: bool,
    width_knob: f32,
    trails_on: bool,
    tempo_knob: f32,
    tempo_src_idx: i32,
    /// 0 until the host reports a tempo
    host_bpm: f32,
    thr_knob: f32,
    focus_knob: f32,
    reverse_on: bool,
    mod_sync_on: bool,
    tap_down: bool,
    tap_active: bool,
    tap_force_manual: bool,
    tap_pending: bool,
    noise_knob: f32,
    filt_mod_knob: f32,
    tap_bpm: f32,
    tap_interval: f32,
    pattern_idx: i32,
    div_r_idx: i32,
    sample_clock: i64,
    last_tap_clock: i64,
    host_playing: bool,

    // ---- smoothed
    fb_sm: f32,
    dry_g_sm: f32,
    wet_g_sm: f32,
    level_sm: f32,
    width_sm: f32,
    duck_sm: f32,
    char_sm: f32,
    gate_sm: f32,
    byp_sm: f32,
    freeze_sm: f32,

    // ---- derived, control rate
    fs: f32,
    ctrl_counter: i32,
    target_l: f32,
    target_r: f32,
    lfo_inc: f32,
    lfo_phase: f32,
    mod_samples: f32,
    wow_inc: f32,
    wow_phase: f32,
    flut_inc: f32,
    flut_phase: f32,
    wow_amt: f32,
    flut_amt: f32,
    drift_amt: f32,
    drift: f32,
    drift_target: f32,
    seed: i64,
    last_lc: f32,
    last_hc: f32,
    style_lp_coef: f32,
    style_hp_coef: f32,
    sat_drive: f32,
    sat_bias: f32,
    sat_makeup: f32,
    bbd_clip: f32,
    crush_levels: f32,
    style_diff: f32,
    pitch_ratio: f32,
    ps_window: f32,
    /// see `update_pitch_guard`
    ps_guard_on: bool,
    ps_xf_len: f32,
    ps_min_d: f32,
    ps_lag_step: i32,
    ps_seg_step: i32,
    ps_lag_count: i32,
    env_coef: f32,
    duck_atk: f32,
    duck_rel: f32,
    duck_env: f32,
    glide_coef: f32,
    xf_inc: f32,

    // ---- dynamics: one detector drives Threshold and Attack Focus
    env_fast: f32,
    env_slow: f32,
    fast_atk: f32,
    fast_rel: f32,
    slow_atk: f32,
    slow_rel: f32,
    thr_lin: f32,
    focus_sm: f32,
    gate_open: bool,
    gate_hold: i32,
    gate_hold_len: i32,
    gate_gain: f32,
    gate_atk: f32,
    gate_rel: f32,
    win: f32,
    win_sm: f32,
    win_decay: f32,
    win_atk: f32,
    win_hold: i32,
    win_hold_len: i32,
    gate_send_counter: i32,

    // ---- per-style flags (set in apply_style)
    glide_style: bool,
    multi_head: bool,
    use_style_filt: bool,
    use_compander: bool,
    use_sat: bool,
    use_crush: bool,
    max_head_ratio: f32,
    head_gain: [f32; 4],
    head_fb_norm: f32,
    ap_delay: [i32; 4],
    loop_comp: f32,

    // ---- rhythm patterns
    pat_n: i32,
    pat_frac: [f32; 4],
    pat_gain: [f32; 4],

    // ---- component-derived behaviour
    /// sample-and-hold clock / sample rate
    hold_ratio: f32,
    use_hold: bool,
    use_bump: bool,
    /// charge-transfer loss, tracks the clock
    bbd_loss_coef: f32,
    noise_amt: f32,
    splice_env: f32,
    splice_depth: f32,
    splice_counter: i32,
    splice_period: i32,
    lfo_now: f32,

    // ---- Rust addition: the tempo `updateControl` settled on (the editor's `getEffectiveBpm`
    // reads the same choice), for `readouts` only.
    bpm_now: f32,
}

impl State {
    /// The member initialisers of `DelayEngineDSP`.
    fn initial() -> Self {
        State {
            gate_out: 0.0,
            tap_bpm_out: 120.0,
            tap_seq: 0,
            time_knob: 0.6215,
            fb_knob: 0.35,
            mix_knob: 0.3,
            style_idx: 0,
            route_idx: 1,
            bypassed: false,
            trim_db: 0.0,
            trim_lin: 1.0,
            trim_sm: 1.0,
            level_knob: 0.7071,
            synced: false,
            div_idx: 6,
            lc_knob: 0.3544,
            hc_knob: 0.8295,
            char_knob: 0.3,
            rate_knob: 0.4537,
            depth_knob: 0.0,
            offset_knob: 0.5,
            heads_idx: 0,
            diff_knob: 0.0,
            duck_knob: 0.0,
            swing_knob: 0.5,
            pitch_idx: 0,
            frozen: false,
            width_knob: 0.5,
            trails_on: true,
            tempo_knob: 0.4,
            tempo_src_idx: 0,
            host_bpm: 0.0,
            thr_knob: 0.0,
            focus_knob: 0.0,
            reverse_on: false,
            mod_sync_on: false,
            tap_down: false,
            tap_active: false,
            tap_force_manual: false,
            tap_pending: false,
            noise_knob: 0.0,
            filt_mod_knob: 0.0,
            tap_bpm: 120.0,
            tap_interval: 0.0,
            pattern_idx: 0,
            div_r_idx: 0,
            sample_clock: 0,
            last_tap_clock: -1_000_000,
            host_playing: false,
            fb_sm: 0.0,
            dry_g_sm: 1.0,
            wet_g_sm: 0.0,
            level_sm: 1.0,
            width_sm: 1.0,
            duck_sm: 0.0,
            char_sm: 0.3,
            gate_sm: 1.0,
            byp_sm: 0.0,
            freeze_sm: 0.0,
            fs: 48_000.0,
            ctrl_counter: 0,
            target_l: 16_800.0,
            target_r: 16_800.0,
            lfo_inc: 0.0,
            lfo_phase: 0.0,
            mod_samples: 0.0,
            wow_inc: 0.0,
            wow_phase: 0.0,
            flut_inc: 0.0,
            flut_phase: 0.0,
            wow_amt: 0.0,
            flut_amt: 0.0,
            drift_amt: 0.0,
            drift: 0.0,
            drift_target: 0.0,
            seed: 22222,
            last_lc: -1.0,
            last_hc: -1.0,
            style_lp_coef: 0.0,
            style_hp_coef: 0.0,
            sat_drive: 1.0,
            sat_bias: 0.0,
            sat_makeup: 1.0,
            bbd_clip: 0.6,
            crush_levels: 0.0,
            style_diff: 0.0,
            pitch_ratio: 1.0,
            ps_window: 4800.0,
            ps_guard_on: false,
            ps_xf_len: 960.0,
            ps_min_d: 600.0,
            ps_lag_step: 2,
            ps_seg_step: 4,
            ps_lag_count: 300,
            env_coef: 0.0,
            duck_atk: 0.0,
            duck_rel: 0.0,
            duck_env: 0.0,
            glide_coef: 0.0,
            xf_inc: 0.0,
            env_fast: 0.0,
            env_slow: 0.0,
            fast_atk: 0.0,
            fast_rel: 0.0,
            slow_atk: 0.0,
            slow_rel: 0.0,
            thr_lin: 0.0,
            focus_sm: 0.0,
            gate_open: false,
            gate_hold: 0,
            gate_hold_len: 7200,
            gate_gain: 0.0,
            gate_atk: 0.0,
            gate_rel: 0.0,
            win: 0.0,
            win_sm: 0.0,
            win_decay: 0.0,
            win_atk: 0.0,
            win_hold: 0,
            win_hold_len: 2400,
            gate_send_counter: 25,
            glide_style: false,
            multi_head: false,
            use_style_filt: false,
            use_compander: false,
            use_sat: false,
            use_crush: false,
            max_head_ratio: 1.0,
            head_gain: [0.0; 4],
            head_fb_norm: 1.0,
            ap_delay: [0; 4],
            loop_comp: 0.0,
            pat_n: 0,
            pat_frac: [0.0; 4],
            pat_gain: [0.0; 4],
            hold_ratio: 1.0,
            use_hold: false,
            use_bump: false,
            bbd_loss_coef: 0.0,
            noise_amt: 0.0,
            splice_env: 0.0,
            splice_depth: 0.0,
            splice_counter: 0,
            splice_period: 18_000,
            lfo_now: 0.0,
            bpm_now: 120.0,
        }
    }
}

// ------------------------------------------------------------------ helpers
#[inline]
fn filter(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

/// coefficients only -- keeps the filter's running state
fn set_lp(q: &mut Biquad, fc: f64, fsr: f64, qq: f64) {
    let k = (PI * fc / fsr).tan();
    let a0 = 1.0 + k / qq + k * k;
    q.b0 = ((k * k) / a0) as f32;
    q.b1 = ((2.0 * k * k) / a0) as f32;
    q.b2 = q.b0;
    q.a1 = ((2.0 * (k * k - 1.0)) / a0) as f32;
    q.a2 = ((1.0 - k / qq + k * k) / a0) as f32;
}

fn set_hp(q: &mut Biquad, fc: f64, fsr: f64, qq: f64) {
    let k = (PI * fc / fsr).tan();
    let a0 = 1.0 + k / qq + k * k;
    q.b0 = (1.0 / a0) as f32;
    q.b1 = (-2.0 / a0) as f32;
    q.b2 = q.b0;
    q.a1 = ((2.0 * (k * k - 1.0)) / a0) as f32;
    q.a2 = ((1.0 - k / qq + k * k) / a0) as f32;
}

fn set_peak(q: &mut Biquad, fc: f64, fsr: f64, qq: f64, gain_db: f64) {
    let a = 10.0f64.powf(gain_db / 40.0);
    let w = 2.0 * PI * fc / fsr;
    let al = w.sin() / (2.0 * qq);
    let a0 = 1.0 + al / a;
    q.b0 = ((1.0 + al * a) / a0) as f32;
    q.b1 = ((-2.0 * w.cos()) / a0) as f32;
    q.b2 = ((1.0 - al * a) / a0) as f32;
    q.a1 = q.b1;
    q.a2 = ((1.0 - al / a) / a0) as f32;
}

fn white_noise(l: &mut Line) -> f32 {
    // int64 LCG; rng stays in 0..2^31, so the product never overflows (as in the C++).
    l.rng = (l.rng.wrapping_mul(1_103_515_245).wrapping_add(12345)) % 2_147_483_647;
    l.rng as f32 / 1_073_741_824.0 - 1.0
}

fn one_pole_coef(fc: f64, fsr: f64) -> f32 {
    (-2.0 * PI * fc / fsr).exp() as f32
}

/// tanh with first-order antiderivative anti-aliasing. F(x) = ln cosh x.
fn sat_adaa(l: &mut Line, u: f32) -> f32 {
    let x1 = f64::from(u);
    let ax = x1.abs();
    let f1 = ax + (1.0 + (-2.0 * ax).exp()).ln() - 0.6931471805599453;
    let dx = x1 - l.sat_x1;
    let y = if dx.abs() < 1.0e-4 { (0.5 * (x1 + l.sat_x1)).tanh() } else { (f1 - l.sat_f1) / dx };
    l.sat_x1 = x1;
    l.sat_f1 = f1;
    y as f32
}

fn soft_limit(w: f32) -> f32 {
    let a = w.abs();
    if a <= 0.7 {
        return w;
    }
    // runaway feedback tops out at 0 dBFS inside the loop
    let o = 0.7 + 0.3 * ((a - 0.7) / 0.3).tanh();
    if w < 0.0 {
        return -o;
    }
    o
}

// ------------------------------------------------------------------ delay reads
fn read_delay(l: &Line, delay_samples: f32) -> f32 {
    let d = clamp_t(delay_samples, 2.0, (BUF - 8) as f32);
    let di = d as i32;
    let fr = d - di as f32;

    let mut i1 = l.write_pos - di;
    if i1 < 0 {
        i1 += BUF;
    }
    let mut i0 = i1 + 1;
    if i0 >= BUF {
        i0 -= BUF;
    }
    let mut i2 = i1 - 1;
    if i2 < 0 {
        i2 += BUF;
    }
    let mut i3 = i2 - 1;
    if i3 < 0 {
        i3 += BUF;
    }

    let y0 = l.buf[i0 as usize];
    let y1 = l.buf[i1 as usize];
    let y2 = l.buf[i2 as usize];
    let y3 = l.buf[i3 as usize];

    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * fr + c2) * fr + c1) * fr + c0
}

// The C++'s two `if`s, kept as written (same result as `clamp`, as BUF - 2 > 1).
#[allow(clippy::manual_clamp)]
fn read_raw(l: &Line, delay_samples: i32) -> f32 {
    let mut d = delay_samples;
    if d < 1 {
        d = 1;
    }
    if d > BUF - 2 {
        d = BUF - 2;
    }
    let mut i = l.write_pos - d;
    if i < 0 {
        i += BUF;
    }
    l.buf[i as usize]
}

// ------------------------------------------------------------------ diffusion + pitch
fn allpass(a: &mut Allpass, x: f32, d: i32) -> f32 {
    if d < 1 {
        return x;
    }
    let mut r = a.pos - d;
    if r < 0 {
        r += APBUF;
    }
    // `apDelay` is at most APBUF - 2, so r is in range; `rem_euclid` only guards the impossible.
    let v = a.b[r.rem_euclid(APBUF) as usize];
    let y = v - AP_G * x;
    a.b[a.pos as usize] = x + AP_G * y;
    a.pos += 1;
    if a.pos >= APBUF {
        a.pos = 0;
    }
    y
}

/// Hermite, the same interpolator `read_delay` uses.
fn ps_read(l: &Line, delay_samples: f32) -> f32 {
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

    let y0 = l.ps[i0 as usize];
    let y1 = l.ps[i1 as usize];
    let y2 = l.ps[i2 as usize];
    let y3 = l.ps[i3 as usize];

    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * fr + c2) * fr + c1) * fr + y1
}

fn ps_at(l: &Line, delay_samples: i32) -> f32 {
    // Cmajor's .at() wraps: fold properly (C++ `%` then one add, which is `rem_euclid`).
    let i = l.ps_pos.wrapping_sub(delay_samples).rem_euclid(PSBUF);
    l.ps[i as usize]
}

impl State {
    /// Where should the next grain start so its waveform lines up with the one that's ending?
    fn ps_best_start(&self, l: &Line, old_delay: i32, first_candidate: i32, direction: i32) -> i32 {
        let mut best = first_candidate;
        let mut best_score = -1.0e9f32;

        for lag in 0..self.ps_lag_count {
            let cand = first_candidate.wrapping_add(direction.wrapping_mul(lag).wrapping_mul(self.ps_lag_step));
            let mut corr = 0.0f32;
            let mut energy = 1.0e-9f32;
            for j in 0..128i32 {
                let a = ps_at(l, old_delay.wrapping_sub(j.wrapping_mul(self.ps_seg_step)));
                let b = ps_at(l, cand.wrapping_sub(j.wrapping_mul(self.ps_seg_step)));
                corr += a * b;
                energy += b * b;
            }
            let score = corr / energy.sqrt();
            if score > best_score {
                best_score = score;
                best = cand;
            }
        }
        best
    }

    /// Splice-aligned two-head pitch shifter.
    fn pitch_shift(&self, l: &mut Line, x: f32) -> f32 {
        l.ps[l.ps_pos as usize] = x;

        let rate = 1.0 - self.pitch_ratio; // how fast a head's delay changes, samples per sample
        let span = (128 * self.ps_seg_step) as f32; // room the matcher needs in front of a head

        let mut d_act = l.ps_d0;
        if l.ps_active == 1 {
            d_act = l.ps_d1;
        }

        if !l.ps_fading {
            let mut splice = false;
            let mut first = 0i32;
            let mut dir = 1i32;

            if self.pitch_ratio > 1.0 {
                // shifting up: delay shrinks. Jump back to the far end of the window before we hit zero.
                if d_act <= self.ps_xf_len * (self.pitch_ratio - 1.0) + span + 8.0 {
                    splice = true;
                    first = self.ps_window as i32;
                    dir = -1;
                }
            } else if self.pitch_ratio < 1.0 {
                // shifting down: delay grows. Jump forward to the near end before we run off the buffer.
                if d_act >= self.ps_window {
                    splice = true;
                    first = self.ps_min_d as i32;
                    dir = 1;
                }
            }

            if splice {
                let start = self.ps_best_start(l, d_act as i32, first, dir) as f32;
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

        let y;
        if l.ps_fading {
            let (mut cur, mut old) = (l.ps_d0, l.ps_d1);
            if l.ps_active == 1 {
                cur = l.ps_d1;
                old = l.ps_d0;
            }
            // matched waveforms -> linear fade keeps level
            y = ps_read(l, cur) * l.ps_xf + ps_read(l, old) * (1.0 - l.ps_xf);
            l.ps_xf += 1.0 / self.ps_xf_len;
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

    /// All playback heads for one line. mod_rel = tape-speed style wobble (relative), mod_abs = LFO (samples).
    fn read_taps(&self, l: &mut Line, target: f32, mod_rel: f32, mod_abs: f32) -> f32 {
        let mut sum = 0.0f32;

        if self.reverse_on {
            // Reverse: two grains, half a delay apart, each reading backwards through the last <time> of audio.
            let t = cmax(target, 64.0);
            l.rev_c += 1.0;
            if l.rev_c >= t {
                l.rev_c -= t;
            }
            let c1 = l.rev_c;
            let mut c2 = c1 + 0.5 * t;
            if c2 >= t {
                c2 -= t;
            }
            let s1 = (PI as f32 * c1 / t).sin();
            let s2 = (PI as f32 * c2 / t).sin();
            sum = read_delay(l, 2.0 * c1 + 2.0) * s1 * s1 + read_delay(l, 2.0 * c2 + 2.0) * s2 * s2;
            l.d_cur = target;
            l.d_a = target;
            l.d_b = target;
            l.fading = false;
            return sum;
        }

        if self.glide_style {
            // the Time knob moves a motor / clock, so the delay slews and the repeats bend in pitch
            l.d_cur += (target - l.d_cur) * self.glide_coef;
            let base = l.d_cur * (1.0 + mod_rel) + mod_abs;

            if self.multi_head && self.pat_n == 0 {
                for h in 0..4 {
                    if self.head_gain[h] > 0.0 {
                        sum += self.head_gain[h] * read_delay(l, base * (h + 1) as f32);
                    }
                }
            } else {
                sum = read_delay(l, base);
            }
        } else {
            // digital: a new time is a new read pointer, crossfaded in. No pitch bend.
            if !l.fading && (target - l.d_a).abs() > 0.5 {
                l.d_b = target;
                l.fading = true;
                l.xf = 0.0;
            }

            let a = read_delay(l, l.d_a + mod_abs);
            if l.fading {
                let b = read_delay(l, l.d_b + mod_abs);
                sum = a * (1.0 - l.xf) + b * l.xf;
                l.xf += self.xf_inc;
                if l.xf >= 1.0 {
                    l.d_a = l.d_b;
                    l.fading = false;
                    l.xf = 0.0;
                }
            } else {
                sum = a;
            }
            l.d_cur = l.d_a;
        }
        sum
    }

    /// Read side: everything a repeat passes through.
    fn read_path(&self, l: &mut Line, target: f32, mod_rel: f32, mod_abs: f32) -> f32 {
        let mut r = self.read_taps(l, target, mod_rel, mod_abs);

        // pattern taps: raw reads at fractions of the current delay. Output only, never fed back.
        let mut extra = 0.0f32;
        if self.pat_n > 0 && !self.reverse_on {
            for t in 0..4 {
                if self.pat_gain[t] > 0.0 {
                    extra += self.pat_gain[t] * read_delay(l, l.d_cur * self.pat_frac[t] * (1.0 + mod_rel) + mod_abs);
                }
            }
        }

        // hiss / bucket noise enters here, inside the loop
        if self.noise_amt > 0.0 {
            r += self.noise_amt * white_noise(l);
        }

        let mut expand = 1.0f32;
        if self.use_compander {
            // NE570 expander half, then the reconstruction filter
            l.env_out += (r.abs() - l.env_out) * self.env_coef;
            expand = cmin(l.env_out / COMP_REF, 20.0);
            r *= expand;
            r = filter(&mut l.recon[0], r);
            r = filter(&mut l.recon[1], r);
        }

        if self.use_style_filt {
            l.lp_z = r + self.style_lp_coef * (l.lp_z - r); // playback head / converter rolloff
            let hp = l.lp_z - l.hp_x1 + self.style_hp_coef * l.hp_y1;
            l.hp_x1 = l.lp_z;
            l.hp_y1 = hp;
            r = hp;
            if self.use_bump {
                // head bump, and the splice going past the head
                r = filter(&mut l.bump, r) * (1.0 - self.splice_depth * self.splice_env);
            }
        }

        r = filter(&mut l.lc, r);
        r = filter(&mut l.hc, r);

        for i in 0..4 {
            r = allpass(&mut l.ap[i], r, self.ap_delay[i]);
        }

        if self.pitch_idx > 0 {
            if self.ps_guard_on {
                r = filter(&mut l.ps_guard[0], r);
                r = filter(&mut l.ps_guard[1], r);
            }
            r = self.pitch_shift(l, r);
        }

        l.fb_sample = r;

        if self.pat_n > 0 && !self.reverse_on {
            extra *= expand;
            if self.use_style_filt || self.use_compander {
                l.lp_zx = extra + self.style_lp_coef * (l.lp_zx - extra);
            } else {
                l.lp_zx = extra;
            }
            r += filter(&mut l.hc_x, l.lp_zx);
        }
        r
    }

    /// Write side: what happens on the way into the line.
    fn record_path(&self, l: &mut Line, x: f32) -> f32 {
        let mut w = x;

        if self.use_compander {
            w = filter(&mut l.aa, w);
            l.env_in += (w.abs() - l.env_in) * self.env_coef;
            let g = cmin((COMP_REF / cmax(l.env_in, 1.0e-4)).sqrt(), 20.0);
            w *= g;
            w = self.bbd_clip * sat_adaa(l, w / self.bbd_clip); // bucket-brigade squash
        } else if self.use_sat {
            // preamp / tape: drive into tanh, bias for a little even-order, makeup so Character isn't a volume knob
            w = (sat_adaa(l, w * self.sat_drive + self.sat_bias) - self.sat_bias.tanh()) * self.sat_makeup;
        }

        if self.use_crush && self.crush_levels > 1.0 {
            w = (w * self.crush_levels + 0.5).floor() / self.crush_levels;
        }

        // Clocked sample-and-hold. On BBD this IS the bucket brigade.
        if self.use_hold && self.hold_ratio < 1.0 {
            l.hold_phase += self.hold_ratio;
            if l.hold_phase >= 1.0 {
                l.hold_phase -= 1.0;
                l.held = w;
            }
            w = l.held;
            if self.use_compander {
                // charge-transfer loss
                l.loss_z = w + self.bbd_loss_coef * (l.loss_z - w);
                w = l.loss_z;
            }
        }

        soft_limit(w)
    }
}

fn write_line(l: &mut Line, w: f32) {
    l.buf[l.write_pos as usize] = w;
    l.write_pos += 1;
    if l.write_pos >= BUF {
        l.write_pos = 0;
    }
}

/// The DelayEngine DSP.
#[derive(Debug, Clone, PartialEq)]
pub struct DelayEngineDsp {
    ln: [Line; 2],
    s: State,
}

impl Default for DelayEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl DelayEngineDsp {
    /// `DelayEngineDSP() { prepare (48000.0); }`. Every buffer is allocated here, once.
    pub fn new() -> Self {
        let mut d = DelayEngineDsp { ln: [Line::new(), Line::new()], s: State::initial() };
        d.prepare(48_000.0);
        d
    }

    /// Puts the engine back exactly as [`DelayEngineDsp::new`] makes it, in place: every field to
    /// its initialiser, the buffers zeroed (never re-made), then the constructor's
    /// `prepare (48000)`. No allocation.
    pub fn restore_initial(&mut self) {
        self.s = State::initial();
        for l in &mut self.ln {
            l.restore_initial();
        }
        self.prepare(48_000.0);
    }

    /// The address of every buffer the engine owns (tests: nothing is re-made).
    #[cfg(test)]
    pub(crate) fn buffer_ptrs(&self) -> Vec<(*const f32, usize)> {
        let mut v = Vec::new();
        for l in &self.ln {
            v.push((l.buf.as_ptr(), l.buf.capacity()));
            v.push((l.ps.as_ptr(), l.ps.capacity()));
            for a in &l.ap {
                v.push((a.b.as_ptr(), a.b.capacity()));
            }
        }
        v
    }

    /// `prepareToPlay` on this live instance (SoundCraft's `reset`: Play, Stop, Locate, a render's
    /// start): every piece of audio state zeroed in place (lines, shifter and allpass buffers,
    /// filter memories, envelopes, holds, heads, grains, detectors, the modulators' phases), the
    /// parameter and tap state kept, then `prepare (sr)` on that state (glides seated at the current
    /// targets, the knob smoothers at their knobs) and the other smoothers at their targets, so the
    /// first repeat after a locate is already where the settings put it. Rust addition: the right
    /// line is seated at its own target (the C++ seats both at the left's, then glides). No
    /// allocation.
    pub fn reset_live(&mut self, sample_rate: f64) {
        for l in &mut self.ln {
            l.buf.fill(0.0);
            l.write_pos = 0;
            l.xf = 0.0;
            l.fading = false;
            let clear = |q: &mut Biquad| {
                q.z1 = 0.0;
                q.z2 = 0.0;
            };
            for q in [&mut l.lc, &mut l.hc, &mut l.aa, &mut l.bump, &mut l.hc_x] {
                clear(q);
            }
            for q in l.recon.iter_mut().chain(l.ps_guard.iter_mut()) {
                clear(q);
            }
            l.lp_z = 0.0;
            l.hp_x1 = 0.0;
            l.hp_y1 = 0.0;
            l.lp_zx = 0.0;
            l.hold_phase = 0.0;
            l.held = 0.0;
            l.loss_z = 0.0;
            l.rev_c = 0.0;
            l.env_in = 0.0;
            l.env_out = 0.0;
            l.sat_x1 = 0.0;
            l.sat_f1 = 0.0;
            for a in &mut l.ap {
                a.clear();
            }
            l.ps.fill(0.0);
            l.ps_pos = 0;
            l.ps_active = 0;
            l.ps_fading = false;
            l.fb_sample = 0.0;
        }
        {
            let s = &mut self.s;
            s.ctrl_counter = 0;
            s.lfo_phase = 0.0;
            s.wow_phase = 0.0;
            s.flut_phase = 0.0;
            s.drift = 0.0;
            // The drift LCG back to its initialiser, so a Tape or Oil Can wobble after a reset does
            // not depend on how much audio played before it.
            s.seed = 22222;
            s.drift_target = 0.0;
            s.splice_env = 0.0;
            s.splice_counter = 0;
            s.duck_env = 0.0;
            s.env_fast = 0.0;
            s.env_slow = 0.0;
            s.gate_open = false;
            s.gate_hold = 0;
            s.win = 0.0;
            s.win_sm = 0.0;
            s.win_hold = 0;
        }
        self.prepare(sample_rate);
        let s = &mut self.s;
        // The smoothers `prepare` leaves alone, at their targets.
        let wm = cmin(1.0, 2.0 * s.mix_knob);
        s.wet_g_sm = wm * wm.sqrt();
        s.dry_g_sm = cmin(1.0, 2.0 * (1.0 - s.mix_knob));
        s.gate_sm = if s.bypassed || s.frozen { 0.0 } else { 1.0 };
        s.byp_sm = if s.bypassed { 1.0 } else { 0.0 };
        s.freeze_sm = if s.frozen { 1.0 } else { 0.0 };
        s.focus_sm = s.focus_knob;
        s.gate_gain = if s.thr_lin > 0.0 { 0.0 } else { 1.0 };
        let (tl, tr) = (s.target_l, s.target_r);
        let [l0, l1] = &mut self.ln;
        for (l, t) in [(l0, tl), (l1, tr)] {
            l.d_cur = t;
            l.d_a = t;
            l.d_b = t;
        }
    }

    /// Whether the tempo in force is the engine's own rather than the host's: Tempo Src on Manual,
    /// or a tap forcing it (`tapForceManual`, cleared by touching Tempo Src).
    pub fn tempo_off_session(&self) -> bool {
        self.s.tempo_src_idx == 1 || self.s.tap_force_manual
    }

    /// The loop's own delay right now (`loopComp`), ms.
    pub fn loop_comp_ms(&self) -> f32 {
        self.s.loop_comp / self.s.fs * 1000.0
    }

    /// The prepared rate.
    pub fn sample_rate(&self) -> f32 {
        self.s.fs
    }

    // ------------------------------------------------------------------ host transport
    /// `setHostTransport (playing, bpm)`: a tempo outside 20..999 BPM (or none) reads as 0.
    pub fn set_host_transport(&mut self, playing: bool, bpm: f32) {
        self.s.host_playing = playing;
        self.s.host_bpm = if (20.0..=999.0).contains(&bpm) { bpm } else { 0.0 };
    }

    pub fn get_host_bpm(&self) -> f32 {
        self.s.host_bpm
    }

    // ------------------------------------------------------------------ outputs back to the UI
    /// `gateOut`: 1 while the Threshold gate is letting playing through.
    pub fn gate_out(&self) -> f32 {
        self.s.gate_out
    }

    /// `tapBpmOut`: the tempo the DSP timed from the Tap button.
    pub fn tap_bpm_out(&self) -> f32 {
        self.s.tap_bpm_out
    }

    /// `tapSeq`: bumped on every accepted tap.
    pub fn tap_seq(&self) -> i32 {
        self.s.tap_seq
    }

    /// The tempo `updateControl` last settled on (host, tapped or manual), 20..999 BPM.
    pub fn bpm_now(&self) -> f32 {
        self.s.bpm_now
    }

    /// The echo times the lines run at right now, in ms: the read targets plus the loop's own
    /// delay (diffusion and pitch shifter), i.e. when a repeat comes back. `(left, right)`.
    pub fn echo_ms(&self) -> (f32, f32) {
        let s = &self.s;
        let to_ms = |t: f32| (t + s.loop_comp) / s.fs * 1000.0;
        (to_ms(s.target_l), to_ms(s.target_r))
    }

    // ------------------------------------------------------------------ events
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        let s = &mut self.s;
        // `int (v + 0.5f)`: Rust's saturating cast equals the C++ for every in-range value.
        let idx = (v + 0.5) as i32;
        match index {
            P_TIME => s.time_knob = v,
            P_FEEDBACK => s.fb_knob = v,
            P_MIX => s.mix_knob = v,
            P_STYLE => {
                s.style_idx = idx;
                self.apply_style();
            }
            P_ROUTING => s.route_idx = idx,
            P_BYPASS => s.bypassed = v > 0.5,
            P_TRIM => {
                s.trim_db = knob_to_trim_db(v);
                s.trim_lin = 10.0f32.powf(s.trim_db / 20.0);
            }
            P_LEVEL => s.level_knob = v,
            P_SYNC => s.synced = v > 0.5,
            P_DIVISION => s.div_idx = idx,
            P_LOW_CUT => s.lc_knob = v,
            P_HIGH_CUT => s.hc_knob = v,
            P_CHARACTER => s.char_knob = v,
            P_MOD_RATE => s.rate_knob = v,
            P_MOD_DEPTH => s.depth_knob = v,
            P_OFFSET => s.offset_knob = v,
            P_HEADS => {
                s.heads_idx = idx;
                self.apply_heads();
            }
            P_DIFFUSION => s.diff_knob = v,
            P_DUCK => s.duck_knob = v,
            P_SWING => s.swing_knob = v,
            P_PITCH => {
                s.pitch_idx = idx;
                self.apply_pitch();
            }
            P_FREEZE => s.frozen = v > 0.5,
            P_WIDTH => s.width_knob = v,
            P_TRAILS => s.trails_on = v > 0.5,
            P_TEMPO => {
                s.tempo_knob = v;
                s.tap_active = false;
            }
            P_TEMPO_SRC => {
                s.tempo_src_idx = idx;
                s.tap_force_manual = false;
            }
            P_THRESHOLD => {
                s.thr_knob = v;
                s.thr_lin = 0.0;
                if v > 0.01 {
                    s.thr_lin = 10.0f32.powf(knob_to_threshold_db(v) / 20.0);
                }
            }
            P_FOCUS => s.focus_knob = v,
            P_TAP => self.handle_tap(v),
            P_REVERSE => s.reverse_on = v > 0.5,
            P_NOISE => s.noise_knob = v,
            P_FILT_MOD => {
                s.filt_mod_knob = v;
                s.last_hc = -1.0;
            }
            P_PATTERN => {
                s.pattern_idx = idx;
                self.apply_pattern();
            }
            P_MOD_SYNC => s.mod_sync_on = v > 0.5,
            P_DIVISION_R => s.div_r_idx = idx,
            _ => {}
        }
    }

    /// Tap tempo lives in the DSP. Two taps set it; more taps average. A tap lands on the start of
    /// its block (as in the C++, which only sees parameters at block boundaries).
    fn handle_tap(&mut self, v: f32) {
        let s = &mut self.s;
        if v > 0.5 {
            if !s.tap_down {
                s.tap_down = true;
                let gap = s.sample_clock.wrapping_sub(s.last_tap_clock) as f32;
                s.last_tap_clock = s.sample_clock;
                if gap >= 0.25 * s.fs && gap <= 1.5 * s.fs {
                    // 240 .. 40 BPM
                    if s.tap_interval > 0.0 && (gap - s.tap_interval).abs() < 0.25 * s.tap_interval {
                        s.tap_interval = 0.5 * (s.tap_interval + gap);
                    } else {
                        s.tap_interval = gap;
                    }
                    s.tap_bpm = 60.0 * s.fs / s.tap_interval;
                    s.tap_active = true;
                    s.tap_force_manual = true;
                    s.tap_pending = true;
                } else {
                    s.tap_interval = 0.0;
                }
            }
        } else {
            s.tap_down = false;
        }
    }

    // ------------------------------------------------------------------ init
    pub fn prepare(&mut self, sample_rate: f64) {
        self.s.fs = sample_rate as f32;
        let fs = self.s.fs;

        for l in &mut self.ln {
            // `buf.assign (BUF, 0)` / `ps.assign (PSBUF, 0)`: same contents, in place.
            l.buf.fill(0.0);
            l.ps.fill(0.0);
        }

        // `apMaxMs` = 3.1, 5.3, 8.9, 13.7: the constant `AP_MAX_MS` (`loop_geometry`).

        for (c, l) in self.ln.iter_mut().enumerate() {
            set_lp(&mut l.aa, 3400.0, sample_rate, 0.7071);
            set_lp(&mut l.recon[0], 3400.0, sample_rate, 0.5412);
            set_lp(&mut l.recon[1], 3400.0, sample_rate, 1.3066);
            l.rng = 777 + 4321 * c as i64;
            set_peak(&mut l.bump, 95.0, sample_rate, 0.9, 2.0);
            l.ps_d0 = 0.05 * fs;
            l.ps_d1 = 0.05 * fs;
            l.ps_xf = 1.0;
            l.d_cur = 0.35 * fs;
            l.d_a = l.d_cur;
            l.d_b = l.d_cur;
        }

        let s = &mut self.s;
        s.env_coef = 1.0 - one_pole_coef(12.0, sample_rate);
        s.duck_atk = 1.0 - one_pole_coef(80.0, sample_rate); // ~2 ms
        s.duck_rel = 1.0 - one_pole_coef(0.6, sample_rate); // ~270 ms
        s.xf_inc = 1.0 / (0.04 * fs); // 40 ms crossfade

        s.fast_atk = 1.0 - one_pole_coef(320.0, sample_rate); // ~0.5 ms
        s.fast_rel = 1.0 - one_pole_coef(8.0, sample_rate); // ~20 ms
        s.slow_atk = 1.0 - one_pole_coef(8.0, sample_rate); // ~20 ms
        s.slow_rel = 1.0 - one_pole_coef(1.1, sample_rate); // ~150 ms
        s.gate_atk = 1.0 - one_pole_coef(160.0, sample_rate); // ~1 ms open
        s.gate_rel = 1.0 - one_pole_coef(2.0, sample_rate); // ~80 ms close
        s.win_atk = 1.0 - one_pole_coef(320.0, sample_rate);
        s.win_decay = one_pole_coef(4.5, sample_rate); // ~35 ms fall after the hold
        s.gate_hold_len = (0.15 * fs) as i32; // a note gets at least 150 ms through the gate
        s.win_hold_len = (0.05 * fs) as i32; // the "attack" is the first 50 ms

        s.trim_lin = 10.0f32.powf(s.trim_db / 20.0);
        s.trim_sm = s.trim_lin;
        s.char_sm = s.char_knob;
        s.level_sm = knob_to_level_gain(s.level_knob);
        s.fb_sm = s.fb_knob * 1.15;
        s.width_sm = s.width_knob * 2.0;
        s.duck_sm = s.duck_knob;

        self.apply_pitch();
        self.apply_pattern();
        self.apply_style();
        let t = self.s.target_l;
        for l in &mut self.ln {
            l.d_cur = t;
            l.d_a = t;
            l.d_b = t;
        }
    }

    // ------------------------------------------------------------------ main
    /// Processes `num_samples` (at most each buffer's length) of channels 0 and, when
    /// `num_channels > 1`, 1. One channel runs the left path only, as the C++ does.
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        if num_channels < 1 || io.is_empty() || self.ln[0].buf.is_empty() {
            return;
        }
        let (left, rest) = io.split_at_mut(1);
        let ch_l = &mut left[0];
        let mut ch_r = if num_channels > 1 { rest.first_mut() } else { None };
        let n = match &ch_r {
            Some(r) => num_samples.min(ch_l.len()).min(r.len()),
            None => num_samples.min(ch_l.len()),
        };

        for i in 0..n {
            let x0 = ch_l[i];
            let x1 = match &ch_r {
                Some(r) => r[i],
                None => x0,
            };
            let (y0, y1) = self.tick(x0, x1);
            ch_l[i] = y0;
            if let Some(r) = ch_r.as_deref_mut() {
                r[i] = y1;
            }
        }
    }

    /// One sample of the C++ `process` loop body.
    fn tick(&mut self, x0: f32, x1: f32) -> (f32, f32) {
        {
            let s = &mut self.s;
            s.sample_clock += 1;

            // ---- smoothing
            let k = 0.005f32;
            s.trim_sm += (s.trim_lin - s.trim_sm) * k;
            s.char_sm += (s.char_knob - s.char_sm) * k;
            s.fb_sm += (s.fb_knob * 1.15 - s.fb_sm) * k;
            s.width_sm += (s.width_knob * 2.0 - s.width_sm) * k;
            s.duck_sm += (s.duck_knob - s.duck_sm) * k;
            s.level_sm += (knob_to_level_gain(s.level_knob) - s.level_sm) * k;

            // Mix: dry stays at unity up to noon, wet comes up underneath it (guitar-pedal law).
            let wm = cmin(1.0, 2.0 * s.mix_knob);
            let dm = cmin(1.0, 2.0 * (1.0 - s.mix_knob));
            s.wet_g_sm += (wm * wm.sqrt() - s.wet_g_sm) * k;
            s.dry_g_sm += (dm - s.dry_g_sm) * k;

            let (mut gate_t, mut byp_t, mut frz_t) = (1.0f32, 0.0f32, 0.0f32);
            if s.bypassed {
                gate_t = 0.0;
                byp_t = 1.0;
            }
            if s.frozen {
                gate_t = 0.0;
                frz_t = 1.0;
            }
            s.gate_sm += (gate_t - s.gate_sm) * 0.002;
            s.byp_sm += (byp_t - s.byp_sm) * 0.002;
            s.freeze_sm += (frz_t - s.freeze_sm) * 0.002;

            s.ctrl_counter += 1;
        }
        if self.s.ctrl_counter >= 32 {
            self.s.ctrl_counter = 0;
            self.update_control();
            let s = &mut self.s;

            if s.tap_pending {
                s.tap_pending = false;
                s.tap_bpm_out = s.tap_bpm;
                s.tap_seq = s.tap_seq.wrapping_add(1);
            }

            s.gate_send_counter += 1;
            if s.gate_send_counter >= 50 {
                // ~30 Hz, offset from the tempo report so they never land together
                s.gate_send_counter = 0;
                let mut lamp = 0.0;
                if s.gate_open && s.thr_lin > 0.0 {
                    lamp = 1.0;
                }
                s.gate_out = lamp;
            }
        }

        let Self { ln, s } = self;
        let k = 0.005f32;

        // ---- modulators
        s.lfo_phase += s.lfo_inc;
        if s.lfo_phase >= 1.0 {
            s.lfo_phase -= 1.0;
        }
        s.wow_phase += s.wow_inc;
        if s.wow_phase >= 1.0 {
            s.wow_phase -= 1.0;
        }
        s.flut_phase += s.flut_inc;
        if s.flut_phase >= 1.0 {
            s.flut_phase -= 1.0;
        }

        let tp = TWO_PI as f32;
        let (mut mod_abs_l, mut mod_abs_r) = (0.0f32, 0.0f32);
        if s.mod_samples > 0.01 {
            mod_abs_l = s.mod_samples * (tp * s.lfo_phase).sin();
            mod_abs_r = s.mod_samples * (tp * (s.lfo_phase + 0.25)).sin(); // quarter turn apart for width
        }
        let mut mod_rel = 0.0f32;
        if s.wow_amt + s.flut_amt > 0.0 {
            mod_rel = s.wow_amt * (tp * s.wow_phase).sin()
                + s.flut_amt * (tp * s.flut_phase).sin()
                + s.drift_amt * s.drift * 6.0
                + 0.0015 * s.splice_depth * s.splice_env;
        }

        // ---- ducking envelope from the dry signal
        let in_m = 0.5 * (x0 + x1);
        let lvl = in_m.abs();
        if lvl > s.duck_env {
            s.duck_env += (lvl - s.duck_env) * s.duck_atk;
        } else {
            s.duck_env += (lvl - s.duck_env) * s.duck_rel;
        }
        let duck_g = 1.0 - s.duck_sm * 0.95 * clamp_t(s.duck_env * 8.0, 0.0, 1.0);

        // ---- dynamics on the way INTO the delay (the dry signal is never touched)
        let det = lvl * s.trim_sm;
        if det > s.env_fast {
            s.env_fast += (det - s.env_fast) * s.fast_atk;
        } else {
            s.env_fast += (det - s.env_fast) * s.fast_rel;
        }
        if det > s.env_slow {
            s.env_slow += (det - s.env_slow) * s.slow_atk;
        } else {
            s.env_slow += (det - s.env_slow) * s.slow_rel;
        }

        // Threshold: opens when you dig in, holds 150 ms, 6 dB of hysteresis
        let mut thr_t = 1.0f32;
        if s.thr_lin > 0.0 {
            if s.env_fast > s.thr_lin {
                s.gate_open = true;
                s.gate_hold = s.gate_hold_len;
            } else if s.env_fast < s.thr_lin * 0.5 {
                if s.gate_hold > 0 {
                    s.gate_hold -= 1;
                } else {
                    s.gate_open = false;
                }
            }
            if !s.gate_open {
                thr_t = 0.0;
            }
        } else {
            s.gate_open = true;
        }
        if thr_t > s.gate_gain {
            s.gate_gain += (thr_t - s.gate_gain) * s.gate_atk;
        } else {
            s.gate_gain += (thr_t - s.gate_gain) * s.gate_rel;
        }

        // Attack Focus: a new pick attack opens a 50 ms window
        s.focus_sm += (s.focus_knob - s.focus_sm) * k;
        if s.env_fast > s.env_slow * 1.35 && s.env_fast > 0.003 {
            s.win = 1.0;
            s.win_hold = s.win_hold_len;
        } else if s.win_hold > 0 {
            s.win_hold -= 1;
        } else {
            s.win *= s.win_decay;
        }
        if s.win > s.win_sm {
            s.win_sm += (s.win - s.win_sm) * s.win_atk;
        } else {
            s.win_sm = s.win;
        }
        let send_g = s.gate_gain * ((1.0 - s.focus_sm) + s.focus_sm * s.win_sm);

        // ---- read both lines first, then decide what gets written back
        let in_l = x0 * s.trim_sm * s.gate_sm * send_g;
        let in_r = x1 * s.trim_sm * s.gate_sm * send_g;
        let in_mono = 0.5 * (in_l + in_r);
        let mut fb = s.fb_sm * s.head_fb_norm;
        if s.pat_n > 0 || s.reverse_on {
            fb = s.fb_sm; // heads are out of circuit in these modes
        }

        let (wet_l, wet_r);
        let mut wr_l: f32;
        let mut wr_r = 0.0f32;
        let s = &*s;
        let [l0, l1] = ln;

        if s.route_idx == 0 {
            let o_l = s.read_path(l0, s.target_l, mod_rel, mod_abs_l);
            wet_l = o_l;
            wet_r = o_l;
            wr_l = s.record_path(l0, in_mono + l0.fb_sample * fb);
        } else if s.route_idx == 2 {
            let o_l = s.read_path(l0, s.target_l, mod_rel, mod_abs_l);
            let o_r = s.read_path(l1, s.target_r, mod_rel, mod_abs_r);
            wet_l = o_l;
            wet_r = o_r;
            let fb1 = l1.fb_sample;
            wr_l = s.record_path(l0, in_mono + fb1 * fb);
            let fb0 = l0.fb_sample;
            wr_r = s.record_path(l1, fb0 * fb);
        } else {
            let o_l = s.read_path(l0, s.target_l, mod_rel, mod_abs_l);
            let o_r = s.read_path(l1, s.target_r, mod_rel, mod_abs_r);
            wet_l = o_l;
            wet_r = o_r;
            wr_l = s.record_path(l0, in_l + l0.fb_sample * fb);
            wr_r = s.record_path(l1, in_r + l1.fb_sample * fb);
        }

        // Freeze: each line re-records itself untouched at a whole-sample delay
        if s.freeze_sm > 0.0001 {
            let fz_l = read_raw(l0, (s.target_l + 0.5) as i32);
            let fz_r = read_raw(l1, (s.target_r + 0.5) as i32);
            wr_l = wr_l * (1.0 - s.freeze_sm) + fz_l * s.freeze_sm;
            wr_r = wr_r * (1.0 - s.freeze_sm) + fz_r * s.freeze_sm;
        }

        write_line(l0, wr_l);
        write_line(l1, wr_r);

        // ---- wet: undo the input trim, width, duck
        let comp = 1.0 / cmax(s.trim_sm, 0.05);
        let m = 0.5 * (wet_l + wet_r);
        let sd = 0.5 * (wet_l - wet_r) * s.width_sm;
        let w_l = (m + sd) * comp * s.wet_g_sm * duck_g * s.level_sm;
        let w_r = (m - sd) * comp * s.wet_g_sm * duck_g * s.level_sm;

        // ---- output. Bypassed = the untouched input, plus the tails if Trails is on.
        let mut tail = 0.0f32;
        if s.trails_on {
            tail = 1.0;
        }

        let y0 = (1.0 - s.byp_sm) * (x0 * s.dry_g_sm * s.level_sm + w_l) + s.byp_sm * (x0 + w_l * tail);
        let y1 = (1.0 - s.byp_sm) * (x1 * s.dry_g_sm * s.level_sm + w_r) + s.byp_sm * (x1 + w_r * tail);
        (y0, y1)
    }

    // ------------------------------------------------------------------ style table
    fn apply_style(&mut self) {
        let s = &mut self.s;
        s.glide_style = false;
        s.multi_head = false;
        s.use_style_filt = false;
        s.use_compander = false;
        s.use_sat = false;
        s.use_crush = false;
        s.use_hold = false;
        s.use_bump = false;
        s.max_head_ratio = 1.0;
        let mut glide_ms = 50.0f32;

        match s.style_idx {
            0 => {
                s.use_crush = true;
                s.use_hold = true;
            }
            1 => {
                s.use_sat = true;
                s.use_crush = true;
                s.use_style_filt = true;
            }
            2 => {
                s.glide_style = true;
                s.use_compander = true;
                s.use_hold = true;
                glide_ms = 50.0;
            }
            3 => {
                s.glide_style = true;
                s.multi_head = true;
                s.use_sat = true;
                s.use_style_filt = true;
                s.use_bump = true;
                glide_ms = 280.0;
                s.max_head_ratio = 3.0;
            }
            4 => {
                s.glide_style = true;
                s.multi_head = true;
                s.use_sat = true;
                s.use_style_filt = true;
                glide_ms = 160.0;
                s.max_head_ratio = 4.0;
            }
            _ => {
                s.glide_style = true;
                s.use_sat = true;
                s.use_style_filt = true;
                glide_ms = 220.0;
            }
        }

        s.glide_coef = 1.0 - (-1.0f32 / (glide_ms * 0.001 * s.fs)).exp();

        // start the new style from where the old one was, so switching styles doesn't swoop
        for l in &mut self.ln {
            l.d_a = l.d_cur;
            l.d_b = l.d_cur;
            l.fading = false;
            l.xf = 0.0;
        }

        self.apply_heads();
        self.update_control();
    }

    fn apply_heads(&mut self) {
        let s = &mut self.s;
        let t = head_table(s.multi_head, s.style_idx, s.heads_idx);
        s.head_gain = t.gain;
        s.head_fb_norm = t.fb_norm;
        s.max_head_ratio = t.max_ratio;
    }

    fn apply_pattern(&mut self) {
        let s = &mut self.s;
        let (n, f, g) = pattern_table(s.pattern_idx);
        s.pat_n = n;
        s.pat_frac = f;
        s.pat_gain = g;
    }

    fn apply_pitch(&mut self) {
        self.s.pitch_ratio = pitch_ratio_for(self.s.pitch_idx);
        self.update_pitch_guard();
    }

    /// Shifting UP reads the line faster than it was written: band-limit first (4th-order
    /// Butterworth). Down-shifts are left completely alone.
    fn update_pitch_guard(&mut self) {
        let s = &mut self.s;
        s.ps_guard_on = s.pitch_ratio > 1.0;
        if !s.ps_guard_on {
            return;
        }
        let fs = f64::from(s.fs);
        let a = 0.92 * fs / (2.0 * f64::from(s.pitch_ratio));
        let b = 0.45 * fs;
        let fc = if b < a { b } else { a };
        for l in &mut self.ln {
            set_lp(&mut l.ps_guard[0], fc, fs, 0.5412);
            set_lp(&mut l.ps_guard[1], fc, fs, 1.3066);
        }
    }

    // ------------------------------------------------------------------ control rate (every 32 samples)
    fn update_control(&mut self) {
        let Self { ln, s } = self;
        let fs = s.fs;
        let ch = s.char_sm;

        // ---- character, per style
        s.sat_drive = 1.0;
        s.sat_bias = 0.0;
        s.sat_makeup = 1.0;
        s.crush_levels = 0.0;
        s.wow_amt = 0.0;
        s.flut_amt = 0.0;
        s.drift_amt = 0.0;
        s.style_diff = 0.0;
        let (mut lp_hz, mut hp_hz, mut wow_hz, mut flut_hz) = (20_000.0f32, 20.0f32, 0.5f32, 6.0f32);

        s.hold_ratio = 1.0;
        s.splice_depth = 0.0;
        let t_sec = cmax(s.target_l + s.loop_comp, 8.0) / fs; // the delay time this tick, seconds

        match s.style_idx {
            0 => {
                if ch > 0.02 {
                    s.crush_levels = 2.0f32.powf(15.0 - 10.0 * ch); // 16 bit -> 6 bit
                    if ch > 0.4 {
                        // top of the range: 32 kHz -> 4 kHz converter
                        s.hold_ratio = 32_000.0 * 0.125f32.powf((ch - 0.4) / 0.6) / fs;
                    }
                }
            }
            1 => {
                s.sat_drive = 1.0 + 7.0 * ch; // up to +18 dB into the preamp
                s.crush_levels = 2048.0; // 12-bit converters
                lp_hz = 12_000.0;
                hp_hz = 30.0;
            }
            2 => {
                s.bbd_clip = 0.7 - 0.45 * ch;
                lp_hz = 3400.0; // (pattern taps only; the main path has the real 6-pole filters)

                // MN3005: 4096 stages, delay = stages / (2 x clock).
                let chips = ((t_sec / 0.3) as i32).wrapping_add(1) as f32;
                let clock_hz = 4096.0 * chips / (2.0 * t_sec);
                s.hold_ratio = clock_hz / fs;
                s.bbd_loss_coef = one_pole_coef(f64::from(cmin(0.4 * clock_hz, 0.45 * fs)), f64::from(fs));
            }
            3 => {
                s.sat_drive = 1.0 + 5.0 * ch;
                s.sat_bias = 0.12;

                // Repeat Rate on a tape echo changes TAPE SPEED, and gap loss follows speed.
                let speed = clamp_t((0.12 / t_sec).sqrt(), 0.45, 1.15);
                lp_hz = (9000.0 - 4500.0 * ch) * speed;
                hp_hz = 70.0;
                for l in ln.iter_mut() {
                    set_peak(&mut l.bump, f64::from(95.0f32 * speed), f64::from(fs), 0.9, 2.0);
                }

                // the loop splice comes round every so often
                s.splice_depth = 0.12 + 0.33 * ch;
                s.splice_period = (clamp_t(11.0 / speed, 6.0, 30.0) * fs / 32.0) as i32;
                s.wow_amt = 0.0012 + 0.0030 * ch;
                wow_hz = 0.55;
                s.flut_amt = 0.0004 + 0.0012 * ch;
                flut_hz = 6.8;
                s.drift_amt = 0.0006 * (1.0 + 2.0 * ch);
            }
            4 => {
                s.sat_drive = 1.0 + 3.0 * ch;
                lp_hz = 9500.0 - 2500.0 * ch;
                hp_hz = 90.0;
                s.flut_amt = 0.0005 + 0.0010 * ch;
                flut_hz = 3.1;
            }
            _ => {
                s.sat_drive = 1.5 + 3.0 * ch;
                s.sat_bias = 0.08;
                lp_hz = 3200.0 - 800.0 * ch;
                hp_hz = 160.0;
                s.wow_amt = 0.0030 + 0.0040 * ch;
                wow_hz = 1.1;
                s.flut_amt = 0.0020 + 0.0040 * ch;
                flut_hz = 5.7;
                s.drift_amt = 0.0010;
                s.style_diff = style_diff(s.style_idx, ch);
            }
        }

        s.sat_makeup = 1.0 / s.sat_drive.powf(0.85); // quiet notes stay near unity, loud ones compress like tape
        if lp_hz > 0.45 * fs {
            lp_hz = 0.45 * fs;
        }
        s.style_lp_coef = one_pole_coef(f64::from(lp_hz), f64::from(fs));
        s.style_hp_coef = one_pole_coef(f64::from(hp_hz), f64::from(fs));
        s.wow_inc = wow_hz / fs;
        s.flut_inc = flut_hz / fs;

        // ---- slow random drift for the mechanical styles
        s.seed = (s.seed.wrapping_mul(1_103_515_245).wrapping_add(12345)) % 2_147_483_647;
        s.drift_target = s.seed as f32 / 1_073_741_824.0 - 1.0;
        s.drift += (s.drift_target - s.drift) * 0.004;

        // tape splice
        s.splice_counter = s.splice_counter.wrapping_add(1);
        if s.splice_counter >= s.splice_period {
            s.splice_counter = 0;
            if s.splice_depth > 0.0 {
                s.splice_env = 1.0;
            }
        }
        s.splice_env *= 0.97; // ~25 ms

        // Noise: -inf .. about -46 dBFS inside the loop. BBD is noisiest, digital styles quietest.
        let mut noise_scale = 0.4f32;
        if s.style_idx == 2 {
            noise_scale = 1.6;
        }
        if s.style_idx >= 3 {
            noise_scale = 1.0;
        }
        s.noise_amt = s.noise_knob * s.noise_knob * 0.005 * noise_scale;

        // ---- user filters (only when they move)
        if (s.lc_knob - s.last_lc).abs() > 0.0005 {
            s.last_lc = s.lc_knob;
            let f = knob_to_low_cut_hz(s.lc_knob);
            for l in ln.iter_mut() {
                set_hp(&mut l.lc, f, f64::from(fs), 0.7071);
            }
        }
        s.lfo_now = (TWO_PI as f32 * s.lfo_phase).sin();
        if (s.hc_knob - s.last_hc).abs() > 0.0005 || s.filt_mod_knob > 0.001 {
            s.last_hc = s.hc_knob;
            let hc_base = knob_to_high_cut_hz(s.hc_knob);
            let top = 0.45 * f64::from(fs);
            // Filter Sweep: the mod LFO moves High Cut by up to +/- 2 octaves, right channel a quarter turn behind
            let mut f_l = hc_base * 2.0f64.powf(f64::from(2.0 * s.filt_mod_knob * s.lfo_now));
            let mut f_r = hc_base * 2.0f64.powf(f64::from(2.0 * s.filt_mod_knob * (TWO_PI as f32 * (s.lfo_phase + 0.25)).sin()));
            if f_l > top {
                f_l = top;
            }
            if f_r > top {
                f_r = top;
            }
            if f_l < 200.0 {
                f_l = 200.0;
            }
            if f_r < 200.0 {
                f_r = 200.0;
            }
            let [l0, l1] = ln;
            set_lp(&mut l0.hc, f_l, f64::from(fs), 0.7071);
            set_lp(&mut l1.hc, f_r, f64::from(fs), 0.7071);
            set_lp(&mut l0.hc_x, f_l, f64::from(fs), 0.7071);
            set_lp(&mut l1.hc_x, f_r, f64::from(fs), 0.7071);
        }

        // ---- tempo: host transport, unless Manual is chosen or you've just tapped one in
        let mut bpm = knob_to_bpm(s.tempo_knob);
        if s.tap_active {
            bpm = s.tap_bpm;
        }
        if s.tempo_src_idx == 0 && !s.tap_force_manual && s.host_bpm >= 20.0 {
            bpm = s.host_bpm;
        }
        bpm = clamp_t(bpm, 20.0, 999.0);
        s.bpm_now = bpm;

        // ---- modulation LFO. Mod Sync turns the rate knob into 8 bars .. 1/16.
        s.lfo_inc = knob_to_mod_rate_hz(s.rate_knob) / fs;
        if s.mod_sync_on {
            let ri = (clamp_t(s.rate_knob, 0.0, 0.999) * 8.0) as i32;
            s.lfo_inc = bpm / 60.0 / lfo_beats(ri) / fs;
        }
        s.mod_samples = s.depth_knob * s.depth_knob * 0.006 * fs; // up to 6 ms

        // ---- diffusion and pitch-shifter geometry (`loop_geometry`: the same lines, shared with the
        // UI so its tap display reads the loop delay the engine runs)
        let g = loop_geometry(fs, s.diff_knob + s.style_diff, s.pitch_ratio, s.pitch_idx);
        s.ap_delay = g.ap_delay;
        s.ps_xf_len = g.ps_xf_len;
        s.ps_lag_step = g.ps_lag_step;
        s.ps_seg_step = g.ps_seg_step;
        s.ps_lag_count = g.ps_lag_count;
        s.ps_min_d = g.ps_min_d;
        s.ps_window = g.ps_window;
        s.loop_comp = g.loop_comp;

        // ---- delay time
        let mut ms = knob_to_ms(s.time_knob);
        if s.synced {
            ms = 60_000.0 / bpm * division_beats(s.div_idx);
        }

        let base = ms * 0.001 * fs;
        let (mut t_l, mut t_r) = (base, base);
        if s.route_idx == 2 {
            let sw = (s.swing_knob - 0.5) * 0.6; // ping-pong: long hop then short hop = swung repeats
            t_l = base * (1.0 + sw);
            t_r = base * (1.0 - sw);
        } else if s.route_idx == 3 {
            t_r = base * (0.5 + s.offset_knob);
            if s.synced && s.div_r_idx > 0 {
                // right side gets its own note value
                t_r = 60_000.0 / bpm * division_beats(s.div_r_idx - 1) * 0.001 * fs;
            }
        }

        let mut room = s.max_head_ratio * 1.05;
        if s.pat_n > 0 {
            room = 1.05;
        }
        if s.reverse_on {
            room = 2.1; // a reverse grain reaches back two delay times
        }
        let lim = (BUF as f32 - 64.0) / room - s.mod_samples;
        s.target_l = clamp_t(t_l - s.loop_comp, 8.0, lim);
        s.target_r = clamp_t(t_r - s.loop_comp, 8.0, lim);
    }
}

/// Mod Sync's note values, 8 bars .. 1/16, in beats (`updateControl`).
pub fn lfo_beats(ri: i32) -> f32 {
    match ri {
        1 => 16.0,
        2 => 8.0,
        3 => 4.0,
        4 => 2.0,
        5 => 1.0,
        6 => 0.5,
        7 => 0.25,
        _ => 32.0,
    }
}
