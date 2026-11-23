//! Auto Pitch: real-time pitch correction for a recorded voice (one voice at a time). FreeDAWm's
//! own design, from the feasibility study's prototype.
//!
//! - **Detect** ([`detect`]): the pitch of the mono sum, every detection hop (about 2.7 ms), from
//!   input that has arrived only.
//! - **Decide** ([`correct`]): the nearest enabled note of the key (the song's Key ruler at that
//!   point, key changes included, or the module's own key, scale and twelve note switches), held
//!   until the voice has clearly moved (Note hold), approached at the Retune speed (more gently
//!   on long notes with Humanize), against the note's centre (Keep vibrato) or the instant pitch
//!   (flattened). Unsure, unvoiced or breathy moments are let through untouched.
//! - **Move** ([`studio`]): Studio mode, the Vocal Editor's own shifter run block by block: about
//!   43 ms of delay, reported through [`Plugin::latency`] so the mixer lines everything else up;
//!   formants kept (or shifted by Formant); every channel moved by the one correction, so a stereo
//!   voice keeps its image.
//! - **Mix**: the corrected voice against the delayed input (0 %: the delayed input exactly).
//!
//! The engine behind "Move" is one of a kind of its own ([`studio::Studio`]); a low-delay engine
//! for singing through it live can sit beside it behind a switch, sharing the detector, the
//! corrector, the key and the controls ([`front::Front`]).
//!
//! Style sets the character in one control: Natural (gentle, keeps the vibrato) and Hard tune
//! (instant, flattened) use their own Retune speed, Humanize, Keep vibrato and Note hold; Custom
//! uses those four controls as set.
//!
//! Every buffer is made in `prepare`; `process` never allocates, locks or panics.
//! `readouts`: `[0]` sung pitch (MIDI cents, NaN when letting the voice through), `[1]` the pitch
//! it is moved to, `[2]` the note aimed at, `[3]` the key's tonic (pitch class), `[4]` the enabled
//! notes (bit per pitch class), `[5]` input peak dBFS, `[6]` delay (ms).

mod correct;
mod detect;
mod front;
pub mod presets;
mod studio;

use crate::params::{Params, choice, p, toggle};
use crate::util::{Smoother, sane_channels, sane_sr};
use crate::{Category, MAX_SONG_KEYS, ParamInfo, Plugin, PluginInfo, SongKey, Taper, Transport, Unit, gain_to_db};
use correct::Settings;
use front::Front;
use studio::Studio;

/// The scales the Scale control offers: (name, bit per semitone above the key's tonic). The same
/// table as the MIDI Scale effect's (`soundcraft_model::SCALES`; a test in the mix keeps them
/// equal), then Custom (the twelve note switches as set).
pub const SCALES: [(&str, u16); 12] = [
    ("Major", 0b1010_1011_0101),
    ("Natural Minor", 0b0101_1010_1101),
    ("Harmonic Minor", 0b1001_1010_1101),
    ("Melodic Minor", 0b1010_1010_1101),
    ("Dorian", 0b0110_1010_1101),
    ("Phrygian", 0b0101_1010_1011),
    ("Lydian", 0b1010_1101_0101),
    ("Mixolydian", 0b0110_1011_0101),
    ("Major Pentatonic", 0b0010_1001_0101),
    ("Minor Pentatonic", 0b0100_1010_1001),
    ("Blues", 0b0100_1110_1001),
    ("Chromatic", 0b1111_1111_1111),
];

/// The Scale control's choices: [`SCALES`] then Custom.
const SCALE_CHOICES: [&str; 13] = [
    "Major",
    "Natural Minor",
    "Harmonic Minor",
    "Melodic Minor",
    "Dorian",
    "Phrygian",
    "Lydian",
    "Mixolydian",
    "Major Pentatonic",
    "Minor Pentatonic",
    "Blues",
    "Chromatic",
    "Custom",
];
/// The Scale choice that uses the note switches as set.
pub const SCALE_CUSTOM: usize = 12;

