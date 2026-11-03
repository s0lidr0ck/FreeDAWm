//! `AmpEngineDSP.h`: the plugin-facing engine. Pick an amp, set its knobs, run a block: the input
//! calibration (0 dBFS = 2 V at the jack), the amp, the built-in cabinet, Auto Level, the Output
//! trim and the meters.
//!
//! Translated line by line. Rust additions: the engine holds the seven amps at both oversampling
//! factors (prepared together), so the wrapper's oversampling re-prepare is a switch plus
//! [`AmpEngineDsp::reset_fresh`] instead of a rebuild on the audio thread; and the meters are
//! plain values written per block (the C++ atomics' `take` is the editor's).

use super::amp_models::{Ac30, Controls, FenderReverb, K_DELUXE_CONFIG, K_PRINCETON_CONFIG, K_SUPER_CONFIG, K_TWIN_CONFIG, Marshall, Orange};
use super::amp_power::{
    Biquad, K_AC30_POWER, K_DELUXE_POWER, K_MARSHALL_POWER, K_OR120_POWER, K_PRINCETON_POWER, K_SUPER_POWER, K_TWIN_POWER, PowerAmp, PowerParts,
};
use super::amp_tube::cmax;

pub const AMP_TWIN: usize = 0;
pub const AMP_MARSHALL: usize = 1;
pub const AMP_DELUXE: usize = 2;
pub const AMP_SUPER: usize = 3;
pub const AMP_PRINCETON: usize = 4;
pub const AMP_AC30: usize = 5;
pub const AMP_ORANGE: usize = 6;
pub const NUM_AMPS: usize = 7;

pub const CAB_BUILT_IN: i32 = 0;
pub const CAB_IMPULSE: i32 = 1;
pub const CAB_OFF: i32 = 2;

// ---- ParamIndex (the order IS the parameter order; append only)
pub const P_AMP: usize = 0;
pub const P_INPUT: usize = 1;
pub const P_VOLUME: usize = 2;
pub const P_BRIGHT: usize = 3;
pub const P_TREBLE: usize = 4;
pub const P_MIDDLE: usize = 5;
pub const P_BASS: usize = 6;
pub const P_REVERB: usize = 7;
pub const P_SPEED: usize = 8;
pub const P_INTENSITY: usize = 9;
pub const P_MASTER: usize = 10;
pub const P_PULL_BOOST: usize = 11;
pub const P_VOLUME2: usize = 12;
pub const P_PRESENCE: usize = 13;
pub const P_CAB: usize = 14;
pub const P_OUTPUT: usize = 15;
pub const P_QUALITY: usize = 16;
pub const P_BYPASS: usize = 17;
pub const P_CHANNEL: usize = 18;
pub const P_JTM_VOLUME: usize = 19;
pub const P_JTM_TREBLE: usize = 20;
pub const P_JTM_MIDDLE: usize = 21;
pub const P_JTM_BASS: usize = 22;
pub const P_JTM_SPEED: usize = 23;
pub const P_JTM_INTENSITY: usize = 24;
pub const P_JTM_INPUT: usize = 25;
pub const P_JTM_OUTPUT: usize = 26;
pub const P_JTM_CAB: usize = 27;
pub const P_AUTO_LEVEL: usize = 28;
pub const P_JTM_AUTO_LEVEL: usize = 29;
pub const P_DLX_VOLUME: usize = 30;
pub const P_DLX_TREBLE: usize = 31;
pub const P_DLX_BASS: usize = 32;
pub const P_DLX_REVERB: usize = 33;
pub const P_DLX_SPEED: usize = 34;
pub const P_DLX_INTENSITY: usize = 35;
pub const P_DLX_INPUT: usize = 36;
pub const P_DLX_OUTPUT: usize = 37;
pub const P_DLX_CAB: usize = 38;
pub const P_DLX_AUTO_LEVEL: usize = 39;
pub const P_DLX_CHANNEL: usize = 40;
pub const P_SUP_VOLUME: usize = 41;
pub const P_SUP_TREBLE: usize = 42;
pub const P_SUP_MIDDLE: usize = 43;
pub const P_SUP_BASS: usize = 44;
pub const P_SUP_REVERB: usize = 45;
pub const P_SUP_SPEED: usize = 46;
pub const P_SUP_INTENSITY: usize = 47;
pub const P_SUP_INPUT: usize = 48;
pub const P_SUP_OUTPUT: usize = 49;
pub const P_SUP_CAB: usize = 50;
pub const P_SUP_AUTO_LEVEL: usize = 51;
pub const P_SUP_CHANNEL: usize = 52;
pub const P_SUP_BRIGHT: usize = 53;
pub const P_PRN_VOLUME: usize = 54;
pub const P_PRN_TREBLE: usize = 55;
pub const P_PRN_BASS: usize = 56;
pub const P_PRN_REVERB: usize = 57;
pub const P_PRN_SPEED: usize = 58;
pub const P_PRN_INTENSITY: usize = 59;
pub const P_PRN_INPUT: usize = 60;
pub const P_PRN_OUTPUT: usize = 61;
pub const P_PRN_CAB: usize = 62;
pub const P_PRN_AUTO_LEVEL: usize = 63;
pub const P_AC_CHANNEL: usize = 64;
pub const P_AC_BRILLIANT: usize = 65;
pub const P_AC_NORMAL: usize = 66;
pub const P_AC_TREBLE: usize = 67;
pub const P_AC_BASS: usize = 68;
pub const P_AC_CUT: usize = 69;
pub const P_AC_INPUT: usize = 70;
pub const P_AC_OUTPUT: usize = 71;
pub const P_AC_CAB: usize = 72;
pub const P_AC_AUTO_LEVEL: usize = 73;
pub const P_OR_GAIN: usize = 74;
pub const P_OR_BASS: usize = 75;
pub const P_OR_TREBLE: usize = 76;
pub const P_OR_FAC: usize = 77;
pub const P_OR_HF_DRIVE: usize = 78;
pub const P_OR_INPUT: usize = 79;
pub const P_OR_OUTPUT: usize = 80;
pub const P_OR_CAB: usize = 81;
pub const P_OR_AUTO_LEVEL: usize = 82;
pub const NUM_PARAMS: usize = 83;

