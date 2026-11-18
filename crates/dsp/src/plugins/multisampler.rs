//! Pantry - Multisampler (id `multisampler`): plays an instrument recorded note by note, 128
//! voices.
//!
//! * **Zones.** Every sample is a zone: the keys and velocities it answers, the note it was
//!   recorded at, its level and tuning, where in the file it starts and ends, an optional loop,
//!   and what sets it off (a key going down, a key coming up, or the sustain pedal going down or
//!   up). Zones arrive as keyed assets (`Plugin::set_asset`), each key a zone's whole description
//!   ([`ZoneSpec::key`]).
//! * **Parts.** A zone belongs to one of eight parts (a piano's notes, its release samples, its
//!   pedal noises, a pad under it), each with its own level. Zones of different parts that
//!   answer a note sound together; within a part, zones in a sequence (round-robins) take turns.
//! * **Captured instruments** use a shorter key, `n{note}.v{velocity}.r{robin}`: the note the
//!   sample was recorded at, the velocity it was played with and its take (1–4). Their zones are
//!   worked out here: a key plays the nearest recorded note (the lower of two equally near), a
//!   layer covers the velocities from just above the layer below it up to its own, and up to 16
//!   layers a note are kept.
//! * **Velocity.** A zone plays as recorded at its reference velocity (a captured sample's own;
//!   127 for a library's) and is turned down below it by Velocity Curve (the level falls as the
//!   velocity ratio to that power), as far as the zone tracks velocity. Layer Blend mixes in the
//!   layer below over the bottom of each layer's range (equal power), so the tone changes
//!   gradually too.
//! * **Notes** have an amp ADSR; the sustain pedal (controller 64) holds released notes until it
//!   comes up; pitch bend is ±2 semitones. A key struck again releases its sounding note.
//!
//! A replaced sample is handed back through [`Plugin::take_released_asset`], so it is never
//! freed on the audio thread. Audio thread: no allocation, no locks, no panics.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use super::sampler::pan_display;
use super::synth::{Adsr, AdsrRates, Event, EventKind, EventQueue, MAX_EVENTS, Stage};
use crate::params::{Params, lin, log, param_plumbing};
use crate::resample::hermite;
use crate::util::{Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, SampleSource, Taper, Unit, db_to_gain};

/// Notes a captured sample can be recorded at.
pub const NOTES: usize = 128;
/// Velocity layers a captured note holds.
pub const LAYERS: usize = 16;
/// Round-robin takes a captured layer holds.
pub const ROBINS: usize = 4;
/// Zones a library's instrument holds.
pub const ZONES: usize = 4096;
/// Parts a zone can belong to, each with its own level.
pub const PARTS: usize = 8;
/// Captured samples' zones come first, then a library's.
const CAPTURED: usize = NOTES * LAYERS * ROBINS;
const VOICES: usize = 128;
/// The most zones one note sets off.
const STARTS: usize = 32;
/// Fastest playback step in sample frames per output frame (keeps hostile pitch bounded).
const MAX_STEP: f64 = 256.0;
/// The most a hit harder than a zone's reference velocity is turned up.
const MAX_BOOST: f32 = 2.0;
/// A blended layer quieter than this is not worth a voice.
const MIN_BLEND_GAIN: f32 = 0.01;
/// A part at or under this level (dB) is off.
pub const PART_OFF_DB: f32 = -72.0;

/// What sets a zone off.
pub const ON_KEY_DOWN: u8 = 0;
pub const ON_KEY_UP: u8 = 1;
pub const ON_PEDAL_DOWN: u8 = 2;
pub const ON_PEDAL_UP: u8 = 3;

/// Where the sustain pedal must be for a zone to play.
pub const PEDAL_ANY: u8 = 0;
pub const PEDAL_UP: u8 = 1;
pub const PEDAL_DOWN: u8 = 2;

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
/// `part1_level`: the first part's level; the eight follow in order.
const PART_FIRST: usize = 10;

/// A part's level.
const fn part(id: &'static str, name: &'static str) -> ParamInfo {
    ParamInfo {
        id,
        name,
        min: PART_OFF_DB,
        max: 12.0,
        default: 0.0,
        unit: Unit::Db,
        taper: Taper::Linear,
        choices: &[],
        display: None,
        group: "Parts",
    }
}

// New parameters go at the end: sessions and presets store them by id, and the indices above
// are the order of this table.
static MULTI_PARAMS: [ParamInfo; 18] = [
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
    part("part1_level", "Part 1 Level"),
    part("part2_level", "Part 2 Level"),
    part("part3_level", "Part 3 Level"),
    part("part4_level", "Part 4 Level"),
    part("part5_level", "Part 5 Level"),
    part("part6_level", "Part 6 Level"),
    part("part7_level", "Part 7 Level"),
    part("part8_level", "Part 8 Level"),
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

/// The asset key of a captured sample: the note it was recorded at, the velocity it was played
/// with and its round-robin take (from 1).
pub fn sample_key(note: u8, velocity: u8, robin: usize) -> String {
    format!("n{note}.v{velocity}.r{robin}")
}

/// A captured sample key's note (0–127), velocity (1–127) and round-robin index (from 0);
/// `None` for anything else.
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

/// A zone as the instrument plays it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Zone {
    used: bool,
    part: u8,
    lokey: u8,
    hikey: u8,
    root: u8,
    lovel: u8,
    hivel: u8,
    /// Its turn in its part's sequence (0: it plays every time).
    seq: u8,
    trigger: u8,
    gain_db: f32,
    gain: f32,
    cents: f32,
    /// Frames of the sample it plays, `end` 0 for all of it; `loop_end` 0 for no loop.
    start: u32,
    end: u32,
    loop_start: u32,
    loop_end: u32,
    xfade: u32,
    keytrack: bool,
    /// How far the velocity sets its level, 0 to 1.
    veltrack: f32,
    /// The velocity it plays as recorded at.
    ref_vel: u8,
    /// Where the sustain pedal must be for it to play: [`PEDAL_ANY`], [`PEDAL_UP`] or [`PEDAL_DOWN`].
    pedal: u8,
}

