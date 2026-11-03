//! CLEAN Q -- Pro-Q-style dynamic parametric EQ.
//! Line-by-line translation of `CleanQ/Source/CleanQDSP.h` (itself ported from CleanQ.cmajor).
//!
//! - 24 band slots, zero latency (IIR only, no lookahead, no FIR)
//! - 7 types: Bell, Low Shelf, High Shelf, Low Cut, High Cut, Notch, Band Pass
//! - 5 places: Stereo, Left, Right, Mid, Side
//! - 8 slopes: 6, 12, 18, 24, 36, 48, 72, 96 dB/oct (Butterworth cascade, up to 8 biquads a band)
//! - per-band dynamic EQ on Bell and the two shelves: own detector band-pass, threshold and range
//! - solo (hear one band's region), phase invert, output trim, auto-gain compensation
//!
//! The packing: Amorph allows at most 128 parameters, so the original packs five fields into each
//! band's `mode` value as a base-N integer, and threshold + range into `dyn`. This engine still
//! speaks the packed 127-parameter layout, on purpose; the plugin wrapper packs the host's
//! unpacked parameters before they get here (`pack_for_dsp` in `mod.rs`, from `Parameters.h`).
//!
//! The analyser: the C++ keeps two 4096-sample rings (`fftInPre`/`fftInPost`) and snapshots them
//! for the editor's FFT once a hop. Here the same two signals -- the mono sum of the input and of
//! the output, `(xl + xr) * 0.5` and `(yl + yr) * 0.5`, taken per sample at the same point -- are
//! pushed to the plugin's [`Analysis`] rings instead, and the UI runs the FFT. The window, the dB
//! mapping and the magnitude scale the editor uses are kept here (`fill_window`,
//! `magnitude_to_norm`, `FFT_MAGNITUDE_SCALE`).
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - Everything runs in `double` exactly where the C++ does; the knob values are `float`.
//! - `(int) (v * 1679.0f + 0.5f)`: float maths, then a truncating cast. Rust `as i32` truncates
//!   the same way for every finite value, and saturates where the C++ would be undefined; the
//!   wrapper clamps every value into its APVTS range first, so that never happens.
//! - `std::lround` is `f64::round` (both round half away from zero).
//! - `clampT` returns `v` for NaN (both comparisons false), as written.
//! - The outputs back to the UI (`autoGainDb`, `dynOffsetDb[]`) are `std::atomic<float>` in the
//!   C++; here they are plain fields the owning plugin reads after `process`.

use std::f64::consts::PI as STD_PI;
use std::sync::Arc;

use crate::analysis::Analysis;

// ------------------------------------------------------------------ shape
pub const NUM_BANDS: usize = 24;
pub const KEYS_PER_BAND: usize = 5; // mode, freq, gain, q, dyn
pub const NUM_GLOBALS: usize = 7;
pub const NUM_PARAMS: usize = NUM_GLOBALS + NUM_BANDS * KEYS_PER_BAND; // 127
pub const MAX_SECTIONS: usize = 8;
pub const FFT_SIZE: usize = 4096;
pub const NUM_BINS: usize = 2048;
pub const HOP_SIZE: usize = 1024;

// The packed layout, exactly as the Cmajor declares it.
pub const P_OUT_GAIN: usize = 0;
pub const P_BYPASS: usize = 1;
pub const P_INVERT: usize = 2;
pub const P_SOLO: usize = 3;
pub const P_AUTO_GAIN: usize = 4;
pub const P_DYN_ATTACK: usize = 5;
pub const P_DYN_RELEASE: usize = 6;
pub const P_FIRST_BAND: usize = NUM_GLOBALS;

// Per-band keys, in the order the Cmajor packs them.
pub const K_MODE: usize = 0;
pub const K_FREQ: usize = 1;
pub const K_GAIN: usize = 2;
pub const K_Q: usize = 3;
pub const K_DYN: usize = 4;

/// Packed parameter index for one band field.
pub const fn band_param(slot: usize, key: usize) -> usize {
    P_FIRST_BAND + slot * KEYS_PER_BAND + key
}

/// `3.141592653589793`, as the header spells it (identical to `std::f64::consts::PI`).
pub const PI: f64 = STD_PI;
pub const LN_TO_DB: f64 = 8.685889638065035;

// ------------------------------------------------------------------ the field tables
pub const TYPE_BELL: i32 = 0;
pub const TYPE_LOW_SHELF: i32 = 1;
pub const TYPE_HIGH_SHELF: i32 = 2;
pub const TYPE_LOW_CUT: i32 = 3;
pub const TYPE_HIGH_CUT: i32 = 4;
pub const TYPE_NOTCH: i32 = 5;
pub const TYPE_BAND_PASS: i32 = 6;
pub const NUM_TYPES: i32 = 7;

pub const PLACE_STEREO: i32 = 0;
pub const PLACE_LEFT: i32 = 1;
pub const PLACE_RIGHT: i32 = 2;
pub const PLACE_MID: i32 = 3;
pub const PLACE_SIDE: i32 = 4;
pub const NUM_PLACES: i32 = 5;

pub const ON_FREE: i32 = 0;
pub const ON_BYPASSED: i32 = 1;
pub const ON_ACTIVE: i32 = 2;
pub const NUM_ON_STATES: i32 = 3;

pub const NUM_SLOPES: i32 = 8;

/// `CleanQDSP::clampT`: `v < lo ? lo : (v > hi ? hi : v)`.
#[inline]
pub(crate) fn clamp_t<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

pub fn slope_db_per_oct(index: i32) -> i32 {
    const S: [i32; NUM_SLOPES as usize] = [6, 12, 18, 24, 36, 48, 72, 96];
    let i = if index < 0 {
        0
    } else if index >= NUM_SLOPES {
        NUM_SLOPES - 1
    } else {
        index
    };
    S[i as usize]
}

