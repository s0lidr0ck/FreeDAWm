//! PITCH ENGINE -- one shared shifter core, five styles.
//! Line-by-line translation of `Engine 3 - Pitch/Source/PitchEngineDSP.h` (class
//! `fs::pitch::PitchEngineDSP`, in the header's order).
//!
//! Core (shared by every shifting style): one circular buffer per channel; every voice is a pair
//! of read heads sweeping across a window at the pitch ratio. When a head runs out of window the
//! other starts at a WAVEFORM-MATCHED point (normalised cross-correlation over the recent
//! history) and the two crossfade. Voices: sub (-2 oct), -1 oct, 5th (3 modes), +1 oct, +2 oct,
//! two detuned doubles, one glide. Wet path: voices -> width pan -> swell (Attack) -> LP filter
//! with Q -> Level. Dry is untouched.
//!
//! Styles: POG (all five interval voices, 30 ms window), Micro POG (sub + octave up, 12 ms),
//! OC-2 (analog flip-flop divider, monophonic), H910 (doubler, two detuned voices, 40 ms),
//! Whammy (one voice glides from unison to a stepped interval under the Pedal knob).
//!
//! Latency: the dry path is untouched (zero). Shifted voices are inherently late by roughly half
//! the window; the C++ does not report it to the host (see `latency_ms`).
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - `float` and `double` exactly where the C++ has them: the signal path, smoothers and knob
//!   maps are `f32` (`std::pow (66.6667f, v)` is `powf`); `setLP` and `onePoleCoef` compute in
//!   `f64` and store `f32`, as the C++ casts.
//! - `std::max` / `std::min` and the header's `clampT` are the C++ comparisons (`cmax`, `cmin`,
//!   `clamp_t`), so a NaN goes the way it goes there rather than being dropped like Rust's `max`.
//! - Integer index arithmetic keeps the C++ `int` (`i32`) expressions, with `%` truncating toward
//!   zero exactly as in C++ (`bufAt`'s modulo fold). Arithmetic that would be signed overflow in
//!   C++ (undefined; unreachable with knobs in 0..1) wraps here instead of panicking. `(int) x`
//!   of a float is `x as i32` (truncation; the C++ value is always in range).
//! - The per-channel buffer (`std::array<float, BUF>` inside the engine object) is a `Vec`
//!   allocated once in [`PitchEngineDsp::new`]; `prepare`'s `c = Channel {}` re-fills it in place
//!   (zeros) and re-defaults every other field, so `prepare` never allocates.
//! - Buffer reads go through `get` with a 0.0 fallback (never-crash); every index the C++ makes
//!   is already folded into `0..BUF`, so the fallback is never taken.
//! - `trackLamp` is a `std::atomic<float>` the editor polls: here a plain field read through
//!   [`PitchEngineDsp::track_lamp`].
//! - The C++ splits `engine (Channel&, ...)` / `shiftVoice (Channel&, Head&, ...)` across one
//!   object; here the scalars live in [`State`] and the channels beside it, so a channel can be
//!   borrowed mutably while the state is read (the same reads and writes, in the same order).

// Constants and literals are kept exactly as the C++ writes them.
#![allow(clippy::excessive_precision, clippy::approx_constant)]
// The C++ clamps written as the C++ writes them (`if (n < 1) n = 1; ...`).
#![allow(clippy::manual_clamp)]
// Index loops over parallel per-voice arrays are kept as the C++ writes them.
#![allow(clippy::needless_range_loop)]

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
// Declared in this order -> param1..param22. `Parameters.h` (and the module's table) must match.
pub const P_STYLE: usize = 0;
pub const P_DRY: usize = 1;
pub const P_SUB: usize = 2;
pub const P_OCT_DOWN: usize = 3;
pub const P_OCT_UP: usize = 4;
pub const P_OCT2_UP: usize = 5;
pub const P_FIFTH: usize = 6;
pub const P_FIFTH_MODE: usize = 7;
pub const P_DETUNE: usize = 8;
pub const P_ATTACK: usize = 9;
pub const P_LPF: usize = 10;
pub const P_RESO: usize = 11;
pub const P_LEVEL: usize = 12;
pub const P_BYPASS: usize = 13;
pub const P_TRIM: usize = 14;
pub const P_VOICE: usize = 15;
pub const P_INTERVAL: usize = 16;
pub const P_PEDAL: usize = 17;
pub const P_GLIDE: usize = 18;
pub const P_WIDTH: usize = 19;
pub const P_GRAIN: usize = 20;
pub const P_TRACK: usize = 21;
pub const NUM_PARAMS: usize = 22;

// ------------------------------------------------------------------ constants
/// Per channel: 341 ms at 48k. Windows never exceed ~85 ms.
pub const BUF: i32 = 16384;
/// 0 sub, 1 -1oct, 2 5th, 3 +1oct, 4 +2oct, 5 dblA, 6 dblB, 7 whammy.
pub const NV: usize = 8;
/// Correlation points per candidate.
pub const MATCH: i32 = 64;

const PI: f64 = 3.14159265358979323846;

pub const STYLE_POG: i32 = 0;
pub const STYLE_MICRO_POG: i32 = 1;
pub const STYLE_OC2: i32 = 2;
pub const STYLE_H910: i32 = 3;
pub const STYLE_WHAMMY: i32 = 4;
pub const NUM_STYLES: i32 = 5;