impl Zone {
    const fn empty() -> Self {
        Self {
            used: false,
            part: 0,
            lokey: 0,
            hikey: 127,
            root: 60,
            lovel: 1,
            hivel: 127,
            seq: 0,
            trigger: ON_KEY_DOWN,
            gain_db: 0.0,
            gain: 1.0,
            cents: 0.0,
            start: 0,
            end: 0,
            loop_start: 0,
            loop_end: 0,
            xfade: 0,
            keytrack: true,
            veltrack: 1.0,
            ref_vel: 127,
            pedal: PEDAL_ANY,
        }
    }

    /// Whether `trigger` sets it off for `key` at `vel` with the sustain pedal `down` or not
    /// (the pedal's zones answer no key).
    fn answers(&self, trigger: u8, key: u8, vel: u8, down: bool) -> bool {
        self.used
            && self.trigger == trigger
            && (self.pedal == PEDAL_ANY || trigger >= ON_PEDAL_DOWN || (self.pedal == PEDAL_DOWN) == down)
            && (trigger >= ON_PEDAL_DOWN || (self.lokey <= key && key <= self.hikey && self.lovel <= vel && vel <= self.hivel))
    }
}

/// A library's zone, as its asset key spells it.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneSpec {
    /// Which of the instrument's zones it is (below [`ZONES`]).
    pub index: usize,
    /// Its part (below [`PARTS`]) and the part's name (letters, digits and spaces are kept).
    pub part: usize,
    pub part_name: String,
    pub lokey: u8,
    pub hikey: u8,
    /// The note the sample was recorded at.
    pub root: u8,
    pub lovel: u8,
    pub hivel: u8,
    /// Its turn in its part's sequence (0: it plays every time).
    pub seq: u8,
    /// [`ON_KEY_DOWN`], [`ON_KEY_UP`], [`ON_PEDAL_DOWN`] or [`ON_PEDAL_UP`].
    pub trigger: u8,
    pub gain_db: f32,
    pub cents: f32,
    /// Frames of the sample as the session holds it; `end` 0 plays it all, `loop_end` 0 never loops.
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub xfade: u32,
    /// Whether the key transposes it (a pedal noise plays at one pitch).
    pub keytrack: bool,
    /// How far the velocity sets its level, 0 to 1.
    pub veltrack: f32,
    /// The velocity it plays as recorded at.
    pub ref_vel: u8,
    /// Where the sustain pedal must be for it to play: [`PEDAL_ANY`], [`PEDAL_UP`] (a piano's
    /// notes recorded with the dampers down) or [`PEDAL_DOWN`] (with them lifted).
    pub pedal: u8,
}

impl Default for ZoneSpec {
    fn default() -> Self {
        Self {
            index: 0,
            part: 0,
            part_name: String::new(),
            lokey: 0,
            hikey: 127,
            root: 60,
            lovel: 1,
            hivel: 127,
            seq: 0,
            trigger: ON_KEY_DOWN,
            gain_db: 0.0,
            cents: 0.0,
            start: 0,
            end: 0,
            loop_start: 0,
            loop_end: 0,
            xfade: 0,
            keytrack: true,
            veltrack: 1.0,
            ref_vel: 127,
            pedal: PEDAL_ANY,
        }
    }
}

impl ZoneSpec {
    /// The zone's asset key.
    pub fn key(&self) -> String {
        let name: String = self.part_name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ').take(24).collect();
        format!(
            "z{} p{} k{}_{} r{} v{}_{} s{} t{} g{} c{} a{} e{} l{}_{}_{} y{} w{} h{} d{} q{}",
            self.index,
            self.part,
            self.lokey,
            self.hikey,
            self.root,
            self.lovel,
            self.hivel,
            self.seq,
            self.trigger,
            (self.gain_db * 10.0).round() as i32,
            self.cents.round() as i32,
            self.start,
            self.end,
            self.loop_start,
            self.loop_end,
            self.xfade,
            u8::from(self.keytrack),
            (self.veltrack * 100.0).round() as i32,
            self.ref_vel,
            self.pedal,
            name.trim(),
        )
    }

    /// The zone a key spells; `None` for anything else (a captured sample's key too).
    pub fn parse(key: &str) -> Option<ZoneSpec> {
        let (index, z, name) = parse_zone(key)?;
        Some(ZoneSpec {
            index,
            part: usize::from(z.part),
            part_name: name.to_string(),
            lokey: z.lokey,
            hikey: z.hikey,
            root: z.root,
            lovel: z.lovel,
            hivel: z.hivel,
            seq: z.seq,
            trigger: z.trigger,
            gain_db: z.gain_db,
            cents: z.cents,
            start: z.start,
            end: z.end,
            loop_start: z.loop_start,
            loop_end: z.loop_end,
            xfade: z.xfade,
            keytrack: z.keytrack,
            veltrack: z.veltrack,
            ref_vel: z.ref_vel,
            pedal: z.pedal,
        })
    }
}

