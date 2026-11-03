//! A faithful translation of `Shale/Source/ShaleDSP.h`: a drum gate that separates the shells
//! from the cymbals (a band-limited detector with an onset criterion, a lookahead gate whose
//! close follows each hit's own decay, DeBleed's coincidence-gated HF duck, and Kit Link's
//! partner triggers and neighbour beacons).
//!
//! Same state, order of operations, constants and arithmetic types as the C++ (`float` stays
//! `f32`, `double` stays `f64`). C++ behaviour Rust does not share is reproduced deliberately:
//! - `std::max` / `std::min` are [`cmax`] / [`cmin`] (`a < b ? b : a`), not `f32::max`;
//! - `(int) std::lround (x)` is `x.round() as i32` (half away from zero; the parameter layer
//!   clamps first, so only finite in-range values reach it);
//! - `readPos % delayCap` is `rem_euclid`: the same index for every reachable state, and never
//!   out of bounds for one the C++ would index off the front of the line with;
//! - the UI outputs (`gainOut`, `detOut`, `trigSeq`, the scope ...) are plain fields read after
//!   `process`: the C++ atomics exist only for the editor's thread. The scope's bins are also
//!   pushed, as they complete, to the analysis ring the UI reads ([`SCOPE_BIN_VALUES`] per bin);
//! - the counters (`trigSeq`, `deferSeq`) wrap where the C++ `int` would overflow.
//!
//! `ScopedNoDenormals` (the wrapper's flush-to-zero) has no safe Rust equivalent, and no decision
//! here needs it emulated: every comparison is against a floor far above the denormal range
//! (the threshold, at least -79 dB; the `1e-9` in the onset ratio and velocity; `1e-4` on the
//! beacon; `1e-8`, `1e-7`, `1e-9` in DeBleed and the slope; the close at least 1e-7 below a hit
//! that cleared the threshold), so a value the C++ would flush to zero is on the same side of each
//! test. Only sub-1e-38 sample values can differ, far below the fidelity bar.

// Index loops kept as the C++ writes them (several arrays per index).
#![allow(clippy::needless_range_loop)]

use crate::analysis::AnalysisRing;

/// Parameter indexes (`ShaleDSP::ParamIndex`).
pub const P_BYPASS: usize = 0;
pub const P_INSTRUMENT: usize = 1;
pub const P_THRESHOLD: usize = 2;
pub const P_SENS: usize = 3;
pub const P_GHOST: usize = 4;
pub const P_REDUCTION: usize = 5;
pub const P_LENGTH: usize = 6;
pub const P_CLOSE_MODE: usize = 7;
pub const P_LOOKAHEAD: usize = 8;
pub const P_HOLD: usize = 9;
pub const P_RETRIG: usize = 10;
pub const P_DEBLEED: usize = 11;
pub const P_DEBLEED_MODE: usize = 12;
pub const P_XOVER: usize = 13;
pub const P_VEL_TRACK: usize = 14;
pub const P_AUTO_BAND: usize = 15;
pub const P_DET_LOW: usize = 16;
pub const P_DET_HIGH: usize = 17;
pub const P_SC_SOURCE: usize = 18;
pub const P_SC_LISTEN: usize = 19;
pub const P_MONITOR: usize = 20;
pub const P_MIX: usize = 21;
pub const P_TRIM: usize = 22;
pub const P_OUTPUT: usize = 23;
pub const P_LINK: usize = 24;
pub const P_LINK_GROUP: usize = 25;
pub const P_LINK_ROLE: usize = 26;
pub const P_REJECT: usize = 27;
pub const P_MIDI_OUT: usize = 28;
pub const P_MIDI_NOTE: usize = 29;
// Phase alignment (`ShaleAlign.h`). Appended; the DSP ignores them: the processor does the
// measuring and the delay.
pub const P_PHASE_REF: usize = 30;
pub const P_ALIGN: usize = 31;
pub const P_ALIGN_OFFSET: usize = 32;
pub const P_ALIGN_FLIP: usize = 33;
pub const P_ALIGN_TO: usize = 34;
pub const NUM_PARAMS: usize = 35;

/// `Instrument`: the mic list, in the order you would meet them going round a kit.
pub const I_KICK_IN: i32 = 0;
pub const I_KICK_OUT: i32 = 1;
pub const I_SUB_KICK: i32 = 2;
pub const I_SNARE_TOP: i32 = 3;
pub const I_SNARE_BTM: i32 = 4;
pub const I_RACK_TOM: i32 = 5;
pub const I_FLOOR_TOM: i32 = 6;
pub const I_HAT: i32 = 7;
pub const I_RIDE: i32 = 8;
pub const I_OVERHEADS: i32 = 9;
pub const I_ROOM: i32 = 10;
pub const NUM_INSTRUMENTS: i32 = 11;

pub const C_DECAY_MATCH: i32 = 0;
pub const C_FIXED: i32 = 1;
pub const D_SHELF: i32 = 0;
pub const D_LEARNED: i32 = 1;
pub const SC_INTERNAL: i32 = 0;
pub const SC_EXTERNAL: i32 = 1;
pub const M_OUTPUT: i32 = 0;
pub const M_BLEED: i32 = 1;
pub const M_DETECTOR: i32 = 2;
pub const R_LEAD: i32 = 0;
pub const R_FOLLOW: i32 = 1;
pub const R_PAIR: i32 = 2;

/// `pi` (3.14159265358979323846).
pub const PI: f64 = std::f64::consts::PI;

/// Bands for Learned DeBleed, split by first-order crossovers.
pub const NUM_BANDS: usize = 6;
pub const BAND_SPLIT_HZ: [f32; NUM_BANDS - 1] = [500.0, 1000.0, 2000.0, 4000.0, 8000.0];

/// `ModeSpec`: the hard parameters behind the Mic choice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeSpec {
    pub name: &'static str,
    /// The detector's band: where the shell is and the cymbal is not.
    pub det_low_hz: f32,
    pub det_high_hz: f32,
    /// DeBleed crossover: above this is "bleed until proven otherwise".
    pub xover_hz: f32,
    /// Minimum open time, so one hit cannot chatter.
    pub min_hold_ms: f32,
    /// Lockout after a trigger.
    pub retrig_ms: f32,
    /// How long a real hit's HF is allowed through.
    pub coinc_ms: f32,
    /// Length maps into this: dB below the hit's own peak.
    pub follow_min_db: f32,
    pub follow_max_db: f32,
    /// Added to the onset requirement.
    pub flux_bias_db: f32,
    pub why: &'static str,
}

const fn spec(
    name: &'static str,
    det: (f32, f32),
    xover_hz: f32,
    min_hold_ms: f32,
    retrig_ms: f32,
    coinc_ms: f32,
    follow: (f32, f32),
    flux_bias_db: f32,
    why: &'static str,
) -> ModeSpec {
    ModeSpec {
        name,
        det_low_hz: det.0,
        det_high_hz: det.1,
        xover_hz,
        min_hold_ms,
        retrig_ms,
        coinc_ms,
        follow_min_db: follow.0,
        follow_max_db: follow.1,
        flux_bias_db,
        why,
    }
}

