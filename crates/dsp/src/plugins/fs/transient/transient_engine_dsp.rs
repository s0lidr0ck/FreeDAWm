//! TransientEngineDSP -- Engine 10: attack and sustain, on anything.
//! Line-by-line translation of `Engine 10 - Transient/Source/TransientEngineDSP.h` (class
//! `fs::transient::TransientEngineDSP`, in the header's order), v2 (2026-10-21).
//!
//! A transient designer that reacts to the SHAPE of the envelope, not its level. v2's bar is
//! transparency: you hear the shape change, never the gain moving. v1 applied a gain recomputed
//! every sample from a peak follower, unsmoothed, so it rode the waveform (static at Sustain past
//! about -4 dB on a piano; attack steps up to 9.3 dB in one sample). v2's detectors, per sample
//! (the louder channel's squared value of each, before any smoothing):
//!
//! ```text
//! L    the sustain band's mean square, two cascaded levelMs one-poles, in dB
//! on   the attack band's 1 ms (twice) mean square 6 dB over its 20 ms one: a 10 ms onset window
//! P    L's peak since the last onset (P = L inside the window), forgotten over memoryMs
//! g    sustain gain: Sustain x clamp((P - L) / (susFull x Sustain Time)), gainMs glide
//!      (0 dB with a 3 ms glide inside the onset window)
//! ar   the attack band pre-emphasised (c - cPrev): fastMs (twice) mean square over a
//!      slowAttMs x Attack Time one, in dB
//! ga   attack gain: Attack x the shape (ar past a 2.5 dB dead zone, full attFullDb on), held
//!      (instant up, holdMs down), gliding 3 ms up and 1 ms down
//! ```
//!
//! A source may also open the onset window from the attack ratio (`onsetAttackDb`) and let the
//! sustain cut go in a straight line (`onsetRampMs`) instead of the 3 ms one-pole
//! (`onsetReleaseMs`): Drums and Percussion do, so a hit into the last one's ring keeps its front.
//! After a Source change, onsets and attacks are ignored for two slow windows (the new bands
//! settle), and every one-pole state flushes to 0 below 1e-30 (no denormal crawl in silence).
//!
//! Both gains fade out below -90 dBFS of L over 10 dB. Two detectors (the attack in the source's
//! transient band, the sustain in the whole note), a Source that sets the bands, times and
//! full-scale ratios, and a Focus filter per gain (full band, RBJ low shelf, bell or high shelf in
//! Direct Form I, its gain moving every sample). Stereo is linked (one set of detectors, one gain
//! on both). No oversampling.
//!
//! **3 ms lookahead**: the detectors read the input as it arrives and the gains are applied to the
//! input `round(0.003 * sr)` samples later (a delay line sized in `prepare`; nothing is allocated
//! in `process`), so a 3 ms attack build-up lands on the transient. The latency is reported
//! always, bypassed or not. Every use of the dry input (mix, the bypass glide, identity) is the
//! delayed input: with both amounts at zero the output is the input delayed, sample for sample.
//!
//! Translation notes (C++ behaviour reproduced deliberately):
//! - `float` and `double` exactly where the C++ has them: the knobs and their unit maps are `f32`
//!   (`std::pow (4.0f, ..)` and `std::pow (10.0f, ..)` are `powf`); detectors, smoothers, filters
//!   and the Focus maths are `f64`; the per-channel output is computed in `f64` and cast to `f32`
//!   where the C++ casts. `float` operands promoted to `double` (`sp.susFullDb * susScale`,
//!   `kMidQ`, ...) are promoted here at the same point (`f64::from`).
//! - `std::max` / `std::min` and the header's `clampT` are the C++ comparisons (`cmax`, `cmin`,
//!   `clamp_t`), so a NaN goes the way it goes there.
//! - `(int) std::lround (x)` rounds half away from zero (`f64::round`), then truncates to `int`.
//!   `int (v + 0.5f)` of a choice knob truncates toward zero (`as i32`); the module only ever
//!   hands the engine finite values.
//! - The scope ring (`std::array<ScopeBin, 2500>` plus an atomic write index the editor reads) is
//!   a `Vec` made once in [`TransientEngineDsp::new`] and a plain index; each finished bin is also
//!   pushed, as four values (`inPeak`, `outPeak`, `attDb`, `susDb`), to the analysis ring the
//!   module hands in, which is how the UI's scope reads it.
//! - The delay line (`std::vector<float> delayLine[2]`, `assign`ed in `prepare`) is two `Vec`s,
//!   resized in [`TransientEngineDsp::prepare`] (no allocation once they have the capacity) and
//!   kept, like the scope ring, across [`TransientEngineDsp::restart`].
//! - Rust-only: `process` also keeps the block's most extreme attack and sustain gain (the same
//!   "largest magnitude wins" rule the scope bins use), and the detectors' levels are readable,
//!   for the module's readouts. They are read, never fed back.
//! - `ScopedNoDenormals` is the wrapper's: see the module docs.

// Constants and literals are kept exactly as the C++ writes them.
#![allow(clippy::excessive_precision, clippy::approx_constant)]
// The C++ clamps and branches written as the C++ writes them.
#![allow(clippy::manual_clamp, clippy::needless_range_loop, clippy::if_same_then_else)]

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
/// `flushT (v)`: a one-pole's state below 1e-30 becomes 0, so a long silence never leaves the
/// detectors decaying through denormals (no flush-to-zero in Rust; the C++ wrapper's
/// `ScopedNoDenormals` does not reach the tests either).
#[inline]
fn flush(v: f64) -> f64 {
    if v.abs() < K_DENORMAL_FLOOR { 0.0 } else { v }
}
const K_DENORMAL_FLOOR: f64 = 1.0e-30;