/// A zone key's index, zone and part name. Does not allocate.
fn parse_zone(key: &str) -> Option<(usize, Zone, &str)> {
    // The name comes last and may hold spaces.
    let (fields, name) = key.split_once(" q").unwrap_or((key, ""));
    let mut z = Zone::empty();
    let mut index = None;
    let range = |text: &str| -> Option<(u8, u8)> {
        let (a, b) = text.split_once('_')?;
        let (a, b): (u8, u8) = (a.parse().ok()?, b.parse().ok()?);
        (a <= 127 && b <= 127).then_some((a.min(b), a.max(b)))
    };
    for field in fields.split(' ') {
        let (tag, value) = (field.get(..1)?, field.get(1..)?);
        match tag {
            "z" => index = value.parse::<usize>().ok().filter(|i| *i < ZONES),
            "p" => z.part = value.parse::<u8>().ok().filter(|p| usize::from(*p) < PARTS)?,
            "k" => (z.lokey, z.hikey) = range(value)?,
            "r" => z.root = value.parse::<u8>().ok().filter(|n| *n <= 127)?,
            "v" => {
                let (lo, hi) = range(value)?;
                (z.lovel, z.hivel) = (lo.max(1), hi.max(1));
            }
            "s" => z.seq = value.parse::<u8>().ok()?.min(63),
            "t" => z.trigger = value.parse::<u8>().ok().filter(|t| *t <= ON_PEDAL_UP)?,
            "g" => z.gain_db = (value.parse::<i32>().ok()? as f32 / 10.0).clamp(-144.0, 24.0),
            "c" => z.cents = (value.parse::<i32>().ok()? as f32).clamp(-4800.0, 4800.0),
            "a" => z.start = value.parse().ok()?,
            "e" => z.end = value.parse().ok()?,
            "l" => {
                let mut n = value.split('_').map(|x| x.parse::<u32>().ok());
                (z.loop_start, z.loop_end, z.xfade) = (n.next()??, n.next()??, n.next()??);
            }
            "y" => z.keytrack = value != "0",
            "w" => z.veltrack = (value.parse::<i32>().ok()? as f32 / 100.0).clamp(0.0, 1.0),
            "h" => z.ref_vel = value.parse::<u8>().ok()?.clamp(1, 127),
            "d" => z.pedal = value.parse::<u8>().ok().filter(|d| *d <= PEDAL_DOWN)?,
            _ => return None,
        }
    }
    z.gain = db_to_gain(z.gain_db);
    z.used = true;
    Some((index?, z, name))
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    active: bool,
    key: u8,
    vel: u8,
    part: u8,
    /// The zone it plays (an index into `Multisampler::zones` and `samples`).
    slot: usize,
    pos: f64,
    /// Sample frames per output frame, before pitch bend.
    step: f64,
    gain: f32,
    env: Adsr,
    /// A note (not a release sample or a pedal noise, which play out whatever the keys do).
    note: bool,
    /// The key is still down.
    down: bool,
    /// The key came up with the pedal down: the pedal holds the note.
    pedalled: bool,
    /// How far into a sample played from disk it has asked for, frames.
    asked: f64,
}

impl Voice {
    const fn new() -> Self {
        Self {
            active: false,
            key: 0,
            vel: 0,
            part: 0,
            slot: 0,
            pos: 0.0,
            step: 1.0,
            gain: 0.0,
            env: Adsr::new(),
            note: false,
            down: false,
            pedalled: false,
            asked: 0.0,
        }
    }

    fn release(&mut self) {
        self.down = false;
        self.pedalled = false;
        if self.note {
            self.env.gate_off();
        }
    }
}