/// `mode()`'s table, in `Instrument` order.
pub static MODES: [ModeSpec; NUM_INSTRUMENTS as usize] = [
    spec(
        "Kick In",
        (40.0, 130.0),
        300.0,
        40.0,
        28.0,
        20.0,
        (8.0, 34.0),
        0.0,
        "Inside the drum, on the beater. Nothing else on a kit lives below 130 Hz, so this is the easy one -- and the beater click makes the onset crisp enough to time from. Long lockout: a kick never needs to retrigger in 10 ms, and a long one rejects the beater bouncing.",
    ),
    spec(
        "Kick Out",
        (28.0, 90.0),
        220.0,
        55.0,
        36.0,
        26.0,
        (8.0, 36.0),
        -2.5,
        "Outside, on the resonant head. All body and no click, so the onset is markedly softer than the inside mic's -- hold it to the same sharpness and it simply gets missed. Longer hold too, because this is the mic carrying the sustain.",
    ),
    spec(
        "Sub Kick",
        (25.0, 70.0),
        160.0,
        70.0,
        45.0,
        30.0,
        (10.0, 38.0),
        -4.0,
        "A speaker used as a microphone. Its cone has mass, so its rise is the slowest thing on the kit and the onset requirement has to come down furthest. There is nothing up top to debleed here -- that is the point of the mic.",
    ),
    spec(
        "Snare Top",
        (170.0, 420.0),
        750.0,
        22.0,
        14.0,
        30.0,
        (6.0, 45.0),
        0.0,
        "The hard case and the reason the plugin exists. 170..420 Hz is the shell fundamental and the hats have nothing there. Short lockout, because a buzz roll is 30 ms between hits and anything longer eats it.",
    ),
    spec(
        "Snare Btm",
        (1800.0, 6000.0),
        7000.0,
        22.0,
        14.0,
        26.0,
        (6.0, 40.0),
        0.0,
        "A separate mode and not a knob, because the bottom mic IS mostly wire noise at 2..6 kHz. Point the snare-top detector at it and it hears nothing. The crossover goes up with it: here the wires are signal, not bleed. Pair this with the top.",
    ),
    spec(
        "Rack Tom",
        (100.0, 280.0),
        620.0,
        30.0,
        20.0,
        28.0,
        (8.0, 40.0),
        -1.0,
        "Sits between the kick and the snare, and both of them are the bleed that matters. This is a mic where Kit Link earns its keep rather than being a luxury.",
    ),
    spec(
        "Floor Tom",
        (55.0, 170.0),
        450.0,
        40.0,
        24.0,
        30.0,
        (8.0, 42.0),
        -1.5,
        "Low enough to collide with the kick outright, so link it or expect the kick to open it. Longer decay than a rack tom, so it follows further down.",
    ),
    spec(
        "Hi-Hat",
        (2500.0, 9000.0),
        12000.0,
        12.0,
        8.0,
        18.0,
        (4.0, 26.0),
        0.0,
        "Gating the hat mic, where the SNARE is the bleed -- the reverse of every other mode. The detector sits above the snare's HF splash, and the lockout is tiny because sixteenth hats are 60 ms apart at worst and a foot chick is faster.",
    ),
    spec(
        "Ride",
        (1200.0, 5000.0),
        9000.0,
        25.0,
        16.0,
        40.0,
        (4.0, 22.0),
        -1.0,
        "Follows further down than anything else by default: a ride cut off mid-wash is far more obvious than a tom cut off mid-decay.",
    ),
    spec(
        "Overheads",
        (180.0, 6000.0),
        9000.0,
        35.0,
        18.0,
        45.0,
        (6.0, 40.0),
        -1.0,
        "Deliberately the widest band on the list, because an overhead is supposed to open on ANY part of the kit -- it is not trying to isolate one drum, it is trying not to run a room's worth of noise and spill between fills. Usually better set to Follow off the snare than left to decide for itself.",
    ),
    spec(
        "Room",
        (60.0, 400.0),
        900.0,
        60.0,
        40.0,
        60.0,
        (10.0, 50.0),
        -3.0,
        "Room and ambience mics, gated from the kit rather than from themselves. Set Link to Follow and the room opens on the snare, which is the trick this mic is usually bought for. Long everything: a room that closes quickly sounds like a fault.",
    ),
];

/// `mode (i)`: the table row, the index clamped.
pub fn mode(i: i32) -> &'static ModeSpec {
    let i = if i < 0 {
        0
    } else if i >= NUM_INSTRUMENTS {
        NUM_INSTRUMENTS - 1
    } else {
        i
    };
    &MODES[i as usize]
}

// ------------------------------------------------------------------ knob -> unit maps
// The UI prints these same numbers.

/// -66..0 dB.
pub fn knob_to_thresh_db(v: f32) -> f32 {
    -66.0 + 66.0 * v
}
/// Onset ratio needed, dB.
pub fn knob_to_flux_db(v: f32) -> f32 {
    9.5 - 8.0 * v
}
/// -3..-60, then mute.
pub fn knob_to_red_db(v: f32) -> f32 {
    -(3.0 + 57.0 * v)
}
pub fn is_full_mute(v: f32) -> bool {
    v >= 0.995
}
/// 8..360 ms.
pub fn knob_to_fixed_ms(v: f32) -> f32 {
    8.0 * 45.0f32.powf(v)
}
/// x minHoldMs.
pub fn knob_to_hold_scale(v: f32) -> f32 {
    0.35 + 1.9 * v
}
pub fn knob_to_retrig_ms(v: f32) -> f32 {
    3.0 + 60.0 * v
}
/// 150 Hz..15 kHz.
pub fn knob_to_xover_hz(v: f32) -> f32 {
    150.0 * 100.0f32.powf(v)
}
/// 25 Hz..12 kHz.
pub fn knob_to_det_hz(v: f32) -> f32 {
    25.0 * 480.0f32.powf(v)
}
pub fn knob_to_trim_db(v: f32) -> f32 {
    -12.0 + 36.0 * v
}
pub fn knob_to_out_db(v: f32) -> f32 {
    -12.0 + 24.0 * v
}
/// Added to the flux requirement.
pub fn knob_to_reject_db(v: f32) -> f32 {
    18.0 * v
}
pub fn knob_to_midi_note(v: f32) -> i32 {
    24 + (v * 84.0).round() as i32
}

/// Lookahead choice -> ms.
pub fn lookahead_ms(choice: i32) -> f32 {
    const MS: [f32; 5] = [0.0, 1.0, 2.0, 3.0, 5.0];
    MS[if choice < 0 {
        0
    } else if choice > 4 {
        4
    } else {
        choice as usize
    }]
}

/// dB below this hit's own peak that the gate follows it down to.
pub fn follow_db(instrument: i32, length_knob: f32) -> f32 {
    let m = mode(instrument);
    m.follow_min_db + (m.follow_max_db - m.follow_min_db) * clamp_t(length_knob, 0.0, 1.0)
}

// ------------------------------------------------------------------ small pieces

/// `clampT`.
pub fn clamp_t<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `std::max`: `a < b ? b : a`.
pub fn cmax<T: PartialOrd>(a: T, b: T) -> T {
    if a < b { b } else { a }
}

/// `std::min`: `b < a ? b : a`.
pub fn cmin<T: PartialOrd>(a: T, b: T) -> T {
    if b < a { b } else { a }
}

pub fn db_to_lin(db: f32) -> f32 {
    10.0f32.powf(db * 0.05)
}

/// `(int) std::lround (x)` for a double.
fn lround(x: f64) -> i32 {
    x.round() as i32
}

/// Raised cosine 0..1.
fn raised_cos(t: f32) -> f32 {
    let x = clamp_t(t, 0.0, 1.0);
    0.5 - 0.5 * (PI as f32 * x).cos()
}

fn velocity_of(env: f32, thresh_lin: f32) -> f32 {
    let db = 20.0 * ((env + 1.0e-9) / (thresh_lin + 1.0e-9)).log10();
    clamp_t(db / 26.0, 0.0, 1.0)
}

/// First-order TPT split. lp + hp == x exactly.
#[derive(Debug, Clone, Copy, Default)]
struct Split1 {
    s: f32,
    g: f32,
}

impl Split1 {
    fn set_cutoff(&mut self, hz: f32, sr: f64) {
        let f = clamp_t(hz, 10.0, (sr * 0.49) as f32);
        let w = (PI * f64::from(f) / sr).tan() as f32;
        self.g = w / (1.0 + w);
    }
    fn reset(&mut self) {
        self.s = 0.0;
    }
    #[inline]
    fn process(&mut self, x: f32) -> (f32, f32) {
        let v = self.g * (x - self.s);
        let lp = v + self.s;
        self.s = lp + v;
        (lp, x - lp)
    }
}