/// `(int) std::lround (x)`: half away from zero, then the `long` → `int` truncation.
#[inline]
fn lround_i32(x: f64) -> i32 {
    x.round() as i32
}

// ---------------------------------------------------------------- enums
pub const S_DRUMS: i32 = 0;
pub const S_PERCUSSION: i32 = 1;
pub const S_GUITAR: i32 = 2;
pub const S_PIANO: i32 = 3;
pub const S_BASS: i32 = 4;
pub const S_VOCAL: i32 = 5;
pub const S_FULL_MIX: i32 = 6;
pub const NUM_SOURCES: i32 = 7;

pub const F_FULL: i32 = 0;
pub const F_LOW: i32 = 1;
pub const F_MID: i32 = 2;
pub const F_HIGH: i32 = 3;
pub const NUM_FOCUS: i32 = 4;

// ---------------------------------------------------------------- parameters (order == Parameters.h)
pub const P_SOURCE: usize = 0;
pub const P_ATTACK: usize = 1;
pub const P_SUSTAIN: usize = 2;
pub const P_ATTACK_TIME: usize = 3;
pub const P_SUSTAIN_TIME: usize = 4;
pub const P_ATTACK_FOCUS: usize = 5;
pub const P_SUSTAIN_FOCUS: usize = 6;
pub const P_MIX: usize = 7;
pub const P_OUTPUT: usize = 8;
pub const P_BYPASS: usize = 9;
pub const NUM_PARAMS: usize = 10;

// ---------------------------------------------------------------- unit maps
/// -18 .. +18 dB at a full-strength onset.
pub fn knob_to_attack_db(v: f32) -> f32 {
    36.0f32 * v - 18.0f32
}
/// -24 .. +24 dB at a full-strength decay.
pub fn knob_to_sustain_db(v: f32) -> f32 {
    48.0f32 * v - 24.0f32
}
/// x0.25 .. x4 the source's attack length / sustain onset, 0.5 = x1.
pub fn knob_to_time_scale(v: f32) -> f32 {
    4.0f32.powf(2.0f32 * v - 1.0f32)
}
/// -12 .. +12 dB.
pub fn knob_to_out_db(v: f32) -> f32 {
    -12.0f32 + 24.0f32 * v
}

pub fn attack_knob_from_db(db: f32) -> f32 {
    clamp_t((db + 18.0f32) / 36.0f32, 0.0f32, 1.0f32)
}
pub fn sustain_knob_from_db(db: f32) -> f32 {
    clamp_t((db + 24.0f32) / 48.0f32, 0.0f32, 1.0f32)
}

/// What each source listens for, and over what time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceSpec {
    /// The attack detector's band.
    pub det_low_hz: f32,
    pub det_high_hz: f32,
    /// The sustain detector's: the whole note.
    pub sus_low_hz: f32,
    pub sus_high_hz: f32,
    /// The sustain level's two one-poles, and the sustain gain's glide.
    pub level_ms: f32,
    pub gain_ms: f32,
    /// The attack detector's fast (twice) and slow mean squares -- the length of "the attack".
    pub fast_ms: f32,
    pub slow_att_ms: f32,
    /// How long the attack shape is held, and how long the peak memory remembers.
    pub hold_ms: f32,
    pub memory_ms: f32,
    /// An onset this far past the dead zone is full-strength; a note this far down is all sustain.
    pub att_full_db: f32,
    pub sus_full_db: f32,
    /// How the sustain gain lets go when a note starts: a one-pole of onsetReleaseMs, or, when
    /// onsetRampMs > 0, a straight line in dB reaching 0 dB in that time.
    pub onset_release_ms: f32,
    pub onset_ramp_ms: f32,
    /// > 0: the pre-emphasised attack ratio past this many dB also opens the onset window.
    pub onset_attack_db: f32,
}

const fn spec_row(r: [f32; 15]) -> SourceSpec {
    SourceSpec {
        det_low_hz: r[0],
        det_high_hz: r[1],
        sus_low_hz: r[2],
        sus_high_hz: r[3],
        level_ms: r[4],
        gain_ms: r[5],
        fast_ms: r[6],
        slow_att_ms: r[7],
        hold_ms: r[8],
        memory_ms: r[9],
        att_full_db: r[10],
        sus_full_db: r[11],
        onset_release_ms: r[12],
        onset_ramp_ms: r[13],
        onset_attack_db: r[14],
    }
}

