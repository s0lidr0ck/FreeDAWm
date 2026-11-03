//! TwineDSP -- a clipper and a limiter, married, for mastering your own work.
//! Line-by-line translation of `Twine/Source/TwineDSP.h` (class `fs::twine::TwineDSP`, in the
//! header's order).
//!
//! ```text
//!   Drive -> Clipper (4x oversampled) -> Limiter (lookahead, true-peak) -> Ceiling
//! ```
//!
//! The clipper runs at 4x the sample rate behind a 257-tap linear-phase Kaiser FIR; the limiter
//! watches 8x-interpolated true peaks and is a lookahead design: the gain for a sample is the
//! box average, over the lookahead window, of the lowest gain any sample in that window needs.
//! Multiband (the Waves L3 idea) splits the clipped signal into three Linkwitz-Riley bands, each
//! with its own lookahead limiter, sharing the reduction by each band's share of the power; the
//! full-band limiter still runs after them. Latency is fixed per mode (Multiband adds one more
//! interpolator and lookahead).
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - `double` and `float` exactly where the C++ has them: the knobs and their maps are `float`
//!   (`std::pow (200.0f, ..)` is `powf`), the signal path is `double`, and the delay lines
//!   (`wetDelay`, `dryDelay`, `bandDelay`) store `float`, so the wet signal is rounded to `f32`
//!   on its way through the lookahead exactly as in the C++.
//! - `std::max` / `std::min` / `std::clamp` are the C++ comparisons (`cmax`, `cmin`, `clamp_c`),
//!   so a NaN propagates the way it does there rather than being dropped like Rust's `max`.
//! - `std::lround` on a choice knob is `f32::round` (half away from zero) then the cast; the knob
//!   is always finite here (the plugin wrapper clamps first, as the APVTS does).
//! - Integer index arithmetic keeps the C++ `int` (`i32`) expressions, including the `+ size * k`
//!   terms that keep `%` non-negative; the fixed deque keeps its `& 1023` wrap and unchecked
//!   `count`, as the C++.
//! - `std::vector::assign` re-fills in place: here `clear` + `resize`, which reuses the capacity,
//!   so `resetLimiters` (a Character or Multiband change on the audio thread) never allocates.
//!   `design` writes into a caller's buffer instead of returning a new vector, for the same
//!   reason; the numbers are the same.
//! - The atomics (`inPeak`, `outTruePeak`, `clipGr`, `limGr`, `bandGr`) are running maxima the
//!   editor takes with `exchange (0)`. Here they are plain fields with the same `raiseMax`, and
//!   [`TwineDsp::take_meters`] is `take` on all of them at once (marked).
//! - The mono input scope (`scope`, `scopeWrite`, `copyScope`) is kept. `process` also hands
//!   each scope sample to an optional [`AnalysisRing`] at the same line (marked): the Rust
//!   module's analysis stream, written without allocating.

// Constants and literals are kept exactly as the C++ writes them (`3.14159265358979` in the
// crossover is a different double from `f64::consts::PI`, and is what the C++ computes with).
#![allow(clippy::excessive_precision, clippy::approx_constant)]
// Index loops over parallel per-channel arrays are kept as the C++ writes them.
#![allow(clippy::needless_range_loop)]

use crate::analysis::AnalysisRing;

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
/// `std::clamp (v, lo, hi)`: `v < lo ? lo : (hi < v ? hi : v)`.
#[inline]
fn clamp_c<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}
/// `std::lround (v)` for a finite knob value: round half away from zero, then to `int`.
#[inline]
fn lround(v: f32) -> i32 {
    v.round() as i32
}

// ------------------------------------------------------------------ ParamIndex
pub const P_DRIVE: usize = 0;
pub const P_CLIP: usize = 1;
pub const P_SHAPE: usize = 2;
pub const P_CHARACTER: usize = 3;
pub const P_CEILING: usize = 4;
pub const P_RELEASE: usize = 5;
pub const P_MATCH: usize = 6;
pub const P_CHAFF: usize = 7;
pub const P_BYPASS: usize = 8;
// appended: multiband
pub const P_MULTI: usize = 9;
pub const P_XOVER_LO: usize = 10;
pub const P_XOVER_HI: usize = 11;
pub const NUM_PARAMS: usize = 12;

// enum Shape
pub const SHAPE_HARD: i32 = 0;
pub const SHAPE_SOFT: i32 = 1;
pub const SHAPE_TAPE: i32 = 2;
pub const NUM_SHAPES: i32 = 3;
// enum Character
pub const CHAR_TRANSPARENT: i32 = 0;
pub const CHAR_PUNCHY: i32 = 1;
pub const CHAR_LOUD: i32 = 2;
pub const NUM_CHARACTERS: i32 = 3;

// ------------------------------------------------------------------ knob maps
pub fn knob_to_drive_db(v: f32) -> f32 {
    24.0f32 * clamp01(v)
}
pub fn knob_to_ceiling_db(v: f32) -> f32 {
    -12.0f32 + 12.0f32 * clamp01(v)
}
/// 5 .. 1000 ms.
pub fn knob_to_release_ms(v: f32) -> f32 {
    5.0f32 * 200.0f32.powf(clamp01(v))
}
pub fn release_ms_to_knob(ms: f32) -> f32 {
    (ms / 5.0f32).ln() / 200.0f32.ln()
}
/// How far above the ceiling the clipper sits: 6 dB at the lightest setting, 0 at 100 %.
pub fn clip_headroom_db(clip: f32) -> f32 {
    6.0f32 * (1.0f32 - clamp01(clip))
}

