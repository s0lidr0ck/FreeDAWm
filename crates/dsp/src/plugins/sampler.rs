//! Lift - Sampler (id `sampler`): plays one recorded sample across the keyboard, 32 voices.
//!
//! Each note plays the sample pitched from its root note (an octave up plays twice as fast) with
//! 4-point cubic interpolation, which also converts the sample's own rate to the host's. Key and
//! velocity ranges, an amp ADSR with velocity sensitivity, a start and an end point, forward or
//! ping-pong loops, gate or one-shot playback, a resonant filter (low-, high- or band-pass) with
//! its own attack/decay envelope, an LFO on pitch, filter and volume, glide, mono (legato) or
//! poly voicing, and pan. The host hands the audio over with [`Plugin::set_sample`]; without one
//! the Sampler is silent.
//!
//! The filter, LFO, glide, voicing and pan parameters came after the first release: their
//! defaults (filter Off, no LFO depth, no glide, Poly, centre) play an older session as before.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use super::synth::{Adsr, AdsrRates, Event, EventKind, EventQueue, MAX_EVENTS, Stage};
use crate::params::{Params, choice, lin, log, param_plumbing, toggle};
use crate::util::{Rng, Smoother, frames_in, ms_to_samples, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, SampleSource, Taper, Unit, db_to_gain};

const PLAY_MODES: &[&str] = &["Gate", "One Shot"];
const LOOP_MODES: &[&str] = &["Off", "Forward", "Ping-Pong"];
/// Filter types, in the order of the `filter_type` choice.
pub const FILTER_TYPES: &[&str] = &["Off", "Low-pass", "High-pass", "Band-pass"];
/// LFO shapes, in the order of the `lfo_shape` choice.
pub const LFO_SHAPES: &[&str] = &["Sine", "Triangle", "Square", "Saw", "Random"];
const VOICE_MODES: &[&str] = &["Poly", "Mono"];

const ROOT: usize = 0;
const TRANSPOSE: usize = 1;
const FINE: usize = 2;
const KEY_TRACK: usize = 3;
const KEY_LOW: usize = 4;
const KEY_HIGH: usize = 5;
const VEL_LOW: usize = 6;
const VEL_HIGH: usize = 7;
const VEL_SENS: usize = 8;
const ATTACK: usize = 9;
const DECAY: usize = 10;
const SUSTAIN: usize = 11;
const RELEASE: usize = 12;
const PLAY_MODE: usize = 13;
const START: usize = 14;
const LOOP_MODE: usize = 15;
const LOOP_START: usize = 16;
const LOOP_END: usize = 17;
const LEVEL: usize = 18;
const END: usize = 19;
const FILTER_TYPE: usize = 20;
const CUTOFF: usize = 21;
const RESONANCE: usize = 22;
const FILTER_ENV: usize = 23;
const FILTER_ATTACK: usize = 24;
const FILTER_DECAY: usize = 25;
const LFO_SHAPE: usize = 26;
const LFO_RATE: usize = 27;
const LFO_PITCH: usize = 28;
const LFO_FILTER: usize = 29;
const LFO_VOLUME: usize = 30;
const GLIDE: usize = 31;
const VOICE_MODE: usize = 32;
const PAN: usize = 33;
const LOOP_XFADE: usize = 34;

fn pan_display(v: f32) -> String {
    let v = v.round();
    if v.abs() < 0.5 {
        "C".to_string()
    } else if v < 0.0 {
        format!("L{:.0}", -v)
    } else {
        format!("R{v:.0}")
    }
}

// New parameters go at the end: sessions and presets store them by id, and the indices above
// are the order of this table.
static SAMPLER_PARAMS: [ParamInfo; 35] = [
    lin("root_note", "Root Note", 0.0, 127.0, 60.0, Unit::None),
    lin("transpose", "Transpose", -48.0, 48.0, 0.0, Unit::Semitones),
    lin("fine_tune", "Fine Tune", -100.0, 100.0, 0.0, Unit::Cents),
    toggle("key_track", "Key Tracking", true),
    lin("key_low", "Lowest Key", 0.0, 127.0, 0.0, Unit::None),
    lin("key_high", "Highest Key", 0.0, 127.0, 127.0, Unit::None),
    lin("vel_low", "Lowest Velocity", 1.0, 127.0, 1.0, Unit::None),
    lin("vel_high", "Highest Velocity", 1.0, 127.0, 127.0, Unit::None),
    lin("velocity", "Velocity Sens", 0.0, 100.0, 70.0, Unit::Percent),
    log("attack", "Attack", 0.1, 5000.0, 1.0, Unit::Ms),
    log("decay", "Decay", 1.0, 10000.0, 300.0, Unit::Ms),
    lin("sustain", "Sustain", 0.0, 100.0, 100.0, Unit::Percent),
    log("release", "Release", 1.0, 10000.0, 150.0, Unit::Ms),
    choice("play_mode", "Play Mode", PLAY_MODES, 0),
    lin("start", "Sample Start", 0.0, 100.0, 0.0, Unit::Percent),
    choice("loop_mode", "Loop", LOOP_MODES, 0),
    lin("loop_start", "Loop Start", 0.0, 100.0, 0.0, Unit::Percent),
    lin("loop_end", "Loop End", 0.0, 100.0, 100.0, Unit::Percent),
    lin("level", "Level", -48.0, 12.0, 0.0, Unit::Db),
    lin("end", "Sample End", 0.0, 100.0, 100.0, Unit::Percent),
    choice("filter_type", "Filter", FILTER_TYPES, 0),
    log("cutoff", "Cutoff", 20.0, 20000.0, 2000.0, Unit::Hz),
    lin("resonance", "Resonance", 0.0, 100.0, 20.0, Unit::Percent),
    lin("filter_env", "Filter Env", -72.0, 72.0, 0.0, Unit::Semitones),
    log("filter_attack", "Filter Attack", 0.1, 5000.0, 1.0, Unit::Ms),
    log("filter_decay", "Filter Decay", 1.0, 10000.0, 300.0, Unit::Ms),
    choice("lfo_shape", "LFO Shape", LFO_SHAPES, 0),
    log("lfo_rate", "LFO Rate", 0.05, 20.0, 4.0, Unit::Hz),
    lin("lfo_pitch", "LFO to Pitch", 0.0, 12.0, 0.0, Unit::Semitones),
    lin("lfo_filter", "LFO to Filter", 0.0, 48.0, 0.0, Unit::Semitones),
    lin("lfo_volume", "LFO to Volume", 0.0, 100.0, 0.0, Unit::Percent),
    lin("glide", "Glide", 0.0, 2000.0, 0.0, Unit::Ms),
    choice("voice_mode", "Voices", VOICE_MODES, 0),
    ParamInfo {
        id: "pan",
        name: "Pan",
        min: -100.0,
        max: 100.0,
        default: 0.0,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: &[],
        display: Some(pan_display),
        group: "",
    },
    lin("loop_xfade", "Loop Crossfade", 0.0, 50.0, 0.0, Unit::Percent),
];