/// `sourceSpecs`, the rows in the struct's member order (`detLowHz, detHighHz, susLowHz,
/// susHighHz, levelMs, gainMs, fastMs, slowAttMs, holdMs, memoryMs, attFullDb, susFullDb,
/// onsetReleaseMs, onsetRampMs, onsetAttackDb`). Fix round 1: Drums and Percussion let a ringing
/// sustain cut go in a 1.25 ms straight line and also hear onsets in the attack ratio (4 dB), so a
/// hit into the last one's ring keeps its front; the others keep the approved 3 ms one-pole.
/// v2 times: Piano is the owner-approved prototype (by ear, 2026-10-21); the others were tuned on
/// the owner's stems at Attack +12 / Sustain -12 (gain motion above 500 Hz at least 60 dB under
/// the output, onsets against an HF-flux list, a drum's snap kept). Bands, slowAttMs, memoryMs
/// and the two full-scale ratios are v1's.
pub static SOURCE_SPECS: [SourceSpec; 7] = [
    //        attack band       sustain band      level   gain    fast  slowAtt  hold  memory   attFull susFull  onsetRelease onsetRamp onsetAttack
    spec_row([40.0, 12000.0, 40.0, 12000.0, 12.0, 36.0, 1.0, 10.0, 40.0, 800.0, 12.0, 12.0, 3.0, 1.25, 4.0]), // Drums       whole kit or one drum
    spec_row([200.0, 14000.0, 150.0, 14000.0, 8.0, 24.0, 1.0, 6.0, 30.0, 500.0, 12.0, 4.0, 3.0, 1.25, 4.0]), // Percussion  hand drums, claps, shakers
    spec_row([1200.0, 8000.0, 70.0, 8000.0, 17.0, 51.0, 1.0, 8.0, 40.0, 2000.0, 8.0, 2.0, 3.0, 0.0, 0.0]), // Guitar      attack from the pick, sustain from the body
    spec_row([80.0, 6000.0, 60.0, 8000.0, 20.0, 60.0, 1.0, 18.0, 25.0, 4000.0, 6.0, 8.0, 3.0, 0.0, 0.0]),  // Piano       hammer, then a long ring
    spec_row([40.0, 2500.0, 35.0, 3000.0, 25.0, 75.0, 1.5, 20.0, 40.0, 1500.0, 8.0, 15.0, 3.0, 0.0, 0.0]), // Bass        pluck and pick on the low end
    spec_row([300.0, 6000.0, 90.0, 8000.0, 20.0, 60.0, 1.0, 20.0, 30.0, 1000.0, 6.0, 4.0, 3.0, 0.0, 0.0]), // Vocal       consonants against vowels
    spec_row([40.0, 12000.0, 40.0, 12000.0, 20.0, 60.0, 1.0, 15.0, 40.0, 800.0, 8.0, 12.0, 3.0, 0.0, 0.0]), // Full Mix    gentle, on everything at once
];

/// Where each Focus lands: a shelf or band of this width (`float` in the C++).
pub const K_LOW_HZ: f32 = 220.0;
pub const K_MID_HZ: f32 = 1000.0;
pub const K_MID_Q: f32 = 0.55;
pub const K_HIGH_HZ: f32 = 2500.0;

/// The lookahead: the gains reach the audio this long after the detectors hear it.
pub const K_LOOKAHEAD_MS: f64 = 3.0;

pub fn spec(s: i32) -> &'static SourceSpec {
    let i = clamp_t(s, 0, NUM_SOURCES - 1) as usize;
    SOURCE_SPECS.get(i).unwrap_or(&SOURCE_SPECS[0])
}

// ---------------------------------------------------------------- the editor's scope
/// One 4 ms bin: input and output peaks, and the most extreme attack and sustain gain in it. The
/// input peak is the delayed input's, so it lines up with the output and the gains.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScopeBin {
    pub in_peak: f32,
    pub out_peak: f32,
    pub att_db: f32,
    pub sus_db: f32,
}
/// Ten seconds of 4 ms bins.
pub const SCOPE_BINS: i32 = 2500;
/// The values a bin is published to the analysis ring as: in, out, attack dB, sustain dB.
pub const SCOPE_BIN_VALUES: usize = 4;

const PI: f64 = 3.14159265358979323846;
const K_FLOOR_DB: f64 = -90.0;
/// On squared values.
const K_TINY: f64 = 1.0e-18;
/// The attack band's fast level this far over its slow one is a new note,
const K_ONSET_DB: f64 = 6.0;
/// and the window it opens.
const K_ONSET_MS: f64 = 10.0;
const K_ONSET_FAST_MS: f64 = 1.0;
const K_ONSET_SLOW_MS: f64 = 20.0;
/// After a Source change, onsets and attacks are ignored for this many slow windows.
const K_SWITCH_WINDOWS: f64 = 2.0;
/// An attack ratio this small is a ringing note's own wobble.
const K_DEAD_DB: f64 = 2.5;
/// The attack gain: builds over 3 ms, lets go over 1.
const K_ATT_UP_MS: f64 = 3.0;
const K_ATT_DOWN_MS: f64 = 1.0;

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
    fn set_coefs(&mut self, o: &Biquad) {
        self.b0 = o.b0;
        self.b1 = o.b1;
        self.b2 = o.b2;
        self.a1 = o.a1;
        self.a2 = o.a2;
    }
}

/// 0 LP, 1 HP, 2 BP (0 dB peak).
fn rbj(kind: i32, fc: f64, fs: f64, q: f64) -> Biquad {
    let w = 2.0 * PI * cmin(fc, 0.45 * fs) / fs;
    let cw = w.cos();
    let al = w.sin() / (2.0 * q);
    let a0 = 1.0 + al;
    let mut b = Biquad::default();
    if kind == 0 {
        b.b0 = (1.0 - cw) * 0.5 / a0;
        b.b1 = (1.0 - cw) / a0;
        b.b2 = b.b0;
    } else if kind == 1 {
        b.b0 = (1.0 + cw) * 0.5 / a0;
        b.b1 = -(1.0 + cw) / a0;
        b.b2 = b.b0;
    } else {
        b.b0 = al / a0;
        b.b1 = 0.0;
        b.b2 = -al / a0;
    }
    b.a1 = -2.0 * cw / a0;
    b.a2 = (1.0 - al) / a0;
    b
}

/// Biquad coefficients, normalised by a0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Default for Coefs {
    fn default() -> Self {
        Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }
}