pub fn lookahead_ms(character: i32) -> f32 {
    if character == CHAR_LOUD { 1.5 } else { 5.0 }
}

// Multiband crossovers, the same ranges as Comp Engine's: 40 Hz .. 1 kHz and 800 Hz .. 12 kHz,
// the upper held at least an octave above the lower.
pub fn knob_to_xover_lo(v: f32) -> f32 {
    40.0f32 * 25.0f32.powf(clamp01(v))
}
pub fn knob_to_xover_hi(v: f32) -> f32 {
    800.0f32 * 15.0f32.powf(clamp01(v))
}
pub fn xover_lo_to_knob(hz: f32) -> f32 {
    (hz / 40.0f32).ln() / 25.0f32.ln()
}
pub fn xover_hi_to_knob(hz: f32) -> f32 {
    (hz / 800.0f32).ln() / 15.0f32.ln()
}

pub fn release_scale(character: i32) -> f32 {
    // Punchy recovers fast between hits so the drums keep their bounce; Loud recovers
    // fairly fast and attacks late (1.5 ms), which is louder and dirtier.
    if character == CHAR_PUNCHY {
        0.35
    } else if character == CHAR_LOUD {
        0.5
    } else {
        1.0
    }
}

/// `clamp01`: `v < 0 ? 0 : (v > 1 ? 1 : v)`, the same comparisons as `std::clamp`.
fn clamp01(v: f32) -> f32 {
    clamp_c(v, 0.0, 1.0)
}

/// `raiseMax`: the compare-exchange loop, single-threaded.
fn raise_max(a: &mut f32, v: f32) {
    if v > *a {
        *a = v;
    }
}

/// The meters, taken (read and zeroed) together. Linear peaks, dB amounts (positive).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Meters {
    pub in_peak: f32,
    pub out_true_peak: f32,
    pub clip_gr: f32,
    pub lim_gr: f32,
    pub band_gr: [f32; 3],
}

// ------------------------------------------------------------------ filters
fn bessel0(x: f64) -> f64 {
    let mut s = 1.0f64;
    let mut t = 1.0f64;
    for k in 1..30 {
        let k = f64::from(k);
        t *= (x / (2.0 * k)) * (x / (2.0 * k));
        s += t;
    }
    s
}

/// Kaiser-windowed sinc low-pass at `cut` (fraction of the 4x rate), `taps` long, odd.
/// (Written into `h`, resized in place: see the module notes.)
fn design(h: &mut Vec<f64>, taps: i32, cut: f64, beta: f64, gain: f64) {
    h.clear();
    h.resize(taps.max(0) as usize, 0.0);
    let mid = taps / 2;
    let pi = 3.14159265358979323846f64;
    for i in 0..taps {
        let t = f64::from(i - mid);
        let sinc = if t == 0.0 { 2.0 * cut } else { (2.0 * pi * cut * t).sin() / (pi * t) };
        let r = t / f64::from(mid);
        let w = bessel0(beta * cmax(0.0, 1.0 - r * r).sqrt()) / bessel0(beta);
        h[i as usize] = sinc * w * gain;
    }
}

/// `vector::assign (n, value)`: same contents, capacity reused.
fn assign<T: Copy>(v: &mut Vec<T>, n: usize, value: T) {
    v.clear();
    v.resize(n, value);
}

// ------------------------------------------------------------------ the limiter
#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    t: i64,
    v: f64,
}

/// A small fixed-capacity deque, so the audio thread never allocates.
#[derive(Debug, Clone)]
struct Deque {
    buf: [Entry; 1024],
    head: i32,
    count: i32,
}

impl Default for Deque {
    fn default() -> Self {
        Deque { buf: [Entry::default(); 1024], head: 0, count: 0 }
    }
}

impl Deque {
    fn empty(&self) -> bool {
        self.count == 0
    }
    fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }
    fn front(&self) -> Entry {
        self.buf[self.head as usize]
    }
    fn back(&self) -> Entry {
        // `(head + count - 1) & 1023`: the C++ int arithmetic, masked like there.
        self.buf[((self.head + self.count - 1) & 1023) as usize]
    }
    fn pop_front(&mut self) {
        self.head = (self.head + 1) & 1023;
        self.count -= 1;
    }
    fn pop_back(&mut self) {
        self.count -= 1;
    }
    fn push_back(&mut self, e: Entry) {
        if self.count < 1024 {
            self.buf[((self.head + self.count) & 1023) as usize] = e;
            self.count += 1;
        }
    }
}

/// One lookahead limiter's gain computer: the gain needed now in, the gain to apply
/// `lookMax` samples later out (see the guarantee at the top). Used for the full-band stage
/// and for each band.
#[derive(Debug, Clone)]
struct LimiterCore {
    held: Vec<f64>,
    gain_delay: Vec<f64>,
    dq: Deque,
    look: i32,
    look_max: i32,
    held_pos: i32,
    gd_pos: i32,
    held_sum: f64,
    release_state: f64,
    tick: i64,
}

impl Default for LimiterCore {
    fn default() -> Self {
        LimiterCore {
            held: Vec::new(),
            gain_delay: Vec::new(),
            dq: Deque::default(),
            look: 1,
            look_max: 1,
            held_pos: 0,
            gd_pos: 0,
            held_sum: 0.0,
            release_state: 1.0,
            tick: 0,
        }
    }
}