/// Topology-preserving state variable filter, one 2nd-order section.
#[derive(Debug, Clone, Copy)]
struct Svf {
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl Default for Svf {
    fn default() -> Self {
        Svf { g: 0.0, k: 1.0, a1: 0.0, a2: 0.0, a3: 0.0, ic1: 0.0, ic2: 0.0 }
    }
}

impl Svf {
    fn set(&mut self, hz: f32, q: f32, sr: f64) {
        let f = clamp_t(hz, 5.0, (sr * 0.47) as f32);
        self.g = (PI * f64::from(f) / sr).tan() as f32;
        self.k = 1.0 / if q > 0.01 { q } else { 0.01 };
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }
    fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
    #[inline]
    fn process(&mut self, v0: f32) -> (f32, f32) {
        let v3 = v0 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v0 - self.k * v1 - v2)
    }
}

/// The two section Qs of a 4th-order Butterworth (the C++ literals, verbatim).
#[allow(clippy::excessive_precision)]
const BUTTER_Q: [f32; 2] = [0.541_196_10, 1.306_562_96];

#[derive(Debug, Clone, Copy)]
struct Env {
    y: f32,
    ca: f32,
    cr: f32,
}

impl Default for Env {
    fn default() -> Self {
        Env { y: 0.0, ca: 1.0, cr: 1.0 }
    }
}

impl Env {
    fn set_times(&mut self, atk_ms: f32, rel_ms: f32, sr: f64) {
        self.ca = if atk_ms <= 0.0 { 1.0 } else { 1.0 - (-1.0f32 / (0.001 * atk_ms * sr as f32)).exp() };
        self.cr = if rel_ms <= 0.0 { 1.0 } else { 1.0 - (-1.0f32 / (0.001 * rel_ms * sr as f32)).exp() };
    }
    fn reset(&mut self) {
        self.y = 0.0;
    }
    #[inline]
    fn process(&mut self, rect: f32) -> f32 {
        self.y += (if rect > self.y { self.ca } else { self.cr }) * (rect - self.y);
        self.y
    }
}

/// Gate states (`State`).
pub const ST_CLOSED: i32 = 0;
pub const ST_OPENING: i32 = 1;
pub const ST_OPEN: i32 = 2;
pub const ST_CLOSING: i32 = 3;

/// What the visualiser draws, per 4 ms bin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScopeBin {
    pub peak: f32,
    pub gain: f32,
    pub det: f32,
}

impl Default for ScopeBin {
    fn default() -> Self {
        ScopeBin { peak: 0.0, gain: 1.0, det: 0.0 }
    }
}

/// Ten seconds of 4 ms bins.
pub const SCOPE_BINS: usize = 2500;
/// Values per bin on the analysis ring: peak, gain, det.
pub const SCOPE_BIN_VALUES: usize = 3;

/// One detected hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trigger {
    /// May be negative: a held trigger released a block or more later.
    pub offset: i32,
    pub velocity: f32,
    /// This mic's detector fired, not only a Pair partner's trigger.
    pub own: bool,
}

impl Default for Trigger {
    fn default() -> Self {
        Trigger { offset: 0, velocity: 0.0, own: true }
    }
}

/// A transient on this mic, broadband and instantaneous, with how loud this mic heard it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beacon {
    pub offset: i32,
    pub level_db: f32,
}

impl Default for Beacon {
    fn default() -> Self {
        Beacon { offset: 0, level_db: -120.0 }
    }
}

/// 3 dB. A new onset during a ring has to beat the decay by this much.
const RETRIG_MARGIN_LIN: f32 = 1.413;
/// The close is matched to the hit's own decay for this many dB, then accelerated.
const MATCH_SPAN_DB: f32 = 8.0;
const CLOSE_ACCEL: f32 = 6.0;
/// The beacon band, one pair for every instance and every mode.
const BEACON_LOW_HZ: f32 = 60.0;
const BEACON_HIGH_HZ: f32 = 1200.0;
/// `1 << 28`: "never" for the ages.
const AGE_NEVER: i32 = 1 << 28;

/// `ShaleDSP`.
#[derive(Debug, Clone)]
pub struct ShaleDsp {
    // ---- outputs back to the UI
    pub gain_out: f32,
    pub det_out: f32,
    pub flux_out: f32,
    pub coinc_out: f32,
    pub hf_cut_out: f32,
    pub trig_seq: i32,
    /// Also read by DeBleed's velocity tracking (`lastVelOut.load()`), as the C++ does.
    pub last_vel_out: f32,
    pub state_out: i32,
    pub slope_out: f32,
    pub defer_seq: i32,
    pub nb_level_out: f32,
    pub my_level_out: f32,

    // ---- the scope
    pub scope: Box<[ScopeBin; SCOPE_BINS]>,
    pub scope_write: usize,

    sample_rate: f64,
    max_block_size: i32,

    // knobs
    bypass: bool,
    ghost: bool,
    auto_band: bool,
    sc_listen: bool,
    link_on: bool,
    midi_out: bool,
    instrument: i32,
    close_mode: i32,
    debleed_mode: i32,
    sc_source: i32,
    monitor: i32,
    lookahead_choice: i32,
    link_group: i32,
    link_role: i32,
    thresh_knob: f32,
    sens_knob: f32,
    red_knob: f32,
    length_knob: f32,
    hold_knob: f32,
    retrig_knob: f32,
    debleed_knob: f32,
    xover_knob: f32,
    vel_track_knob: f32,
    det_low_knob: f32,
    det_high_knob: f32,
    mix_amt: f32,
    midi_note_knob: f32,
    reject_knob: f32,
    trim_lin: f32,
    out_lin: f32,
    smoothed_trim: f32,
    smoothed_out: f32,
    red_lin: f32,

    scope_acc: ScopeBin,
    scope_hop: i32,
    scope_count: i32,

    lookahead_samples: i32,
    delay_cap: i32,
    force_geometry: bool,

    // detector
    det_hp: [Svf; 2],
    det_lp: [Svf; 2],
    env_fast: Env,
    env_slow: Env,
    env_follow: Env,

    // gate
    state: i32,
    gain: f32,
    open_from: f32,
    hold_count: i32,
    lockout_count: i32,
    open_count: i32,
    hit_peak: f32,
    since_trigger: i32,
    slope_at20: f32,
    slope_at60: f32,
    measured_slope: f32,
    close_step_db: f32,
    close_db: f32,
    close_accel_from: f32,
    coinc: f32,
    reject_env: f32,
    partner_vel: f32,

    // the broadband beacon path
    wide_hp: Svf,
    wide_lp: Svf,
    wide_fast: Env,
    wide_slow: Env,
    wide_peak: f32,
    nb_best_db: f32,
    beacon_peak: f32,
    my_beacon_db: f32,
    nb_age: i32,
    beacon_lock: i32,
    beacon_pend: i32,
    beacon_offset: i32,
    my_beacon_age: i32,

    pending_trig_valid: bool,
    pending_trig_offset: i32,
    pending_trig_age: i32,
    pending_trig_vel: f32,
    pending_trig_own: bool,

    // debleed
    band_split: [[Split1; NUM_BANDS - 1]; 2],
    shelf_split: [Split1; 2],
    band_env: [Env; NUM_BANDS],
    profile: [f32; NUM_BANDS],
    band_gain: [f32; NUM_BANDS],
    band_gain_coef: f32,
    shelf_hz: f32,

    // delay
    delay: [Vec<f32>; 2],
    delay_write: i32,

    // triggers out
    trigs: [Trigger; 64],
    trig_count: i32,
    beacons: [Beacon; 64],
    beacon_count: i32,

