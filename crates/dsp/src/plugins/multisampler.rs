//! Pantry - Multisampler (id `multisampler`): plays an instrument recorded note by note, each note at
//! several strengths, 96 voices.
//!
//! * **Samples** arrive as keyed assets (`Plugin::set_asset`), `n{note}.v{velocity}.r{robin}`:
//!   the note the sample was recorded at (0–127), the velocity it was played with (1–127) and its
//!   round-robin take (1–4). A note holds up to 16 velocity layers, in any order.
//! * **Keys.** A key plays the nearest recorded note (the lower of two equally near), pitched to
//!   the key with 4-point cubic interpolation, which also converts the sample's own rate.
//! * **Velocity.** A layer covers the velocities from just above the layer below it up to its
//!   own. A softer hit than the layer was recorded at is turned down by Velocity Curve (the
//!   level falls as the velocity ratio to that power), so the level rises evenly through a layer
//!   and into the next. Layer Blend mixes in the layer below over the bottom of each layer's
//!   range (equal power), so the tone changes gradually too: at 100% every hit between two
//!   layers is a mix of both.
//! * **Round-robins.** Each hit of a recorded note plays its next take.
//! * **Notes** have an amp ADSR; the sustain pedal (controller 64) holds released notes until it
//!   comes up; pitch bend is ±2 semitones. A key struck again releases its sounding note.
//!
//! A replaced sample is handed back through [`Plugin::take_released_asset`], so it is never
//! freed on the audio thread. Audio thread: no allocation, no locks, no panics.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use super::sampler::{hermite, pan_display};
use super::synth::{Adsr, AdsrRates, Event, EventKind, EventQueue, MAX_EVENTS, Stage};
use crate::params::{Params, lin, log, param_plumbing};
use crate::util::{Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, SampleSource, Taper, Unit, db_to_gain};

/// Notes a sample can be recorded at.
pub const NOTES: usize = 128;
/// Velocity layers a recorded note holds.
pub const LAYERS: usize = 16;
/// Round-robin takes a layer holds.
pub const ROBINS: usize = 4;
const VOICES: usize = 96;
/// Fastest playback step in sample frames per output frame (keeps hostile pitch bounded).
const MAX_STEP: f64 = 256.0;
/// The most a hit harder than the top layer is turned up.
const MAX_BOOST: f32 = 2.0;
/// A blended layer quieter than this is not worth a voice.
const MIN_BLEND_GAIN: f32 = 0.01;
/// `key_root` for a key with no recorded note to play.
const NO_ROOT: u8 = 255;

const LEVEL: usize = 0;
const PAN: usize = 1;
const TRANSPOSE: usize = 2;
const FINE: usize = 3;
const ATTACK: usize = 4;
const DECAY: usize = 5;
const SUSTAIN: usize = 6;
const RELEASE: usize = 7;
const CURVE: usize = 8;
const BLEND: usize = 9;

// New parameters go at the end: sessions and presets store them by id, and the indices above
// are the order of this table.
static MULTI_PARAMS: [ParamInfo; 10] = [
    lin("level", "Level", -48.0, 12.0, 0.0, Unit::Db),
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
    lin("transpose", "Transpose", -48.0, 48.0, 0.0, Unit::Semitones),
    lin("fine_tune", "Fine Tune", -100.0, 100.0, 0.0, Unit::Cents),
    log("attack", "Attack", 0.1, 5000.0, 0.5, Unit::Ms),
    log("decay", "Decay", 1.0, 10000.0, 300.0, Unit::Ms),
    lin("sustain", "Sustain", 0.0, 100.0, 100.0, Unit::Percent),
    log("release", "Release", 1.0, 10000.0, 250.0, Unit::Ms),
    lin("velocity_curve", "Velocity Curve", 0.0, 4.0, 1.5, Unit::None),
    lin("layer_blend", "Layer Blend", 0.0, 100.0, 50.0, Unit::Percent),
];