impl LimiterCore {
    fn reset(&mut self, look_in: i32, look_max_in: i32) {
        self.look_max = cmax(1, look_max_in);
        self.look = clamp_c(look_in, 1, self.look_max);
        assign(&mut self.held, self.look_max as usize + 1, 1.0);
        assign(&mut self.gain_delay, self.look_max as usize + 1, 1.0);
        self.dq.clear();
        self.held_pos = 0;
        self.held_sum = f64::from(self.look);
        self.gd_pos = 0;
        self.release_state = 1.0;
        self.tick = 0;
    }

    fn step(&mut self, need: f64, rel_a: f64) -> f64 {
        // sliding minimum of `need` over the last `look` samples
        self.tick += 1;
        while !self.dq.empty() && self.dq.back().v >= need {
            self.dq.pop_back();
        }
        self.dq.push_back(Entry { t: self.tick, v: need });
        while self.dq.front().t <= self.tick - i64::from(self.look) {
            self.dq.pop_front();
        }
        let m = self.dq.front().v;

        // the held minimum may only rise at the release rate
        self.release_state = if m < self.release_state { m } else { self.release_state + (m - self.release_state) * rel_a };

        // box average over `look`: the smooth ramp down
        let hp = self.held_pos as usize;
        self.held_sum += self.release_state - self.held[hp];
        self.held[hp] = self.release_state;
        self.held_pos = (self.held_pos + 1) % self.look;
        let g_now = cmin(1.0, self.held_sum / f64::from(self.look));

        // line up with the fixed latency
        let extra = self.look_max - self.look;
        if extra <= 0 {
            return g_now;
        }
        let size = self.gain_delay.len() as i32;
        self.gain_delay[self.gd_pos as usize] = g_now;
        let out = self.gain_delay[((self.gd_pos - extra + size * 4) % size) as usize];
        self.gd_pos = (self.gd_pos + 1) % size;
        out
    }
}

// ------------------------------------------------------------------ the multiband stage
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
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
    fn set_coefs(&mut self, c: &Biquad) {
        self.b0 = c.b0;
        self.b1 = c.b1;
        self.b2 = c.b2;
        self.a1 = c.a1;
        self.a2 = c.a2;
    }
}

fn butter(hz: f64, fs: f64, high: bool) -> Biquad {
    let w = (3.14159265358979 * cmin(hz, fs * 0.45) / fs).tan();
    let k = 1.0 / 2.0f64.sqrt();
    let n = 1.0 / (1.0 + w / k + w * w);
    let mut q = Biquad::default();
    if high {
        q.b0 = n;
        q.b1 = -2.0 * n;
        q.b2 = n;
    } else {
        q.b0 = w * w * n;
        q.b1 = 2.0 * w * w * n;
        q.b2 = w * w * n;
    }
    q.a1 = 2.0 * (w * w - 1.0) * n;
    q.a2 = (1.0 - w / k + w * w) * n;
    q
}

fn allpass(hz: f64, fs: f64) -> Biquad {
    let w = (3.14159265358979 * cmin(hz, fs * 0.45) / fs).tan();
    let k = 1.0 / 2.0f64.sqrt();
    let n = 1.0 / (1.0 + w / k + w * w);
    let mut q = Biquad::default();
    q.b0 = (1.0 - w / k + w * w) * n;
    q.b1 = 2.0 * (w * w - 1.0) * n;
    q.b2 = 1.0;
    q.a1 = q.b1;
    q.a2 = q.b0;
    q
}

/// Linkwitz-Riley 24 dB/oct, three ways; the low band through the upper split's allpass,
/// so the three sum back flat. The same crossover as Comp Engine's.
#[derive(Debug, Clone, Copy, Default)]
struct Crossover {
    lo1a: Biquad,
    lo1b: Biquad,
    hi1a: Biquad,
    hi1b: Biquad,
    lo2a: Biquad,
    lo2b: Biquad,
    hi2a: Biquad,
    hi2b: Biquad,
    ap2: Biquad,
}

impl Crossover {
    fn split(&mut self, x: f64, out: &mut [f64; 3]) {
        let low = self.ap2.run(self.lo1b.run(self.lo1a.run(x)));
        let rest = self.hi1b.run(self.hi1a.run(x));
        out[0] = low;
        out[1] = self.lo2b.run(self.lo2a.run(rest));
        out[2] = self.hi2b.run(self.hi2a.run(rest));
    }
}

const TP_OVER: i32 = 8;
const SCOPE_SIZE: i32 = 4096;

/// `fs::twine::TwineDSP`.
#[derive(Debug, Clone)]
pub struct TwineDsp {
    // ------------------------------------------------------------------ meters (audio -> UI)
    // Running maxima since the editor last took them (take* resets).
    in_peak: f32,
    out_true_peak: f32,
    clip_gr: f32,
    lim_gr: f32,
    band_gr: [f32; 3], // Multiband: each band's limiter

    // ------------------------------------------------------------------ state
    sr: f64,
    drive_knob: f32,
    clip_knob: f32,
    ceiling_knob: f32,
    release_knob: f32,
    shape: i32,
    character: i32,
    match_: bool,
    chaff: bool,
    bypass: bool,

    os_h: Vec<f64>,
    up_branch: Vec<Vec<f64>>,
    tp_branch: Vec<Vec<f64>>,
    /// The temporary `tp` of `buildFilters`, kept so a rebuild reuses its capacity.
    tp_scratch: Vec<f64>,
    os_cut: f64, // of the 4x rate: 0.468 of the base rate (22.5 kHz at 48k)
    os_taps: i32,
    up_taps: i32,
    os_delay: i32,
    tp_taps: i32,
    tp_taps_total: i32,
    tp_delay: i32,
    look_max: i32,
    #[allow(dead_code)] // declared and never used in the C++ either
    look: i32,
    latency: i32,