/// Direct Form I: its state is past inputs and outputs, so a coefficient change mid-stream cannot
/// leave a stale internal value behind to ring out.
#[derive(Debug, Clone, Copy, Default)]
struct Df1 {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Df1 {
    #[inline]
    fn run(&mut self, x: f64, c: &Coefs) -> f64 {
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    /// 4th-order band for the attack detector.
    det_hp1: Biquad,
    det_hp2: Biquad,
    det_lp1: Biquad,
    det_lp2: Biquad,
    /// And for the sustain detector.
    sus_hp1: Biquad,
    sus_hp2: Biquad,
    sus_lp1: Biquad,
    sus_lp2: Biquad,
    /// The attack band's last sample, for its pre-emphasis.
    c_prev: f64,
    /// The attack and sustain Focus filters.
    fa: Df1,
    fs: Df1,
}

#[derive(Debug, Clone, Copy)]
struct Trig {
    cw: f64,
    sw: f64,
}

impl Default for Trig {
    fn default() -> Self {
        Trig { cw: 1.0, sw: 0.0 }
    }
}

/// The engine. Made once (its scope ring is made in `new`, its delay line sized in `prepare`);
/// `prepare` and `restart` reuse both.
pub struct TransientEngineDsp {
    scope: Vec<ScopeBin>,
    scope_write: i32,

    ch: [Channel; 2],
    trig: [Trig; NUM_FOCUS as usize],

    // ---------------------------------------------------------------- state
    source: i32,
    attack_focus: i32,
    sustain_focus: i32,
    attack_knob: f32,
    sustain_knob: f32,
    att_time_knob: f32,
    sus_time_knob: f32,
    mix_knob: f32,
    out_knob: f32,
    bypassed: bool,

    att_sm: f64,
    sus_sm: f64,
    mix_sm: f64,
    out_sm: f64,
    byp_sm: f64,
    smooth: f64,
    sample_rate: f64,
    last_att_db: f64,
    last_sus_db: f64,

    // the detectors (linked: one set for both channels)
    /// Sustain level.
    m1: f64,
    m2: f64,
    /// Sustain onset.
    f1: f64,
    f2: f64,
    s1: f64,
    /// Attack.
    q1: f64,
    q2: f64,
    qs: f64,
    peak: f64,
    sus_gain: f64,
    att_gain: f64,
    att_held: f64,
    onset_left: i32,
    onset_samples: i32,
    was_onset: bool,
    ramp_from: f64,
    ramp_pos: i32,
    ramp_samples: i32,
    /// Samples left in which a Source change's detector transient is ignored.
    switch_quiet: i32,
    k_level: f64,
    k_gain: f64,
    k_memory: f64,
    k_on_fast: f64,
    k_on_slow: f64,
    k_onset_gain: f64,
    k_fast: f64,
    k_slow: f64,
    k_hold: f64,
    k_att_up: f64,
    k_att_down: f64,
    sus_scale: f64,

    // the lookahead
    delay_line: [Vec<f32>; 2],
    delay_pos: i32,
    latency_samples: i32,

    scope_hop: i32,
    scope_count: i32,
    scope_acc: ScopeBin,

    // Rust-only, for the readouts: the last block's most extreme gains.
    block_att_db: f64,
    block_sus_db: f64,
}

impl Default for TransientEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl TransientEngineDsp {
    /// The C++ object as constructed (member initialisers), with its scope ring allocated.
    pub fn new() -> Self {
        Self::with_buffers(vec![ScopeBin::default(); SCOPE_BINS as usize], [Vec::new(), Vec::new()])
    }

    fn with_buffers(mut scope: Vec<ScopeBin>, mut delay_line: [Vec<f32>; 2]) -> Self {
        // `std::array<ScopeBin, scopeBins> scope {}`: zeroed. Re-filled in place, never re-made.
        scope.clear();
        scope.resize(SCOPE_BINS as usize, ScopeBin::default());
        // `std::vector<float> delayLine[2]`: empty until `prepare`; emptied, keeping its capacity.
        for d in &mut delay_line {
            d.clear();
        }
        TransientEngineDsp {
            scope,
            scope_write: 0,
            ch: [Channel::default(); 2],
            trig: [Trig::default(); NUM_FOCUS as usize],
            source: S_DRUMS,
            attack_focus: F_FULL,
            sustain_focus: F_FULL,
            attack_knob: 0.5,
            sustain_knob: 0.5,
            att_time_knob: 0.5,
            sus_time_knob: 0.5,
            mix_knob: 1.0,
            out_knob: 0.5,
            bypassed: false,
            att_sm: 0.0,
            sus_sm: 0.0,
            mix_sm: 1.0,
            out_sm: 1.0,
            byp_sm: 0.0,
            smooth: 0.001,
            sample_rate: 0.0,
            last_att_db: 0.0,
            last_sus_db: 0.0,
            m1: 0.0,
            m2: 0.0,
            f1: 0.0,
            f2: 0.0,
            s1: 0.0,
            q1: 0.0,
            q2: 0.0,
            qs: 0.0,
            peak: -200.0,
            sus_gain: 0.0,
            att_gain: 0.0,
            att_held: 0.0,
            onset_left: 0,
            onset_samples: 480,
            was_onset: false,
            ramp_from: 0.0,
            ramp_pos: 0,
            ramp_samples: 0,
            switch_quiet: 0,
            k_level: 0.01,
            k_gain: 0.01,
            k_memory: 0.001,
            k_on_fast: 0.02,
            k_on_slow: 0.001,
            k_onset_gain: 0.007,
            k_fast: 0.02,
            k_slow: 0.001,
            k_hold: 0.001,
            k_att_up: 0.007,
            k_att_down: 0.02,
            sus_scale: 1.0,
            delay_line,
            delay_pos: 0,
            latency_samples: 0,
            scope_hop: 192,
            scope_count: 0,
            scope_acc: ScopeBin::default(),
            block_att_db: 0.0,
            block_sus_db: 0.0,
        }
    }

    /// Rust-only: back to exactly what [`new`](Self::new) made, keeping the scope ring's and the
    /// delay line's allocations (no allocation: they are moved out, emptied and moved back).
    pub fn restart(&mut self) {
        let scope = std::mem::take(&mut self.scope);
        let delay = std::mem::take(&mut self.delay_line);
        *self = Self::with_buffers(scope, delay);
    }