    // link in
    pending_partner_hits: [Trigger; 32],
    pending_neighbour_hits: [Trigger; 32],
    pending_partner: i32,
    pending_neighbour: i32,
}

impl Default for ShaleDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl ShaleDsp {
    /// `ShaleDSP()`: the member initialisers, then `prepare (48000.0, 512)`.
    pub fn new() -> Self {
        let mut d = ShaleDsp {
            gain_out: 1.0,
            det_out: 0.0,
            flux_out: 0.0,
            coinc_out: 0.0,
            hf_cut_out: 0.0,
            trig_seq: 0,
            last_vel_out: 0.0,
            state_out: 0,
            slope_out: 0.0,
            defer_seq: 0,
            nb_level_out: -120.0,
            my_level_out: -120.0,
            scope: Box::new([ScopeBin::default(); SCOPE_BINS]),
            scope_write: 0,
            sample_rate: 48000.0,
            max_block_size: 512,
            bypass: false,
            ghost: false,
            auto_band: true,
            sc_listen: false,
            link_on: false,
            midi_out: false,
            instrument: I_SNARE_TOP,
            close_mode: C_DECAY_MATCH,
            debleed_mode: D_SHELF,
            sc_source: SC_INTERNAL,
            monitor: M_OUTPUT,
            lookahead_choice: 2,
            link_group: 0,
            link_role: R_LEAD,
            thresh_knob: 0.55,
            sens_knob: 0.5,
            red_knob: 1.0,
            length_knob: 0.4,
            hold_knob: 0.35,
            retrig_knob: 0.2,
            debleed_knob: 0.0,
            xover_knob: 0.5,
            vel_track_knob: 0.5,
            det_low_knob: 0.4,
            det_high_knob: 0.6,
            mix_amt: 1.0,
            midi_note_knob: 0.1667,
            reject_knob: 0.5,
            trim_lin: 1.0,
            out_lin: 1.0,
            smoothed_trim: 1.0,
            smoothed_out: 1.0,
            red_lin: 0.0,
            scope_acc: ScopeBin::default(),
            scope_hop: 192,
            scope_count: 0,
            lookahead_samples: 96,
            delay_cap: 2048,
            force_geometry: true,
            det_hp: [Svf::default(); 2],
            det_lp: [Svf::default(); 2],
            env_fast: Env::default(),
            env_slow: Env::default(),
            env_follow: Env::default(),
            state: ST_CLOSED,
            gain: 0.0,
            open_from: 0.0,
            hold_count: 0,
            lockout_count: 0,
            open_count: 0,
            hit_peak: 0.0,
            since_trigger: -1,
            slope_at20: 0.0,
            slope_at60: 0.0,
            measured_slope: 0.0,
            close_step_db: 0.0,
            close_db: 0.0,
            close_accel_from: 0.0,
            coinc: 0.0,
            reject_env: 0.0,
            partner_vel: 0.0,
            wide_hp: Svf::default(),
            wide_lp: Svf::default(),
            wide_fast: Env::default(),
            wide_slow: Env::default(),
            wide_peak: 0.0,
            nb_best_db: -120.0,
            beacon_peak: 0.0,
            my_beacon_db: -120.0,
            nb_age: AGE_NEVER,
            beacon_lock: 0,
            beacon_pend: 0,
            beacon_offset: 0,
            my_beacon_age: AGE_NEVER,
            pending_trig_valid: false,
            pending_trig_offset: 0,
            pending_trig_age: 0,
            pending_trig_vel: 0.0,
            pending_trig_own: true,
            band_split: [[Split1::default(); NUM_BANDS - 1]; 2],
            shelf_split: [Split1::default(); 2],
            band_env: [Env::default(); NUM_BANDS],
            profile: [0.0; NUM_BANDS],
            band_gain: [0.0; NUM_BANDS],
            band_gain_coef: 0.004,
            shelf_hz: 750.0,
            delay: [Vec::new(), Vec::new()],
            delay_write: 0,
            trigs: [Trigger::default(); 64],
            trig_count: 0,
            beacons: [Beacon::default(); 64],
            beacon_count: 0,
            pending_partner_hits: [Trigger::default(); 32],
            pending_neighbour_hits: [Trigger::default(); 32],
            pending_partner: 0,
            pending_neighbour: 0,
        };
        d.prepare(48000.0, 512);
        d
    }

    // ------------------------------------------------------------------ link input

    /// A hit from a partner (same group, Pair or Follow), fed in before `process`.
    pub fn add_partner_hit(&mut self, velocity: f32, offset: i32) {
        if (self.pending_partner as usize) < self.pending_partner_hits.len() {
            self.pending_partner_hits[self.pending_partner as usize] = Trigger { offset, velocity, own: true };
            self.pending_partner += 1;
        }
    }

    /// A beacon from another mic on the same kit (the level travels in `velocity`, as the C++).
    pub fn add_neighbour_beacon(&mut self, level_db: f32, offset: i32) {
        if (self.pending_neighbour as usize) < self.pending_neighbour_hits.len() {
            self.pending_neighbour_hits[self.pending_neighbour as usize] = Trigger { offset, velocity: level_db, own: true };
            self.pending_neighbour += 1;
        }
    }

    pub fn num_beacons(&self) -> usize {
        self.beacon_count.max(0) as usize
    }
    pub fn beacon(&self, i: usize) -> Beacon {
        self.beacons.get(i).copied().unwrap_or_default()
    }
    pub fn num_triggers(&self) -> usize {
        self.trig_count.max(0) as usize
    }
    pub fn trigger(&self, i: usize) -> Trigger {
        self.trigs.get(i).copied().unwrap_or_default()
    }
    pub fn clear_triggers(&mut self) {
        self.trig_count = 0;
    }

    // ------------------------------------------------------------------ lifecycle

    /// `prepare`. Sizes the delay line: allocates only when it has to grow (never when the rate
    /// and the block size are the same as last time, so a live re-prepare is allocation-free).
    pub fn prepare(&mut self, new_sample_rate: f64, max_block: i32) {
        self.sample_rate = if new_sample_rate > 0.0 { new_sample_rate } else { 48000.0 };
        self.max_block_size = if max_block > 0 { max_block } else { 512 };

        // Longest delay we can ever be asked for: 5 ms of lookahead, plus one block.
        let max_la = (0.005 * self.sample_rate).ceil() as i32 + 8;
        self.delay_cap = max_la + self.max_block_size + 8;

        let cap = self.delay_cap.max(1) as usize;
        for d in &mut self.delay {
            // `assign (delayCap, 0.0f)`.
            d.clear();
            d.resize(cap, 0.0);
        }
        self.delay_write = 0;

        let sr = self.sample_rate;
        self.env_slow.set_times(14.0, 45.0, sr);
        self.env_follow.set_times(1.0, 26.0, sr);
        self.wide_fast.set_times(0.4, 8.0, sr);
        self.wide_slow.set_times(12.0, 80.0, sr);
        self.wide_hp.set(BEACON_LOW_HZ, 0.707, sr);
        self.wide_lp.set(BEACON_HIGH_HZ, 0.707, sr);

        for b in 0..NUM_BANDS {
            self.band_env[b].set_times(1.2, 60.0, sr);
            self.profile[b] = 0.0;
            self.band_gain[b] = 1.0;
        }

        self.band_gain_coef = self.one_pole_coef(5.0); // 5 ms on the per-band gain

        // One scope bin per 4 ms.
        self.scope_hop = 1.max(lround(0.004 * self.sample_rate));

        self.reset();
        self.rebuild_filters();
        self.force_geometry = true;
    }