/// Plays a multisampled instrument.
pub struct Multisampler {
    p: Params,
    sr: f32,
    ch: usize,
    /// Every zone: the captured samples' (`(note * LAYERS + layer) * ROBINS + robin`), then a
    /// library's by index. `samples` holds each one's audio.
    zones: Vec<Zone>,
    samples: Vec<Option<Arc<dyn SampleSource>>>,
    /// Each captured layer slot's velocity (`note * LAYERS + layer`); 0 for an empty slot.
    layer_vel: Vec<u8>,
    /// Hits of each key so far in each part, for its sequences.
    turns: Vec<u8>,
    part_gain: [f32; PARTS],
    /// Where voices of samples played from disk read them: what plays, and what a loop fades in.
    windows: [Window; 2],
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
            zones: vec![Zone::empty(); CAPTURED + ZONES],
            samples: (0..CAPTURED + ZONES).map(|_| None).collect(),
            layer_vel: vec![0; NOTES * LAYERS],
            turns: vec![0; PARTS * NOTES],
            part_gain: [1.0; PARTS],
            windows: [Window::new(), Window::new()],
            voices: [Voice::new(); VOICES],
            events: EventQueue::new(),
            rates: AdsrRates::default(),
            level: Smoother::new(1.0),
            pan: Smoother::new(0.0),
            pedal: false,
            bend: 1.0,
            released: Vec::with_capacity(CAPTURED + ZONES + 64),
        };
        s.update();
        s
    }

    fn update(&mut self) {
        let v = |i| self.p.v(i);
        self.rates = AdsrRates::new(v(ATTACK), v(DECAY), v(SUSTAIN) / 100.0, v(RELEASE), self.sr);
        self.level.set(db_to_gain(v(LEVEL)));
        self.pan.set((v(PAN) / 100.0).clamp(-1.0, 1.0));
        for (i, g) in self.part_gain.iter_mut().enumerate() {
            let db = self.p.v(PART_FIRST + i);
            *g = if db <= PART_OFF_DB { 0.0 } else { db_to_gain(db) };
        }
    }

    /// The velocities of a captured note's layer slots (0: empty).
    fn layers(&self, note: usize) -> &[u8] {
        self.layer_vel.get(note * LAYERS..(note + 1) * LAYERS).unwrap_or(&[])
    }

    /// Works out the captured samples' zones: a key plays the nearest recorded note (the lower
    /// of two as near), and a layer the velocities from just above the layer below up to its own.
    fn map_captured(&mut self) {
        let recorded = |s: &Self, note: usize| s.layers(note).iter().any(|&v| v != 0);
        let mut root = [None::<u8>; NOTES];
        let mut below = None;
        for key in 0..NOTES {
            if recorded(self, key) {
                below = Some(key as u8);
            }
            if let Some(r) = root.get_mut(key) {
                *r = below;
            }
        }
        let mut above = None::<u8>;
        for key in (0..NOTES).rev() {
            if recorded(self, key) {
                above = Some(key as u8);
            }
            let Some(r) = root.get_mut(key) else { continue };
            let nearer = match (*r, above) {
                (None, _) => true,
                (Some(b), Some(a)) => usize::from(a) - key < key - usize::from(b),
                _ => false,
            };
            if nearer {
                *r = above;
            }
        }
        for note in 0..NOTES {
            let keys = (0..NOTES).filter(|k| root.get(*k).copied().flatten() == Some(note as u8));
            let (lokey, hikey) = keys.fold((127u8, 0u8), |(lo, hi), k| (lo.min(k as u8), hi.max(k as u8)));
            for layer in 0..LAYERS {
                let vel = self.layers(note).get(layer).copied().unwrap_or(0);
                let under = self.layers(note).iter().copied().filter(|v| *v != 0 && *v < vel).max();
                let top = !self.layers(note).iter().any(|v| *v > vel);
                for robin in 0..ROBINS {
                    let slot = (note * LAYERS + layer) * ROBINS + robin;
                    let held = self.samples.get(slot).is_some_and(Option::is_some);
                    let Some(z) = self.zones.get_mut(slot) else { continue };
                    *z = Zone::empty();
                    if vel != 0 && held {
                        z.used = true;
                        (z.lokey, z.hikey, z.root) = (lokey, hikey, note as u8);
                        (z.lovel, z.hivel) = (under.map_or(1, |u| u + 1), if top { 127 } else { vel });
                        (z.seq, z.ref_vel) = (robin as u8 + 1, vel);
                    }
                }
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

    /// Sets off the zones `trigger` starts for `key` at `vel`: every part's that answer, one of
    /// each part's sequence, and with Layer Blend the layer below.
    fn fire(&mut self, trigger: u8, key: u8, vel: u8) {
        let vel = vel.clamp(1, 127);
        let pedal = self.pedal;
        // Whose turn it is in each part's sequence.
        let mut seqs = [0u64; PARTS];
        for z in self.zones.iter().filter(|z| z.seq > 0 && z.answers(trigger, key, vel, pedal)) {
            if let Some(s) = seqs.get_mut(usize::from(z.part)) {
                *s |= 1u64 << z.seq.min(63);
            }
        }
        let mut turn = [0u8; PARTS];
        for (part, bits) in seqs.iter().enumerate().filter(|(_, b)| **b != 0) {
            let at = part * NOTES + usize::from(key);
            let n = self.turns.get(at).copied().unwrap_or(0);
            if let Some(t) = self.turns.get_mut(at) {
                *t = n.wrapping_add(1);
            }
            let nth = u32::from(n) % bits.count_ones();
            let seq = (0..64u8).filter(|s| bits & (1u64 << s) != 0).nth(nth as usize).unwrap_or(0);
            if let Some(t) = turn.get_mut(part) {
                *t = seq;
            }
        }
        let its_turn = |z: &Zone| z.seq == 0 || turn.get(usize::from(z.part)).copied() == Some(z.seq);
        let curve = self.p.v(CURVE);
        let level = |z: &Zone| {
            let tracked = (f32::from(vel) / f32::from(z.ref_vel.max(1))).powf(curve).min(MAX_BOOST);
            z.gain * (1.0 - z.veltrack + z.veltrack * tracked)
        };
        let blend = self.p.v(BLEND) / 100.0;
        let mut starts = [(0usize, 0.0f32); STARTS];
        let mut count = 0;
        let mut push = |slot: usize, gain: f32| {
            if gain >= MIN_BLEND_GAIN * 0.01
                && let Some(s) = starts.get_mut(count)
            {
                *s = (slot, gain);
                count += 1;
            }
        };
        for (slot, z) in self.zones.iter().enumerate().filter(|(_, z)| z.answers(trigger, key, vel, pedal) && its_turn(z)) {
            let mut gain = level(z);
            // The bottom of a layer's range mixes with the layer just below it in its part.
            if trigger == ON_KEY_DOWN && blend > 0.0 && z.lovel > 1 {
                let floor = f32::from(z.lovel - 1);
                // (a captured top layer answers every harder hit too: its range ends at its own velocity)
                let top = f32::from(z.hivel.min(z.ref_vel)).max(floor + 1.0);
                let place = ((f32::from(vel) - floor) / (top - floor)).clamp(0.0, 1.0);
                let under = self.zones.iter().enumerate().find(|(_, u)| {
                    u.used
                        && u.trigger == ON_KEY_DOWN
                        && u.part == z.part
                        && u.pedal == z.pedal
                        && u.hivel == z.lovel - 1
                        && u.lokey <= key
                        && key <= u.hikey
                        && its_turn(u)
                });
                if let Some((under_slot, u)) = under.filter(|_| place < blend) {
                    let x = place / blend * FRAC_PI_2;
                    if x.cos() >= MIN_BLEND_GAIN {
                        push(under_slot, x.cos() * level(u));
                    }
                    gain *= x.sin();
                    if x.sin() < MIN_BLEND_GAIN {
                        continue;
                    }
                }
            }
            push(slot, gain);
        }
        for &(slot, gain) in starts.iter().take(count) {
            self.begin(trigger, key, vel, slot, gain);
        }
    }

    fn begin(&mut self, trigger: u8, key: u8, vel: u8, slot: usize, gain: f32) {
        let Some(z) = self.zones.get(slot).copied() else { return };
        let Some(rate) = self.samples.get(slot).and_then(Option::as_deref).map(SampleSource::sample_rate) else { return };
        let rate = if rate.is_finite() && rate > 0.0 { f64::from(rate.min(1_536_000.0)) } else { f64::from(self.sr) };
        let played = if z.keytrack { f64::from(key) - f64::from(z.root) } else { 0.0 };
        let semis = played + f64::from(self.p.v(TRANSPOSE)) + (f64::from(self.p.v(FINE)) + f64::from(z.cents)) / 100.0;
        let step = 2f64.powf(semis / 12.0) * rate / f64::from(self.sr);
        // A free voice, or the one heard least: a released note before a held one.
        let loudness = |v: &Voice| if v.active { v.env.level * v.gain * if v.down || v.pedalled { 4.0 } else { 1.0 } } else { -1.0 };
        let Some(v) = self.voices.iter_mut().min_by(|a, b| loudness(a).total_cmp(&loudness(b))) else { return };
        let note = trigger == ON_KEY_DOWN;
        *v = Voice {
            active: true,
            key,
            vel,
            part: z.part,
            slot,
            pos: f64::from(z.start),
            step: if step.is_finite() { step.clamp(0.0, MAX_STEP) } else { 1.0 },
            gain: if gain.is_finite() { gain } else { 0.0 },
            env: Adsr::new(),
            note,
            down: note,
            pedalled: false,
            asked: 0.0,
        };
        v.env.gate_on();
    }

    /// Lets the notes of `key` go, and sets off what a key coming up starts.
    fn key_up(&mut self, key: u8) {
        let Some(vel) = self.voices.iter().find(|v| v.active && v.note && v.down && v.key == key).map(|v| v.vel) else { return };
        let pedal = self.pedal;
        for v in self.voices.iter_mut().filter(|v| v.active && v.note && v.down && v.key == key) {
            if pedal {
                v.down = false;
                v.pedalled = true;
            } else {
                v.release();
            }
        }
        if !pedal {
            self.fire(ON_KEY_UP, key, vel);
        }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(key, vel) => {
                for v in self.voices.iter_mut().filter(|v| v.active && v.note && v.key == key) {
                    v.release();
                }
                self.fire(ON_KEY_DOWN, key, vel);
            }
            EventKind::Off(key) => self.key_up(key),
            EventKind::AllOff => self.voices.iter_mut().for_each(Voice::release),
            EventKind::Sustain(on) => self.set_pedal(on),
            EventKind::Bend(ratio) => self.bend = if ratio.is_finite() && ratio > 0.0 { f64::from(ratio) } else { 1.0 },
            // (no vibrato: the mod wheel does nothing here)
            EventKind::Mod(_) => {}
            EventKind::ResetControllers => {
                self.set_pedal(false);
                self.bend = 1.0;
            }
        }
    }

    fn set_pedal(&mut self, on: bool) {
        if on == self.pedal {
            return;
        }
        self.pedal = on;
        if on {
            self.fire(ON_PEDAL_DOWN, 60, 127);
            return;
        }
        // The notes the pedal was holding come up now.
        let mut held = [0u8; NOTES];
        for v in self.voices.iter_mut().filter(|v| v.active && v.pedalled) {
            if let Some(h) = held.get_mut(usize::from(v.key)) {
                *h = v.vel.max(1);
            }
            v.release();
        }
        for (key, vel) in held.iter().enumerate().filter(|(_, v)| **v != 0) {
            self.fire(ON_KEY_UP, key as u8, *vel);
        }
        self.fire(ON_PEDAL_UP, 60, 127);
    }

    /// Adds the voices' frames `from..to` to `io`.
    fn render(&mut self, io: &mut [Vec<f32>], ch: usize, from: usize, to: usize) {
        let Some((left, rest)) = io.split_first_mut() else { return };
        let (Some(left), right) = (left.get_mut(from..to), rest.first_mut().filter(|_| ch >= 2).and_then(|r| r.get_mut(from..to))) else { return };
        let mut right = right;
        let [win, lead_win] = &mut self.windows;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            let (Some(src), Some(z)) = (self.samples.get(v.slot).and_then(Option::as_deref), self.zones.get(v.slot)) else {
                v.active = false;
                continue;
            };
            let streamed = src.stream_channels();
            let (tap, frames) = match (streamed, src.channel(0)) {
                (0, Some(l)) => {
                    let r = src.channel(1).unwrap_or(l);
                    (Tap::Memory(l, r), l.len().min(r.len()))
                }
                (0, None) => {
                    v.active = false;
                    continue;
                }
                (n, _) => (Tap::Disk(src, n >= 2), src.stream_frames()),
            };
            let end = if z.end > 0 { (z.end as usize).min(frames) } else { frames } as f64;
            // A loop inside the part that plays, long enough to step through.
            let (ls, le) = (f64::from(z.loop_start), f64::from(z.loop_end).min(end));
            let looped = z.loop_end > 0 && le - ls >= 2.0;
            let xfade = f64::from(z.xfade).min(ls).min(le - ls);
            let part = self.part_gain.get(usize::from(v.part)).copied().unwrap_or(1.0);
            let step = (v.step * self.bend).clamp(0.0, MAX_STEP);
            win.len = 0;
            lead_win.len = 0;
            if streamed > 0 && v.pos + READ_AHEAD as f64 / 2.0 > v.asked {
                // What it plays next is asked for well before it gets there.
                src.want(v.pos as usize, READ_AHEAD);
                v.asked = v.pos + READ_AHEAD as f64;
            }
            for n in 0..left.len() {
                let g = v.env.next(&self.rates) * v.gain * part;
                if v.env.stage == Stage::Idle || v.pos >= end {
                    v.active = false;
                    break;
                }
                // Frames the rest of this block reads from here on.
                let span = ((left.len() - n) as f64 * step) as usize + 4;
                let (mut a, mut b) = tap.at(win, v.pos, span);
                // Nearing the loop's end, what leads into its start fades in over what leaves.
                if looped && xfade >= 1.0 && v.pos > le - xfade {
                    let t = ((v.pos - (le - xfade)) / xfade) as f32;
                    let (la, lb) = tap.at(lead_win, v.pos - (le - ls), span);
                    a = a * (1.0 - t) + la * t;
                    b = b * (1.0 - t) + lb * t;
                }
                let (a, b) = (a * g, b * g);
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
                if looped && v.pos >= le {
                    v.pos -= le - ls;
                    v.asked = 0.0;
                }
            }
        }
    }
}