    up_hist: [Vec<f64>; 2],
    down_hist: [Vec<f64>; 2],
    tp_hist: [Vec<f64>; 2],
    out_tp_hist: [Vec<f64>; 2],
    wet_delay: [Vec<f32>; 2],
    dry_delay: [Vec<f32>; 2],
    up_pos: i32,
    down_pos_for: [i32; 2],
    down_pos: i32,
    tp_pos: i32,
    out_tp_pos: i32,
    wet_pos: i32,
    dry_pos: i32,

    wide: LimiterCore,
    band_lim: [LimiterCore; 3],

    // multiband
    multi: bool,
    x_lo_knob: f32, // 150 Hz
    x_hi_knob: f32, // 3 kHz
    x_lo: f32,
    x_hi: f32,
    xover: [Crossover; 2],
    band_delay: [[Vec<f32>; 3]; 2],
    tp2_hist: [Vec<f64>; 2],
    band_env: [f64; 3],
    sum_env: f64,
    aim_now: f64,
    band_pos: i32,
    tp_pos2: i32,
    stage_delay: i32,
    latency_single: i32,
    latency_multi: i32,

    scope: Box<[f32; SCOPE_SIZE as usize]>,
    scope_write: i32,
    scope_w: i32,

    drive_s: f64,
    ceil_s: f64,
    byp_s: f64,
    smooth_init: bool,
}

impl Default for TwineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl TwineDsp {
    // ------------------------------------------------------------------ lifecycle
    /// `TwineDSP() { prepare (48000.0, 512); }`, with every member at its C++ initialiser.
    pub fn new() -> Self {
        let mut d = TwineDsp {
            in_peak: 0.0,
            out_true_peak: 0.0,
            clip_gr: 0.0,
            lim_gr: 0.0,
            band_gr: [0.0; 3],
            sr: 48000.0,
            drive_knob: 0.0,
            clip_knob: 0.5,
            ceiling_knob: 11.0f32 / 12.0f32,
            release_knob: 0.5654,
            shape: SHAPE_SOFT,
            character: CHAR_TRANSPARENT,
            match_: false,
            chaff: false,
            bypass: false,
            os_h: Vec::new(),
            up_branch: Vec::new(),
            tp_branch: Vec::new(),
            tp_scratch: Vec::new(),
            os_cut: 0.117,
            os_taps: 257,
            up_taps: 65,
            os_delay: 64,
            tp_taps: 33,
            tp_taps_total: 257,
            tp_delay: 16,
            look_max: 240,
            look: 240,
            latency: 309,
            up_hist: Default::default(),
            down_hist: Default::default(),
            tp_hist: Default::default(),
            out_tp_hist: Default::default(),
            wet_delay: Default::default(),
            dry_delay: Default::default(),
            up_pos: 0,
            down_pos_for: [0, 0],
            down_pos: 0,
            tp_pos: 0,
            out_tp_pos: 0,
            wet_pos: 0,
            dry_pos: 0,
            wide: LimiterCore::default(),
            band_lim: Default::default(),
            multi: false,
            x_lo_knob: 0.4106,
            x_hi_knob: 0.4881,
            x_lo: 150.0,
            x_hi: 3000.0,
            xover: [Crossover::default(); 2],
            band_delay: Default::default(),
            tp2_hist: Default::default(),
            band_env: [0.0; 3],
            sum_env: 0.0,
            aim_now: 1.0,
            band_pos: 0,
            tp_pos2: 0,
            stage_delay: 255,
            latency_single: 319,
            latency_multi: 574,
            scope: Box::new([0.0; SCOPE_SIZE as usize]),
            scope_write: 0,
            scope_w: 0,
            drive_s: 1.0,
            ceil_s: 1.0,
            byp_s: 0.0,
            smooth_init: false,
        };
        d.prepare(48000.0, 512);
        d
    }

    /// Allocates (or, at an unchanged rate, re-fills in place) every buffer.
    pub fn prepare(&mut self, sample_rate: f64, max_block: i32) {
        self.sr = if sample_rate > 0.0 { sample_rate } else { 48000.0 };
        let _ = max_block; // juce_unused (maxBlock)

        self.build_filters();

        self.look_max = (f64::from(lookahead_ms(CHAR_TRANSPARENT)) * 0.001 * self.sr).ceil() as i32;
        self.stage_delay = self.tp_delay + self.look_max - 1;
        self.latency_single = self.os_delay + self.stage_delay;
        self.latency_multi = self.latency_single + self.stage_delay;
        self.latency = if self.multi { self.latency_multi } else { self.latency_single };

        for c in 0..2 {
            assign(&mut self.up_hist[c], self.up_taps as usize, 0.0);
            assign(&mut self.down_hist[c], self.os_taps as usize, 0.0);
            assign(&mut self.tp_hist[c], self.tp_taps as usize, 0.0);
            assign(&mut self.tp2_hist[c], self.tp_taps as usize, 0.0);
            for bd in &mut self.band_delay[c] {
                assign(bd, (self.stage_delay + 8) as usize, 0.0f32);
            }
            assign(&mut self.out_tp_hist[c], self.tp_taps as usize, 0.0);
            assign(&mut self.wet_delay[c], (self.tp_delay + self.look_max + 8) as usize, 0.0f32);
            assign(&mut self.dry_delay[c], (self.latency_multi + 1) as usize, 0.0f32); // long enough for either mode
        }
        self.up_pos = 0;
        self.down_pos = 0;
        self.tp_pos = 0;
        self.out_tp_pos = 0;
        self.down_pos_for = [0, 0];
        self.wet_pos = 0;
        self.dry_pos = 0;
        self.band_pos = 0;
        self.band_env = [0.0; 3];
        self.sum_env = 0.0;
        self.update_crossover();
        for x in &mut self.xover {
            *x = Crossover::default();
        }
        self.update_crossover();

        self.reset_limiters();

        self.byp_s = if self.bypass { 1.0 } else { 0.0 };
        self.smooth_init = false;
    }