/// The parameter that plays a role for an amp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Volume = 0,
    Treble,
    Middle,
    Bass,
    Speed,
    Intensity,
    Input,
    Output,
    Cab,
    AutoLevel,
    Reverb,
    Channel,
    Bright,
}

const N: i32 = -1;
#[rustfmt::skip]
const ROLES: [[i32; 13]; NUM_AMPS] = [
    [P_VOLUME as i32, P_TREBLE as i32, P_MIDDLE as i32, P_BASS as i32, P_SPEED as i32, P_INTENSITY as i32, P_INPUT as i32, P_OUTPUT as i32, P_CAB as i32, P_AUTO_LEVEL as i32,
     P_REVERB as i32, P_CHANNEL as i32, P_BRIGHT as i32],
    [P_JTM_VOLUME as i32, P_JTM_TREBLE as i32, P_JTM_MIDDLE as i32, P_JTM_BASS as i32, P_JTM_SPEED as i32, P_JTM_INTENSITY as i32, P_JTM_INPUT as i32, P_JTM_OUTPUT as i32,
     P_JTM_CAB as i32, P_JTM_AUTO_LEVEL as i32, N, N, N],
    [P_DLX_VOLUME as i32, P_DLX_TREBLE as i32, N, P_DLX_BASS as i32, P_DLX_SPEED as i32, P_DLX_INTENSITY as i32, P_DLX_INPUT as i32, P_DLX_OUTPUT as i32,
     P_DLX_CAB as i32, P_DLX_AUTO_LEVEL as i32, P_DLX_REVERB as i32, P_DLX_CHANNEL as i32, N],
    [P_SUP_VOLUME as i32, P_SUP_TREBLE as i32, P_SUP_MIDDLE as i32, P_SUP_BASS as i32, P_SUP_SPEED as i32, P_SUP_INTENSITY as i32, P_SUP_INPUT as i32, P_SUP_OUTPUT as i32,
     P_SUP_CAB as i32, P_SUP_AUTO_LEVEL as i32, P_SUP_REVERB as i32, P_SUP_CHANNEL as i32, P_SUP_BRIGHT as i32],
    [P_PRN_VOLUME as i32, P_PRN_TREBLE as i32, N, P_PRN_BASS as i32, P_PRN_SPEED as i32, P_PRN_INTENSITY as i32, P_PRN_INPUT as i32, P_PRN_OUTPUT as i32,
     P_PRN_CAB as i32, P_PRN_AUTO_LEVEL as i32, P_PRN_REVERB as i32, N, N],
    // (the AC30's "Volume" role is its Brilliant volume; its Normal volume and Cut are its own)
    [P_AC_BRILLIANT as i32, P_AC_TREBLE as i32, N, P_AC_BASS as i32, N, N, P_AC_INPUT as i32, P_AC_OUTPUT as i32,
     P_AC_CAB as i32, P_AC_AUTO_LEVEL as i32, N, N, N],
    [P_OR_GAIN as i32, P_OR_TREBLE as i32, N, P_OR_BASS as i32, N, N, P_OR_INPUT as i32, P_OR_OUTPUT as i32,
     P_OR_CAB as i32, P_OR_AUTO_LEVEL as i32, N, N, N],
];

