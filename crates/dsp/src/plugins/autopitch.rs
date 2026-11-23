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
//!   **Live** mode ([`live`]): the study's low-delay "splice" engine for singing through it, about
//!   4 to 8 ms by Voice range (formants move a little with the correction; Formant does not
//!   apply). Low Latency Monitoring keeps it in on an armed or monitored track
//!   ([`Plugin::live_monitoring`]).
//!
//! Each mode (and each Voice range in Live) is a lane of its own: its own input history,
//! detector, corrector and engine, all made in `prepare`; switching lanes starts the new one
//! clean (the mixer re-aligns the changed latency with its crossfade).
//!
//! Style sets the character in one control: Natural (gentle, keeps the vibrato) and Hard tune
//! (instant, flattened) use their own Retune speed, Humanize, Keep vibrato and Note hold; Custom
//! uses those four controls as set.
//!
//! Every buffer is made in `prepare`; `process` never allocates, locks or panics.
//! `readouts`: `[0]` sung pitch (MIDI cents, NaN when letting the voice through), `[1]` the pitch
//! it is moved to, `[2]` the note aimed at, `[3]` the key's tonic (pitch class), `[4]` the enabled
//! notes (bit per pitch class), `[5]` input peak dBFS, `[6]` delay (ms), `[7]` mode (0 Studio,
//! 1 Live).

mod correct;
mod detect;
mod front;
mod live;
pub mod presets;
mod studio;

use crate::params::{Params, choice, p, toggle};
use crate::util::{Smoother, sane_channels, sane_sr};
use crate::{Category, MAX_SONG_KEYS, ParamInfo, Plugin, PluginInfo, SongKey, Taper, Transport, Unit, gain_to_db};
use correct::Settings;
use front::Front;
use live::Live;
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

/// The Mode control: Studio (43 ms, cleanest) or Live (low delay, for singing through it).
const MODE_CHOICES: [&str; 2] = ["Studio", "Live"];
pub const MODE_STUDIO: usize = 0;
pub const MODE_LIVE: usize = 1;

/// Live mode's Voice range: the lowest note it listens for, which sets its delay.
const RANGE_CHOICES: [&str; 3] = ["Low (from 70 Hz)", "Middle (from 100 Hz)", "High (from 150 Hz)"];
/// The floors of [`RANGE_CHOICES`] (Hz).
pub const RANGE_FLOORS: [f64; 3] = [70.0, 100.0, 150.0];

const fn note(id: &'static str, name: &'static str, on: bool) -> ParamInfo {
    toggle(id, name, on)
}

