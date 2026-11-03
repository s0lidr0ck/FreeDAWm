//! `LevelUpDSP.h`, translated line by line: the LUFS gain-matching leveler (the original, between
//! the "the original" markers, ported from `LevelUp.cmajor`), plus the mastering additions: the
//! reference in place of the mix, the BS.1770-4 delivery meter (integrated loudness in a 0.1 dB
//! histogram, 4x oversampled true peak), the platform preview and TPDF dither.
//!
//! C++ semantics kept deliberately:
//! - every `float` expression stays `f32` and every `double` one `f64`, in the C++ order (no
//!   fused multiply-add: MSVC does not contract under `/fp:precise`);
//! - `std::log10`/`std::exp`/`std::pow`/`std::tanh` on a `float` are the `f32` functions;
//! - `std::round` (half away from zero) is `f32::round`; `std::lround` likewise, then the cast;
//! - `(int)` of a float truncates toward zero (`as i32`, which also saturates where the C++ would
//!   be undefined; no reachable value is out of range);
//! - the readouts are `std::atomic`s in the C++ (the editor polls them from another thread);
//!   here the plugin reads them after `process` on the same thread, so they are plain fields.
//!
//! The reference track (`Reference`: 16-bit samples plus the integrated loudness measured on
//! load) reaches [`LevelUpDsp::process`] as a [`RefView`]: the samples as the host decoded them,
//! read through `Reference::toFloat (Reference::fromFloat (x))`, the exact value the C++ plays.

use std::f64::consts::PI;

// ---------------------------------------------------------------- parameters
pub const P_ATTACK: usize = 0;
pub const P_RELEASE: usize = 1;
pub const P_GATE: usize = 2;
pub const P_MAX_GAIN: usize = 3;
pub const P_TARGET: usize = 4;
pub const P_MODE: usize = 5;
pub const P_NORMALIZE: usize = 6;
pub const P_FREEZE: usize = 7;
pub const P_CEILING: usize = 8;
pub const P_LEARN: usize = 9;
pub const P_LINK: usize = 10;
pub const P_LINK_GROUP: usize = 11;
pub const P_REFERENCE: usize = 12;
pub const P_REF_SLOT: usize = 13;
pub const P_PLATFORM: usize = 14;
pub const P_PREVIEW: usize = 15;
pub const P_DITHER: usize = 16;
pub const P_SHAPING: usize = 17;
pub const P_BYPASS: usize = 18;
pub const NUM_PARAMS: usize = 19;

/// `enum Link`.
pub const LINK_OFF: i32 = 0;
pub const LINK_IN: i32 = 1;
pub const LINK_OUT: i32 = 2;

/// `enum Dither`.
pub const DITHER_OFF: i32 = 0;
pub const DITHER_24: i32 = 1;
pub const DITHER_16: i32 = 2;

// ---------------------------------------------------------------- platforms
/// `struct Platform`: published targets, 2025-26. `turns_up`: the service also raises quiet
/// tracks, only as far as their peaks allow (to -1 dBTP).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Platform {
    pub name: &'static str,
    pub target_lufs: f32,
    pub turns_up: bool,
}

pub const NUM_PLATFORMS: usize = 7;
pub static PLATFORMS: [Platform; NUM_PLATFORMS] = [
    Platform { name: "None", target_lufs: 0.0, turns_up: false },
    Platform { name: "Spotify", target_lufs: -14.0, turns_up: true },
    Platform { name: "Apple Music", target_lufs: -16.0, turns_up: true },
    Platform { name: "YouTube", target_lufs: -14.0, turns_up: false },
    Platform { name: "TIDAL", target_lufs: -14.0, turns_up: false },
    Platform { name: "Amazon Music", target_lufs: -14.0, turns_up: false },
    Platform { name: "Deezer", target_lufs: -15.0, turns_up: false },
];

/// `platform (i)`: clamped into the table.
pub fn platform(i: i32) -> &'static Platform {
    let at = if i < 0 {
        0
    } else if i >= NUM_PLATFORMS as i32 {
        NUM_PLATFORMS - 1
    } else {
        i as usize
    };
    &PLATFORMS[at]
}

/// `platformGainDb`: the gain in dB a platform applies on playback to a master measured at this
/// integrated loudness and true peak. 0 for "None", or before anything has been measured.
pub fn platform_gain_db(platform_index: i32, integrated_lufs: f32, true_peak_dbtp: f32) -> f32 {
    let p = platform(platform_index);
    if platform_index <= 0 || integrated_lufs <= -69.0 {
        return 0.0;
    }
    let g = p.target_lufs - integrated_lufs;
    if g <= 0.0 {
        return g; // everybody turns loud masters down
    }
    if !p.turns_up {
        return 0.0;
    }
    // std::max (0.0f, std::min (g, -1.0f - truePeakDbtp))
    let m = if (-1.0 - true_peak_dbtp) < g { -1.0 - true_peak_dbtp } else { g };
    if 0.0 < m { m } else { 0.0 }
}

