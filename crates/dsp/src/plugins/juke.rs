//! Juke — Drum Machine (`docs/juke.md`): 16 pads, each a sample player, a synthesized drum voice
//! or both layered, with its own envelopes, filter, drive, level, pan, choke group and two sends.
//!
//! * **Triggers.** Each pad answers its note (`pN_note`; General MIDI drums by default) and can
//!   fire up to two other pads ("stacks") after 0–50 ms, for layers and flams. Gate pads stop on
//!   note-off; one-shot pads play their envelope out. A pad in a choke group (1–8) silences the
//!   group's other pads within 4 ms (the open hat cut by the closed hat).
//! * **Samples** arrive as keyed assets (`Plugin::set_asset`), `p{pad}.v{layer}.r{robin}` with
//!   pad 1–16, velocity layer 1–4 (1 the softest) and round-robin 1–4. A hit picks the layer by its
//!   velocity among the layers that hold samples, then the next round-robin of that layer.
//!   Playback is pitched by Tune and Fine (and the sample's own rate), 4-point cubic, forwards or
//!   reversed. A replaced sample is handed back through [`Plugin::take_released_asset`], so it is
//!   never freed on the audio thread.
//! * **Synth voices** (kick, snare, clap, tom, closed and open hat, cymbal, rim, cowbell, shaker),
//!   shaped by Tone and Snappy; "Both" mixes sample and synth by Blend.
//! * **Shaping** per hit: amp envelope (attack, hold, decay), velocity sensitivity, a pitch
//!   envelope, a low-, high- or band-pass filter with its own envelope, and drive.
//! * **Output:** each pad's level and equal-power pan into one stereo out, plus sends to buses A
//!   and B, which return into the same out (their effect slots come with the hosted effects).
//!   Mute and solo per pad. Humanize varies every hit's level and pitch a little, from a seeded
//!   generator, so a bounce is repeatable.
//!
//! Audio thread: no allocation, no locks, no panics. 64 voices, stolen oldest-first.

use std::f32::consts::{FRAC_1_SQRT_2, TAU};
use std::sync::Arc;

use super::synth::{EventKind, EventQueue, MAX_EVENTS};
use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, choice, lin, log, param_plumbing, toggle};
use crate::util::{Rng, Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, SampleSource, Unit, db_to_gain};

/// Pads in a kit.
pub const PADS: usize = 16;
/// Velocity layers per pad.
pub const LAYERS: usize = 4;
/// Round-robin variations per layer.
pub const ROBINS: usize = 4;
/// Parameters per pad (see [`PadParam`]).
const K: usize = 33;
const VOICES: usize = 64;
const MAX_PENDING: usize = 64;
/// Envelopes below this are silent.
const OFF: f32 = 1.0e-4;
/// Effect slots per bus, and every bus's: bus A's four, bus B's, then the output's
/// ([`Plugin::set_child`] slots `0..FX_SLOTS`).
pub const FX_PER_BUS: usize = 4;
pub const FX_SLOTS: usize = FX_PER_BUS * 3;
/// Frames the work buffers hold before `prepare` sizes them.
const DEFAULT_BLOCK: usize = 512;

/// Where a pad gets its sound.
pub const SOURCES: [&str; 3] = ["Sample", "Synth", "Both"];
/// The synthesized voices.
pub const VOICE_NAMES: [&str; 10] = ["Kick", "Snare", "Clap", "Tom", "Closed hat", "Open hat", "Cymbal", "Rim", "Cowbell", "Shaker"];
const FILTERS: [&str; 4] = ["Off", "Low-pass", "High-pass", "Band-pass"];
const CHOKES: [&str; 9] = ["Off", "1", "2", "3", "4", "5", "6", "7", "8"];
const MODES: [&str; 2] = ["One-shot", "Gate"];
const STACKS: [&str; 17] = [
    "None", "Pad 1", "Pad 2", "Pad 3", "Pad 4", "Pad 5", "Pad 6", "Pad 7", "Pad 8", "Pad 9", "Pad 10", "Pad 11", "Pad 12", "Pad 13", "Pad 14",
    "Pad 15", "Pad 16",
];

/// A pad parameter's offset within the pad's block (`pad * K + offset`).
#[derive(Clone, Copy)]
#[repr(usize)]
enum PadParam {
    Note,
    Source,
    Voice,
    Tone,
    Snappy,
    Blend,
    Tune,
    Fine,
    Attack,
    Hold,
    Decay,
    Velocity,
    PitchAmount,
    PitchDecay,
    Filter,
    Cutoff,
    Resonance,
    FilterEnv,
    FilterDecay,
    Drive,
    Level,
    Pan,
    SendA,
    SendB,
    Choke,
    Mode,
    Reverse,
    Mute,
    Solo,
    Stack1,
    Stack1Ms,
    Stack2,
    Stack2Ms,
}

// Kit-wide parameters, after the pads.
const G_LEVEL: usize = PADS * K;
const G_HUMANIZE: usize = PADS * K + 1;
const G_RETURN_A: usize = PADS * K + 2;
const G_RETURN_B: usize = PADS * K + 3;

