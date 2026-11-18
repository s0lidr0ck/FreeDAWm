//! Wavetable Synth: 16 voices, each a band-limited wavetable oscillator (thirteen banks of 32
//! frames, every one made in code: see [`tables`]) with a sub oscillator and noise, unison of up
//! to seven detuned copies spread across the stereo field, the Subtractive Synth's filter (12 or
//! 24 dB low-pass, high-pass, band-pass, notch) with key tracking, an amp envelope, a second
//! envelope to the table position and the cutoff, and an LFO to pitch, cutoff, position and
//! level. The table position also follows velocity and the mod wheel. Pitch bend, the mod wheel's
//! vibrato, the sustain pedal, glide and voice stealing work as on the Subtractive Synth, whose
//! envelopes, filter, event queue, unison spread and voice choice it shares.

use super::synth::{
    Adsr, AdsrRates, EventKind, EventQueue, FILTERS, LFO_WAVES, MAX_EVENTS, MAX_UNISON, SILENT, Stage, Svf, UNISON, VIBRATO_HZ, event_of, lfo_shape,
    pick_voice, unison_copies,
};
use crate::osc::{Osc, Waveform};
use crate::params::{Params, choice, lin, log, param_plumbing};
use crate::util::{Smoother, frames_in, ms_to_samples, note_hz, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

pub mod presets;
#[cfg(test)]
mod reference;

pub mod tables;
pub mod user;

static WT_PARAMS: [ParamInfo; 36] = [
    choice("table", "Table", tables::BANKS, 0),
    lin("position", "Position", 0.0, 100.0, 0.0, Unit::Percent),
    // What moves the position, each as a share of the whole table.
    lin("env2_position", "Env 2 to Position", -100.0, 100.0, 0.0, Unit::Percent),
    lin("lfo_position", "LFO to Position", 0.0, 100.0, 0.0, Unit::Percent),
    lin("velocity_position", "Velocity to Position", -100.0, 100.0, 0.0, Unit::Percent),
    lin("wheel_position", "Mod Wheel to Position", -100.0, 100.0, 0.0, Unit::Percent),
    lin("osc_pitch", "Pitch", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("osc_fine", "Fine", -100.0, 100.0, 0.0, Unit::Cents),
    lin("sub_level", "Sub Osc", 0.0, 100.0, 0.0, Unit::Percent),
    lin("noise_level", "Noise", 0.0, 100.0, 0.0, Unit::Percent),
    choice("unison", "Unison", UNISON, 0),
    lin("unison_detune", "Unison Detune", 0.0, 100.0, 20.0, Unit::Cents),
    lin("unison_spread", "Unison Spread", 0.0, 100.0, 50.0, Unit::Percent),
    choice("filter_type", "Filter Type", FILTERS, 0),
    log("cutoff", "Cutoff", 20.0, 20000.0, 12000.0, Unit::Hz),
    lin("resonance", "Resonance", 0.0, 100.0, 10.0, Unit::Percent),
    lin("env2_cutoff", "Env 2 to Cutoff", -100.0, 100.0, 0.0, Unit::Percent),
    lin("key_track", "Key Tracking", 0.0, 100.0, 0.0, Unit::Percent),
    log("amp_attack", "Amp Attack", 0.5, 5000.0, 5.0, Unit::Ms),
    log("amp_decay", "Amp Decay", 1.0, 5000.0, 300.0, Unit::Ms),
    lin("amp_sustain", "Amp Sustain", 0.0, 100.0, 80.0, Unit::Percent),
    log("amp_release", "Amp Release", 1.0, 10000.0, 300.0, Unit::Ms),
    log("env2_attack", "Env 2 Attack", 0.5, 5000.0, 5.0, Unit::Ms),
    log("env2_decay", "Env 2 Decay", 1.0, 5000.0, 600.0, Unit::Ms),
    lin("env2_sustain", "Env 2 Sustain", 0.0, 100.0, 0.0, Unit::Percent),
    log("env2_release", "Env 2 Release", 1.0, 10000.0, 300.0, Unit::Ms),
    choice("lfo_wave", "LFO Wave", LFO_WAVES, 0),
    log("lfo_rate", "LFO Rate", 0.05, 30.0, 2.0, Unit::Hz),
    lin("lfo_pitch", "LFO to Pitch", 0.0, 1200.0, 0.0, Unit::Cents),
    lin("lfo_cutoff", "LFO to Cutoff", 0.0, 100.0, 0.0, Unit::Percent),
    lin("lfo_amp", "LFO to Level", 0.0, 100.0, 0.0, Unit::Percent),
    lin("glide", "Glide", 0.0, 1000.0, 0.0, Unit::Ms),
    lin("velocity", "Velocity Sens", 0.0, 100.0, 70.0, Unit::Percent),
    lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
    lin("bend_range", "Bend Range", 0.0, 24.0, 2.0, Unit::Semitones),
    lin("mod_wheel", "Mod Wheel Vibrato", 0.0, 100.0, 0.0, Unit::Cents),
];

const TABLE: usize = 0;
const POSITION: usize = 1;
const ENV2_POS: usize = 2;
const LFO_POS: usize = 3;
const VEL_POS: usize = 4;
const WHEEL_POS: usize = 5;
const PITCH: usize = 6;
const FINE: usize = 7;
const SUB_LEVEL: usize = 8;
const NOISE_LEVEL: usize = 9;
const UNISON_P: usize = 10;
const UNISON_DETUNE: usize = 11;
const UNISON_SPREAD: usize = 12;
const FILTER_TYPE: usize = 13;
const CUTOFF: usize = 14;
const RESONANCE: usize = 15;
const ENV2_CUTOFF: usize = 16;
const KEY_TRACK: usize = 17;
const AMP_A: usize = 18;
const ENV2_A: usize = 22;
const LFO_WAVE: usize = 26;
const LFO_RATE: usize = 27;
const LFO_PITCH: usize = 28;
const LFO_CUTOFF: usize = 29;
const LFO_AMP: usize = 30;
const GLIDE: usize = 31;
const VEL_SENS: usize = 32;
const LEVEL: usize = 33;
const BEND_RANGE: usize = 34;
const MOD_WHEEL: usize = 35;

pub static WAVETABLE_INFO: PluginInfo = PluginInfo {
    id: "wavetable_synth",
    name: "Wavetable Synth",
    short_name: "WTSynth",
    category: Category::Instrument,
    params: &WT_PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

const VOICES: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Voice {
    note: u8,
    /// The level velocity gives (after Velocity Sens) and the raw velocity (0..1).
    vel: f32,
    vel_raw: f32,
    freq: f32,
    target: f32,
    /// Each unison copy's phase (0..1).
    phases: [f32; MAX_UNISON],
    amp: Adsr,
    env2: Adsr,
    svf: Svf,
    svf2: Svf,
    svf_r: Svf,
    svf2_r: Svf,
    sub: Osc,
    noise: u32,
    age: u64,
    held: bool,
    sustained: bool,
}

impl Voice {
    fn new(seed: u32) -> Self {
        Self {
            note: 0,
            vel: 0.0,
            vel_raw: 0.0,
            freq: 440.0,
            target: 440.0,
            phases: [0.0; MAX_UNISON],
            amp: Adsr::new(),
            env2: Adsr::new(),
            svf: Svf::default(),
            svf2: Svf::default(),
            svf_r: Svf::default(),
            svf2_r: Svf::default(),
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

    /// A fresh start: copy 0 from phase 0, the others spread (so a stack does not start as one
    /// wave), the filters emptied.
    fn restart(&mut self) {
        for (i, p) in self.phases.iter_mut().enumerate() {
            *p = (i as f32 * 0.618_034).fract();
        }
        self.sub.reset();
        for f in [&mut self.svf, &mut self.svf2, &mut self.svf_r, &mut self.svf2_r] {
            *f = Svf::default();
        }
    }
}

/// Polyphonic wavetable synthesizer.
pub struct WavetableSynth {
    p: Params,
    t: &'static tables::Tables,
    sr: f32,
    ch: usize,
    voices: [Voice; VOICES],
    events: EventQueue,
    amp_rates: AdsrRates,
    env2_rates: AdsrRates,
    glide_coef: f32,
    last_freq: Option<f32>,
    counter: u64,
    level: Smoother,
    cutoff: Smoother,
    position: Smoother,
    damping: Smoother,
    sustain: bool,
    bend: f32,
    wheel: f32,
    vib_phase: f32,
    lfo_phase: f32,
    /// A user's own table, playing instead of the built-in bank while loaded (see [`user`]).
    user: user::Slot,
}

impl Default for WavetableSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl WavetableSynth {
    /// A new synth (builds the shared tables the first time one is made).
    pub fn new() -> Self {
        let mut voices = [Voice::new(1); VOICES];
        for (i, v) in voices.iter_mut().enumerate() {
            *v = Voice::new(0x2000 + i as u32 * 37);
        }
        let mut s = Self {
            p: Params::new(&WT_PARAMS),
            t: tables::tables(),
            sr: 48_000.0,
            ch: 0,
            voices,
            events: EventQueue::new(),
            amp_rates: AdsrRates::default(),
            env2_rates: AdsrRates::default(),
            glide_coef: 0.0,
            last_freq: None,
            counter: 0,
            level: Smoother::new(0.5),
            cutoff: Smoother::new(12_000.0),
            position: Smoother::new(0.0),
            damping: Smoother::new(2.0),
            sustain: false,
            bend: 0.0,
            wheel: 0.0,
            vib_phase: 0.0,
            lfo_phase: 0.0,
            user: user::Slot::default(),
        };
        s.update();
        s
    }

    fn update(&mut self) {
        let sr = self.sr;
        let v = |i| self.p.v(i);
        self.amp_rates = AdsrRates::new(v(AMP_A), v(AMP_A + 1), v(AMP_A + 2) / 100.0, v(AMP_A + 3), sr);
        self.env2_rates = AdsrRates::new(v(ENV2_A), v(ENV2_A + 1), v(ENV2_A + 2) / 100.0, v(ENV2_A + 3), sr);
        let glide = v(GLIDE);
        self.glide_coef = if glide < 0.5 { 0.0 } else { crate::util::time_coef(glide / 3.0, sr) };
        self.level.set(db_to_gain(v(LEVEL)));
        self.cutoff.set(v(CUTOFF));
        self.position.set(v(POSITION) / 100.0);
        self.damping.set(2.0 - 1.95 * (v(RESONANCE) / 100.0));
    }

    fn start_note(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.stop_note(note);
            return;
        }
        self.counter += 1;
        let freq = note_hz(f32::from(note.min(127)));
        let idx = pick_voice(self.voices.iter().map(|v| (v.active(), v.note, v.held, v.amp.level)), note);
        let sens = self.p.v(VEL_SENS) / 100.0;
        let glide = self.glide_coef > 0.0;
        let start = if glide { self.last_freq.unwrap_or(freq) } else { freq };
        let counter = self.counter;
        if let Some(v) = self.voices.get_mut(idx) {
            let was_active = v.active();
            let raw = f32::from(vel.min(127)) / 127.0;
            v.note = note;
            v.vel = (1.0 - sens) + sens * raw;
            v.vel_raw = raw;
            v.target = freq;
            v.freq = if was_active && glide { v.freq } else { start };
            if !was_active {
                v.restart();
            }
            v.amp.gate_on();
            v.env2.gate_on();
            v.age = counter;
            v.held = true;
            v.sustained = false;
        }
        self.last_freq = Some(freq);
    }

    fn stop_note(&mut self, note: u8) {
        let sustain = self.sustain;
        for v in self.voices.iter_mut().filter(|v| v.held && v.note == note) {
            v.held = false;
            if sustain {
                v.sustained = true;
            } else {
                v.amp.gate_off();
                v.env2.gate_off();
            }
        }
    }

    fn set_sustain(&mut self, on: bool) {
        self.sustain = on;
        if !on {
            for v in self.voices.iter_mut().filter(|v| v.sustained) {
                v.sustained = false;
                v.amp.gate_off();
                v.env2.gate_off();
            }
        }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(n, v) => self.start_note(n, v),
            EventKind::Off(n) => self.stop_note(n),
            EventKind::AllOff => {
                for v in &mut self.voices {
                    v.held = false;
                    v.sustained = false;
                    v.amp.gate_off();
                    v.env2.gate_off();
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

    /// The bend and the mod wheel's vibrato, cents.
    #[inline]
    fn pitch_cents(&self) -> f32 {
        let mut cents = self.bend * self.p.v(BEND_RANGE) * 100.0;
        if self.wheel > 0.0 {
            cents += self.p.v(MOD_WHEEL) * self.wheel * (std::f32::consts::TAU * self.vib_phase).sin();
        }
        cents
    }

    /// One sample, left and right (the same unless unison spreads the copies).
    #[inline]
    fn render_sample(&mut self) -> (f32, f32) {
        let sr = self.sr;
        let bank = self.p.choice(TABLE).min(tables::BANKS.len() - 1);
        let unison = self.p.choice(UNISON_P).min(MAX_UNISON - 1) + 1;
        let k = self.damping.next();
        let cutoff = self.cutoff.next();
        let base_pos = self.position.next();
        let nyq = sr * 0.45;
        let cents = self.pitch_cents();
        self.vib_phase = (self.vib_phase + VIBRATO_HZ / sr).fract();
        let lfo = lfo_shape(self.p.choice(LFO_WAVE), self.lfo_phase);
        self.lfo_phase = (self.lfo_phase + self.p.v(LFO_RATE) / sr).fract();
        let cents = cents + self.p.v(LFO_PITCH) * lfo + self.p.v(PITCH) * 100.0 + self.p.v(FINE);
        let lfo_oct = self.p.v(LFO_CUTOFF) / 100.0 * 4.0 * lfo;
        let lfo_amp = 1.0 - self.p.v(LFO_AMP) / 100.0 * 0.5 * (1.0 - lfo);
        // Shared position offsets: the LFO (±half the table at full) and the mod wheel.
        let shared_pos = base_pos + self.p.v(LFO_POS) / 100.0 * 0.5 * lfo + self.p.v(WHEEL_POS) / 100.0 * self.wheel;
        let env2_pos = self.p.v(ENV2_POS) / 100.0;
        let vel_pos = self.p.v(VEL_POS) / 100.0;
        let env2_oct = self.p.v(ENV2_CUTOFF) / 100.0 * 6.0;
        let kind = self.p.choice(FILTER_TYPE);
        let key_track = self.p.v(KEY_TRACK) / 100.0;
        let sub_level = self.p.v(SUB_LEVEL) / 100.0;
        let noise_level = self.p.v(NOISE_LEVEL) / 100.0;
        let ratio = if cents == 0.0 { 1.0 } else { 2f32.powf(cents / 1200.0) };
        let copies = unison_copies(unison, self.p.v(UNISON_DETUNE), self.p.v(UNISON_SPREAD) / 100.0);
        // The highest copy's ratio picks the band limit for the whole stack.
        let top_ratio = copies.iter().take(unison).fold(1.0f32, |m, c| m.max(c.0));
        let (mut out_l, mut out_r) = (0.0, 0.0);
        // A user table in place of the bank (and the one faded from while a new one comes in).
        let table = self.user.current();
        let from = self.user.fading_from();
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
            let e2 = v.env2.next(&self.env2_rates);
            let pos = (shared_pos + env2_pos * e2 + vel_pos * v.vel_raw).clamp(0.0, 1.0);
            let inc = (v.freq * ratio / sr).clamp(0.0, 0.49);
            let level = tables::level_for(inc * top_ratio);
            let (mut l, mut r) = (0.0, 0.0);
            for (ph, &(cr, gl, gr)) in v.phases.iter_mut().zip(copies.iter()).take(unison) {
                let s = if !self.user.active() {
                    self.t.read(bank, pos, level, *ph)
                } else {
                    let s = user::read_either(self.t, table, bank, pos, level, *ph);
                    match from {
                        Some((old, left)) => s + (user::read_either(self.t, old, bank, pos, level, *ph) - s) * left,
                        None => s,
                    }
                };
                *ph += inc * cr;
                if *ph >= 1.0 {
                    *ph -= 1.0;
                }
                if !ph.is_finite() || *ph >= 1.0 {
                    *ph = 0.0;
                }
                l += s * gl;
                r += s * gr;
            }
            let mut centre = 0.0;
            if sub_level > 0.0 {
                centre += sub_level * v.sub.next(Waveform::Square, inc * 0.5);
            }
            if noise_level > 0.0 {
                v.noise ^= v.noise << 13;
                v.noise ^= v.noise >> 17;
                v.noise ^= v.noise << 5;
                centre += noise_level * (v.noise as f32 / u32::MAX as f32 * 2.0 - 1.0);
            }
            let track = if key_track > 0.0 { key_track * (f32::from(v.note) - 60.0) / 12.0 } else { 0.0 };
            let fc = (cutoff * 2f32.powf(env2_oct * e2 + track + lfo_oct)).clamp(20.0, nyq);
            let g = (std::f32::consts::PI * fc / sr).tan();
            let gain = a * v.vel * lfo_amp;
            if unison == 1 {
                let mut y = v.svf.run(l + centre, g, k, kind);
                if kind == 1 {
                    y = v.svf2.run(y, g, k, 1);
                }
                out_l += y * gain;
                out_r += y * gain;
                continue;
            }
            let mut yl = v.svf.run(l + centre, g, k, kind);
            let mut yr = v.svf_r.run(r + centre, g, k, kind);
            if kind == 1 {
                yl = v.svf2.run(yl, g, k, 1);
                yr = v.svf2_r.run(yr, g, k, 1);
            }
            out_l += yl * gain;
            out_r += yr * gain;
        }
        self.user.step();
        (out_l * 0.3, out_r * 0.3)
    }
}

impl Plugin for WavetableSynth {
    param_plumbing!(WAVETABLE_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        for s in [&mut self.level, &mut self.cutoff, &mut self.position, &mut self.damping] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for (i, v) in self.voices.iter_mut().enumerate() {
            *v = Voice::new(0x2000 + i as u32 * 37);
        }
        self.events.clear();
        self.last_freq = None;
        for s in [&mut self.level, &mut self.cutoff, &mut self.position, &mut self.damping] {
            s.snap();
        }
        self.sustain = false;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.vib_phase = 0.0;
        self.lfo_phase = 0.0;
        self.user.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let mut evs = [super::synth::Event { offset: 0, kind: EventKind::AllOff }; MAX_EVENTS];
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
                if let Some(x) = io.first_mut().and_then(|b| b.get_mut(n)) {
                    *x = 0.5 * (l + r);
                }
                continue;
            }
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                if let Some(x) = b.get_mut(n) {
                    *x = if c % 2 == 0 { l } else { r };
                }
            }
        }
        for e in evs.iter().take(count).skip(next) {
            self.apply(e.kind);
        }
    }

    fn tail_samples(&self) -> usize {
        ms_to_samples(self.p.v(AMP_A + 3) * (SILENT.ln() / 0.001f32.ln()), self.sr) as usize
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

    /// A user table made ready off the audio thread ([`user::prepare`]) under
    /// [`crate::PREPARED_ASSET_KEY`], `None` to go back to the built-in bank. Swapped in with a
    /// short crossfade while notes sound (at once when none do); the one let go of comes back
    /// out of `take_released_asset`.
    fn set_asset(&mut self, key: &str, asset: Option<std::sync::Arc<dyn crate::SampleSource>>) -> bool {
        key == crate::PREPARED_ASSET_KEY && self.user.set(asset, self.sr, self.voices.iter().any(Voice::active))
    }

    fn take_released_asset(&mut self) -> Option<std::sync::Arc<dyn crate::SampleSource>> {
        self.user.take_released()
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod user_tests;