pub static MULTISAMPLER_INFO: PluginInfo = PluginInfo {
    id: "multisampler",
    name: "Pantry - Multisampler",
    short_name: "Pantry",
    category: Category::Instrument,
    params: &MULTI_PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The asset key of a sample: the note it was recorded at, the velocity it was played with and
/// its round-robin take (from 1).
pub fn sample_key(note: u8, velocity: u8, robin: usize) -> String {
    format!("n{note}.v{velocity}.r{robin}")
}

/// A sample key's note (0–127), velocity (1–127) and round-robin index (from 0); `None` for
/// anything else.
pub fn parse_sample_key(key: &str) -> Option<(usize, u8, usize)> {
    let mut parts = key.split('.');
    let num = |s: Option<&str>, prefix: char, lo: usize, hi: usize| -> Option<usize> {
        let n: usize = s?.strip_prefix(prefix)?.parse().ok()?;
        (lo..=hi).contains(&n).then_some(n)
    };
    let note = num(parts.next(), 'n', 0, NOTES - 1)?;
    let velocity = u8::try_from(num(parts.next(), 'v', 1, 127)?).ok()?;
    let robin = num(parts.next(), 'r', 1, ROBINS)? - 1;
    parts.next().is_none().then_some((note, velocity, robin))
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    active: bool,
    key: u8,
    /// The sample it plays (an index into `Multisampler::samples`).
    slot: usize,
    pos: f64,
    /// Sample frames per output frame, before pitch bend.
    step: f64,
    gain: f32,
    env: Adsr,
    /// The key is still down.
    down: bool,
    /// The key came up with the pedal down: the pedal holds the note.
    pedalled: bool,
}

impl Voice {
    const fn new() -> Self {
        Self { active: false, key: 0, slot: 0, pos: 0.0, step: 1.0, gain: 0.0, env: Adsr::new(), down: false, pedalled: false }
    }

    fn release(&mut self) {
        self.down = false;
        self.pedalled = false;
        self.env.gate_off();
    }
}

/// Plays a multisampled instrument.
pub struct Multisampler {
    p: Params,
    sr: f32,
    ch: usize,
    /// Every sample slot: `(note * LAYERS + layer) * ROBINS + robin`.
    samples: Vec<Option<Arc<dyn SampleSource>>>,
    /// Each layer slot's velocity (`note * LAYERS + layer`); 0 for an empty slot.
    layer_vel: Vec<u8>,
    /// The recorded note each key plays (`NO_ROOT`: none).
    key_root: [u8; NOTES],
    /// Hits of each recorded note so far, for its round-robins.
    turns: [u8; NOTES],
    voices: [Voice; VOICES],
    events: EventQueue,
    rates: AdsrRates,
    level: Smoother,
    pan: Smoother,
    pedal: bool,
    /// Pitch bend as a frequency ratio.
    bend: f64,
    released: Vec<Arc<dyn SampleSource>>,
}

impl Default for Multisampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Multisampler {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&MULTI_PARAMS),
            sr: 48_000.0,
            ch: 0,
            samples: (0..NOTES * LAYERS * ROBINS).map(|_| None).collect(),
            layer_vel: vec![0; NOTES * LAYERS],
            key_root: [NO_ROOT; NOTES],
            turns: [0; NOTES],
            voices: [Voice::new(); VOICES],
            events: EventQueue::new(),
            rates: AdsrRates::default(),
            level: Smoother::new(1.0),
            pan: Smoother::new(0.0),
            pedal: false,
            bend: 1.0,
            released: Vec::with_capacity(NOTES * LAYERS * ROBINS + 64),
        };
        s.update();
        s
    }

    fn update(&mut self) {
        let v = |i| self.p.v(i);
        self.rates = AdsrRates::new(v(ATTACK), v(DECAY), v(SUSTAIN) / 100.0, v(RELEASE), self.sr);
        self.level.set(db_to_gain(v(LEVEL)));
        self.pan.set((v(PAN) / 100.0).clamp(-1.0, 1.0));
    }

    /// The velocities of a recorded note's layer slots (0: empty).
    fn layers(&self, note: usize) -> &[u8] {
        self.layer_vel.get(note * LAYERS..(note + 1) * LAYERS).unwrap_or(&[])
    }

    /// Works out the recorded note each key plays: the nearest one, the lower of two as near.
    fn map_keys(&mut self) {
        let mut below = NO_ROOT;
        for key in 0..NOTES {
            if self.layers(key).iter().any(|&v| v != 0) {
                below = key as u8;
            }
            if let Some(r) = self.key_root.get_mut(key) {
                *r = below;
            }
        }
        let mut above = NO_ROOT;
        for key in (0..NOTES).rev() {
            if self.layers(key).iter().any(|&v| v != 0) {
                above = key as u8;
            }
            let Some(r) = self.key_root.get_mut(key) else { continue };
            let nearer = *r == NO_ROOT || (above != NO_ROOT && usize::from(above) - key < key - usize::from(*r));
            if nearer {
                *r = above;
            }
        }
    }

    /// Keeps a sample the plugin has let go of for the host to drop.
    fn let_go(&mut self, old: Arc<dyn SampleSource>) {
        if self.released.len() < self.released.capacity() {
            self.released.push(old);
        } else {
            // The host stopped collecting: leak it rather than free it on the audio thread
            // (never happens with a working host).
            std::mem::forget(old);
        }
    }

    /// The `turn`th take among a layer's samples (an index into `samples`).
    fn take(&self, note: usize, layer: usize, turn: u8) -> Option<usize> {
        let base = (note * LAYERS + layer) * ROBINS;
        let held = |r: &usize| self.samples.get(base + r).is_some_and(Option::is_some);
        let count = (0..ROBINS).filter(held).count();
        (0..ROBINS).filter(held).nth(usize::from(turn) % count.max(1)).map(|r| base + r)
    }

    fn start_note(&mut self, key: u8, vel: u8) {
        let Some(&root) = self.key_root.get(usize::from(key)).filter(|&&r| r != NO_ROOT) else { return };
        let note = usize::from(root);
        let vel = vel.clamp(1, 127);
        // The layer to play: the softest one recorded at this velocity or harder, or the hardest.
        let mut upper: Option<(usize, u8)> = None;
        let mut top: Option<(usize, u8)> = None;
        for (i, &lv) in self.layers(note).iter().enumerate().filter(|(_, v)| **v != 0) {
            if lv >= vel && upper.is_none_or(|u| lv < u.1) {
                upper = Some((i, lv));
            }
            if top.is_none_or(|t| lv > t.1) {
                top = Some((i, lv));
            }
        }
        let Some((layer, layer_vel)) = upper.or(top) else { return };
        // And the one below it, whose range ends where this one's starts.
        let mut lower: Option<(usize, u8)> = None;
        for (i, &lv) in self.layers(note).iter().enumerate().filter(|(_, v)| **v != 0) {
            if lv < layer_vel && lower.is_none_or(|l| lv > l.1) {
                lower = Some((i, lv));
            }
        }
        let turn = self.turns.get(note).copied().unwrap_or(0);
        if let Some(t) = self.turns.get_mut(note) {
            *t = turn.wrapping_add(1);
        }
        for v in self.voices.iter_mut().filter(|v| v.active && v.key == key) {
            v.release();
        }
        let curve = self.p.v(CURVE);
        let gain_from = |recorded: u8| (f32::from(vel) / f32::from(recorded)).powf(curve).min(MAX_BOOST);
        let floor = lower.map_or(0.0, |l| f32::from(l.1));
        let span = (f32::from(layer_vel) - floor).max(1.0);
        // Where the hit is in the layer's range: 0 at the layer below, 1 at this one.
        let place = ((f32::from(vel) - floor) / span).clamp(0.0, 1.0);
        let blend = self.p.v(BLEND) / 100.0;
        let (this, under) = match lower {
            Some(_) if place < blend => {
                let x = place / blend * FRAC_PI_2;
                (x.sin(), x.cos())
            }
            _ => (1.0, 0.0),
        };
        let semis = f64::from(key) - f64::from(root) + f64::from(self.p.v(TRANSPOSE)) + f64::from(self.p.v(FINE)) / 100.0;
        if this >= MIN_BLEND_GAIN
            && let Some(slot) = self.take(note, layer, turn)
        {
            self.begin(key, slot, this * gain_from(layer_vel), semis);
        }
        if under >= MIN_BLEND_GAIN
            && let Some((l, lv)) = lower
            && let Some(slot) = self.take(note, l, turn)
        {
            self.begin(key, slot, under * gain_from(lv), semis);
        }
    }

    fn begin(&mut self, key: u8, slot: usize, gain: f32, semis: f64) {
        let Some(rate) = self.samples.get(slot).and_then(Option::as_deref).map(SampleSource::sample_rate) else { return };
        let rate = if rate.is_finite() && rate > 0.0 { f64::from(rate.min(1_536_000.0)) } else { f64::from(self.sr) };
        let step = 2f64.powf(semis / 12.0) * rate / f64::from(self.sr);
        // A free voice, or the one heard least: a released note before a held one.
        let loudness = |v: &Voice| if v.active { v.env.level * v.gain * if v.down || v.pedalled { 4.0 } else { 1.0 } } else { -1.0 };
        let Some(v) = self.voices.iter_mut().min_by(|a, b| loudness(a).total_cmp(&loudness(b))) else { return };
        *v = Voice {
            active: true,
            key,
            slot,
            pos: 0.0,
            step: if step.is_finite() { step.clamp(0.0, MAX_STEP) } else { 1.0 },
            gain: if gain.is_finite() { gain } else { 0.0 },
            env: Adsr::new(),
            down: true,
            pedalled: false,
        };
        v.env.gate_on();
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(key, vel) => self.start_note(key, vel),
            EventKind::Off(key) => {
                let pedal = self.pedal;
                for v in self.voices.iter_mut().filter(|v| v.active && v.down && v.key == key) {
                    if pedal {
                        v.down = false;
                        v.pedalled = true;
                    } else {
                        v.release();
                    }
                }
            }
            EventKind::AllOff => self.voices.iter_mut().for_each(Voice::release),
            EventKind::Sustain(on) => self.set_pedal(on),
            EventKind::Bend(ratio) => self.bend = if ratio.is_finite() && ratio > 0.0 { f64::from(ratio) } else { 1.0 },
            EventKind::ResetControllers => {
                self.set_pedal(false);
                self.bend = 1.0;
            }
        }
    }

    fn set_pedal(&mut self, on: bool) {
        self.pedal = on;
        if !on {
            for v in self.voices.iter_mut().filter(|v| v.pedalled) {
                v.release();
            }
        }
    }

    /// Adds the voices' frames `from..to` to `io`.
    fn render(&mut self, io: &mut [Vec<f32>], ch: usize, from: usize, to: usize) {
        let Some((left, rest)) = io.split_first_mut() else { return };
        let (Some(left), right) = (left.get_mut(from..to), rest.first_mut().filter(|_| ch >= 2).and_then(|r| r.get_mut(from..to))) else { return };
        let mut right = right;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            let Some(src) = self.samples.get(v.slot).and_then(Option::as_deref) else {
                v.active = false;
                continue;
            };
            let Some(l) = src.channel(0) else {
                v.active = false;
                continue;
            };
            let r = src.channel(1).unwrap_or(l);
            let frames = l.len().min(r.len()) as f64;
            let step = (v.step * self.bend).clamp(0.0, MAX_STEP);
            for n in 0..left.len() {
                let g = v.env.next(&self.rates) * v.gain;
                if v.env.stage == Stage::Idle || v.pos >= frames {
                    v.active = false;
                    break;
                }
                let (a, b) = (hermite(l, v.pos) * g, hermite(r, v.pos) * g);
                match right.as_deref_mut().and_then(|r| r.get_mut(n)) {
                    Some(out) => {
                        *out += b;
                        if let Some(out) = left.get_mut(n) {
                            *out += a;
                        }
                    }
                    None => {
                        if let Some(out) = left.get_mut(n) {
                            *out += 0.5 * (a + b);
                        }
                    }
                }
                v.pos += step;
            }
        }
    }
}