/// The parameter table: every pad's block (`p1_…` to `p16_…`, its defaults given per pad), then
/// the kit-wide ones.
macro_rules! juke_table {
    ($( ($n:literal, $note:literal, $voice:literal, $tune:literal, $decay:literal, $choke:literal, $pan:literal) ),* $(,)?) => {
        [
            $(
                lin(concat!("p", $n, "_note"), concat!("Pad ", $n, " Note"), 0.0, 127.0, $note, Unit::None),
                choice(concat!("p", $n, "_source"), concat!("Pad ", $n, " Source"), &SOURCES, 1),
                choice(concat!("p", $n, "_voice"), concat!("Pad ", $n, " Voice"), &VOICE_NAMES, $voice),
                lin(concat!("p", $n, "_tone"), concat!("Pad ", $n, " Tone"), 0.0, 100.0, 50.0, Unit::Percent),
                lin(concat!("p", $n, "_snappy"), concat!("Pad ", $n, " Snappy"), 0.0, 100.0, 60.0, Unit::Percent),
                lin(concat!("p", $n, "_blend"), concat!("Pad ", $n, " Blend"), 0.0, 100.0, 50.0, Unit::Percent),
                lin(concat!("p", $n, "_tune"), concat!("Pad ", $n, " Tune"), -24.0, 24.0, $tune, Unit::Semitones),
                lin(concat!("p", $n, "_fine"), concat!("Pad ", $n, " Fine"), -100.0, 100.0, 0.0, Unit::Cents),
                lin(concat!("p", $n, "_attack"), concat!("Pad ", $n, " Attack"), 0.0, 200.0, 0.0, Unit::Ms),
                lin(concat!("p", $n, "_hold"), concat!("Pad ", $n, " Hold"), 0.0, 500.0, 0.0, Unit::Ms),
                log(concat!("p", $n, "_decay"), concat!("Pad ", $n, " Decay"), 10.0, 5000.0, $decay, Unit::Ms),
                lin(concat!("p", $n, "_velocity"), concat!("Pad ", $n, " Velocity"), 0.0, 100.0, 75.0, Unit::Percent),
                lin(concat!("p", $n, "_pitch_amount"), concat!("Pad ", $n, " Pitch Env"), -24.0, 24.0, 0.0, Unit::Semitones),
                log(concat!("p", $n, "_pitch_decay"), concat!("Pad ", $n, " Pitch Decay"), 1.0, 2000.0, 60.0, Unit::Ms),
                choice(concat!("p", $n, "_filter"), concat!("Pad ", $n, " Filter"), &FILTERS, 0),
                log(concat!("p", $n, "_cutoff"), concat!("Pad ", $n, " Cutoff"), 20.0, 20000.0, 20000.0, Unit::Hz),
                lin(concat!("p", $n, "_resonance"), concat!("Pad ", $n, " Resonance"), 0.0, 100.0, 0.0, Unit::Percent),
                lin(concat!("p", $n, "_filter_env"), concat!("Pad ", $n, " Filter Env"), -4.0, 4.0, 0.0, Unit::None),
                log(concat!("p", $n, "_filter_decay"), concat!("Pad ", $n, " Filter Decay"), 1.0, 2000.0, 120.0, Unit::Ms),
                lin(concat!("p", $n, "_drive"), concat!("Pad ", $n, " Drive"), 0.0, 100.0, 0.0, Unit::Percent),
                lin(concat!("p", $n, "_level"), concat!("Pad ", $n, " Level"), -48.0, 6.0, 0.0, Unit::Db),
                lin(concat!("p", $n, "_pan"), concat!("Pad ", $n, " Pan"), -100.0, 100.0, $pan, Unit::None),
                lin(concat!("p", $n, "_send_a"), concat!("Pad ", $n, " Send A"), 0.0, 100.0, 0.0, Unit::Percent),
                lin(concat!("p", $n, "_send_b"), concat!("Pad ", $n, " Send B"), 0.0, 100.0, 0.0, Unit::Percent),
                choice(concat!("p", $n, "_choke"), concat!("Pad ", $n, " Choke"), &CHOKES, $choke),
                choice(concat!("p", $n, "_mode"), concat!("Pad ", $n, " Play"), &MODES, 0),
                toggle(concat!("p", $n, "_reverse"), concat!("Pad ", $n, " Reverse"), false),
                toggle(concat!("p", $n, "_mute"), concat!("Pad ", $n, " Mute"), false),
                toggle(concat!("p", $n, "_solo"), concat!("Pad ", $n, " Solo"), false),
                choice(concat!("p", $n, "_stack1"), concat!("Pad ", $n, " Stack 1"), &STACKS, 0),
                lin(concat!("p", $n, "_stack1_ms"), concat!("Pad ", $n, " Stack 1 Delay"), 0.0, 50.0, 0.0, Unit::Ms),
                choice(concat!("p", $n, "_stack2"), concat!("Pad ", $n, " Stack 2"), &STACKS, 0),
                lin(concat!("p", $n, "_stack2_ms"), concat!("Pad ", $n, " Stack 2 Delay"), 0.0, 50.0, 0.0, Unit::Ms),
            )*
            lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
            lin("humanize", "Humanize", 0.0, 100.0, 0.0, Unit::Percent),
            lin("return_a", "Return A", -48.0, 6.0, -6.0, Unit::Db),
            lin("return_b", "Return B", -48.0, 6.0, -6.0, Unit::Db),
        ]
    };
}

/// The default kit: General MIDI drum notes, a synth voice each, the hats in choke group 1.
/// (pad, note, voice, tune, decay ms, choke, pan)
static JUKE_PARAMS: [ParamInfo; PADS * K + 4] = juke_table![
    ("1", 36.0, 0, 0.0, 450.0, 0, 0.0),
    ("2", 37.0, 7, 0.0, 60.0, 0, -10.0),
    ("3", 38.0, 1, 0.0, 220.0, 0, 0.0),
    ("4", 39.0, 2, 0.0, 300.0, 0, 5.0),
    ("5", 42.0, 4, 0.0, 70.0, 1, 15.0),
    ("6", 44.0, 4, -2.0, 90.0, 1, 15.0),
    ("7", 46.0, 5, 0.0, 700.0, 1, 15.0),
    ("8", 41.0, 3, -5.0, 500.0, 0, -25.0),
    ("9", 47.0, 3, 0.0, 450.0, 0, 0.0),
    ("10", 50.0, 3, 5.0, 400.0, 0, 25.0),
    ("11", 49.0, 6, 0.0, 1800.0, 0, -20.0),
    ("12", 51.0, 6, -5.0, 1400.0, 0, 20.0),
    ("13", 54.0, 9, 4.0, 250.0, 0, 30.0),
    ("14", 56.0, 8, 0.0, 300.0, 0, -15.0),
    ("15", 70.0, 9, 0.0, 150.0, 0, -30.0),
    ("16", 75.0, 7, 7.0, 80.0, 0, 10.0),
];

pub static JUKE_INFO: PluginInfo = PluginInfo {
    id: "juke",
    name: "Juke — Drum Machine",
    short_name: "Juke",
    category: Category::Instrument,
    params: &JUKE_PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The asset key of a pad's sample (`p3.v2.r1`), each index from 0.
pub fn sample_key(pad: usize, layer: usize, robin: usize) -> String {
    format!("p{}.v{}.r{}", pad + 1, layer + 1, robin + 1)
}

/// The slot a sample key names: (pad, layer, robin), each from 0.
pub fn parse_sample_key(key: &str) -> Option<(usize, usize, usize)> {
    let mut parts = key.split('.');
    let num = |s: Option<&str>, prefix: char, max: usize| -> Option<usize> {
        let n: usize = s?.strip_prefix(prefix)?.parse().ok()?;
        (1..=max).contains(&n).then(|| n - 1)
    };
    let slot = (num(parts.next(), 'p', PADS)?, num(parts.next(), 'v', LAYERS)?, num(parts.next(), 'r', ROBINS)?);
    parts.next().is_none().then_some(slot)
}

/// Coefficient that decays by 60 dB over `ms`.
fn decay60(ms: f32, sr: f32) -> f32 {
    (0.001f32.ln() / ms_to_samples(ms, sr).max(1.0)).exp()
}

fn hermite(x: &[f32], pos: f64) -> f32 {
    let i = pos as usize;
    let t = (pos - i as f64) as f32;
    let at = |k: usize| x.get(k).copied().unwrap_or(0.0);
    let (xm1, x0, x1, x2) = (at(i.saturating_sub(1)), at(i), at(i.saturating_add(1)), at(i.saturating_add(2)));
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    let y = ((c3 * t + c2) * t + c1) * t + x0;
    if y.is_finite() { y } else { 0.0 }
}

/// Inharmonic partials (Hz) of the metallic voices.
const METAL: [f32; 6] = [263.0, 400.0, 421.0, 474.0, 587.0, 845.0];

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    pad: usize,
    age: u64,
    t: u32,
    gain: f32,
    pan_l: f32,
    pan_r: f32,
    send_a: f32,
    send_b: f32,
    // amp envelope
    attack: u32,
    hold: u32,
    env: f32,
    decay: f32,
    releasing: bool,
    release: f32,
    // pitch and filter envelopes
    pitch_amt: f32,
    pitch_env: f32,
    pitch_coef: f32,
    pitch_ratio: f32,
    filter: usize,
    cutoff: f32,
    q: f32,
    fenv_amt: f32,
    fenv: f32,
    fenv_coef: f32,
    filt: Biquad,
    drive: f32,
    // sample part
    sample: Option<(usize, usize)>,
    w_sample: f32,
    pos: f64,
    step: f64,
    reverse: bool,
    // synth part
    synth: usize,
    w_synth: f32,
    tone: f32,
    snappy: f32,
    freq: f32,
    phase: f32,
    phase2: f32,
    sweep: f32,
    sweep_coef: f32,
    noise: f32,
    noise_coef: f32,
    metal: [f32; 6],
    sf1: Biquad,
    sf2: Biquad,
    rng: Rng,
}

