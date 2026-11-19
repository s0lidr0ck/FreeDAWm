//! Subtractive Synth: 16-voice polyphonic, two PolyBLEP oscillators with a sub oscillator (a
//! square an octave below oscillator 1) and noise, a resonant state-variable filter (TPT form;
//! 12 or 24 dB low-pass, high-pass, band-pass or notch) with its own ADSR and key tracking, amp
//! ADSR, an LFO (sine, triangle, square or saw) to pitch, cutoff and level, poly, mono or legato
//! play, optional glide, unison (up to seven detuned copies of both oscillators, spread across the
//! stereo field) and hard sync (oscillator 2 restarted by oscillator 1, the restart band-limited
//! by a PolyBLEP step). The parameters past Mod Wheel Vibrato came later and default to the sound
//! the synth had before them (12 dB low-pass, LFO amounts, sub, noise and key tracking at 0, poly,
//! unison and sync off), so sessions saved before sound as they did, sample for sample.

use super::synth_mod::{self as sm, ModMatrix, VoiceMod};
use crate::osc::{Osc, Waveform};
use crate::params::{Params, choice, lin, log, param_plumbing};
use crate::util::{Smoother, frames_in, ms_to_samples, note_hz, sane_channels, sane_sr, sort_events};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

pub mod presets;
#[cfg(test)]
mod reference;
pub mod suite;

pub(crate) const MAX_EVENTS: usize = 512;

#[derive(Debug, Clone, Copy)]
pub(crate) enum EventKind {
    On(u8, u8),
    Off(u8),
    AllOff,
    Sustain(bool),
    /// Pitch bend, -1..1 (the instrument's Bend Range is a full throw).
    Bend(f32),
    /// The mod wheel (CC 1), 0..1.
    Mod(f32),
    ResetControllers,
}

/// A pitch-bend message's position, -1..1 (the centre 0).
pub(crate) fn bend_of(d1: u8, d2: u8) -> f32 {
    let v = (i32::from(d2 & 0x7F) << 7 | i32::from(d1 & 0x7F)) - 8192;
    (v as f32 / 8192.0).clamp(-1.0, 1.0)
}

/// The controller messages the synths act on (sustain, all notes off, reset controllers, mod
/// wheel, pitch bend) as events; anything else `None`.
pub(crate) fn event_of(msg: [u8; 3]) -> Option<EventKind> {
    let [status, d1, d2] = msg;
    Some(match (status & 0xF0, d1) {
        (0xB0, 64) => EventKind::Sustain(d2 >= 64),
        (0xB0, 120 | 123) => EventKind::AllOff,
        (0xB0, 121) => EventKind::ResetControllers,
        (0xB0, 1) => EventKind::Mod(f32::from(d2 & 0x7F) / 127.0),
        (0xE0, _) => EventKind::Bend(bend_of(d1, d2)),
        _ => return None,
    })
}

/// The voice a new note takes, from each voice's (sounding, note, key held, amp level): one
/// already playing this note, else a free one, else the quietest released one, else the
/// quietest held one (the synths' voice stealing).
pub(crate) fn pick_voice(voices: impl Iterator<Item = (bool, u8, bool, f32)> + Clone, note: u8) -> usize {
    voices
        .clone()
        .position(|(active, n, _, _)| active && n == note)
        .or_else(|| voices.clone().position(|(active, _, _, _)| !active))
        .or_else(|| {
            voices.enumerate().min_by(|(_, a), (_, b)| (a.2, a.3).partial_cmp(&(b.2, b.3)).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i)
        })
        .unwrap_or(0)
}

/// An LFO's value (-1..1) at `phase` (0..1) for wave `wave` ([`LFO_WAVES`]: sine, triangle,
/// square, saw).
#[inline]
pub(crate) fn lfo_shape(wave: usize, phase: f32) -> f32 {
    match wave {
        1 => 1.0 - 4.0 * (phase - 0.5).abs(),
        2 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        3 => 2.0 * phase - 1.0,
        _ => (std::f32::consts::TAU * phase).sin(),
    }
}

/// Per unison copy: its pitch ratio and its left and right gains, `unison` copies detuned across
/// `detune` cents and panned across `spread` (0..1) at constant power, the stack's level kept by
/// 1/√n (one copy: the centre, at full level on both sides).
#[inline]
pub(crate) fn unison_copies(unison: usize, detune: f32, spread: f32) -> [(f32, f32, f32); MAX_UNISON] {
    let norm = 1.0 / (unison as f32).sqrt();
    let mut copies = [(1.0f32, 1.0f32, 1.0f32); MAX_UNISON];
    if unison > 1 {
        for (i, c) in copies.iter_mut().take(unison).enumerate() {
            let x = 2.0 * i as f32 / (unison - 1) as f32 - 1.0;
            let angle = (1.0 + spread * x) * std::f32::consts::FRAC_PI_4;
            let g = std::f32::consts::SQRT_2 * norm;
            *c = (2f32.powf(detune * x / 1200.0), g * angle.cos(), g * angle.sin());
        }
    }
    copies
}

/// The vibrato the mod wheel brings in (the synth's; the sampler uses its own LFO), Hz.
pub(crate) const VIBRATO_HZ: f32 = 5.5;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Event {
    pub(crate) offset: usize,
    pub(crate) kind: EventKind,
}

/// Fixed-capacity event queue (never reallocates on the audio thread). Full, an event is dropped
/// and counted: `push` runs on the audio thread (live MIDI can bring more than fits in a block),
/// so it never logs.
#[derive(Debug)]
pub(crate) struct EventQueue {
    ev: Vec<Event>,
    /// Events dropped because the queue was full, since it was made.
    dropped: u64,
}

impl EventQueue {
    pub(crate) fn new() -> Self {
        Self { ev: Vec::with_capacity(MAX_EVENTS), dropped: 0 }
    }