    /// `reset`.
    pub fn reset(&mut self) {
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.delay_write = 0;

        for f in &mut self.det_hp {
            f.reset();
        }
        for f in &mut self.det_lp {
            f.reset();
        }
        self.wide_fast.reset();
        self.wide_slow.reset();
        self.wide_hp.reset();
        self.wide_lp.reset();
        self.wide_peak = 0.0;
        self.nb_best_db = -120.0;
        self.nb_age = AGE_NEVER;
        self.beacon_count = 0;
        self.beacon_lock = 0;
        self.beacon_pend = 0;
        self.beacon_offset = 0;
        self.beacon_peak = 0.0;
        self.my_beacon_db = -120.0;
        self.my_beacon_age = AGE_NEVER;
        self.pending_trig_valid = false;
        self.pending_trig_offset = 0;
        self.pending_trig_age = 0;
        self.pending_trig_vel = 0.0;
        self.env_fast.reset();
        self.env_slow.reset();
        self.env_follow.reset();

        for s in &mut self.band_split {
            for c in s.iter_mut() {
                c.reset();
            }
        }
        for c in &mut self.shelf_split {
            c.reset();
        }
        for b in 0..NUM_BANDS {
            self.band_env[b].reset();
            self.band_gain[b] = 1.0;
        }

        self.scope.fill(ScopeBin::default());
        self.scope_write = 0;
        self.scope_acc = ScopeBin::default();
        self.scope_count = 0;

        self.gain = 0.0;
        self.state = ST_CLOSED;
        self.hold_count = 0;
        self.lockout_count = 0;
        self.open_count = 0;
        self.hit_peak = 0.0;
        self.coinc = 0.0;
        self.reject_env = 0.0;
        self.close_step_db = 0.0;
        self.since_trigger = -1;
        self.slope_at20 = 0.0;
        self.slope_at60 = 0.0;
        self.measured_slope = 0.0;
        self.trig_count = 0;
        self.pending_partner = 0;
        self.pending_neighbour = 0;
        self.smoothed_trim = self.trim_lin;
        self.smoothed_out = self.out_lin;
    }

    /// What the host has to compensate for: the lookahead delay, and nothing else.
    pub fn latency_samples(&self) -> i32 {
        self.lookahead_samples
    }

