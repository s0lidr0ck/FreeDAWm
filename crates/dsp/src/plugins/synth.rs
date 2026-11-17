//! Subtractive Synth: 16-voice polyphonic, two PolyBLEP oscillators with a sub oscillator (a
//! square an octave below oscillator 1) and noise, a resonant state-variable filter (TPT form;
//! 12 or 24 dB low-pass, high-pass, band-pass or notch) with its own ADSR and key tracking, amp
//! ADSR, an LFO (sine, triangle, square or saw) to pitch, cutoff and level, poly, mono or legato
//! play, optional glide. The parameters past Mod Wheel Vibrato came later and default to the
//! sound the synth had before them (12 dB low-pass, LFO amounts, sub, noise and key tracking at
//! 0, poly), so sessions saved before sound as they did.

use crate::osc::{Osc, Waveform};
use crate::params::{Params, choice, lin, log, param_plumbing};
use crate::util::{Smoother, frames_in, ms_to_samples, note_hz, sane_channels, sane_sr, sort_events};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

pub mod presets;

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

/// The vibrato the mod wheel brings in (the synth's; the sampler uses its own LFO), Hz.
const VIBRATO_HZ: f32 = 5.5;

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

const FILTERS: &[&str] = &["Low-pass 12 dB", "Low-pass 24 dB", "High-pass", "Band-pass", "Notch"];
const LFO_WAVES: &[&str] = &["Sine", "Triangle", "Square", "Saw"];
const MODES: &[&str] = &["Poly", "Mono", "Legato"];

static SYNTH_PARAMS: [ParamInfo; 31] = [
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
];
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
    name: "Subtractive Synth",
    short_name: "SubSynth",
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
const SILENT: f32 = 1.0e-5;

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

/// Topology-preserving-transform state-variable low-pass.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// One sample through the filter as `kind` ([`FILTERS`]: 0 and 1 low-pass, 2 high-pass, 3
    /// band-pass, 4 notch).
    #[inline]
    fn run(&mut self, x: f32, g: f32, k: f32, kind: usize) -> f32 {
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
    noise: u32,
    age: u64,
    held: bool,
    /// Released while the sustain pedal was down.
    sustained: bool,
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
            age: 0,
            held: false,
            sustained: false,
        }
    }

    fn active(&self) -> bool {
        self.amp.stage != Stage::Idle
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
        };
        s.update();
        s
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
        let idx = self
            .voices
            .iter()
            .position(|v| v.active() && v.note == note)
            .or_else(|| self.voices.iter().position(|v| !v.active()))
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        let ka = (a.held, a.amp.level);
                        let kb = (b.held, b.amp.level);
                        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
            })
            .unwrap_or(0);
        let sens = self.p.v(17) / 100.0;
        let glide = self.glide_coef > 0.0;
        let start = if glide { self.last_freq.unwrap_or(freq) } else { freq };
        let counter = self.counter;
        if let Some(v) = self.voices.get_mut(idx) {
            let was_active = v.active();
            v.note = note;
            v.vel = (1.0 - sens) + sens * f32::from(vel.min(127)) / 127.0;
            v.target = freq;
            v.freq = if was_active && glide { v.freq } else { start };
            if !was_active {
                v.osc1.reset();
                v.osc2.phase = 0.37;
                v.svf = Svf::default();
                v.svf2 = Svf::default();
            }
            v.amp.gate_on();
            v.filt.gate_on();
            v.age = counter;
            v.held = true;
            v.sustained = false;
        }
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
            }
            if fresh {
                v.vel = (1.0 - sens) + sens * f32::from(vel.min(127)) / 127.0;
            }
            if !(legato && sounding) {
                v.amp.gate_on();
                v.filt.gate_on();
            }
            v.age = counter;
            v.held = true;
            v.sustained = false;
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

    #[inline]
    fn render_sample(&mut self) -> f32 {
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
        self.lfo_phase = (self.lfo_phase + self.p.v(LFO_RATE) / sr).fract();
        let cents = cents + self.p.v(LFO_PITCH) * lfo;
        let lfo_oct = self.p.v(LFO_CUTOFF) / 100.0 * 4.0 * lfo;
        let lfo_amp = 1.0 - self.p.v(LFO_AMP) / 100.0 * 0.5 * (1.0 - lfo);
        let kind = self.p.choice(FILTER_TYPE);
        let key_track = self.p.v(KEY_TRACK) / 100.0;
        let sub_level = self.p.v(SUB_LEVEL) / 100.0;
        let noise_level = self.p.v(NOISE_LEVEL) / 100.0;
        let bend = if cents == 0.0 { 1.0 } else { 2f32.powf(cents / 1200.0) };
        let mut out = 0.0;
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
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
            let mut y = v.svf.run(raw, g, k, kind);
            if kind == 1 {
                y = v.svf2.run(y, g, k, 1);
            }
            out += y * a * v.vel * lfo_amp;
        }
        out * 0.3
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

    fn reset(&mut self) {
        for (i, v) in self.voices.iter_mut().enumerate() {
            *v = Voice::new(0x1000 + i as u32 * 31);
        }
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
            let y = self.render_sample() * self.level.next();
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x = y;
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
        let [status, d1, d2] = msg;
        let kind = match (status & 0xF0, d1) {
            (0xB0, 64) => EventKind::Sustain(d2 >= 64),
            (0xB0, 120 | 123) => EventKind::AllOff,
            (0xB0, 121) => EventKind::ResetControllers,
            (0xB0, 1) => EventKind::Mod(f32::from(d2 & 0x7F) / 127.0),
            (0xE0, _) => EventKind::Bend(bend_of(d1, d2)),
            _ => return,
        };
        self.events.push(offset, kind);
    }

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