    /// The scope ring's address and length (made once in `new`), for the allocation tests.
    pub fn heap_block(&self) -> (*const ScopeBin, usize) {
        (self.scope.as_ptr(), self.scope.capacity())
    }

    /// The delay line's two blocks (address and capacity), for the allocation tests: sized in
    /// `prepare`, never in `process`.
    pub fn delay_blocks(&self) -> [(*const f32, usize); 2] {
        [(self.delay_line[0].as_ptr(), self.delay_line[0].capacity()), (self.delay_line[1].as_ptr(), self.delay_line[1].capacity())]
    }

    pub fn set_parameter(&mut self, index: usize, v: f32) {
        match index {
            P_SOURCE => {
                let s = clamp_t((v + 0.5f32) as i32, 0, NUM_SOURCES - 1);
                if s != self.source {
                    self.source = s;
                    if self.sample_rate > 0.0 {
                        self.update_detector();
                        // one slow window (the longer of the onset's 20 ms and the new source's
                        // slow attack average), twice over, for the bands to settle
                        let at = f64::from(knob_to_time_scale(self.att_time_knob));
                        let slow = cmax(K_ONSET_SLOW_MS, f64::from(spec(s).slow_att_ms) * at);
                        self.switch_quiet = lround_i32(K_SWITCH_WINDOWS * slow * 0.001 * self.sample_rate);
                    }
                }
            }
            P_ATTACK => self.attack_knob = clamp_t(v, 0.0f32, 1.0f32),
            P_SUSTAIN => self.sustain_knob = clamp_t(v, 0.0f32, 1.0f32),
            P_ATTACK_TIME => {
                self.att_time_knob = clamp_t(v, 0.0f32, 1.0f32);
                if self.sample_rate > 0.0 {
                    self.update_times();
                }
            }
            P_SUSTAIN_TIME => {
                self.sus_time_knob = clamp_t(v, 0.0f32, 1.0f32);
                if self.sample_rate > 0.0 {
                    self.update_times();
                }
            }
            P_ATTACK_FOCUS => {
                let f = clamp_t((v + 0.5f32) as i32, 0, NUM_FOCUS - 1);
                if f != self.attack_focus {
                    self.attack_focus = f;
                    if self.sample_rate > 0.0 {
                        self.update_focus();
                    }
                }
            }
            P_SUSTAIN_FOCUS => {
                let f = clamp_t((v + 0.5f32) as i32, 0, NUM_FOCUS - 1);
                if f != self.sustain_focus {
                    self.sustain_focus = f;
                    if self.sample_rate > 0.0 {
                        self.update_focus();
                    }
                }
            }
            P_MIX => self.mix_knob = clamp_t(v, 0.0f32, 1.0f32),
            P_OUTPUT => self.out_knob = clamp_t(v, 0.0f32, 1.0f32),
            P_BYPASS => self.bypassed = v > 0.5f32,
            _ => {}
        }
    }

    // ---------------------------------------------------------------- lifecycle
    pub fn prepare(&mut self, fs: f64) {
        self.sample_rate = if fs > 0.0 { fs } else { 48000.0 };
        // 15 ms, for the knobs
        self.smooth = 1.0 - (-1.0 / (0.015 * self.sample_rate)).exp();
        self.scope_hop = cmax(1, lround_i32(0.004 * self.sample_rate));
        for c in &mut self.ch {
            *c = Channel::default();
        }
        self.reset_detectors();
        self.update_detector();
        self.update_times();
        self.update_focus();

        // The lookahead delay line: sized here, never in process.
        self.latency_samples = cmax(0, lround_i32(K_LOOKAHEAD_MS * 0.001 * self.sample_rate));
        let len = cmax(1, self.latency_samples) as usize;
        for d in &mut self.delay_line {
            // `assign (n, 0.0f)`
            d.clear();
            d.resize(len, 0.0f32);
        }
        self.delay_pos = 0;

        self.att_sm = f64::from(knob_to_attack_db(self.attack_knob));
        self.sus_sm = f64::from(knob_to_sustain_db(self.sustain_knob));
        self.mix_sm = f64::from(self.mix_knob);
        self.out_sm = f64::from(self.out_gain_target());
        self.byp_sm = if self.bypassed { 1.0 } else { 0.0 };

        self.scope_write = 0;
        self.scope.fill(ScopeBin::default());
        self.scope_acc = ScopeBin::default();
        self.scope_count = 0;
    }

    /// The lookahead, in samples: `round(3 ms * sr)` (0 before the first `prepare`). Always
    /// reported -- bypassed, and with both amounts at zero -- because the output is always the
    /// input that much later.
    pub fn get_latency_samples(&self) -> i32 {
        self.latency_samples
    }

    /// The sample rate `prepare` settled on (0 before the first `prepare`).
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn scope_bins_per_second(&self) -> f32 {
        self.sample_rate as f32 / cmax(1, self.scope_hop) as f32
    }

    /// The scope ring and its next write index (`scope`, `scopeWrite`).
    pub fn scope(&self) -> (&[ScopeBin], i32) {
        (&self.scope, self.scope_write)
    }

    /// For the tests: the gains applied to the last sample, in dB.
    pub fn last_attack_db(&self) -> f32 {
        self.last_att_db as f32
    }
    pub fn last_sustain_db(&self) -> f32 {
        self.last_sus_db as f32
    }

    /// Rust-only: the most extreme attack and sustain gain (dB) of the last `process` call.
    pub fn block_gains_db(&self) -> (f32, f32) {
        (self.block_att_db as f32, self.block_sus_db as f32)
    }