/// `param(ampIndex, role)`: the parameter that plays `role` for the amp, `None` where it has no
/// such control. An out-of-range amp is the Twin (the C++ switch's default).
pub fn param(amp: usize, r: Role) -> Option<usize> {
    let row = ROLES.get(amp).unwrap_or(&ROLES[AMP_TWIN]);
    usize::try_from(row[r as usize]).ok()
}

// ---- knob maps (shared with the readouts)
/// -24 .. +24 dB.
pub fn knob_to_trim_db(v: f32) -> f32 {
    (v - 0.5) * 48.0
}
/// 0 .. 10 on the panel.
pub fn knob_to_amp_dial(v: f32) -> f32 {
    v * 10.0
}

pub const INPUT_VOLTS_AT_FULL_SCALE: f64 = 2.0;

/// Each amp's rating and speaker load, and its power section.
#[derive(Debug, Clone, Copy)]
pub struct AmpInfo {
    pub rated_watts: f64,
    pub load: f64,
    pub power: &'static PowerParts,
}

static TWIN_POWER: PowerParts = K_TWIN_POWER;
static MARSHALL_POWER: PowerParts = K_MARSHALL_POWER;
static DELUXE_POWER: PowerParts = K_DELUXE_POWER;
static SUPER_POWER: PowerParts = K_SUPER_POWER;
static PRINCETON_POWER: PowerParts = K_PRINCETON_POWER;
static AC30_POWER: PowerParts = K_AC30_POWER;
static OR120_POWER: PowerParts = K_OR120_POWER;

static AMP_INFO: [AmpInfo; NUM_AMPS] = [
    AmpInfo { rated_watts: 135.0, load: 4.0, power: &TWIN_POWER },
    AmpInfo { rated_watts: 45.0, load: 16.0, power: &MARSHALL_POWER },
    AmpInfo { rated_watts: 22.0, load: 8.0, power: &DELUXE_POWER },
    AmpInfo { rated_watts: 45.0, load: 2.0, power: &SUPER_POWER },
    AmpInfo { rated_watts: 12.0, load: 8.0, power: &PRINCETON_POWER },
    AmpInfo { rated_watts: 30.0, load: 8.0, power: &AC30_POWER },
    AmpInfo { rated_watts: 120.0, load: 16.0, power: &OR120_POWER },
];

pub fn amp_info(a: usize) -> &'static AmpInfo {
    &AMP_INFO[a.min(NUM_AMPS - 1)]
}

/// The knobs an amp's model reads, from the parameter values (`applyControls`' `controlsFor`).
pub fn controls_for(a: usize, values: &[f32]) -> Controls {
    let val = |i: usize| f64::from(values.get(i).copied().unwrap_or(0.0));
    let v = |r: Role, otherwise: f64| param(a, r).map_or(otherwise, val);
    let mut c = Controls {
        volume: v(Role::Volume, 0.35),
        treble: v(Role::Treble, 0.5),
        middle: v(Role::Middle, 1.0), // (the Deluxe has none: its stack ignores it)
        bass: v(Role::Bass, 0.5),
        speed: v(Role::Speed, 0.4),
        intensity: v(Role::Intensity, 0.0),
        reverb: v(Role::Reverb, 0.0),
        normal_channel: v(Role::Channel, 0.0) > 0.5,
        bright: v(Role::Bright, 0.0) > 0.5, // Twin, Super (the Deluxe's is always in)
        master: val(P_MASTER),              // Twin only
        pull_boost: values.get(P_PULL_BOOST).copied().unwrap_or(0.0) > 0.5, // Twin only
        volume2: val(P_VOLUME2),            // JTM45 only
        presence: val(P_PRESENCE),          // JTM45 only
        ..Controls::default()
    };
    if a == AMP_AC30 {
        c.volume2 = val(P_AC_NORMAL); // its Normal volume
        c.cut = val(P_AC_CUT);
        c.ac_input = lround(values.get(P_AC_CHANNEL).copied().unwrap_or(0.0));
    }
    if a == AMP_ORANGE {
        c.fac = lround(values.get(P_OR_FAC).copied().unwrap_or(0.0));
        c.hf_drive = val(P_OR_HF_DRIVE);
    }
    c
}

/// `std::lround` (half away from zero), with NaN at 0 (the C++ result is unspecified there).
fn lround(v: f32) -> i32 {
    if v.is_finite() { v.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32 } else { 0 }
}