// ---------------------------------------------------------------- unit maps (the original's)
/// (`f32::clamp` keeps a NaN, as the C++'s comparisons do.)
fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}
fn clamp_t<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}
fn max_f(a: f32, b: f32) -> f32 {
    // std::max (a, b): `(a < b) ? b : a`
    if a < b { b } else { a }
}
fn max_d(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

pub fn map_lin(n: f32, lo: f32, hi: f32) -> f32 {
    lo + clamp01(n) * (hi - lo)
}
pub fn knob_to_attack(v: f32) -> f32 {
    map_lin(v, 0.1, 5.0)
}
pub fn knob_to_release(v: f32) -> f32 {
    map_lin(v, 0.5, 20.0)
}
pub fn knob_to_gate(v: f32) -> f32 {
    (map_lin(v, 3.0, 40.0) + 0.5).floor()
}
pub fn knob_to_max_gain(v: f32) -> f32 {
    map_lin(v, 0.0, 24.0)
}
pub fn knob_to_target(v: f32) -> f32 {
    (map_lin(v, -50.0, -6.0) + 0.5).floor()
}
pub fn target_to_knob(lufs: f32) -> f32 {
    (lufs + 50.0) / 44.0
}

// ---------------------------------------------------------------- a reference track
/// `Reference::toFloat`.
pub fn ref_to_float(v: i16) -> f32 {
    f32::from(v) * (1.0 / 32767.0)
}
/// `Reference::fromFloat`: clamped, `std::lround (c * 32767)`. (A NaN, which the host never
/// hands over, becomes 0.)
pub fn ref_from_float(v: f32) -> i16 {
    let c = v.clamp(-1.0, 1.0);
    (c * 32767.0).round() as i16
}

/// The reference for one block: the decoded samples at the session rate (each played as
/// `toFloat (fromFloat (x))`, the C++'s 16-bit copy) and the integrated loudness measured on load.
#[derive(Debug, Clone, Copy)]
pub struct RefView<'a> {
    pub left: &'a [f32],
    pub right: &'a [f32],
    pub lufs: f32,
}

impl RefView<'_> {
    /// `valid()`.
    pub fn valid(&self) -> bool {
        !self.left.is_empty() && self.left.len() == self.right.len()
    }
}

// ---------------------------------------------------------------- biquad
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
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

/// K-weighting (ITU-R BS.1770) shelf, coefficients derived per sample rate.
fn make_shelf(fs: f64) -> Biquad {
    let f0 = 1681.974450955533f64;
    let g = 3.999843853973347f64;
    let q = 0.7071752369554196f64;
    let k = (PI * f0 / fs).tan();
    let vh = 10.0f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    Biquad {
        b0: ((vh + vb * k / q + k * k) / a0) as f32,
        b1: ((2.0 * (k * k - vh)) / a0) as f32,
        b2: ((vh - vb * k / q + k * k) / a0) as f32,
        a1: ((2.0 * (k * k - 1.0)) / a0) as f32,
        a2: ((1.0 - k / q + k * k) / a0) as f32,
        ..Biquad::default()
    }
}

fn make_hpf(fs: f64) -> Biquad {
    let f0 = 38.13547087602444f64;
    let q = 0.5003270373238773f64;
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad { b0: 1.0, b1: -2.0, b2: 1.0, a1: ((2.0 * (k * k - 1.0)) / a0) as f32, a2: ((1.0 - k / q + k * k) / a0) as f32, ..Biquad::default() }
}