    pub fn latency_samples(&self) -> i32 {
        self.latency
    }

    pub fn set_parameter(&mut self, index: usize, v: f32) {
        match index {
            P_DRIVE => self.drive_knob = v,
            P_CLIP => self.clip_knob = v,
            P_SHAPE => self.shape = clamp_c(lround(v), 0, NUM_SHAPES - 1),
            P_CHARACTER => {
                let c = clamp_c(lround(v), 0, NUM_CHARACTERS - 1);
                if c != self.character {
                    self.character = c;
                    self.reset_limiters();
                }
            }
            P_CEILING => self.ceiling_knob = v,
            P_RELEASE => self.release_knob = v,
            P_MATCH => self.match_ = v > 0.5,
            P_CHAFF => self.chaff = v > 0.5,
            P_BYPASS => self.bypass = v > 0.5,
            P_MULTI => {
                let m = v > 0.5;
                if m != self.multi {
                    self.multi = m;
                    self.latency = if self.multi { self.latency_multi } else { self.latency_single };
                    self.reset_limiters();
                }
            }
            P_XOVER_LO => {
                self.x_lo_knob = v;
                self.update_crossover();
            }
            P_XOVER_HI => {
                self.x_hi_knob = v;
                self.update_crossover();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ audio
    /// Processes `num_samples` of the first `num_channels` buffers in place. `scope_out`
    /// (Rust addition) receives each scope sample as the C++ writes it to `scope`.
    pub fn process(&mut self, ch: &mut [Vec<f32>], num_channels: usize, num_samples: usize, scope_out: Option<&AnalysisRing>) {
        // `std::clamp (numChannels, 1, 2)`; with no buffer at all there is nothing to read.
        let num_channels = clamp_c(num_channels, 1, 2).min(ch.len());
        if num_channels == 0 {
            return;
        }
        let num_samples = ch[..num_channels].iter().map(Vec::len).fold(num_samples, usize::min);

        let drive = 10.0f64.powf(f64::from(knob_to_drive_db(self.drive_knob)) / 20.0);
        let ceiling = 10.0f64.powf(f64::from(knob_to_ceiling_db(self.ceiling_knob)) / 20.0);
        let clip_on = self.clip_knob > 0.0005f32;
        let clip_t = ceiling * 10.0f64.powf(f64::from(clip_headroom_db(self.clip_knob)) / 20.0);
        // float * float, then widened, as the C++
        let rel_ms = f64::from(knob_to_release_ms(self.release_knob) * release_scale(self.character));
        let rel_a = 1.0 - (-1.0 / cmax(1.0, rel_ms * 0.001 * self.sr)).exp();
        let out_gain = if self.match_ { 1.0 / drive } else { 1.0 };

        if !self.smooth_init {
            self.drive_s = drive;
            self.ceil_s = ceiling;
            self.smooth_init = true;
        }

        let mut in_pk = 0.0f32;
        let mut out_tp = 0.0f32;
        let mut clip_max = 0.0f32;
        let mut lim_min = 1.0f32;
        let mut band_min = [1.0f64; 3];

        let last = num_channels - 1;
        for n in 0..num_samples {
            self.drive_s += (drive - self.drive_s) * 0.002;
            self.ceil_s += (ceiling - self.ceil_s) * 0.002;

            let mut post = [0.0f64; 2];
            let mut dry_d = [0.0f64; 2];
            let mut tp_now = 0.0f64;

            for c in 0..num_channels {
                let x = f64::from(ch[c][n]);
                in_pk = cmax(in_pk, x.abs() as f32);

                // the dry path, for Bypass and Chaff, delayed to line up with everything
                {
                    let dd = &mut self.dry_delay[c];
                    let size = dd.len() as i32;
                    dd[self.dry_pos as usize] = x as f32;
                    dry_d[c] = f64::from(dd[((self.dry_pos - self.latency + size * 2) % size) as usize]);
                }

                // ---- drive, then the clipper at 4x
                let xd = x * self.drive_s;
                let mut y = 0.0f64;
                self.up_push(c, xd);
                for ph in 0..4 {
                    let u = self.up_phase(c, ph);
                    let mut v = u;
                    if clip_on {
                        let t = clip_t * self.ceil_s / cmax(1.0e-9, ceiling); // follows a moving ceiling smoothly
                        v = self.clip_sample(u, t);
                        if u.abs() > t {
                            clip_max = cmax(clip_max, (20.0 * (u.abs() / cmax(1.0e-9, v.abs())).log10()) as f32);
                        }
                    }
                    self.down_push(c, v);
                    // decimate on phase 0: that keeps the whole round trip an exact 64 base
                    // samples, so the dry path (Bypass, Chaff) lines up to the sample
                    if ph == 0 {
                        y = self.down_out(c);
                    }
                }
                post[c] = y;

                // ---- the true peak of what the limiter will be given
                let tp = Self::true_peak(&self.tp_branch, self.tp_taps, &mut self.tp_hist[c], self.tp_pos, y);
                tp_now = cmax(tp_now, tp);
            }
            if num_channels == 1 {
                post[1] = post[0];
                dry_d[1] = dry_d[0];
            }
            self.tp_pos = (self.tp_pos + 1) % self.tp_taps;
            self.up_pos = (self.up_pos + 1) % self.up_taps;

            // float + float, then the double 0.5, then back to float, as the C++
            let sc = (0.5 * f64::from(ch[0][n] + ch[last][n])) as f32;
            self.scope[(self.scope_w & (SCOPE_SIZE - 1)) as usize] = sc;
            self.scope_w = self.scope_w.wrapping_add(1);
            self.scope_write = self.scope_w;
            // Rust addition: the module's analysis stream, from the same sample.
            if let Some(ring) = scope_out {
                ring.push(sc);
            }

            let aim = self.ceil_s * 0.98855; // -0.1 dB: see below

            // ---- Multiband: the bands take the reduction first, shared out by who made the
            // peak; what comes out goes on to the full-band limiter as if it were the input
            if self.multi {
                let need_all = if tp_now > aim { aim / tp_now } else { 1.0 };
                self.aim_now = aim;
                self.multiband_step(&mut post, need_all, tp_now, rel_a, &mut band_min);

                tp_now = 0.0;
                for (c, &p) in post.iter().enumerate().take(num_channels) {
                    let tp = Self::true_peak(&self.tp_branch, self.tp_taps, &mut self.tp2_hist[c], self.tp_pos2, p);
                    tp_now = cmax(tp_now, tp);
                }
                self.tp_pos2 = (self.tp_pos2 + 1) % self.tp_taps;
            }

            // ---- the limiter: what gain this moment needs, and the guarantee (see the top)
            // aimed a tenth of a dB under the ceiling: the 8x detector can still read a bright
            // peak up to about that much low (measured), and this is the one number a master
            // limiter is not allowed to get wrong
            let need = if tp_now > aim { aim / tp_now } else { 1.0 };
            let g = self.wide.step(need, rel_a);
            lim_min = cmin(lim_min, g as f32);

            // ---- the post-clip audio, delayed to meet its gain
            let mut out = [0.0f64; 2];
            for c in 0..2 {
                let wd = &mut self.wet_delay[c];
                let size = wd.len() as i32;
                wd[self.wet_pos as usize] = post[c] as f32;
                let back = self.tp_delay + self.look_max - 1;
                let w = f64::from(wd[((self.wet_pos - back + size * 4) % size) as usize]);

                let mut v = w * g;
                v = clamp_c(v, -self.ceil_s, self.ceil_s); // the last sample-peak catch

                if self.chaff {
                    v = dry_d[c] * self.drive_s - v; // what was knocked off
                }
                v *= out_gain;
                out[c] = v;
            }
            self.wet_pos = (self.wet_pos + 1) % self.wet_delay[0].len() as i32;
            self.dry_pos = (self.dry_pos + 1) % self.dry_delay[0].len() as i32;

            // ---- bypass, crossfaded, at the same latency
            self.byp_s += ((if self.bypass { 1.0 } else { 0.0 }) - self.byp_s) * 0.005;
            for c in 0..num_channels {
                let v = out[c] + (dry_d[c] - out[c]) * self.byp_s;
                ch[c][n] = v as f32;
                let tp = Self::true_peak(&self.tp_branch, self.tp_taps, &mut self.out_tp_hist[c], self.out_tp_pos, v);
                out_tp = cmax(out_tp, tp as f32);
            }
            self.out_tp_pos = (self.out_tp_pos + 1) % self.tp_taps;
        }

        raise_max(&mut self.in_peak, in_pk);
        raise_max(&mut self.out_true_peak, out_tp);
        raise_max(&mut self.clip_gr, clip_max);
        // In Multiband the "limit" reading is the deepest any part of the limiting went: the
        // full-band stage plus the band that worked hardest.
        let deepest_band = cmin(band_min[0], cmin(band_min[1], band_min[2]));
        let lim = f64::from(lim_min) * if self.multi { deepest_band } else { 1.0 };
        raise_max(&mut self.lim_gr, (-20.0 * cmax(1.0e-6, lim).log10()) as f32);
        for (gr, &bm) in self.band_gr.iter_mut().zip(&band_min) {
            raise_max(gr, if self.multi { (-20.0 * cmax(1.0e-6, bm).log10()) as f32 } else { 0.0 });
        }
    }

    /// The clipper's curve, public for the test and the display. t is the clip level.
    pub fn clip_sample(&self, x: f64, t: f64) -> f64 {
        let a = x.abs();
        let s = if x < 0.0 { -1.0 } else { 1.0 };
        if self.shape == SHAPE_HARD {
            return if a > t { s * t } else { x };
        }

        // Soft and Tape: linear up to a knee, then a tanh bend that arrives at t and never
        // passes it. Tape bends earlier and rounder.
        let knee = (if self.shape == SHAPE_SOFT { 0.75 } else { 0.45 }) * t;
        if a <= knee {
            return x;
        }
        let room = t - knee;
        s * (knee + room * ((a - knee) / room).tanh())
    }

    // ------------------------------------------------------------------ Rust accessors
    /// The meters, as the editor's `take` on each: the running maxima since the last call, then
    /// zeroed (`exchange (0)`).
    pub fn take_meters(&mut self) -> Meters {
        let m =
            Meters { in_peak: self.in_peak, out_true_peak: self.out_true_peak, clip_gr: self.clip_gr, lim_gr: self.lim_gr, band_gr: self.band_gr };
        self.in_peak = 0.0;
        self.out_true_peak = 0.0;
        self.clip_gr = 0.0;
        self.lim_gr = 0.0;
        self.band_gr = [0.0; 3];
        m
    }

    /// `copyScope`: the last `n` input samples (mono), for the crossover strip's spectrum.
    pub fn copy_scope(&self, out: &mut [f32]) {
        let n = cmin(out.len() as i32, SCOPE_SIZE);
        let w = self.scope_write;
        for i in 0..n {
            out[i as usize] = self.scope[((w - n + i) & (SCOPE_SIZE - 1)) as usize];
        }
    }

    pub fn xover_lo_hz(&self) -> f32 {
        self.x_lo
    }
    pub fn xover_hi_hz(&self) -> f32 {
        self.x_hi
    }
    pub fn is_multiband(&self) -> bool {
        self.multi
    }
    pub fn sample_rate(&self) -> f64 {
        self.sr
    }
    /// `latencySingle` / `latencyMulti` for the prepared rate.
    pub fn latencies(&self) -> (i32, i32) {
        (self.latency_single, self.latency_multi)
    }
    /// Capacity and address of every heap buffer (allocation guard in tests).
    #[cfg(test)]
    pub(crate) fn buffer_fingerprint(&self) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        let mut add = |b: &[f64]| v.push((b.as_ptr() as usize, b.len()));
        add(&self.os_h);
        for b in self.up_branch.iter().chain(&self.tp_branch).chain(&self.up_hist).chain(&self.down_hist) {
            add(b);
        }
        for b in self.tp_hist.iter().chain(&self.out_tp_hist).chain(&self.tp2_hist) {
            add(b);
        }
        for l in std::iter::once(&self.wide).chain(&self.band_lim) {
            add(&l.held);
            add(&l.gain_delay);
        }
        for b in self.wet_delay.iter().chain(&self.dry_delay).chain(self.band_delay.iter().flatten()) {
            v.push((b.as_ptr() as usize, b.capacity()));
        }
        v
    }

    // ------------------------------------------------------------------ filters
    fn build_filters(&mut self) {
        // The oversampler: 257 taps at 4x, cut at the base rate's Nyquist, ~80 dB stopband.
        // Flat to about 20 kHz at 44.1; group delay 128 at 4x = 32 base samples, each way.
        self.os_taps = 257;
        design(&mut self.os_h, self.os_taps, self.os_cut, 7.86, 1.0);
        self.up_taps = (self.os_taps + 3) / 4; // per polyphase branch
        self.os_delay = 2 * ((self.os_taps - 1) / 2) / 4; // up and down: 64 base samples

        self.up_branch.resize_with(4, Vec::new);
        for b in &mut self.up_branch {
            assign(b, self.up_taps as usize, 0.0);
        }
        for i in 0..self.os_taps {
            self.up_branch[(i % 4) as usize][(i / 4) as usize] = self.os_h[i as usize] * 4.0; // zero-stuffing costs 4x
        }

        // The true-peak interpolator, 8x. BS.1770's reference is 4x with 48 taps, which is a
        // METER's accuracy: looking at four points per sample can miss the top of a bright
        // peak by up to ~0.4 dB, and a limiter steering by it lets exactly that much through
        // (measured: it did). Eight points and a long, steep filter miss by under 0.1.
        self.tp_taps_total = 257;
        design(&mut self.tp_scratch, self.tp_taps_total, 1.0 / (2.0 * f64::from(TP_OVER)), 8.0, f64::from(TP_OVER));
        self.tp_taps = (self.tp_taps_total + TP_OVER - 1) / TP_OVER;
        self.tp_branch.resize_with(TP_OVER as usize, Vec::new);
        for b in &mut self.tp_branch {
            assign(b, self.tp_taps as usize, 0.0);
        }
        for i in 0..self.tp_taps_total {
            self.tp_branch[(i % TP_OVER) as usize][(i / TP_OVER) as usize] = self.tp_scratch[i as usize];
        }
        self.tp_delay = ((self.tp_taps_total - 1) / 2) / TP_OVER; // 16 base samples
    }

    // ---- up: one base sample in, four out
    fn up_push(&mut self, c: usize, x: f64) {
        let p = self.up_pos_for(c) as usize;
        self.up_hist[c][p] = x;
    }
    fn up_pos_for(&self, _c: usize) -> i32 {
        self.up_pos
    }

    fn up_phase(&self, c: usize, ph: usize) -> f64 {
        let h = &self.up_hist[c];
        let b = &self.up_branch[ph];
        let mut s = 0.0f64;
        let mut k = self.up_pos;
        for bi in b.iter().take(self.up_taps as usize) {
            s += bi * h[k as usize];
            k = if k == 0 { self.up_taps - 1 } else { k - 1 };
        }
        s
    }

    // ---- down: four in, one out (only the kept output is computed)
    fn down_push(&mut self, c: usize, v: f64) {
        let p = self.down_pos_for[c];
        self.down_hist[c][p as usize] = v;
        self.down_pos_for[c] = (p + 1) % self.os_taps;
    }

    fn down_out(&self, c: usize) -> f64 {
        let h = &self.down_hist[c];
        let mut s = 0.0f64;
        let mut k = (self.down_pos_for[c] - 1 + self.os_taps) % self.os_taps;
        for hi in self.os_h.iter().take(self.os_taps as usize) {
            s += hi * h[k as usize];
            k = if k == 0 { self.os_taps - 1 } else { k - 1 };
        }
        s
    }

    /// Push a sample into a true-peak history and return the largest of its eight phases. The
    /// caller advances `pos` once per sample for all channels. (An associated function taking
    /// the branches, so the caller can lend one of `self`'s histories mutably.)
    fn true_peak(tp_branch: &[Vec<f64>], tp_taps: i32, h: &mut [f64], pos: i32, x: f64) -> f64 {
        h[pos as usize] = x;
        let mut best = 0.0f64;
        for b in tp_branch.iter().take(TP_OVER as usize) {
            let mut s = 0.0f64;
            let mut k = pos;
            for bi in b.iter().take(tp_taps as usize) {
                s += bi * h[k as usize];
                k = if k == 0 { tp_taps - 1 } else { k - 1 };
            }
            best = cmax(best, s.abs());
        }
        best
    }

    fn reset_limiters(&mut self) {
        let look = clamp_c((f64::from(lookahead_ms(self.character)) * 0.001 * self.sr).ceil() as i32, 1, cmax(1, self.look_max));
        self.wide.reset(look, self.look_max);
        for b in &mut self.band_lim {
            b.reset(look, self.look_max);
        }
    }

    fn update_crossover(&mut self) {
        self.x_lo = knob_to_xover_lo(self.x_lo_knob);
        self.x_hi = cmax(knob_to_xover_hi(self.x_hi_knob), self.x_lo * 2.0f32);
        let lp1 = butter(f64::from(self.x_lo), self.sr, false);
        let hp1 = butter(f64::from(self.x_lo), self.sr, true);
        let lp2 = butter(f64::from(self.x_hi), self.sr, false);
        let hp2 = butter(f64::from(self.x_hi), self.sr, true);
        let ap = allpass(f64::from(self.x_hi), self.sr);
        for x in &mut self.xover {
            x.lo1a.set_coefs(&lp1);
            x.lo1b.set_coefs(&lp1);
            x.hi1a.set_coefs(&hp1);
            x.hi1b.set_coefs(&hp1);
            x.lo2a.set_coefs(&lp2);
            x.lo2b.set_coefs(&lp2);
            x.hi2a.set_coefs(&hp2);
            x.hi2b.set_coefs(&hp2);
            x.ap2.set_coefs(&ap);
        }
    }

    /// One sample of the band stage, in place on `io` (both channels).
    ///
    /// How the reduction is shared. The full-band detector says how much has to come off this
    /// moment (needAll). Each band's recent peak level says how loud it is; a band's share of
    /// the reduction is its share of the POWER, so the band actually making the peak takes
    /// almost all of it: a kick at +6 dB over a vocal takes about 94 % of the cut. The band's
    /// own gain is then whatever takes its share off its own level -- and each band's gain runs
    /// through the same lookahead and release as the full-band limiter.
    fn multiband_step(&mut self, io: &mut [f64; 2], need_all: f64, tp_all: f64, rel_a: f64, band_min: &mut [f64; 3]) {
        let mut b = [[0.0f64; 2]; 3];
        for c in 0..2 {
            let mut o = [0.0f64; 3];
            self.xover[c].split(io[c], &mut o);
            for i in 0..3 {
                b[i][c] = o[i];
            }
        }

        // each band's recent peak, and the peak of their sum: instant attack, ~20 ms fall.
        //
        // How much has to come off is measured HERE, on the bands' own sum, not taken from the
        // full-band detector. The crossover's low band lags the full-band signal by a couple of
        // milliseconds, so the full-band figure says "take 8 dB off" before the kick has shown
        // up in the low band -- and the only band loud enough to be handed that cut is the one
        // with the vocal in it. The lookahead then holds that mistake for the whole release.
        // (Measured: the vocal dipped 13.8 dB on every kick, twice as much as with no bands.)
        let fall = (-1.0 / (0.020 * self.sr)).exp();
        let mut sum_sq = 0.0f64;
        for i in 0..3 {
            let a = cmax(b[i][0].abs(), b[i][1].abs());
            self.band_env[i] = if a > self.band_env[i] { a } else { self.band_env[i] * fall };
            sum_sq += self.band_env[i] * self.band_env[i];
        }
        {
            let a = cmax((b[0][0] + b[1][0] + b[2][0]).abs(), (b[0][1] + b[1][1] + b[2][1]).abs());
            self.sum_env = if a > self.sum_env { a } else { self.sum_env * fall };
        }
        let _ = (need_all, tp_all); // juce_unused (needAll); juce_unused (tpAll);
        let remove = cmax(0.0, self.sum_env - self.aim_now); // amplitude that has to come off
        for i in 0..3 {
            let mut need_i = 1.0f64;
            if remove > 0.0 && sum_sq > 1.0e-18 && self.band_env[i] > 1.0e-9 {
                let share = self.band_env[i] * self.band_env[i] / sum_sq;
                need_i = clamp_c(1.0 - remove * share / self.band_env[i], 0.05, 1.0);
            }
            let g = self.band_lim[i].step(need_i, rel_a);
            band_min[i] = cmin(band_min[i], g);

            // the band's audio, delayed to meet its gain
            for c in 0..2 {
                let d = &mut self.band_delay[c][i];
                let size = d.len() as i32;
                d[self.band_pos as usize] = b[i][c] as f32;
                b[i][c] = f64::from(d[((self.band_pos - self.stage_delay + size * 4) % size) as usize]) * g;
            }
        }
        self.band_pos = (self.band_pos + 1) % self.band_delay[0][0].len() as i32;

        for c in 0..2 {
            io[c] = b[0][c] + b[1][c] + b[2][c];
        }
    }
}