impl Voice {
    fn new(seed: u32) -> Voice {
        Voice {
            active: false,
            pad: 0,
            age: 0,
            t: 0,
            gain: 0.0,
            pan_l: 0.0,
            pan_r: 0.0,
            send_a: 0.0,
            send_b: 0.0,
            attack: 0,
            hold: 0,
            env: 0.0,
            decay: 0.0,
            releasing: false,
            release: 0.0,
            pitch_amt: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            pitch_ratio: 1.0,
            filter: 0,
            cutoff: 20_000.0,
            q: 0.7,
            fenv_amt: 0.0,
            fenv: 0.0,
            fenv_coef: 0.0,
            filt: Biquad::default(),
            drive: 0.0,
            sample: None,
            w_sample: 0.0,
            pos: 0.0,
            step: 1.0,
            reverse: false,
            synth: 0,
            w_synth: 0.0,
            tone: 0.5,
            snappy: 0.5,
            freq: 100.0,
            phase: 0.0,
            phase2: 0.0,
            sweep: 0.0,
            sweep_coef: 0.0,
            noise: 0.0,
            noise_coef: 0.0,
            metal: [0.0; 6],
            sf1: Biquad::default(),
            sf2: Biquad::default(),
            rng: Rng::new(seed),
        }
    }
}

/// The Juke drum machine.
pub struct Juke {
    p: Params,
    sr: f32,
    ch: usize,
    voices: [Voice; VOICES],
    events: EventQueue,
    /// Stacked hits waiting their delay: (samples left, pad, velocity).
    pending: [(u32, usize, u8); MAX_PENDING],
    pending_len: usize,
    counter: u64,
    /// The next round-robin, per pad and velocity layer.
    robin: [usize; PADS * LAYERS],
    /// Samples by slot: `(pad * LAYERS + layer) * ROBINS + robin`.
    samples: Vec<Option<Arc<dyn SampleSource>>>,
    /// Samples let go, for the host to drop off the audio thread.
    released: Vec<Arc<dyn SampleSource>>,
    rng: Rng,
    level: Smoother,
    ret_a: Smoother,
    ret_b: Smoother,
    /// Each pad's peak over the block just rendered (before the kit level), for the pad meters.
    pad_peak: [f32; PADS],
    /// Effects hosted on the buses (see [`FX_SLOTS`]).
    fx: [Option<Box<dyn Plugin>>; FX_SLOTS],
    /// Stereo work buffers, one block long: the pads' dry mix, bus A and bus B.
    work: [Vec<Vec<f32>>; 3],
}

/// A pad level in a readout: 0–1 in 4096 steps.
const LEVEL_STEPS: f32 = 4095.0;

/// The pad levels a [`Juke`]'s readouts carry: slot `i` holds pads `2i` and `2i + 1`, each 0–1
/// in 4096 steps (`a × 4096 + b`, exact in an `f32`). NaN or out-of-range slots read as silence.
pub fn unpack_pad_levels(readouts: &[f32; 8]) -> [f32; PADS] {
    let mut out = [0.0; PADS];
    for (i, &v) in readouts.iter().enumerate() {
        if !(v.is_finite() && (0.0..16_777_216.0).contains(&v)) {
            continue;
        }
        let n = v as u32;
        if let Some(a) = out.get_mut(2 * i) {
            *a = (n / 4096) as f32 / LEVEL_STEPS;
        }
        if let Some(b) = out.get_mut(2 * i + 1) {
            *b = (n % 4096) as f32 / LEVEL_STEPS;
        }
    }
    out
}

/// Runs `len` frames of the stereo buffer `io` through `fx` in order (empty slots pass through).
fn run_effects(fx: &mut [Option<Box<dyn Plugin>>], io: &mut [Vec<f32>], len: usize) {
    for f in fx.iter_mut().flatten() {
        f.process(io, len);
    }
}

impl Default for Juke {
    fn default() -> Self {
        Self::new()
    }
}

impl Juke {
    pub fn new() -> Juke {
        let mut voices = [Voice::new(1); VOICES];
        for (i, v) in voices.iter_mut().enumerate() {
            *v = Voice::new(0x5EED_0000 + i as u32 * 7919);
        }
        let mut j = Juke {
            p: Params::new(&JUKE_PARAMS),
            sr: 48_000.0,
            ch: 2,
            voices,
            events: EventQueue::new(),
            pending: [(0, 0, 0); MAX_PENDING],
            pending_len: 0,
            counter: 0,
            robin: [0; PADS * LAYERS],
            samples: vec![None; PADS * LAYERS * ROBINS],
            released: Vec::with_capacity(PADS * LAYERS * ROBINS * 2),
            rng: Rng::new(0x4A55_4B45),
            level: Smoother::new(0.5),
            ret_a: Smoother::new(0.5),
            ret_b: Smoother::new(0.5),
            pad_peak: [0.0; PADS],
            fx: std::array::from_fn(|_| None),
            work: std::array::from_fn(|_| vec![vec![0.0; DEFAULT_BLOCK]; 2]),
        };
        j.update();
        j
    }

    fn update(&mut self) {
        self.level.set(db_to_gain(self.p.v(G_LEVEL)));
        self.ret_a.set(db_to_gain(self.p.v(G_RETURN_A)));
        self.ret_b.set(db_to_gain(self.p.v(G_RETURN_B)));
    }

    #[inline]
    fn pv(&self, pad: usize, k: PadParam) -> f32 {
        self.p.v(pad * K + k as usize)
    }

    #[inline]
    fn pc(&self, pad: usize, k: PadParam) -> usize {
        self.p.choice(pad * K + k as usize)
    }

    fn slot(pad: usize, layer: usize, robin: usize) -> usize {
        (pad * LAYERS + layer) * ROBINS + robin
    }

    fn has_sample(&self, pad: usize, layer: usize, robin: usize) -> bool {
        self.samples.get(Self::slot(pad, layer, robin)).is_some_and(Option::is_some)
    }

    /// The (layer, robin) a hit at `vel` plays on `pad`, if the pad has samples.
    fn pick_sample(&mut self, pad: usize, vel: u8) -> Option<(usize, usize)> {
        let mut filled = [0usize; LAYERS];
        let mut n = 0;
        for layer in 0..LAYERS {
            if (0..ROBINS).any(|r| self.has_sample(pad, layer, r)) {
                if let Some(f) = filled.get_mut(n) {
                    *f = layer;
                }
                n += 1;
            }
        }
        if n == 0 {
            return None;
        }
        let pick = ((usize::from(vel.min(127)) * n) / 128).min(n - 1);
        let layer = filled.get(pick).copied().unwrap_or(0);
        let mut robins = [0usize; ROBINS];
        let mut m = 0;
        for r in 0..ROBINS {
            if self.has_sample(pad, layer, r) {
                if let Some(x) = robins.get_mut(m) {
                    *x = r;
                }
                m += 1;
            }
        }
        let counter = self.robin.get_mut(pad * LAYERS + layer)?;
        let robin = robins.get(*counter % m.max(1)).copied().unwrap_or(0);
        *counter = counter.wrapping_add(1);
        Some((layer, robin))
    }

    fn any_solo(&self) -> bool {
        (0..PADS).any(|pad| self.pv(pad, PadParam::Solo) >= 0.5)
    }