/// Frames of a sample played from disk that a voice asks for ahead of where it plays.
const READ_AHEAD: usize = 1 << 16;
/// Frames a [`Window`] holds at most.
const WINDOW: usize = 4096;

/// A stretch of a sample played from disk, copied out for one voice to read.
struct Window {
    l: Vec<f32>,
    r: Vec<f32>,
    start: usize,
    len: usize,
}

impl Window {
    fn new() -> Self {
        Self { l: vec![0.0; WINDOW], r: vec![0.0; WINDOW], start: 0, len: 0 }
    }
}

/// Where a voice reads its sample: in memory, or from disk through a [`Window`].
enum Tap<'a> {
    Memory(&'a [f32], &'a [f32]),
    /// The source, and whether it has a second channel.
    Disk(&'a dyn SampleSource, bool),
}

impl Tap<'_> {
    /// Both channels at `pos`. A window that does not hold `pos` is filled from there with the
    /// `span` frames the voice reads next (what is not loaded yet plays as silence).
    #[inline]
    fn at(&self, w: &mut Window, pos: f64, span: usize) -> (f32, f32) {
        match self {
            Tap::Memory(l, r) => (hermite(l, pos), hermite(r, pos)),
            Tap::Disk(src, stereo) => {
                let i = pos as usize;
                let inside = w.len > 0 && i >= w.start.saturating_add(usize::from(w.start > 0)) && i.saturating_add(2) < w.start + w.len;
                if !inside {
                    w.start = i.saturating_sub(1);
                    w.len = span.saturating_add(4).clamp(8, WINDOW);
                    if let Some(l) = w.l.get_mut(..w.len) {
                        src.read(0, w.start, l);
                    }
                    if let (true, Some(r)) = (*stereo, w.r.get_mut(..w.len)) {
                        src.read(1, w.start, r);
                    }
                }
                let at = pos - w.start as f64;
                let l = hermite(w.l.get(..w.len).unwrap_or_default(), at);
                if *stereo { (l, hermite(w.r.get(..w.len).unwrap_or_default(), at)) } else { (l, l) }
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
        self.turns.iter_mut().for_each(|t| *t = 0);
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
        let captured = parse_sample_key(key);
        let slot = match (captured, parse_zone(key)) {
            // A captured sample: its note's layer at this velocity, or (for a new one) a free layer.
            (Some((note, velocity, robin)), _) => {
                let at = |v: u8| self.layers(note).iter().position(|&l| l == v);
                let Some(layer) = at(velocity).or_else(|| asset.as_ref().and_then(|_| at(0))) else {
                    // Nothing to clear, or a 17th layer: the sample is not kept.
                    if let Some(a) = asset {
                        self.let_go(a);
                    }
                    return true;
                };
                (note * LAYERS + layer) * ROBINS + robin
            }
            (None, Some((index, zone, _))) => {
                let slot = CAPTURED + index;
                if let Some(z) = self.zones.get_mut(slot) {
                    *z = Zone { used: asset.is_some(), ..zone };
                }
                slot
            }
            (None, None) => return false,
        };
        let Some(held) = self.samples.get_mut(slot) else { return false };
        let old = std::mem::replace(held, asset);
        for v in self.voices.iter_mut().filter(|v| v.slot == slot) {
            v.active = false;
        }
        if let Some(old) = old {
            self.let_go(old);
        }
        if let Some((note, velocity, _)) = captured {
            let layer = slot / ROBINS % LAYERS;
            let base = (note * LAYERS + layer) * ROBINS;
            let kept = (0..ROBINS).any(|r| self.samples.get(base + r).is_some_and(Option::is_some));
            if let Some(v) = self.layer_vel.get_mut(note * LAYERS + layer) {
                *v = if kept { velocity } else { 0 };
            }
            self.map_captured();
        }
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

    /// A sample played from disk: no `channel`, read a stretch at a time.
    struct Streamed(SampleBuffer);

    impl SampleSource for Streamed {
        fn sample_rate(&self) -> f32 {
            self.0.sample_rate
        }
        fn num_channels(&self) -> usize {
            0
        }
        fn channel(&self, _ch: usize) -> Option<&[f32]> {
            None
        }
        fn stream_channels(&self) -> usize {
            self.0.channels.len()
        }
        fn stream_frames(&self) -> usize {
            self.0.channels.first().map_or(0, Vec::len)
        }
        fn read(&self, ch: usize, start: usize, out: &mut [f32]) -> bool {
            self.0.read(ch, start, out)
        }
    }

    #[test]
    fn a_zone_for_one_pedal_position_plays_only_there() {
        let mut s = Multisampler::new();
        s.prepare(SR, 512, 2);
        let zone = |index, pedal| ZoneSpec { index, lokey: 0, hikey: 127, root: 60, lovel: 1, hivel: 127, pedal, ..ZoneSpec::default() };
        assert!(s.set_asset(&zone(0, PEDAL_UP).key(), Some(sine(300.0, 0.5, SR as usize))));
        assert!(s.set_asset(&zone(1, PEDAL_DOWN).key(), Some(sine(700.0, 0.5, SR as usize))));
        let out = held(&mut s, 60, 127);
        assert!(partial(&out, 300.0) > 0.3 && partial(&out, 700.0) < 0.01, "pedal up");
        s.note_off(0, 60);
        run(&mut s, 48_000);
        s.midi(0, [0xB0, 64, 127]);
        let out = held(&mut s, 60, 127);
        assert!(partial(&out, 700.0) > 0.3 && partial(&out, 300.0) < 0.01, "pedal down");
    }

    #[test]
    fn a_sample_played_from_disk_sounds_as_one_in_memory() {
        // Two channels that differ, a loop with a crossfade, and a key off the recorded note.
        let len = 30_000usize;
        let tone = |hz: f32| -> Vec<f32> {
            (0..len).map(|i| 0.4 * (std::f32::consts::TAU * hz * i as f32 / SR).sin() * (1.0 - i as f32 / len as f32)).collect()
        };
        let buf = SampleBuffer { sample_rate: SR, channels: vec![tone(220.0), tone(331.0)] };
        let zone = ZoneSpec {
            index: 0,
            lokey: 0,
            hikey: 127,
            root: 60,
            lovel: 1,
            hivel: 127,
            loop_start: 9_000,
            loop_end: 21_000,
            xfade: 3_000,
            ..ZoneSpec::default()
        };
        let play = |source: Arc<dyn SampleSource>| {
            let mut s = Multisampler::new();
            s.prepare(SR, 512, 2);
            assert!(s.set_asset(&zone.key(), Some(source)));
            s.note_on(0, 67, 100);
            let mut out = run(&mut s, 40_000);
            s.note_off(0, 67);
            let tail = run(&mut s, 4_000);
            out[0].extend(tail[0].iter());
            out[1].extend(tail[1].iter());
            out
        };
        let memory = play(Arc::new(buf.clone()));
        let disk = play(Arc::new(Streamed(buf)));
        assert!(peak(&memory[0]) > 0.05 && peak(&memory[1]) > 0.05);
        assert_eq!(memory[0], disk[0]);
        assert_eq!(memory[1], disk[1]);
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
        assert!(s.layers(60).iter().all(|&v| v == 0) && !s.zones.iter().any(|z| z.used));
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

    /// A zone of a library, with `set` filling in what differs from a whole-keyboard zone.
    fn zone(s: &mut Multisampler, index: usize, hz: f32, amp: f32, set: impl FnOnce(&mut ZoneSpec)) {
        let mut spec = ZoneSpec { index, ..ZoneSpec::default() };
        set(&mut spec);
        assert!(s.set_asset(&spec.key(), Some(sine(hz, amp, SR as usize))), "{}", spec.key());
    }

    #[test]
    fn zone_keys_spell_a_zone_out_and_read_back() {
        let spec = ZoneSpec {
            index: 41,
            part: 3,
            part_name: "P LAYER (notes)!".into(),
            lokey: 30,
            hikey: 34,
            root: 34,
            lovel: 1,
            hivel: 70,
            seq: 2,
            trigger: ON_KEY_UP,
            gain_db: 8.5,
            cents: -19.0,
            start: 27_584,
            end: 1_211_610,
            loop_start: 100,
            loop_end: 900,
            xfade: 50,
            keytrack: false,
            veltrack: 0.25,
            ref_vel: 100,
            pedal: PEDAL_UP,
        };
        let key = spec.key();
        assert_eq!(key, "z41 p3 k30_34 r34 v1_70 s2 t1 g85 c-19 a27584 e1211610 l100_900_50 y0 w25 h100 d1 qP LAYER notes");
        assert_eq!(ZoneSpec::parse(&key), Some(ZoneSpec { part_name: "P LAYER notes".into(), ..spec }));
        assert_eq!(ZoneSpec::parse(&ZoneSpec::default().key()), Some(ZoneSpec::default()));
        for bad in ["", "z5000 p0", "z1 p8", "z1 k10_200", "z1 x3", "p1 k1_2", "n60.v100.r1", "z1 t9"] {
            assert_eq!(ZoneSpec::parse(bad), None, "{bad}");
        }
        assert!(parse_sample_key(&key).is_none());
    }

    #[test]
    fn a_zone_answers_its_keys_and_velocities_at_its_level_and_tuning() {
        let mut s = multi(&[]);
        s.set_param("velocity_curve", 0.0);
        zone(&mut s, 0, 400.0, 0.5, |z| (z.lokey, z.hikey, z.root, z.hivel) = (58, 60, 60, 70));
        zone(&mut s, 1, 400.0, 0.5, |z| (z.lokey, z.hikey, z.root, z.lovel, z.gain_db, z.cents) = (58, 60, 60, 71, -6.0, 100.0));
        // Inside its keys, pitched from its root; outside them, silence.
        assert!((freq(&held(&mut s, 60, 50)) - 400.0).abs() < 6.0);
        assert!((freq(&held(&mut s, 58, 50)) - 400.0 * 2f32.powf(-2.0 / 12.0)).abs() < 6.0);
        assert_eq!(peak(&held(&mut s, 61, 50)), 0.0);
        // The harder zone: 6 dB down and a semitone up.
        let out = held(&mut s, 60, 100);
        assert!((peak(&out) - 0.25).abs() < 0.01 && (freq(&out) - 400.0 * 2f32.powf(1.0 / 12.0)).abs() < 6.0);
    }

    #[test]
    fn a_librarys_zones_follow_the_velocity_from_full_scale() {
        let mut s = multi(&[]);
        s.set_param("velocity_curve", 2.0);
        zone(&mut s, 0, 400.0, 0.5, |z| z.hivel = 70);
        zone(&mut s, 1, 400.0, 0.5, |z| (z.lovel, z.veltrack) = (71, 0.5));
        // As recorded at 127, falling with the curve: no step up from one layer to the next.
        assert!((peak(&held(&mut s, 60, 64)) - 0.5 * (64.0f32 / 127.0).powi(2)).abs() < 0.01);
        // Half tracked: halfway between as recorded and fully turned down.
        assert!((peak(&held(&mut s, 60, 80)) - 0.5 * (0.5 + 0.5 * (80.0f32 / 127.0).powi(2))).abs() < 0.01);
    }

    #[test]
    fn parts_sound_together_each_at_its_own_level() {
        let mut s = multi(&[]);
        s.set_param("velocity_curve", 0.0);
        zone(&mut s, 0, 300.0, 0.4, |z| z.part = 0);
        zone(&mut s, 1, 500.0, 0.4, |z| z.part = 1);
        let out = held(&mut s, 60, 100);
        assert!((partial(&out, 300.0) - 0.4).abs() < 0.02 && (partial(&out, 500.0) - 0.4).abs() < 0.02);
        s.set_param("part2_level", -6.0);
        let out = held(&mut s, 60, 100);
        assert!((partial(&out, 300.0) - 0.4).abs() < 0.02 && (partial(&out, 500.0) - 0.2).abs() < 0.02);
        s.set_param("part2_level", PART_OFF_DB);
        assert!(partial(&held(&mut s, 60, 100), 500.0) < 0.002, "a part all the way down is off");
    }

    #[test]
    fn a_parts_sequence_takes_turns_while_other_parts_play_every_time() {
        let mut s = multi(&[]);
        s.set_param("velocity_curve", 0.0);
        zone(&mut s, 0, 300.0, 0.4, |z| z.seq = 1);
        zone(&mut s, 1, 500.0, 0.4, |z| z.seq = 2);
        zone(&mut s, 2, 900.0, 0.2, |z| z.part = 1);
        for expect in [300.0, 500.0, 300.0] {
            let out = held(&mut s, 60, 100);
            let other = if expect == 300.0 { 500.0 } else { 300.0 };
            assert!(partial(&out, expect) > 0.35 && partial(&out, other) < 0.01 && partial(&out, 900.0) > 0.15, "{expect}");
        }
    }

    #[test]
    fn layer_blend_mixes_a_librarys_layers_within_a_part() {
        let mut s = multi(&[]);
        s.set_param("velocity_curve", 0.0);
        s.set_param("layer_blend", 100.0);
        zone(&mut s, 0, 300.0, 0.5, |z| z.hivel = 70);
        zone(&mut s, 1, 500.0, 0.5, |z| z.lovel = 71);
        zone(&mut s, 2, 900.0, 0.5, |z| (z.part, z.hivel) = (1, 70));
        // Just above the lower layer: mostly the lower layer, of the same part only.
        let out = held(&mut s, 60, 75);
        assert!(
            partial(&out, 300.0) > partial(&out, 500.0) * 2.0 && partial(&out, 900.0) < 0.01,
            "{} {}",
            partial(&out, 300.0),
            partial(&out, 500.0)
        );
        // At the top of the range, the upper layer alone.
        let out = held(&mut s, 60, 127);
        assert!(partial(&out, 300.0) < 0.01 && (partial(&out, 500.0) - 0.5).abs() < 0.02);
    }

    #[test]
    fn a_zone_plays_its_part_of_the_sample_and_loops() {
        let mut s = multi(&[]);
        s.set_param("velocity_curve", 0.0);
        // Silence for 0.1 s, then a tone: the zone starts at the tone, and stops 0.05 s in.
        let mut data = vec![0.0f32; 4800];
        data.extend((0..43_200).map(|i| 0.5 * (std::f32::consts::TAU * 400.0 * i as f32 / SR).sin()));
        let spec = ZoneSpec { start: 4800, end: 7200, ..ZoneSpec::default() };
        s.set_asset(&spec.key(), Some(Arc::new(SampleBuffer { sample_rate: SR, channels: vec![data.clone()] })));
        s.note_on(0, 60, 100);
        let out = run(&mut s, 4800);
        assert!(peak(&out[0][..240]) > 0.2, "no wait for the silence before the start");
        assert_eq!(peak(&out[0][2500..]), 0.0, "nothing past the end");
        // With a loop it sounds for as long as the key is held.
        let spec = ZoneSpec { start: 4800, loop_start: 9600, loop_end: 14_400, xfade: 480, ..ZoneSpec::default() };
        s.set_asset(&spec.key(), Some(Arc::new(SampleBuffer { sample_rate: SR, channels: vec![data] })));
        s.note_on(0, 60, 100);
        run(&mut s, 96_000);
        let out = run(&mut s, 9600);
        assert!(peak(&out[0]) > 0.4 && (freq(&out[0]) - 400.0).abs() < 8.0);
    }

    #[test]
    fn release_samples_and_pedal_noises_play_when_keys_and_the_pedal_move() {
        let mut s = multi(&[]);
        s.set_param("velocity_curve", 0.0);
        s.set_param("release", 5.0);
        zone(&mut s, 0, 300.0, 0.4, |_| {});
        zone(&mut s, 1, 700.0, 0.4, |z| (z.part, z.trigger) = (1, ON_KEY_UP));
        zone(&mut s, 2, 1100.0, 0.4, |z| (z.part, z.trigger, z.keytrack, z.root) = (2, ON_PEDAL_DOWN, false, 0));
        zone(&mut s, 3, 1500.0, 0.4, |z| (z.part, z.trigger, z.keytrack, z.root) = (2, ON_PEDAL_UP, false, 0));
        // Held: the note alone. Let go: the release sample.
        s.note_on(0, 60, 100);
        let out = run(&mut s, 4800);
        assert!(partial(&out[0], 300.0) > 0.3 && partial(&out[0], 700.0) < 0.01);
        s.note_off(0, 60);
        let out = run(&mut s, 4800);
        assert!(partial(&out[0][960..], 700.0) > 0.3 && partial(&out[0][960..], 300.0) < 0.02);
        run(&mut s, 96_000);
        // A key let go that was not sounding sets nothing off.
        s.note_off(0, 60);
        assert_eq!(peak(&run(&mut s, 2400)[0]), 0.0);
        // The pedal: its own noise each way, at its own pitch; a note it holds releases with it.
        s.midi(0, [0xB0, 64, 127]);
        let out = run(&mut s, 4800);
        assert!(partial(&out[0], 1100.0) > 0.3 && partial(&out[0], 1500.0) < 0.01);
        run(&mut s, 96_000);
        s.note_on(0, 60, 100);
        s.note_off(0, 60);
        let out = run(&mut s, 4800);
        assert!(partial(&out[0], 300.0) > 0.3 && partial(&out[0], 700.0) < 0.01, "held by the pedal: no release sample yet");
        s.midi(0, [0xB0, 64, 0]);
        let out = run(&mut s, 4800);
        assert!(partial(&out[0][960..], 700.0) > 0.3 && partial(&out[0][960..], 1500.0) > 0.3);
        // The pedal noise's part turned down takes it out.
        run(&mut s, 96_000);
        s.set_param("part3_level", PART_OFF_DB);
        s.midi(0, [0xB0, 64, 127]);
        assert!(peak(&run(&mut s, 4800)[0]) < 1.0e-4);
    }
}