#[inline]
fn filter(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

// ---------------------------------------------------------------- true peak
const OS_PHASES: usize = 4;
const OS_TAPS: usize = 12;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct OsState {
    hist: [f32; OS_TAPS],
    pos: usize,
}

// ---------------------------------------------------------------- delivery histogram
const HIST_LO: f32 = -70.0;
const HIST_STEP: f32 = 0.1;
const HIST_BINS: usize = 800;

/// `class LevelUpDSP`. Plain data (no heap fields): the plugin keeps it in one box made in `new`.
#[derive(Debug, Clone)]
pub struct LevelUpDsp {
    // ---- readouts (the C++ atomics)
    pub short_term_in: f32,
    pub gain_out: f32,
    pub learned_knob: f32,
    pub short_term_out: f32,
    pub integrated_out: f32,
    pub true_peak_out: f32,
    pub ref_gain_out: f32,
    pub preview_gain_out: f32,

    // ======== the original
    attack_time: f32,
    release_time: f32,
    gate_db: f32,
    max_gain_db: f32,
    target_lufs: f32,
    static_mode: bool,
    frozen: bool,
    ceiling_on: bool,
    normalize_request: bool,
    learn_request: bool,
    normalize_held: bool,
    learn_held: bool,

    shelf_l: Biquad,
    shelf_r: Biquad,
    hpf_l: Biquad,
    hpf_r: Biquad,

    block_power: [f32; 30],
    block_index: usize,
    block_sum: f32,
    block_count: i32,
    block_len: i32,

    gain_db: f32,
    target_lin: f32,
    gain_lin: f32,
    current_lufs: f32,
    ceil_lin: f32,
    // ======== end of the original
    external_on: bool,
    external_lufs: f32,
    link_role: i32,

    out_shelf_l: Biquad,
    out_shelf_r: Biquad,
    out_hpf_l: Biquad,
    out_hpf_r: Biquad,
    out_blocks: [f64; 30],
    out_block_index: usize,
    out_block_count: i32,
    out_blocks_seen: i32,
    out_block_sum: f64,

    hist_power: [f64; HIST_BINS],
    hist_count: [i32; HIST_BINS],

    integrated_lufs_now: f32,
    current_out_short: f32,
    true_peak_db: f32,

    os_coef: [[f32; OS_TAPS]; OS_PHASES],
    os_l: OsState,
    os_r: OsState,

    reference_on: bool,
    preview_on: bool,
    platform_index: i32,
    ref_pos: i64,
    ref_gain_lin: f32,
    preview_lin: f32,

    dither_mode: i32,
    shaping_on: bool,
    err_l: f32,
    err_r: f32,
    rng: u32,

    reset_delivery_request: bool,
    /// Preview's gain held across a reset until the meter measures again (`reset_live`).
    preview_hold: Option<f32>,
    bypassed: bool,
    fs: f64,
}

impl Default for LevelUpDsp {
    fn default() -> Self {
        Self::new()
    }
}

/// The xorshift's seed (`rng = 0x9E3779B9u`).
const RNG_SEED: u32 = 0x9E37_79B9;

impl LevelUpDsp {
    /// The C++ member initialisers.
    pub fn new() -> Self {
        LevelUpDsp {
            short_term_in: -100.0,
            gain_out: 0.0,
            learned_knob: -1.0,
            short_term_out: -100.0,
            integrated_out: -100.0,
            true_peak_out: -120.0,
            ref_gain_out: 0.0,
            preview_gain_out: 0.0,
            attack_time: 1.0,
            release_time: 4.0,
            gate_db: 12.0,
            max_gain_db: 12.0,
            target_lufs: -18.0,
            static_mode: false,
            frozen: false,
            ceiling_on: true,
            normalize_request: false,
            learn_request: false,
            normalize_held: false,
            learn_held: false,
            shelf_l: Biquad::default(),
            shelf_r: Biquad::default(),
            hpf_l: Biquad::default(),
            hpf_r: Biquad::default(),
            block_power: [0.0; 30],
            block_index: 0,
            block_sum: 0.0,
            block_count: 0,
            block_len: 4800,
            gain_db: 0.0,
            target_lin: 1.0,
            gain_lin: 1.0,
            current_lufs: -100.0,
            ceil_lin: 0.891,
            external_on: false,
            external_lufs: -18.0,
            link_role: LINK_OFF,
            out_shelf_l: Biquad::default(),
            out_shelf_r: Biquad::default(),
            out_hpf_l: Biquad::default(),
            out_hpf_r: Biquad::default(),
            out_blocks: [0.0; 30],
            out_block_index: 0,
            out_block_count: 0,
            out_blocks_seen: 0,
            out_block_sum: 0.0,
            hist_power: [0.0; HIST_BINS],
            hist_count: [0; HIST_BINS],
            integrated_lufs_now: -100.0,
            current_out_short: -100.0,
            true_peak_db: -120.0,
            os_coef: [[0.0; OS_TAPS]; OS_PHASES],
            os_l: OsState::default(),
            os_r: OsState::default(),
            reference_on: false,
            preview_on: false,
            platform_index: 0,
            ref_pos: 0,
            ref_gain_lin: 1.0,
            preview_lin: 1.0,
            dither_mode: DITHER_OFF,
            shaping_on: false,
            err_l: 0.0,
            err_r: 0.0,
            rng: RNG_SEED,
            reset_delivery_request: false,
            preview_hold: None,
            bypassed: false,
            fs: 48_000.0,
        }
    }

    // ---------------------------------------------------------------- parameters in
    /// `setParameter`. `std::lround` of a choice value, then the `(int)`.
    pub fn set_parameter(&mut self, index: usize, v: f32) {
        let lround = |v: f32| v.round() as i32;
        match index {
            P_ATTACK => self.attack_time = knob_to_attack(v),
            P_RELEASE => self.release_time = knob_to_release(v),
            P_GATE => self.gate_db = knob_to_gate(v),
            P_MAX_GAIN => self.max_gain_db = knob_to_max_gain(v),
            P_TARGET => self.target_lufs = knob_to_target(v),
            P_MODE => self.static_mode = v > 0.5,
            P_NORMALIZE => {
                let on = v > 0.5;
                if on && !self.normalize_held {
                    self.normalize_request = true;
                }
                self.normalize_held = on;
            }
            P_FREEZE => self.frozen = v > 0.5,
            P_CEILING => self.ceiling_on = v > 0.5,
            P_LEARN => {
                let on = v > 0.5;
                if on && !self.learn_held {
                    self.learn_request = true;
                }
                self.learn_held = on;
            }
            P_LINK => self.link_role = lround(v),
            P_LINK_GROUP => {} // the processor's business
            P_REFERENCE => self.reference_on = v > 0.5,
            P_REF_SLOT => {} // the processor picks the slot
            P_PLATFORM => self.platform_index = lround(v),
            P_PREVIEW => self.preview_on = v > 0.5,
            P_DITHER => self.dither_mode = lround(v),
            P_SHAPING => self.shaping_on = v > 0.5,
            P_BYPASS => self.bypassed = v > 0.5,
            _ => {}
        }
    }

    /// `setExternalTarget`: an OUT linked to an IN aims at the IN's short-term loudness.
    pub fn set_external_target(&mut self, active: bool, lufs: f32) {
        self.external_on = active;
        self.external_lufs = lufs;
    }

    /// `resetDelivery`: clears integrated loudness and the true-peak hold at the next block.
    pub fn reset_delivery(&mut self) {
        self.reset_delivery_request = true;
    }

    /// The rate `prepare` was given.
    pub fn sample_rate(&self) -> f64 {
        self.fs
    }

    // ---------------------------------------------------------------- init
    /// `prepare`. Keeps what the C++ keeps: the parameters, the momentaries' held state and
    /// pending requests, the link target, the readouts it does not store here, `outBlockCount`,
    /// `currentOutShort` and the dither's random state (see [`LevelUpDsp::clear_history`]).
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = sample_rate;
        self.block_len = ((self.fs * 0.1) as i32).max(1);
        self.shelf_l = make_shelf(self.fs);
        self.shelf_r = make_shelf(self.fs);
        self.hpf_l = make_hpf(self.fs);
        self.hpf_r = make_hpf(self.fs);
        self.out_shelf_l = make_shelf(self.fs);
        self.out_shelf_r = make_shelf(self.fs);
        self.out_hpf_l = make_hpf(self.fs);
        self.out_hpf_r = make_hpf(self.fs);

        self.block_power = [0.0; 30];
        self.block_index = 0;
        self.block_sum = 0.0;
        self.block_count = 0;
        self.out_blocks = [0.0; 30];
        self.out_block_index = 0;
        self.out_block_sum = 0.0;
        self.out_blocks_seen = 0;
        self.gain_db = 0.0;
        self.target_lin = 1.0;
        self.gain_lin = 1.0;
        self.current_lufs = -100.0;

        self.build_oversampler();
        self.reset_delivery_now();
        self.ref_pos = 0;
        self.ref_gain_lin = 1.0;
        self.preview_lin = 1.0;
        self.err_l = 0.0;
        self.err_r = 0.0;
    }

    /// The DAW's reset (every Play, Stop and Locate): the C++ `prepare` on the live instance, with
    /// the controller's rulings for a DAW (a Rust addition; the first `prepare` stays the C++'s):
    /// - **the ride is kept**: `gainDb`, `targetLin` and `gainLin` are put back after `prepare`,
    ///   and so is the leveler's short-term window (`blockPower`, its index and `currentLufs`), so
    ///   the gain neither steps nor pumps for 3 s after a Locate;
    /// - **the reference does not jump**: its gain smoother (`refGainLin`) and the loudness it is
    ///   matched to before an integrated reading exists (`currentOutShort`, with the delivery
    ///   meter's 3 s short-term window behind it) are kept;
    /// - **Preview holds** its last gain until the delivery meter has measured again (an
    ///   integrated reading), instead of dropping to 0 dB and gliding back;
    /// - **the delivery meter restarts**: the integrated histogram, its 400 ms gating count and
    ///   the true-peak hold (`resetDeliveryNow`), as the C++ `prepare` does, and its filters;
    /// - cleared besides: `outBlockCount` (the meter's position in its 100 ms block), the dither's
    ///   xorshift state; the readouts are re-seated from the kept state.
    pub fn reset_live(&mut self) {
        let (gain_db, target_lin, gain_lin) = (self.gain_db, self.target_lin, self.gain_lin);
        let (block_power, block_index, current_lufs) = (self.block_power, self.block_index, self.current_lufs);
        let (out_blocks, out_block_index, current_out_short) = (self.out_blocks, self.out_block_index, self.current_out_short);
        let (ref_gain_lin, preview_lin) = (self.ref_gain_lin, self.preview_lin);
        let hold = self.preview_on.then_some(self.preview_gain_out);
        let fs = self.fs;
        self.prepare(fs);
        self.gain_db = gain_db;
        self.target_lin = target_lin;
        self.gain_lin = gain_lin;
        self.block_power = block_power;
        self.block_index = block_index;
        self.current_lufs = current_lufs;
        self.out_blocks = out_blocks;
        self.out_block_index = out_block_index;
        self.current_out_short = current_out_short;
        self.ref_gain_lin = ref_gain_lin;
        self.preview_lin = preview_lin;
        self.preview_hold = hold;
        self.out_block_count = 0;
        self.rng = RNG_SEED;
        self.short_term_in = current_lufs;
        self.gain_out = gain_db;
        self.short_term_out = current_out_short;
    }

    // ---------------------------------------------------------------- process
    /// `process (ch, numChannels, numSamples, ref, hostPos)`: `io[0]` (and `io[1]` when
    /// `num_channels > 1`; with one channel the left is read as both sides and only it written).
    /// `reference` is what to play when Reference is on (`None`: none); `host_pos` the timeline
    /// position of the block's first sample, or -1 when the transport is not playing.
    pub fn process(&mut self, io: &mut [Vec<f32>], num_channels: usize, num_samples: usize, reference: Option<RefView<'_>>, host_pos: i64) {
        if num_channels == 0 || io.is_empty() {
            return;
        }

        if std::mem::take(&mut self.reset_delivery_request) {
            self.reset_delivery_now();
        }

        if self.bypassed {
            return;
        }

        // The reference follows the song position when the host has one; otherwise it runs on.
        let reference = reference.filter(|r| self.reference_on && r.valid());
        let ref_playing = reference.is_some();
        if let Some(r) = reference
            && host_pos >= 0
        {
            self.ref_pos = host_pos % r.left.len() as i64;
        }

        // How loud the mix is, for matching the reference to it: integrated once there is enough
        // of it, short-term before that.
        let mix_loud = if self.integrated_lufs_now > -69.0 { self.integrated_lufs_now } else { self.current_out_short };
        let ref_gain_db = match reference {
            Some(r) if mix_loud > -69.0 => clamp_t(mix_loud - r.lufs, -24.0, 24.0),
            _ => 0.0,
        };
        let ref_target = 10.0f32.powf(ref_gain_db / 20.0);

        // (Rust addition, see `reset_live`: after a reset Preview holds its last gain until the
        // delivery meter has an integrated reading again.)
        if !self.preview_on || self.integrated_lufs_now > -69.0 {
            self.preview_hold = None;
        }
        let prev_db = match (self.preview_on, self.preview_hold) {
            (false, _) => 0.0,
            (true, Some(h)) => h,
            (true, None) => platform_gain_db(self.platform_index, self.integrated_lufs_now, self.true_peak_db),
        };
        let prev_target = 10.0f32.powf(prev_db / 20.0);

        let stereo = num_channels > 1 && io.len() > 1;
        let (first, rest) = io.split_at_mut(1);
        let l_buf = &mut first[0];
        let mut r_buf = if stereo { rest.first_mut() } else { None };
        let n_max = num_samples.min(l_buf.len()).min(r_buf.as_ref().map_or(usize::MAX, |r| r.len()));

        for n in 0..n_max {
            // ======== the original leveler ========
            let x0 = l_buf[n];
            let x1 = match r_buf.as_ref() {
                Some(r) => r[n],
                None => x0,
            };
            let l = filter(&mut self.hpf_l, filter(&mut self.shelf_l, x0));
            let r = filter(&mut self.hpf_r, filter(&mut self.shelf_r, x1));
            self.block_sum += l * l + r * r;

            self.block_count += 1;
            if self.block_count >= self.block_len {
                self.update_block();
                self.block_sum = 0.0;
                self.block_count = 0;
            }

            self.gain_lin += (self.target_lin - self.gain_lin) * 0.001; // ~20 ms click-free smoothing
            let mut y0 = x0 * self.gain_lin;
            let mut y1 = x1 * self.gain_lin;

            if self.ceiling_on {
                y0 = self.soft_ceil(y0);
                y1 = self.soft_ceil(y1);
            }
            // ======== end of the original ========

            // ---- the mix is measured here, before a reference or a preview replaces it
            self.measure_out(y0, y1);

            // ---- a reference, in place of the mix
            if let Some(rf) = reference {
                self.ref_gain_lin += (ref_target - self.ref_gain_lin) * 0.001;
                let i = self.ref_pos as usize;
                let (a, b) = (rf.left.get(i).copied().unwrap_or(0.0), rf.right.get(i).copied().unwrap_or(0.0));
                y0 = ref_to_float(ref_from_float(a)) * self.ref_gain_lin;
                y1 = ref_to_float(ref_from_float(b)) * self.ref_gain_lin;
                if self.ceiling_on {
                    y0 = self.soft_ceil(y0);
                    y1 = self.soft_ceil(y1);
                }
                self.ref_pos += 1;
                if self.ref_pos >= rf.left.len() as i64 {
                    self.ref_pos = 0;
                }
            }

            // ---- what the platform would do to it
            self.preview_lin += (prev_target - self.preview_lin) * 0.001;
            y0 *= self.preview_lin;
            y1 *= self.preview_lin;

            // ---- dither, last of all
            if self.dither_mode != DITHER_OFF {
                let mut e = self.err_l;
                y0 = self.dither(y0, &mut e);
                self.err_l = e;
                let mut e = self.err_r;
                y1 = self.dither(y1, &mut e);
                self.err_r = e;
            }

            l_buf[n] = y0;
            if let Some(r) = r_buf.as_mut() {
                r[n] = y1;
            }
        }

        self.ref_gain_out = if ref_playing { ref_gain_db } else { 0.0 };
        self.preview_gain_out = prev_db;
    }

    // ---------------------------------------------------------------- loudness, measuring
    /// `meter (l, r)`: the delivery meter on its own, sample by sample. `prepare` first.
    pub fn meter(&mut self, l: f32, r: f32) {
        self.measure_out(l, r);
    }
    /// `integratedNow`.
    pub fn integrated_now(&self) -> f32 {
        self.integrated_lufs_now
    }
    /// `truePeakNow`.
    pub fn true_peak_now(&self) -> f32 {
        self.true_peak_db
    }

    /// `measureIntegrated`: the integrated loudness (and true peak) of a whole stereo buffer.
    pub fn measure_integrated(left: &[f32], right: &[f32], sample_rate: f64) -> (f32, f32) {
        let mut m = Box::new(LevelUpDsp::new());
        m.prepare(sample_rate);
        for (l, r) in left.iter().zip(right) {
            m.meter(*l, *r);
        }
        (m.integrated_now(), m.true_peak_now())
    }

    // ======================================================================== the original
    /// Soft peak ceiling: transparent below 80% of ceiling, soft knee above.
    fn soft_ceil(&self, x: f32) -> f32 {
        let a = x.abs();
        let knee = self.ceil_lin * 0.8;
        if a <= knee {
            return x;
        }
        let over = (a - knee) / (self.ceil_lin - knee);
        let y = knee + (self.ceil_lin - knee) * over.tanh();
        if x < 0.0 { -y } else { y }
    }

    fn update_block(&mut self) {
        self.block_power[self.block_index] = self.block_sum / self.block_len as f32;
        self.block_index = (self.block_index + 1) % 30;

        let mut sum = 0.0f32;
        for i in 0..30 {
            sum += self.block_power[i];
        }
        let avg = sum / 30.0;

        self.current_lufs = -0.691 + 10.0 * max_f(avg, 1.0e-12).log10();
        self.short_term_in = self.current_lufs;

        if self.learn_request {
            if self.current_lufs > -70.0 {
                self.target_lufs = clamp_t((self.current_lufs + 0.5).floor(), -50.0, -6.0);
                self.learned_knob = (self.target_lufs + 50.0) / 44.0;
            }
            self.learn_request = false;
        }

        // An OUT linked to an IN aims at the IN's loudness instead of the knob.
        let target = if self.external_on { clamp_t(self.external_lufs, -70.0, 0.0) } else { self.target_lufs };

        let gated = self.current_lufs < target - self.gate_db;
        let wanted = clamp_t(target - self.current_lufs, -self.max_gain_db, self.max_gain_db);

        if self.normalize_request {
            if !gated {
                self.gain_db = wanted;
            }
            self.normalize_request = false;
        } else if !self.static_mode && !self.frozen && !gated {
            let t = if wanted < self.gain_db { self.attack_time } else { self.release_time };
            let coef = (-0.1f32 / t).exp();
            self.gain_db = wanted + (self.gain_db - wanted) * coef;
        }

        self.target_lin = 10.0f32.powf(self.gain_db / 20.0);
        self.gain_out = self.gain_db;
    }
    // ======================================================================== end of the original

    // ---------------------------------------------------------------- delivery metering
    fn reset_delivery_now(&mut self) {
        self.hist_power = [0.0; HIST_BINS];
        self.hist_count = [0; HIST_BINS];
        self.integrated_lufs_now = -100.0;
        self.true_peak_db = -120.0;
        self.integrated_out = -100.0;
        self.true_peak_out = -120.0;
    }

    fn measure_out(&mut self, y0: f32, y1: f32) {
        let l = filter(&mut self.out_hpf_l, filter(&mut self.out_shelf_l, y0));
        let r = filter(&mut self.out_hpf_r, filter(&mut self.out_shelf_r, y1));
        self.out_block_sum += f64::from(l) * f64::from(l) + f64::from(r) * f64::from(r);

        let mut st = self.os_l;
        self.true_peak(y0, &mut st);
        self.os_l = st;
        let mut st = self.os_r;
        self.true_peak(y1, &mut st);
        self.os_r = st;

        self.out_block_count += 1;
        if self.out_block_count < self.block_len {
            return;
        }

        self.out_blocks[self.out_block_index] = self.out_block_sum / f64::from(self.block_len);
        self.out_block_index = (self.out_block_index + 1) % 30;
        self.out_block_sum = 0.0;
        self.out_block_count = 0;
        self.out_blocks_seen += 1;

        // short-term: the last 3 s
        let mut st = 0.0f64;
        for p in self.out_blocks {
            st += p;
        }
        self.current_out_short = -0.691 + 10.0 * (max_d(st / 30.0, 1.0e-12).log10() as f32);
        self.short_term_out = self.current_out_short;

        // momentary block: the last 400 ms, which is what BS.1770 gates on
        if self.out_blocks_seen >= 4 {
            let mut m = 0.0f64;
            for k in 1..=4usize {
                m += self.out_blocks[(self.out_block_index + 30 - k) % 30];
            }
            m /= 4.0;
            let lufs = -0.691 + 10.0 * (max_d(m, 1.0e-12).log10() as f32);
            if lufs > HIST_LO {
                let bin = clamp_t(((lufs - HIST_LO) / HIST_STEP) as i32, 0, HIST_BINS as i32 - 1) as usize;
                self.hist_power[bin] += m;
                self.hist_count[bin] += 1;
            }
            self.integrated_lufs_now = self.integrated();
            self.integrated_out = self.integrated_lufs_now;
        }

        self.true_peak_out = self.true_peak_db;
    }

    fn integrated(&self) -> f32 {
        // absolute gate already applied by the histogram's floor; now the relative one
        let mut p = 0.0f64;
        let mut c: i64 = 0;
        for b in 0..HIST_BINS {
            p += self.hist_power[b];
            c += i64::from(self.hist_count[b]);
        }
        if c == 0 {
            return -100.0;
        }
        let ungated = -0.691 + 10.0 * ((p / c as f64).log10() as f32);
        let rel_gate = ungated - 10.0;

        p = 0.0;
        c = 0;
        for b in 0..HIST_BINS {
            let bin_lufs = HIST_LO + (b as f32 + 0.5) * HIST_STEP;
            if bin_lufs <= rel_gate {
                continue;
            }
            p += self.hist_power[b];
            c += i64::from(self.hist_count[b]);
        }
        if c == 0 {
            return -100.0;
        }
        -0.691 + 10.0 * ((p / c as f64).log10() as f32)
    }

    /// 4x oversampling by a 48-tap polyphase windowed sinc (Kaiser, beta 5), each phase
    /// normalised to sum to one.
    fn build_oversampler(&mut self) {
        let n = (OS_PHASES * OS_TAPS) as i32;
        let i0 = |v: f64| {
            let mut s = 1.0f64;
            let mut term = 1.0f64;
            for m in 1..25 {
                let h = v / (2.0 * f64::from(m));
                term *= h * h;
                s += term;
            }
            s
        };
        for k in 0..n {
            let t = (f64::from(k) - f64::from(n - 1) * 0.5) / OS_PHASES as f64; // in input samples
            let sinc = if t.abs() < 1e-9 { 1.0 } else { (PI * t).sin() / (PI * t) };
            let x = 2.0 * f64::from(k) / f64::from(n - 1) - 1.0;
            let w = i0(5.0 * max_d(0.0, 1.0 - x * x).sqrt()) / i0(5.0);
            self.os_coef[(k as usize) % OS_PHASES][(k as usize) / OS_PHASES] = (sinc * w) as f32;
        }
        for ph in self.os_coef.iter_mut() {
            let mut s = 0.0f32;
            for c in ph.iter() {
                s += *c;
            }
            if s.abs() > 1e-6 {
                for c in ph.iter_mut() {
                    *c /= s;
                }
            }
        }
        self.os_l = OsState::default();
        self.os_r = OsState::default();
    }

    fn true_peak(&mut self, x: f32, st: &mut OsState) {
        st.hist[st.pos] = x;
        let mut peak = 0.0f32;
        for ph in 0..OS_PHASES {
            let mut acc = 0.0f32;
            for k in 0..OS_TAPS {
                acc += self.os_coef[ph][k] * st.hist[(st.pos + OS_TAPS - k) % OS_TAPS];
            }
            peak = max_f(peak, acc.abs());
        }
        st.pos = (st.pos + 1) % OS_TAPS;

        let db = 20.0 * max_f(peak, 1.0e-6).log10();
        if db > self.true_peak_db {
            self.true_peak_db = db;
        }
    }

    // ---------------------------------------------------------------- dither
    fn uniform(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 * (1.0 / 16_777_216.0) - 0.5
    }

    fn dither(&mut self, x: f32, err: &mut f32) -> f32 {
        let bits = if self.dither_mode == DITHER_16 { 16.0f32 } else { 24.0 };
        let lsb = 1.0 / 2.0f32.powf(bits - 1.0);
        let v = if self.shaping_on { x - *err } else { x };
        // (uniform() + uniform()): the C++ evaluates the two calls left to right here
        let u1 = self.uniform();
        let u2 = self.uniform();
        let noisy = v + (u1 + u2) * lsb;
        let q = (noisy / lsb).round() * lsb;
        *err = q - v;
        q
    }

    // ---------------------------------------------------------------- tests
    #[cfg(test)]
    pub(crate) fn gain_db(&self) -> f32 {
        self.gain_db
    }
    #[cfg(test)]
    pub(crate) fn target_lufs(&self) -> f32 {
        self.target_lufs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_and_platforms_match_the_header() {
        assert_eq!(knob_to_target(0.72727), -18.0);
        assert_eq!(knob_to_gate(0.24324), 12.0);
        assert!((knob_to_attack(0.18367) - 1.0).abs() < 1e-4);
        assert!((knob_to_release(0.17949) - 4.0).abs() < 1e-4);
        assert_eq!(knob_to_max_gain(0.5), 12.0);
        assert_eq!(platform(-3).name, "None");
        assert_eq!(platform(99).name, "Deezer");
        // loud masters are turned down by everybody, quiet ones up only by Spotify and Apple, to -1 dBTP
        assert_eq!(platform_gain_db(3, -9.0, -0.5), -5.0);
        assert_eq!(platform_gain_db(3, -20.0, -6.0), 0.0);
        assert_eq!(platform_gain_db(1, -20.0, -6.0), 5.0);
        assert_eq!(platform_gain_db(1, -20.0, -3.0), 2.0);
        assert_eq!(platform_gain_db(1, -20.0, 1.0), 0.0);
        assert_eq!(platform_gain_db(0, -20.0, -6.0), 0.0);
        assert_eq!(platform_gain_db(1, -80.0, -6.0), 0.0);
        assert_eq!(ref_from_float(2.0), 32767);
        assert_eq!(ref_from_float(-0.5), -16384);
        assert_eq!(ref_to_float(32767), 1.0);
    }

    fn sine(len: usize, sr: f64, f: f64, amp: f64) -> Vec<f32> {
        (0..len).map(|i| (amp * (std::f64::consts::TAU * f * i as f64 / sr).sin()) as f32).collect()
    }

    #[test]
    fn ebu_tech_3341_cases() {
        // a stereo 997 Hz sine at -23 dBFS reads -23 LUFS, at 48 and 44.1 kHz
        for sr in [48_000.0, 44_100.0] {
            let x = sine((20.0 * sr) as usize, sr, 997.0, 10f64.powf(-23.0 / 20.0));
            let (lufs, _) = LevelUpDsp::measure_integrated(&x, &x, sr);
            assert!((lufs + 23.0).abs() < 0.1, "{sr}: {lufs}");
        }
        // one channel reads 3 dB less
        let sr = 48_000.0;
        let x = sine((20.0 * sr) as usize, sr, 997.0, 10f64.powf(-23.0 / 20.0));
        let z = vec![0.0; x.len()];
        let (lufs, _) = LevelUpDsp::measure_integrated(&x, &z, sr);
        assert!((lufs + 26.0).abs() < 0.1, "{lufs}");
        // silence does not pull a reading down (the absolute gate)
        let mut y = x.clone();
        y.extend(std::iter::repeat_n(0.0, (20.0 * sr) as usize));
        let (lufs, _) = LevelUpDsp::measure_integrated(&y, &y, sr);
        assert!((lufs + 23.0).abs() < 0.1, "{lufs}");
        // nothing measured
        assert_eq!(LevelUpDsp::measure_integrated(&[], &[], sr).0, -100.0);
    }

    #[test]
    fn true_peak_finds_the_peak_between_samples() {
        // a sine at fs/4 whose samples sit 3 dB under its peak reads about 0 dBTP
        let sr = 48_000.0;
        let x: Vec<f32> = (0..48_000).map(|i| (std::f64::consts::FRAC_PI_2 * i as f64 + std::f64::consts::FRAC_PI_4).sin() as f32).collect();
        let (_, tp) = LevelUpDsp::measure_integrated(&x, &x, sr);
        assert!(tp > -0.5 && tp < 0.5, "{tp}");
    }
}