static AUTOPITCH_PARAMS: [ParamInfo; 23] = [
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
    choice("mode", "Mode", &MODE_CHOICES, MODE_STUDIO),
    choice("range", "Voice Range", &RANGE_CHOICES, 1),
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
const P_MODE: usize = 21;
const P_RANGE: usize = 22;

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

/// One mode's front end and engine.
struct Lane {
    front: Front,
    engine: Engine,
}

enum Engine {
    Studio(Box<Studio>),
    Live(Live),
}

impl Lane {
    fn studio(sr: f64, channels: usize) -> Lane {
        let studio = Studio::new(sr, channels);
        let hop = (studio.frame_hop() / 2).max(1);
        Lane { front: Front::new(sr, channels, hop, studio.history(), detect::FLOOR_HZ), engine: Engine::Studio(Box::new(studio)) }
    }

    fn live(sr: f64, channels: usize, floor: f64) -> Lane {
        let live = Live::new(sr, floor);
        // Detections about every 2.7 ms at any rate (128 at 48 kHz).
        let hop = ((128.0 * sr / 48_000.0).round() as usize).max(1);
        Lane { front: Front::new(sr, channels, hop, live.history(), floor), engine: Engine::Live(live) }
    }

    fn latency(&self) -> usize {
        match &self.engine {
            Engine::Studio(e) => e.latency(),
            Engine::Live(e) => e.latency(),
        }
    }

    fn reset(&mut self) {
        self.front.reset();
        match &mut self.engine {
            Engine::Studio(e) => e.reset(),
            Engine::Live(e) => e.reset(),
        }
    }
}

pub struct AutoPitch {
    p: Params,
    sr: f32,
    channels: usize,
    /// Lane 0 Studio, lanes 1..=3 Live by Voice range: each its own input history, detector,
    /// corrector and engine.
    lanes: Vec<Lane>,
    /// The lane running now.
    active: usize,
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
            lanes: Vec::new(),
            active: 0,
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

    /// The lane the controls ask for: Studio, or Live at its Voice range.
    fn wanted(&self) -> usize {
        if self.p.choice(P_MODE) == MODE_LIVE { 1 + self.p.choice(P_RANGE).min(RANGE_FLOORS.len() - 1) } else { 0 }
    }

    /// Delay range of Live mode since the last reset (samples: mean, largest); `None` in Studio.
    pub fn live_delay(&self) -> Option<(f64, f64)> {
        match self.lanes.get(self.active).map(|l| &l.engine) {
            Some(Engine::Live(e)) => Some(e.delay_stats()),
            _ => None,
        }
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

#[cfg(test)]
impl AutoPitch {
    /// Addresses and capacities of every lane's heap buffers (tests: nothing reallocates).
    fn buffers(&self) -> Vec<(usize, usize)> {
        let mut out = vec![(self.lanes.as_ptr() as usize, self.lanes.capacity())];
        for l in &self.lanes {
            out.extend(l.front.buffers());
            if let Engine::Studio(e) = &l.engine {
                out.extend(e.buffers());
            }
        }
        out
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
        let ch = self.channels;
        self.lanes = std::iter::once(Lane::studio(sr, ch)).chain(RANGE_FLOORS.iter().map(|f| Lane::live(sr, ch, *f))).collect();
        self.active = self.wanted();
        self.mix.set_time(20.0, self.sr);
        self.update();
        self.mix.snap();
        self.reset();
    }

    fn reset(&mut self) {
        self.active = self.wanted();
        if let Some(l) = self.lanes.get_mut(self.active) {
            l.reset();
        }
        self.mix.snap();
        self.block_n_in = 0;
        self.sung = f32::NAN;
        self.moved = f32::NAN;
        self.target = f32::NAN;
        self.in_peak = 0.0;
    }

    fn latency(&self) -> usize {
        self.lanes.get(self.wanted()).map_or(0, Lane::latency)
    }

    fn live_monitoring(&self) -> bool {
        self.wanted() != 0
    }

    fn set_transport(&mut self, t: &Transport) {
        self.playing = t.playing;
        self.at_samples = t.samples;
        self.block_n_in = self.lanes.get(self.active).map_or(0, |l| l.front.n_in);
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
        // A change of mode or Voice range: the new lane starts clean.
        let want = self.wanted();
        if want != self.active {
            self.active = want;
            if let Some(l) = self.lanes.get_mut(want) {
                l.reset();
            }
            self.block_n_in = 0;
        }
        let Some(lane) = self.lanes.get(self.active) else { return };
        let hop = lane.front.hop as u64;
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
            let Some(n_in) = self.lanes.get(self.active).map(|l| l.front.n_in) else { return };
            let to_boundary = (hop - n_in % hop) as usize;
            let take = to_boundary.min(n - pos);
            let due = {
                let mix = &mut self.mix;
                let Some(lane) = self.lanes.get_mut(self.active) else { return };
                lane.front.push(io, pos, take);
                // Live reads the decisions made up to the last detection before these samples
                // (whatever the block size): it emits before this hop's detection.
                if let Engine::Live(e) = &mut lane.engine {
                    e.emit(&lane.front, io, pos, take, || mix.next());
                }
                lane.front.detection_due().then(|| lane.front.n_in.saturating_sub(self.block_n_in) as f64 - lane.front.next_tc_offset())
            };
            if let Some(since) = due {
                let at = if self.playing { self.at_samples.saturating_add(since.round() as i64) } else { self.at_samples };
                let (tonic, mask) = self.key_at(at);
                let Some(lane) = self.lanes.get_mut(self.active) else { return };
                let e = lane.front.detect(&s, mask);
                self.tonic = tonic;
                self.mask = mask;
                self.sung = e.cents as f32;
                self.target = e.target as f32;
                self.moved = (e.cents + e.corr) as f32;
            }
            let mix = &mut self.mix;
            let Some(lane) = self.lanes.get_mut(self.active) else { return };
            match &mut lane.engine {
                Engine::Studio(e) => {
                    e.run_frames(&lane.front, formant);
                    e.emit(&lane.front, io, pos, take, || mix.next());
                }
                Engine::Live(_) => {}
            }
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
        out[6] = self.lanes.get(self.active).map_or(0, Lane::latency) as f32 / self.sr * 1000.0;
        out[7] = if self.active == 0 { 0.0 } else { 1.0 };
    }
}

#[cfg(test)]
mod tests;