/// The Key control's choices: the song's Key ruler, or a tonic of its own.
const KEY_CHOICES: [&str; 13] = ["Follow the song", "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
/// Note names by pitch class, as the editor and the note switches show them.
pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// The Style control: one switch for the character.
const STYLE_CHOICES: [&str; 3] = ["Natural", "Hard tune", "Custom"];
pub const STYLE_NATURAL: usize = 0;
pub const STYLE_HARD: usize = 1;
pub const STYLE_CUSTOM: usize = 2;

/// What each Style sets: (retune ms, humanize %, keep vibrato, note hold cents). Natural is the
/// Gentle preset's; Hard tune snaps at once and flattens, with enough Note hold that a wide
/// vibrato sung next to a semitone neighbour does not flip between the two.
pub const STYLE_VALUES: [(f32, f32, bool, f32); 2] = [(80.0, 50.0, true, 40.0), (0.0, 0.0, false, 45.0)];

fn retune_text(v: f32) -> String {
    if v < 0.5 { "Instant".into() } else { format!("{v:.0} ms") }
}

fn formant_text(v: f32) -> String {
    if v.abs() < 0.05 { "Keep".into() } else { format!("{v:+.1} st") }
}

fn hold_text(v: f32) -> String {
    format!("{v:.0} c")
}

const fn disp(mut info: ParamInfo, f: fn(f32) -> String) -> ParamInfo {
    info.display = Some(f);
    info
}

const fn note(id: &'static str, name: &'static str, on: bool) -> ParamInfo {
    toggle(id, name, on)
}

static AUTOPITCH_PARAMS: [ParamInfo; 21] = [
    choice("style", "Style", &STYLE_CHOICES, STYLE_NATURAL),
    choice("key", "Key", &KEY_CHOICES, 0),
    choice("scale", "Scale", &SCALE_CHOICES, 0),
    note("note0", "C", true),
    note("note1", "C#", false),
    note("note2", "D", true),
    note("note3", "Eb", false),
    note("note4", "E", true),
    note("note5", "F", true),
    note("note6", "F#", false),
    note("note7", "G", true),
    note("note8", "Ab", false),
    note("note9", "A", true),
    note("note10", "Bb", false),
    note("note11", "B", true),
    disp(p("retune", "Retune Speed", 0.0, 200.0, STYLE_VALUES[0].0, Unit::Ms, Taper::Linear), retune_text),
    p("humanize", "Humanize", 0.0, 100.0, STYLE_VALUES[0].1, Unit::Percent, Taper::Linear),
    toggle("vibrato", "Keep Vibrato", STYLE_VALUES[0].2),
    disp(p("hold", "Note Hold", 0.0, 100.0, STYLE_VALUES[0].3, Unit::Cents, Taper::Linear), hold_text),
    disp(p("formant", "Formant", -12.0, 12.0, 0.0, Unit::Semitones, Taper::Linear), formant_text),
    p("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent, Taper::Linear),
];

const P_STYLE: usize = 0;
const P_KEY: usize = 1;
const P_SCALE: usize = 2;
const P_NOTE0: usize = 3;
const P_RETUNE: usize = 15;
const P_HUMANIZE: usize = 16;
const P_VIBRATO: usize = 17;
const P_HOLD: usize = 18;
const P_FORMANT: usize = 19;
const P_MIX: usize = 20;

pub static AUTOPITCH_INFO: PluginInfo = PluginInfo {
    id: "autopitch",
    name: "Auto Pitch",
    short_name: "AutoPtch",
    category: Category::Other,
    params: &AUTOPITCH_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// `mask` (bit per semitone above the tonic) moved to start at pitch class `tonic`.
pub fn rotate(mask: u16, tonic: u8) -> u16 {
    let t = u32::from(tonic % 12);
    let m = u32::from(mask & 0xFFF);
    (((m << t) | (m >> (12 - t))) & 0xFFF) as u16
}

pub struct AutoPitch {
    p: Params,
    sr: f32,
    channels: usize,
    front: Front,
    studio: Studio,
    mix: Smoother,
    keys: [SongKey; MAX_SONG_KEYS],
    n_keys: usize,
    /// The transport at the block's start and the input count there.
    playing: bool,
    at_samples: i64,
    block_n_in: u64,
    // Readouts.
    sung: f32,
    moved: f32,
    target: f32,
    tonic: u8,
    mask: u16,
    in_peak: f32,
}

impl Default for AutoPitch {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoPitch {
    pub fn new() -> Self {
        let mut a = AutoPitch {
            p: Params::new(&AUTOPITCH_PARAMS),
            sr: 48_000.0,
            channels: 2,
            front: Front::new(48_000.0, 2, 128, 4096, detect::FLOOR_HZ),
            studio: Studio::new(48_000.0, 2),
            mix: Smoother::new(1.0),
            keys: [SongKey::default(); MAX_SONG_KEYS],
            n_keys: 0,
            playing: false,
            at_samples: 0,
            block_n_in: 0,
            sung: f32::NAN,
            moved: f32::NAN,
            target: f32::NAN,
            tonic: 0,
            mask: SCALES[0].1,
            in_peak: 0.0,
        };
        a.prepare(48_000.0, 1024, 2);
        a
    }

    fn update(&mut self) {
        self.mix.set(self.p.v(P_MIX) / 100.0);
    }

    /// The corrector's settings: the Style's, or the four controls under Custom.
    fn settings(&self) -> Settings {
        let (retune, humanize, vibrato, hold) = match STYLE_VALUES.get(self.p.choice(P_STYLE)) {
            Some(v) => *v,
            None => (self.p.v(P_RETUNE), self.p.v(P_HUMANIZE), self.p.on(P_VIBRATO), self.p.v(P_HOLD)),
        };
        Settings::from_controls(f64::from(retune), f64::from(humanize) / 100.0, vibrato, f64::from(hold))
    }

    /// The key's tonic and enabled notes at timeline sample `pos`.
    fn key_at(&self, pos: i64) -> (u8, u16) {
        let key = self.p.choice(P_KEY);
        if key == 0 && self.n_keys > 0 {
            let keys = self.keys.get(..self.n_keys).unwrap_or(&[]);
            let k = keys.partition_point(|k| k.start <= pos);
            let sk = keys.get(k.saturating_sub(1)).copied().unwrap_or_default();
            let scale = if sk.minor { SCALES[1].1 } else { SCALES[0].1 };
            return (sk.tonic % 12, rotate(scale, sk.tonic));
        }
        let tonic = key.saturating_sub(1).min(11) as u8;
        let scale = self.p.choice(P_SCALE);
        let mask = match SCALES.get(scale) {
            Some(s) => rotate(s.1, tonic),
            None => (0..12).filter(|i| self.p.on(P_NOTE0 + i)).fold(0u16, |m, i| m | (1 << i)),
        };
        (tonic, mask)
    }
}

impl Plugin for AutoPitch {
    fn info(&self) -> &'static PluginInfo {
        &AUTOPITCH_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if self.p.set(id, value).is_some() {
            self.update();
            true
        } else {
            false
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.channels = sane_channels(channels).max(1);
        let sr = f64::from(self.sr);
        self.studio = Studio::new(sr, self.channels);
        let hop = (self.studio.frame_hop() / 2).max(1);
        self.front = Front::new(sr, self.channels, hop, self.studio.history(), detect::FLOOR_HZ);
        self.mix.set_time(20.0, self.sr);
        self.update();
        self.mix.snap();
        self.reset();
    }

    fn reset(&mut self) {
        self.front.reset();
        self.studio.reset();
        self.mix.snap();
        self.block_n_in = 0;
        self.sung = f32::NAN;
        self.moved = f32::NAN;
        self.target = f32::NAN;
        self.in_peak = 0.0;
    }

    fn latency(&self) -> usize {
        self.studio.latency()
    }

    fn set_transport(&mut self, t: &Transport) {
        self.playing = t.playing;
        self.at_samples = t.samples;
        self.block_n_in = self.front.n_in;
    }

    fn follows_song_key(&self) -> bool {
        true
    }

    fn set_song_keys(&mut self, keys: &[SongKey]) {
        let n = keys.len().min(MAX_SONG_KEYS);
        for (d, s) in self.keys.iter_mut().zip(keys.iter().take(n)) {
            *d = *s;
        }
        self.n_keys = n;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = io.len().min(self.channels);
        let n = io.iter().take(ch).map(Vec::len).fold(frames, usize::min);
        if ch == 0 {
            return;
        }
        let s = self.settings();
        let formant = f64::from(self.p.v(P_FORMANT));
        let hop = self.front.hop as u64;
        self.in_peak = 0.0;
        for buf in io.iter().take(ch) {
            for v in buf.iter().take(n) {
                if v.is_finite() {
                    self.in_peak = self.in_peak.max(v.abs());
                }
            }
        }
        let mut pos = 0;
        while pos < n {
            let to_boundary = (hop - self.front.n_in % hop) as usize;
            let take = to_boundary.min(n - pos);
            self.front.push(io, pos, take);
            if self.front.detection_due() {
                let since = self.front.n_in.saturating_sub(self.block_n_in) as f64 - self.front.next_tc_offset();
                let at = if self.playing { self.at_samples.saturating_add(since.round() as i64) } else { self.at_samples };
                let (tonic, mask) = self.key_at(at);
                let e = self.front.detect(&s, mask);
                self.tonic = tonic;
                self.mask = mask;
                self.sung = e.cents as f32;
                self.target = e.target as f32;
                self.moved = (e.cents + e.corr) as f32;
            }
            self.studio.run_frames(&self.front, formant);
            let mix = &mut self.mix;
            self.studio.emit(&self.front, io, pos, take, || mix.next());
            pos += take;
        }
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.sung;
        out[1] = self.moved;
        out[2] = self.target;
        out[3] = f32::from(self.tonic);
        out[4] = f32::from(self.mask);
        out[5] = gain_to_db(self.in_peak);
        out[6] = self.studio.latency() as f32 / self.sr * 1000.0;
    }
}

#[cfg(test)]
mod tests;