pub static SAMPLER_INFO: PluginInfo = PluginInfo {
    id: "sampler",
    name: "Lift - Sampler",
    short_name: "Lift",
    category: Category::Instrument,
    params: &SAMPLER_PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

const VOICES: usize = 32;
/// Fastest playback step in sample frames per output frame (keeps hostile pitch bounded).
const MAX_STEP: f64 = 256.0;
/// Frames between updates of each voice's pitch and filter (the modulation's control rate).
const CONTROL: usize = 16;
/// Notes a mono sampler remembers being held, to return to when the newest is let go.
const HELD_STACK: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
enum LoopMode {
    Off,
    Forward,
    PingPong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FilterKind {
    Off,
    LowPass,
    HighPass,
    BandPass,
}

/// The filter envelope: a linear rise to full over the attack, then an exponential fall to
/// silence (-60 dB) over the decay. It ignores the key being let go.
#[derive(Debug, Clone, Copy)]
struct AdEnv {
    level: f32,
    rising: bool,
}

impl AdEnv {
    const fn new() -> Self {
        Self { level: 0.0, rising: false }
    }

    fn trigger(&mut self) {
        self.level = 0.0;
        self.rising = true;
    }

    #[inline]
    fn next(&mut self, attack: f32, decay: f32) -> f32 {
        if self.rising {
            self.level += attack;
            if self.level >= 1.0 {
                self.level = 1.0;
                self.rising = false;
            }
        } else {
            self.level = crate::util::flush(self.level * decay);
        }
        self.level
    }
}

/// A topology-preserving state-variable filter (one per voice and side).
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    #[inline]
    fn run(&mut self, x: f32, g: f32, k: f32, kind: FilterKind) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = crate::util::flush(2.0 * v1 - self.ic1);
        self.ic2 = crate::util::flush(2.0 * v2 - self.ic2);
        let y = match kind {
            FilterKind::Off => x,
            FilterKind::LowPass => v2,
            // Scaled by the damping so the band's peak stays at unity at any resonance.
            FilterKind::BandPass => k * v1,
            FilterKind::HighPass => x - k * v1 - v2,
        };
        if y.is_finite() {
            y
        } else {
            *self = Svf::default();
            0.0
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    note: u8,
    /// Key still down (gate mode only; one-shot voices are never held).
    held: bool,
    gain: f32,
    /// Read position in sample frames.
    pos: f64,
    step: f64,
    forward: bool,
    looping: bool,
    env: Adsr,
    /// Current pitch in semitones from the sample's own (gliding towards `target`).
    pitch: f64,
    target: f64,
    fenv: AdEnv,
    svf: [Svf; 2],
    /// Filter coefficients for the current control period.
    g: f32,
    k: f32,
}

impl Voice {
    const fn new() -> Self {
        Self {
            note: 0,
            held: false,
            gain: 0.0,
            pos: 0.0,
            step: 1.0,
            forward: true,
            looping: false,
            env: Adsr::new(),
            pitch: 0.0,
            target: 0.0,
            fenv: AdEnv::new(),
            svf: [Svf { ic1: 0.0, ic2: 0.0 }; 2],
            g: 0.0,
            k: 2.0,
        }
    }

    fn active(&self) -> bool {
        self.env.stage != Stage::Idle
    }

    fn stop(&mut self) {
        *self = Voice::new();
    }
}

/// Values derived from the parameters (refreshed on every change).
#[derive(Debug, Clone, Copy)]
struct Derived {
    root: f64,
    /// Transpose plus fine tune, in semitones.
    offset: f64,
    key_track: bool,
    keys: (u8, u8),
    vels: (u8, u8),
    sens: f32,
    one_shot: bool,
    /// Start and end as fractions of the sample (start <= end).
    span: (f64, f64),
    loop_mode: LoopMode,
    /// Loop start and end as fractions of the sample (start <= end).
    loop_span: (f64, f64),
    filter: FilterKind,
    cutoff: f32,
    /// Filter damping (2 = no resonance).
    k: f32,
    fenv_amount: f32,
    fenv_attack: f32,
    fenv_decay: f32,
    lfo_shape: usize,
    lfo_inc: f32,
    lfo_pitch: f64,
    lfo_filter: f32,
    lfo_volume: f32,
    /// One-pole glide coefficient (0 = no glide).
    glide: f64,
    mono: bool,
    /// Forward-loop crossfade as a fraction of the loop's length.
    xfade: f64,
}

/// A MIDI key or velocity parameter as a whole number in `lo..=127`.
fn whole(v: f32, lo: u8) -> u8 {
    if v.is_finite() { v.round().clamp(f32::from(lo), 127.0) as u8 } else { lo }
}

fn ordered(a: u8, b: u8) -> (u8, u8) {
    (a.min(b), a.max(b))
}

/// 4-point cubic (Catmull-Rom) interpolation of `x` at fractional frame `pos` (>= 0). Reads
/// outside the slice are silence.
#[inline]
fn hermite(x: &[f32], pos: f64) -> f32 {
    let i = pos as usize;
    let t = (pos - i as f64) as f32;
    let at = |k: usize| x.get(k).copied().unwrap_or(0.0);
    let x0 = at(i);
    if t == 0.0 {
        return if x0.is_finite() { x0 } else { 0.0 };
    }
    let xm1 = at(i.saturating_sub(1));
    let x1 = at(i.saturating_add(1));
    let x2 = at(i.saturating_add(2));
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    let y = ((c3 * t + c2) * t + c1) * t + x0;
    // A float file can hold NaN or infinities; they play as silence.
    if y.is_finite() { y } else { 0.0 }
}

/// Plays a sample across the keyboard.
pub struct Sampler {
    p: Params,
    d: Derived,
    sr: f32,
    ch: usize,
    sample: Option<Arc<dyn SampleSource>>,
    /// Frames in the sample (0 = nothing to play).
    len: usize,
    /// The sample's own rate.
    sample_sr: f32,
    voices: [Voice; VOICES],
    events: EventQueue,
    rates: AdsrRates,
    level: Smoother,
    pan: Smoother,
    /// LFO phase (0..1), its value, and the random shape's held value.
    lfo_phase: f32,
    lfo: f32,
    rng: Rng,
    random: f32,
    /// Frames until the voices' pitch and filter are next updated.
    control: usize,
    /// The pitch (semitones from the sample's) the last note started at, for glide.
    last_pitch: Option<f64>,
    /// Mono: the keys held, oldest first.
    held: [u8; HELD_STACK],
    held_len: usize,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&SAMPLER_PARAMS),
            d: Derived {
                root: 60.0,
                offset: 0.0,
                key_track: true,
                keys: (0, 127),
                vels: (1, 127),
                sens: 0.7,
                one_shot: false,
                span: (0.0, 1.0),
                loop_mode: LoopMode::Off,
                loop_span: (0.0, 1.0),
                filter: FilterKind::Off,
                cutoff: 2000.0,
                k: 2.0,
                fenv_amount: 0.0,
                fenv_attack: 1.0,
                fenv_decay: 0.0,
                lfo_shape: 0,
                lfo_inc: 0.0,
                lfo_pitch: 0.0,
                lfo_filter: 0.0,
                lfo_volume: 0.0,
                glide: 0.0,
                mono: false,
                xfade: 0.0,
            },
            sr: 48_000.0,
            ch: 0,
            sample: None,
            len: 0,
            sample_sr: 48_000.0,
            voices: [Voice::new(); VOICES],
            events: EventQueue::new(),
            rates: AdsrRates::default(),
            level: Smoother::new(1.0),
            pan: Smoother::new(0.0),
            lfo_phase: 0.0,
            lfo: 0.0,
            rng: Rng::new(0x5A3C_91E7),
            random: 0.0,
            control: 0,
            last_pitch: None,
            held: [0; HELD_STACK],
            held_len: 0,
        };
        s.update();
        s
    }

    fn update(&mut self) {
        let v = |i| self.p.v(i);
        let pct = |i| f64::from(v(i) / 100.0).clamp(0.0, 1.0);
        let (ls, le) = (pct(LOOP_START), pct(LOOP_END));
        let (st, en) = (pct(START), pct(END));
        let sr = self.sr;
        let exp60 = |ms: f32| (0.001f32.ln() / ms_to_samples(ms, sr).max(1.0)).exp();
        let finite = |x: f32, or: f32| if x.is_finite() { x } else { or };
        let was_mono = self.d.mono;
        self.d = Derived {
            root: f64::from(whole(v(ROOT), 0)),
            offset: f64::from(v(TRANSPOSE)) + f64::from(v(FINE)) / 100.0,
            key_track: self.p.on(KEY_TRACK),
            keys: ordered(whole(v(KEY_LOW), 0), whole(v(KEY_HIGH), 0)),
            vels: ordered(whole(v(VEL_LOW), 1), whole(v(VEL_HIGH), 1)),
            sens: (v(VEL_SENS) / 100.0).clamp(0.0, 1.0),
            one_shot: self.p.choice(PLAY_MODE) == 1,
            span: (st.min(en), st.max(en)),
            loop_mode: match self.p.choice(LOOP_MODE) {
                1 => LoopMode::Forward,
                2 => LoopMode::PingPong,
                _ => LoopMode::Off,
            },
            loop_span: (ls.min(le), ls.max(le)),
            filter: match self.p.choice(FILTER_TYPE) {
                1 => FilterKind::LowPass,
                2 => FilterKind::HighPass,
                3 => FilterKind::BandPass,
                _ => FilterKind::Off,
            },
            cutoff: finite(v(CUTOFF), 2000.0),
            // Resonance 0..100 % takes Q from 0.5 to about 25.
            k: 2.0 - 1.92 * (v(RESONANCE) / 100.0).clamp(0.0, 1.0),
            fenv_amount: finite(v(FILTER_ENV), 0.0),
            fenv_attack: 1.0 / ms_to_samples(v(FILTER_ATTACK), sr).max(1.0),
            fenv_decay: exp60(v(FILTER_DECAY)),
            lfo_shape: self.p.choice(LFO_SHAPE),
            lfo_inc: finite(v(LFO_RATE) / sr, 0.0).clamp(0.0, 0.5),
            lfo_pitch: f64::from(finite(v(LFO_PITCH), 0.0)),
            lfo_filter: finite(v(LFO_FILTER), 0.0),
            lfo_volume: (v(LFO_VOLUME) / 100.0).clamp(0.0, 1.0),
            // A third of the glide time is the time constant: the slide is all but done by then.
            glide: f64::from(time_coef(v(GLIDE) / 3.0, sr)),
            mono: self.p.choice(VOICE_MODE) == 1,
            xfade: f64::from(v(LOOP_XFADE) / 100.0).clamp(0.0, 0.5),
        };
        if was_mono != self.d.mono {
            self.held_len = 0;
        }
        self.rates = AdsrRates::new(v(ATTACK), v(DECAY), v(SUSTAIN) / 100.0, v(RELEASE), sr);
        self.level.set(db_to_gain(v(LEVEL)));
        self.pan.set((v(PAN) / 100.0).clamp(-1.0, 1.0));
    }

    /// The loop region in sample frames, when it is long enough to loop.
    fn loop_frames(&self) -> Option<(f64, f64)> {
        if self.d.loop_mode == LoopMode::Off {
            return None;
        }
        let len = self.len as f64;
        let (a, b) = (self.d.loop_span.0 * len, self.d.loop_span.1 * len);
        (b - a >= 1.0).then_some((a, b))
    }

    /// The frame a voice that is not looping stops at.
    fn end_frame(&self) -> f64 {
        self.d.span.1 * self.len as f64
    }

    /// A key's pitch in semitones from the sample's own.
    fn key_pitch(&self, note: u8) -> f64 {
        let d = &self.d;
        let keyed = if d.key_track { f64::from(note) - d.root } else { 0.0 };
        keyed + d.offset
    }

    /// Playback step for a pitch in semitones from the sample's own.
    fn step_for(&self, semis: f64) -> f64 {
        let step = f64::from(self.sample_sr) / f64::from(self.sr) * 2f64.powf(semis / 12.0);
        if step.is_finite() { step.clamp(0.0, MAX_STEP) } else { 0.0 }
    }

    fn start_note(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.stop_note(note);
            return;
        }
        let d = self.d;
        let (note, vel) = (note.min(127), vel.min(127));
        if self.len == 0 || note < d.keys.0 || note > d.keys.1 || vel < d.vels.0 || vel > d.vels.1 {
            return;
        }
        let pitch = self.key_pitch(note);
        let step = self.step_for(pitch);
        if step <= 0.0 {
            return;
        }
        let gain = (1.0 - d.sens) + d.sens * f32::from(vel) / 127.0;
        if d.mono {
            self.push_held(note);
            // Legato: a key pressed while another still sounds moves the pitch and keeps the
            // sample playing; one-shot playback always starts again.
            let voice = self.voices.first().copied().unwrap_or(Voice::new());
            if voice.active() && voice.held && !d.one_shot {
                let glide = d.glide > 0.0;
                if let Some(v) = self.voices.first_mut() {
                    v.note = note;
                    v.target = pitch;
                    if !glide {
                        v.pitch = pitch;
                        v.step = step;
                    }
                }
                self.last_pitch = Some(pitch);
                return;
            }
            for v in self.voices.iter_mut().skip(1) {
                v.stop();
            }
            self.begin(0, note, gain, pitch, step);
            return;
        }
        if !d.one_shot {
            // Retriggering a held key releases the earlier voice instead of stacking it.
            self.stop_note(note);
        }
        let idx = self
            .voices
            .iter()
            .position(|v| !v.active())
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| (a.held, a.env.level).partial_cmp(&(b.held, b.env.level)).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
            })
            .unwrap_or(0);
        self.begin(idx, note, gain, pitch, step);
    }

    /// Starts voice `idx` from the sample start (gliding from the last note's pitch).
    fn begin(&mut self, idx: usize, note: u8, gain: f32, pitch: f64, step: f64) {
        let d = self.d;
        let start = d.span.0 * self.len.saturating_sub(1) as f64;
        let looping = self.loop_frames().is_some_and(|(_, end)| start < end);
        let from = if d.glide > 0.0 { self.last_pitch.unwrap_or(pitch) } else { pitch };
        let step = if from == pitch { step } else { self.step_for(from) };
        if let Some(v) = self.voices.get_mut(idx) {
            *v = Voice { note, held: !d.one_shot, gain, pos: start, step, forward: true, looping, pitch: from, target: pitch, ..Voice::new() };
            v.env.gate_on();
            v.fenv.trigger();
        }
        self.last_pitch = Some(pitch);
        // Fresh coefficients before the voice's first frame.
        self.control = 0;
    }

    fn push_held(&mut self, note: u8) {
        self.remove_held(note);
        if self.held_len == HELD_STACK {
            self.held.copy_within(1.., 0);
            self.held_len -= 1;
        }
        if let Some(slot) = self.held.get_mut(self.held_len) {
            *slot = note;
            self.held_len += 1;
        }
    }

    fn remove_held(&mut self, note: u8) {
        let len = self.held_len.min(HELD_STACK);
        if let Some(i) = self.held.iter().take(len).position(|n| *n == note) {
            self.held.copy_within(i + 1..len, i);
            self.held_len = len - 1;
        }
    }

    fn stop_note(&mut self, note: u8) {
        if self.d.mono {
            self.remove_held(note);
            let back = self.held_len.checked_sub(1).and_then(|i| self.held.get(i)).copied();
            let sounding = self.voices.first().is_some_and(|v| v.held && v.note == note);
            if !sounding {
                return;
            }
            match back {
                // Letting go of the newest key returns to the one still held before it.
                Some(prev) => {
                    let pitch = self.key_pitch(prev);
                    let step = self.step_for(pitch);
                    let glide = self.d.glide > 0.0;
                    if let Some(v) = self.voices.first_mut() {
                        v.note = prev;
                        v.target = pitch;
                        if !glide {
                            v.pitch = pitch;
                            v.step = step;
                        }
                    }
                    self.last_pitch = Some(pitch);
                }
                None => {
                    if let Some(v) = self.voices.first_mut() {
                        v.held = false;
                        v.env.gate_off();
                    }
                }
            }
            return;
        }
        for v in self.voices.iter_mut().filter(|v| v.held && v.note == note) {
            v.held = false;
            v.env.gate_off();
        }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(n, v) => self.start_note(n, v),
            EventKind::Off(n) => self.stop_note(n),
            EventKind::AllOff => {
                self.held_len = 0;
                for v in &mut self.voices {
                    v.held = false;
                    v.env.gate_off();
                }
            }
            EventKind::Sustain(_) | EventKind::Bend(_) | EventKind::ResetControllers => {}
        }
    }

    /// Moves the LFO one frame on and returns its value (-1..1).
    #[inline]
    fn lfo_next(&mut self) -> f32 {
        let ph = self.lfo_phase;
        let y = match self.d.lfo_shape {
            1 => 1.0 - 4.0 * (ph - 0.5).abs(),
            2 => {
                if ph < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            3 => 2.0 * ph - 1.0,
            4 => self.random,
            _ => (TAU * ph).sin(),
        };
        let mut next = ph + self.d.lfo_inc;
        if next >= 1.0 {
            next -= 1.0;
            self.random = self.rng.bipolar();
        }
        self.lfo_phase = if next.is_finite() { next.clamp(0.0, 1.0) } else { 0.0 };
        self.lfo = if y.is_finite() { y } else { 0.0 };
        self.lfo
    }

    /// Glides each voice's pitch on by a control period and works out its step and filter.
    fn control_update(&mut self) {
        let d = self.d;
        // The glide coefficient is per frame; a control period is CONTROL frames.
        let glide = if d.glide > 0.0 { d.glide.powi(CONTROL as i32) } else { 0.0 };
        let base = f64::from(self.sample_sr) / f64::from(self.sr);
        let lfo = self.lfo;
        let nyquist = 0.45 * self.sr;
        for v in self.voices.iter_mut().filter(|v| v.active()) {
            v.pitch = v.target + (v.pitch - v.target) * glide;
            if (v.pitch - v.target).abs() < 1e-4 {
                v.pitch = v.target;
            }
            let semis = v.pitch + d.lfo_pitch * f64::from(lfo);
            let step = base * 2f64.powf(semis / 12.0);
            v.step = if step.is_finite() { step.clamp(0.0, MAX_STEP) } else { 0.0 };
            if d.filter != FilterKind::Off {
                let semis = d.fenv_amount * v.fenv.level + d.lfo_filter * lfo;
                let hz = (d.cutoff * (semis / 12.0).exp2()).clamp(20.0, nyquist);
                let g = (PI * hz / self.sr).tan();
                v.g = if g.is_finite() { g } else { 0.0 };
                v.k = d.k;
            }
        }
    }

    /// Adds every voice's output for frame `n` into `io` and advances the voices.
    #[inline]
    fn render_frame(&mut self, io: &mut [Vec<f32>], ch: usize, n: usize, level: f32, pan: (f32, f32)) {
        let Some(sample) = self.sample.as_deref() else { return };
        let nch = sample.num_channels();
        let end = self.end_frame();
        let mode = self.d.loop_mode;
        let region = self.loop_frames();
        let d = self.d;
        let trem = 1.0 - d.lfo_volume * (0.5 + 0.5 * self.lfo);
        // A forward loop's crossfade: the last `xf` frames before the loop end fade into the
        // frames the same distance before the loop start (no more than there is before it).
        let fade = match region {
            Some((ls, le)) if mode == LoopMode::Forward && d.xfade > 0.0 => {
                let xf = (d.xfade * (le - ls)).min(ls);
                (xf >= 2.0).then_some((le - ls, le - xf, xf))
            }
            _ => None,
        };
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            v.fenv.next(d.fenv_attack, d.fenv_decay);
            let a = v.env.next(&self.rates) * v.gain * level * trem;
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                let src = if nch == 1 { 0 } else { c };
                if let (Some(x), Some(data)) = (b.get_mut(n), sample.channel(src)) {
                    let mut y = hermite(data, v.pos);
                    if v.looping
                        && let Some((len, from, xf)) = fade
                        && v.pos >= from
                    {
                        // Equal power: the loop's end fades out as its lead-in fades up.
                        let t = (((v.pos - from) / xf) as f32).clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
                        y = y * t.cos() + hermite(data, v.pos - len) * t.sin();
                    }
                    if d.filter != FilterKind::Off
                        && let Some(f) = v.svf.get_mut(c.min(1))
                    {
                        y = f.run(y, v.g, v.k, d.filter);
                    }
                    let side = match c {
                        0 if ch >= 2 => pan.0,
                        1 => pan.1,
                        _ => 1.0,
                    };
                    *x += y * a * side;
                }
            }
            let r = if v.looping { region } else { None };
            advance(v, end, r, mode);
        }
    }
}