    fn schedule(&mut self, delay: u32, pad: usize, vel: u8) {
        if delay == 0 {
            self.trigger(pad, vel, false);
        } else if let Some(slot) = self.pending.get_mut(self.pending_len) {
            // Counted down from the next frame on.
            *slot = (delay, pad, vel);
            self.pending_len += 1;
        }
    }

    fn note_on_now(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.note_off_now(note);
            return;
        }
        for pad in 0..PADS {
            if self.pv(pad, PadParam::Note).round() as i32 == i32::from(note) {
                self.trigger(pad, vel, true);
            }
        }
    }

    fn note_off_now(&mut self, note: u8) {
        let rel = decay60(40.0, self.sr);
        for pad in 0..PADS {
            if self.pv(pad, PadParam::Note).round() as i32 == i32::from(note) && self.pc(pad, PadParam::Mode) == 1 {
                for v in self.voices.iter_mut().filter(|v| v.active && v.pad == pad) {
                    v.releasing = true;
                    v.release = v.release.min(rel);
                }
            }
        }
    }

    /// Plays `pad` at `vel`; with `stacks`, also fires the pads it stacks.
    fn trigger(&mut self, pad: usize, vel: u8, stacks: bool) {
        if pad >= PADS || vel == 0 {
            return;
        }
        if self.pv(pad, PadParam::Mute) >= 0.5 || (self.any_solo() && self.pv(pad, PadParam::Solo) < 0.5) {
            return;
        }
        let sr = self.sr;
        if stacks {
            for (target, ms) in [(PadParam::Stack1, PadParam::Stack1Ms), (PadParam::Stack2, PadParam::Stack2Ms)] {
                let to = self.pc(pad, target);
                if to > 0 && to - 1 != pad {
                    let delay = ms_to_samples(self.pv(pad, ms), sr).max(0.0) as u32;
                    self.schedule(delay, to - 1, vel);
                }
            }
        }
        // Choke: the group's other pads fade within 4 ms.
        let group = self.pc(pad, PadParam::Choke);
        if group > 0 {
            let fast = decay60(4.0, sr);
            for other in 0..PADS {
                if other != pad && self.pc(other, PadParam::Choke) == group {
                    for v in self.voices.iter_mut().filter(|v| v.active && v.pad == other) {
                        v.releasing = true;
                        v.release = v.release.min(fast);
                    }
                }
            }
        }
        let source = self.pc(pad, PadParam::Source);
        let sample = if source == 1 { None } else { self.pick_sample(pad, vel) };
        let synth_on = source >= 1;
        if sample.is_none() && !synth_on {
            return;
        }
        let blend = self.pv(pad, PadParam::Blend) / 100.0;
        let (w_sample, w_synth) = match (sample.is_some(), synth_on) {
            (true, true) => ((1.0 - blend).sqrt(), blend.sqrt()),
            (true, false) => (1.0, 0.0),
            _ => (0.0, 1.0),
        };
        // Humanize: a little level and pitch per hit.
        let h = self.p.v(G_HUMANIZE) / 100.0;
        let hum_db = h * 2.0 * self.rng.bipolar();
        let hum_cents = h * 20.0 * self.rng.bipolar();
        let sens = self.pv(pad, PadParam::Velocity) / 100.0;
        let vel01 = f32::from(vel.min(127)) / 127.0;
        let gain = db_to_gain(self.pv(pad, PadParam::Level) + hum_db) * (1.0 - sens + sens * vel01);
        let pan = (self.pv(pad, PadParam::Pan) / 100.0).clamp(-1.0, 1.0);
        let angle = (pan + 1.0) * 0.25 * std::f32::consts::PI;
        let semis = self.pv(pad, PadParam::Tune) + (self.pv(pad, PadParam::Fine) + hum_cents) / 100.0;
        let ratio = 2f32.powf(semis / 12.0);
        let voice_kind = self.pc(pad, PadParam::Voice);
        let (tone, snappy) = (self.pv(pad, PadParam::Tone) / 100.0, self.pv(pad, PadParam::Snappy) / 100.0);
        let (filter, cutoff, reso) = (self.pc(pad, PadParam::Filter), self.pv(pad, PadParam::Cutoff), self.pv(pad, PadParam::Resonance) / 100.0);
        let reverse = self.pv(pad, PadParam::Reverse) >= 0.5;
        let step = match sample {
            Some((layer, robin)) => {
                let src_sr = self.samples.get(Self::slot(pad, layer, robin)).and_then(Option::as_ref).map_or(sr, |s| s.sample_rate());
                f64::from(sane_sr(src_sr) / sr * ratio)
            }
            None => 1.0,
        };
        let attack = ms_to_samples(self.pv(pad, PadParam::Attack), sr).max(0.0) as u32;
        let hold = ms_to_samples(self.pv(pad, PadParam::Hold), sr).max(0.0) as u32;
        let decay = decay60(self.pv(pad, PadParam::Decay), sr);
        let (pitch_amt, pitch_coef) = (self.pv(pad, PadParam::PitchAmount), decay60(self.pv(pad, PadParam::PitchDecay), sr));
        let (fenv_amt, fenv_coef) = (self.pv(pad, PadParam::FilterEnv), decay60(self.pv(pad, PadParam::FilterDecay), sr));
        let drive = self.pv(pad, PadParam::Drive) / 100.0;
        let (send_a, send_b) = (self.pv(pad, PadParam::SendA) / 100.0, self.pv(pad, PadParam::SendB) / 100.0);
        self.counter += 1;
        let counter = self.counter;
        let idx = self
            .voices
            .iter()
            .position(|v| !v.active)
            .or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i))
            .unwrap_or(0);
        let Some(v) = self.voices.get_mut(idx) else { return };
        let seed = v.rng.next_u32();
        *v = Voice::new(seed);
        v.active = true;
        v.pad = pad;
        v.age = counter;
        v.gain = if gain.is_finite() { gain } else { 0.0 };
        v.pan_l = angle.cos();
        v.pan_r = angle.sin();
        v.send_a = send_a;
        v.send_b = send_b;
        v.attack = attack;
        v.hold = hold;
        v.env = if attack == 0 { 1.0 } else { 0.0 };
        v.decay = decay;
        v.release = 1.0;
        v.pitch_amt = pitch_amt;
        v.pitch_env = 1.0;
        v.pitch_coef = pitch_coef;
        v.pitch_ratio = 2f32.powf(pitch_amt / 12.0);
        v.filter = filter;
        v.cutoff = cutoff;
        v.q = 0.7 + reso * 9.0;
        v.fenv_amt = fenv_amt;
        v.fenv = 1.0;
        v.fenv_coef = fenv_coef;
        v.drive = drive;
        v.sample = sample;
        v.w_sample = w_sample;
        v.pos = 0.0;
        v.step = step;
        v.reverse = reverse;
        v.w_synth = w_synth;
        v.synth = voice_kind;
        v.tone = tone;
        v.snappy = snappy;
        if synth_on {
            init_synth(v, voice_kind, ratio, sr);
        }
        if v.filter > 0 {
            set_filter(v, sr);
        }
    }

    fn release_all(&mut self, ms: f32) {
        let c = decay60(ms, self.sr);
        for v in self.voices.iter_mut().filter(|v| v.active) {
            v.releasing = true;
            v.release = v.release.min(c);
        }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(n, v) => self.note_on_now(n, v),
            EventKind::Off(n) => self.note_off_now(n),
            EventKind::AllOff => self.release_all(30.0),
            EventKind::Sustain(_) | EventKind::Bend(_) | EventKind::ResetControllers => {}
        }
    }

    /// One frame: (left, right) of the dry pads plus the two bus inputs.
    #[inline]
    fn render(&mut self) -> (f32, f32, f32, f32, f32, f32) {
        let sr = self.sr;
        let (mut l, mut r, mut al, mut ar, mut bl, mut br) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for vi in 0..VOICES {
            let Some(v) = self.voices.get_mut(vi) else { continue };
            if !v.active {
                continue;
            }
            // Amp envelope: attack, hold, then the decay; a release (gate, choke) on top.
            if v.t < v.attack {
                v.env = (v.t + 1) as f32 / v.attack.max(1) as f32;
            } else if v.t >= v.attack.saturating_add(v.hold) {
                v.env *= v.decay;
            }
            if v.releasing {
                v.env *= v.release;
            }
            // Pitch envelope (a ratio that decays back to 1) and filter envelope.
            v.pitch_env *= v.pitch_coef;
            let pitch = if v.pitch_amt == 0.0 { 1.0 } else { 1.0 + (v.pitch_ratio - 1.0) * v.pitch_env };
            v.fenv *= v.fenv_coef;
            let mut y = 0.0;
            let mut sample_done = true;
            if let Some((layer, robin)) = v.sample
                && let Some(Some(src)) = self.samples.get(Self::slot(v.pad, layer, robin))
                && v.pos < src.frames() as f64
            {
                let frames = src.frames();
                let at = if v.reverse { (frames as f64 - 1.0 - v.pos).max(0.0) } else { v.pos };
                let n = src.num_channels().clamp(1, 2);
                let mut s = 0.0;
                for c in 0..n {
                    s += src.channel(c).map_or(0.0, |x| hermite(x, at));
                }
                y += s / n as f32 * v.w_sample;
                v.pos += v.step * f64::from(pitch);
                sample_done = false;
            }
            if v.w_synth > 0.0 {
                y += synth_sample(v, pitch, sr) * v.w_synth;
            }
            if v.filter > 0 {
                if v.fenv_amt != 0.0 && v.t.is_multiple_of(32) {
                    set_filter(v, sr);
                }
                y = v.filt.process_sample(y);
            }
            if v.drive > 0.0 {
                let g = 1.0 + v.drive * 8.0;
                y = (y * g).tanh() / g.tanh();
            }
            let out = y * v.env * v.gain;
            v.t = v.t.saturating_add(1);
            let past_attack = v.t > v.attack;
            if !out.is_finite() || (past_attack && v.env < OFF) || (sample_done && v.w_synth == 0.0) {
                v.active = false;
                continue;
            }
            let (ol, or) = (out * v.pan_l, out * v.pan_r);
            if let Some(pk) = self.pad_peak.get_mut(v.pad) {
                *pk = pk.max(ol.abs()).max(or.abs());
            }
            l += ol;
            r += or;
            al += ol * v.send_a;
            ar += or * v.send_a;
            bl += ol * v.send_b;
            br += or * v.send_b;
        }
        (l, r, al, ar, bl, br)
    }

    fn tick_pending(&mut self) {
        let mut i = 0;
        while i < self.pending_len {
            let due = match self.pending.get_mut(i) {
                Some(slot) => {
                    slot.0 = slot.0.saturating_sub(1);
                    (slot.0 == 0).then_some((slot.1, slot.2))
                }
                None => None,
            };
            if let Some((pad, vel)) = due {
                // Remove by moving the last one in.
                let last = self.pending_len - 1;
                if let Some(&tail) = self.pending.get(last)
                    && let Some(slot) = self.pending.get_mut(i)
                {
                    *slot = tail;
                }
                self.pending_len = last;
                self.trigger(pad, vel, false);
            } else {
                i += 1;
            }
        }
    }
}