// ------------------------------------------------------------------ knob -> unit maps
// The UI prints these same numbers. They live here, and only here.
pub fn knob_to_trim_db(v: f32) -> f32 {
    -12.0 + 36.0 * clamp_t(v, 0.0, 1.0)
}
/// 300 Hz .. 20 kHz.
pub fn knob_to_filter_hz(v: f32) -> f32 {
    300.0 * 66.6667f32.powf(v)
}
/// 0.5 .. 6.
pub fn knob_to_q(v: f32) -> f32 {
    0.5 + 5.5 * v * v
}
/// 0 .. 500 ms.
pub fn knob_to_glide_ms(v: f32) -> f32 {
    500.0 * v * v
}
pub fn knob_to_attack_ms(v: f32) -> f32 {
    if v <= 0.01 { 0.0 } else { (0.02 + 1.48 * v * v) * 1000.0 }
}
/// 0 .. 50 cents.
pub fn knob_to_cents(v: f32) -> f32 {
    50.0 * v
}
/// 200 Hz .. 2 kHz.
pub fn knob_to_track_hz(v: f32) -> f32 {
    200.0 * 10.0f32.powf(v)
}
/// 0.5x .. 2x.
pub fn knob_to_grain(v: f32) -> f32 {
    2.0f32.powf(2.0 * v - 1.0)
}
/// Dry and Level share the audio taper: 0.7071 is unity.
pub fn knob_to_gain(v: f32) -> f32 {
    2.0 * v * v
}

/// The window a style uses, in ms, before Grain scales it.
pub fn style_window_ms(style: i32) -> f32 {
    if style == STYLE_MICRO_POG {
        return 12.0;
    }
    if style == STYLE_H910 {
        return 40.0;
    }
    if style == STYLE_WHAMMY {
        return 24.0;
    }
    30.0
}

/// Roughly how late the shifted voices are, in ms. Zero for OC-2, which does not shift.
pub fn latency_ms(style: i32, grain_knob: f32) -> f32 {
    if style == STYLE_OC2 {
        return 0.0;
    }
    0.5 * style_window_ms(style) * knob_to_grain(grain_knob)
}

pub fn semi_ratio(semis: f32) -> f32 {
    2.0f32.powf(semis / 12.0)
}

/// The Whammy interval table, in semitones.
pub fn interval_semis(i: i32) -> f32 {
    const S: [f32; 10] = [24.0, 12.0, 7.0, 5.0, 2.0, -2.0, -5.0, -7.0, -12.0, -24.0];
    S[(if i < 0 {
        0
    } else if i > 9 {
        9
    } else {
        i
    }) as usize]
}

// ------------------------------------------------------------------ structs
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

#[derive(Debug, Clone, Copy)]
struct Head {
    /// The two read heads' delays (samples).
    d0: f32,
    d1: f32,
    /// Which head is current.
    active: i32,
    /// 0..1 crossfade into the current head.
    xf: f32,
    fading: bool,
    live: bool,
}

impl Default for Head {
    fn default() -> Self {
        Head { d0: 0.0, d1: 0.0, active: 0, xf: 1.0, fading: false, live: false }
    }
}

struct Channel {
    buf: Vec<f32>,
    pos: i32,
    h: [Head; NV],
    /// Wet filter.
    lp: Biquad,
    // swell
    env_fast: f32,
    env_slow: f32,
    swell_g: f32,
    onset_hold: i32,
    // OC-2 divider
    trk: Biquad,
    env: f32,
    #[allow(dead_code)] // declared (and reset) by the C++, never read
    last_f: f32,
    hi: bool,
    sq1: bool,
    sq2: bool,
    #[allow(dead_code)] // declared (and reset) by the C++, never read
    sq1_prev: bool,
    sub1: Biquad,
    sub2: Biquad,
    gate: f32,
}

impl Channel {
    /// The one allocation: the circular buffer.
    fn new() -> Self {
        Channel {
            buf: vec![0.0; BUF as usize],
            pos: 0,
            h: [Head::default(); NV],
            lp: Biquad::default(),
            env_fast: 0.0,
            env_slow: 0.0,
            swell_g: 1.0,
            onset_hold: 0,
            trk: Biquad::default(),
            env: 0.0,
            last_f: 0.0,
            hi: false,
            sq1: false,
            sq2: false,
            sq1_prev: false,
            sub1: Biquad::default(),
            sub2: Biquad::default(),
            gate: 0.0,
        }
    }

    /// `c = Channel {}`, in place: the buffer re-filled with zeros (no allocation), every other
    /// field back to its default.
    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
        self.h = [Head::default(); NV];
        self.lp = Biquad::default();
        self.env_fast = 0.0;
        self.env_slow = 0.0;
        self.swell_g = 1.0;
        self.onset_hold = 0;
        self.trk = Biquad::default();
        self.env = 0.0;
        self.last_f = 0.0;
        self.hi = false;
        self.sq1 = false;
        self.sq2 = false;
        self.sq1_prev = false;
        self.sub1 = Biquad::default();
        self.sub2 = Biquad::default();
        self.gate = 0.0;
    }
}