/// Moves a voice's read position one output frame on, looping or ending it at `end`.
#[inline]
fn advance(v: &mut Voice, end: f64, region: Option<(f64, f64)>, mode: LoopMode) {
    let was_forward = v.forward;
    v.pos = if was_forward { v.pos + v.step } else { v.pos - v.step };
    match region {
        Some((ls, le)) if mode == LoopMode::PingPong => {
            // How far past the edge it was heading for, measured from the edge it left. A step of
            // more than the loop's width crosses it more than once: fold the distance into the
            // loop (even crossings keep the direction, odd ones turn it round) instead of
            // reflecting once and dropping the rest of the travel.
            let width = le - ls;
            let dist = if was_forward { v.pos - ls } else { le - v.pos };
            if width > 0.0 && dist >= width && dist.is_finite() {
                let crossings = (dist / width).floor();
                let into = dist - crossings * width;
                let odd = crossings % 2.0 == 1.0;
                (v.pos, v.forward) = match (was_forward, odd) {
                    (true, false) => (ls + into, true),
                    (true, true) => (le - into, false),
                    (false, false) => (le - into, false),
                    (false, true) => (ls + into, true),
                };
            }
        }
        Some((ls, le)) => {
            if v.pos >= le {
                v.pos = ls + (v.pos - ls) % (le - ls);
            }
        }
        None => {
            if v.pos >= end || !v.pos.is_finite() {
                v.stop();
            }
        }
    }
    if !v.pos.is_finite() || v.pos < 0.0 {
        v.stop();
    }
}