    /// Rust addition (a safety deviation): `clamp (lookahead, 0, delayCap - maxBlock - 8)`, the
    /// setter's own bound, again. The C++ works the lookahead out in samples only when the choice
    /// changes, so after a live re-prepare at a LOWER rate it can exceed the new, shorter delay
    /// line: the delay then wraps and the reported latency is no longer the real one. The wrapper
    /// calls this after such a prepare; the latency is reported again.
    pub fn reclamp_lookahead(&mut self) {
        let bound = self.delay_cap - self.max_block_size - 8;
        let la = clamp_t(self.lookahead_samples, 0, bound);
        if la != self.lookahead_samples {
            self.lookahead_samples = la;
            self.force_geometry = true;
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn max_block_size(&self) -> i32 {
        self.max_block_size
    }

    // ------------------------------------------------------------------ events

    /// `setParameter`.
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        match index {
            P_BYPASS => self.bypass = v > 0.5,
            P_INSTRUMENT => {
                let next = lround(f64::from(v));
                if next != self.instrument {
                    self.instrument = if next < 0 {
                        0
                    } else if next >= NUM_INSTRUMENTS {
                        NUM_INSTRUMENTS - 1
                    } else {
                        next
                    };
                    self.rebuild_filters();
                    // A learned profile belongs to one detector band.
                    self.profile = [0.0; NUM_BANDS];
                }
            }
            P_THRESHOLD => self.thresh_knob = clamp_t(v, 0.0, 1.0),
            P_SENS => self.sens_knob = clamp_t(v, 0.0, 1.0),
            P_GHOST => self.ghost = v > 0.5,
            P_REDUCTION => {
                self.red_knob = clamp_t(v, 0.0, 1.0);
                self.red_lin = if is_full_mute(self.red_knob) { 0.0 } else { db_to_lin(knob_to_red_db(self.red_knob)) };
            }
            P_LENGTH => self.length_knob = clamp_t(v, 0.0, 1.0),
            P_CLOSE_MODE => self.close_mode = lround(f64::from(v)),
            P_LOOKAHEAD => {
                let next = lround(f64::from(v));
                if next != self.lookahead_choice {
                    self.lookahead_choice = next;
                    self.lookahead_samples = lround(f64::from(lookahead_ms(self.lookahead_choice)) * 0.001 * self.sample_rate);
                    self.lookahead_samples = clamp_t(self.lookahead_samples, 0, self.delay_cap - self.max_block_size - 8);
                    self.force_geometry = true;
                }
            }
            P_HOLD => self.hold_knob = clamp_t(v, 0.0, 1.0),
            P_RETRIG => self.retrig_knob = clamp_t(v, 0.0, 1.0),
            P_DEBLEED => self.debleed_knob = clamp_t(v, 0.0, 1.0),
            P_DEBLEED_MODE => self.debleed_mode = lround(f64::from(v)),
            P_XOVER => {
                self.xover_knob = clamp_t(v, 0.0, 1.0);
                self.rebuild_filters();
            }
            P_VEL_TRACK => self.vel_track_knob = clamp_t(v, 0.0, 1.0),
            P_AUTO_BAND => {
                if (v > 0.5) != self.auto_band {
                    self.auto_band = v > 0.5;
                    self.rebuild_filters();
                }
            }
            P_DET_LOW => {
                self.det_low_knob = clamp_t(v, 0.0, 1.0);
                self.rebuild_filters();
            }
            P_DET_HIGH => {
                self.det_high_knob = clamp_t(v, 0.0, 1.0);
                self.rebuild_filters();
            }
            P_SC_SOURCE => self.sc_source = lround(f64::from(v)),
            P_SC_LISTEN => self.sc_listen = v > 0.5,
            P_MONITOR => self.monitor = lround(f64::from(v)),
            P_MIX => self.mix_amt = clamp_t(v, 0.0, 1.0),
            P_TRIM => self.trim_lin = db_to_lin(knob_to_trim_db(clamp_t(v, 0.0, 1.0))),
            P_OUTPUT => self.out_lin = db_to_lin(knob_to_out_db(clamp_t(v, 0.0, 1.0))),
            P_LINK => {
                let next = v > 0.5;
                if next != self.link_on {
                    self.link_on = next;
                    self.force_geometry = true;
                }
            }
            P_LINK_GROUP => self.link_group = lround(f64::from(v)),
            P_LINK_ROLE => {
                let next = lround(f64::from(v));
                if next != self.link_role {
                    self.link_role = next;
                    self.force_geometry = true;
                }
            }
            P_REJECT => self.reject_knob = clamp_t(v, 0.0, 1.0),
            P_MIDI_OUT => self.midi_out = v > 0.5,
            P_MIDI_NOTE => self.midi_note_knob = clamp_t(v, 0.0, 1.0),
            _ => {}
        }
    }

    pub fn instrument(&self) -> i32 {
        self.instrument
    }
    pub fn link_group(&self) -> i32 {
        self.link_group
    }
    pub fn link_role(&self) -> i32 {
        self.link_role
    }
    pub fn is_link_on(&self) -> bool {
        self.link_on
    }
    pub fn is_midi_out(&self) -> bool {
        self.midi_out
    }
    /// Stored, never read by the C++ engine (SC Listen has no effect on the sound).
    pub fn is_sc_listen(&self) -> bool {
        self.sc_listen
    }
    pub fn midi_note(&self) -> i32 {
        knob_to_midi_note(self.midi_note_knob)
    }

    /// True the first time it is asked after anything changed the reported latency.
    pub fn take_geometry_changed(&mut self) -> bool {
        std::mem::take(&mut self.force_geometry)
    }

    pub fn det_low_hz(&self) -> f32 {
        if self.auto_band { mode(self.instrument).det_low_hz } else { knob_to_det_hz(self.det_low_knob) }
    }
    pub fn det_high_hz(&self) -> f32 {
        if self.auto_band { mode(self.instrument).det_high_hz } else { knob_to_det_hz(self.det_high_knob) }
    }
    pub fn xover_hz(&self) -> f32 {
        if self.auto_band { mode(self.instrument).xover_hz } else { knob_to_xover_hz(self.xover_knob) }
    }

    /// Seconds of history the scope holds.
    pub fn scope_seconds(&self) -> f32 {
        (self.scope_hop as usize * SCOPE_BINS) as f32 / self.sample_rate as f32
    }
    /// How many bins make one second.
    pub fn scope_bins_per_second(&self) -> f32 {
        self.sample_rate as f32 / 1.max(self.scope_hop) as f32
    }

    /// The gate's current gain (linear).
    pub fn gate_gain(&self) -> f32 {
        self.gain
    }

    // ------------------------------------------------------------------ audio

    /// `process`: `io` is the main bus, in place (the first `num_channels` buffers, at most 2
    /// used); `sc` an optional external sidechain with `sc_channels` buffers. `scope_out` gets
    /// each completed scope bin (`[peak, gain, det]`).
    pub fn process(
        &mut self,
        io: &mut [Vec<f32>],
        num_channels: usize,
        num_samples: usize,
        sc: Option<&[Vec<f32>]>,
        sc_channels: usize,
        scope_out: Option<&AnalysisRing>,
    ) {
        self.trig_count = 0;
        self.beacon_count = 0;

        // The delay line is two channels: anything wider is clamped.
        let num_channels = num_channels.min(2).min(io.len());
        let num_samples = io.iter().take(num_channels).map(Vec::len).fold(num_samples, usize::min);

        if num_channels == 0 || num_samples == 0 {
            self.pending_partner = 0;
            self.pending_neighbour = 0;
            return;
        }

        if self.bypass {
            // Still has to delay, or bypassing would shift the track by the reported latency.
            self.run_delay_only(io, num_channels, num_samples);
            self.pending_partner = 0;
            self.pending_neighbour = 0;
            self.gain_out = 1.0;
            self.state_out = 2;
            return;
        }

        // The sidechain's channels that are really there (at most 2, as the C++'s jmin). A key
        // buffer shorter than the block is silent where it has no samples (never the track's own
        // input: the C++ bus is the block's length whatever the source does).
        let sc = sc.filter(|_| sc_channels > 0);
        let sc_n = sc.map_or(0, |s| s.len().min(sc_channels).min(2));
        let sc = sc.filter(|_| sc_n > 0);

        let sr = self.sample_rate;
        let thresh_lin = db_to_lin(knob_to_thresh_db(self.thresh_knob) + if self.ghost { -13.0 } else { 0.0 });
        let m = *mode(self.instrument);
        let flux_req = knob_to_flux_db(self.sens_knob) + m.flux_bias_db + if self.ghost { -3.5 } else { 0.0 };
        let fol_db = follow_db(self.instrument, self.length_knob);
        let min_hold = m.min_hold_ms * knob_to_hold_scale(self.hold_knob);
        let min_hold_n = lround(f64::from(min_hold) * 0.001 * sr);
        let retrig_n = lround(f64::from(cmax(m.retrig_ms * 0.4, knob_to_retrig_ms(self.retrig_knob))) * 0.001 * sr);
        let max_open_n = lround(2.5 * sr);
        let coinc_coef = self.one_pole_coef(m.coinc_ms / 3.0);
        // 12 ms: block-order jitter between two instances, the acoustic path across a kit, and
        // the 3 ms each beacon spends measuring its own peak before it can be published at all.
        let link_guard_n = lround(0.012 * sr);
        let wide_peak_dec = 1.0 - self.one_pole_coef(18.0);

        let defer_margin_db = 20.0 * (1.0 - self.reject_knob);
        let link_guard_active = self.link_on && self.reject_knob > 0.0;
        let reject_coef = self.one_pole_coef(60.0);
        let open_n = cmax(self.lookahead_samples, lround(0.0002 * sr)) as f32;
        let red_db_now = if is_full_mute(self.red_knob) { 90.0 } else { -knob_to_red_db(self.red_knob) };

        let mut block_det = 0.0f32;
        let mut block_flux = -120.0f32;

        let trim_step = (self.trim_lin - self.smoothed_trim) / num_samples as f32;
        let out_step = (self.out_lin - self.smoothed_out) / num_samples as f32;

        let at20 = lround(0.020 * sr);
        let at60 = lround(0.060 * sr);
        let cap = self.delay_cap.max(1);

        for n in 0..num_samples {
            let ni = n as i32;
            self.smoothed_trim += trim_step;
            self.smoothed_out += out_step;

            // The dim trace on the display is the INPUT, taken before the output is written.
            let scope_in = io[0][n].abs();

            let mut det = 0.0f32;
            if self.sc_source == SC_EXTERNAL
                && let Some(sc) = sc
            {
                for c in sc.iter().take(sc_n) {
                    det += c.get(n).copied().unwrap_or(0.0);
                }
                det /= sc_n as f32;
            } else {
                for c in io.iter().take(num_channels) {
                    det += c[n] * self.smoothed_trim;
                }
                det /= num_channels as f32;
            }

            // ---- band-pass into the detector band.
            let mut d = det;
            for f in &mut self.det_hp {
                d = f.process(d).1;
            }
            for f in &mut self.det_lp {
                d = f.process(d).0;
            }

            let rect = d.abs();
            let ef = self.env_fast.process(rect);
            let es = self.env_slow.process(rect);
            let efo = self.env_follow.process(rect);

            // ---- the onset function: a RATIO, so it is level-independent.
            let flux = 20.0 * ((ef + 1.0e-9) / (es + 1.0e-9)).log10();

            // ---- the beacon path: 60..1200 Hz.
            let (_, whi) = self.wide_hp.process(det);
            let (wlo, _) = self.wide_lp.process(whi);
            let wr = wlo.abs();
            let wf = self.wide_fast.process(wr);
            let ws = self.wide_slow.process(wr);
            self.wide_peak = cmax(wf, self.wide_peak * wide_peak_dec);
            let my_wide_db = 20.0 * (self.wide_peak + 1.0e-9).log10();

            if self.beacon_lock > 0 {
                self.beacon_lock -= 1;
            }

            if self.beacon_pend > 0 {
                self.beacon_peak = cmax(self.beacon_peak, wf);
                self.beacon_pend -= 1;
                if self.beacon_pend == 0 {
                    self.my_beacon_db = 20.0 * (self.beacon_peak + 1.0e-9).log10();
                    self.my_beacon_age = 0;
                    if (self.beacon_count as usize) < self.beacons.len() {
                        self.beacons[self.beacon_count as usize] =
                            Beacon { offset: if self.beacon_offset < 0 { 0 } else { self.beacon_offset }, level_db: self.my_beacon_db };
                        self.beacon_count += 1;
                    }
                }
            } else if self.beacon_lock == 0 && wf > 1.0e-4 && wf > ws * 2.0 {
                // +6 dB broadband
                self.beacon_pend = lround(0.003 * sr);
                self.beacon_peak = wf;
                self.beacon_offset = ni;
                self.beacon_lock = lround(0.008 * sr);
            }

            // ---- a held trigger comes out once it has outlived the retraction window
            if self.pending_trig_valid && {
                self.pending_trig_age += 1;
                self.pending_trig_age >= link_guard_n
            } {
                if (self.trig_count as usize) < self.trigs.len() {
                    self.trigs[self.trig_count as usize] =
                        Trigger { offset: self.pending_trig_offset, velocity: self.pending_trig_vel, own: self.pending_trig_own };
                    self.trig_count += 1;
                }
                self.pending_trig_valid = false;
            }

            // ---- neighbour deferral ages out
            self.reject_env -= reject_coef * self.reject_env;
            if self.nb_age < AGE_NEVER {
                self.nb_age += 1;
            }
            if self.my_beacon_age < AGE_NEVER {
                self.my_beacon_age += 1;
            }

            // ---- link input for this sample
            let mut partner_fired = false;
            for i in 0..self.pending_partner.max(0) as usize {
                let h = self.pending_partner_hits[i];
                if h.offset == ni {
                    partner_fired = true;
                    self.partner_vel = h.velocity;
                }
            }
            for i in 0..self.pending_neighbour.max(0) as usize {
                let h = self.pending_neighbour_hits[i];
                if h.offset != ni {
                    continue;
                }
                // velocity carries the neighbour level in dB here
                let nb_db = h.velocity;
                if self.nb_age >= link_guard_n || nb_db > self.nb_best_db {
                    self.nb_best_db = nb_db;
                }
                self.nb_age = 0;
                self.reject_env = 1.0;
            }

            // ---- the trigger decision
            let link_active = self.link_on && self.reject_knob > 0.0 && self.nb_age < link_guard_n;
            let defer = link_active && self.nb_best_db > my_wide_db + defer_margin_db;
            let retract = link_active && self.my_beacon_age < link_guard_n && self.nb_best_db > self.my_beacon_db + defer_margin_db;

            // RETRACTION: a losing comparison cancels a trigger that is still young.
            if (defer || retract) && self.since_trigger >= 0 && self.since_trigger < link_guard_n {
                self.defer_seq = self.defer_seq.wrapping_add(1);
                self.nb_level_out = self.nb_best_db;
                self.my_level_out = if retract { self.my_beacon_db } else { my_wide_db };
                self.pending_trig_valid = false; // never published, nothing to take back

                self.state = ST_CLOSED;
                self.gain = self.red_lin;
                self.since_trigger = -1;
                self.hold_count = 0;
                self.lockout_count = 0;
                self.coinc = 0.0;

                // Take the outgoing trigger back too, while it is still in this block.
                while self.trig_count > 0 && self.trigs[(self.trig_count - 1) as usize].offset > ni - link_guard_n {
                    self.trig_count -= 1;
                }
            }

            let mut want_open;
            let vel;

            // While a hit is still ringing, a second trigger has to STAND ABOVE its decay.
            let ringing = self.since_trigger >= 0;
            let above_decay = !ringing || ef > efo * RETRIG_MARGIN_LIN;

            let self_fires = (ef > thresh_lin) && (flux > flux_req) && above_decay && !defer;

            if self.link_on && self.link_role == R_FOLLOW {
                want_open = partner_fired; // does not decide for itself at all
                vel = self.partner_vel;
            } else if self.link_on && self.link_role == R_PAIR {
                want_open = self_fires || partner_fired; // union: both mics see the same hit
                vel = if partner_fired { cmax(self.partner_vel, velocity_of(ef, thresh_lin)) } else { velocity_of(ef, thresh_lin) };
            } else {
                want_open = self_fires;
                vel = velocity_of(ef, thresh_lin);
            }

            if self.lockout_count > 0 {
                self.lockout_count -= 1;
                want_open = false;
            }

            if want_open && self.state != ST_OPENING {
                // a new hit
                self.state = ST_OPENING;
                self.open_from = self.gain; // ramp from wherever we actually are, not from 0
                self.hold_count = min_hold_n;
                self.open_count = 0;
                self.lockout_count = retrig_n;
                self.hit_peak = cmax(efo, ef);
                self.coinc = 1.0;
                self.since_trigger = 0;
                self.slope_at20 = 0.0;
                self.slope_at60 = 0.0;
                self.measured_slope = 0.0;

                // With the link live, publication waits out the guard. The gate does not.
                if link_guard_active {
                    self.pending_trig_valid = true;
                    self.pending_trig_offset = ni;
                    self.pending_trig_vel = clamp_t(vel, 0.0, 1.0);
                    self.pending_trig_own = self_fires;
                    self.pending_trig_age = 0;
                } else if (self.trig_count as usize) < self.trigs.len() {
                    self.trigs[self.trig_count as usize] = Trigger { offset: ni, velocity: clamp_t(vel, 0.0, 1.0), own: self_fires };
                    self.trig_count += 1;
                }

                self.trig_seq = self.trig_seq.wrapping_add(1);
                self.last_vel_out = clamp_t(vel, 0.0, 1.0);
            }

            // ---- per-hit peak and decay-slope measurement
            if self.since_trigger >= 0 {
                self.since_trigger += 1;
                if efo > self.hit_peak {
                    self.hit_peak = efo;
                }

                if self.since_trigger == at20 {
                    self.slope_at20 = efo;
                }
                if self.since_trigger == at60 {
                    self.slope_at60 = efo;
                    if self.slope_at20 > 1.0e-7 && self.slope_at60 > 1.0e-9 && self.slope_at60 < self.slope_at20 {
                        self.measured_slope = 20.0 * (self.slope_at20 / self.slope_at60).log10() / 0.040;
                    } else {
                        self.measured_slope = 0.0; // still rising, or sustained: not measurable
                    }
                    self.slope_out = self.measured_slope;
                }
            }

            // ---- gate state machine
            match self.state {
                ST_OPENING => {
                    self.open_count += 1;
                    self.gain = self.open_from + (1.0 - self.open_from) * raised_cos(self.open_count as f32 / open_n);
                    if self.open_count as f32 >= open_n {
                        self.gain = 1.0;
                        self.state = ST_OPEN;
                    }
                }
                ST_OPEN => {
                    self.gain = 1.0;
                    if self.hold_count > 0 {
                        self.hold_count -= 1;
                    }
                    self.open_count += 1;

                    // Per-hit relative close: follow this hit down to folDb below its OWN peak.
                    let close_at = self.hit_peak * db_to_lin(-fol_db);
                    let quiet = efo < close_at;
                    let droning = self.open_count > max_open_n;

                    if (quiet && self.hold_count == 0) || droning {
                        self.state = ST_CLOSING;
                        self.open_from = self.gain;

                        if self.close_mode == C_DECAY_MATCH && self.measured_slope > 1.0 {
                            let matched_ms = clamp_t(1000.0 * MATCH_SPAN_DB / self.measured_slope, 4.0, 120.0);
                            self.close_step_db = MATCH_SPAN_DB / (matched_ms * 0.001 * sr as f32);
                            self.close_accel_from = MATCH_SPAN_DB;
                        } else {
                            let n_samp = cmax(1.0, knob_to_fixed_ms(self.length_knob) * 0.001 * sr as f32);
                            self.close_step_db = red_db_now / n_samp;
                            self.close_accel_from = red_db_now; // Fixed means fixed: no acceleration
                        }
                        self.close_db = 0.0;
                    }
                }
                ST_CLOSING => {
                    self.close_db += self.close_step_db * if self.close_db < self.close_accel_from { 1.0 } else { CLOSE_ACCEL };
                    self.gain = self.open_from * db_to_lin(-self.close_db);
                    if self.close_db >= red_db_now || self.gain <= self.red_lin {
                        self.gain = self.red_lin;
                        self.state = ST_CLOSED;
                        self.since_trigger = -1;
                    }
                }
                _ => {
                    self.gain = self.red_lin;
                }
            }

            // ---- DeBleed coincidence window
            self.coinc -= coinc_coef * self.coinc;

            // ---- read the delayed audio
            let read_pos = self.delay_write - self.lookahead_samples + cap;
            let w = self.delay_write.rem_euclid(cap) as usize;
            let r = read_pos.rem_euclid(cap) as usize;

            let gain = self.gain;
            let coinc = self.coinc;
            for c in 0..num_channels {
                let input = io[c][n] * self.smoothed_trim;
                let line = &mut self.delay[c];
                if let Some(s) = line.get_mut(w) {
                    *s = input;
                }
                let dry = line.get(r).copied().unwrap_or(0.0);

                let mut wet = self.apply_debleed(dry, c, coinc, gain);
                wet *= gain;

                let o = match self.monitor {
                    M_BLEED => dry - wet, // exactly what is removed
                    M_DETECTOR => d,      // what the detector hears
                    _ => dry + (wet - dry) * self.mix_amt,
                };

                io[c][n] = o * self.smoothed_out;
            }

            self.delay_write += 1;
            if self.delay_write >= cap {
                self.delay_write = 0;
            }

            // ---- scope
            self.scope_acc.peak = cmax(self.scope_acc.peak, scope_in);
            self.scope_acc.gain = cmin(self.scope_acc.gain, gain);
            self.scope_acc.det = cmax(self.scope_acc.det, ef);

            self.scope_count += 1;
            if self.scope_count >= self.scope_hop {
                let w = self.scope_write % SCOPE_BINS;
                self.scope[w] = self.scope_acc;
                self.scope_write = (w + 1) % SCOPE_BINS;
                if let Some(ring) = scope_out {
                    let b = self.scope_acc;
                    ring.push_slice(&[b.peak, b.gain, b.det]);
                }
                self.scope_acc = ScopeBin::default();
                self.scope_count = 0;
            }

            if ef > block_det {
                block_det = ef;
            }
            if flux > block_flux {
                block_flux = flux;
            }
        }

        // A beacon still being measured when the block ends keeps its offset meaningful by
        // sliding it back.
        if self.beacon_pend > 0 {
            self.beacon_offset -= num_samples as i32;
        }
        if self.pending_trig_valid {
            self.pending_trig_offset -= num_samples as i32;
        }

        self.det_out = block_det;
        self.flux_out = block_flux;

        self.gain_out = self.gain;
        self.coinc_out = self.coinc;
        self.state_out = self.state;
        self.pending_partner = 0;
        self.pending_neighbour = 0;
    }

    fn one_pole_coef(&self, ms: f32) -> f32 {
        if ms <= 0.0 {
            return 1.0;
        }
        1.0 - (-1.0f32 / (0.001 * ms * self.sample_rate as f32)).exp()
    }

    fn rebuild_filters(&mut self) {
        let low_hz = self.det_low_hz();
        let high_hz = cmax(self.det_high_hz(), low_hz * 1.2);
        let sr = self.sample_rate;

        for i in 0..2 {
            self.det_hp[i].set(low_hz, BUTTER_Q[i], sr);
            self.det_lp[i].set(high_hz, BUTTER_Q[i], sr);
        }

        // The fast envelope has to span a full cycle of the LOWEST frequency it is watching.
        let fast_rel_ms = cmax(5.5, 1400.0 / cmax(20.0, low_hz));
        self.env_fast.set_times(0.4, fast_rel_ms, sr);

        for c in 0..2 {
            for k in 0..NUM_BANDS - 1 {
                self.band_split[c][k].set_cutoff(BAND_SPLIT_HZ[k], sr);
            }
        }

        self.shelf_hz = self.xover_hz();
        for c in 0..2 {
            self.shelf_split[c].set_cutoff(self.shelf_hz, sr);
        }
    }

    /// Both DeBleed modes: the signal with the non-coincident high content taken out.
    fn apply_debleed(&mut self, x: f32, ch: usize, coinc_now: f32, gate_gain: f32) -> f32 {
        if self.debleed_knob <= 0.0005 {
            return x;
        }

        // Velocity tracking: a soft hit has less HF OF ITS OWN.
        let vel = self.last_vel_out;
        let vel_scale = 1.0 - self.vel_track_knob * (1.0 - clamp_t(0.3 + 0.7 * vel, 0.0, 1.0));
        let amt = clamp_t(self.debleed_knob * vel_scale, 0.0, 1.0);

        let pass = clamp_t(coinc_now, 0.0, 1.0);

        if self.debleed_mode == D_SHELF {
            let (lp, hp) = self.shelf_split[ch.min(1)].process(x);

            let hf_gain = 1.0 - amt * (1.0 - pass);
            if ch == 0 {
                self.hf_cut_out = -20.0 * cmax(1.0e-4f32, hf_gain).log10();
            }
            return lp + hp * hf_gain;
        }

        // ---- Learned: split into bands, keeping the residual so it reconstructs
        let mut band = [0.0f32; NUM_BANDS];
        let mut rest = x;
        for k in 0..NUM_BANDS - 1 {
            let (lp, hp) = self.band_split[ch.min(1)][k].process(rest);
            band[k] = lp;
            rest = hp;
        }
        band[NUM_BANDS - 1] = rest;

        let closed_now = gate_gain <= self.red_lin * 1.08 + 1.0e-6;
        let mut out = 0.0f32;
        let mut worst_cut = 0.0f32;

        for b in 0..NUM_BANDS {
            let e = self.band_env[b].process(band[b].abs());

            // Only channel 0 updates the profile.
            if closed_now && ch == 0 {
                self.profile[b] += 0.00035 * (e - self.profile[b]);
            }

            let mut g = 1.0f32;
            let centre = if b == 0 {
                250.0
            } else if b == NUM_BANDS - 1 {
                12000.0
            } else {
                0.5 * (BAND_SPLIT_HZ[b - 1] + BAND_SPLIT_HZ[b])
            };

            if centre >= self.shelf_hz {
                let target = self.profile[b] * amt * (1.0 - pass);
                if e > 1.0e-8 {
                    g = clamp_t(1.0 - target / e, 1.0 - amt, 1.0);
                }
            }

            // 5 ms of smoothing on the band gain.
            let gs = &mut self.band_gain[b];
            *gs += self.band_gain_coef * (g - *gs);
            out += band[b] * *gs;
            worst_cut = cmin(worst_cut, 20.0 * cmax(1.0e-4f32, *gs).log10());
        }

        if ch == 0 {
            self.hf_cut_out = -worst_cut;
        }
        out
    }

    /// Bypass still has to run the delay line.
    fn run_delay_only(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize) {
        let num_channels = num_channels.min(2);
        let cap = self.delay_cap.max(1);
        for n in 0..num_samples {
            let read_pos = self.delay_write - self.lookahead_samples + cap;
            let w = self.delay_write.rem_euclid(cap) as usize;
            let r = read_pos.rem_euclid(cap) as usize;
            for c in 0..num_channels {
                let input = io[c][n];
                let line = &mut self.delay[c];
                if let Some(s) = line.get_mut(w) {
                    *s = input;
                }
                io[c][n] = line.get(r).copied().unwrap_or(0.0);
            }
            self.delay_write += 1;
            if self.delay_write >= cap {
                self.delay_write = 0;
            }
        }
    }

    // ------------------------------------------------------------------ Rust-only

    /// The live-reset seeding (Rust addition, see `fs_shale`'s module docs): DeBleed's velocity
    /// tracking reads the last hit's velocity, which the C++ `prepare` leaves from the previous
    /// history; a reset starts it at its initial 0 so the output after a reset does not depend
    /// on what played before.
    pub fn seed_after_reset(&mut self) {
        self.last_vel_out = 0.0;
    }

    /// The delay line's buffers as (pointer, capacity) (allocation tests).
    #[cfg(test)]
    pub(crate) fn heap(&self) -> [(*const f32, usize); 2] {
        [(self.delay[0].as_ptr(), self.delay[0].capacity()), (self.delay[1].as_ptr(), self.delay[1].capacity())]
    }

    /// The trim and output smoothing (current, target) (reset tests).
    #[cfg(test)]
    pub(crate) fn smoothing(&self) -> [(f32, f32); 2] {
        [(self.smoothed_trim, self.trim_lin), (self.smoothed_out, self.out_lin)]
    }

    /// The learned DeBleed profile (tests).
    #[cfg(test)]
    pub(crate) fn profile(&self) -> [f32; NUM_BANDS] {
        self.profile
    }
}