/// Sets up `v`'s synth part for `kind`, pitched by `ratio`.
fn init_synth(v: &mut Voice, kind: usize, ratio: f32, sr: f32) {
    let (tone, snappy) = (v.tone, v.snappy);
    v.sweep = 1.0;
    v.noise = 1.0;
    let bp = |f: f32, q: f32| Biquad::design(FilterType::BandPass, f, q, 0.0, sr);
    let hp = |f: f32| Biquad::design(FilterType::HighPass, f, 0.7, 0.0, sr);
    let lp = |f: f32| Biquad::design(FilterType::LowPass, f, 0.7, 0.0, sr);
    match kind {
        0 => {
            // Kick: a falling sine, a click shaped by Tone.
            v.freq = 50.0 * ratio;
            v.sweep_coef = decay60(60.0 + 120.0 * tone, sr);
            v.noise_coef = decay60(5.0, sr);
            v.sf1 = lp(2000.0 + 6000.0 * tone);
        }
        1 => {
            // Snare: a short body and a noise rattle; Snappy sets the rattle, Tone its brightness.
            v.freq = 185.0 * ratio;
            v.sweep_coef = decay60(30.0, sr);
            v.noise_coef = decay60(120.0 + 200.0 * snappy, sr);
            v.sf1 = hp(1200.0);
            v.sf2 = lp(3500.0 + 8000.0 * tone);
        }
        2 => {
            v.freq = 1100.0 * ratio;
            v.noise_coef = decay60(180.0 + 200.0 * snappy, sr);
            v.sweep_coef = decay60(20.0, sr);
            v.sf1 = bp((900.0 + 600.0 * tone) * ratio.sqrt(), 1.4);
        }
        3 => {
            v.freq = 110.0 * ratio;
            v.sweep_coef = decay60(150.0, sr);
            v.noise_coef = decay60(8.0, sr);
            v.sf1 = lp(3000.0 + 3000.0 * tone);
        }
        4..=6 => {
            // Hats and cymbal: a metallic square bank and noise; Tone sets the high-pass.
            v.freq = ratio;
            for (i, m) in v.metal.iter_mut().enumerate() {
                *m = i as f32 * 0.13;
            }
            let cut = if kind == 6 { 3500.0 } else { 5500.0 } + 4000.0 * tone;
            v.sf1 = hp(cut);
            v.sf2 = hp(cut);
            v.noise_coef = 1.0;
        }
        7 => {
            // Rim: a short high sine knock and a click.
            v.freq = 820.0 * ratio;
            v.sweep_coef = decay60(8.0, sr);
            v.noise_coef = decay60(4.0, sr);
            v.sf1 = bp(1700.0 + 1500.0 * tone, 2.0);
        }
        8 => {
            // Cowbell: two square partials through a band-pass.
            v.freq = 540.0 * ratio;
            v.sf1 = bp(2400.0 + 1200.0 * tone, 1.2);
            v.noise_coef = 1.0;
        }
        _ => {
            // Shaker: bright noise with a soft start.
            v.sf1 = hp(5000.0 + 4000.0 * tone);
            v.sf2 = lp(14_000.0);
            v.noise_coef = decay60(40.0 + 80.0 * snappy, sr);
        }
    }
}