/// The seven amps at one oversampling factor.
#[derive(Debug, Clone)]
pub struct Amps {
    pub twin: FenderReverb,
    pub marshall: Marshall,
    pub deluxe: FenderReverb,
    pub super_: FenderReverb,
    pub princeton: FenderReverb,
    pub ac30: Ac30,
    pub orange: Orange,
}

impl Default for Amps {
    fn default() -> Self {
        Amps {
            twin: FenderReverb::new(K_TWIN_CONFIG),
            marshall: Marshall::default(),
            deluxe: FenderReverb::new(K_DELUXE_CONFIG),
            super_: FenderReverb::new(K_SUPER_CONFIG),
            princeton: FenderReverb::new(K_PRINCETON_CONFIG),
            ac30: Ac30::default(),
            orange: Orange::default(),
        }
    }
}

impl Amps {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        for f in [&self.twin, &self.deluxe, &self.super_, &self.princeton] {
            f.heap_blocks(out);
        }
        self.marshall.heap_blocks(out);
        self.ac30.heap_blocks(out);
        self.orange.heap_blocks(out);
    }

    fn prepare(&mut self, sr: f64, os: usize) {
        self.twin.prepare(sr, os);
        self.marshall.prepare(sr, os);
        self.deluxe.prepare(sr, os);
        self.super_.prepare(sr, os);
        self.princeton.prepare(sr, os);
        self.ac30.prepare(sr, os);
        self.orange.prepare(sr, os);
    }

    fn reset_fresh(&mut self) {
        self.twin.reset_fresh();
        self.marshall.reset_fresh();
        self.deluxe.reset_fresh();
        self.super_.reset_fresh();
        self.princeton.reset_fresh();
        self.ac30.reset_fresh();
        self.orange.reset_fresh();
    }

    fn set_controls(&mut self, values: &[f32]) {
        self.twin.set_controls(&controls_for(AMP_TWIN, values));
        self.marshall.set_controls(&controls_for(AMP_MARSHALL, values));
        self.deluxe.set_controls(&controls_for(AMP_DELUXE, values));
        self.super_.set_controls(&controls_for(AMP_SUPER, values));
        self.princeton.set_controls(&controls_for(AMP_PRINCETON, values));
        self.ac30.set_controls(&controls_for(AMP_AC30, values));
        self.orange.set_controls(&controls_for(AMP_ORANGE, values));
    }

    #[inline]
    fn process(&mut self, amp: usize, x: f64) -> f64 {
        match amp {
            AMP_MARSHALL => self.marshall.process(x),
            AMP_DELUXE => self.deluxe.process(x),
            AMP_SUPER => self.super_.process(x),
            AMP_PRINCETON => self.princeton.process(x),
            AMP_AC30 => self.ac30.process(x),
            AMP_ORANGE => self.orange.process(x),
            _ => self.twin.process(x),
        }
    }

    fn power(&self, amp: usize) -> &PowerAmp {
        match amp {
            AMP_MARSHALL => self.marshall.power(),
            AMP_DELUXE => self.deluxe.power(),
            AMP_SUPER => self.super_.power(),
            AMP_PRINCETON => self.princeton.power(),
            AMP_AC30 => self.ac30.power(),
            AMP_ORANGE => self.orange.power(),
            _ => self.twin.power(),
        }
    }

    fn latency(&self) -> usize {
        self.twin.latency()
    }
}

/// Auto Level's state, per amp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoLevelState {
    pub di: f64,
    pub out: f64,
    pub target: f64,
    pub gain: f64,
    pub primed_secs: f64,
    pub peak: f64,
    pub limit: f64,
}

impl Default for AutoLevelState {
    fn default() -> Self {
        AutoLevelState { di: 0.0, out: 0.0, target: 1.0, gain: 1.0, primed_secs: 0.0, peak: 0.0, limit: 1.0 }
    }
}

/// The meters, per block (the C++ editor takes running maxima from atomics).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Meters {
    /// Input peak (linear, after the Input trim, relative to full scale).
    pub in_peak: f32,
    /// Output peak (linear).
    pub out_peak: f32,
    /// Watts into the speaker load.
    pub watts: f32,
    pub b_plus: f32,
    pub b_plus_idle: f32,
    pub auto_gain_db: f32,
}

/// The engine.
#[derive(Debug, Clone)]
pub struct AmpEngineDsp {
    sr: f64,
    os: usize,
    /// The amps at 2x (`[0]`) and 4x (`[1]`).
    sets: [Amps; 2],
    mono: Vec<f32>,
    values: [f32; NUM_PARAMS],
    cab_t: [Biquad; 8],
    cab_m: [Biquad; 8],
    cab_s: [Biquad; 8],
    cab_p: [Biquad; 8],
    cab_v: [Biquad; 8],
    cab_o: [Biquad; 8],
    bypass_delay: Vec<f32>,
    bypass_pos: usize,
    amp: usize,
    bypass: bool,
    controls_dirty: bool,
    impulse_ready: bool,
    block_di_mean_square: f64,
    auto_state: [AutoLevelState; NUM_AMPS],
    pub meters: Meters,
}