impl Plugin for Multisampler {
    param_plumbing!(MULTISAMPLER_INFO);

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
            v.active = false;
        }
        self.events.clear();
        self.level.snap();
        self.pan.snap();
        self.pedal = false;
        self.bend = 1.0;
        self.turns = [0; NOTES];
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
        let mut done = 0;
        for e in evs.iter().take(count) {
            let at = e.offset.min(frames);
            if at > done {
                self.render(io, ch, done, at);
                done = at;
            }
            self.apply(e.kind);
        }
        self.render(io, ch, done, frames);
        // Level, then balance: the centre leaves both sides as they are; turning one way fades
        // the other.
        for n in 0..frames {
            let level = self.level.next();
            let p = self.pan.next();
            let sides = [level * (1.0 - p).min(1.0), level * (1.0 + p).min(1.0)];
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                if let Some(x) = b.get_mut(n) {
                    *x *= if ch >= 2 { sides.get(c).copied().unwrap_or(0.0) } else { level };
                }
            }
        }
    }

    fn tail_samples(&self) -> usize {
        ms_to_samples(self.p.v(RELEASE), self.sr) as usize
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
            (0xE0, _) => {
                let v = (i32::from(d2 & 0x7F) << 7 | i32::from(d1 & 0x7F)) - 8192;
                EventKind::Bend(2f32.powf(v as f32 / 8192.0 * 2.0 / 12.0))
            }
            _ => return,
        };
        self.events.push(offset, kind);
    }

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
    }

    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        let Some((note, velocity, robin)) = parse_sample_key(key) else { return false };
        // The note's layer at this velocity, or (for a new one) a free layer.
        let at = |v: u8| self.layers(note).iter().position(|&l| l == v);
        let Some(layer) = at(velocity).or_else(|| asset.as_ref().and_then(|_| at(0))) else {
            // Nothing to clear, or a 17th layer: the sample is not kept.
            if let Some(a) = asset {
                self.let_go(a);
            }
            return true;
        };
        let base = (note * LAYERS + layer) * ROBINS;
        let Some(slot) = self.samples.get_mut(base + robin) else { return false };
        let old = std::mem::replace(slot, asset);
        for v in self.voices.iter_mut().filter(|v| v.slot == base + robin) {
            v.active = false;
        }
        if let Some(old) = old {
            self.let_go(old);
        }
        let held = (0..ROBINS).any(|r| self.samples.get(base + r).is_some_and(Option::is_some));
        if let Some(v) = self.layer_vel.get_mut(note * LAYERS + layer) {
            *v = if held { velocity } else { 0 };
        }
        self.map_keys();
        true
    }

    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        self.released.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SampleBuffer;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, amp: f32, len: usize) -> Arc<dyn SampleSource> {
        let data: Vec<f32> = (0..len).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR).sin()).collect();
        Arc::new(SampleBuffer { sample_rate: SR, channels: vec![data] })
    }

    /// A Multisampler with one-second sines: `(note, velocity, robin, Hz)`.
    fn multi(samples: &[(u8, u8, usize, f32)]) -> Multisampler {
        let mut s = Multisampler::new();
        s.prepare(SR, 512, 2);
        s.set_param("layer_blend", 0.0);
        for &(note, vel, robin, hz) in samples {
            assert!(s.set_asset(&sample_key(note, vel, robin), Some(sine(hz, 0.5, SR as usize))));
        }
        s
    }

    fn run(s: &mut Multisampler, len: usize) -> [Vec<f32>; 2] {
        let mut out = [Vec::new(), Vec::new()];
        let mut left = len;
        while left > 0 {
            let n = left.min(512);
            let mut io = vec![vec![0.0; n], vec![0.0; n]];
            s.process(&mut io, n);
            out[0].extend_from_slice(&io[0]);
            out[1].extend_from_slice(&io[1]);
            left -= n;
        }
        out
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    /// The frequency of a steady tone, from its zero crossings.
    fn freq(x: &[f32]) -> f32 {
        let ups = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        ups as f32 * SR / x.len() as f32
    }

    /// The amplitude of the `hz` partial of `x`.
    fn partial(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let ph = f64::from(std::f32::consts::TAU * hz * i as f32 / SR);
            re += f64::from(*v) * ph.cos();
            im += f64::from(*v) * ph.sin();
        }
        (2.0 * (re * re + im * im).sqrt() / x.len() as f64) as f32
    }

    /// One held note's steady sound: 0.2 s after a short settle.
    fn held(s: &mut Multisampler, key: u8, vel: u8) -> Vec<f32> {
        s.note_on(0, key, vel);
        run(s, 2400);
        let out = run(s, 9600);
        s.all_notes_off();
        run(s, 48_000);
        out[0].clone()
    }

    #[test]
    fn sample_keys_parse_and_refuse_anything_else() {
        assert_eq!(parse_sample_key(&sample_key(60, 95, 2)), Some((60, 95, 1)));
        assert_eq!(parse_sample_key("n0.v1.r1"), Some((0, 1, 0)));
        assert_eq!(parse_sample_key("n127.v127.r4"), Some((127, 127, 3)));
        for bad in ["", "n128.v1.r1", "n60.v0.r1", "n60.v128.r1", "n60.v1.r5", "n60.v1.r0", "n60.v1", "n60.v1.r1.x", "p1.v1.r1", "n-1.v1.r1"] {
            assert_eq!(parse_sample_key(bad), None, "{bad}");
        }
        let mut s = Multisampler::new();
        assert!(!s.set_asset("p1.v1.r1", None));
        assert!(s.set_asset("n60.v100.r1", None), "clearing an empty slot is fine");
    }

    #[test]
    fn silent_without_samples() {
        let mut s = multi(&[]);
        s.note_on(0, 60, 100);
        assert_eq!(peak(&run(&mut s, 4800)[0]), 0.0);
    }

    #[test]
    fn a_key_plays_the_nearest_recorded_note_at_its_own_pitch() {
        let mut s = multi(&[(48, 100, 1, 200.0), (60, 100, 1, 400.0)]);
        // The recorded notes play as they are.
        assert!((freq(&held(&mut s, 48, 100)) - 200.0).abs() < 6.0);
        assert!((freq(&held(&mut s, 60, 100)) - 400.0).abs() < 6.0);
        // Two keys up from one, and below and above them all.
        assert!((freq(&held(&mut s, 50, 100)) - 200.0 * 2f32.powf(2.0 / 12.0)).abs() < 6.0);
        assert!((freq(&held(&mut s, 36, 100)) - 100.0).abs() < 6.0);
        assert!((freq(&held(&mut s, 72, 100)) - 800.0).abs() < 6.0);
        // Halfway between the two goes to the lower one; one key on, to the upper.
        assert!((freq(&held(&mut s, 54, 100)) - 200.0 * 2f32.powf(6.0 / 12.0)).abs() < 6.0);
        assert!((freq(&held(&mut s, 55, 100)) - 400.0 * 2f32.powf(-5.0 / 12.0)).abs() < 6.0);
    }

    #[test]
    fn velocity_picks_the_layer_recorded_at_it_or_just_above() {
        let mut s = multi(&[(60, 40, 1, 300.0), (60, 80, 1, 500.0), (60, 127, 1, 700.0)]);
        for (vel, hz) in [(1, 300.0), (40, 300.0), (41, 500.0), (80, 500.0), (81, 700.0), (127, 700.0)] {
            assert!((freq(&held(&mut s, 60, vel)) - hz).abs() < 6.0, "velocity {vel}");
        }
        // Harder than the hardest layer still plays it.
        let mut s = multi(&[(60, 40, 1, 300.0), (60, 80, 1, 500.0)]);
        assert!((freq(&held(&mut s, 60, 120)) - 500.0).abs() < 6.0);
    }

    #[test]
    fn the_velocity_curve_turns_softer_hits_down() {
        let mut s = multi(&[(60, 100, 1, 400.0)]);
        s.set_param("velocity_curve", 2.0);
        let (full, half) = (peak(&held(&mut s, 60, 100)), peak(&held(&mut s, 60, 50)));
        assert!((full - 0.5).abs() < 0.01 && (half / full - 0.25).abs() < 0.01, "{full} {half}");
        // A harder hit than the top layer is turned up, to a limit.
        assert!((peak(&held(&mut s, 60, 127)) / full - (1.27f32).powi(2)).abs() < 0.02);
        s.set_param("velocity_curve", 0.0);
        assert!((peak(&held(&mut s, 60, 20)) - full).abs() < 0.01, "a flat curve plays every hit as recorded");
    }

    #[test]
    fn layer_blend_mixes_in_the_layer_below() {
        let mut s = multi(&[(60, 40, 1, 300.0), (60, 80, 1, 500.0)]);
        s.set_param("velocity_curve", 0.0);
        s.set_param("layer_blend", 100.0);
        // Halfway between the layers: both, at equal power.
        let out = held(&mut s, 60, 60);
        let (low, high) = (partial(&out, 300.0), partial(&out, 500.0));
        assert!((low - 0.5 * 0.5f32.sqrt()).abs() < 0.02 && (high - low).abs() < 0.02, "{low} {high}");
        // At a layer's own velocity, that layer alone.
        let out = held(&mut s, 60, 80);
        assert!(partial(&out, 300.0) < 0.005 && (partial(&out, 500.0) - 0.5).abs() < 0.02);
        // Nearer the upper layer, more of it.
        let out = held(&mut s, 60, 70);
        assert!(partial(&out, 500.0) > partial(&out, 300.0) * 1.5);
        // Half a blend: the upper half of the range is the upper layer alone.
        s.set_param("layer_blend", 50.0);
        let out = held(&mut s, 60, 62);
        assert!(partial(&out, 300.0) < 0.005 && (partial(&out, 500.0) - 0.5).abs() < 0.02);
        // The bottom layer has nothing under it to blend with.
        let out = held(&mut s, 60, 10);
        assert!(partial(&out, 500.0) < 0.005 && (partial(&out, 300.0) - 0.5).abs() < 0.02);
    }

    #[test]
    fn hits_cycle_through_the_round_robins() {
        let mut s = multi(&[(60, 100, 1, 300.0), (60, 100, 2, 500.0), (60, 100, 4, 700.0)]);
        let heard: Vec<f32> = (0..6).map(|_| (freq(&held(&mut s, 60, 100)) / 100.0).round()).collect();
        assert_eq!(heard, [3.0, 5.0, 7.0, 3.0, 5.0, 7.0]);
    }

    #[test]
    fn the_sustain_pedal_holds_released_notes() {
        let mut s = multi(&[(60, 100, 1, 400.0)]);
        s.set_param("release", 20.0);
        s.midi(0, [0xB0, 64, 127]);
        s.note_on(0, 60, 100);
        run(&mut s, 2400);
        s.note_off(0, 60);
        assert!(peak(&run(&mut s, 9600)[0][4800..]) > 0.4, "held by the pedal");
        s.midi(0, [0xB0, 64, 0]);
        assert!(peak(&run(&mut s, 9600)[0][4800..]) < 1.0e-3, "let go with it");
        // Without the pedal a released note dies away.
        s.note_on(0, 60, 100);
        run(&mut s, 2400);
        s.note_off(0, 60);
        assert!(peak(&run(&mut s, 9600)[0][4800..]) < 1.0e-3);
    }

    #[test]
    fn notes_start_and_stop_on_their_sample() {
        let mut s = multi(&[(60, 100, 1, 400.0)]);
        s.note_on(100, 60, 100);
        let out = run(&mut s, 512);
        assert!(peak(&out[0][..100]) == 0.0 && peak(&out[0][100..]) > 0.1);
        // A sample plays to its end and no further.
        run(&mut s, 48_000);
        assert_eq!(peak(&run(&mut s, 4800)[0]), 0.0);
    }

    #[test]
    fn stereo_samples_keep_their_sides_and_pan_fades_one() {
        let mut s = multi(&[]);
        let left: Vec<f32> = vec![0.5; 48_000];
        s.set_asset("n60.v100.r1", Some(Arc::new(SampleBuffer { sample_rate: SR, channels: vec![left, vec![0.0; 48_000]] })));
        s.note_on(0, 60, 100);
        let out = run(&mut s, 4800);
        assert!(peak(&out[0][2400..]) > 0.4 && peak(&out[1]) == 0.0);
        s.set_param("pan", 100.0);
        run(&mut s, 9600);
        assert!(peak(&run(&mut s, 2400)[0]) < 0.01, "panned right, the left side is gone");
    }

    #[test]
    fn a_sample_at_another_rate_plays_at_pitch() {
        let mut s = multi(&[]);
        let data: Vec<f32> = (0..24_000).map(|i| 0.5 * (std::f32::consts::TAU * 400.0 * i as f32 / 24_000.0).sin()).collect();
        s.set_asset("n60.v100.r1", Some(Arc::new(SampleBuffer { sample_rate: 24_000.0, channels: vec![data] })));
        assert!((freq(&held(&mut s, 60, 100)) - 400.0).abs() < 6.0);
    }

    #[test]
    fn a_replaced_sample_stops_its_voices_and_is_handed_back() {
        let mut s = multi(&[(60, 100, 1, 400.0)]);
        s.note_on(0, 60, 100);
        assert!(peak(&run(&mut s, 2400)[0]) > 0.1);
        s.set_asset("n60.v100.r1", Some(sine(600.0, 0.5, 48_000)));
        assert_eq!(peak(&run(&mut s, 2400)[0]), 0.0);
        assert!(s.take_released_asset().is_some() && s.take_released_asset().is_none());
        // Cleared, the key has nothing to play, and the layer's slot is free again.
        s.set_asset("n60.v100.r1", None);
        s.note_on(0, 60, 100);
        assert_eq!(peak(&run(&mut s, 2400)[0]), 0.0);
        assert!(s.layers(60).iter().all(|&v| v == 0) && s.key_root.iter().all(|&r| r == NO_ROOT));
    }

    #[test]
    fn a_seventeenth_layer_is_not_kept() {
        let mut s = multi(&[]);
        for v in 1..=17u8 {
            assert!(s.set_asset(&sample_key(60, v, 1), Some(sine(400.0, 0.5, 4800))));
        }
        assert!(s.layers(60).iter().all(|&v| (1..=16).contains(&v)));
        assert!(s.take_released_asset().is_some(), "the one that did not fit comes back");
    }

    #[test]
    fn many_notes_steal_voices_and_stay_bounded() {
        let mut s = multi(&[(60, 100, 1, 400.0)]);
        s.midi(0, [0xB0, 64, 127]);
        for k in 0..=127u8 {
            s.note_on(0, k, 127);
            s.note_off(0, k);
        }
        let out = run(&mut s, 4800);
        assert!(out[0].iter().all(|v| v.is_finite()) && s.voices.iter().filter(|v| v.active).count() <= VOICES);
    }
}
