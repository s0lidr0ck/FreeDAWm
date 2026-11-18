//! FM Synth: 16 voices of four sine operators in eight algorithms (how the operators modulate
//! one another), each operator with a frequency ratio (coarse, fine and detune) or a fixed
//! frequency, a level, its own envelope, velocity sensitivity, key scaling and self-feedback.
//! An LFO moves the pitch, the level and the modulators' depth; the mod wheel brings in vibrato
//! and can deepen or lighten the modulators. Phase modulation runs at four times the sample rate
//! and comes down through two half-band filters, so the sidebands above half the sample rate are
//! mostly taken out instead of folding back.
//!
//! The envelopes, the event queue, the controller messages and the voice choice are the
//! Subtractive Synth's.

use super::synth::{
    Adsr, AdsrRates, Event, EventKind, EventQueue, LFO_WAVES, MAX_EVENTS, OFF_ON, SILENT, Stage, VIBRATO_HZ, event_of, lfo_shape, pick_voice,
};
use crate::params::{Params, choice, lin, log, param_plumbing};
use crate::util::{Smoother, frames_in, ms_to_samples, note_hz, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

pub mod presets;
#[cfg(test)]
mod reference;

/// Operator frequency ratios: 0.5, then 1 to 16.
const COARSE: &[&str] = &["0.5", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

/// The algorithms: `a>b` is operator a modulating operator b; the operators not modulating
/// another are heard (the carriers).
pub const ALGORITHMS: &[&str] =
    &["1: 4>3>2>1", "2: 3+4>2>1", "3: 3>2>1, 4>1", "4: 4>3>1, 2>1", "5: 2>1, 4>3", "6: 4>1, 4>2, 4>3", "7: 4>3, 1, 2", "8: 1, 2, 3, 4"];

/// Per algorithm: for each operator (1 to 4), the operators modulating it, as a bit mask
/// (bit 0 = operator 1).
pub const MODULATORS: [[u8; 4]; 8] = [
    [0b0010, 0b0100, 0b1000, 0],
    [0b0010, 0b1100, 0, 0],
    [0b1010, 0b0100, 0, 0],
    [0b0110, 0, 0b1000, 0],
    [0b0010, 0, 0b1000, 0],
    [0b1000, 0b1000, 0b1000, 0],
    [0, 0, 0b1000, 0],
    [0, 0, 0, 0],
];

/// The operators heard in each algorithm (bit 0 = operator 1).
pub const CARRIERS: [u8; 8] = [0b0001, 0b0001, 0b0001, 0b0001, 0b0101, 0b0111, 0b0111, 0b1111];

/// The parameter list: each operator's (`opN_` ids, with its default level), then the globals.
macro_rules! op_params {
    ($(($n:literal, $level:expr)),*; $($global:expr),*) => {
        [
            $(
            choice(concat!("op", $n, "_coarse"), concat!("Op ", $n, " Ratio"), COARSE, 1),
            lin(concat!("op", $n, "_fine"), concat!("Op ", $n, " Fine"), 0.0, 100.0, 0.0, Unit::Percent),
            lin(concat!("op", $n, "_detune"), concat!("Op ", $n, " Detune"), -50.0, 50.0, 0.0, Unit::Cents),
            choice(concat!("op", $n, "_fixed"), concat!("Op ", $n, " Fixed"), OFF_ON, 0),
            log(concat!("op", $n, "_freq"), concat!("Op ", $n, " Fixed Freq"), 1.0, 8000.0, 440.0, Unit::Hz),
            lin(concat!("op", $n, "_level"), concat!("Op ", $n, " Level"), 0.0, 100.0, $level, Unit::Percent),
            log(concat!("op", $n, "_attack"), concat!("Op ", $n, " Attack"), 0.5, 5000.0, 2.0, Unit::Ms),
            log(concat!("op", $n, "_decay"), concat!("Op ", $n, " Decay"), 1.0, 10000.0, 800.0, Unit::Ms),
            lin(concat!("op", $n, "_sustain"), concat!("Op ", $n, " Sustain"), 0.0, 100.0, 70.0, Unit::Percent),
            log(concat!("op", $n, "_release"), concat!("Op ", $n, " Release"), 1.0, 10000.0, 300.0, Unit::Ms),
            lin(concat!("op", $n, "_velocity"), concat!("Op ", $n, " Velocity"), 0.0, 100.0, 50.0, Unit::Percent),
            lin(concat!("op", $n, "_key_scale"), concat!("Op ", $n, " Key Scaling"), -12.0, 12.0, 0.0, Unit::Db),
            lin(concat!("op", $n, "_feedback"), concat!("Op ", $n, " Feedback"), 0.0, 100.0, 0.0, Unit::Percent),
            )*
            $($global),*
        ]
    };
}

/// Parameters per operator, in the order `op_params!` lists them.
const OP_N: usize = 13;
const FINE: usize = 1;
const DETUNE: usize = 2;
const FIXED: usize = 3;
const FREQ: usize = 4;
const OP_LEVEL: usize = 5;
const ATTACK: usize = 6;
const VELOCITY: usize = 10;
const KEY_SCALE: usize = 11;
const FEEDBACK: usize = 12;

static FM_PARAMS: [ParamInfo; 4 * OP_N + 10] = op_params![
    (1, 100.0),
    (2, 45.0),
    (3, 0.0),
    (4, 0.0);
    choice("algorithm", "Algorithm", ALGORITHMS, 0),
    lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
    lin("bend_range", "Bend Range", 0.0, 24.0, 2.0, Unit::Semitones),
    lin("mod_wheel", "Mod Wheel Vibrato", 0.0, 100.0, 0.0, Unit::Cents),
    lin("wheel_mod", "Mod Wheel to Modulators", -100.0, 100.0, 0.0, Unit::Percent),
    choice("lfo_wave", "LFO Wave", LFO_WAVES, 0),
    log("lfo_rate", "LFO Rate", 0.05, 30.0, 5.0, Unit::Hz),
    lin("lfo_pitch", "LFO to Pitch", 0.0, 1200.0, 0.0, Unit::Cents),
    lin("lfo_amp", "LFO to Level", 0.0, 100.0, 0.0, Unit::Percent),
    lin("lfo_mod", "LFO to Modulators", 0.0, 100.0, 0.0, Unit::Percent)
];

const ALGORITHM: usize = 4 * OP_N;
const LEVEL: usize = ALGORITHM + 1;
const BEND_RANGE: usize = ALGORITHM + 2;
const MOD_WHEEL: usize = ALGORITHM + 3;
const WHEEL_MOD: usize = ALGORITHM + 4;
const LFO_WAVE: usize = ALGORITHM + 5;
const LFO_RATE: usize = ALGORITHM + 6;
const LFO_PITCH: usize = ALGORITHM + 7;
const LFO_AMP: usize = ALGORITHM + 8;
const LFO_MOD: usize = ALGORITHM + 9;

pub static FM_INFO: PluginInfo = PluginInfo {
    id: "fm_synth",
    name: "Steeple",
    short_name: "Steeple",
    category: Category::Instrument,
    params: &FM_PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

const VOICES: usize = 16;
/// How far a modulator at full level moves its target's phase, cycles (about 12.6 radians).
const MOD_DEPTH: f32 = 2.0;
/// How far an operator's feedback at 100% moves its own phase, cycles.
const FB_DEPTH: f32 = 0.5;
/// Sine table length (a power of two), read with linear interpolation.
const SINE_N: usize = 4096;
/// The half-band decimator's taps (odd; the centre at `HB_TAPS / 2`).
const HB_TAPS: usize = 63;

/// An operator's level control (0..100) as an amplitude: squared, so the lower half of the
/// control gives the finer steps a modulation index needs.
#[inline]
fn level_amp(level: f32) -> f32 {
    let x = (level / 100.0).clamp(0.0, 1.0);
    x * x
}

/// An operator's frequency ratio from its coarse choice, fine (0..100 %) and detune (cents).
#[inline]
pub fn ratio_of(coarse: usize, fine: f32, detune: f32) -> f32 {
    let c = if coarse == 0 { 0.5 } else { coarse.min(COARSE.len() - 1) as f32 };
    c * (1.0 + fine / 100.0) * 2f32.powf(detune / 1200.0)
}

/// sin(2π x) from the table `sine` (`SINE_N` + 1 entries), linearly interpolated.
#[inline]
fn sin_cycles(sine: &[f32], x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let p = x - x.floor();
    let f = p * SINE_N as f32;
    let i = (f as usize).min(SINE_N - 1);
    let fr = f - i as f32;
    let a = sine.get(i).copied().unwrap_or(0.0);
    a + (sine.get(i + 1).copied().unwrap_or(0.0) - a) * fr
}

#[derive(Debug, Clone, Copy)]
struct Op {
    phase: f32,
    env: Adsr,
    /// The last two outputs (for feedback, averaged so it does not ring at the Nyquist).
    y1: f32,
    y2: f32,
}

impl Op {
    const fn new() -> Self {
        Op { phase: 0.0, env: Adsr::new(), y1: 0.0, y2: 0.0 }
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    note: u8,
    vel: f32,
    freq: f32,
    ops: [Op; 4],
    age: u64,
    held: bool,
    sustained: bool,
}

impl Voice {
    const fn new() -> Self {
        Voice { note: 0, vel: 0.0, freq: 440.0, ops: [Op::new(); 4], age: 0, held: false, sustained: false }
    }

    /// Sounding while any operator's envelope is (an idle carrier is silent; a modulator alone
    /// is not heard, but its envelope still ends with the note).
    fn active(&self) -> bool {
        self.ops.iter().any(|o| o.env.stage != Stage::Idle)
    }

    fn loudness(&self) -> f32 {
        self.ops.iter().fold(0.0f32, |m, o| m.max(o.env.level))
    }

    fn gate_on(&mut self) {
        for o in &mut self.ops {
            o.env.gate_on();
        }
    }

    fn gate_off(&mut self) {
        for o in &mut self.ops {
            o.env.gate_off();
        }
    }
}

/// A linear-phase half-band low-pass taking a stream at twice the rate down to the rate: a
/// Kaiser-windowed sinc, flat to about 0.42 of the output rate and -80 dB from about 0.58.
#[derive(Debug, Clone)]
struct HalfBand {
    h: [f32; HB_TAPS],
    hist: [f32; 64],
    at: usize,
}

impl HalfBand {
    fn new() -> Self {
        let mut h = [0.0f32; HB_TAPS];
        let mid = (HB_TAPS / 2) as f64;
        let i0 = |x: f64| {
            let (mut s, mut t) = (1.0, 1.0);
            for k in 1..30 {
                let q = x / (2 * k) as f64;
                t *= q * q;
                s += t;
            }
            s
        };
        let beta = 8.0;
        for (i, c) in h.iter_mut().enumerate() {
            let t = i as f64 - mid;
            let sinc = if t == 0.0 { 0.5 } else { (std::f64::consts::PI * 0.5 * t).sin() / (std::f64::consts::PI * t) };
            let r = t / mid;
            *c = (sinc * i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0(beta)) as f32;
        }
        let sum: f32 = h.iter().sum();
        if sum.abs() > 1e-6 {
            for c in &mut h {
                *c /= sum;
            }
        }
        HalfBand { h, hist: [0.0; 64], at: 0 }
    }

    fn reset(&mut self) {
        self.hist = [0.0; 64];
        self.at = 0;
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.at = (self.at + 1) & 63;
        if let Some(s) = self.hist.get_mut(self.at) {
            *s = x;
        }
    }

    /// The filtered value at the newest sample pushed.
    #[inline]
    fn out(&self) -> f32 {
        let mut y = 0.0;
        for (i, c) in self.h.iter().enumerate() {
            y += c * self.hist.get((self.at + 64 - i) & 63).copied().unwrap_or(0.0);
        }
        y
    }
}

/// Polyphonic four-operator FM synthesizer.
pub struct FmSynth {
    p: Params,
    sr: f32,
    ch: usize,
    voices: [Voice; VOICES],
    events: EventQueue,
    rates: [AdsrRates; 4],
    sine: Vec<f32>,
    /// Four times the rate down to twice, then twice down to the rate.
    down4: HalfBand,
    down: HalfBand,
    /// Four times the sample rate, through the half-bands (off only in tests, to measure what it
    /// saves).
    oversample: bool,
    counter: u64,
    level: Smoother,
    sustain: bool,
    bend: f32,
    wheel: f32,
    vib_phase: f32,
    lfo_phase: f32,
}

impl Default for FmSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl FmSynth {
    pub fn new() -> Self {
        let sine = (0..=SINE_N).map(|i| (std::f64::consts::TAU * i as f64 / SINE_N as f64).sin() as f32).collect();
        let mut s = Self {
            p: Params::new(&FM_PARAMS),
            sr: 48_000.0,
            ch: 0,
            voices: [Voice::new(); VOICES],
            events: EventQueue::new(),
            rates: [AdsrRates::default(); 4],
            sine,
            down4: HalfBand::new(),
            down: HalfBand::new(),
            oversample: true,
            counter: 0,
            level: Smoother::new(0.5),
            sustain: false,
            bend: 0.0,
            wheel: 0.0,
            vib_phase: 0.0,
            lfo_phase: 0.0,
        };
        s.update();
        s
    }

    /// The rate the operators run at.
    fn inner_sr(&self) -> f32 {
        if self.oversample { self.sr * 4.0 } else { self.sr }
    }

    fn update(&mut self) {
        let sr = self.inner_sr();
        for (n, r) in self.rates.iter_mut().enumerate() {
            let b = n * OP_N + ATTACK;
            *r = AdsrRates::new(self.p.v(b), self.p.v(b + 1), self.p.v(b + 2) / 100.0, self.p.v(b + 3), sr);
        }
        self.level.set(db_to_gain(self.p.v(LEVEL)));
    }

    fn start_note(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.stop_note(note);
            return;
        }
        self.counter += 1;
        let idx = pick_voice(self.voices.iter().map(|v| (v.active(), v.note, v.held, v.loudness())), note);
        let counter = self.counter;
        if let Some(v) = self.voices.get_mut(idx) {
            if !v.active() {
                for o in &mut v.ops {
                    o.phase = 0.0;
                    o.y1 = 0.0;
                    o.y2 = 0.0;
                }
            }
            v.note = note;
            v.vel = f32::from(vel.min(127)) / 127.0;
            v.freq = note_hz(f32::from(note.min(127)));
            v.gate_on();
            v.age = counter;
            v.held = true;
            v.sustained = false;
        }
    }

    fn stop_note(&mut self, note: u8) {
        let sustain = self.sustain;
        for v in self.voices.iter_mut().filter(|v| v.held && v.note == note) {
            v.held = false;
            if sustain {
                v.sustained = true;
            } else {
                v.gate_off();
            }
        }
    }

    fn set_sustain(&mut self, on: bool) {
        self.sustain = on;
        if !on {
            for v in self.voices.iter_mut().filter(|v| v.sustained) {
                v.sustained = false;
                v.gate_off();
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
                    v.gate_off();
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

    #[inline]
    fn pitch_cents(&self) -> f32 {
        let mut cents = self.bend * self.p.v(BEND_RANGE) * 100.0;
        if self.wheel > 0.0 {
            cents += self.p.v(MOD_WHEEL) * self.wheel * (std::f32::consts::TAU * self.vib_phase).sin();
        }
        cents
    }

    /// One sample at the operators' rate.
    #[inline]
    fn render_inner(&mut self) -> f32 {
        let sr = self.inner_sr();
        let alg = self.p.choice(ALGORITHM).min(MODULATORS.len() - 1);
        let mods = MODULATORS.get(alg).copied().unwrap_or([0; 4]);
        let carriers = CARRIERS.get(alg).copied().unwrap_or(1);
        let norm = 1.0 / carriers.count_ones().max(1) as f32;
        let cents = self.pitch_cents();
        self.vib_phase = (self.vib_phase + VIBRATO_HZ / sr).fract();
        let lfo = lfo_shape(self.p.choice(LFO_WAVE), self.lfo_phase);
        self.lfo_phase = (self.lfo_phase + self.p.v(LFO_RATE) / sr).fract();
        let cents = cents + self.p.v(LFO_PITCH) * lfo;
        let bend = if cents == 0.0 { 1.0 } else { 2f32.powf(cents / 1200.0) };
        let lfo_amp = 1.0 - self.p.v(LFO_AMP) / 100.0 * 0.5 * (1.0 - lfo);
        // The modulators' depth: the LFO (down from full) and the mod wheel (either way).
        let mod_scale = ((1.0 - self.p.v(LFO_MOD) / 100.0 * 0.5 * (1.0 - lfo)) * (1.0 + self.p.v(WHEEL_MOD) / 100.0 * self.wheel)).max(0.0);
        // Per operator: its ratio (or fixed frequency), level, velocity share, key scaling and
        // feedback, read once for every voice.
        let mut cfg = [(0.0f32, false, 0.0f32, 0.0f32, 0.0f32, 0.0f32); 4];
        for (n, c) in cfg.iter_mut().enumerate() {
            let b = n * OP_N;
            let fixed = self.p.choice(b + FIXED) == 1;
            let f = if fixed { self.p.v(b + FREQ) / sr } else { ratio_of(self.p.choice(b), self.p.v(b + FINE), self.p.v(b + DETUNE)) };
            *c = (
                f,
                fixed,
                level_amp(self.p.v(b + OP_LEVEL)),
                self.p.v(b + VELOCITY) / 100.0,
                self.p.v(b + KEY_SCALE),
                self.p.v(b + FEEDBACK) / 100.0 * FB_DEPTH,
            );
        }
        let mut out = 0.0;
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            let key_oct = (f32::from(v.note) - 60.0) / 12.0;
            let base = v.freq * bend / sr;
            let mut y = [0.0f32; 4];
            for n in (0..4).rev() {
                let (Some(op), Some(&(f, fixed, lvl, vs, ks, fb)), Some(&mask)) = (v.ops.get_mut(n), cfg.get(n), mods.get(n)) else { continue };
                let env = op.env.next(&self.rates[n]);
                let inc = if fixed { f } else { base * f };
                let inc = if inc.is_finite() { inc.clamp(0.0, 0.499) } else { 0.0 };
                let mut m = 0.0;
                for (i, yi) in y.iter().enumerate() {
                    if mask & (1 << i) != 0 {
                        m += yi * MOD_DEPTH;
                    }
                }
                if fb > 0.0 {
                    m += fb * 0.5 * (op.y1 + op.y2);
                }
                let key = if ks == 0.0 { 1.0 } else { db_to_gain((ks * key_oct).clamp(-48.0, 12.0)) };
                let mut amp = env * lvl * (1.0 - vs + vs * v.vel) * key;
                if carriers & (1 << n) == 0 {
                    amp *= mod_scale;
                }
                let out_n = sin_cycles(&self.sine, op.phase + m) * amp;
                op.y2 = op.y1;
                op.y1 = if out_n.is_finite() { out_n } else { 0.0 };
                op.phase += inc;
                if op.phase >= 1.0 {
                    op.phase -= 1.0;
                }
                if let Some(yn) = y.get_mut(n) {
                    *yn = op.y1;
                }
            }
            for (n, yn) in y.iter().enumerate() {
                if carriers & (1 << n) != 0 {
                    out += yn * norm;
                }
            }
            // A voice whose carriers have all finished is free, whatever its modulators do.
            if v.ops.iter().enumerate().all(|(n, o)| carriers & (1 << n) == 0 || o.env.stage == Stage::Idle) {
                for o in &mut v.ops {
                    o.env = Adsr::new();
                }
            }
        }
        out * lfo_amp * 0.5
    }
}

impl Plugin for FmSynth {
    param_plumbing!(FM_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.voices = [Voice::new(); VOICES];
        self.events.clear();
        self.down.reset();
        self.down4.reset();
        self.level.snap();
        self.sustain = false;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.vib_phase = 0.0;
        self.lfo_phase = 0.0;
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
            let y = if self.oversample {
                for _ in 0..2 {
                    for _ in 0..2 {
                        let a = self.render_inner();
                        self.down4.push(a);
                    }
                    let half = self.down4.out();
                    self.down.push(half);
                }
                self.down.out()
            } else {
                self.render_inner()
            };
            let y = y * self.level.next();
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x = y;
                }
            }
        }
        for e in evs.iter().take(count).skip(next) {
            self.apply(e.kind);
        }
    }

    fn tail_samples(&self) -> usize {
        let longest = (0..4).map(|n| self.p.v(n * OP_N + ATTACK + 3)).fold(0.0f32, f32::max);
        ms_to_samples(longest * (SILENT.ln() / 0.001f32.ln()), self.sr) as usize + HB_TAPS * 3 / 4
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
mod tests;