impl Plugin for Sampler {
    param_plumbing!(SAMPLER_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.pan.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.stop();
        }
        self.events.clear();
        self.level.snap();
        self.pan.snap();
        self.lfo_phase = 0.0;
        self.control = 0;
        self.last_pitch = None;
        self.held_len = 0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        for b in io.iter_mut().take(ch) {
            b.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
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
            self.lfo_next();
            if self.control == 0 {
                self.control_update();
                self.control = CONTROL;
            }
            self.control -= 1;
            let level = self.level.next();
            // Balance: the centre leaves both sides as they are; turning one way fades the other.
            let p = self.pan.next();
            let pan = ((1.0 - p).min(1.0), (1.0 + p).min(1.0));
            self.render_frame(io, ch, n, level, pan);
        }
        // Events for an empty block take effect immediately.
        for e in evs.iter().take(count).skip(next) {
            self.apply(e.kind);
        }
    }

    fn tail_samples(&self) -> usize {
        let release = ms_to_samples(self.p.v(RELEASE), self.sr);
        // A one-shot keeps playing its sample after the note ends.
        let span = (self.d.span.1 - self.d.span.0) as f32;
        let rest = if self.d.one_shot { span * self.len as f32 * self.sr / self.sample_sr } else { 0.0 };
        let t = release + rest;
        if t.is_finite() { t.min(1.0e9) as usize } else { 0 }
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.events.push(offset, EventKind::On(note, velocity));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.events.push(offset, EventKind::Off(note));
    }

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
    }

    fn set_sample(&mut self, sample: Option<Arc<dyn SampleSource>>) -> bool {
        for v in &mut self.voices {
            v.stop();
        }
        self.held_len = 0;
        self.len = sample.as_deref().map_or(0, |s| if s.num_channels() == 0 { 0 } else { s.frames() });
        let rate = sample.as_deref().map_or(self.sr, |s| s.sample_rate());
        // An unusable rate plays at the host rate rather than not at all.
        self.sample_sr = if rate.is_finite() && rate > 0.0 { rate.min(1_536_000.0) } else { self.sr };
        self.sample = sample;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SampleBuffer;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn sampler(sample: SampleBuffer) -> Sampler {
        let mut s = Sampler::new();
        s.prepare(SR, 512, 2);
        assert!(s.set_sample(Some(Arc::new(sample))));
        s
    }

    fn mono(sample_rate: f32, data: Vec<f32>) -> SampleBuffer {
        SampleBuffer { sample_rate, channels: vec![data] }
    }

    fn sine(freq: f32, sr: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| 0.5 * (TAU * freq * i as f32 / sr).sin()).collect()
    }

    /// Renders `len` frames in 512-frame blocks; returns both channels.
    fn run(s: &mut Sampler, len: usize) -> [Vec<f32>; 2] {
        let mut out = [Vec::new(), Vec::new()];
        let mut buf = vec![vec![1.0f32; 512]; 2];
        let mut done = 0;
        while done < len {
            let n = 512.min(len - done);
            s.process(&mut buf, n);
            out[0].extend_from_slice(&buf[0][..n]);
            out[1].extend_from_slice(&buf[1][..n]);
            done += n;
        }
        out
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Frequency from upward zero crossings.
    fn freq(x: &[f32], sr: f32) -> f32 {
        let ups: Vec<usize> = x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
        let (Some(a), Some(b)) = (ups.first(), ups.last()) else { return 0.0 };
        (ups.len() - 1) as f32 * sr / (b - a) as f32
    }

    #[test]
    fn silent_without_a_sample_and_replaces_the_buffer() {
        let mut s = Sampler::new();
        s.prepare(SR, 512, 2);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 4800);
        assert_eq!(peak(&out[0]), 0.0);
        assert_eq!(peak(&out[1]), 0.0);
    }

    #[test]
    fn root_note_plays_the_sample_verbatim() {
        let data = sine(440.0, SR, 9600);
        let mut s = sampler(mono(SR, data.clone()));
        s.set_param("velocity", 0.0);
        s.note_on(0, 60, 100);
        let out = run(&mut s, 9600);
        // Past the 0.1-ms-ish attack the output is the sample itself, on both sides.
        for i in 100..9600 {
            assert!((out[0][i] - data[i]).abs() < 1e-6, "frame {i}: {} vs {}", out[0][i], data[i]);
        }
        assert_eq!(out[0], out[1], "a mono sample feeds both channels");
    }

    #[test]
    fn keys_transpose_from_the_root_note() {
        let mut s = sampler(mono(SR, sine(400.0, SR, 96_000)));
        s.note_on(0, 72, 127);
        let up = run(&mut s, 24_000);
        assert!((freq(&up[0][480..], SR) - 800.0).abs() < 2.0, "an octave up: {}", freq(&up[0][480..], SR));
        s.reset();
        s.note_on(0, 53, 127);
        let down = run(&mut s, 24_000);
        let want = 400.0 * 2f32.powf(-7.0 / 12.0);
        assert!((freq(&down[0][480..], SR) - want).abs() < 2.0);
        // Root note 48 makes key 60 play an octave up; transpose and fine tune add on top.
        s.reset();
        s.set_param("root_note", 48.0);
        s.set_param("transpose", -12.0);
        s.set_param("fine_tune", 100.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 24_000);
        let want = 400.0 * 2f32.powf(1.0 / 12.0);
        assert!((freq(&out[0][480..], SR) - want).abs() < 2.0, "{}", freq(&out[0][480..], SR));
    }

    #[test]
    fn key_tracking_off_plays_every_key_at_the_sample_pitch() {
        let mut s = sampler(mono(SR, sine(400.0, SR, 48_000)));
        s.set_param("key_track", 0.0);
        s.note_on(0, 90, 127);
        let out = run(&mut s, 24_000);
        assert!((freq(&out[0][480..], SR) - 400.0).abs() < 2.0);
    }

    #[test]
    fn converts_the_sample_rate() {
        // A 300 Hz tone recorded at 22.05 kHz still plays at 300 Hz on a 48 kHz host.
        let mut s = sampler(mono(22_050.0, sine(300.0, 22_050.0, 22_050)));
        s.note_on(0, 60, 127);
        let out = run(&mut s, 60_000);
        assert!((freq(&out[0][480..], SR) - 300.0).abs() < 1.0, "{}", freq(&out[0][480..], SR));
        // And it lasts as long as the recording: one second.
        assert!(peak(&out[0][46_000..47_500]) > 0.1);
        assert_eq!(peak(&out[0][48_200..]), 0.0, "the sample ended");
        // A 96 kHz recording is read faster, not cut short.
        let mut s = sampler(mono(96_000.0, sine(1000.0, 96_000.0, 96_000)));
        s.note_on(0, 60, 127);
        let out = run(&mut s, 48_000);
        assert!((freq(&out[0][480..], SR) - 1000.0).abs() < 2.0);
    }

    #[test]
    fn key_and_velocity_ranges_gate_notes() {
        let mut s = sampler(mono(SR, vec![0.5; 4800]));
        s.set_param("key_low", 48.0);
        s.set_param("key_high", 60.0);
        s.set_param("vel_low", 40.0);
        s.set_param("vel_high", 100.0);
        for (note, vel, sounds) in
            [(47, 80, false), (61, 80, false), (48, 80, true), (60, 80, true), (55, 39, false), (55, 101, false), (55, 40, true)]
        {
            s.reset();
            s.note_on(0, note, vel);
            let out = run(&mut s, 960);
            assert_eq!(peak(&out[0]) > 0.0, sounds, "note {note} velocity {vel}");
        }
        // Reversed ranges are read in order rather than muting everything.
        s.set_param("key_low", 60.0);
        s.set_param("key_high", 48.0);
        s.reset();
        s.note_on(0, 50, 80);
        assert!(peak(&run(&mut s, 960)[0]) > 0.0);
    }

    #[test]
    fn velocity_sensitivity_scales_level() {
        let level = |sens: f32, vel: u8| {
            let mut s = sampler(mono(SR, vec![0.5; 4800]));
            s.set_param("velocity", sens);
            s.note_on(0, 60, vel);
            peak(&run(&mut s, 2400)[0][1000..])
        };
        assert!((level(100.0, 127) - 0.5).abs() < 1e-4);
        assert!((level(100.0, 64) - 0.5 * 64.0 / 127.0).abs() < 1e-4);
        assert!((level(0.0, 1) - 0.5).abs() < 1e-4, "no sensitivity: every velocity is full level");
        assert!(level(70.0, 30) < level(70.0, 127));
    }

    #[test]
    fn adsr_shapes_the_note() {
        let mut s = sampler(mono(SR, vec![0.5; 96_000]));
        s.set_param("velocity", 0.0);
        s.set_param("attack", 100.0);
        s.set_param("decay", 100.0);
        s.set_param("sustain", 50.0);
        s.set_param("release", 100.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 24_000);
        // Linear attack: halfway up at 50 ms, full level at 100 ms, then decay to the sustain level.
        assert!((out[0][2400] - 0.25).abs() < 0.01, "attack midpoint {}", out[0][2400]);
        assert!(peak(&out[0][4700..4900]) > 0.49);
        assert!((out[0][20_000] - 0.25).abs() < 0.005, "sustain {}", out[0][20_000]);
        s.note_off(0, 60);
        let out = run(&mut s, 24_000);
        assert!(out[0][100] > 0.2, "release starts from the sustain level");
        assert!(peak(&out[0][5000..]) < 1e-3, "released after 100 ms");
        assert_eq!(peak(&out[0][12_000..]), 0.0, "the voice ended");
    }

    #[test]
    fn gate_mode_stops_on_note_off_and_one_shot_plays_through() {
        let mut s = sampler(mono(SR, vec![0.5; 24_000]));
        s.set_param("release", 1.0);
        s.note_on(0, 60, 127);
        s.note_off(1000, 60);
        let out = run(&mut s, 24_000);
        assert!(peak(&out[0][..900]) > 0.1);
        assert_eq!(peak(&out[0][2000..]), 0.0, "gate: released at the note-off");
        s.set_param("play_mode", 1.0);
        s.reset();
        s.note_on(0, 60, 127);
        s.note_off(1000, 60);
        let out = run(&mut s, 30_000);
        assert!(peak(&out[0][20_000..23_900]) > 0.1, "one shot: ignores the note-off");
        assert_eq!(peak(&out[0][24_100..]), 0.0, "and stops at the end of the sample");
        assert!(s.tail_samples() >= 24_000, "a one-shot's tail covers the sample");
        // Stopping the transport still silences one-shots.
        s.reset();
        s.note_on(0, 60, 127);
        run(&mut s, 512);
        s.all_notes_off();
        assert_eq!(peak(&run(&mut s, 4800)[0][2400..]), 0.0);
    }

    #[test]
    fn retriggering_a_held_key_releases_the_old_voice() {
        let mut s = sampler(mono(SR, vec![0.25; 48_000]));
        s.set_param("velocity", 0.0);
        s.set_param("release", 1.0);
        s.note_on(0, 60, 127);
        s.note_on(256, 60, 127);
        let out = run(&mut s, 4800);
        assert!(peak(&out[0][2400..]) < 0.26, "voices did not stack: {}", peak(&out[0][2400..]));
        // One-shots overlap.
        s.set_param("play_mode", 1.0);
        s.reset();
        s.note_on(0, 60, 127);
        s.note_on(256, 60, 127);
        let out = run(&mut s, 4800);
        assert!((peak(&out[0][2400..]) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn forward_loop_sustains_past_the_end() {
        let data = sine(480.0, SR, 4800);
        let mut s = sampler(mono(SR, data.clone()));
        s.set_param("velocity", 0.0);
        s.set_param("loop_mode", 1.0);
        s.set_param("loop_start", 50.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 48_000);
        assert!(peak(&out[0][40_000..]) > 0.4, "still sounding long after the 0.1 s sample");
        // After the first pass it repeats the loop region (2400 frames, whole cycles of 480 Hz).
        for i in 0..2400 {
            assert!((out[0][4800 + i] - data[2400 + i]).abs() < 1e-4, "frame {i}");
        }
        assert!((freq(&out[0][4800..], SR) - 480.0).abs() < 1.0);
        // Without the loop the note ends with the sample.
        s.set_param("loop_mode", 0.0);
        s.reset();
        s.note_on(0, 60, 127);
        assert_eq!(peak(&run(&mut s, 9600)[0][4900..]), 0.0);
    }

    #[test]
    fn ping_pong_loop_reverses_direction() {
        // A rising ramp, looped ping-pong over the whole sample, rises then falls.
        let data: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let mut s = sampler(mono(SR, data));
        s.set_param("velocity", 0.0);
        s.set_param("loop_mode", 2.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 3000);
        assert!(out[0][900] > out[0][500], "rising");
        assert!(out[0][1500] < out[0][1100], "falling after the loop end");
        assert!(out[0][2500] > out[0][2100], "rising again after the loop start");
        assert!(out[0].iter().all(|x| x.is_finite() && *x <= 1.0));
    }

    /// Where a ping-pong voice is after `n` steps, by walking a triangle wave over the loop.
    fn triangle(start: f64, forward: bool, step: f64, n: u32, ls: f64, le: f64) -> (f64, bool) {
        let width = le - ls;
        // Unfold the walk onto one line, then fold it back: phase 0..width rising, width..2 width falling.
        let signed = if forward { start - ls } else { (le - start) + width };
        let t = (signed + f64::from(n) * step).rem_euclid(2.0 * width);
        if t < width { (ls + t, true) } else { (le - (t - width), false) }
    }

    #[test]
    fn a_ping_pong_step_wider_than_the_loop_keeps_the_phase() {
        let (ls, le) = (100.0, 110.0);
        // Steps below, equal to and above the loop's width, with fractions, both ways round.
        for step in [0.37, 3.0, 9.99, 10.0, 10.5, 23.7, 25.0, 31.3, 99.9] {
            for forward in [true, false] {
                let mut v = Voice::new();
                v.pos = if forward { 102.5 } else { 107.25 };
                v.forward = forward;
                v.step = step;
                let start = v.pos;
                for n in 1..=40u32 {
                    advance(&mut v, 1000.0, Some((ls, le)), LoopMode::PingPong);
                    let (want, want_forward) = triangle(start, forward, step, n, ls, le);
                    assert!(v.pos >= ls - 1e-9 && v.pos <= le + 1e-9, "step {step} n {n}: {} left the loop", v.pos);
                    // At an exact end the direction is where the position is heading next.
                    let at_end = (want - ls).abs() < 1e-6 || (want - le).abs() < 1e-6;
                    assert!((v.pos - want).abs() < 1e-6, "step {step} {forward} n {n}: at {} want {want}", v.pos);
                    assert!(at_end || v.forward == want_forward, "step {step} {forward} n {n}: direction");
                }
            }
        }
    }

    #[test]
    fn a_ping_pong_step_below_the_loop_width_moves_as_before() {
        let mut v = Voice::new();
        v.pos = 108.0;
        v.step = 5.0;
        advance(&mut v, 1000.0, Some((100.0, 110.0)), LoopMode::PingPong);
        assert_eq!((v.pos, v.forward), (107.0, false), "one reflection off the end");
        advance(&mut v, 1000.0, Some((100.0, 110.0)), LoopMode::PingPong);
        assert_eq!((v.pos, v.forward), (102.0, false));
        advance(&mut v, 1000.0, Some((100.0, 110.0)), LoopMode::PingPong);
        assert_eq!((v.pos, v.forward), (103.0, true), "and one off the start");
    }

    #[test]
    fn a_loop_too_short_to_loop_plays_once() {
        let mut s = sampler(mono(SR, vec![0.5; 1000]));
        s.set_param("loop_mode", 1.0);
        s.set_param("loop_start", 40.0);
        s.set_param("loop_end", 40.0);
        s.note_on(0, 60, 127);
        assert_eq!(peak(&run(&mut s, 4800)[0][1100..]), 0.0);
    }

    #[test]
    fn sample_start_skips_into_the_sample() {
        let data: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let mut s = sampler(mono(SR, data));
        s.set_param("velocity", 0.0);
        s.set_param("attack", 0.1);
        s.set_param("start", 50.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 2000);
        assert!((out[0][10] - 0.509).abs() < 0.002, "{}", out[0][10]);
        assert_eq!(peak(&out[0][520..]), 0.0, "half the sample left");
    }

    #[test]
    fn stereo_samples_keep_their_sides() {
        let sample = SampleBuffer { sample_rate: SR, channels: vec![vec![0.5; 4800], vec![-0.25; 4800]] };
        let mut s = sampler(sample);
        s.set_param("velocity", 0.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 2400);
        assert!((out[0][1000] - 0.5).abs() < 1e-6);
        assert!((out[1][1000] + 0.25).abs() < 1e-6);
    }

    #[test]
    fn note_offsets_are_sample_accurate() {
        let mut s = sampler(mono(SR, vec![0.5; 4800]));
        s.note_on(300, 60, 127);
        let out = run(&mut s, 512);
        assert!(out[0][..300].iter().all(|&v| v == 0.0));
        assert!(out[0][301..].iter().any(|&v| v != 0.0));
    }

    #[test]
    fn many_notes_steal_voices_and_stay_bounded() {
        let mut s = sampler(mono(SR, sine(220.0, SR, 48_000)));
        s.set_param("level", 12.0);
        for n in 0..100u8 {
            s.note_on(usize::from(n), n, 127);
        }
        let out = run(&mut s, 9600);
        assert!(out[0].iter().all(|v| v.is_finite()));
        assert!(s.voices.iter().filter(|v| v.active()).count() <= VOICES);
    }

    #[test]
    fn a_new_sample_stops_the_old_voices() {
        let mut s = sampler(mono(SR, vec![0.5; 48_000]));
        s.note_on(0, 60, 127);
        run(&mut s, 512);
        assert!(s.set_sample(Some(Arc::new(mono(SR, vec![0.1; 10])))));
        assert_eq!(peak(&run(&mut s, 512)[0]), 0.0);
        assert!(s.set_sample(None));
        s.note_on(0, 60, 127);
        assert_eq!(peak(&run(&mut s, 512)[0]), 0.0);
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn sample_end_stops_playback_early() {
        let data: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let mut s = sampler(mono(SR, data));
        s.set_param("velocity", 0.0);
        s.set_param("end", 50.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 2000);
        assert!(out[0][400] > 0.3, "plays up to the end point");
        assert_eq!(peak(&out[0][510..]), 0.0, "stops at half way");
    }

    #[test]
    fn crossed_start_and_end_play_the_span_between_them() {
        let data: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let mut s = sampler(mono(SR, data));
        s.set_param("velocity", 0.0);
        s.set_param("attack", 0.1);
        s.set_param("start", 60.0);
        s.set_param("end", 40.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 2000);
        assert!((out[0][10] - 0.41).abs() < 0.005, "starts at 40 %: {}", out[0][10]);
        assert_eq!(peak(&out[0][210..]), 0.0, "ends at 60 %");
    }

    #[test]
    fn a_one_shot_tail_covers_only_the_span() {
        let mut s = sampler(mono(SR, vec![0.5; 48_000]));
        s.set_param("play_mode", 1.0);
        s.set_param("release", 1.0);
        let whole = s.tail_samples();
        s.set_param("end", 25.0);
        let quarter = s.tail_samples();
        assert!(quarter < whole / 3 && quarter > whole / 5, "{quarter} of {whole}");
    }

    #[test]
    fn the_filter_shapes_the_sound_and_off_leaves_it_alone() {
        let high = sine(6000.0, SR, 48_000);
        let low = sine(100.0, SR, 48_000);
        let level = |data: &Vec<f32>, kind: f32, cutoff: f32, resonance: f32| {
            let mut s = sampler(mono(SR, data.clone()));
            s.set_param("velocity", 0.0);
            s.set_param("filter_type", kind);
            s.set_param("cutoff", cutoff);
            s.set_param("resonance", resonance);
            s.note_on(0, 60, 127);
            rms(&run(&mut s, 9600)[0][2400..])
        };
        let full = 0.5 / 2f32.sqrt();
        let open = level(&high, 0.0, 200.0, 0.0);
        assert!((open - full).abs() < 0.01, "filter off plays it as it is: {open}");
        assert!(level(&high, 1.0, 300.0, 0.0) < open * 0.02, "low-pass removes the highs");
        assert!(level(&low, 1.0, 300.0, 0.0) > 0.3, "and keeps the lows");
        assert!(level(&low, 2.0, 3000.0, 0.0) < 0.02, "high-pass removes the lows");
        assert!(level(&high, 2.0, 3000.0, 0.0) > 0.25, "and keeps the highs");
        let mid = sine(1000.0, SR, 48_000);
        let band = level(&mid, 3.0, 1000.0, 60.0);
        assert!((band - full).abs() < 0.02, "band-pass keeps its band at unity: {band}");
        assert!(level(&low, 3.0, 1000.0, 60.0) < 0.05 && level(&high, 3.0, 1000.0, 60.0) < 0.07, "and cuts either side");
        assert!(level(&mid, 1.0, 1000.0, 90.0) > full * 3.0, "resonance rings at the cutoff");
    }

    #[test]
    fn the_filter_envelope_opens_then_closes() {
        let mut s = sampler(mono(SR, sine(4000.0, SR, 48_000)));
        s.set_param("velocity", 0.0);
        s.set_param("filter_type", 1.0);
        s.set_param("cutoff", 100.0);
        s.set_param("resonance", 0.0);
        s.set_param("filter_env", 72.0);
        s.set_param("filter_attack", 1.0);
        s.set_param("filter_decay", 1000.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 24_000);
        let early = rms(&out[0][96..960]);
        let late = rms(&out[0][19_200..24_000]);
        assert!(early > 0.2, "open at the start: {early}");
        assert!(late < early * 0.05, "closed after the decay: {late} vs {early}");
    }

    #[test]
    fn the_lfo_wobbles_pitch_and_volume() {
        let mut s = sampler(mono(SR, sine(1000.0, SR, 96_000)));
        s.set_param("velocity", 0.0);
        s.set_param("lfo_shape", 2.0);
        s.set_param("lfo_rate", 1.0);
        s.set_param("lfo_pitch", 12.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 48_000);
        let up = freq(&out[0][2000..20_000], SR);
        let down = freq(&out[0][26_000..46_000], SR);
        assert!((up / 2000.0 - 1.0).abs() < 0.02, "an octave up on the square's top: {up}");
        assert!((down / 500.0 - 1.0).abs() < 0.02, "an octave down on its bottom: {down}");

        let mut s = sampler(mono(SR, vec![0.5; 96_000]));
        s.set_param("velocity", 0.0);
        s.set_param("lfo_shape", 2.0);
        s.set_param("lfo_rate", 1.0);
        s.set_param("lfo_volume", 100.0);
        s.note_on(0, 60, 127);
        let out = run(&mut s, 48_000);
        assert!(peak(&out[0][2000..20_000]) < 1e-6, "full depth silences the top half");
        assert!((out[0][30_000] - 0.5).abs() < 1e-3, "and leaves the bottom half: {}", out[0][30_000]);
    }

    #[test]
    fn glide_slides_from_the_last_note() {
        let mut s = sampler(mono(SR, sine(500.0, SR, 96_000)));
        s.set_param("velocity", 0.0);
        s.set_param("glide", 300.0);
        s.note_on(0, 60, 127);
        run(&mut s, 4800);
        s.note_off(0, 60);
        s.set_param("release", 1.0);
        run(&mut s, 4800);
        s.note_on(0, 72, 127);
        let out = run(&mut s, 48_000);
        let start = freq(&out[0][0..1200], SR);
        let end = freq(&out[0][36_000..48_000], SR);
        assert!(start < 650.0, "starts near the last note: {start}");
        assert!((end / 1000.0 - 1.0).abs() < 0.01, "arrives an octave up: {end}");
    }

    #[test]
    fn mono_plays_legato_and_returns_to_the_held_key() {
        let mut s = sampler(mono(SR, sine(500.0, SR, 192_000)));
        s.set_param("velocity", 0.0);
        s.set_param("voice_mode", 1.0);
        s.set_param("release", 1.0);
        s.note_on(0, 60, 127);
        run(&mut s, 4800);
        s.note_on(0, 72, 127);
        let out = run(&mut s, 9600);
        assert_eq!(s.voices.iter().filter(|v| v.active()).count(), 1, "one voice");
        assert!((freq(&out[0][1000..9600], SR) / 1000.0 - 1.0).abs() < 0.01, "the new key's pitch");
        assert!(rms(&out[0][..480]) > 0.3, "no gap: the sample keeps playing");
        s.note_off(0, 72);
        let out = run(&mut s, 9600);
        assert!((freq(&out[0][1000..9600], SR) / 500.0 - 1.0).abs() < 0.01, "back to the key still held");
        s.note_off(0, 60);
        let out = run(&mut s, 4800);
        assert!(peak(&out[0][960..]) < 1e-3, "released when no key is held");
    }

    #[test]
    fn pan_turns_down_the_other_side() {
        let level = |pan: f32| {
            let mut s = sampler(mono(SR, vec![0.5; 9600]));
            s.set_param("velocity", 0.0);
            s.set_param("pan", pan);
            s.reset();
            s.note_on(0, 60, 127);
            let out = run(&mut s, 4800);
            (out[0][2400], out[1][2400])
        };
        assert_eq!(level(0.0), (0.5, 0.5), "centre leaves both sides");
        let (l, r) = level(-100.0);
        assert!((l - 0.5).abs() < 1e-6 && r.abs() < 1e-6, "hard left: {l} {r}");
        let (l, r) = level(50.0);
        assert!((l - 0.25).abs() < 1e-3 && (r - 0.5).abs() < 1e-6, "half right: {l} {r}");
    }

    #[test]
    fn a_loop_crossfade_smooths_the_seam() {
        // A ramp 0..1 looped over its second half jumps from 1 back to 0.5 at the seam; with a
        // crossfade the end fades into the audio before the loop start, so there is no jump.
        let data: Vec<f32> = (0..2000).map(|i| i as f32 / 2000.0).collect();
        let jump = |xfade: f32| {
            let mut s = sampler(mono(SR, data.clone()));
            s.set_param("velocity", 0.0);
            s.set_param("loop_mode", 1.0);
            s.set_param("loop_start", 50.0);
            s.set_param("loop_end", 100.0);
            s.set_param("loop_xfade", xfade);
            s.note_on(0, 60, 127);
            let out = run(&mut s, 6000);
            out[0][100..5900].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max)
        };
        assert!(jump(0.0) > 0.4, "a hard seam: {}", jump(0.0));
        assert!(jump(25.0) < 0.01, "crossfaded: {}", jump(25.0));
    }

    #[test]
    fn hostile_samples_and_params_never_panic() {
        let samples = [
            SampleBuffer { sample_rate: f32::NAN, channels: vec![vec![0.5; 100]] },
            SampleBuffer { sample_rate: 0.0, channels: vec![vec![0.5; 100]] },
            SampleBuffer { sample_rate: f32::INFINITY, channels: vec![vec![0.5; 100]] },
            SampleBuffer { sample_rate: 1.0e-30, channels: vec![vec![0.5; 100]] },
            SampleBuffer { sample_rate: SR, channels: vec![] },
            SampleBuffer { sample_rate: SR, channels: vec![vec![]] },
            SampleBuffer { sample_rate: SR, channels: vec![vec![0.5; 100], vec![0.5; 3]] },
            SampleBuffer { sample_rate: SR, channels: vec![vec![0.5; 1]] },
            SampleBuffer { sample_rate: SR, channels: vec![vec![f32::NAN; 100]; 9] },
        ];
        let params: Vec<(&str, f32)> = SAMPLER_PARAMS.iter().flat_map(|p| [(p.id, f32::NAN), (p.id, f32::INFINITY), (p.id, -1.0e30)]).collect();
        for sample in samples {
            let mut s = Sampler::new();
            s.prepare(f32::NAN, 0, 99);
            s.set_sample(Some(Arc::new(sample)));
            for (id, v) in &params {
                assert!(s.set_param(id, *v));
                for n in [0u8, 60, 127, 255] {
                    s.note_on(0, n, 255);
                    s.note_on(7, n, 1);
                }
                let mut io = vec![vec![0.0f32; 64]; 3];
                s.process(&mut io, 1000);
                assert!(io.iter().flatten().all(|v| v.is_finite()), "{id} = {v}");
                s.process(&mut [], 64);
                let _ = s.tail_samples();
            }
        }
        // Loops with every mode at the extremes of pitch.
        let mut s = sampler(mono(SR, sine(100.0, SR, 1000)));
        for mode in [0.0, 1.0, 2.0] {
            s.set_param("loop_mode", mode);
            for (t, root) in [(48.0, 0.0), (-48.0, 127.0)] {
                s.set_param("transpose", t);
                s.set_param("root_note", root);
                s.note_on(0, 127, 127);
                s.note_on(0, 0, 127);
                let out = run(&mut s, 4096);
                assert!(out[0].iter().all(|v| v.is_finite() && v.abs() < 4.0), "mode {mode} transpose {t}");
            }
        }
    }
}