impl Default for AmpEngineDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl AmpEngineDsp {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for set in &self.sets {
            set.heap_blocks(&mut out);
        }
        super::amp_network::heap_vec(&mut out, &self.mono);
        super::amp_network::heap_vec(&mut out, &self.bypass_delay);
        out
    }

    pub fn new() -> Self {
        AmpEngineDsp {
            sr: 48_000.0,
            os: 2,
            sets: [Amps::default(), Amps::default()],
            mono: Vec::new(),
            values: [0.0; NUM_PARAMS],
            cab_t: [Biquad::default(); 8],
            cab_m: [Biquad::default(); 8],
            cab_s: [Biquad::default(); 8],
            cab_p: [Biquad::default(); 8],
            cab_v: [Biquad::default(); 8],
            cab_o: [Biquad::default(); 8],
            bypass_delay: Vec::new(),
            bypass_pos: 0,
            amp: AMP_TWIN,
            bypass: false,
            controls_dirty: true,
            impulse_ready: false,
            block_di_mean_square: 0.0,
            auto_state: [AutoLevelState::default(); NUM_AMPS],
            meters: Meters { b_plus_idle: 500.0, ..Meters::default() },
        }
    }

    // ------------------------------------------------------------------------------ setup
    /// `prepare(sampleRate, maxBlock, oversampling)`. Allocates; both factors are prepared.
    pub fn prepare(&mut self, sample_rate: f64, max_block: usize, oversampling: usize) {
        self.sr = sample_rate;
        self.os = if oversampling == 2 { 2 } else { 4 };
        self.sets[0].prepare(self.sr, 2);
        self.sets[1].prepare(self.sr, 4);
        self.mono = vec![0.0; max_block.max(1)];
        self.cab_prepare();
        self.bypass_delay = vec![0.0; self.latency_samples() + 1];
        self.bypass_pos = 0;
        self.controls_dirty = true;
    }

    /// (Rust) `prepare` again at the same rate and size, in place: the amps (at `oversampling`)
    /// back where `prepare` leaves them, the cabinet's and the bypass line's history cleared, the
    /// controls re-applied before the next sample. What `prepare` does not touch (the knob values,
    /// Auto Level's state, the meters) stays. No allocation.
    pub fn reset_fresh(&mut self, oversampling: usize) {
        self.os = if oversampling == 2 { 2 } else { 4 };
        let set = self.set_index();
        self.sets[set].reset_fresh();
        for chain in [&mut self.cab_t, &mut self.cab_m, &mut self.cab_s, &mut self.cab_p, &mut self.cab_v, &mut self.cab_o] {
            for b in chain.iter_mut() {
                b.clear();
            }
        }
        self.bypass_delay.iter_mut().for_each(|x| *x = 0.0);
        self.bypass_pos = 0;
        self.controls_dirty = true;
    }

    fn set_index(&self) -> usize {
        usize::from(self.os != 2)
    }

    pub fn get_oversampling(&self) -> usize {
        self.os
    }

    /// Fixed: the oversampling filters' delay, the same at 2x and 4x.
    pub fn latency_samples(&self) -> usize {
        self.sets[self.set_index()].latency()
    }

    pub fn set_parameter(&mut self, index: usize, v: f32) {
        let Some(slot) = self.values.get_mut(index) else { return };
        *slot = v;
        // a setting that changes the amp's gain makes Auto Level re-level quickly
        let relevel = |s: &mut Self| {
            for a in &mut s.auto_state {
                a.primed_secs = 0.0;
            }
        };
        match index {
            P_AMP => {
                self.amp = usize::try_from(lround(v)).unwrap_or(0);
                relevel(self);
            }
            P_BYPASS => self.bypass = v > 0.5,
            P_QUALITY => {} // the processor re-prepares for it
            P_OUTPUT | P_JTM_OUTPUT | P_DLX_OUTPUT | P_SUP_OUTPUT | P_PRN_OUTPUT | P_AC_OUTPUT | P_AUTO_LEVEL | P_JTM_AUTO_LEVEL
            | P_DLX_AUTO_LEVEL | P_SUP_AUTO_LEVEL | P_PRN_AUTO_LEVEL | P_AC_AUTO_LEVEL | P_OR_OUTPUT | P_OR_AUTO_LEVEL => {} // read per block
            P_INPUT | P_CAB | P_JTM_INPUT | P_JTM_CAB | P_DLX_INPUT | P_DLX_CAB | P_SUP_INPUT | P_SUP_CAB | P_PRN_INPUT | P_PRN_CAB | P_AC_INPUT
            | P_AC_CAB | P_OR_INPUT | P_OR_CAB => relevel(self),
            _ => {
                self.controls_dirty = true;
                relevel(self);
            }
        }
    }

    /// `relevel()` for every amp (the wrapper's forced push after a prepare sets every parameter,
    /// and most of them re-level).
    pub fn relevel(&mut self) {
        for a in &mut self.auto_state {
            a.primed_secs = 0.0;
        }
    }

    fn value(&self, i: Option<usize>) -> f32 {
        i.and_then(|i| self.values.get(i).copied()).unwrap_or(0.0)
    }

    // ------------------------------------------------------------------------------ audio
    /// Mono in (the channels averaged), the amp, the built-in cabinet if chosen, and the result on
    /// every channel. With Cab = IR the wrapper convolves next; then `finish`.
    /// Runs frames `at..at + n` of `ch` (the wrapper chunks a host block to `prepare`'s size).
    pub fn process(&mut self, ch: &mut [Vec<f32>], num_ch: usize, at: usize, n: usize) {
        let num_ch = num_ch.min(ch.len());
        let n = ch.iter().take(num_ch).map(Vec::len).min().unwrap_or(0).saturating_sub(at).min(n);
        if num_ch == 0 || n == 0 {
            return;
        }
        if self.mono.len() < n {
            // (the C++ grows it here; this port is never handed more than `prepare`'s block)
            return;
        }

        for i in 0..n {
            let mut s = 0.0f32;
            for c in ch.iter().take(num_ch) {
                s += c[at + i];
            }
            self.mono[i] = s / num_ch as f32;
        }

        if self.bypass {
            // the dry signal, delayed by the latency the host is compensating for
            let len = self.bypass_delay.len().max(1);
            for i in 0..n {
                let x = self.mono[i];
                let y = self.bypass_delay.get(self.bypass_pos).copied().unwrap_or(0.0);
                if let Some(slot) = self.bypass_delay.get_mut(self.bypass_pos) {
                    *slot = x;
                }
                self.bypass_pos = (self.bypass_pos + 1) % len;
                for c in ch.iter_mut().take(num_ch) {
                    c[at + i] = y;
                }
            }
            return;
        }

        if self.controls_dirty {
            self.apply_controls();
        }

        let amp = self.amp;
        let in_gain = 10.0f32.powf(knob_to_trim_db(self.value(param(amp, Role::Input))) / 20.0);
        let cab = lround(self.value(param(amp, Role::Cab)));
        let info = amp_info(amp);
        let v_full = (2.0 * info.rated_watts * info.load).sqrt();
        let out_scale = 0.16 / v_full; // flat out ~ -16 dBFS into a resistor
        let in_scale = INPUT_VOLTS_AT_FULL_SCALE * f64::from(in_gain);
        // IR chosen but none loaded: the built-in one, rather than the raw speaker voltage
        let cab_now = if cab == CAB_IMPULSE && !self.impulse_ready { CAB_BUILT_IN } else { cab };
        let built_in = cab_now == CAB_BUILT_IN;

        let set = self.set_index();
        let mut in_pk = 0.0f32;
        let (mut spk_sq, mut di_sq) = (0.0f64, 0.0f64);
        let g = f64::from(in_gain);
        for i in 0..n {
            let m = f64::from(self.mono[i]);
            // the guitar's level as Auto Level sees it: after the Input trim
            di_sq += m * m * g * g;
            let x = m * in_scale;
            in_pk = in_pk.max(((x / INPUT_VOLTS_AT_FULL_SCALE) as f32).abs());
            let v = self.sets[set].process(amp, x);
            let vl = self.sets[set].power(amp).load_volts_now();
            spk_sq += vl * vl;
            let mut y = v * out_scale;
            if built_in {
                y = self.cab_run(y);
            }
            for c in ch.iter_mut().take(num_ch) {
                c[at + i] = y as f32;
            }
        }
        self.block_di_mean_square = di_sq / n as f64;

        let power = self.sets[set].power(amp);
        self.meters.in_peak = in_pk;
        self.meters.watts = (spk_sq / n as f64 / info.load) as f32;
        self.meters.b_plus = power.supply_now() as f32;
        self.meters.b_plus_idle = info.power.b_plus_idle as f32;
    }

    /// The last step, after the cabinet: Auto Level, the Output trim, the output meter.
    pub fn finish(&mut self, ch: &mut [Vec<f32>], num_ch: usize, at: usize, n: usize) {
        let num_ch = num_ch.min(ch.len());
        let n = ch.iter().take(num_ch).map(Vec::len).min().unwrap_or(0).saturating_sub(at).min(n);
        if num_ch == 0 || n == 0 {
            return;
        }
        if self.bypass {
            return;
        }

        let amp = self.amp;
        let sr = self.sr;
        let on = self.value(param(amp, Role::AutoLevel)) > 0.5;
        let trim = 10.0f32.powf(knob_to_trim_db(self.value(param(amp, Role::Output))) / 20.0);
        let block_di = self.block_di_mean_square;
        let al = &mut self.auto_state[amp.min(NUM_AMPS - 1)];

        let (mut out_sq, mut out_pk_in) = (0.0f64, 0.0f64);
        for &s in ch[0].iter().skip(at).take(n) {
            out_sq += f64::from(s) * f64::from(s);
            out_pk_in = cmax(out_pk_in, f64::from(s.abs()));
        }
        let out_ms = out_sq / n as f64;
        // the recent peak, held and let go over ~2 s
        al.peak = cmax(out_pk_in, al.peak * (-(n as f64 / sr) / 2.0).exp());

        if on && block_di > 1.0e-7 {
            // -70 dBFS (after Input): the guitar is sounding
            let block_secs = n as f64 / sr;
            // quick to settle after a change, then slow (a second)
            let tau = if al.primed_secs < 0.5 { 0.08 } else { 1.0 };
            let k = 1.0 - (-block_secs / tau).exp();
            al.di += (block_di - al.di) * k;
            al.out += (out_ms - al.out) * k;
            al.primed_secs += block_secs;
            if al.out > 1.0e-14 {
                al.target = super::amp_tube::cclamp(2.0 * (al.di / al.out).sqrt(), 0.01, 100.0); // +6 dB over the DI
            }
            // ...but never so much that the recent peaks go over -3 dBFS
            if al.peak > 1.0e-9 {
                al.target = super::amp_tube::cmin(al.target, 0.708 / al.peak);
            }
        }
        let want = if on { al.target } else { 1.0 };

        // glide to it (about 50 ms), and a safety limiter with Auto Level on
        let glide = 1.0 - (-1.0 / (0.05 * sr)).exp();
        let ceiling = f64::from(0.891f32);
        let release = 1.0 - (-1.0 / (0.15 * sr)).exp();
        let mut out_pk = 0.0f32;
        for i in 0..n {
            al.gain += (want - al.gain) * glide;
            let mut g = al.gain;
            if on {
                let level = f64::from(ch[0][at + i].abs()) * g;
                let need = if level > ceiling { ceiling / level } else { 1.0 };
                al.limit = if need < al.limit { need } else { al.limit + (1.0 - al.limit) * release };
                g *= al.limit;
            }
            let gf = g as f32 * trim;
            for c in ch.iter_mut().take(num_ch) {
                c[at + i] *= gf;
            }
            out_pk = out_pk.max(ch[0][at + i].abs());
        }
        self.meters.out_peak = out_pk;
        self.meters.auto_gain_db = (20.0 * cmax(1.0e-6, al.gain).log10()) as f32;
    }

    /// Whether the wrapper has an impulse response to convolve with (set per block).
    pub fn set_impulse_ready(&mut self, b: bool) {
        self.impulse_ready = b;
    }

    /// The cabinet actually in use: IR only when one is loaded.
    pub fn effective_cab(&self) -> i32 {
        let cab = lround(self.value(param(self.amp, Role::Cab)));
        if cab == CAB_IMPULSE && !self.impulse_ready { CAB_BUILT_IN } else { cab }
    }

    pub fn current_amp(&self) -> usize {
        self.amp
    }

    pub fn is_bypassed(&self) -> bool {
        self.bypass
    }

    pub fn auto_level(&self, amp: usize) -> Option<&AutoLevelState> {
        self.auto_state.get(amp)
    }

    /// (Rust) The power tubes' cathode volts of the amp in use (the AC30's self-bias).
    pub fn cathode_now(&self) -> f64 {
        self.sets[self.set_index()].power(self.amp).cathode_now()
    }

    /// The amps at the factor in use (tests).
    pub fn amps(&self) -> &Amps {
        &self.sets[self.set_index()]
    }

    fn apply_controls(&mut self) {
        self.controls_dirty = false;
        let values = self.values;
        let set = self.set_index();
        self.sets[set].set_controls(&values);
    }

    // ---- the built-in cabinet: a few filters after each amp's speakers
    fn cab_prepare(&mut self) {
        let fs = self.sr;
        self.cab_t = [
            hp(75.0, 0.7, fs),
            peak(110.0, 1.2, 2.5, fs),
            peak(500.0, 1.1, -3.0, fs),
            peak(2600.0, 1.6, 3.0, fs),
            lp(4800.0, 0.75, fs),
            lp(5200.0, 1.1, fs),
            lp(6500.0, 0.6, fs),
            lp(8000.0, 0.55, fs),
        ];
        self.cab_m = [
            hp(85.0, 0.7, fs),
            peak(120.0, 1.1, 2.0, fs),
            peak(1100.0, 0.9, 3.0, fs),
            peak(2600.0, 2.0, 3.5, fs),
            lp(4200.0, 0.85, fs),
            lp(4600.0, 1.1, fs),
            lp(6000.0, 0.6, fs),
            lp(7500.0, 0.55, fs),
        ];
        self.cab_s = [
            hp(90.0, 0.7, fs),
            peak(140.0, 1.2, 2.0, fs),
            peak(500.0, 1.1, -2.5, fs),
            peak(2800.0, 1.6, 3.5, fs),
            lp(5200.0, 0.75, fs),
            lp(5800.0, 1.1, fs),
            lp(7000.0, 0.6, fs),
            lp(9000.0, 0.55, fs),
        ];
        self.cab_p = [
            hp(100.0, 0.7, fs),
            peak(160.0, 1.2, 2.5, fs),
            peak(450.0, 1.1, -2.0, fs),
            peak(2500.0, 1.5, 3.0, fs),
            lp(5000.0, 0.75, fs),
            lp(5500.0, 1.1, fs),
            lp(7000.0, 0.6, fs),
            lp(8500.0, 0.55, fs),
        ];
        self.cab_v = [
            hp(80.0, 0.7, fs),
            peak(120.0, 1.2, 2.0, fs),
            peak(700.0, 1.0, -1.5, fs),
            peak(3000.0, 1.4, 4.5, fs),
            lp(5500.0, 0.8, fs),
            lp(6000.0, 1.1, fs),
            lp(7500.0, 0.6, fs),
            lp(9500.0, 0.55, fs),
        ];
        self.cab_o = [
            hp(70.0, 0.75, fs),
            peak(110.0, 1.0, 3.0, fs),
            peak(550.0, 1.0, -1.5, fs),
            peak(2300.0, 1.3, 3.5, fs),
            lp(4700.0, 0.8, fs),
            lp(5200.0, 1.1, fs),
            lp(6500.0, 0.6, fs),
            lp(8000.0, 0.55, fs),
        ];
    }

    #[inline]
    fn cab_run(&mut self, mut x: f64) -> f64 {
        // the Deluxe's open-back 1x12 takes the Twin's Fender voicing
        let chain = match self.amp {
            AMP_MARSHALL => &mut self.cab_m,
            AMP_SUPER => &mut self.cab_s,
            AMP_PRINCETON => &mut self.cab_p,
            AMP_AC30 => &mut self.cab_v,
            AMP_ORANGE => &mut self.cab_o,
            _ => &mut self.cab_t,
        };
        for b in chain.iter_mut() {
            x = b.run(x);
        }
        x
    }
}