    /// Rust-only: the detectors' levels, linear RMS: the attack band's (its 1 ms onset mean
    /// square) and the sustain level `L`.
    pub fn envelopes(&self) -> (f64, f64) {
        (self.f2.sqrt(), self.m2.sqrt())
    }

    // ---------------------------------------------------------------- audio
    /// `process (channels, numChannels, numSamples)`. `nc` channels of `io` are processed (at most
    /// 2), over `frames` samples (clipped to the shortest of them). Each finished scope bin is
    /// also pushed to `scope_out`.
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize, scope_out: Option<&AnalysisRing>) {
        let nc = cmin(cmin(num_channels, 2), io.len());
        self.block_att_db = 0.0;
        self.block_sus_db = 0.0;
        if nc == 0 {
            return;
        }
        let frames = io.iter().take(nc).map(Vec::len).fold(num_samples, cmin);
        let sp = *spec(self.source);

        for n in 0..frames {
            let t = f64::from(knob_to_attack_db(self.attack_knob));
            Self::glide(self.smooth, &mut self.att_sm, t);
            let t = f64::from(knob_to_sustain_db(self.sustain_knob));
            Self::glide(self.smooth, &mut self.sus_sm, t);
            let t = f64::from(self.mix_knob);
            Self::glide(self.smooth, &mut self.mix_sm, t);
            let t = f64::from(self.out_gain_target());
            Self::glide(self.smooth, &mut self.out_sm, t);
            let t = if self.bypassed { 1.0 } else { 0.0 };
            Self::glide(self.smooth, &mut self.byp_sm, t);

            let sample = |c: usize| io.get(c).and_then(|ch| ch.get(n)).copied().unwrap_or(0.0);
            let input: [f32; 2] = [sample(0), if nc > 1 { sample(1) } else { 0.0f32 }];

            // ---- detectors, on the input as it arrives: the source's sustain band, its attack
            // band and the attack band's pre-emphasis, squared; the louder channel's of each
            let mut b_sq = 0.0f64;
            let mut c_sq = 0.0f64;
            let mut d_sq = 0.0f64;
            for c in 0..nc {
                let k = &mut self.ch[c];
                let x = f64::from(input[c]);
                let b = k.sus_lp2.run(k.sus_lp1.run(k.sus_hp2.run(k.sus_hp1.run(x))));
                let a = k.det_lp2.run(k.det_lp1.run(k.det_hp2.run(k.det_hp1.run(x))));
                let d = a - k.c_prev;
                k.c_prev = a;
                b_sq = cmax(b_sq, b * b);
                c_sq = cmax(c_sq, a * a);
                d_sq = cmax(d_sq, d * d);
            }

            // ---- 1. the sustain level: two cascaded mean-square one-poles, in dB
            self.m1 = flush(self.m1 + self.k_level * (b_sq - self.m1));
            self.m2 = flush(self.m2 + self.k_level * (self.m1 - self.m2));
            let level = 10.0 * (self.m2 + K_TINY).log10();

            // ---- 6. the attack detector: the pre-emphasised band's fast (twice) mean square
            // against its slow one (ahead of the onset: a source may let it open the window)
            self.q1 = flush(self.q1 + self.k_fast * (d_sq - self.q1));
            self.q2 = flush(self.q2 + self.k_fast * (self.q1 - self.q2));
            self.qs = flush(self.qs + self.k_slow * (self.q2 - self.qs));
            let att_ratio_db = 10.0 * ((self.q2 + K_TINY) / (self.qs + K_TINY)).log10();

            // After a Source change the detector bands restart from the old bands' state: for one
            // slow window that transient is neither an onset nor an attack.
            let settling = self.switch_quiet > 0;
            if settling {
                self.switch_quiet -= 1;
            }

            // ---- 2. the sustain onset: the attack band's 1 ms (twice) mean square 6 dB ahead
            // of its 20 ms one opens a 10 ms window -- and, on a source that says so, the
            // pre-emphasised attack ratio past onsetAttackDb (a stick into a ringing drum is
            // heard there first)
            self.f1 = flush(self.f1 + self.k_on_fast * (c_sq - self.f1));
            self.f2 = flush(self.f2 + self.k_on_fast * (self.f1 - self.f2));
            self.s1 = flush(self.s1 + self.k_on_slow * (self.f2 - self.s1));
            let onset_ratio = 10.0 * ((self.f2 + K_TINY) / (self.s1 + K_TINY)).log10() > K_ONSET_DB;
            let onset_attack = sp.onset_attack_db > 0.0f32 && att_ratio_db > f64::from(sp.onset_attack_db);
            if !settling && (onset_ratio || onset_attack) {
                self.onset_left = self.onset_samples;
            }
            if self.onset_left > 0 {
                self.onset_left -= 1;
            }
            let in_onset = self.onset_left > 0;
            let onset_starts = in_onset && !self.was_onset;
            self.was_onset = in_onset;

            // ---- 3. the peak memory: the level since the last onset, forgotten slowly
            if in_onset {
                self.peak = level;
            } else if level > self.peak {
                self.peak = level;
            } else {
                self.peak += self.k_memory * (level - self.peak);
            }

            // ---- 4. below the floor there is no note to shape, only noise. Faded out over
            // 10 dB; the CURRENT level decides, not the remembered peak.
            let live = clamp_t((level - K_FLOOR_DB) / 10.0, 0.0, 1.0);

            // ---- 5. the sustain gain: how far the note has fallen, gliding; back to 0 dB while a
            // note starts -- over the source's onsetReleaseMs (a one-pole), or, where it sets
            // onsetRampMs, a straight line in dB that lands on 0 dB in that time (so the front of
            // a hit landing in the last one's ring leaves the delay line uncut)
            let sus_shape = clamp_t((self.peak - level) / (f64::from(sp.sus_full_db) * self.sus_scale), 0.0, 1.0) * live;
            let sus_target = if in_onset { 0.0 } else { self.sus_sm * sus_shape };
            if onset_starts {
                self.ramp_from = self.sus_gain;
                self.ramp_pos = 0;
            }
            if in_onset && self.ramp_samples > 0 {
                self.ramp_pos += 1;
                self.sus_gain = if self.ramp_pos >= self.ramp_samples {
                    0.0
                } else {
                    self.ramp_from * (1.0 - f64::from(self.ramp_pos) / f64::from(self.ramp_samples))
                };
            } else {
                self.sus_gain += (if in_onset { self.k_onset_gain } else { self.k_gain }) * (sus_target - self.sus_gain);
            }
            self.sus_gain = flush(self.sus_gain);

            // ---- 7. the attack shape, past a dead zone (a ringing note's own wobble is not an
            // onset), held: instant up, holdMs down
            let att_shape = if settling {
                0.0
            } else {
                clamp_t((att_ratio_db - K_DEAD_DB) / ((f64::from(sp.att_full_db) + K_DEAD_DB) - K_DEAD_DB), 0.0, 1.0) * live
            };
            self.att_held = flush(if att_shape > self.att_held { att_shape } else { self.att_held + self.k_hold * (att_shape - self.att_held) });

            // ---- 8. the attack gain: 3 ms toward a bigger gain, 1 ms back
            let att_target = self.att_sm * self.att_held;
            self.att_gain = flush(
                self.att_gain + (if att_target.abs() > self.att_gain.abs() { self.k_att_up } else { self.k_att_down }) * (att_target - self.att_gain),
            );

            let att_db = self.att_gain;
            let sus_db = self.sus_gain;
            self.last_att_db = att_db;
            self.last_sus_db = sus_db;
            let identity = att_db == 0.0 && sus_db == 0.0;
            let ca = self.focus_coefs(self.attack_focus, att_db);
            let cs = self.focus_coefs(self.sustain_focus, sus_db);

            let pos = self.delay_pos as usize;
            for c in 0..nc {
                let s = &mut self.ch[c];
                // ---- 9. the gains reach the input 3 ms late: the delay line
                let line = &mut self.delay_line[c];
                let dry = if self.latency_samples > 0 { line.get(pos).copied().unwrap_or(0.0f32) } else { input[c] };
                if self.latency_samples > 0
                    && let Some(slot) = line.get_mut(pos)
                {
                    *slot = input[c];
                }
                let x = f64::from(dry);

                // Attack, then sustain. The filters always run, so a Focus that comes into use
                // starts warm; with both amounts at zero the output is x itself.
                let mut y = s.fa.run(x, &ca);
                y = s.fs.run(y, &cs);
                if identity {
                    y = x;
                }

                let wet = x + self.mix_sm * (y - x);
                let out = (wet * self.out_sm) as f32;
                // Once bypass has landed it is the (delayed) input, exactly -- not
                // out + 1 * (in - out).
                let o = if self.byp_sm >= 1.0 { dry } else { out + self.byp_sm as f32 * (dry - out) };
                if let Some(slot) = io.get_mut(c).and_then(|ch| ch.get_mut(n)) {
                    *slot = o;
                }

                self.scope_acc.in_peak = cmax(self.scope_acc.in_peak, dry.abs());
                self.scope_acc.out_peak = cmax(self.scope_acc.out_peak, o.abs());
            }
            if self.latency_samples > 0 {
                self.delay_pos += 1;
                if self.delay_pos >= self.latency_samples {
                    self.delay_pos = 0;
                }
            }

            if att_db.abs() > f64::from(self.scope_acc.att_db.abs()) {
                self.scope_acc.att_db = att_db as f32;
            }
            if sus_db.abs() > f64::from(self.scope_acc.sus_db.abs()) {
                self.scope_acc.sus_db = sus_db as f32;
            }
            // Rust-only: the block's extremes, by the same rule.
            if att_db.abs() > self.block_att_db.abs() {
                self.block_att_db = att_db;
            }
            if sus_db.abs() > self.block_sus_db.abs() {
                self.block_sus_db = sus_db;
            }
            self.scope_count += 1;
            if self.scope_count >= self.scope_hop {
                let w = self.scope_write;
                if let Some(bin) = self.scope.get_mut(w as usize) {
                    *bin = self.scope_acc;
                }
                self.scope_write = (w + 1) % SCOPE_BINS;
                if let Some(ring) = scope_out {
                    let b = self.scope_acc;
                    ring.push_slice(&[b.in_peak, b.out_peak, b.att_db, b.sus_db]);
                }
                self.scope_acc = ScopeBin::default();
                self.scope_count = 0;
            }
        }
    }