// ------------------------------------------------------------------ helpers
fn filter(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

/// Coefficients only -- keeps the filter's running state.
fn set_lp(q: &mut Biquad, fc: f64, fsr: f64, qv: f64) {
    let k = (PI * fc / fsr).tan();
    let a0 = 1.0 + k / qv + k * k;
    q.b0 = ((k * k) / a0) as f32;
    q.b1 = ((2.0 * k * k) / a0) as f32;
    q.b2 = q.b0;
    q.a1 = ((2.0 * (k * k - 1.0)) / a0) as f32;
    q.a2 = ((1.0 - k / qv + k * k) / a0) as f32;
}

fn one_pole_coef(fc: f64, fsr: f64) -> f32 {
    (-2.0 * PI * fc / fsr).exp() as f32
}

/// Reads one sample at an integer delay.
///
/// The fold is a MODULO, not a single add (the C++ comment): the correlation search asks for
/// `oldDelay - j * segStep`, which goes negative by up to MATCH*segStep = 256. Rust's `%` on
/// `i32` truncates toward zero exactly as the C++ `%`.
fn buf_at(buf: &[f32], pos: i32, delay_samples: i32) -> f32 {
    let mut i = pos.wrapping_sub(delay_samples);
    i %= BUF;
    if i < 0 {
        i += BUF;
    }
    buf.get(i as usize).copied().unwrap_or(0.0)
}

fn buf_read(buf: &[f32], pos: i32, delay_samples: f32) -> f32 {
    let d = clamp_t(delay_samples, 1.0, (BUF - 4) as f32);
    let di = d as i32;
    let fr = d - di as f32;

    let mut i1 = pos.wrapping_sub(di);
    if i1 < 0 {
        i1 += BUF;
    }
    let mut i2 = i1 - 1;
    if i2 < 0 {
        i2 += BUF;
    }

    let at = |i: i32| buf.get(i as usize).copied().unwrap_or(0.0);
    at(i1) * (1.0 - fr) + at(i2) * fr
}

/// OC-2: flip-flop octave divider.
///
/// Tracking filter pulls out the fundamental, a comparator with hysteresis squares it, two
/// flip-flops divide by 2 and 4, and each square ring-modulates the fundamental (that is the
/// OC-2 trick: the sub keeps the note's own envelope). Gated so it does not chatter between
/// notes. Returns `(outDown1, outDown2)`.
fn divider(c: &mut Channel, x: f32) -> (f32, f32) {
    let f = filter(&mut c.trk, x);
    let a = f.abs();

    if a > c.env {
        c.env += (a - c.env) * 0.01;
    } else {
        c.env += (a - c.env) * 0.0005;
    }

    let hyst = 0.15 * c.env + 0.0005;

    if !c.hi && f > hyst {
        c.hi = true;
        c.sq1 = !c.sq1;
        if c.sq1 {
            c.sq2 = !c.sq2;
        }
    } else if c.hi && f < -hyst {
        c.hi = false;
    }

    let g1: f32 = if c.sq1 { 1.0 } else { -1.0 };
    let g2: f32 = if c.sq2 { 1.0 } else { -1.0 };

    let gate_t: f32 = if c.env > 0.004 { 1.0 } else { 0.0 };
    c.gate += (gate_t - c.gate) * 0.003;

    // -1: the ring-modulated fundamental (keeps the note's own shape). -2: half that plus half a
    // plain envelope-shaped square, so the sub's fundamental stays above its harmonics.
    let out_down1 = filter(&mut c.sub1, f * g1) * 1.05 * c.gate;
    let out_down2 = filter(&mut c.sub2, 0.5 * f * g2 + 0.9 * c.env * g2) * 1.05 * c.gate;
    (out_down1, out_down2)
}

/// Everything the engine keeps but the channels: the parameter state and the smoothed and
/// derived values, as the C++ members (same names, same defaults).
#[derive(Debug, Clone)]
struct State {
    // ---------------------------------------------------------------- parameter state
    style_idx: i32,
    dry_knob: f32,
    sub_knob: f32,
    dn_knob: f32,
    up_knob: f32,
    up2_knob: f32,
    fifth_knob: f32,
    fifth_idx: i32,
    det_knob: f32,
    atk_knob: f32,
    lpf_knob: f32,
    reso_knob: f32,
    level_knob: f32,
    bypassed: bool,
    trim_db: f32,
    trim_lin: f32,
    trim_sm: f32,
    voice_knob: f32,
    interval_idx: i32,
    pedal_knob: f32,
    glide_knob: f32,
    width_knob: f32,
    grain_knob: f32,
    track_knob: f32,

    // ---------------------------------------------------------------- smoothed / derived
    dry_sm: f32,
    level_sm: f32,
    width_sm: f32,
    byp_sm: f32,
    v_gain: [f32; NV],
    v_gain_sm: [f32; NV],
    pan_l: [f32; NV],
    pan_r: [f32; NV],

    fs: f32,
    ctrl_counter: i32,
    lamp_counter: i32,
    last_lpf: f32,
    last_reso: f32,
    last_track: f32,
    last_grain: f32,
    last_style_for_grain: i32,
    ratio: [f32; NV],
    win: [f32; NV],
    min_d: f32,
    xf_len: f32,
    base_win: f32,
    lag_step: i32,
    seg_step: i32,
    span: f32,
    shifting: bool,
    wham_semi: f32,
    wham_target: f32,
    glide_coef: f32,
    atk_inc: f32,
    onset_hold_len: f32,
}

impl Default for State {
    fn default() -> Self {
        State {
            style_idx: 0,
            dry_knob: 0.7071,
            sub_knob: 0.0,
            dn_knob: 0.6,
            up_knob: 0.6,
            up2_knob: 0.0,
            fifth_knob: 0.0,
            fifth_idx: 1,
            det_knob: 0.25,
            atk_knob: 0.0,
            lpf_knob: 1.0,
            reso_knob: 0.0,
            level_knob: 0.7071,
            bypassed: false,
            trim_db: 0.0,
            trim_lin: 1.0,
            trim_sm: 1.0,
            voice_knob: 0.7,
            interval_idx: 1,
            pedal_knob: 0.0,
            glide_knob: 0.2,
            width_knob: 0.35,
            grain_knob: 0.5,
            track_knob: 0.5,
            dry_sm: 1.0,
            level_sm: 1.0,
            width_sm: 0.35,
            byp_sm: 0.0,
            v_gain: [0.0; NV],
            v_gain_sm: [0.0; NV],
            pan_l: [0.0; NV],
            pan_r: [0.0; NV],
            fs: 48000.0,
            ctrl_counter: 0,
            lamp_counter: 0,
            last_lpf: -1.0,
            last_reso: -1.0,
            last_track: -1.0,
            last_grain: -1.0,
            last_style_for_grain: -1,
            ratio: [0.0; NV],
            win: [0.0; NV],
            min_d: 200.0,
            xf_len: 300.0,
            base_win: 1440.0,
            lag_step: 2,
            seg_step: 4,
            span: 256.0,
            shifting: true,
            wham_semi: 0.0,
            wham_target: 0.0,
            glide_coef: 0.0,
            atk_inc: 0.0,
            onset_hold_len: 2400.0,
        }
    }
}

impl State {
    // -------------------------------------------------------------- style / ratio tables
    fn apply_ratios(&mut self) {
        let cents = knob_to_cents(self.det_knob);

        self.ratio[0] = 0.25;
        self.ratio[1] = 0.5;
        self.ratio[2] = semi_ratio(7.0);
        if self.fifth_idx == 1 {
            self.ratio[2] = semi_ratio(19.0);
        }
        if self.fifth_idx == 2 {
            self.ratio[2] = semi_ratio(-5.0);
        }
        self.ratio[3] = 2.0;
        if self.style_idx == STYLE_POG {
            self.ratio[3] = 2.0 * semi_ratio(cents * 0.4 / 100.0); // POG: Detune pulls the octave sharp
        }
        self.ratio[4] = 4.0;
        self.ratio[5] = semi_ratio(-cents / 100.0);
        self.ratio[6] = semi_ratio(cents / 100.0);
        // ratio[7] (whammy) is slewed in the main loop
    }

    fn reset_head(&self, h: &mut Head, v: usize) {
        h.d0 = self.min_d;
        if self.ratio[v] > 1.0 {
            h.d0 = self.win[v];
        }
        h.d1 = h.d0;
        h.active = 0;
        h.xf = 1.0;
        h.fading = false;
        h.live = false;
    }

    // -------------------------------------------------------------- the shifter core
    /// The correlation score of one candidate start point against the grain that is ending.
    fn match_score(&self, buf: &[f32], pos: i32, old_delay: i32, cand: i32) -> f32 {
        let mut corr = 0.0f32;
        let mut energy = 1.0e-9f32;

        for j in 0..MATCH {
            let a = buf_at(buf, pos, old_delay.wrapping_sub(j * self.seg_step));
            let b = buf_at(buf, pos, cand.wrapping_sub(j * self.seg_step));
            corr += a * b;
            energy += b * b;
        }

        corr / energy.sqrt()
    }

    /// Where should the next grain start so its waveform lines up with the one that's ending?
    /// Slides a MATCH-point window over the candidates and keeps the best normalised match.
    fn best_start(&self, buf: &[f32], pos: i32, old_delay: i32, first_candidate: i32, direction: i32, limit: i32) -> i32 {
        let mut best = first_candidate;
        let mut best_score = -1.0e9f32;

        let mut n = limit.wrapping_sub(first_candidate).wrapping_mul(direction) / self.lag_step;
        if n < 1 {
            n = 1;
        }
        if n > 1200 {
            n = 1200;
        }

        for lag in 0..n {
            let mut cand = first_candidate.wrapping_add(direction * lag * self.lag_step);
            cand = clamp_t(cand, 1, BUF - 8);

            let score = self.match_score(buf, pos, old_delay, cand);
            if score > best_score {
                best_score = score;
                best = cand;
            }
        }

        // fine pass: the coarse search steps by lagStep, so try the neighbours at single-sample
        // resolution
        let mut fine = best;

        for k in -self.lag_step..=self.lag_step {
            let cand = best.wrapping_add(k);
            let in_range = cand.wrapping_sub(first_candidate).wrapping_mul(direction) >= 0 && limit.wrapping_sub(cand).wrapping_mul(direction) >= 0;

            if k != 0 && cand > 0 && cand < BUF - 8 && in_range {
                let score = self.match_score(buf, pos, old_delay, cand);
                if score > best_score {
                    best_score = score;
                    fine = cand;
                }
            }
        }

        fine
    }

    /// Splice-aligned two-head pitch shifter, one voice.
    fn shift_voice(&self, buf: &[f32], pos: i32, h: &mut Head, r: f32, w: f32) -> f32 {
        let rate = 1.0 - r; // how fast a head's delay changes, samples per sample

        let mut d_act = h.d0;
        if h.active == 1 {
            d_act = h.d1;
        }

        if !h.fading {
            let mut splice = false;
            let (mut first, mut dir, mut limit) = (0i32, 1i32, 0i32);

            if r > 1.0 {
                // shifting up: delay shrinks. Jump back to the far end of the window before the
                // matcher's floor.
                let floor_d = self.xf_len * (r - 1.0) + self.span + 8.0;
                if d_act <= floor_d {
                    splice = true;
                    first = w as i32;
                    dir = -1;
                    limit = (floor_d + 0.5 * (w - floor_d)) as i32;
                }
            } else if r < 1.0 {
                // shifting down: delay grows. Jump forward to the near end before running off the
                // window.
                if d_act >= w {
                    splice = true;
                    first = self.min_d as i32;
                    dir = 1;
                    limit = (w - self.xf_len * (1.0 - r) - 8.0 - 0.5 * (w - self.min_d)) as i32;
                }
            }

            if splice {
                let start = self.best_start(buf, pos, d_act as i32, first, dir, limit) as f32;
                if h.active == 0 {
                    h.d1 = start;
                    h.active = 1;
                } else {
                    h.d0 = start;
                    h.active = 0;
                }
                h.xf = 0.0;
                h.fading = true;
            }
        }

        // (`y` is declared `0.0f` in the C++ and assigned on every path below.)
        if h.fading {
            let (mut cur, mut old) = (h.d0, h.d1);
            if h.active == 1 {
                cur = h.d1;
                old = h.d0;
            }

            // matched waveforms -> a linear fade keeps the level
            let y = buf_read(buf, pos, cur) * h.xf + buf_read(buf, pos, old) * (1.0 - h.xf);
            h.xf += 1.0 / self.xf_len;
            if h.xf >= 1.0 {
                h.xf = 1.0;
                h.fading = false;
            }
            h.d0 += rate;
            h.d1 += rate;
            y
        } else if h.active == 0 {
            let y = buf_read(buf, pos, h.d0);
            h.d0 += rate;
            y
        } else {
            let y = buf_read(buf, pos, h.d1);
            h.d1 += rate;
            y
        }
    }

    // -------------------------------------------------------------- per-channel engine
    /// One channel, one sample: `(wetL, wetR)`.
    fn engine(&self, c: &mut Channel, x: f32) -> (f32, f32) {
        let xin = x * self.trim_sm;
        if let Some(s) = c.buf.get_mut(c.pos as usize) {
            *s = xin;
        }

        // onset detector for the swell (Attack)
        let a = xin.abs();
        if a > c.env_fast {
            c.env_fast += (a - c.env_fast) * 0.05;
        } else {
            c.env_fast += (a - c.env_fast) * 0.002;
        }
        if a > c.env_slow {
            c.env_slow += (a - c.env_slow) * 0.002;
        } else {
            c.env_slow += (a - c.env_slow) * 0.0005;
        }
        if c.onset_hold > 0 {
            c.onset_hold -= 1;
        }

        if self.atk_inc > 0.0 {
            if c.onset_hold == 0 && c.env_fast > 0.004 && c.env_fast > 2.0 * c.env_slow {
                c.swell_g = 0.0;
                c.onset_hold = self.onset_hold_len as i32;
            }

            c.swell_g += self.atk_inc;
            if c.swell_g > 1.0 {
                c.swell_g = 1.0;
            }
        } else {
            c.swell_g = 1.0;
        }

        let (mut wl, mut wr) = (0.0f32, 0.0f32);

        if self.shifting {
            for v in 0..NV {
                if self.v_gain_sm[v] > 0.0001 {
                    if !c.h[v].live {
                        self.reset_head(&mut c.h[v], v);
                        c.h[v].live = true;
                    }

                    let y = self.shift_voice(&c.buf, c.pos, &mut c.h[v], self.ratio[v], self.win[v]) * self.v_gain_sm[v];
                    wl += y * self.pan_l[v];
                    wr += y * self.pan_r[v];
                } else {
                    c.h[v].live = false;
                }
            }
        } else {
            let (d1, d2) = divider(c, xin);
            let y = d2 * self.v_gain_sm[0] + d1 * self.v_gain_sm[1];
            wl += y;
            wr += y;
        }

        c.pos += 1;
        if c.pos >= BUF {
            c.pos = 0;
        }

        (wl * c.swell_g, wr * c.swell_g)
    }
}

/// The engine: `PitchEngineDSP`.
pub struct PitchEngineDsp {
    s: State,
    ch: [Channel; 2],
    /// `trackLamp`: 1 while the OC-2 divider is locked on a note.
    track_lamp: f32,
}

impl Default for PitchEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl PitchEngineDsp {
    /// `PitchEngineDSP() { prepare (48000.0); }`. Allocates the two channel buffers.
    pub fn new() -> Self {
        let mut e = PitchEngineDsp { s: State::default(), ch: [Channel::new(), Channel::new()], track_lamp: 0.0 };
        e.prepare(48000.0);
        e
    }

    /// The OC-2 divider's lock lamp, as the editor polls it.
    pub fn track_lamp(&self) -> f32 {
        self.track_lamp
    }

    /// Back to exactly what [`PitchEngineDsp::new`] makes, in place: every scalar to its C++
    /// default (plain data, no allocation), the lamp off, then the constructor's
    /// `prepare (48000.0)`, which re-fills the buffers. A Rust addition for the host's reset (the
    /// C++ has none); the caller then prepares at its rate and pushes every parameter, as for a
    /// freshly made module.
    pub fn restart(&mut self) {
        self.s = State::default();
        self.track_lamp = 0.0;
        self.prepare(48000.0);
    }

    /// `prepare (sampleRate)` on the live engine, for the host's transport reset (Play / Stop /
    /// Locate / bounce start). The C++ `prepare` already rebuilds from the engine's own knobs
    /// (audio cleared, Trim / Dry / Level / Width / bypass seated at their targets, every cache
    /// re-derived); two Rust additions make the result independent of what played before and free
    /// of glides: the voice gains are seated at their targets (the C++ fades them in from 0) and
    /// the Whammy slew at its target (the C++ keeps the old `whamSemi`), and the control and
    /// lamp counters and the lamp start from 0. Plain data, no allocation.
    pub fn reset_live(&mut self, sample_rate: f64) {
        self.prepare(sample_rate);
        let s = &mut self.s;
        s.v_gain_sm = s.v_gain;
        s.wham_semi = s.wham_target;
        s.ctrl_counter = 0;
        s.lamp_counter = 0;
        self.track_lamp = 0.0;
    }

    /// The Whammy voice's current (slewed) interval in semitones, `whamSemi`.
    pub fn whammy_semis(&self) -> f32 {
        self.s.wham_semi
    }

    /// The sample rate `prepare` settled on.
    pub fn sample_rate(&self) -> f64 {
        f64::from(self.s.fs)
    }

    // -------------------------------------------------------------- events
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        let s = &mut self.s;
        match index {
            P_STYLE => {
                s.style_idx = clamp_t((v + 0.5) as i32, 0, NUM_STYLES - 1);
                self.apply_style();
            }
            P_DRY => s.dry_knob = v,
            P_SUB => s.sub_knob = v,
            P_OCT_DOWN => s.dn_knob = v,
            P_OCT_UP => s.up_knob = v,
            P_OCT2_UP => s.up2_knob = v,
            P_FIFTH => s.fifth_knob = v,
            P_FIFTH_MODE => {
                s.fifth_idx = clamp_t((v + 0.5) as i32, 0, 2);
                s.apply_ratios();
            }
            P_DETUNE => {
                s.det_knob = v;
                s.apply_ratios();
            }
            P_ATTACK => s.atk_knob = v,
            P_LPF => s.lpf_knob = v,
            P_RESO => s.reso_knob = v,
            P_LEVEL => s.level_knob = v,
            P_BYPASS => s.bypassed = v > 0.5,
            P_TRIM => {
                s.trim_db = knob_to_trim_db(v);
                s.trim_lin = 10.0f32.powf(s.trim_db / 20.0);
            }
            P_VOICE => s.voice_knob = v,
            P_INTERVAL => s.interval_idx = clamp_t((v + 0.5) as i32, 0, 9),
            P_PEDAL => s.pedal_knob = v,
            P_GLIDE => s.glide_knob = v,
            P_WIDTH => s.width_knob = v,
            P_GRAIN => s.grain_knob = v,
            P_TRACK => s.track_knob = v,
            _ => {}
        }
    }

    // -------------------------------------------------------------- init
    pub fn prepare(&mut self, sample_rate: f64) {
        let s = &mut self.s;
        s.fs = (if sample_rate > 0.0 { sample_rate } else { 48000.0 }) as f32;

        for c in &mut self.ch {
            c.clear();
            c.pos = 0;
            c.swell_g = 1.0;
            c.gate = 0.0;
            set_lp(&mut c.sub1, 500.0, f64::from(s.fs), 0.7071);
            set_lp(&mut c.sub2, 250.0, f64::from(s.fs), 0.7071);
            set_lp(&mut c.trk, 632.0, f64::from(s.fs), 0.9);
            set_lp(&mut c.lp, 20000.0, f64::from(s.fs), 0.7071);
        }

        for v in 0..NV {
            s.v_gain_sm[v] = 0.0;
            s.pan_l[v] = 1.0;
            s.pan_r[v] = 1.0;
        }

        s.ratio[7] = 1.0;
        s.width_sm = s.width_knob;
        s.onset_hold_len = 0.05 * s.fs;
        s.trim_lin = 10.0f32.powf(s.trim_db / 20.0);
        s.trim_sm = s.trim_lin;
        s.dry_sm = knob_to_gain(s.dry_knob);
        s.level_sm = knob_to_gain(s.level_knob);
        s.byp_sm = if s.bypassed { 1.0 } else { 0.0 };

        // force a re-derive of everything that caches
        s.last_lpf = -1.0;
        s.last_reso = -1.0;
        s.last_track = -1.0;

        self.apply_style();
        self.update_control();
    }

    // -------------------------------------------------------------- main
    /// `process (channels, numChannels, numSamples)`: `num_channels` of `io` (1 or 2 are used;
    /// fewer than one is a no-op, as the C++), `num_samples` frames each.
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        if num_channels < 1 {
            return;
        }
        let (left, rest) = match io.split_first_mut() {
            Some(x) => x,
            None => return,
        };
        let mut right: Option<&mut Vec<f32>> = if num_channels > 1 { rest.first_mut() } else { None };
        // Never past the end of a buffer the host handed over (the C++ trusts numSamples).
        let mut n = num_samples.min(left.len());
        if let Some(r) = right.as_deref() {
            n = n.min(r.len());
        }

        for i in 0..n {
            let x0 = left[i];
            let x1 = match right.as_deref() {
                Some(r) => r[i],
                None => x0,
            };

            let s = &mut self.s;
            let k = 0.005f32;
            s.trim_sm += (s.trim_lin - s.trim_sm) * k;
            s.dry_sm += (knob_to_gain(s.dry_knob) - s.dry_sm) * k;
            s.level_sm += (knob_to_gain(s.level_knob) - s.level_sm) * k;
            s.width_sm += (s.width_knob - s.width_sm) * k;

            for v in 0..NV {
                s.v_gain_sm[v] += (s.v_gain[v] - s.v_gain_sm[v]) * 0.002;
            }

            let byp_t: f32 = if s.bypassed { 1.0 } else { 0.0 };
            s.byp_sm += (byp_t - s.byp_sm) * 0.002;

            // whammy voice: slew the interval in semitones, then convert
            s.wham_semi += (s.wham_target - s.wham_semi) * s.glide_coef;
            s.ratio[7] = semi_ratio(s.wham_semi);

            if s.ratio[7] > 1.0 {
                s.win[7] = cmin(cmax(s.base_win, s.xf_len * (s.ratio[7] - 1.0) + s.span + 8.0 + 0.4 * s.base_win), (BUF - 64) as f32);
            } else {
                s.win[7] = cmin(cmax(s.base_win, s.min_d + s.span + 8.0 + 0.4 * s.base_win), (BUF - 64) as f32);
            }

            s.ctrl_counter += 1;
            if s.ctrl_counter >= 32 {
                s.ctrl_counter = 0;
                self.update_control();

                self.s.lamp_counter += 1;
                if self.s.lamp_counter >= 50 {
                    // ~30 Hz
                    self.s.lamp_counter = 0;
                    self.track_lamp = if !self.s.shifting && self.ch[0].gate > 0.5 { 1.0 } else { 0.0 };
                }
            }

            // the shifted voices, both channels (true stereo)
            let [c0, c1] = &mut self.ch;
            let (e_l0, e_l1) = self.s.engine(c0, x0);
            let (e_r0, e_r1) = self.s.engine(c1, x1);

            let s = &self.s;
            // stereo sources stay on their own side; the pan bleeds across as Width comes up
            let ws = s.width_sm;
            let mut wet_l = e_l0 * (1.0 - 0.5 * ws) + e_r0 * 0.5 * ws;
            let mut wet_r = e_r1 * (1.0 - 0.5 * ws) + e_l1 * 0.5 * ws;

            if s.lpf_knob < 0.995 {
                wet_l = filter(&mut c0.lp, wet_l);
                wet_r = filter(&mut c1.lp, wet_r);
            }

            let inv = s.level_sm / cmax(s.trim_sm, 0.01);
            wet_l *= inv;
            wet_r *= inv;

            let y0 = x0 * s.dry_sm + wet_l;
            let y1 = x1 * s.dry_sm + wet_r;

            left[i] = x0 * s.byp_sm + y0 * (1.0 - s.byp_sm);
            if let Some(r) = right.as_deref_mut() {
                r[i] = x1 * s.byp_sm + y1 * (1.0 - s.byp_sm);
            }
        }
    }

    // -------------------------------------------------------------- style
    fn apply_style(&mut self) {
        let s = &mut self.s;
        s.shifting = s.style_idx != STYLE_OC2;

        let (mut win_ms, mut min_ms, mut xf_ms) = (30.0f32, 5.0f32, 6.0f32);
        s.lag_step = 2;
        s.seg_step = 4;

        if s.style_idx == STYLE_MICRO_POG {
            win_ms = 12.0;
            min_ms = 2.0;
            xf_ms = 3.0;
            s.seg_step = 2;
        } else if s.style_idx == STYLE_H910 {
            win_ms = 40.0;
            min_ms = 8.0;
            xf_ms = 10.0;
        } else if s.style_idx == STYLE_WHAMMY {
            win_ms = 24.0;
            min_ms = 4.0;
            xf_ms = 5.0;
        }

        let g = knob_to_grain(s.grain_knob);
        s.base_win = win_ms * 0.001 * s.fs * g;
        s.min_d = cmax(min_ms * 0.001 * s.fs, 8.0);
        s.xf_len = cmax(xf_ms * 0.001 * s.fs * g, 16.0);
        s.span = (MATCH * s.seg_step) as f32;
        s.last_grain = s.grain_knob;
        s.last_style_for_grain = s.style_idx;

        s.apply_ratios();

        // every voice gets a window big enough for its own splice floor; clamp to the buffer
        for v in 0..NV {
            let mut floor_d = s.min_d + s.span + 8.0;
            if s.ratio[v] > 1.0 {
                floor_d = s.xf_len * (s.ratio[v] - 1.0) + s.span + 8.0;
            }
            s.win[v] = cmin(cmax(s.base_win, floor_d + 0.4 * s.base_win), (BUF - 64) as f32);
        }

        if s.ratio[7] < 0.1 {
            s.ratio[7] = 1.0;
        }

        // re-seat every head so a style change never reads garbage
        let s = &self.s;
        for c in &mut self.ch {
            for v in 0..NV {
                s.reset_head(&mut c.h[v], v);
            }
        }
    }

    // -------------------------------------------------------------- control rate
    fn update_control(&mut self) {
        let s = &mut self.s;
        // per-voice gain targets. Muted voices are skipped entirely in the audio loop.
        for v in 0..NV {
            s.v_gain[v] = 0.0;
        }

        if s.style_idx == STYLE_POG {
            s.v_gain[0] = s.sub_knob;
            s.v_gain[1] = s.dn_knob;
            s.v_gain[2] = s.fifth_knob;
            s.v_gain[3] = s.up_knob;
            s.v_gain[4] = s.up2_knob;
        } else if s.style_idx == STYLE_MICRO_POG {
            s.v_gain[0] = s.sub_knob;
            s.v_gain[3] = s.up_knob;
        } else if s.style_idx == STYLE_OC2 {
            s.v_gain[0] = s.sub_knob;
            s.v_gain[1] = s.dn_knob;
        } else if s.style_idx == STYLE_H910 {
            s.v_gain[5] = s.voice_knob;
            s.v_gain[6] = s.voice_knob;
        } else {
            s.v_gain[7] = s.voice_knob;
        }

        for v in 0..NV {
            s.v_gain[v] = knob_to_gain(s.v_gain[v]);
        }

        // stereo placement: how far each voice sits from centre at Width = 1
        let w = s.width_sm;

        for v in 0..NV {
            let mut p = 0.0f32;
            if v == 2 {
                p = 0.5;
            }
            if v == 3 {
                p = -0.4;
            }
            if v == 4 {
                p = 0.7;
            }
            if v == 5 {
                p = -1.0;
            }
            if v == 6 {
                p = 1.0;
            }
            p *= w;

            let ang = (p + 1.0) * 0.25 * PI as f32;
            s.pan_l[v] = ang.cos() * 1.4142;
            s.pan_r[v] = ang.sin() * 1.4142;
        }

        // grain / style window
        if (s.grain_knob - s.last_grain).abs() > 0.001 || s.style_idx != s.last_style_for_grain {
            self.apply_style();
        }

        let s = &mut self.s;
        // wet filter
        if (s.lpf_knob - s.last_lpf).abs() > 0.0005 || (s.reso_knob - s.last_reso).abs() > 0.0005 {
            s.last_lpf = s.lpf_knob;
            s.last_reso = s.reso_knob;
            let hz = knob_to_filter_hz(s.lpf_knob);
            let q = knob_to_q(s.reso_knob);
            for c in &mut self.ch {
                set_lp(&mut c.lp, f64::from(cmin(hz, 0.45 * s.fs)), f64::from(s.fs), f64::from(q));
            }
        }

        // OC-2 tracking filter
        if (s.track_knob - s.last_track).abs() > 0.001 {
            s.last_track = s.track_knob;
            let hz = knob_to_track_hz(s.track_knob);
            for c in &mut self.ch {
                set_lp(&mut c.trk, f64::from(hz), f64::from(s.fs), 0.9);
            }
        }

        // whammy
        s.wham_target = interval_semis(s.interval_idx) * s.pedal_knob;
        let glide_ms = knob_to_glide_ms(s.glide_knob);
        s.glide_coef = 1.0 - one_pole_coef(f64::from(1000.0 / cmax(glide_ms, 1.0)), f64::from(s.fs));
        if s.glide_knob < 0.01 {
            s.glide_coef = 1.0;
        }

        // swell
        s.atk_inc = 0.0;
        if s.atk_knob > 0.01 {
            s.atk_inc = 1.0 / ((0.02 + 1.48 * s.atk_knob * s.atk_knob) * s.fs);
        }
    }

    /// The voices' smoothed gains and their targets (tests).
    #[cfg(test)]
    pub(crate) fn voice_gains_smoothed(&self) -> [f32; NV] {
        self.s.v_gain_sm
    }
    #[cfg(test)]
    pub(crate) fn voice_gains_target(&self) -> [f32; NV] {
        self.s.v_gain
    }

    /// Addresses and lengths of every heap block the engine owns (the two channel buffers): a
    /// test sees that nothing on the audio path re-allocates them.
    #[cfg(test)]
    pub(crate) fn heap_blocks(&self) -> [(usize, usize); 2] {
        [(self.ch[0].buf.as_ptr() as usize, self.ch[0].buf.capacity()), (self.ch[1].buf.as_ptr() as usize, self.ch[1].buf.capacity())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buf_at_folds_like_the_cpp_modulo() {
        let mut buf = vec![0.0f32; BUF as usize];
        for (i, s) in buf.iter_mut().enumerate() {
            *s = i as f32;
        }
        // pos - delay, folded by C++'s truncating `%` then one add.
        assert_eq!(buf_at(&buf, 10, 5), 5.0);
        assert_eq!(buf_at(&buf, 10, 11), (BUF - 1) as f32);
        assert_eq!(buf_at(&buf, 10, -256), 266.0);
        assert_eq!(buf_at(&buf, BUF - 1, -256), 255.0);
        assert_eq!(buf_at(&buf, 0, BUF + 3), (BUF - 3) as f32);
        assert_eq!(buf_at(&buf, 0, 3 * BUF + 1), (BUF - 1) as f32);
    }

    #[test]
    fn buf_read_interpolates_between_two_taps() {
        let mut buf = vec![0.0f32; BUF as usize];
        for (i, s) in buf.iter_mut().enumerate() {
            *s = i as f32;
        }
        // d = 2.25 at pos 100: taps 98 and 97, 3/4 and 1/4.
        assert!((buf_read(&buf, 100, 2.25) - 97.75).abs() < 1e-4);
        // clamped to 1 .. BUF - 4; wraps below zero
        assert_eq!(buf_read(&buf, 100, 0.0), 99.0);
        assert_eq!(buf_read(&buf, 0, 1.0), (BUF - 1) as f32);
        let _ = buf_read(&buf, 5, f32::NAN); // no panic (the C++ is undefined here; unreachable)
    }

    #[test]
    fn maps_match_the_header() {
        assert_eq!(knob_to_trim_db(0.0), -12.0);
        assert_eq!(knob_to_trim_db(1.0), 24.0);
        assert_eq!(knob_to_trim_db(2.0), 24.0);
        assert!((knob_to_filter_hz(1.0) - 20_000.0).abs() < 1.0);
        assert_eq!(knob_to_filter_hz(0.0), 300.0);
        assert_eq!(knob_to_q(1.0), 6.0);
        assert_eq!(knob_to_glide_ms(1.0), 500.0);
        assert_eq!(knob_to_attack_ms(0.01), 0.0);
        assert!((knob_to_attack_ms(1.0) - 1500.0).abs() < 1e-3);
        assert_eq!(knob_to_cents(1.0), 50.0);
        assert!((knob_to_track_hz(1.0) - 2000.0).abs() < 1e-3);
        assert_eq!(knob_to_grain(0.5), 1.0);
        assert!((knob_to_gain(0.7071) - 1.0).abs() < 1e-4);
        assert_eq!(latency_ms(STYLE_OC2, 1.0), 0.0);
        assert_eq!(latency_ms(STYLE_POG, 0.5), 15.0);
        assert_eq!(latency_ms(STYLE_MICRO_POG, 0.5), 6.0);
        assert_eq!(interval_semis(-3), 24.0);
        assert_eq!(interval_semis(99), -24.0);
        assert_eq!(interval_semis(5), -2.0);
    }

    #[test]
    fn apply_style_windows_at_48k() {
        let mut e = PitchEngineDsp::new();
        // POG, grain 1x: 30 ms window = 1440, min 240, xf 288, span 256.
        assert!((e.s.base_win - 1440.0).abs() < 1e-3);
        assert!((e.s.min_d - 240.0).abs() < 1e-3);
        assert!((e.s.xf_len - 288.0).abs() < 1e-3);
        assert_eq!(e.s.span, 256.0);
        assert!(e.s.shifting);
        e.set_parameter(P_STYLE, 2.0);
        assert!(!e.s.shifting, "OC-2 does not shift");
        e.set_parameter(P_STYLE, 1.0);
        assert_eq!(e.s.span, 128.0, "Micro POG: segStep 2");
        // every window inside the buffer
        for w in e.s.win {
            assert!(w > 0.0 && w <= (BUF - 64) as f32);
        }
    }
}