fn hp(hz: f64, q: f64, fs: f64) -> Biquad {
    let w = 2.0 * 3.14159265358979 * hz / fs;
    let (c, a) = (w.cos(), w.sin() / (2.0 * q));
    let n = 1.0 + a;
    Biquad { b0: (1.0 + c) / 2.0 / n, b1: -(1.0 + c) / n, b2: (1.0 + c) / 2.0 / n, a1: -2.0 * c / n, a2: (1.0 - a) / n, z1: 0.0, z2: 0.0 }
}

fn lp(hz: f64, q: f64, fs: f64) -> Biquad {
    let w = 2.0 * 3.14159265358979 * hz / fs;
    let (c, a) = (w.cos(), w.sin() / (2.0 * q));
    let n = 1.0 + a;
    Biquad { b0: (1.0 - c) / 2.0 / n, b1: (1.0 - c) / n, b2: (1.0 - c) / 2.0 / n, a1: -2.0 * c / n, a2: (1.0 - a) / n, z1: 0.0, z2: 0.0 }
}

fn peak(hz: f64, q: f64, db: f64, fs: f64) -> Biquad {
    let a_ = 10f64.powf(db / 40.0);
    let w = 2.0 * 3.14159265358979 * hz / fs;
    let (c, a) = (w.cos(), w.sin() / (2.0 * q));
    let n = 1.0 + a / a_;
    Biquad { b0: (1.0 + a * a_) / n, b1: -2.0 * c / n, b2: (1.0 - a * a_) / n, a1: -2.0 * c / n, a2: (1.0 - a / a_) / n, z1: 0.0, z2: 0.0 }
}