    /// One-pole glide that LANDS, in double: see SatEngineDSP::glide.
    #[inline]
    fn glide(smooth: f64, v: &mut f64, target: f64) {
        *v += smooth * (target - *v);
        if (target - *v).abs() < 1.0e-9 {
            *v = target;
        }
    }

    /// The Focus filter at `db` of gain: RBJ low shelf, bell or high shelf (shelf slope 1), or a
    /// plain gain on the full band. Only the gain changes per sample; the corner's sine and cosine
    /// are computed once, in `update_focus`.
    pub fn focus_coefs(&self, focus: i32, db: f64) -> Coefs {
        let mut c = Coefs::default();
        if focus == F_FULL {
            c.b0 = 10.0f64.powf(db / 20.0);
            return c;
        }
        if db == 0.0 {
            return c;
        }

        let a = 10.0f64.powf(db / 40.0);
        let t = self.trig.get(focus as usize).copied().unwrap_or_default();
        let (cw, sw) = (t.cw, t.sw);
        let (b0, b1, b2, a0, a1, a2);
        if focus == F_MID {
            let al = sw / (2.0 * f64::from(K_MID_Q));
            b0 = 1.0 + al * a;
            b1 = -2.0 * cw;
            b2 = 1.0 - al * a;
            a0 = 1.0 + al / a;
            a1 = -2.0 * cw;
            a2 = 1.0 - al / a;
        } else {
            // shelf slope S = 1
            let al = sw / 2.0 * 2.0f64.sqrt();
            let sq = 2.0 * a.sqrt() * al;
            if focus == F_LOW {
                b0 = a * ((a + 1.0) - (a - 1.0) * cw + sq);
                b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cw);
                b2 = a * ((a + 1.0) - (a - 1.0) * cw - sq);
                a0 = (a + 1.0) + (a - 1.0) * cw + sq;
                a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cw);
                a2 = (a + 1.0) + (a - 1.0) * cw - sq;
            } else {
                b0 = a * ((a + 1.0) + (a - 1.0) * cw + sq);
                b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cw);
                b2 = a * ((a + 1.0) + (a - 1.0) * cw - sq);
                a0 = (a + 1.0) - (a - 1.0) * cw + sq;
                a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cw);
                a2 = (a + 1.0) - (a - 1.0) * cw - sq;
            }
        }
        c.b0 = b0 / a0;
        c.b1 = b1 / a0;
        c.b2 = b2 / a0;
        c.a1 = a1 / a0;
        c.a2 = a2 / a0;
        c
    }

    fn reset_detectors(&mut self) {
        self.m1 = 0.0;
        self.m2 = 0.0;
        self.f1 = 0.0;
        self.f2 = 0.0;
        self.s1 = 0.0;
        self.q1 = 0.0;
        self.q2 = 0.0;
        self.qs = 0.0;
        self.peak = -200.0;
        self.sus_gain = 0.0;
        self.att_gain = 0.0;
        self.att_held = 0.0;
        self.onset_left = 0;
        self.was_onset = false;
        self.ramp_from = 0.0;
        self.ramp_pos = 0;
        self.switch_quiet = 0;
    }

    fn update_detector(&mut self) {
        let sp = *spec(self.source);
        let sr = self.sample_rate;
        for c in &mut self.ch {
            // Butterworth, 4th order
            c.det_hp1.set_coefs(&rbj(1, f64::from(sp.det_low_hz), sr, 0.5412));
            c.det_hp2.set_coefs(&rbj(1, f64::from(sp.det_low_hz), sr, 1.3066));
            c.det_lp1.set_coefs(&rbj(0, f64::from(sp.det_high_hz), sr, 0.5412));
            c.det_lp2.set_coefs(&rbj(0, f64::from(sp.det_high_hz), sr, 1.3066));
            c.sus_hp1.set_coefs(&rbj(1, f64::from(sp.sus_low_hz), sr, 0.5412));
            c.sus_hp2.set_coefs(&rbj(1, f64::from(sp.sus_low_hz), sr, 1.3066));
            c.sus_lp1.set_coefs(&rbj(0, f64::from(sp.sus_high_hz), sr, 0.5412));
            c.sus_lp2.set_coefs(&rbj(0, f64::from(sp.sus_high_hz), sr, 1.3066));
        }
        self.update_times();
    }

    /// `k (ms) = 1 - exp (-1 / (ms * 0.001 * sr))`: a one-pole's coefficient.
    fn coef(ms: f64, fs: f64) -> f64 {
        1.0 - (-1.0 / (cmax(0.01, ms) * 0.001 * fs)).exp()
    }

    fn update_times(&mut self) {
        let sp = *spec(self.source);
        let at = f64::from(knob_to_time_scale(self.att_time_knob));
        let st = f64::from(knob_to_time_scale(self.sus_time_knob));
        let sr = self.sample_rate;
        self.k_level = Self::coef(f64::from(sp.level_ms), sr);
        self.k_gain = Self::coef(f64::from(sp.gain_ms), sr);
        self.k_memory = Self::coef(f64::from(sp.memory_ms), sr);
        self.k_on_fast = Self::coef(K_ONSET_FAST_MS, sr);
        self.k_on_slow = Self::coef(K_ONSET_SLOW_MS, sr);
        self.k_onset_gain = Self::coef(f64::from(sp.onset_release_ms), sr);
        self.ramp_samples = lround_i32(f64::from(sp.onset_ramp_ms) * 0.001 * sr);
        self.onset_samples = lround_i32(K_ONSET_MS * 0.001 * sr);
        // Attack Time: the length of "the attack", the slow average the fast one is held against
        self.k_fast = Self::coef(f64::from(sp.fast_ms), sr);
        self.k_slow = Self::coef(f64::from(sp.slow_att_ms) * at, sr);
        self.k_hold = Self::coef(f64::from(sp.hold_ms), sr);
        self.k_att_up = Self::coef(K_ATT_UP_MS, sr);
        self.k_att_down = Self::coef(K_ATT_DOWN_MS, sr);
        // Sustain Time: how far a note has to fall before the sustain gain is all the way in.
        self.sus_scale = st;
    }

    fn update_focus(&mut self) {
        let hz: [f64; NUM_FOCUS as usize] = [1000.0, f64::from(K_LOW_HZ), f64::from(K_MID_HZ), f64::from(K_HIGH_HZ)];
        let sr = self.sample_rate;
        for (t, f) in self.trig.iter_mut().zip(hz) {
            let w = 2.0 * PI * cmin(f, 0.45 * sr) / sr;
            t.cw = w.cos();
            t.sw = w.sin();
        }
    }

    fn out_gain_target(&self) -> f32 {
        10.0f32.powf(knob_to_out_db(self.out_knob) / 20.0f32)
    }
}