/// Slope index -> filter order, as the Cmajor's slopeToOrder().
pub fn slope_to_order(si: i32) -> i32 {
    if si <= 0 {
        return 1;
    }
    if si == 1 {
        return 2;
    }
    if si == 2 {
        return 3;
    }
    if si == 3 {
        return 4;
    }
    if si == 4 {
        return 6;
    }
    if si == 5 {
        return 8;
    }
    if si == 6 {
        return 12;
    }
    16
}

// ------------------------------------------------------------------ pack / unpack
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeFields {
    pub on: i32,
    pub type_: i32,
    pub place: i32,
    pub slope: i32,
    pub dyn_: bool,
}

impl Default for ModeFields {
    fn default() -> Self {
        ModeFields { on: ON_ACTIVE, type_: TYPE_BELL, place: PLACE_STEREO, slope: 1, dyn_: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynFields {
    pub threshold_db: f64,
    pub range_db: f64,
}

impl Default for DynFields {
    fn default() -> Self {
        DynFields { threshold_db: -24.0, range_db: 0.0 }
    }
}

pub fn pack_mode(m: &ModeFields) -> f32 {
    let on = clamp_t(m.on, 0, NUM_ON_STATES - 1);
    let type_ = clamp_t(m.type_, 0, NUM_TYPES - 1);
    let place = clamp_t(m.place, 0, NUM_PLACES - 1);
    let slope = clamp_t(m.slope, 0, NUM_SLOPES - 1);
    (on + 3 * type_ + 21 * place + 105 * slope + 840 * i32::from(m.dyn_)) as f32 / 1679.0f32
}

pub fn unpack_mode(v: f32) -> ModeFields {
    let n = clamp_t((v * 1679.0f32 + 0.5f32) as i32, 0, 1679);
    ModeFields { on: n % 3, type_: (n / 3) % 7, place: (n / 21) % 5, slope: (n / 105) % 8, dyn_: ((n / 840) % 2) == 1 }
}

pub fn pack_dyn(d: &DynFields) -> f32 {
    let thr = clamp_t(d.threshold_db, -60.0, 0.0);
    let rng = clamp_t(d.range_db, -30.0, 30.0);
    let t = ((thr + 60.0) / 0.5).round() as i32;
    let r = ((rng + 30.0) / 0.5).round() as i32;
    (t * 121 + r) as f32 / 14640.0f32
}

pub fn unpack_dyn(v: f32) -> DynFields {
    let n = clamp_t((v * 14640.0f32 + 0.5f32) as i32, 0, 14640);
    DynFields { threshold_db: f64::from(n / 121) * 0.5 - 60.0, range_db: f64::from(n % 121) * 0.5 - 30.0 }
}

// ------------------------------------------------------------------ knob -> unit maps
/// 10 Hz .. 20 kHz.
pub fn knob_to_freq(v: f32) -> f64 {
    10.0 * 2000.0f64.powf(f64::from(v))
}
/// -30 .. +30 dB.
pub fn knob_to_gain_db(v: f32) -> f64 {
    (f64::from(v) - 0.5) * 60.0
}
/// 0.025 .. 40.
pub fn knob_to_q(v: f32) -> f64 {
    0.025 * 1600.0f64.powf(f64::from(v))
}
/// 0.1 .. 500 ms.
pub fn knob_to_attack_ms(v: f32) -> f64 {
    0.1 * 5000.0f64.powf(f64::from(v))
}
/// 5 .. 5000 ms.
pub fn knob_to_release_ms(v: f32) -> f64 {
    5.0 * 1000.0f64.powf(f64::from(v))
}
/// -24 .. +24 dB.
pub fn knob_to_out_db(v: f32) -> f64 {
    (f64::from(v) - 0.5) * 48.0
}

pub fn freq_to_knob(f: f64) -> f32 {
    ((f / 10.0).ln() / 2000.0f64.ln()) as f32
}
pub fn gain_to_knob(g: f64) -> f32 {
    (g / 60.0 + 0.5) as f32
}
pub fn q_to_knob(q: f64) -> f32 {
    ((q / 0.025).ln() / 1600.0f64.ln()) as f32
}
pub fn attack_to_knob(ms: f64) -> f32 {
    ((ms / 0.1).ln() / 5000.0f64.ln()) as f32
}
pub fn release_to_knob(ms: f64) -> f32 {
    ((ms / 5.0).ln() / 1000.0f64.ln()) as f32
}
pub fn out_db_to_knob(db: f64) -> f32 {
    (db / 48.0 + 0.5) as f32
}

// ------------------------------------------------------------------ analyser constants (for the UI)
/// The Hann window the Cmajor used, so the UI's FFT produces the same picture.
pub fn fill_window(w: &mut [f32]) {
    for (n, o) in w.iter_mut().take(FFT_SIZE).enumerate() {
        *o = (0.5 - 0.5 * (2.0 * PI * n as f64 / (FFT_SIZE - 1) as f64).cos()) as f32;
    }
}

/// Raw one-sided magnitude -> the 0..1 the display works in (-96..0 dBFS).
pub fn magnitude_to_norm(mag: f32) -> f32 {
    let db = 20.0f32 * mag.max(1.0e-6f32).log10();
    clamp_t((db + 96.0f32) / 96.0f32, 0.0f32, 1.0f32)
}

/// Hann coherent gain 0.5 -> x2, then x2 again for one-sided.
pub const FFT_MAGNITUDE_SCALE: f32 = 4.0f32 / FFT_SIZE as f32;

// ------------------------------------------------------------------ types
/// Coefficients only. Kept apart from the delay line so a re-design can overwrite the
/// coefficients without clobbering the filter's state.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    c: Coeffs,
    z1l: f64,
    z2l: f64,
    z1r: f64,
    z2r: f64,
}

#[derive(Debug, Clone, Copy)]
struct Band {
    p_mode: f32,
    p_freq: f32,
    p_gain: f32,
    p_q: f32,
    p_dyn_pack: f32,

    dirty: bool,
    place: i32,
    used: bool,
    was_used: bool,
    active: bool,
    type_: i32,
    order: i32,
    last_type: i32,
    last_order: i32,

    freq_t: f64,
    gain_t: f64,
    q_t: f64,
    freq_s: f64,
    gain_s: f64,
    q_s: f64,
    last_f: f64,
    last_g: f64,
    last_q: f64,

    dyn_: bool,
    thr: f64,
    range: f64,
    att_c: f64,
    rel_c: f64,
    env: f64,
    dyn_off: f64,

    n_sec: i32,
    has_first: bool,
    sec: [Biquad; MAX_SECTIONS],
    fb0: f64,
    fb1: f64,
    fa1: f64,
    fz_l: f64,
    fz_r: f64,
    det: Biquad,
}

impl Default for Band {
    fn default() -> Self {
        Band {
            p_mode: 0.0,
            p_freq: 0.5,
            p_gain: 0.5,
            p_q: 0.5,
            p_dyn_pack: 0.665527,
            dirty: true,
            place: 0,
            used: false,
            was_used: false,
            active: false,
            type_: 0,
            order: 1,
            last_type: -1,
            last_order: -1,
            freq_t: 0.0,
            gain_t: 0.0,
            q_t: 0.0,
            freq_s: 1000.0,
            gain_s: 0.0,
            q_s: 1.0,
            last_f: 0.0,
            last_g: 0.0,
            last_q: 0.0,
            dyn_: false,
            thr: 0.0,
            range: 0.0,
            att_c: 0.0,
            rel_c: 0.0,
            env: 0.0,
            dyn_off: 0.0,
            n_sec: 1,
            has_first: false,
            sec: [Biquad::default(); MAX_SECTIONS],
            fb0: 0.0,
            fb1: 0.0,
            fa1: 0.0,
            fz_l: 0.0,
            fz_r: 0.0,
            det: Biquad::default(),
        }
    }
}

/// One band's filter, designed from plain values. Both the audio path and the curve the editor
/// draws go through here, so the picture cannot drift away from the sound. `order` comes from
/// `slope_to_order()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Design {
    pub sec: [Coeffs; MAX_SECTIONS],
    pub n_sec: i32,
    pub has_first: bool,
    pub fb0: f64,
    pub fb1: f64,
    pub fa1: f64,
}

impl Default for Design {
    fn default() -> Self {
        Design { sec: [Coeffs::default(); MAX_SECTIONS], n_sec: 1, has_first: false, fb0: 0.0, fb1: 0.0, fa1: 0.0 }
    }
}

// ------------------------------------------------------------------ coefficients
fn set_bq(s: &mut Coeffs, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
    s.b0 = b0 / a0;
    s.b1 = b1 / a0;
    s.b2 = b2 / a0;
    s.a1 = a1 / a0;
    s.a2 = a2 / a0;
}

fn clear_bq(s: &mut Biquad) {
    s.z1l = 0.0;
    s.z2l = 0.0;
    s.z1r = 0.0;
    s.z2r = 0.0;
}

fn clear_band(b: &mut Band) {
    for s in &mut b.sec {
        clear_bq(s);
    }
    clear_bq(&mut b.det);
    b.fz_l = 0.0;
    b.fz_r = 0.0;
}

pub fn design_filter(type_: i32, order: i32, freq_hz: f64, gain_db: f64, q: f64, fs: f64) -> Design {
    let mut d = Design::default();

    let f = freq_hz.min(fs * 0.49);
    let w0 = 2.0 * PI * f / fs;
    let c = w0.cos();
    let sn = w0.sin();

    if type_ == TYPE_BELL {
        let a = 10.0f64.powf(gain_db / 40.0);
        let alpha = sn / (2.0 * q);
        set_bq(&mut d.sec[0], 1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a);
    } else if type_ == TYPE_LOW_SHELF || type_ == TYPE_HIGH_SHELF {
        let a = 10.0f64.powf(gain_db / 40.0);
        let qq = clamp_t(q, 0.1, 4.0);
        let alpha = sn / (2.0 * qq);
        let s_a = 2.0 * a.sqrt() * alpha;

        if type_ == TYPE_LOW_SHELF {
            set_bq(
                &mut d.sec[0],
                a * ((a + 1.0) - (a - 1.0) * c + s_a),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - s_a),
                (a + 1.0) + (a - 1.0) * c + s_a,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - s_a,
            );
        } else {
            set_bq(
                &mut d.sec[0],
                a * ((a + 1.0) + (a - 1.0) * c + s_a),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - s_a),
                (a + 1.0) - (a - 1.0) * c + s_a,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - s_a,
            );
        }
    } else if type_ == TYPE_LOW_CUT || type_ == TYPE_HIGH_CUT {
        // Butterworth cascade
        let n = order.max(1);
        let is_hp = type_ == TYPE_LOW_CUT;
        let odd = (n % 2) == 1;
        let pairs = n / 2;
        // `0.7071067811865476` in the C++: the same double as FRAC_1_SQRT_2.
        let q_scale = clamp_t(q, 0.1, 12.0) / std::f64::consts::FRAC_1_SQRT_2;

        if odd {
            let k = (w0 * 0.5).tan();
            d.has_first = true;
            d.fa1 = (k - 1.0) / (k + 1.0);

            if is_hp {
                d.fb0 = 1.0 / (1.0 + k);
                d.fb1 = -d.fb0;
            } else {
                d.fb0 = k / (1.0 + k);
                d.fb1 = d.fb0;
            }
        }

        d.n_sec = pairs;

        let mut ki = 0;
        while ki < pairs && (ki as usize) < MAX_SECTIONS {
            let mut qk = if odd {
                1.0 / (2.0 * (f64::from(ki + 1) * PI / f64::from(n)).cos())
            } else {
                1.0 / (2.0 * (f64::from(2 * ki + 1) * PI / f64::from(2 * n)).cos())
            };

            if ki == pairs - 1 {
                qk *= q_scale;
            }

            let alpha = sn / (2.0 * qk);
            let s = &mut d.sec[ki as usize];

            if is_hp {
                set_bq(s, (1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
            } else {
                set_bq(s, (1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
            }
            ki += 1;
        }
    } else if type_ == TYPE_NOTCH {
        let alpha = sn / (2.0 * q);
        set_bq(&mut d.sec[0], 1.0, -2.0 * c, 1.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    } else {
        // band pass, 0 dB peak
        let alpha = sn / (2.0 * q);
        set_bq(&mut d.sec[0], alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }

    d
}

fn section_mag(s: &Coeffs, c1: f64, s1: f64, c2: f64, s2: f64) -> f64 {
    let nr = s.b0 + s.b1 * c1 + s.b2 * c2;
    let ni = s.b1 * s1 + s.b2 * s2;
    let dr = 1.0 + s.a1 * c1 + s.a2 * c2;
    let di = s.a1 * s1 + s.a2 * s2;
    ((nr * nr + ni * ni) / (dr * dr + di * di).max(1.0e-20)).sqrt()
}

fn first_order_mag(b0: f64, b1: f64, a1: f64, c1: f64, s1: f64) -> f64 {
    let nr = b0 + b1 * c1;
    let ni = b1 * s1;
    let dr = 1.0 + a1 * c1;
    let di = a1 * s1;
    ((nr * nr + ni * ni) / (dr * dr + di * di).max(1.0e-20)).sqrt()
}

/// |H| in dB of a designed filter at angular frequency w.
pub fn magnitude_db(d: &Design, w: f64) -> f64 {
    let (c1, s1) = (w.cos(), -w.sin());
    let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
    let mut mag = 1.0;

    let mut k = 0;
    while k < d.n_sec && (k as usize) < MAX_SECTIONS {
        mag *= section_mag(&d.sec[k as usize], c1, s1, c2, s2);
        k += 1;
    }

    if d.has_first {
        mag *= first_order_mag(d.fb0, d.fb1, d.fa1, c1, s1);
    }

    20.0 * mag.max(1.0e-9).log10()
}

/// The curve the editor draws: designed from the values the knobs hold right now, not from the
/// DSP's smoothed state.
pub fn curve_magnitude_db(type_: i32, slope_index: i32, freq_hz: f64, gain_db: f64, q: f64, w: f64, fs: f64) -> f64 {
    magnitude_db(&design_filter(type_, slope_to_order(slope_index), freq_hz, gain_db, q, fs), w)
}

/// The band's response from its LIVE (smoothed) coefficients -- what auto-gain measures.
fn band_mag_db(b: &Band, w: f64) -> f64 {
    let (c1, s1) = (w.cos(), -w.sin());
    let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
    let mut mag = 1.0;

    let mut k = 0;
    while k < b.n_sec && (k as usize) < MAX_SECTIONS {
        mag *= section_mag(&b.sec[k as usize].c, c1, s1, c2, s2);
        k += 1;
    }

    if b.has_first {
        mag *= first_order_mag(b.fb0, b.fb1, b.fa1, c1, s1);
    }

    20.0 * mag.max(1.0e-9).log10()
}

// ------------------------------------------------------------------ per sample
#[inline]
fn run_l(s: &mut Biquad, x: f64) -> f64 {
    let y = s.c.b0 * x + s.z1l;
    s.z1l = s.c.b1 * x - s.c.a1 * y + s.z2l;
    s.z2l = s.c.b2 * x - s.c.a2 * y;
    y
}

#[inline]
fn run_r(s: &mut Biquad, x: f64) -> f64 {
    let y = s.c.b0 * x + s.z1r;
    s.z1r = s.c.b1 * x - s.c.a1 * y + s.z2r;
    s.z2r = s.c.b2 * x - s.c.a2 * y;
    y
}

#[inline]
fn detect(b: &mut Band, xl: f64, xr: f64) {
    let dl = run_l(&mut b.det, xl);
    let dr = run_r(&mut b.det, xr);
    let lvl = dl.abs().max(dr.abs());

    if lvl > b.env {
        b.env = lvl + b.att_c * (b.env - lvl);
    } else {
        b.env = lvl + b.rel_c * (b.env - lvl);
    }
}

#[inline]
fn process_l(b: &mut Band, x: f64) -> f64 {
    let mut y = x;

    if b.has_first {
        let o = b.fb0 * y + b.fz_l;
        b.fz_l = b.fb1 * y - b.fa1 * o;
        y = o;
    }

    let mut k = 0;
    while k < b.n_sec && (k as usize) < MAX_SECTIONS {
        y = run_l(&mut b.sec[k as usize], y);
        k += 1;
    }

    y
}

#[inline]
fn process_r(b: &mut Band, x: f64) -> f64 {
    let mut y = x;

    if b.has_first {
        let o = b.fb0 * y + b.fz_r;
        b.fz_r = b.fb1 * y - b.fa1 * o;
        y = o;
    }

    let mut k = 0;
    while k < b.n_sec && (k as usize) < MAX_SECTIONS {
        y = run_r(&mut b.sec[k as usize], y);
        k += 1;
    }

    y
}

/// The CleanQ engine. Fixed-size state only: nothing here allocates after construction.
pub struct CleanQDsp {
    // ---- outputs back to the UI
    /// Compensation currently applied (published every 24000 samples).
    pub auto_gain_db: f32,
    /// Per-band dynamic offset in dB, for the moving node in the graph (published every 1024).
    pub dyn_offset_db: [f32; NUM_BANDS],

    fs: f64,
    bands: [Band; NUM_BANDS],

    p_out: f32,
    p_bypass_p: f32,
    p_invert_p: f32,
    p_solo_p: f32,
    p_auto_p: f32,
    p_att: f32,
    p_rel: f32,
    auto_counter: i32,

    analysis: Arc<Analysis>,

    auto_s: f64,
    auto_db: f64,
    solo_slot: i32,
    solo_last: i32,
    invert_s: f64,
    solo_mix: f64,
    solo_f: f64,
    solo_q: f64,
    solo_bq: Biquad,
    out_gain_s: f64,
    bypass_s: f64,

    ctl_counter: i32,
    dyn_counter: i32,
    sr_counter: i32,
}

impl CleanQDsp {
    /// `CleanQDSP() { prepare (48000.0); }`. `analysis` receives the analyser's pre/post signals.
    pub fn new(analysis: Arc<Analysis>) -> Self {
        let mut d = CleanQDsp {
            auto_gain_db: 0.0,
            dyn_offset_db: [0.0; NUM_BANDS],
            fs: 48000.0,
            bands: [Band::default(); NUM_BANDS],
            p_out: 0.5,
            p_bypass_p: 0.0,
            p_invert_p: 0.0,
            p_solo_p: 0.0,
            p_auto_p: 0.0,
            p_att: 0.540725,
            p_rel: 0.433677,
            auto_counter: 0,
            analysis,
            auto_s: 1.0,
            auto_db: 0.0,
            solo_slot: -1,
            solo_last: -1,
            invert_s: 1.0,
            solo_mix: 0.0,
            solo_f: 1000.0,
            solo_q: 1.0,
            solo_bq: Biquad::default(),
            out_gain_s: 1.0,
            bypass_s: 0.0,
            ctl_counter: 0,
            dyn_counter: 0,
            sr_counter: 0,
        };
        d.prepare(48000.0);
        d
    }

    // ------------------------------------------------------------------ events
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        match index {
            P_OUT_GAIN => {
                self.p_out = v;
                return;
            }
            P_BYPASS => {
                self.p_bypass_p = v;
                return;
            }
            P_INVERT => {
                self.p_invert_p = v;
                return;
            }
            P_SOLO => {
                self.p_solo_p = v;
                return;
            }
            P_AUTO_GAIN => {
                self.p_auto_p = v;
                return;
            }
            P_DYN_ATTACK => {
                self.p_att = v;
                for b in &mut self.bands {
                    b.dirty = true;
                }
                return;
            }
            P_DYN_RELEASE => {
                self.p_rel = v;
                for b in &mut self.bands {
                    b.dirty = true;
                }
                return;
            }
            _ => {}
        }

        if !(P_FIRST_BAND..NUM_PARAMS).contains(&index) {
            return;
        }

        let slot = (index - P_FIRST_BAND) / KEYS_PER_BAND;
        let key = (index - P_FIRST_BAND) % KEYS_PER_BAND;
        self.set_p(slot, key, v);
    }

    pub fn set_p(&mut self, slot: usize, k: usize, v: f32) {
        let Some(b) = self.bands.get_mut(slot) else { return };

        if k == K_MODE {
            b.p_mode = v;
        } else if k == K_FREQ {
            b.p_freq = v;
        } else if k == K_GAIN {
            b.p_gain = v;
        } else if k == K_Q {
            b.p_q = v;
        } else {
            b.p_dyn_pack = v;
        }

        b.dirty = true;
    }

    // ------------------------------------------------------------------ init
    /// Resets every band and smoother. The knob values (`p*`) are kept, as in the C++.
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = if sample_rate > 0.0 { sample_rate } else { 48000.0 };

        for b in &mut self.bands {
            // `b = Band {}` resets the knob values too; the wrapper's forcePush re-sends them.
            *b = Band::default();
            b.last_type = -1;
            b.last_order = -1;
            b.dirty = true;
        }

        self.auto_s = 1.0;
        self.auto_db = 0.0;
        self.auto_counter = 0;
        self.solo_slot = -1;
        self.solo_last = -1;
        self.solo_f = 1000.0;
        self.solo_q = 1.0;
        self.solo_mix = 0.0;
        self.invert_s = 1.0;
        self.out_gain_s = 1.0;
        self.bypass_s = 0.0;
        clear_bq(&mut self.solo_bq);
        self.ctl_counter = 0;
        self.dyn_counter = 0;
        self.sr_counter = 0;

        self.dyn_offset_db = [0.0; NUM_BANDS];
        self.auto_gain_db = 0.0;
    }

    // ------------------------------------------------------------------ the curve, for drawing
    /// |H| in dB of one band at angular frequency w, from the live coefficients.
    pub fn band_magnitude_db(&self, slot: usize, w: f64) -> f64 {
        match self.bands.get(slot) {
            Some(b) => band_mag_db(b, w),
            None => 0.0,
        }
    }

    pub fn band_is_used(&self, slot: usize) -> bool {
        self.bands.get(slot).is_some_and(|b| b.used)
    }
    pub fn band_is_active(&self, slot: usize) -> bool {
        self.bands.get(slot).is_some_and(|b| b.active)
    }
    pub fn band_place(&self, slot: usize) -> i32 {
        self.bands.get(slot).map_or(0, |b| b.place)
    }

    /// The live transport reset (not in the C++, whose `prepareToPlay` is [`Self::prepare`]): clears
    /// all audio state (every band's filter and detector lines, envelopes, the solo filter) and
    /// seeds every glide at its current target, so nothing re-glides from defaults and two resets
    /// of the same settings render alike. The knob values (`p*`) are kept and must already be
    /// pushed. `sample_rate` may differ from the current one (a live re-prepare). No allocation.
    pub fn reset_live(&mut self, sample_rate: f64) {
        self.fs = if sample_rate > 0.0 { sample_rate } else { 48000.0 };
        let (fs, p_att, p_rel) = (self.fs, self.p_att, self.p_rel);
        for b in &mut self.bands {
            // Every value derived again from the knobs, as `mapParams` does when dirty.
            Self::map_params(fs, p_att, p_rel, b);
            clear_band(b);
            b.env = 0.0;
            b.dyn_off = 0.0;
            if !b.used {
                b.was_used = false;
                b.last_type = -1;
                b.last_order = -1;
                continue;
            }
            // At target: with the envelope cleared the dynamic offset is 0 (`controlBand`).
            b.freq_s = b.freq_t;
            b.q_s = b.q_t;
            b.gain_s = clamp_t(b.gain_t, -36.0, 36.0);
            b.was_used = true;
            b.last_type = b.type_;
            b.last_order = b.order;
            Self::compute_coefs(fs, b);
        }

        self.bypass_s = if self.p_bypass_p > 0.5f32 { 1.0 } else { 0.0 };
        self.invert_s = if self.p_invert_p > 0.5f32 { -1.0 } else { 1.0 };
        self.out_gain_s = (knob_to_out_db(self.p_out) / LN_TO_DB).exp();

        // Solo, as `control` sets it, at its target.
        self.solo_slot = ((self.p_solo_p * 24.0f32 + 0.5f32) as i32).saturating_sub(1);
        if self.solo_slot >= NUM_BANDS as i32 {
            self.solo_slot = NUM_BANDS as i32 - 1;
        }
        clear_bq(&mut self.solo_bq);
        self.solo_mix = 0.0;
        if self.solo_slot >= 0 {
            let (used, f, q) = {
                let sb = &self.bands[self.solo_slot as usize];
                (sb.used, sb.freq_s, sb.q_s)
            };
            if used {
                self.solo_mix = 1.0;
                self.solo_f = f;
                self.solo_q = q;
                let w0 = 2.0 * PI * self.solo_f.min(self.fs * 0.49) / self.fs;
                let sq = clamp_t(self.solo_q, 0.3, 40.0);
                let al = w0.sin() / (2.0 * sq);
                set_bq(&mut self.solo_bq.c, al, 0.0, -al, 1.0 + al, -2.0 * w0.cos(), 1.0 - al);
            }
        }
        self.solo_last = self.solo_slot;

        // Auto gain measured from the seeded curve, at its target; its counter restarts.
        if self.p_auto_p > 0.5f32 {
            self.auto_db = clamp_t(-self.curve_mean_db(), -24.0, 24.0);
            self.auto_counter = 0;
            self.auto_s = 10.0f64.powf(self.auto_db / 20.0);
        } else {
            self.auto_db = 0.0;
            self.auto_counter = 127;
            self.auto_s = 1.0;
        }

        self.ctl_counter = 0;
        self.dyn_counter = 0;
        self.sr_counter = 0;
        self.dyn_offset_db = [0.0; NUM_BANDS];
        self.auto_gain_db = self.auto_db as f32;
    }

    // ------------------------------------------------------------------ main
    /// In place on the first `num_channels` (1 or 2) buffers; `numSamples` is clipped to the
    /// buffers' length.
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        if num_channels < 1 {
            return;
        }
        let Some((first, rest)) = io.split_first_mut() else { return };
        let ch_l: &mut [f32] = first.as_mut_slice();
        let mut ch_r: Option<&mut [f32]> = if num_channels > 1 { rest.first_mut().map(Vec::as_mut_slice) } else { None };
        let n = num_samples.min(ch_l.len()).min(ch_r.as_ref().map_or(usize::MAX, |r| r.len()));

        for i in 0..n {
            self.ctl_counter += 1;
            if self.ctl_counter >= 16 {
                self.ctl_counter = 0;
                self.control();
            }

            let xl = f64::from(ch_l[i]);
            let xr = match &ch_r {
                Some(r) => f64::from(r[i]),
                None => xl,
            };
            let mut l = xl;
            let mut r = xr;

            for b in &mut self.bands {
                if !b.used {
                    continue;
                }

                let pl = b.place;

                if b.dyn_ && b.type_ <= TYPE_HIGH_SHELF {
                    if pl == PLACE_MID {
                        let m = (xl + xr) * 0.5;
                        detect(b, m, m);
                    } else if pl == PLACE_SIDE {
                        let s = (xl - xr) * 0.5;
                        detect(b, s, s);
                    } else if pl == PLACE_LEFT {
                        detect(b, xl, xl);
                    } else if pl == PLACE_RIGHT {
                        detect(b, xr, xr);
                    } else {
                        detect(b, xl, xr);
                    }
                }

                if b.active {
                    if pl == PLACE_STEREO {
                        l = process_l(b, l);
                        r = process_r(b, r);
                    } else if pl == PLACE_LEFT {
                        l = process_l(b, l);
                    } else if pl == PLACE_RIGHT {
                        r = process_r(b, r);
                    } else {
                        let mut m = (l + r) * 0.5;
                        let mut s = (l - r) * 0.5;

                        if pl == PLACE_MID {
                            m = process_l(b, m);
                        } else {
                            s = process_l(b, s);
                        }

                        l = m + s;
                        r = m - s;
                    }
                }
            }

            if self.solo_mix > 0.001 {
                // Solo: hear only the band's region of the input
                let mut sl = xl;
                let mut sr = xr;
                let spl = self.bands[self.solo_slot.max(0) as usize % NUM_BANDS].place;

                if spl == PLACE_MID {
                    let m = (xl + xr) * 0.5;
                    sl = m;
                    sr = m;
                } else if spl == PLACE_SIDE {
                    let s = (xl - xr) * 0.5;
                    sl = s;
                    sr = -s;
                } else if spl == PLACE_LEFT {
                    sr = 0.0;
                } else if spl == PLACE_RIGHT {
                    sl = 0.0;
                }

                sl = run_l(&mut self.solo_bq, sl);
                sr = run_r(&mut self.solo_bq, sr);

                l = l * (1.0 - self.solo_mix) + sl * self.solo_mix;
                r = r * (1.0 - self.solo_mix) + sr * self.solo_mix;
            }

            l = l * self.out_gain_s * self.invert_s * self.auto_s;
            r = r * self.out_gain_s * self.invert_s * self.auto_s;

            let wet = 1.0 - self.bypass_s;
            let yl = l * wet + xl * self.bypass_s;
            let yr = r * wet + xr * self.bypass_s;

            ch_l[i] = yl as f32;
            if let Some(rr) = ch_r.as_deref_mut() {
                rr[i] = yr as f32;
            }

            // fftInPre / fftInPost: the analyser's two signals, now the UI's rings.
            self.analysis.pre.push(((xl + xr) * 0.5) as f32);
            self.analysis.post.push(((yl + yr) * 0.5) as f32);

            // The C++ snapshots its rings for the editor here once a hop (`hopCount`,
            // `snapshotAnalyser`); the analysis rings need no snapshot, so neither is ported.

            self.dyn_counter += 1;
            if self.dyn_counter >= 1024 {
                self.dyn_counter = 0;
                self.publish_dynamics();
            }

            self.sr_counter -= 1;
            if self.sr_counter <= 0 {
                self.sr_counter = 24000;
                self.auto_gain_db = self.auto_db as f32;
            }
        }
    }

    // ------------------------------------------------------------------ mapping
    /// `mapParams`. Takes the globals it reads instead of `self`, so a band can be borrowed alone.
    fn map_params(fs: f64, p_att: f32, p_rel: f32, b: &mut Band) {
        let n = clamp_t((b.p_mode * 1679.0f32 + 0.5f32) as i32, 0, 1679);
        let on_i = n % 3;
        b.used = on_i >= 1;
        b.active = on_i == 2;
        b.type_ = (n / 3) % 7;
        b.place = (n / 21) % 5;
        b.order = slope_to_order((n / 105) % 8);
        let dyn_on = ((n / 840) % 2) == 1;

        let d = clamp_t((b.p_dyn_pack * 14640.0f32 + 0.5f32) as i32, 0, 14640);

        b.freq_t = knob_to_freq(b.p_freq);
        b.gain_t = knob_to_gain_db(b.p_gain);
        b.q_t = knob_to_q(b.p_q);

        b.dyn_ = dyn_on;
        b.thr = f64::from(d / 121) * 0.5 - 60.0;
        b.range = f64::from(d % 121) * 0.5 - 30.0;

        let att_ms = knob_to_attack_ms(p_att);
        let rel_ms = knob_to_release_ms(p_rel);
        b.att_c = (-1.0 / (att_ms * 0.001 * fs)).exp();
        b.rel_c = (-1.0 / (rel_ms * 0.001 * fs)).exp();

        if !b.dyn_ {
            b.dyn_off = 0.0;
        }

        b.dirty = false;
    }

    fn compute_coefs(fs: f64, b: &mut Band) {
        let d = design_filter(b.type_, b.order, b.freq_s, b.gain_s, b.q_s, fs);

        b.n_sec = d.n_sec;
        b.has_first = d.has_first;
        b.fb0 = d.fb0;
        b.fb1 = d.fb1;
        b.fa1 = d.fa1;

        // coefficients only -- the delay lines keep running
        for (s, c) in b.sec.iter_mut().zip(d.sec.iter()) {
            s.c = *c;
        }

        // Detector: band pass around the band, for dynamic mode
        let f = b.freq_s.min(fs * 0.49);
        let w0 = 2.0 * PI * f / fs;
        let dq = clamp_t(b.q_s, 0.3, 12.0);
        let da = w0.sin() / (2.0 * dq);
        set_bq(&mut b.det.c, da, 0.0, -da, 1.0 + da, -2.0 * w0.cos(), 1.0 - da);

        b.last_f = b.freq_s;
        b.last_g = b.gain_s;
        b.last_q = b.q_s;
    }

    // ------------------------------------------------------------------ control rate (every 16 samples)
    fn control_band(fs: f64, p_att: f32, p_rel: f32, b: &mut Band) {
        if b.dirty {
            Self::map_params(fs, p_att, p_rel, b);
        }

        if !b.used {
            b.was_used = false;
            return;
        }

        let mut force = false;

        if !b.was_used || b.type_ != b.last_type || b.order != b.last_order {
            if !b.was_used {
                b.freq_s = b.freq_t;
                b.q_s = b.q_t;
                b.gain_s = b.gain_t;
                b.env = 0.0;
                b.dyn_off = 0.0;
            }

            clear_band(b);
            b.was_used = true;
            b.last_type = b.type_;
            b.last_order = b.order;
            force = true;
        }

        let mut g = b.gain_t;

        if b.dyn_ && b.type_ <= TYPE_HIGH_SHELF {
            let env_db = LN_TO_DB * (b.env + 1.0e-9).ln();
            let over = env_db - b.thr;
            let amt = clamp_t(over / 6.0, 0.0, 1.0);
            b.dyn_off = b.range * amt;
            g += b.dyn_off;
        }

        g = clamp_t(g, -36.0, 36.0);

        let k = 0.25;
        b.gain_s = b.gain_s + (g - b.gain_s) * k;
        b.freq_s = (b.freq_s.ln() + (b.freq_t.ln() - b.freq_s.ln()) * k).exp();
        b.q_s = (b.q_s.ln() + (b.q_t.ln() - b.q_s.ln()) * k).exp();

        if force
            || (b.gain_s - b.last_g).abs() > 0.001
            || (b.freq_s - b.last_f).abs() > b.last_f * 1.0e-5
            || (b.q_s - b.last_q).abs() > b.last_q * 1.0e-5
        {
            Self::compute_coefs(fs, b);
        }
    }

    /// Mean curve gain in dB over 32 log-spaced points, 20 Hz .. 20 kHz (equal weight per
    /// octave = pink).
    fn curve_mean_db(&self) -> f64 {
        let mut sum = 0.0;
        let mut count = 0.0;

        for n in 0..32 {
            let f = 20.0 * 1000.0f64.powf(f64::from(n) / 31.0);

            if f < self.fs * 0.45 {
                let w = 2.0 * PI * f / self.fs;
                let mut g = 0.0;

                for b in &self.bands {
                    if b.used && b.active {
                        g += band_mag_db(b, w);
                    }
                }

                sum += g;
                count += 1.0;
            }
        }

        if count > 0.0 { sum / count } else { 0.0 }
    }

    fn control(&mut self) {
        let (fs, p_att, p_rel) = (self.fs, self.p_att, self.p_rel);
        for b in &mut self.bands {
            Self::control_band(fs, p_att, p_rel, b);
        }

        let byp_target = if self.p_bypass_p > 0.5f32 { 1.0 } else { 0.0 };
        self.bypass_s = self.bypass_s + (byp_target - self.bypass_s) * 0.25;

        let inv_t = if self.p_invert_p > 0.5f32 { -1.0 } else { 1.0 };
        self.invert_s = self.invert_s + (inv_t - self.invert_s) * 0.25;

        // `saturating_sub`: the C++ `- 1` cannot overflow for any reachable value; this keeps a
        // hostile `set_parameter` (a huge negative solo) from overflowing in Rust.
        self.solo_slot = ((self.p_solo_p * 24.0f32 + 0.5f32) as i32).saturating_sub(1);
        if self.solo_slot >= NUM_BANDS as i32 {
            self.solo_slot = NUM_BANDS as i32 - 1;
        }

        let mut solo_t = 0.0;

        if self.solo_slot >= 0 {
            // Only the fields used, not a copy of the whole band.
            let (sb_used, sb_freq_s, sb_q_s) = {
                let sb = &self.bands[self.solo_slot as usize];
                (sb.used, sb.freq_s, sb.q_s)
            };

            if sb_used {
                solo_t = 1.0;

                if self.solo_slot != self.solo_last
                    || (sb_freq_s - self.solo_f).abs() > self.solo_f * 1.0e-4
                    || (sb_q_s - self.solo_q).abs() > self.solo_q * 1.0e-4
                {
                    self.solo_f = sb_freq_s;
                    self.solo_q = sb_q_s;
                    let w0 = 2.0 * PI * self.solo_f.min(self.fs * 0.49) / self.fs;
                    let sq = clamp_t(self.solo_q, 0.3, 40.0);
                    let al = w0.sin() / (2.0 * sq);
                    set_bq(&mut self.solo_bq.c, al, 0.0, -al, 1.0 + al, -2.0 * w0.cos(), 1.0 - al);
                    if self.solo_slot != self.solo_last {
                        clear_bq(&mut self.solo_bq);
                    }
                }
            }
        }

        self.solo_last = self.solo_slot;
        self.solo_mix = self.solo_mix + (solo_t - self.solo_mix) * 0.25;

        let mut auto_t = 1.0;

        if self.p_auto_p > 0.5f32 {
            self.auto_counter += 1;
            if self.auto_counter >= 128 {
                // every 2048 samples
                self.auto_counter = 0;
                self.auto_db = clamp_t(-self.curve_mean_db(), -24.0, 24.0);
            }

            auto_t = 10.0f64.powf(self.auto_db / 20.0);
        } else {
            self.auto_db = 0.0;
            self.auto_counter = 127;
        }

        self.auto_s = self.auto_s + (auto_t - self.auto_s) * 0.05;

        let out_t = (knob_to_out_db(self.p_out) / LN_TO_DB).exp();
        self.out_gain_s = self.out_gain_s + (out_t - self.out_gain_s) * 0.25;
    }

    /// The Cmajor sent one dynamic band per call on a rotating cursor; publish them all.
    fn publish_dynamics(&mut self) {
        for (o, b) in self.dyn_offset_db.iter_mut().zip(self.bands.iter()) {
            *o = if b.used && b.dyn_ && b.type_ <= TYPE_HIGH_SHELF { clamp_t(b.dyn_off, -99.0, 99.0) as f32 } else { 0.0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_and_unpack_round_trip() {
        for on in 0..3 {
            for type_ in 0..7 {
                for place in 0..5 {
                    for slope in 0..8 {
                        for dyn_ in [false, true] {
                            let m = ModeFields { on, type_, place, slope, dyn_ };
                            assert_eq!(unpack_mode(pack_mode(&m)), m);
                        }
                    }
                }
            }
        }
        let d = unpack_dyn(pack_dyn(&DynFields { threshold_db: -24.0, range_db: -6.0 }));
        assert_eq!((d.threshold_db, d.range_db), (-24.0, -6.0));
        // The Cmajor's packed default reads back as -20 dB / +1.5 dB (see Parameters.h).
        let d = unpack_dyn(0.665527);
        assert_eq!((d.threshold_db, d.range_db), (-20.0, 1.5));
    }

    #[test]
    fn knob_maps_hit_their_ends() {
        assert!((knob_to_freq(0.0) - 10.0).abs() < 1e-9 && (knob_to_freq(1.0) - 20000.0).abs() < 1e-6);
        assert_eq!((knob_to_gain_db(0.0), knob_to_gain_db(1.0)), (-30.0, 30.0));
        assert!((knob_to_q(1.0) - 40.0).abs() < 1e-9);
        assert!((knob_to_attack_ms(1.0) - 500.0).abs() < 1e-9 && (knob_to_release_ms(1.0) - 5000.0).abs() < 1e-6);
        assert_eq!(knob_to_out_db(1.0), 24.0);
        assert!((knob_to_freq(freq_to_knob(1000.0)) - 1000.0).abs() < 0.01);
        assert_eq!(slope_db_per_oct(-3), 6);
        assert_eq!(slope_db_per_oct(99), 96);
        assert_eq!(slope_to_order(7), 16);
    }

    #[test]
    fn butterworth_cut_is_minus_3_db_at_the_corner_for_every_slope() {
        let fs = 48000.0;
        let w = 2.0 * PI * 1000.0 / fs;
        for si in 0..8 {
            for t in [TYPE_LOW_CUT, TYPE_HIGH_CUT] {
                let db = curve_magnitude_db(t, si, 1000.0, 0.0, std::f64::consts::FRAC_1_SQRT_2, w, fs);
                assert!((db + 3.0103).abs() < 0.05, "slope {si} type {t}: {db}");
            }
        }
    }
}