    pub(crate) fn push(&mut self, offset: usize, kind: EventKind) {
        if self.ev.len() < MAX_EVENTS {
            self.ev.push(Event { offset, kind });
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// Events dropped for want of room (see the type's docs).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    pub(crate) fn sorted(&mut self) -> &[Event] {
        sort_events(&mut self.ev, |e| e.offset);
        &self.ev
    }

    pub(crate) fn clear(&mut self) {
        self.ev.clear();
    }
}

const WAVES: &[&str] = &["Saw", "Square", "Sine", "Triangle"];

fn wave(i: usize) -> Waveform {
    match i {
        1 => Waveform::Square,
        2 => Waveform::Sine,
        3 => Waveform::Triangle,
        _ => Waveform::Saw,
    }
}

pub(crate) const FILTERS: &[&str] = &["Low-pass 12 dB", "Low-pass 24 dB", "High-pass", "Band-pass", "Notch"];
pub(crate) const LFO_WAVES: &[&str] = &["Sine", "Triangle", "Square", "Saw"];
const MODES: &[&str] = &["Poly", "Mono", "Legato"];
pub(crate) const UNISON: &[&str] = &["Off", "2", "3", "4", "5", "6", "7"];
pub(crate) const OFF_ON: &[&str] = &["Off", "On"];

static SYNTH_PARAMS: [ParamInfo; 43] = [
    choice("osc1_wave", "Osc 1", WAVES, 0),
    choice("osc2_wave", "Osc 2", WAVES, 0),
    lin("osc2_semitones", "Osc 2 Pitch", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("osc2_detune", "Osc 2 Detune", -100.0, 100.0, 7.0, Unit::Cents),
    lin("osc_mix", "Osc Mix", 0.0, 100.0, 50.0, Unit::Percent),
    log("cutoff", "Cutoff", 20.0, 20000.0, 2000.0, Unit::Hz),
    lin("resonance", "Resonance", 0.0, 100.0, 20.0, Unit::Percent),
    lin("filter_env_amount", "Filter Env", -100.0, 100.0, 30.0, Unit::Percent),
    log("amp_attack", "Amp Attack", 0.5, 5000.0, 5.0, Unit::Ms),
    log("amp_decay", "Amp Decay", 1.0, 5000.0, 200.0, Unit::Ms),
    lin("amp_sustain", "Amp Sustain", 0.0, 100.0, 70.0, Unit::Percent),
    log("amp_release", "Amp Release", 1.0, 10000.0, 300.0, Unit::Ms),
    log("filter_attack", "Filter Attack", 0.5, 5000.0, 5.0, Unit::Ms),
    log("filter_decay", "Filter Decay", 1.0, 5000.0, 300.0, Unit::Ms),
    lin("filter_sustain", "Filter Sustain", 0.0, 100.0, 30.0, Unit::Percent),
    log("filter_release", "Filter Release", 1.0, 10000.0, 300.0, Unit::Ms),
    lin("glide", "Glide", 0.0, 1000.0, 0.0, Unit::Ms),
    lin("velocity", "Velocity Sens", 0.0, 100.0, 70.0, Unit::Percent),
    lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
    // The pitch bend's throw each way, and the vibrato (cents each way) the mod wheel brings in.
    lin("bend_range", "Bend Range", 0.0, 24.0, 2.0, Unit::Semitones),
    lin("mod_wheel", "Mod Wheel Vibrato", 0.0, 100.0, 0.0, Unit::Cents),
    choice("filter_type", "Filter Type", FILTERS, 0),
    lin("key_track", "Key Tracking", 0.0, 100.0, 0.0, Unit::Percent),
    lin("sub_level", "Sub Osc", 0.0, 100.0, 0.0, Unit::Percent),
    lin("noise_level", "Noise", 0.0, 100.0, 0.0, Unit::Percent),
    choice("lfo_wave", "LFO Wave", LFO_WAVES, 0),
    log("lfo_rate", "LFO Rate", 0.05, 30.0, 5.0, Unit::Hz),
    lin("lfo_pitch", "LFO to Pitch", 0.0, 1200.0, 0.0, Unit::Cents),
    lin("lfo_cutoff", "LFO to Cutoff", 0.0, 100.0, 0.0, Unit::Percent),
    lin("lfo_amp", "LFO to Level", 0.0, 100.0, 0.0, Unit::Percent),
    choice("voice_mode", "Voice Mode", MODES, 0),
    // Unison: copies of both oscillators per note, detuned (cents across the stack) and spread
    // (the outer copies to the sides); hard sync restarts oscillator 2 with oscillator 1.
    choice("unison", "Unison", UNISON, 0),
    lin("unison_detune", "Unison Detune", 0.0, 100.0, 20.0, Unit::Cents),
    lin("unison_spread", "Unison Spread", 0.0, 100.0, 50.0, Unit::Percent),
    choice("osc_sync", "Osc 2 Sync", OFF_ON, 0),
    // The macros: each a knob the modulation matrix's macro targets follow (nothing at 0).
    lin("macro1", "Macro 1", 0.0, 100.0, 0.0, Unit::Percent),
    lin("macro2", "Macro 2", 0.0, 100.0, 0.0, Unit::Percent),
    lin("macro3", "Macro 3", 0.0, 100.0, 0.0, Unit::Percent),
    lin("macro4", "Macro 4", 0.0, 100.0, 0.0, Unit::Percent),
    // Cross-modulation at audio rate: oscillator 2 or the sub frequency-modulates oscillator 1
    // (linear FM, up to 90 % of its frequency each way) and sweeps the cutoff (up to four
    // octaves each way). Both 0: as before.
    choice("xmod_source", "X-Mod Source", XMOD_SOURCES, 0),
    lin("xmod_pitch", "X-Mod to Pitch", 0.0, 100.0, 0.0, Unit::Percent),
    lin("xmod_cutoff", "X-Mod to Cutoff", 0.0, 100.0, 0.0, Unit::Percent),
    // Drive: the oscillators saturated on their way into the filter ([`drive`]); 0 = as before.
    lin("drive", "Drive", 0.0, 100.0, 0.0, Unit::Percent),
];
const DRIVE: usize = 42;

/// The drive stage: `x` through a tanh curve pushed `1 + 9 d` times harder at drive `d` (0..1),
/// scaled so a full-scale input comes out at full scale: quiet parts are lifted and peaks
/// rounded off, as a driven filter input does. `d` 0 leaves `x` as it is (the caller skips it).
#[inline]
pub(crate) fn drive(x: f32, d: f32) -> f32 {
    let g = 1.0 + 9.0 * d.clamp(0.0, 1.0);
    let y = (g * x).tanh() / g.tanh();
    if y.is_finite() { y } else { 0.0 }
}
const XMOD_SOURCE: usize = 39;
const XMOD_PITCH: usize = 40;
const XMOD_CUTOFF: usize = 41;
/// What cross-modulates oscillator 1 and the cutoff.
pub(crate) const XMOD_SOURCES: &[&str] = &["Osc 2", "Sub"];
/// X-Mod to Pitch at 100 %: the frequency swings by this share of itself each way (linear FM,
/// so the pitch stays where it is); X-Mod to Cutoff: octaves each way.
const XMOD_PITCH_DEPTH: f32 = 0.9;
const XMOD_CUTOFF_OCT: f32 = 4.0;
/// The first macro's parameter index.
const MACRO1: usize = 35;

/// What the modulation matrix can move (parameter ids, and the virtual `pitch`), by index.
pub(crate) const MOD_DESTS: [&str; 19] = [
    sm::PITCH,
    "osc2_semitones",
    "osc2_detune",
    "osc_mix",
    "cutoff",
    "resonance",
    "filter_env_amount",
    "key_track",
    "sub_level",
    "noise_level",
    sm::SHARED_DEST,
    "lfo_pitch",
    "lfo_cutoff",
    "lfo_amp",
    "unison_detune",
    "unison_spread",
    "level",
    "xmod_pitch",
    "xmod_cutoff",
];
const D_PITCH: usize = 0;
const D_SEMIS2: usize = 1;
const D_DETUNE2: usize = 2;
const D_MIX: usize = 3;
const D_CUTOFF: usize = 4;
const D_RES: usize = 5;
const D_ENV: usize = 6;
const D_KEY: usize = 7;
const D_SUB: usize = 8;
const D_NOISE: usize = 9;
const D_LFO_RATE: usize = 10;
const D_LFO_PITCH: usize = 11;
const D_LFO_CUTOFF: usize = 12;
const D_LFO_AMP: usize = 13;
const D_UNI_DETUNE: usize = 14;
const D_UNI_SPREAD: usize = 15;
const D_LEVEL: usize = 16;
const D_XPITCH: usize = 17;
const D_XCUT: usize = 18;
const UNISON_P: usize = 31;
const UNISON_DETUNE: usize = 32;
const UNISON_SPREAD: usize = 33;
const OSC_SYNC: usize = 34;
/// Most copies a unison note plays.
pub(crate) const MAX_UNISON: usize = 7;
const FILTER_TYPE: usize = 21;
const KEY_TRACK: usize = 22;
const SUB_LEVEL: usize = 23;
const NOISE_LEVEL: usize = 24;
const LFO_WAVE: usize = 25;
const LFO_RATE: usize = 26;
const LFO_PITCH: usize = 27;
const LFO_CUTOFF: usize = 28;
const LFO_AMP: usize = 29;
const VOICE_MODE: usize = 30;
const BEND_RANGE: usize = 19;
const MOD_WHEEL: usize = 20;

pub static SYNTH_INFO: PluginInfo = PluginInfo {
    id: "subtractive_synth",
    name: "Burley",
    short_name: "Burley",
    category: Category::Instrument,
    params: &SYNTH_PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

const VOICES: usize = 16;
pub(crate) const SILENT: f32 = 1.0e-5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Linear-attack, exponential decay/release ADSR. Times are "to -60 dB" for decay/release.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Adsr {
    pub(crate) stage: Stage,
    pub(crate) level: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AdsrRates {
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
}

impl AdsrRates {
    pub(crate) fn new(a_ms: f32, d_ms: f32, s: f32, r_ms: f32, sr: f32) -> Self {
        let exp60 = |ms: f32| (0.001f32.ln() / ms_to_samples(ms, sr).max(1.0)).exp();
        Self { attack: 1.0 / ms_to_samples(a_ms, sr).max(1.0), decay: exp60(d_ms), sustain: s.clamp(0.0, 1.0), release: exp60(r_ms) }
    }
}

impl Adsr {
    pub(crate) const fn new() -> Self {
        Self { stage: Stage::Idle, level: 0.0 }
    }

    pub(crate) fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub(crate) fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    #[inline]
    pub(crate) fn next(&mut self, r: &AdsrRates) -> f32 {
        match self.stage {
            Stage::Idle => self.level = 0.0,
            Stage::Attack => {
                self.level += r.attack;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = r.sustain + (self.level - r.sustain) * r.decay;
                if (self.level - r.sustain).abs() < 1e-4 {
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = r.sustain,
            Stage::Release => {
                self.level *= r.release;
                if self.level < SILENT {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

/// Topology-preserving-transform state-variable filter (shared with the Wavetable Synth).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// One sample through the filter as `kind` ([`FILTERS`]: 0 and 1 low-pass, 2 high-pass, 3
    /// band-pass, 4 notch).
    #[inline]
    pub(crate) fn run(&mut self, x: f32, g: f32, k: f32, kind: usize) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = crate::util::flush(2.0 * v1 - self.ic1);
        self.ic2 = crate::util::flush(2.0 * v2 - self.ic2);
        let y = match kind {
            2 => x - k * v1 - v2,
            3 => v1,
            4 => x - k * v1,
            _ => v2,
        };
        if y.is_finite() {
            y
        } else {
            *self = Svf::default();
            0.0
        }
    }

    /// The low-pass the synth had before the filter types (kept to check `run` against it).
    #[inline]
    #[cfg(test)]
    fn lowpass(&mut self, x: f32, g: f32, k: f32) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = crate::util::flush(2.0 * v1 - self.ic1);
        self.ic2 = crate::util::flush(2.0 * v2 - self.ic2);
        if v2.is_finite() {
            v2
        } else {
            *self = Svf::default();
            0.0
        }
    }
}

/// The naive (not band-limited) value of `wave` at phase `p`: what a hard-sync restart jumps
/// between.
#[inline]
fn naive(wave: Waveform, p: f32) -> f32 {
    match wave {
        Waveform::Saw => 2.0 * p - 1.0,
        Waveform::Square => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        Waveform::Triangle => 1.0 - 4.0 * ((p + 0.25).fract() - 0.5).abs(),
        _ => (std::f32::consts::TAU * p).sin(),
    }
}

/// The step `Osc` corrects for itself just after phase 0 (its own wrap): a saw falls by 2, a
/// square rises by 2; the others have none there.
#[inline]
fn own_step(wave: Waveform) -> f32 {
    match wave {
        Waveform::Saw => -2.0,
        Waveform::Square => 2.0,
        _ => 0.0,
    }
}

/// One copy of the two oscillators: oscillator 2 optionally hard-synced to oscillator 1.
#[derive(Debug, Clone, Copy)]
struct Pair {
    osc1: Osc,
    osc2: Osc,
    /// The second half of the last restart's PolyBLEP, added to oscillator 2's next sample.
    fix: f32,
    /// Oscillator 2's last sample (what cross-modulates oscillator 1 next).
    last2: f32,
}

impl Pair {
    fn new(seed: u32) -> Pair {
        Pair { osc1: Osc::new(seed), osc2: Osc::new(seed.wrapping_mul(7919)), fix: 0.0, last2: 0.0 }
    }

    /// One sample of each oscillator; with `sync`, oscillator 2 starts again where oscillator 1
    /// wraps, the jump spread over the two samples around it (a PolyBLEP step).
    #[inline]
    fn next(&mut self, w1: Waveform, w2: Waveform, inc1: f32, inc2: f32, sync: bool) -> (f32, f32) {
        if !sync {
            let (s1, s2) = (self.osc1.next(w1, inc1), self.osc2.next(w2, inc2));
            self.last2 = s2;
            return (s1, s2);
        }
        let dt1 = if inc1.is_finite() { inc1.clamp(0.0, 0.49) } else { 0.0 };
        let dt2 = if inc2.is_finite() { inc2.clamp(0.0, 0.49) } else { 0.0 };
        let (p1, p2) = (self.osc1.phase, self.osc2.phase);
        let s1 = self.osc1.next(w1, inc1);
        let mut s2 = self.osc2.next(w2, inc2) + std::mem::take(&mut self.fix);
        if dt1 > 0.0 && p1 + dt1 >= 1.0 {
            // Oscillator 1 wrapped `d` of a sample before the next sample: oscillator 2 restarts
            // there, from where it had got to.
            let d = ((p1 + dt1 - 1.0) / dt1).clamp(0.0, 1.0);
            let at = (p2 + dt2 * (1.0 - d)).fract();
            let h = naive(w2, 0.0) - naive(w2, at);
            s2 += 0.5 * h * d * d;
            self.osc2.phase = (dt2 * d).min(0.999_999);
            // The next sample's `Osc` already adds the after-step of its own wrap; the rest of
            // this jump's is added then.
            let e = 1.0 - d;
            self.fix = if dt2 * d < dt2 { -0.5 * (h - own_step(w2)) * e * e } else { -0.5 * h * e * e };
            if !self.fix.is_finite() {
                self.fix = 0.0;
            }
        }
        self.last2 = s2;
        (s1, s2)
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    note: u8,
    vel: f32,
    freq: f32,
    target: f32,
    osc1: Osc,
    osc2: Osc,
    amp: Adsr,
    filt: Adsr,
    svf: Svf,
    /// The second stage of the 24 dB low-pass.
    svf2: Svf,
    sub: Osc,
    /// The sub's last sample (when it cross-modulates).
    last_sub: f32,
    noise: u32,
    /// Unison and hard sync: the copies of the oscillators (the first is `osc1`/`osc2`'s place
    /// when either is on), and the right side's filters.
    pairs: [Pair; MAX_UNISON],
    svf_r: Svf,
    svf2_r: Svf,
    age: u64,
    held: bool,
    /// Released while the sustain pedal was down.
    sustained: bool,
    /// The note's velocity, 0..1 (the matrix's velocity source).
    vel_raw: f32,
    /// The matrix's values for this voice, and its unison copies when the matrix moves their
    /// detune or spread.
    m: VoiceMod,
    copies: [(f32, f32, f32); MAX_UNISON],
}

impl Voice {
    fn new(seed: u32) -> Self {
        Self {
            note: 0,
            vel: 0.0,
            freq: 440.0,
            target: 440.0,
            osc1: Osc::new(seed),
            osc2: Osc::new(seed.wrapping_mul(7919)),
            amp: Adsr::new(),
            filt: Adsr::new(),
            svf: Svf::default(),
            svf2: Svf::default(),
            sub: Osc::new(seed.wrapping_mul(104_729)),
            noise: seed.wrapping_mul(2_654_435_761).max(1),
            last_sub: 0.0,
            pairs: [Pair::new(seed); MAX_UNISON],
            svf_r: Svf::default(),
            svf2_r: Svf::default(),
            age: 0,
            held: false,
            sustained: false,
            vel_raw: 0.0,
            m: VoiceMod::new(),
            copies: [(1.0, 1.0, 1.0); MAX_UNISON],
        }
    }

    fn active(&self) -> bool {
        self.amp.stage != Stage::Idle
    }

    /// A fresh start for the unison copies and the right filters: each copy from its own phase
    /// (so the stack does not start as one wave), oscillator 2 at the phase the plain path uses.
    fn restart_pairs(&mut self) {
        for (i, p) in self.pairs.iter_mut().enumerate() {
            let spread = (i as f32 * 0.618_034).fract();
            p.osc1.reset();
            p.osc1.phase = spread;
            p.osc2.reset();
            p.osc2.phase = (0.37 + spread).fract();
            p.fix = 0.0;
            p.last2 = 0.0;
        }
        self.svf_r = Svf::default();
        self.svf2_r = Svf::default();
    }
}

/// Polyphonic subtractive synthesizer.
pub struct SubtractiveSynth {
    p: Params,
    sr: f32,
    ch: usize,
    voices: [Voice; VOICES],
    events: EventQueue,
    amp_rates: AdsrRates,
    filt_rates: AdsrRates,
    glide_coef: f32,
    last_freq: Option<f32>,
    counter: u64,
    level: Smoother,
    cutoff: Smoother,
    /// Oscillator 2's tuning in semitones (with its detune), the oscillator mix, the filter's
    /// damping (from the resonance) and the filter envelope's amount in octaves: smoothed, so
    /// moving them does not step.
    tune2: Smoother,
    mix: Smoother,
    damping: Smoother,
    env_oct: Smoother,
    sustain: bool,
    /// The bend's position (-1..1) and the mod wheel's (0..1).
    bend: f32,
    wheel: f32,
    /// The vibrato's phase (0..1).
    vib_phase: f32,
    /// The LFO's phase (0..1).
    lfo_phase: f32,
    /// Mono and Legato: the keys held, oldest first (the last one sounds).
    mono_held: [u8; 16],
    mono_n: usize,
    /// The modulation matrix, its destinations' descriptions, the knobs' values and the macros'
    /// offsets at the last evaluation, the samples to the next one, the shared LFO rate it gave,
    /// and the per-note random values' generator.
    mm: ModMatrix,
    infos: [&'static ParamInfo; sm::MAX_DESTS],
    bases: [f32; sm::MAX_DESTS],
    macro_off: [f32; sm::MAX_DESTS],
    tick_left: usize,
    lfo_rate_mod: f32,
    rng: u32,
}

/// The values one sample shares across voices, which the matrix may change per voice.
#[derive(Debug, Clone, Copy)]
struct Shared {
    /// The bend and vibrato in cents (before the LFO), the LFO now and its pitch depth.
    cents: f32,
    lfo: f32,
    lfo_pitch: f32,
    /// The pitch ratio from all of them.
    bend: f32,
    semis2: f32,
    detune2: f32,
    ratio2: f32,
    mix: f32,
    k: f32,
    env_oct: f32,
    cutoff: f32,
    lfo_oct: f32,
    lfo_amp: f32,
    key_track: f32,
    sub_level: f32,
    noise_level: f32,
    gain: f32,
    /// Cross-modulation depths (octaves at a full-scale source).
    xpitch: f32,
    xcut: f32,
}

impl Shared {
    /// `self` with what the matrix sets for the voice whose values are `m`.
    #[inline]
    fn modded(mut self, mm: &ModMatrix, m: &VoiceMod) -> Shared {
        let on = |d| mm.on(d);
        if on(D_PITCH) || on(D_LFO_PITCH) {
            let depth = if on(D_LFO_PITCH) { m.get(D_LFO_PITCH) } else { self.lfo_pitch };
            let cents = self.cents + depth * self.lfo;
            let ratio = if on(D_PITCH) { m.get(D_PITCH) } else { 1.0 };
            self.bend = 2f32.powf(cents / 1200.0) * ratio;
        }
        if on(D_SEMIS2) || on(D_DETUNE2) {
            let s = if on(D_SEMIS2) { m.get(D_SEMIS2) } else { self.semis2 };
            let d = if on(D_DETUNE2) { m.get(D_DETUNE2) } else { self.detune2 };
            self.ratio2 = 2f32.powf((s + d / 100.0) / 12.0);
        }
        let pick = |d, v: f32| if on(d) { m.get(d) } else { v };
        self.mix = pick(D_MIX, self.mix);
        self.k = pick(D_RES, self.k);
        self.env_oct = pick(D_ENV, self.env_oct);
        self.cutoff = pick(D_CUTOFF, self.cutoff);
        if on(D_LFO_CUTOFF) {
            self.lfo_oct = m.get(D_LFO_CUTOFF) * self.lfo;
        }
        if on(D_LFO_AMP) {
            self.lfo_amp = 1.0 - m.get(D_LFO_AMP) * 0.5 * (1.0 - self.lfo);
        }
        self.key_track = pick(D_KEY, self.key_track);
        self.sub_level = pick(D_SUB, self.sub_level);
        self.noise_level = pick(D_NOISE, self.noise_level);
        self.gain = pick(D_LEVEL, self.gain);
        self.xpitch = pick(D_XPITCH, self.xpitch);
        self.xcut = pick(D_XCUT, self.xcut);
        self
    }
}

impl Default for SubtractiveSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl SubtractiveSynth {
    pub fn new() -> Self {
        let mut voices = [Voice::new(1); VOICES];
        for (i, v) in voices.iter_mut().enumerate() {
            *v = Voice::new(0x1000 + i as u32 * 31);
        }
        let mut s = Self {
            p: Params::new(&SYNTH_PARAMS),
            sr: 48_000.0,
            ch: 0,
            voices,
            events: EventQueue::new(),
            amp_rates: AdsrRates::default(),
            filt_rates: AdsrRates::default(),
            glide_coef: 0.0,
            last_freq: None,
            counter: 0,
            level: Smoother::new(0.5),
            cutoff: Smoother::new(2000.0),
            tune2: Smoother::new(0.0),
            mix: Smoother::new(0.0),
            damping: Smoother::new(2.0),
            env_oct: Smoother::new(0.0),
            sustain: false,
            bend: 0.0,
            wheel: 0.0,
            vib_phase: 0.0,
            lfo_phase: 0.0,
            mono_held: [0; 16],
            mono_n: 0,
            mm: ModMatrix::new(),
            infos: sm::infos(SYNTH_INFO.id),
            bases: [0.0; sm::MAX_DESTS],
            macro_off: [0.0; sm::MAX_DESTS],
            tick_left: 0,
            lfo_rate_mod: 5.0,
            rng: sm::RANDOM_SEED,
        };
        s.update();
        s
    }

    /// Cross-modulation is on (its knobs, or the matrix moving them).
    #[inline]
    fn xmod_on(&self) -> bool {
        self.p.v(XMOD_PITCH) > 0.0 || self.p.v(XMOD_CUTOFF) > 0.0 || self.mm.on(D_XPITCH) || self.mm.on(D_XCUT)
    }

    /// The LFO's rate now: the knob's, or what the matrix makes of it.
    #[inline]
    fn lfo_rate(&self) -> f32 {
        if self.mm.on(D_LFO_RATE) { self.lfo_rate_mod } else { self.p.v(LFO_RATE) }
    }

    /// A note starts on voice `i`: its random value, and the matrix evaluated at once.
    fn mod_note_on(&mut self, i: usize) {
        let r = sm::next_random(&mut self.rng);
        if let Some(v) = self.voices.get_mut(i) {
            v.m.rnd = r;
            v.m.fresh = true;
        }
        self.tick_left = 0;
    }

    /// The matrix, once a sample while anything is patched: every [`sm::TICK`] samples each
    /// sounding voice aims at its new values; every sample each moves along its ramps.
    fn mod_step(&mut self) {
        if self.tick_left == 0 {
            self.tick_left = sm::TICK;
            self.mod_tick();
        }
        self.tick_left -= 1;
        let used = self.mm.used();
        for v in self.voices.iter_mut().filter(|v| v.active()) {
            v.m.advance(used);
        }
    }

    fn mod_tick(&mut self) {
        let p = &self.p;
        let macros = [p.v(MACRO1) / 100.0, p.v(MACRO1 + 1) / 100.0, p.v(MACRO1 + 2) / 100.0, p.v(MACRO1 + 3) / 100.0];
        self.mm.macro_offsets(macros, &mut self.macro_off);
        let b = &mut self.bases;
        b[D_PITCH] = 0.0;
        b[D_SEMIS2] = p.v(2);
        b[D_DETUNE2] = p.v(3);
        b[D_MIX] = self.mix.value() * 100.0;
        b[D_CUTOFF] = self.cutoff.value();
        b[D_RES] = (2.0 - self.damping.value()) / 1.95 * 100.0;
        b[D_ENV] = self.env_oct.value() / 6.0 * 100.0;
        b[D_KEY] = p.v(KEY_TRACK);
        b[D_SUB] = p.v(SUB_LEVEL);
        b[D_NOISE] = p.v(NOISE_LEVEL);
        b[D_LFO_RATE] = p.v(LFO_RATE);
        b[D_LFO_PITCH] = p.v(LFO_PITCH);
        b[D_LFO_CUTOFF] = p.v(LFO_CUTOFF);
        b[D_LFO_AMP] = p.v(LFO_AMP);
        b[D_UNI_DETUNE] = p.v(UNISON_DETUNE);
        b[D_UNI_SPREAD] = p.v(UNISON_SPREAD);
        b[D_LEVEL] = p.v(18);
        b[D_XPITCH] = p.v(XMOD_PITCH);
        b[D_XCUT] = p.v(XMOD_CUTOFF);
        let lfo = lfo_shape(p.choice(LFO_WAVE), self.lfo_phase);
        if self.mm.on(D_LFO_RATE) {
            self.lfo_rate_mod = sm::shared_value(&self.mm, D_LFO_RATE, lfo, self.wheel, &self.macro_off, self.infos[D_LFO_RATE], b[D_LFO_RATE]);
        }
        let unison = p.choice(UNISON_P).min(MAX_UNISON - 1) + 1;
        let copies_moved = self.mm.on(D_UNI_DETUNE) || self.mm.on(D_UNI_SPREAD);
        let mut src = [0.0f32; sm::N_SOURCES];
        src[sm::SRC_LFO] = lfo;
        src[sm::SRC_WHEEL] = self.wheel;
        for v in self.voices.iter_mut().filter(|v| v.active()) {
            src[sm::SRC_AMP] = v.amp.level;
            src[sm::SRC_ENV2] = v.filt.level;
            src[sm::SRC_VEL] = v.vel_raw;
            src[sm::SRC_KEY] = sm::key_value(v.note);
            src[sm::SRC_RND] = v.m.rnd;
            sm::aim_voice(&self.mm, &mut v.m, &src, &self.macro_off, &self.bases, &self.infos, D_LFO_RATE, sm::TICK, |d, x, base| match d {
                D_PITCH => 2f32.powf(x / 12.0),
                D_MIX | D_KEY | D_SUB | D_NOISE | D_LFO_AMP | D_UNI_SPREAD => x / 100.0,
                D_RES => 2.0 - 1.95 * (x / 100.0),
                D_ENV => x / 100.0 * 6.0,
                D_LFO_CUTOFF => x / 100.0 * 4.0,
                D_XPITCH => x / 100.0 * XMOD_PITCH_DEPTH,
                D_XCUT => x / 100.0 * XMOD_CUTOFF_OCT,
                D_LEVEL => sm::level_gain(x, base),
                _ => x,
            });
            if copies_moved {
                let detune = if self.mm.on(D_UNI_DETUNE) { v.m.get(D_UNI_DETUNE) } else { self.bases[D_UNI_DETUNE] };
                let spread = if self.mm.on(D_UNI_SPREAD) { v.m.get(D_UNI_SPREAD) } else { self.bases[D_UNI_SPREAD] / 100.0 };
                v.copies = unison_copies(unison, detune, spread);
            }
        }
    }

    fn update(&mut self) {
        let sr = self.sr;
        let v = |i| self.p.v(i);
        self.amp_rates = AdsrRates::new(v(8), v(9), v(10) / 100.0, v(11), sr);
        self.filt_rates = AdsrRates::new(v(12), v(13), v(14) / 100.0, v(15), sr);
        let glide = v(16);
        self.glide_coef = if glide < 0.5 { 0.0 } else { crate::util::time_coef(glide / 3.0, sr) };
        self.level.set(db_to_gain(v(18)));
        self.cutoff.set(v(5));
        self.tune2.set(v(2) + v(3) / 100.0);
        self.mix.set(v(4) / 100.0);
        self.damping.set(2.0 - 1.95 * (v(6) / 100.0));
        self.env_oct.set(v(7) / 100.0 * 6.0);
    }

    fn start_note(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.stop_note(note);
            return;
        }
        if self.p.choice(VOICE_MODE) != 0 {
            self.mono_push(note);
            self.mono_play(note, vel, true);
            return;
        }
        self.counter += 1;
        let freq = note_hz(f32::from(note.min(127)));
        // Reuse a voice already playing this note, else a free one, else steal.
        let idx = pick_voice(self.voices.iter().map(|v| (v.active(), v.note, v.held, v.amp.level)), note);
        let sens = self.p.v(17) / 100.0;
        let glide = self.glide_coef > 0.0;
        let start = if glide { self.last_freq.unwrap_or(freq) } else { freq };
        let counter = self.counter;
        if let Some(v) = self.voices.get_mut(idx) {
            let was_active = v.active();
            v.note = note;
            v.vel = (1.0 - sens) + sens * f32::from(vel.min(127)) / 127.0;
            v.vel_raw = f32::from(vel.min(127)) / 127.0;
            v.target = freq;
            v.freq = if was_active && glide { v.freq } else { start };
            if !was_active {
                v.osc1.reset();
                v.osc2.phase = 0.37;
                v.svf = Svf::default();
                v.svf2 = Svf::default();
                v.restart_pairs();
            }
            v.amp.gate_on();
            v.filt.gate_on();
            v.age = counter;
            v.held = true;
            v.sustained = false;
        }
        self.mod_note_on(idx);
        self.last_freq = Some(freq);
    }

    /// Mono and Legato: `note` joins the keys held (at the end).
    fn mono_push(&mut self, note: u8) {
        self.mono_pop(note);
        if self.mono_n >= self.mono_held.len() {
            self.mono_held.copy_within(1.., 0);
            self.mono_n = self.mono_held.len() - 1;
        }
        if let Some(slot) = self.mono_held.get_mut(self.mono_n) {
            *slot = note;
            self.mono_n += 1;
        }
    }

    fn mono_pop(&mut self, note: u8) {
        let n = self.mono_n.min(self.mono_held.len());
        if let Some(i) = self.mono_held.iter().take(n).position(|k| *k == note) {
            self.mono_held.copy_within(i + 1..n, i);
            self.mono_n = n - 1;
        }
    }

    /// Mono and Legato: voice 0 plays `note`. Legato with the voice still held moves to it
    /// without starting the envelopes again (gliding with Glide); Mono starts them again. `fresh`:
    /// a key pressed (its velocity counts), not a return to a key still held.
    fn mono_play(&mut self, note: u8, vel: u8, fresh: bool) {
        self.counter += 1;
        let freq = note_hz(f32::from(note.min(127)));
        let legato = self.p.choice(VOICE_MODE) == 2;
        let sens = self.p.v(17) / 100.0;
        let glide = self.glide_coef > 0.0;
        let counter = self.counter;
        // Only voice 0 plays: the others stop.
        for v in self.voices.iter_mut().skip(1) {
            v.held = false;
            v.amp.gate_off();
            v.filt.gate_off();
        }
        if let Some(v) = self.voices.first_mut() {
            let sounding = v.active() && v.held;
            v.note = note;
            v.target = freq;
            if !(sounding && glide) {
                v.freq = freq;
            }
            if !v.active() {
                v.osc1.reset();
                v.osc2.phase = 0.37;
                v.svf = Svf::default();
                v.svf2 = Svf::default();
                v.restart_pairs();
            }
            if fresh {
                v.vel = (1.0 - sens) + sens * f32::from(vel.min(127)) / 127.0;
                v.vel_raw = f32::from(vel.min(127)) / 127.0;
            }
            if !(legato && sounding) {
                v.amp.gate_on();
                v.filt.gate_on();
            }
            v.age = counter;
            v.held = true;
            v.sustained = false;
        }
        if fresh {
            self.mod_note_on(0);
        }
        self.last_freq = Some(freq);
    }

    fn stop_note(&mut self, note: u8) {
        if self.p.choice(VOICE_MODE) != 0 {
            self.mono_pop(note);
            let playing = self.voices.first().is_some_and(|v| v.held && v.note == note);
            // Back to the last key still held, else let go.
            if playing && let Some(prev) = self.mono_n.checked_sub(1).and_then(|i| self.mono_held.get(i)).copied() {
                self.mono_play(prev, 100, false);
                return;
            }
        }
        let sustain = self.sustain;
        for v in self.voices.iter_mut().filter(|v| v.held && v.note == note) {
            v.held = false;
            if sustain {
                v.sustained = true;
            } else {
                v.amp.gate_off();
                v.filt.gate_off();
            }
        }
    }

    fn set_sustain(&mut self, on: bool) {
        self.sustain = on;
        if !on {
            for v in self.voices.iter_mut().filter(|v| v.sustained) {
                v.sustained = false;
                v.amp.gate_off();
                v.filt.gate_off();
            }
        }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(n, v) => self.start_note(n, v),
            EventKind::Off(n) => self.stop_note(n),
            EventKind::AllOff => {
                self.mono_n = 0;
                for v in &mut self.voices {
                    v.held = false;
                    v.sustained = false;
                    v.amp.gate_off();
                    v.filt.gate_off();
                }
            }
            EventKind::Sustain(on) => self.set_sustain(on),
            EventKind::Bend(b) => self.bend = if b.is_finite() { b.clamp(-1.0, 1.0) } else { 0.0 },
            EventKind::Mod(m) => self.wheel = if m.is_finite() { m.clamp(0.0, 1.0) } else { 0.0 },
            EventKind::ResetControllers => {
                self.set_sustain(false);
                self.bend = 0.0;
                self.wheel = 0.0;
            }
        }
    }

    /// How far the bend (over Bend Range) and the mod wheel's vibrato move the pitch now, cents.
    #[inline]
    fn pitch_cents(&self) -> f32 {
        let mut cents = self.bend * self.p.v(BEND_RANGE) * 100.0;
        if self.wheel > 0.0 {
            cents += self.p.v(MOD_WHEEL) * self.wheel * (std::f32::consts::TAU * self.vib_phase).sin();
        }
        cents
    }

    /// One sample, left and right: the same unless unison spreads the copies.
    #[inline]
    fn render_sample(&mut self) -> (f32, f32) {
        if self.mm.active() {
            self.mod_step();
        }
        let unison = self.p.choice(UNISON_P).min(MAX_UNISON - 1) + 1;
        if unison > 1 || self.p.choice(OSC_SYNC) == 1 || self.xmod_on() {
            return self.render_wide(unison);
        }
        let y = self.render_plain();
        (y, y)
    }

    /// The synth as it was before unison and hard sync, sample for sample.
    #[inline]
    fn render_plain(&mut self) -> f32 {
        let sr = self.sr;
        let w1 = wave(self.p.choice(0));
        let w2 = wave(self.p.choice(1));
        let ratio2 = 2f32.powf(self.tune2.next() / 12.0);
        let mix = self.mix.next();
        let k = self.damping.next();
        let env_oct = self.env_oct.next();
        let cutoff = self.cutoff.next();
        let nyq = sr * 0.45;
        // the bend over its range, and the mod wheel's vibrato, as one frequency ratio
        let cents = self.pitch_cents();
        let cents_base = cents;
        self.vib_phase = (self.vib_phase + VIBRATO_HZ / sr).fract();
        // The LFO: one for every voice, -1..1.
        let ph = self.lfo_phase;
        let lfo = match self.p.choice(LFO_WAVE) {
            1 => 1.0 - 4.0 * (ph - 0.5).abs(),
            2 => {
                if ph < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            3 => 2.0 * ph - 1.0,
            _ => (std::f32::consts::TAU * ph).sin(),
        };
        self.lfo_phase = (self.lfo_phase + self.lfo_rate() / sr).fract();
        let cents = cents + self.p.v(LFO_PITCH) * lfo;
        let lfo_oct = self.p.v(LFO_CUTOFF) / 100.0 * 4.0 * lfo;
        let lfo_amp = 1.0 - self.p.v(LFO_AMP) / 100.0 * 0.5 * (1.0 - lfo);
        let kind = self.p.choice(FILTER_TYPE);
        let key_track = self.p.v(KEY_TRACK) / 100.0;
        let sub_level = self.p.v(SUB_LEVEL) / 100.0;
        let noise_level = self.p.v(NOISE_LEVEL) / 100.0;
        let drive_amt = self.p.v(DRIVE) / 100.0;
        let bend = if cents == 0.0 { 1.0 } else { 2f32.powf(cents / 1200.0) };
        let modded = self.mm.active();
        let sh = Shared {
            cents: cents_base,
            lfo,
            lfo_pitch: self.p.v(LFO_PITCH),
            bend,
            semis2: self.p.v(2),
            detune2: self.p.v(3),
            ratio2,
            mix,
            k,
            env_oct,
            cutoff,
            lfo_oct,
            lfo_amp,
            key_track,
            sub_level,
            noise_level,
            gain: 1.0,
            xpitch: self.p.v(XMOD_PITCH) / 100.0 * XMOD_PITCH_DEPTH,
            xcut: self.p.v(XMOD_CUTOFF) / 100.0 * XMOD_CUTOFF_OCT,
        };
        let mut out = 0.0;
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            let Shared { bend, ratio2, mix, k, env_oct, cutoff, lfo_oct, lfo_amp, key_track, sub_level, noise_level, gain, .. } =
                if modded { sh.modded(&self.mm, &v.m) } else { sh };
            if self.glide_coef > 0.0 {
                v.freq = v.target + (v.freq - v.target) * self.glide_coef;
            } else {
                v.freq = v.target;
            }
            let a = v.amp.next(&self.amp_rates);
            let fe = v.filt.next(&self.filt_rates);
            let s1 = v.osc1.next(w1, v.freq * bend / sr);
            let s2 = v.osc2.next(w2, v.freq * bend * ratio2 / sr);
            let mut raw = s1 * (1.0 - mix) + s2 * mix;
            if sub_level > 0.0 {
                raw += sub_level * v.sub.next(Waveform::Square, v.freq * bend * 0.5 / sr);
            }
            if noise_level > 0.0 {
                v.noise ^= v.noise << 13;
                v.noise ^= v.noise >> 17;
                v.noise ^= v.noise << 5;
                raw += noise_level * (v.noise as f32 / u32::MAX as f32 * 2.0 - 1.0);
            }
            let track = if key_track > 0.0 { key_track * (f32::from(v.note) - 60.0) / 12.0 } else { 0.0 };
            let fc = (cutoff * 2f32.powf(env_oct * fe + track + lfo_oct)).clamp(20.0, nyq);
            let g = (std::f32::consts::PI * fc / sr).tan();
            let raw = if drive_amt > 0.0 { drive(raw, drive_amt) } else { raw };
            let mut y = v.svf.run(raw, g, k, kind);
            if kind == 1 {
                y = v.svf2.run(y, g, k, 1);
            }
            out += y * a * v.vel * lfo_amp * gain;
        }
        out * 0.3
    }

    /// The control values one sample shares across voices (as `render_plain` reads them).
    #[inline]
    fn lfo_now(&mut self) -> f32 {
        let lfo = lfo_shape(self.p.choice(LFO_WAVE), self.lfo_phase);
        self.lfo_phase = (self.lfo_phase + self.lfo_rate() / self.sr).fract();
        lfo
    }

    /// Unison and/or hard sync: `unison` copies of the oscillator pair per voice, detuned across
    /// Unison Detune and panned across Unison Spread (constant power, the stack's level kept by
    /// 1/√n), each side through its own filter.
    #[inline]
    fn render_wide(&mut self, unison: usize) -> (f32, f32) {
        let sr = self.sr;
        let w1 = wave(self.p.choice(0));
        let w2 = wave(self.p.choice(1));
        let ratio2 = 2f32.powf(self.tune2.next() / 12.0);
        let mix = self.mix.next();
        let k = self.damping.next();
        let env_oct = self.env_oct.next();
        let cutoff = self.cutoff.next();
        let nyq = sr * 0.45;
        let cents = self.pitch_cents();
        let cents_base = cents;
        self.vib_phase = (self.vib_phase + VIBRATO_HZ / sr).fract();
        let lfo = self.lfo_now();
        let cents = cents + self.p.v(LFO_PITCH) * lfo;
        let lfo_oct = self.p.v(LFO_CUTOFF) / 100.0 * 4.0 * lfo;
        let lfo_amp = 1.0 - self.p.v(LFO_AMP) / 100.0 * 0.5 * (1.0 - lfo);
        let kind = self.p.choice(FILTER_TYPE);
        let key_track = self.p.v(KEY_TRACK) / 100.0;
        let sub_level = self.p.v(SUB_LEVEL) / 100.0;
        let noise_level = self.p.v(NOISE_LEVEL) / 100.0;
        let drive_amt = self.p.v(DRIVE) / 100.0;
        let sync = self.p.choice(OSC_SYNC) == 1;
        let x_sub = self.p.choice(XMOD_SOURCE) == 1;
        let xmod = self.xmod_on();
        let detune = self.p.v(UNISON_DETUNE);
        let spread = self.p.v(UNISON_SPREAD) / 100.0;
        let bend = if cents == 0.0 { 1.0 } else { 2f32.powf(cents / 1200.0) };
        let copies = unison_copies(unison, detune, spread);
        let modded = self.mm.active();
        let own_copies = self.mm.on(D_UNI_DETUNE) || self.mm.on(D_UNI_SPREAD);
        let sh = Shared {
            cents: cents_base,
            lfo,
            lfo_pitch: self.p.v(LFO_PITCH),
            bend,
            semis2: self.p.v(2),
            detune2: self.p.v(3),
            ratio2,
            mix,
            k,
            env_oct,
            cutoff,
            lfo_oct,
            lfo_amp,
            key_track,
            sub_level,
            noise_level,
            gain: 1.0,
            xpitch: self.p.v(XMOD_PITCH) / 100.0 * XMOD_PITCH_DEPTH,
            xcut: self.p.v(XMOD_CUTOFF) / 100.0 * XMOD_CUTOFF_OCT,
        };
        let (mut out_l, mut out_r) = (0.0, 0.0);
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            let Shared { bend, ratio2, mix, k, env_oct, cutoff, lfo_oct, lfo_amp, key_track, sub_level, noise_level, gain, xpitch, xcut, .. } =
                if modded { sh.modded(&self.mm, &v.m) } else { sh };
            let copies = if own_copies { v.copies } else { copies };
            if self.glide_coef > 0.0 {
                v.freq = v.target + (v.freq - v.target) * self.glide_coef;
            } else {
                v.freq = v.target;
            }
            let a = v.amp.next(&self.amp_rates);
            let fe = v.filt.next(&self.filt_rates);
            let base = v.freq * bend / sr;
            let (mut l, mut r) = (0.0, 0.0);
            // Cross-modulation: the source's last sample (oscillator 2's of the first copy, or
            // the sub's) for the cutoff; each copy's own oscillator 2 for its oscillator 1.
            let x_cut_src = if x_sub { v.last_sub } else { v.pairs[0].last2 };
            for (pair, &(ratio, gl, gr)) in v.pairs.iter_mut().zip(copies.iter()).take(unison) {
                let inc1 = if xmod && xpitch != 0.0 {
                    let s = if x_sub { v.last_sub } else { pair.last2 };
                    base * ratio * (1.0 + xpitch * s).max(0.0)
                } else {
                    base * ratio
                };
                let (s1, s2) = pair.next(w1, w2, inc1, base * ratio * ratio2, sync);
                let raw = s1 * (1.0 - mix) + s2 * mix;
                l += raw * gl;
                r += raw * gr;
            }
            let mut centre = 0.0;
            if sub_level > 0.0 || (xmod && x_sub) {
                let s = v.sub.next(Waveform::Square, base * 0.5);
                v.last_sub = s;
                if sub_level > 0.0 {
                    centre += sub_level * s;
                }
            }
            if noise_level > 0.0 {
                v.noise ^= v.noise << 13;
                v.noise ^= v.noise >> 17;
                v.noise ^= v.noise << 5;
                centre += noise_level * (v.noise as f32 / u32::MAX as f32 * 2.0 - 1.0);
            }
            let track = if key_track > 0.0 { key_track * (f32::from(v.note) - 60.0) / 12.0 } else { 0.0 };
            let oct = env_oct * fe + track + lfo_oct;
            let oct = if xmod && xcut != 0.0 { oct + xcut * x_cut_src } else { oct };
            let fc = (cutoff * 2f32.powf(oct)).clamp(20.0, nyq);
            let g = (std::f32::consts::PI * fc / sr).tan();
            let gain = a * v.vel * lfo_amp * gain;
            let (inl, inr) = if drive_amt > 0.0 { (drive(l + centre, drive_amt), drive(r + centre, drive_amt)) } else { (l + centre, r + centre) };
            if unison == 1 {
                // Sync alone: one path, the same on both sides.
                let mut y = v.svf.run(inl, g, k, kind);
                if kind == 1 {
                    y = v.svf2.run(y, g, k, 1);
                }
                out_l += y * gain;
                out_r += y * gain;
                continue;
            }
            let mut yl = v.svf.run(inl, g, k, kind);
            let mut yr = v.svf_r.run(inr, g, k, kind);
            if kind == 1 {
                yl = v.svf2.run(yl, g, k, 1);
                yr = v.svf2_r.run(yr, g, k, 1);
            }
            out_l += yl * gain;
            out_r += yr * gain;
        }
        (out_l * 0.3, out_r * 0.3)
    }
}

impl Plugin for SubtractiveSynth {
    param_plumbing!(SYNTH_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.cutoff.set_time(20.0, self.sr);
        for s in [&mut self.tune2, &mut self.mix, &mut self.damping, &mut self.env_oct] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn set_prepared_state(&mut self, data: &[f64]) -> bool {
        self.mm.load(data, MOD_DESTS.len());
        for v in &mut self.voices {
            v.m.fresh = true;
        }
        self.tick_left = 0;
        true
    }

    fn reset(&mut self) {
        for (i, v) in self.voices.iter_mut().enumerate() {
            *v = Voice::new(0x1000 + i as u32 * 31);
        }
        self.rng = sm::RANDOM_SEED;
        self.tick_left = 0;
        self.events.clear();
        self.last_freq = None;
        self.level.snap();
        self.cutoff.snap();
        for s in [&mut self.tune2, &mut self.mix, &mut self.damping, &mut self.env_oct] {
            s.snap();
        }
        self.sustain = false;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.vib_phase = 0.0;
        self.lfo_phase = 0.0;
        self.mono_n = 0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let mut evs = [Event { offset: 0, kind: EventKind::AllOff }; MAX_EVENTS];
        let count = {
            let sorted = self.events.sorted();
            for (d, s) in evs.iter_mut().zip(sorted) {
                *d = *s;
            }
            sorted.len().min(MAX_EVENTS)
        };
        self.events.clear();
        let mut next = 0;
        for n in 0..frames {
            while let Some(e) = evs.get(next).filter(|_| next < count) {
                if e.offset > n && n + 1 < frames {
                    break;
                }
                let kind = e.kind;
                self.apply(kind);
                next += 1;
            }
            let (l, r) = self.render_sample();
            let g = self.level.next();
            let (l, r) = (l * g, r * g);
            if ch == 1 {
                // One channel: both sides (the same sample when unison does not spread them).
                let y = if l == r { l } else { 0.5 * (l + r) };
                if let Some(x) = io.first_mut().and_then(|b| b.get_mut(n)) {
                    *x = y;
                }
                continue;
            }
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                if let Some(x) = b.get_mut(n) {
                    *x = if c % 2 == 0 { l } else { r };
                }
            }
        }
        // Events for an empty block take effect immediately.
        for e in evs.iter().take(count).skip(next) {
            self.apply(e.kind);
        }
    }

    fn tail_samples(&self) -> usize {
        // The release is "to -60 dB"; a voice rings on to `SILENT` (-100 dB).
        ms_to_samples(self.p.v(11) * (SILENT.ln() / 0.001f32.ln()), self.sr) as usize
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.events.push(offset, EventKind::On(note, velocity));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.events.push(offset, EventKind::Off(note));
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        if let Some(kind) = event_of(msg) {
            self.events.push(offset, kind);
        }
    }

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(set: &[(&str, f32)], unison: bool) -> Vec<f32> {
        let mut s = SubtractiveSynth::new();
        s.prepare(48_000.0, 256, 2);
        for (k, v) in set {
            assert!(s.set_param(k, *v));
        }
        if unison {
            assert!(s.set_param("unison", 2.0));
        }
        s.note_on(0, 45, 110);
        let mut out = Vec::new();
        let mut io = vec![vec![0.0f32; 256]; 2];
        for _ in 0..40 {
            s.process(&mut io, 256);
            out.extend_from_slice(&io[0]);
        }
        out
    }

    /// Drive at 0 leaves the synth as it was, sample for sample (plain and unison paths); turned
    /// up, it saturates the oscillators into the filter: a full-scale input stays at full scale,
    /// quieter ones are lifted, and the sound changes.
    #[test]
    fn drive_at_zero_changes_nothing_and_up_it_saturates() {
        for unison in [false, true] {
            assert_eq!(render(&[], unison), render(&[("drive", 0.0)], unison));
            let driven = render(&[("drive", 80.0), ("cutoff", 20_000.0)], unison);
            let clean = render(&[("cutoff", 20_000.0)], unison);
            assert!(driven.iter().all(|x| x.is_finite()));
            assert!(driven.iter().zip(&clean).any(|(a, b)| (a - b).abs() > 1e-3));
        }
        assert!((drive(1.0, 1.0) - 1.0).abs() < 1e-6 && (drive(-1.0, 0.5) + 1.0).abs() < 1e-6);
        assert!(drive(0.1, 1.0) > 0.5, "quiet parts are lifted");
        assert_eq!(drive(f32::NAN, 1.0), 0.0);
    }

    #[test]
    fn a_full_event_queue_drops_and_counts_without_growing() {
        let mut q = EventQueue::new();
        let cap = q.ev.capacity();
        for i in 0..MAX_EVENTS + 7 {
            q.push(i, EventKind::AllOff);
        }
        assert_eq!((q.ev.len(), q.dropped()), (MAX_EVENTS, 7));
        assert_eq!(q.ev.capacity(), cap, "never reallocated");
    }

    /// Bug 28: the oscillator mix, resonance, detune and envelope amount glide to a new value
    /// instead of stepping; once settled they are exactly the parameter's value.
    #[test]
    fn mix_resonance_detune_and_envelope_amount_are_smoothed() {
        let mut s = SubtractiveSynth::new();
        s.prepare(48_000.0, 512, 2);
        assert_eq!(s.mix.value(), 0.5);
        assert!(s.set_param("osc_mix", 100.0) && s.set_param("resonance", 100.0));
        s.note_on(0, 60, 100);
        let mut io = vec![vec![0.0f32; 64]; 2];
        s.process(&mut io, 64);
        assert!(s.mix.value() > 0.5 && s.mix.value() < 0.95, "{}", s.mix.value());
        for _ in 0..2_000 {
            s.process(&mut io, 64);
        }
        assert_eq!((s.mix.value(), s.damping.value()), (1.0, 2.0 - 1.95));
    }

    /// Doc 07: Bend Range sets the bend's throw; the mod wheel brings in a vibrato up to Mod Wheel
    /// Vibrato, none with the wheel down; resetting the controllers centres both.
    #[test]
    fn bend_range_and_the_mod_wheel() {
        let mut s = SubtractiveSynth::new();
        s.prepare(48_000.0, 512, 2);
        let mut io = vec![vec![0.0f32; 64]; 2];
        let swing = |s: &mut SubtractiveSynth, io: &mut Vec<Vec<f32>>| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for _ in 0..400 {
                s.process(io, 64);
                lo = lo.min(s.pitch_cents());
                hi = hi.max(s.pitch_cents());
            }
            (lo, hi)
        };
        s.midi(0, [0xE0, 0x7F, 0x7F]);
        s.process(&mut io, 64);
        assert!((s.pitch_cents() - 200.0).abs() < 0.1, "two semitones by default: {}", s.pitch_cents());
        assert!(s.set_param("bend_range", 12.0));
        assert!((s.pitch_cents() - 1200.0).abs() < 0.2);
        s.midi(0, [0xE0, 0x00, 0x40]);
        assert_eq!(swing(&mut s, &mut io), (0.0, 0.0), "no wheel, no vibrato");
        s.midi(0, [0xB0, 1, 127]);
        // Mod Wheel Vibrato starts at 0 (owner's decision): mod-wheel data in a session made
        // before it brings in no vibrato until the user turns it up.
        assert_eq!(swing(&mut s, &mut io), (0.0, 0.0), "Mod Wheel Vibrato at 0 by default");
        assert!(s.set_param("mod_wheel", 50.0));
        let (lo, hi) = swing(&mut s, &mut io);
        assert!(lo < -45.0 && hi > 45.0 && lo >= -50.01 && hi <= 50.01, "{lo} {hi}");
        s.midi(0, [0xB0, 121, 0]);
        s.process(&mut io, 64);
        assert_eq!(s.pitch_cents(), 0.0);
    }

    /// Bug 30: the synth's and the drum synth's reported tails cover the sound after the last
    /// note-off (a freeze or a bounce stops there).
    #[test]
    fn reported_tails_cover_the_release() {
        for (id, note) in [("subtractive_synth", 60u8), ("drum_synth", 46)] {
            let Some(mut p) = crate::create(id) else { panic!("{id}") };
            p.prepare(48_000.0, 512, 2);
            p.note_on(0, note, 127);
            let mut io = vec![vec![0.0f32; 512]; 2];
            p.process(&mut io, 512);
            p.note_off(0, note);
            let tail = p.tail_samples();
            let mut last = 0usize;
            for b in 0..(tail / 512 + 400) {
                p.process(&mut io, 512);
                if io.iter().any(|c| c.iter().any(|x| x.abs() > 1.0e-6)) {
                    last = (b + 1) * 512;
                }
            }
            assert!(last <= tail + 512, "{id}: sound until {last}, tail {tail}");
        }
    }
}

#[cfg(test)]
mod upgrade_tests {
    use super::*;

    fn render(set: &[(&str, f32)], notes: &[(usize, u8)], frames: usize) -> Vec<f32> {
        let mut s = SubtractiveSynth::new();
        s.prepare(48_000.0, 512, 1);
        for (id, v) in set {
            assert!(s.set_param(id, *v), "{id}");
        }
        let mut out = Vec::with_capacity(frames);
        let mut io = vec![vec![0.0f32; 256]];
        let mut at = 0;
        while at < frames {
            for (when, note) in notes.iter().filter(|(w, _)| (at..at + 256).contains(w)) {
                s.note_on(when - at, *note, 100);
            }
            s.process(&mut io, 256);
            out.extend_from_slice(&io[0]);
            at += 256;
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// The new filter code at its defaults is the low-pass the synth had, sample for sample.
    #[test]
    fn the_default_filter_is_the_old_low_pass() {
        let (mut a, mut b) = (Svf::default(), Svf::default());
        for i in 0..2_000 {
            let x = ((i * 7919) % 200) as f32 / 100.0 - 1.0;
            assert_eq!(a.run(x, 0.3, 0.8, 0).to_bits(), b.lowpass(x, 0.3, 0.8).to_bits());
        }
    }

    #[test]
    fn high_pass_thins_a_low_note_the_low_pass_keeps() {
        let low = |kind: f32| rms(&render(&[("filter_type", kind), ("cutoff", 2000.0), ("filter_env_amount", 0.0)], &[(0, 36)], 24_000)[4_000..]);
        assert!(low(2.0) < low(0.0) * 0.5, "{} against {}", low(2.0), low(0.0));
        // 24 dB takes off more of a bright saw above the cutoff than 12 dB.
        let bright = |kind: f32| rms(&render(&[("filter_type", kind), ("cutoff", 200.0), ("filter_env_amount", 0.0)], &[(0, 72)], 24_000)[4_000..]);
        assert!(bright(1.0) < bright(0.0), "{} against {}", bright(1.0), bright(0.0));
    }

    #[test]
    fn the_lfo_to_level_makes_a_tremolo() {
        let x = render(&[("lfo_amp", 100.0), ("lfo_rate", 4.0), ("lfo_wave", 2.0), ("amp_sustain", 100.0)], &[(0, 60)], 48_000);
        // A square LFO at 4 Hz: full for an eighth of a second, then silent.
        let a = rms(&x[2_000..5_000]);
        let b = rms(&x[8_000..11_000]);
        assert!(b < a * 0.1 || a < b * 0.1, "{a} {b}");
    }

    #[test]
    fn sub_and_noise_add_to_the_sound_only_when_raised() {
        let base = rms(&render(&[], &[(0, 60)], 12_000)[2_000..]);
        assert!(rms(&render(&[("sub_level", 100.0)], &[(0, 60)], 12_000)[2_000..]) > base * 1.1);
        assert!(rms(&render(&[("noise_level", 100.0), ("cutoff", 20000.0)], &[(0, 60)], 12_000)[2_000..]) > base * 1.1);
        assert_eq!(render(&[("sub_level", 0.0), ("noise_level", 0.0)], &[(0, 60)], 4_096), render(&[], &[(0, 60)], 4_096));
    }

    /// What the synth plays at its defaults and with each of the first twelve factory presets
    /// (a phrase with a chord, a bend and the mod wheel), as a fingerprint compared within
    /// [`crate::plugins::golden::TOLERANCE`] (1e-4, -80 dB): see that module for why not bit for
    /// bit. The stored fingerprints (`synth/reference.rs`) were rendered on a machine where the
    /// same renders also matched, bit for bit, hashes taken before unison and hard sync were
    /// added, so unison and sync at their defaults leave the sound as it was.
    #[test]
    fn the_defaults_and_the_factory_presets_sound_as_before_unison_and_sync() {
        let sets: Vec<(&str, &[(&str, f32)])> =
            std::iter::once(("defaults", &[][..])).chain(presets::SYNTH_PRESETS.iter().take(12).map(|p| (p.name, p.set))).collect();
        let got: Vec<Vec<f32>> = sets
            .iter()
            .map(|(_, set)| {
                let mut s = SubtractiveSynth::new();
                for (id, v) in *set {
                    s.set_param(id, *v);
                }
                crate::plugins::golden::fingerprint(&mut s)
            })
            .collect();
        let names: Vec<&str> = sets.iter().map(|(n, _)| *n).collect();
        crate::plugins::golden::check(&names, &got, reference::REF, "src/plugins/synth/reference.rs");
    }

    fn render2(set: &[(&str, f32)], notes: &[(usize, u8)], frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut s = SubtractiveSynth::new();
        s.prepare(48_000.0, 512, 2);
        for (id, v) in set {
            assert!(s.set_param(id, *v), "{id}");
        }
        let (mut l, mut r) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        let mut io = vec![vec![0.0f32; 256]; 2];
        let mut at = 0;
        while at < frames {
            for (when, note) in notes.iter().filter(|(w, _)| (at..at + 256).contains(w)) {
                s.note_on(when - at, *note, 100);
            }
            s.process(&mut io, 256);
            l.extend_from_slice(&io[0]);
            r.extend_from_slice(&io[1]);
            at += 256;
        }
        (l, r)
    }

    /// Hann-windowed DFT magnitude of `x` at `hz` (48 kHz).
    fn mag(x: &[f32], hz: f64) -> f64 {
        let n = x.len() as f64;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n).cos();
            let a = std::f64::consts::TAU * hz * i as f64 / 48_000.0;
            re += f64::from(*v) * w * a.cos();
            im += f64::from(*v) * w * a.sin();
        }
        (re * re + im * im).sqrt()
    }

    #[test]
    fn unison_spreads_a_detuned_stack_across_the_sides_at_the_same_level() {
        let set = |n: f32, spread: f32| vec![("unison", n), ("unison_spread", spread), ("unison_detune", 25.0), ("amp_sustain", 100.0)];
        let (pl, pr) = render2(&[("amp_sustain", 100.0)], &[(0, 57)], 24_000);
        assert_eq!(pl, pr, "one copy: the same on both sides");
        let (l, r) = render2(&set(4.0, 100.0), &[(0, 57)], 24_000);
        let diff = rms(&l.iter().zip(&r).map(|(a, b)| a - b).collect::<Vec<_>>()[4_000..]);
        assert!(diff > rms(&l[4_000..]) * 0.2, "spread: the sides differ ({diff})");
        // The stack keeps the level of one copy within 3 dB.
        let one = rms(&pl[4_000..]);
        let both = ((rms(&l[4_000..]).powi(2) + rms(&r[4_000..]).powi(2)) / 2.0).sqrt();
        assert!((both / one).log10().abs() * 20.0 < 3.0, "{both} against {one}");
        // No spread: the sides are the same, but the detuned copies still beat against each other.
        let (l0, r0) = render2(&set(4.0, 0.0), &[(0, 57)], 24_000);
        assert_eq!(l0, r0);
        assert_ne!(l0, pl);
        // Detune widens each harmonic: by the eighth harmonic's outer copy, 25 cents up, a
        // detuned stack has far more energy than one copy.
        let f = 1760.0;
        let side = |x: &[f32]| mag(x, f * 2f64.powf(25.0 / 1200.0)) / mag(x, f);
        assert!(side(&l0[4_000..]) > side(&pl[4_000..]) * 2.0, "{} {}", side(&l0[4_000..]), side(&pl[4_000..]));
        // A mono output folds the sides.
        let mono = render(&set(4.0, 100.0), &[(0, 57)], 4_096);
        assert!(mono.iter().zip(l.iter().zip(&r)).all(|(m, (a, b))| (m - 0.5 * (a + b)).abs() < 1e-6));
    }

    /// The restart is band-limited: what folds back between the harmonics of oscillator 1 is
    /// well under what a naive restart leaves, and the sound repeats at oscillator 1's period.
    #[test]
    fn hard_sync_locks_osc_2_to_osc_1_without_the_naive_aliasing() {
        let (f1, f2) = (1234.5f64, 1234.5f64 * 2f64.powf(7.0 / 12.0));
        let (dt1, dt2) = ((f1 / 48_000.0) as f32, (f2 / 48_000.0) as f32);
        for w in [Waveform::Saw, Waveform::Square] {
            let mut p = Pair::new(3);
            p.osc1.phase = 0.0;
            p.osc2.phase = 0.0;
            let ours: Vec<f32> = (0..48_000).map(|_| p.next(Waveform::Saw, w, dt1, dt2, true).1).collect();
            let (mut a, mut b) = (0.0f64, 0.0f64);
            let naive: Vec<f32> = (0..48_000)
                .map(|_| {
                    let y = naive(w, b as f32);
                    a += f1 / 48_000.0;
                    b += f2 / 48_000.0;
                    if a >= 1.0 {
                        a -= 1.0;
                        b = a * f2 / f1;
                    }
                    b = b.fract();
                    y
                })
                .collect();
            let between = |x: &[f32]| (1..15).map(|k| mag(x, (k as f64 + 0.5) * f1)).sum::<f64>();
            let harmonics = |x: &[f32]| (1..15).map(|k| mag(x, k as f64 * f1)).sum::<f64>();
            // At least 6 dB less folded back (the saw does far better; a square restarted while
            // high has no jump at all, so less of its folding came from the restart).
            let most = if w == Waveform::Saw { 0.35 } else { 0.5 };
            assert!(between(&ours) < between(&naive) * most, "{w:?}: {} against {}", between(&ours), between(&naive));
            assert!(harmonics(&ours) > between(&ours) * 20.0, "{w:?}: locked to oscillator 1");
            assert!(ours.iter().all(|x| x.is_finite() && x.abs() < 2.5));
        }
        // In the synth: off, it is the plain sound; on, oscillator 2 follows oscillator 1.
        let plain = render(&[("osc_mix", 100.0), ("osc2_semitones", 7.0)], &[(0, 48)], 8_192);
        let synced = render(&[("osc_mix", 100.0), ("osc2_semitones", 7.0), ("osc_sync", 1.0)], &[(0, 48)], 8_192);
        assert_ne!(plain, synced);
        assert!(synced.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn mono_plays_one_voice_and_returns_to_the_key_still_held() {
        let mut s = SubtractiveSynth::new();
        s.prepare(48_000.0, 512, 1);
        assert!(s.set_param("voice_mode", 2.0));
        let mut io = vec![vec![0.0f32; 64]];
        s.note_on(0, 60, 100);
        s.note_on(1, 64, 100);
        s.process(&mut io, 64);
        assert_eq!(s.voices.iter().filter(|v| v.active() && v.held).count(), 1);
        assert_eq!(s.voices[0].note, 64);
        s.note_off(0, 64);
        s.process(&mut io, 64);
        assert_eq!((s.voices[0].note, s.voices[0].held), (60, true), "back to the key still held");
        s.note_off(0, 60);
        s.process(&mut io, 64);
        assert!(!s.voices[0].held);
    }
}