/// The synth part's next sample (before the amp envelope).
#[inline]
fn synth_sample(v: &mut Voice, pitch: f32, sr: f32) -> f32 {
    let nyq = sr * 0.45;
    let adv = |phase: &mut f32, f: f32| {
        *phase += (f * pitch).min(nyq) / sr;
        *phase -= phase.floor();
    };
    match v.synth {
        0 | 3 => {
            v.sweep *= v.sweep_coef;
            let f = v.freq * (1.0 + if v.synth == 0 { 2.4 } else { 0.6 } * v.sweep);
            adv(&mut v.phase, f);
            let body = (v.phase * TAU).sin();
            let click = v.sf1.process_sample(v.rng.bipolar()) * v.noise * if v.synth == 0 { 0.2 + 0.6 * v.tone } else { 0.15 };
            v.noise *= v.noise_coef;
            body + click
        }
        1 => {
            v.sweep *= v.sweep_coef;
            adv(&mut v.phase, v.freq * (1.0 + 0.4 * v.sweep));
            let body = (v.phase * TAU).sin() * (0.8 - 0.4 * v.snappy);
            let rattle = v.sf2.process_sample(v.sf1.process_sample(v.rng.bipolar())) * v.noise * (0.4 + v.snappy);
            v.noise *= v.noise_coef;
            body + rattle
        }
        2 => {
            // Three quick bursts, then the tail.
            let gap = (sr * 0.011) as u32;
            let burst = if v.t < gap.saturating_mul(3) && v.t.is_multiple_of(gap.max(1)) { 1.0 } else { 0.0 };
            v.sweep = v.sweep * v.sweep_coef + burst;
            let env = v.sweep.min(1.0).max(if v.t >= gap.saturating_mul(2) { v.noise * 0.6 } else { 0.0 });
            if v.t >= gap.saturating_mul(2) {
                v.noise *= v.noise_coef;
            }
            v.sf1.process_sample(v.rng.bipolar()) * env * 1.8
        }
        4..=6 => {
            let scale = if v.synth == 6 { 1.0 } else { 1.9 } * v.freq;
            let mut metal = 0.0;
            for (m, f) in v.metal.iter_mut().zip(METAL) {
                *m += (f * scale * pitch).min(nyq) / sr;
                *m -= m.floor();
                metal += if *m < 0.5 { 1.0 } else { -1.0 };
            }
            let raw = metal / 6.0 * (1.0 - v.snappy * 0.5) + v.rng.bipolar() * v.snappy * 0.6;
            v.sf2.process_sample(v.sf1.process_sample(raw))
        }
        7 => {
            v.sweep *= v.sweep_coef;
            adv(&mut v.phase, v.freq);
            let knock = (v.phase * TAU).sin() * (0.5 + 0.5 * v.sweep);
            let click = v.sf1.process_sample(v.rng.bipolar()) * v.noise;
            v.noise *= v.noise_coef;
            knock * 0.8 + click
        }
        8 => {
            adv(&mut v.phase, v.freq);
            adv(&mut v.phase2, v.freq * 1.48);
            let sq = |p: f32| if p < 0.5 { 1.0 } else { -1.0 };
            v.sf1.process_sample((sq(v.phase) + sq(v.phase2)) * 0.5) * 1.5
        }
        _ => {
            let onset = (v.t as f32 / (sr * 0.006)).min(1.0);
            let y = v.sf2.process_sample(v.sf1.process_sample(v.rng.bipolar())) * (onset * (0.3 + 0.7 * v.noise));
            v.noise *= v.noise_coef;
            y
        }
    }
}

/// Sets `v`'s filter from its cutoff and filter envelope.
fn set_filter(v: &mut Voice, sr: f32) {
    let kind = match v.filter {
        1 => FilterType::LowPass,
        2 => FilterType::HighPass,
        _ => FilterType::BandPass,
    };
    let f = (v.cutoff * 2f32.powf(v.fenv_amt * v.fenv)).clamp(20.0, sr * 0.45);
    // New coefficients, the filter's state kept (a sweep, not a click).
    v.filt.set(kind, f, v.q, 0.0, sr);
}

impl Plugin for Juke {
    param_plumbing!(JUKE_INFO);

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        let block = max_block.clamp(1, 1 << 16);
        for w in &mut self.work {
            for c in w.iter_mut() {
                c.resize(block, 0.0);
            }
        }
        for s in [&mut self.level, &mut self.ret_a, &mut self.ret_b] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.active = false;
        }
        for fx in self.fx.iter_mut().flatten() {
            fx.reset();
        }
        self.pending_len = 0;
        self.events.clear();
        for s in [&mut self.level, &mut self.ret_a, &mut self.ret_b] {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let mut evs = [(0usize, EventKind::AllOff); MAX_EVENTS];
        let count = {
            let sorted = self.events.sorted();
            for (d, s) in evs.iter_mut().zip(sorted) {
                *d = (s.offset, s.kind);
            }
            sorted.len().min(MAX_EVENTS)
        };
        self.events.clear();
        self.pad_peak = [0.0; PADS];
        let block = self.work.iter().flat_map(|w| w.iter().map(Vec::len)).min().unwrap_or(0);
        if block == 0 {
            return;
        }
        let mut next = 0;
        let mut start = 0;
        while start < frames {
            let len = (frames - start).min(block);
            // The pads, dry and into the buses.
            for k in 0..len {
                let n = start + k;
                if self.pending_len > 0 {
                    self.tick_pending();
                }
                while next < count {
                    let Some(&(off, kind)) = evs.get(next) else { break };
                    if off > n && n + 1 < frames {
                        break;
                    }
                    self.apply(kind);
                    next += 1;
                }
                let (l, r, al, ar, bl, br) = self.render();
                for (w, (x, y)) in self.work.iter_mut().zip([(l, r), (al, ar), (bl, br)]) {
                    if let [wl, wr, ..] = w.as_mut_slice()
                        && let (Some(a), Some(b)) = (wl.get_mut(k), wr.get_mut(k))
                    {
                        *a = x;
                        *b = y;
                    }
                }
            }
            // Each bus through its effects, returned into the mix; the mix through the output's.
            let [dry, bus_a, bus_b] = &mut self.work;
            let (fx_a, rest) = self.fx.split_at_mut(FX_PER_BUS);
            let (fx_b, fx_out) = rest.split_at_mut(FX_PER_BUS);
            run_effects(fx_a, bus_a, len);
            run_effects(fx_b, bus_b, len);
            if let ([dl, dr, ..], [al, ar, ..], [bl, br, ..]) = (dry.as_mut_slice(), bus_a.as_slice(), bus_b.as_slice()) {
                for k in 0..len {
                    let (ga, gb) = (self.ret_a.next(), self.ret_b.next());
                    if let (Some(l), Some(r)) = (dl.get_mut(k), dr.get_mut(k)) {
                        *l += al.get(k).copied().unwrap_or(0.0) * ga + bl.get(k).copied().unwrap_or(0.0) * gb;
                        *r += ar.get(k).copied().unwrap_or(0.0) * ga + br.get(k).copied().unwrap_or(0.0) * gb;
                    }
                }
            }
            run_effects(fx_out, dry, len);
            if let [dl, dr, ..] = dry.as_slice() {
                for k in 0..len {
                    let g = self.level.next();
                    let (mut out_l, mut out_r) = (dl.get(k).copied().unwrap_or(0.0) * g, dr.get(k).copied().unwrap_or(0.0) * g);
                    if !(out_l.is_finite() && out_r.is_finite()) {
                        (out_l, out_r) = (0.0, 0.0);
                    }
                    let n = start + k;
                    match &mut *io {
                        [only] => {
                            if let Some(x) = only.get_mut(n) {
                                *x = (out_l + out_r) * FRAC_1_SQRT_2;
                            }
                        }
                        [left, right, rest @ ..] => {
                            if let Some(x) = left.get_mut(n) {
                                *x = out_l;
                            }
                            if let Some(x) = right.get_mut(n) {
                                *x = out_r;
                            }
                            for b in rest.iter_mut().take(ch.saturating_sub(2)) {
                                if let Some(x) = b.get_mut(n) {
                                    *x = 0.0;
                                }
                            }
                        }
                        [] => {}
                    }
                }
            }
            start += len;
        }
        for &(_, kind) in evs.iter().take(count).skip(next) {
            self.apply(kind);
        }
    }

    fn set_transport(&mut self, transport: &crate::Transport) {
        for fx in self.fx.iter_mut().flatten() {
            fx.set_transport(transport);
        }
    }

    fn set_offline(&mut self, offline: bool) {
        for fx in self.fx.iter_mut().flatten() {
            fx.set_offline(offline);
        }
    }

    fn set_child(&mut self, slot: usize, child: Option<Box<dyn Plugin>>) -> Option<Box<dyn Plugin>> {
        match self.fx.get_mut(slot) {
            Some(at) => std::mem::replace(at, child),
            None => child,
        }
    }

    fn child_mut(&mut self, slot: usize) -> Option<&mut (dyn Plugin + 'static)> {
        self.fx.get_mut(slot)?.as_deref_mut()
    }

    fn tail_samples(&self) -> usize {
        let longest =
            (0..PADS).map(|pad| self.pv(pad, PadParam::Decay) + self.pv(pad, PadParam::Hold) + self.pv(pad, PadParam::Attack)).fold(0.0, f32::max);
        // A bus effect (a reverb, a delay) rings on after the pads.
        let fx = self.fx.iter().flatten().map(|f| f.tail_samples()).max().unwrap_or(0);
        (ms_to_samples(longest + 50.0, self.sr) as usize).saturating_add(fx)
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

    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        let Some((pad, layer, robin)) = parse_sample_key(key) else { return false };
        let Some(slot) = self.samples.get_mut(Self::slot(pad, layer, robin)) else { return false };
        let old = std::mem::replace(slot, asset);
        for v in self.voices.iter_mut().filter(|v| v.active && v.pad == pad && v.sample == Some((layer, robin))) {
            v.active = false;
        }
        if let Some(old) = old {
            if self.released.len() < self.released.capacity() {
                self.released.push(old);
            } else {
                // The host stopped collecting: keep the last reference out of the audio thread's
                // way by leaking it rather than freeing here (never happens with a working host).
                std::mem::forget(old);
            }
        }
        true
    }

    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        self.released.pop()
    }

    /// The pads' peaks of the last block, two per slot (see [`unpack_pad_levels`]).
    fn readouts(&self, out: &mut [f32; 8]) {
        let q = |pad: usize| (self.pad_peak.get(pad).copied().unwrap_or(0.0).clamp(0.0, 1.0) * LEVEL_STEPS).round() as u32;
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = (q(2 * i) * 4096 + q(2 * i + 1)) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SampleBuffer;

    const SR: f32 = 48_000.0;

    fn juke() -> Juke {
        let mut j = Juke::new();
        j.prepare(SR, 512, 2);
        j
    }

    /// Runs `frames` frames in 512-frame blocks, `hits` (offset in the whole run, note, velocity)
    /// delivered in their blocks; returns (left, right).
    fn run(j: &mut Juke, frames: usize, hits: &[(usize, u8, u8)]) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (Vec::new(), Vec::new());
        let mut at = 0;
        while at < frames {
            let n = 512.min(frames - at);
            for &(off, note, vel) in hits {
                if (at..at + n).contains(&off) {
                    j.note_on(off - at, note, vel);
                }
            }
            let mut io = vec![vec![0.0; n]; 2];
            j.process(&mut io, n);
            l.extend_from_slice(&io[0]);
            r.extend_from_slice(&io[1]);
            at += n;
        }
        (l, r)
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    fn set(j: &mut Juke, id: &str, v: f32) {
        assert!(j.set_param(id, v), "{id}");
    }

    fn click(len: usize, value: f32) -> Arc<dyn SampleSource> {
        let mut x = vec![0.0; len];
        if let Some(s) = x.first_mut() {
            *s = value;
        }
        Arc::new(SampleBuffer { sample_rate: SR, channels: vec![x] })
    }

    fn onsets(x: &[f32], threshold: f32) -> Vec<usize> {
        let mut out = Vec::new();
        let mut quiet = true;
        for (i, v) in x.iter().enumerate() {
            if v.abs() > threshold && quiet {
                out.push(i);
                quiet = false;
            } else if v.abs() < threshold * 0.1 {
                quiet = true;
            }
        }
        out
    }

    #[test]
    fn every_default_pad_sounds_and_ends() {
        for pad in 0..PADS {
            let mut j = juke();
            let note = j.pv(pad, PadParam::Note) as u8;
            let (l, r) = run(&mut j, 48_000 * 8, &[(0, note, 110)]);
            let mix: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a + b).collect();
            assert!(peak(&mix[..4800]) > 0.01, "pad {} (note {note}) is silent", pad + 1);
            assert!(peak(&mix[48_000 * 7..]) < 1e-5, "pad {} rings on", pad + 1);
            assert!(mix.iter().all(|v| v.is_finite() && v.abs() < 4.0));
        }
    }

    #[test]
    fn the_closed_hat_chokes_the_open_hat() {
        let mut free = juke();
        let (open, _) = run(&mut free, 24_000, &[(0, 46, 110)]);
        let mut choked = juke();
        let (cut, _) = run(&mut choked, 24_000, &[(0, 46, 110), (4800, 42, 1)]);
        // A velocity-1 closed hat is quiet; past its own short life, only the open hat could sound.
        assert!(peak(&open[12_000..]) > 1e-3, "the open hat rings");
        assert!(peak(&cut[12_000..]) < 1e-5, "cut by its group");
    }

    #[test]
    fn samples_pick_their_velocity_layer_and_round_robin() {
        let mut j = juke();
        set(&mut j, "p1_source", 0.0);
        set(&mut j, "p1_velocity", 0.0);
        assert!(j.set_asset("p1.v1.r1", Some(click(100, 0.25))));
        assert!(j.set_asset("p1.v2.r1", Some(click(100, 0.5))));
        assert!(j.set_asset("p1.v2.r2", Some(click(100, 0.75))));
        let level = |j: &mut Juke, vel: u8| {
            let (l, r) = run(j, 600, &[(0, 36, vel)]);
            l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()))
        };
        let soft = level(&mut j, 20);
        let hard1 = level(&mut j, 120);
        let hard2 = level(&mut j, 120);
        let hard3 = level(&mut j, 120);
        let g = db_to_gain(-6.0) * std::f32::consts::FRAC_1_SQRT_2;
        assert!((soft - 0.25 * g).abs() < 1e-3, "soft layer {soft}");
        assert!(
            (hard1 - 0.5 * g).abs() < 1e-3 && (hard2 - 0.75 * g).abs() < 1e-3 && (hard3 - 0.5 * g).abs() < 1e-3,
            "round-robin {hard1} {hard2} {hard3}"
        );
    }

    #[test]
    fn a_replaced_sample_goes_back_to_the_host() {
        let mut j = juke();
        let a = click(10, 1.0);
        assert!(j.set_asset("p2.v1.r1", Some(Arc::clone(&a))));
        assert!(j.set_asset("p2.v1.r1", Some(click(10, 0.5))));
        assert!(j.set_asset("p2.v1.r1", None));
        assert_eq!(j.take_released_asset().is_some() as u8 + j.take_released_asset().is_some() as u8, 2);
        assert!(j.take_released_asset().is_none());
        for bad in ["p0.v1.r1", "p17.v1.r1", "p1.v5.r1", "p1.v1.r5", "p1.v1", "x1.v1.r1", "p1.v1.r1.x", "ref0"] {
            assert!(!j.set_asset(bad, None), "{bad}");
        }
    }

    #[test]
    fn stacks_fire_after_their_delay() {
        let mut j = juke();
        set(&mut j, "p3_source", 0.0);
        set(&mut j, "p4_source", 0.0);
        assert!(j.set_asset("p3.v1.r1", Some(click(10, 1.0))));
        assert!(j.set_asset("p4.v1.r1", Some(click(10, 1.0))));
        set(&mut j, "p3_stack1", 4.0); // pad 4
        set(&mut j, "p3_stack1_ms", 10.0);
        let (l, _) = run(&mut j, 4800, &[(100, 38, 127)]);
        assert_eq!(onsets(&l, 0.01), vec![100, 100 + 480], "the snare, then its stacked pad 10 ms later");
    }

    #[test]
    fn mute_solo_and_gate() {
        let mut j = juke();
        set(&mut j, "p1_mute", 1.0);
        let (l, _) = run(&mut j, 4800, &[(0, 36, 120)]);
        assert_eq!(peak(&l), 0.0, "a muted pad is silent");
        let mut j = juke();
        set(&mut j, "p3_solo", 1.0);
        let (l, _) = run(&mut j, 4800, &[(0, 36, 120)]);
        assert_eq!(peak(&l), 0.0, "only the soloed pad plays");
        let (l, _) = run(&mut j, 4800, &[(0, 38, 120)]);
        assert!(peak(&l) > 0.01);

        // Gate: a note-off ends a long hit.
        let mut j = juke();
        set(&mut j, "p11_mode", 1.0);
        j.note_on(0, 49, 120);
        let mut io = vec![vec![0.0; 4800]; 2];
        j.process(&mut io, 4800);
        j.note_off(0, 49);
        let mut io = vec![vec![0.0; 9600]; 2];
        j.process(&mut io, 9600);
        assert!(peak(&io[0][4800..]) < 1e-5, "released within 100 ms");
    }

    #[test]
    fn pan_and_mono_fold_down() {
        let mut j = juke();
        set(&mut j, "p1_pan", -100.0);
        let (l, r) = run(&mut j, 4800, &[(0, 36, 120)]);
        assert!(peak(&l) > 0.01 && peak(&r) < 1e-6, "hard left");
        let mut j = Juke::new();
        j.prepare(SR, 512, 1);
        j.note_on(0, 36, 120);
        let mut io = vec![vec![0.0; 4800]];
        j.process(&mut io, 4800);
        assert!(peak(&io[0]) > 0.01);
    }

    #[test]
    fn sends_return_through_the_buses() {
        let dry = {
            let mut j = juke();
            set(&mut j, "p1_source", 0.0);
            assert!(j.set_asset("p1.v1.r1", Some(click(10, 1.0))));
            peak(&run(&mut j, 100, &[(0, 36, 127)]).0)
        };
        let mut j = juke();
        set(&mut j, "p1_source", 0.0);
        set(&mut j, "p1_send_a", 100.0);
        set(&mut j, "return_a", 0.0);
        assert!(j.set_asset("p1.v1.r1", Some(click(10, 1.0))));
        // Let the return's 20 ms glide settle before the hit.
        let wet = peak(&run(&mut j, 4900, &[(4800, 36, 127)]).0[4800..]);
        assert!((wet / dry - 2.0).abs() < 0.01, "dry plus a full send at unity: {}", wet / dry);
    }

    #[test]
    fn bus_effects_shape_their_bus_and_the_output() {
        let gain = |db: f32| {
            let mut g = crate::create("gain").unwrap();
            g.prepare(SR, 512, 2);
            g.set_param("gain", db);
            g
        };
        let hit = |j: &mut Juke| peak(&run(j, 4900, &[(4800, 36, 127)]).0[4800..]);
        let setup = || {
            let mut j = juke();
            set(&mut j, "p1_source", 0.0);
            set(&mut j, "p1_send_a", 100.0);
            set(&mut j, "return_a", 0.0);
            assert!(j.set_asset("p1.v1.r1", Some(click(10, 1.0))));
            j
        };
        let mut j = setup();
        let plain = hit(&mut j);
        // Bus A's effect doubles the bus: dry + 2 × send = 3 × dry, against 2 × dry before.
        let mut j = setup();
        assert!(j.set_child(0, Some(gain(6.0206))).is_none());
        let louder = hit(&mut j);
        assert!((louder / plain - 1.5).abs() < 0.02, "bus effect: {}", louder / plain);
        // An output effect takes the whole kit.
        let mut j = setup();
        assert!(j.set_child(FX_SLOTS - 1, Some(gain(-6.0206))).is_none());
        assert!((hit(&mut j) / plain - 0.5).abs() < 0.02);
        // Its parameters reach it; emptying the slot hands it back; a slot out of range refuses.
        assert!(j.child_mut(FX_SLOTS - 1).is_some_and(|c| c.set_param("gain", 0.0)));
        assert!(j.set_child(FX_SLOTS - 1, None).is_some());
        assert!(j.set_child(FX_SLOTS, Some(gain(0.0))).is_some(), "no such slot: the child comes back");
        assert!(j.child_mut(0).is_none());
    }

    #[test]
    fn processing_keeps_its_buffers() {
        let mut j = juke();
        for pad in 1..=PADS {
            assert!(j.set_asset(&sample_key(pad - 1, 0, 0), Some(click(4800, 0.5))));
        }
        let samples = (j.samples.as_ptr(), j.samples.capacity());
        let released = (j.released.as_ptr(), j.released.capacity());
        let mut io = vec![vec![0.0; 256]; 2];
        for blk in 0..1000usize {
            j.note_on(blk % 256, [36u8, 38, 42, 46, 49][blk % 5], 100);
            j.set_param("p1_source", (blk % 3) as f32);
            j.set_param("humanize", (blk % 100) as f32);
            j.process(&mut io, 256);
            if blk % 50 == 0 {
                j.reset();
            }
        }
        assert_eq!(samples, (j.samples.as_ptr(), j.samples.capacity()));
        assert_eq!(released, (j.released.as_ptr(), j.released.capacity()));
        assert!(io.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn pad_levels_reach_the_readouts() {
        let mut j = juke();
        let mut io = vec![vec![0.0; 2400]; 2];
        j.note_on(0, 38, 120);
        j.process(&mut io, 2400);
        let mut r = [f32::NAN; 8];
        j.readouts(&mut r);
        let levels = unpack_pad_levels(&r);
        assert!(levels[2] > 0.05, "the snare (pad 3) shows: {levels:?}");
        assert!(levels.iter().enumerate().all(|(i, &l)| i == 2 || l == 0.0), "only the snare: {levels:?}");
        assert_eq!(unpack_pad_levels(&[f32::NAN, -1.0, 1e30, 0.0, 0.0, 0.0, 0.0, 0.0]), [0.0; PADS]);
    }

    #[test]
    fn sample_keys_round_trip() {
        for pad in 0..PADS {
            for layer in 0..LAYERS {
                for robin in 0..ROBINS {
                    assert_eq!(parse_sample_key(&sample_key(pad, layer, robin)), Some((pad, layer, robin)));
                }
            }
        }
    }

    #[test]
    fn humanize_is_repeatable() {
        let render = || {
            let mut j = juke();
            set(&mut j, "humanize", 100.0);
            run(&mut j, 9600, &[(0, 38, 100), (4800, 38, 100)]).0
        };
        assert_eq!(render(), render());
    }
}
