//! Pantry, the multisampler: instruments recorded note by note on instrument tracks.
//!
//! `track.capture_multisample` makes one from any instrument track: it plays the track's
//! instrument a note at a time, at several velocities, through the track's module chain
//! (offline, as Freeze renders it), cuts each note where it has died away, and puts the notes in
//! a new track's Pantry.
//! `mix.multisample_load` reads an SFZ file's key-down regions, or a Decent Sampler preset's or
//! library's samples, into the track's Multisampler.
//! Each sample is a session source (`Insert::samples`, keyed `n{note}.v{velocity}.r{robin}`: the
//! note it was recorded at, the top of its velocity range and its round-robin take), saved in
//! `Audio Files/` with the session; the mix engine hands each to the running instrument. Of an
//! SFZ file only the mapping is read (`sample`, `pitch_keycenter` or `key`, `hivel`,
//! `seq_position`, `default_path`, with `<global>`, `<master>` and `<group>` inherited): the
//! instrument's own parameters shape the sound. `mix.multisample_save` writes a Pantry as such a
//! file with its samples beside it (and its parameters, which a load restores), and
//! `mix.multisample_import` takes loose sample files, placed by their names.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_dsp::plugins::multisampler::{
    LAYERS, NOTES, ON_KEY_DOWN, ON_KEY_UP, ON_PEDAL_DOWN, ON_PEDAL_UP, PART_OFF_DB, PARTS, PEDAL_ANY, PEDAL_DOWN, PEDAL_UP, ROBINS, ZONES, ZoneSpec,
    parse_sample_key, sample_key,
};
use soundcraft_midi::{Note, Sequence, note_name};
use soundcraft_model::{ChannelFormat, Clip, Insert, Route, TrackKind};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The Multisampler's plugin id.
pub const MULTISAMPLER: &str = "multisampler";

/// Largest SFZ file read (bytes): mappings are text, a few hundred kilobytes at most.
const MAX_SFZ_BYTES: usize = 8 << 20;
/// Largest single sample (frames × channels): two minutes of 192 kHz stereo.
const MAX_SAMPLE_VALUES: usize = 192_000 * 120 * 2;

/// Silence rendered before a captured note's first strike: the chain's own noise is measured there.
const CAPTURE_LEAD_SECS: f64 = 0.5;
/// A captured note ends this long after its last sound above the silence level, in a fade.
const CAPTURE_AFTER_SECS: f64 = 0.05;
const CAPTURE_FADE_SECS: f64 = 0.02;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.capture_multisample",
            "Capture to Pantry",
            [],
            None,
            "{track?, low?: 21, high?: 108 (MIDI notes), step?: 3 (semitones between captured notes, 1-12), layers?: 12 (evenly spaced velocities up to 127, 1-16) or velocities?: [1-127…], robins?: 1 (strikes of each, 1-4), hold?: 8 (seconds the lowest note is held), hold_high?: 3 (the highest; the ones between shorten evenly), tail?: 4 (seconds left to ring after each), silence?: -72 (dBFS a note has died away at), release?: false (also capture what sounds when a key comes up, as release samples), pedal?: false (also capture the sustain pedal's own noises), mono?: false, name?} -> {track, samples, notes, layers, release, pedal, seconds, silent, ringing}: plays the track's instrument a note at a time through its modules (pre-fader, offline) and makes a new instrument track whose Pantry plays the captured notes; `silent` notes made no sound and are left out, `ringing` ones were still sounding when the next began",
            has_selection,
            capture
        ),
        cmd!(
            "mix.multisample_load",
            "Load Multisample",
            [],
            None,
            "{track?, path: an SFZ file (.sfz), a Decent Sampler preset (.dspreset) or library (.dslibrary), preset?: which preset of a library (default: its first by name)} -> {track, name, samples, notes, layers, skipped, presets}: the track's Multisampler plays the instrument's samples in place of its own (the mapping only: recorded note, velocity layer and round-robin of each key-down sample; at most 16 layers a note and 4 round-robins a layer, the rest skipped); `presets` names a library's presets",
            has_selection,
            load
        ),
        cmd!(
            "mix.multisample_import",
            "Import Samples to Multisample",
            [],
            None,
            "{track?, folder?: a folder of audio files, paths?: [audio files], replace?: false} -> {track, samples, notes, layers, skipped, unnamed}: puts samples in the track's Multisampler by their file names, beside what it holds (or in its place with `replace`): the note and velocity each was recorded at as capture names them (`Piano C3 v95.wav`, `Piano D_1 v95 rr2 747.wav`) or as Save to Library does (`060_C3_v095_rr1.wav`); files whose names say neither are counted in `unnamed`, samples that did not fit in `skipped`",
            has_selection,
            import
        ),
        cmd!(noundo
            "mix.multisample_save",
            "Save Multisample",
            [],
            None,
            "{track?, path: where to write the .sfz file, replace?: false} -> {path, name, samples, bytes}: writes the track's Multisampler as an SFZ file, its samples (32-bit float WAV) in a folder of the file's name beside it, and its parameters; mix.multisample_load reads it back as it was, and other SFZ players play it",
            has_selection,
            save
        ),
        cmd!(
            "mix.multisample_clear",
            "Clear Multisample",
            [],
            None,
            "{track?} -> {track, cleared}: empties the track's Multisampler of its samples (its parameters stay)",
            has_selection,
            clear
        ),
    ]
}

/// A Pantry with every parameter at its default.
fn pantry_insert() -> Insert {
    let mut ins = Insert::new(MULTISAMPLER);
    if let Some(info) = soundcraft_dsp::plugin_info(MULTISAMPLER) {
        for p in info.params {
            ins.params.insert(p.id.to_string(), p.default);
        }
    }
    ins
}

/// A whole number parameter within `lo..=hi` (`default` when absent).
fn whole_param(p: &Value, id: &str, key: &str, lo: i64, hi: i64, default: i64) -> Result<i64> {
    match p.get(key) {
        None => Ok(default),
        Some(v) => v.as_i64().filter(|n| (lo..=hi).contains(n)).ok_or_else(|| bad(id, format!("`{key}` must be a whole number from {lo} to {hi}"))),
    }
}

/// A number parameter within `lo..=hi` (`default` when absent).
fn number_param(p: &Value, id: &str, key: &str, lo: f64, hi: f64, default: f64) -> Result<f64> {
    match p.get(key) {
        None => Ok(default),
        Some(v) => v.as_f64().filter(|n| (lo..=hi).contains(n)).ok_or_else(|| bad(id, format!("`{key}` must be a number from {lo} to {hi}"))),
    }
}

/// The loudest sample of frames `a..b` of `audio`.
fn peak_in(audio: &[Vec<f32>], a: usize, b: usize) -> f32 {
    audio.iter().filter_map(|c| c.get(a.min(c.len())..b.min(c.len()))).flatten().fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { m })
}

/// What a capture has made: the samples by key, and what it found on the way.
#[derive(Debug, Default)]
pub struct Captured {
    /// The command's parameters: the capture is for the command run with these.
    params: Value,
    /// Each note sample's key, source name and audio.
    samples: Vec<(String, String, AudioBuffer)>,
    /// The release samples and pedal noises: each one's zone (its index is set when it goes into
    /// the session), source name and audio.
    zones: Vec<(ZoneSpec, String, AudioBuffer)>,
    silent: usize,
    ringing: usize,
}

/// What a stretch of a capture's render holds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Strike {
    /// A note, held: its velocity and take.
    Note {
        velocity: u8,
        robin: usize,
    },
    /// What sounds after a short note's key comes up, for the velocities `lovel..=hivel`.
    Release {
        velocity: u8,
        lovel: u8,
        hivel: u8,
    },
    PedalDown,
    PedalUp,
}

/// A strike in a render: the frame its event is at (a key or the pedal going down), and the
/// frame the sound to keep starts at (the same, or for a release the key coming up).
#[derive(Debug, Clone, Copy)]
struct Cut {
    kind: Strike,
    on: usize,
    start: usize,
}

/// How long a note is held before the release that is captured, and the parts capture makes.
const CAPTURE_RELEASE_HOLD_SECS: f64 = 0.3;
const CAPTURE_PEDAL_SECS: f64 = 2.0;
const RELEASE_PART: usize = 1;
const PEDAL_PART: usize = 2;

/// `track.capture_multisample` a slice at a time, so a host can keep drawing behind a progress
/// window: [`CaptureJob::step`] until it returns true, [`CaptureJob::finish`], then the command
/// with the same parameters puts the result in the session (one undo step). The job renders
/// from the session as it was when the job was made, through instances of its own.
pub struct CaptureJob {
    id: &'static str,
    params: Value,
    /// The session the job renders: the track alone plays one clip, a note's strikes.
    scratch: soundcraft_model::Session,
    track: TrackId,
    track_name: String,
    name: String,
    notes: Vec<u8>,
    velocities: Vec<u8>,
    robins: usize,
    low: i64,
    high: i64,
    hold: f64,
    hold_high: f64,
    tail: f64,
    quiet_at: f32,
    mono: bool,
    lag: Samples,
    /// Whether to capture what sounds when keys come up, and the pedal's own noises.
    release: bool,
    pedal: bool,
    /// The render in hand (an index into `notes`; one past them, the pedal's), and its strikes.
    at: usize,
    render: Option<(soundcraft_mix::TrackRender, Vec<Cut>)>,
    total: usize,
    out: Captured,
}

impl CaptureJob {
    /// Plans the capture `p` asks for; an error is the command's own refusal.
    pub fn new(e: &Engine, p: &Value) -> Result<CaptureJob> {
        let id = "track.capture_multisample";
        let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
        let s = e.session_arc();
        let tr = s.track(t).ok_or_else(|| bad(id, "no track"))?;
        if !tr.kind.is_midi() || tr.instrument.is_none() {
            return Err(bad(id, format!("`{}` plays no instrument to capture", tr.name)));
        }
        if tr.frozen {
            return Err(bad(id, super::freeze::UNFREEZE_TO_EDIT));
        }
        let low = whole_param(p, id, "low", 0, 127, 21)?;
        let high = whole_param(p, id, "high", 0, 127, 108)?;
        if low > high {
            return Err(bad(id, "`low` is above `high`"));
        }
        let step = whole_param(p, id, "step", 1, 12, 3)? as usize;
        let robins = whole_param(p, id, "robins", 1, ROBINS as i64, 1)? as usize;
        let velocities: BTreeSet<u8> = match p.get("velocities") {
            Some(v) => {
                let list = v.as_array().filter(|a| !a.is_empty()).ok_or_else(|| bad(id, "`velocities` must be a list of velocities, 1 to 127"))?;
                let read = |v: &Value| v.as_u64().filter(|n| (1..=127).contains(n)).map(|n| n as u8);
                list.iter().map(read).collect::<Option<_>>().ok_or_else(|| bad(id, "`velocities` must be a list of velocities, 1 to 127"))?
            }
            None => {
                let n = whole_param(p, id, "layers", 1, LAYERS as i64, 12)?;
                (1..=n).map(|i| ((127 * i + n / 2) / n).clamp(1, 127) as u8).collect()
            }
        };
        if velocities.len() > LAYERS {
            return Err(bad(id, format!("{LAYERS} velocities at most")));
        }
        let hold = number_param(p, id, "hold", 0.05, 60.0, 8.0)?;
        let hold_high = number_param(p, id, "hold_high", 0.05, 60.0, hold.min(3.0))?;
        let tail = number_param(p, id, "tail", 0.0, 30.0, 4.0)?;
        let silence = number_param(p, id, "silence", -120.0, -20.0, -72.0)?;
        let name = str_param(p, "name").map_or_else(|| format!("{} Pantry", tr.name), str::to_string);
        let strikes = velocities.len() * robins;
        let longest =
            s.sample_rate.samples(CAPTURE_LEAD_SECS + (hold.max(hold_high) + tail) * strikes as f64 + soundcraft_mix::INSTRUMENT_TAIL_MAX_SECS);
        crate::io::refuse_longer(&s, id, &tr.name, "capture", longest, crate::io::render_limit(2))?;
        Ok(CaptureJob {
            id,
            params: p.clone(),
            scratch: capture_scratch(&s, t),
            track: t,
            track_name: tr.name.clone(),
            name,
            notes: (low..=high).step_by(step).filter_map(|n| u8::try_from(n).ok()).collect(),
            velocities: velocities.into_iter().collect(),
            robins,
            low,
            high,
            hold,
            hold_high,
            tail,
            quiet_at: 10f32.powf(silence as f32 / 20.0),
            mono: bool_or(p, "mono", false),
            lag: soundcraft_mix::midi_lag(&s, tr),
            release: bool_or(p, "release", false),
            pedal: bool_or(p, "pedal", false),
            at: 0,
            render: None,
            total: 0,
            out: Captured { params: p.clone(), ..Captured::default() },
        })
    }

    /// The command and the parameters the job was made for.
    pub fn command(&self) -> (&'static str, &Value) {
        (self.id, &self.params)
    }

    /// Renders done so far, and how many there are (a note each, and one for the pedal).
    pub fn counts(&self) -> (usize, usize) {
        let all = self.notes.len() + usize::from(self.pedal);
        (self.at.min(all), all)
    }

    /// What is being rendered: a note by name, or the pedal (`None` once all are done).
    pub fn note(&self) -> Option<String> {
        match self.notes.get(self.at) {
            Some(n) => Some(note_name(*n)),
            None => (self.pedal && self.at == self.notes.len()).then(|| "Pedal".to_string()),
        }
    }

    /// How far along, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let within = self.render.as_ref().map_or(0.0, |(r, _)| r.progress());
        ((self.at as f32 + within) / self.counts().1.max(1) as f32).clamp(0.0, 1.0)
    }

    /// The frame an event at `at` sounds at, and its tick.
    fn event(&self, at: Samples) -> (i64, usize) {
        let sr = self.scratch.sample_rate;
        let tick = self.scratch.tempo.samples_to_ticks(at, sr);
        (tick, usize::try_from(self.scratch.tempo.tick_to_samples(tick, sr).saturating_add(self.lag).max(0)).unwrap_or(0))
    }

    /// Starts the render in hand: half a second of silence, then a note's strikes (held ones,
    /// then short ones for their releases), or the pedal going down and up.
    fn begin(&mut self) -> Result<()> {
        let sr = self.scratch.sample_rate;
        let mut seq = Sequence::default();
        let mut cuts = Vec::new();
        let mut at = sr.samples(CAPTURE_LEAD_SECS);
        if let Some(&note) = self.notes.get(self.at) {
            let along = if self.high > self.low { (i64::from(note) - self.low) as f64 / (self.high - self.low) as f64 } else { 0.0 };
            let held = self.hold + (self.hold_high - self.hold) * along;
            let (hold_len, slot) = (sr.samples(held).max(1), sr.samples(held + self.tail).max(1));
            let mut strike = |job: &Self, at: Samples, len: Samples, velocity: u8| {
                let ((on, sounds), (off, lets_go)) = (job.event(at), job.event(at.saturating_add(len)));
                seq.notes.push(Note {
                    pitch: note,
                    velocity,
                    release_velocity: 64,
                    channel: 0,
                    muted: false,
                    probability: 100,
                    start: on,
                    length: off.saturating_sub(on).max(1),
                });
                (sounds, lets_go)
            };
            for &velocity in &self.velocities {
                for robin in 1..=self.robins {
                    let (on, _) = strike(self, at, hold_len, velocity);
                    cuts.push(Cut { kind: Strike::Note { velocity, robin }, on, start: on });
                    at = at.saturating_add(slot);
                }
            }
            if self.release {
                // A soft and a hard release when there are layers to tell apart, else one.
                let top = self.velocities.last().copied().unwrap_or(127);
                let soft = self.velocities.get(self.velocities.len().saturating_sub(1) / 2).copied().filter(|v| *v < top);
                let short = sr.samples(CAPTURE_RELEASE_HOLD_SECS).max(1);
                for (velocity, lovel, hivel) in soft.map(|v| (v, 1, v)).into_iter().chain([(top, soft.map_or(1, |v| v + 1), 127)]) {
                    let (on, start) = strike(self, at, short, velocity);
                    cuts.push(Cut { kind: Strike::Release { velocity, lovel, hivel }, on, start });
                    at = at.saturating_add(short).saturating_add(sr.samples(self.tail).max(1));
                }
            }
        } else {
            for (kind, value) in [(Strike::PedalDown, 127), (Strike::PedalUp, 0)] {
                let (tick, on) = self.event(at);
                seq.ctrls.push(soundcraft_midi::CtrlEvent {
                    tick,
                    channel: 0,
                    kind: soundcraft_midi::CtrlKind::Cc(64),
                    value,
                    shape: Default::default(),
                });
                cuts.push(Cut { kind, on, start: on });
                at = at.saturating_add(sr.samples(CAPTURE_PEDAL_SECS));
            }
        }
        seq.sort();
        let clip = Clip::midi(self.scratch.new_clip_id(), "Capture", 0, at, seq);
        match self.scratch.track_mut(self.track).and_then(|tr| tr.playlist_mut()) {
            Some(pl) => pl.clips = vec![clip],
            None => return Err(bad(self.id, "the track has no playlist to play from")),
        }
        self.render = Some((soundcraft_mix::TrackRender::frozen(&self.scratch, self.track, at, None, None), cuts));
        Ok(())
    }

    /// The keys a captured note plays: the nearest one's, the lower of two as near.
    fn keys_of(&self, note: u8) -> (u8, u8) {
        let at = self.notes.iter().position(|n| *n == note).unwrap_or(0);
        let lo = if at == 0 { 0 } else { self.notes.get(at - 1).map_or(0, |p| ((u16::from(*p) + u16::from(note)) / 2 + 1) as u8) };
        (lo, self.notes.get(at + 1).map_or(127, |n| ((u16::from(note) + u16::from(*n)) / 2) as u8))
    }

    /// Cuts the strikes of the render just done out of `audio`.
    fn cut(&mut self, audio: &[Vec<f32>], cuts: &[Cut]) -> Result<()> {
        let note = self.notes.get(self.at).copied();
        let sr = self.scratch.sample_rate;
        let frames = |secs: f64| usize::try_from(sr.samples(secs).max(0)).unwrap_or(0);
        let rendered = audio.iter().map(Vec::len).min().unwrap_or(0);
        // What counts as died away: the level asked for, or just above the chain's own noise.
        let quiet = self.quiet_at.max(peak_in(audio, 0, frames(CAPTURE_LEAD_SECS)) * 1.5);
        let block = frames(0.005).max(1);
        for (i, cut) in cuts.iter().enumerate() {
            let (start, last_strike) = (cut.start, i + 1 == cuts.len());
            let limit = cuts.get(i + 1).map_or(rendered, |next| next.on.min(rendered));
            if start >= limit || peak_in(audio, start, limit) <= quiet {
                // A note that made no sound is told of; no release or pedal noise is no fault.
                self.out.silent += usize::from(matches!(cut.kind, Strike::Note { .. }));
                continue;
            }
            let mut loud = start;
            let mut k = start;
            while k < limit {
                let to = k.saturating_add(block).min(limit);
                if peak_in(audio, k, to) > quiet {
                    loud = to;
                }
                k = to;
            }
            if loud >= limit && !last_strike && matches!(cut.kind, Strike::Note { .. }) {
                self.out.ringing += 1;
            }
            let end = loud.saturating_add(frames(CAPTURE_AFTER_SECS)).min(limit);
            let mut channels: Vec<Vec<f32>> = audio.iter().map(|c| c.get(start..end).map(<[f32]>::to_vec).unwrap_or_default()).collect();
            if self.mono && channels.len() > 1 {
                let n = channels.len() as f32;
                let len = channels.iter().map(Vec::len).min().unwrap_or(0);
                channels = vec![(0..len).map(|f| channels.iter().filter_map(|c| c.get(f)).sum::<f32>() / n).collect()];
            }
            let fade = frames(CAPTURE_FADE_SECS).max(1);
            for c in &mut channels {
                let len = c.len();
                for (j, x) in c.iter_mut().rev().take(fade).enumerate() {
                    let g = 0.5 - 0.5 * (std::f32::consts::PI * j as f32 / fade.min(len) as f32).cos();
                    *x *= g.clamp(0.0, 1.0);
                }
            }
            self.total = self.total.saturating_add(channels.iter().map(Vec::len).sum::<usize>());
            if self.total > crate::io::MAX_IMPORT_SAMPLES {
                return Err(bad(self.id, "too much audio to hold in memory: capture fewer notes or layers, shorter, or in mono"));
            }
            let buf = AudioBuffer { sample_rate: sr.hz(), channels };
            let name = &self.name;
            match (cut.kind, note) {
                (Strike::Note { velocity, robin }, Some(note)) => {
                    let file =
                        format!("{name} {} v{velocity}{}", note_name(note), if self.robins > 1 { format!(" rr{robin}") } else { String::new() });
                    self.out.samples.push((sample_key(note, velocity, robin), file, buf));
                }
                (Strike::Release { velocity, lovel, hivel }, Some(note)) => {
                    let (lokey, hikey) = self.keys_of(note);
                    let zone = ZoneSpec {
                        part: RELEASE_PART,
                        part_name: "Release".into(),
                        lokey,
                        hikey,
                        root: note,
                        lovel,
                        hivel,
                        trigger: ON_KEY_UP,
                        ref_vel: velocity,
                        ..ZoneSpec::default()
                    };
                    self.out.zones.push((zone, format!("{name} {} release v{velocity}", note_name(note)), buf));
                }
                (Strike::PedalDown | Strike::PedalUp, _) => {
                    let down = cut.kind == Strike::PedalDown;
                    let zone = ZoneSpec {
                        part: PEDAL_PART,
                        part_name: "Pedal".into(),
                        trigger: if down { ON_PEDAL_DOWN } else { ON_PEDAL_UP },
                        keytrack: false,
                        veltrack: 0.0,
                        ..ZoneSpec::default()
                    };
                    self.out.zones.push((zone, format!("{name} pedal {}", if down { "down" } else { "up" }), buf));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Renders while `more` says so (asked after every block). `Ok(true)` once every note is
    /// done; an error ends the job.
    pub fn step(&mut self, mut more: impl FnMut() -> bool) -> Result<bool> {
        loop {
            if self.at >= self.counts().1 {
                return Ok(true);
            }
            if self.render.is_none() {
                self.begin()?;
            }
            let done = self.render.as_mut().is_none_or(|(r, _)| r.step());
            if done && let Some((render, cuts)) = self.render.take() {
                let (audio, _) = render.finish();
                self.cut(&audio, &cuts)?;
                self.at += 1;
            }
            if !more() {
                return Ok(self.at >= self.counts().1);
            }
        }
    }

    /// What the job made, for [`Engine::offer_capture`]; refuses a capture of silence.
    pub fn finish(self) -> Result<Captured> {
        if self.out.samples.is_empty() {
            return Err(bad(self.id, format!("`{}` made no sound to capture", self.track_name)));
        }
        Ok(self.out)
    }
}

/// The session a capture of `track` renders: `s`, with the track playing each note to its
/// instrument exactly as written. The global transpose track and the track's MIDI Real-Time
/// Properties (transpose, velocity, length, delay, quantize) are how the track plays its clips,
/// not part of the instrument: the Pantry made follows the global transpose itself, so a note
/// captured through it would be transposed twice, and a sample is labelled with the note and
/// velocity it was asked for, so it must be played at them.
fn capture_scratch(s: &soundcraft_model::Session, track: TrackId) -> soundcraft_model::Session {
    let mut scratch = s.clone();
    if let Some(tr) = scratch.track_mut(track) {
        tr.transpose_independent = true;
    }
    let rtp = format!("rtp.{}.", track.0);
    scratch.edit.values.retain(|k, _| !k.starts_with(&rtp));
    scratch
}

fn capture(e: &mut Engine, p: &Value) -> Result<Value> {
    // A host may have rendered it already, behind a progress window.
    let made = match e.take_capture().filter(|c| c.params == *p) {
        Some(made) => made,
        None => {
            let mut job = CaptureJob::new(e, p)?;
            while !job.step(|| true)? {}
            job.finish()?
        }
    };
    let id = "track.capture_multisample";
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    let name = match str_param(p, "name") {
        Some(n) => n.to_string(),
        None => format!("{} Pantry", e.session().track(t).map_or("", |tr| tr.name.as_str())),
    };
    let sr = e.session().sample_rate;
    let frames = made.samples.iter().map(|(_, _, b)| b.frames()).chain(made.zones.iter().map(|(_, _, b)| b.frames())).sum::<usize>();
    let seconds = sr.seconds(Samples::try_from(frames).unwrap_or(Samples::MAX));
    let notes: BTreeSet<&str> = made.samples.iter().filter_map(|(k, _, _)| k.split('.').next()).collect();
    let layers: BTreeSet<&str> = made.samples.iter().filter_map(|(k, _, _)| k.split('.').nth(1)).collect();
    let (count, notes, layers) = (made.samples.len(), notes.len(), layers.len());
    let releases = made.zones.iter().filter(|(z, _, _)| z.trigger == ON_KEY_UP).count();
    let pedal = made.zones.len() - releases;
    let mut ins = pantry_insert();
    let session = e.session_mut();
    for (key, file, buf) in made.samples {
        ins.samples.insert(key, crate::io::add_source(session, &file, buf, None, FileFormat::Wav));
    }
    for (index, (zone, file, buf)) in made.zones.into_iter().enumerate().take(ZONES) {
        ins.samples.insert(ZoneSpec { index, ..zone }.key(), crate::io::add_source(session, &file, buf, None, FileFormat::Wav));
    }
    let track = session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
    if let Some(tr) = session.track_mut(track) {
        tr.instrument = Some(ins);
        tr.mixer.input = Route::Midi(soundcraft_model::MidiIn::all());
    }
    session.edit.selected_tracks = vec![track];
    Ok(json!({
        "track": track,
        "samples": count,
        "notes": notes,
        "layers": layers,
        "release": releases,
        "pedal": pedal,
        "seconds": seconds,
        "silent": made.silent,
        "ringing": made.ringing,
    }))
}

/// A sample of an instrument file, and where it plays.
#[derive(Debug, Clone, PartialEq)]
struct Region {
    /// The sample's path as the file gives it (after `default_path`), with `/` separators.
    sample: String,
    /// The note it was recorded at, the top of its velocity range (or, of a file Pantry saved
    /// from a capture, the velocity it was played at) and its round-robin take, from 1.
    note: u8,
    velocity: u8,
    robin: usize,
    /// A captured sample of a file Pantry saved: placed by note, velocity and take, its ranges
    /// worked out by the instrument.
    captured: bool,
    /// Everything about where and how it plays (its index and part number are set on loading;
    /// frames count in the file's own sample rate), and its part's name.
    zone: ZoneSpec,
    part: String,
}

/// A level as Decent Sampler writes it: decibels (`-3.0dB`) or a linear gain (`0.7`).
fn level_db(text: &str) -> Option<f32> {
    let t = text.trim();
    let db = match t.to_ascii_lowercase().strip_suffix("db") {
        Some(db) => db.trim().parse::<f32>().ok()?,
        None => {
            let gain = t.parse::<f32>().ok()?;
            if gain <= 0.000_25 { PART_OFF_DB } else { 20.0 * gain.log10() }
        }
    };
    db.is_finite().then_some(db.clamp(-144.0, 24.0))
}

/// A note as SFZ writes it: a number, or a name (`c4` is 60).
fn sfz_note(text: &str) -> Option<u8> {
    let t = text.trim().to_ascii_lowercase();
    if let Ok(n) = t.parse::<i64>() {
        return u8::try_from(n).ok().filter(|n| *n <= 127);
    }
    let mut chars = t.chars();
    let mut n: i64 = match chars.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let rest = match rest.chars().next()? {
        '#' => {
            n += 1;
            rest.get(1..)?
        }
        'b' => {
            n -= 1;
            rest.get(1..)?
        }
        _ => rest,
    };
    let octave: i64 = rest.parse().ok()?;
    u8::try_from(n.checked_add(octave.checked_add(1)?.checked_mul(12)?)?).ok().filter(|n| *n <= 127)
}

/// The key-down regions of an SFZ file that name a sample file, in file order.
fn parse_sfz(text: &str) -> Vec<Region> {
    type Opcodes = BTreeMap<String, String>;
    let (mut control, mut global, mut master, mut group, mut region) =
        (Opcodes::new(), Opcodes::new(), Opcodes::new(), Opcodes::new(), Opcodes::new());
    let mut out = Vec::new();
    let mut scope = String::new();
    // The opcode a value's later words (a file name with spaces) belong to.
    let mut open: Option<String> = None;
    // The first keyswitch met, for a file that names no default.
    let mut first_switch: Option<u8> = None;
    let mut finish = |region: &mut Opcodes, control: &Opcodes, global: &Opcodes, master: &Opcodes, group: &Opcodes, out: &mut Vec<Region>| {
        let get = |k: &str| region.get(k).or_else(|| group.get(k)).or_else(|| master.get(k)).or_else(|| global.get(k)).map(String::as_str);
        let sample = get("sample").map(|s| s.trim().replace('\\', "/")).filter(|s| !s.is_empty() && !s.starts_with('*'));
        // Of an instrument with keyswitched versions (`sw_last`), the one it starts on.
        let switched_off = get("sw_last").and_then(sfz_note).is_some_and(|last| match get("sw_default").and_then(sfz_note) {
            Some(default) => last != default,
            None => *first_switch.get_or_insert(last) != last,
        });
        // What sets it off: a key going down or coming up, or the sustain pedal. A sound another
        // controller starts is left out.
        let started_by =
            |k: &String| k.starts_with("on_locc") || k.starts_with("on_hicc") || k.starts_with("start_locc") || k.starts_with("start_hicc");
        let controllers: Vec<&String> =
            region.keys().chain(group.keys()).chain(master.keys()).chain(global.keys()).filter(|k| started_by(k)).collect();
        let whole = |k: &str| get(k).and_then(|v| v.trim().parse::<f64>().ok());
        let trigger = if controllers.is_empty() {
            match get("trigger").map(|t| t.trim().to_ascii_lowercase()).as_deref() {
                None | Some("attack") => Some(ON_KEY_DOWN),
                Some("release" | "release_key") => Some(ON_KEY_UP),
                _ => None,
            }
        } else if controllers.iter().all(|k| k.ends_with("cc64")) {
            Some(if whole("on_locc64").unwrap_or(0.0) >= 64.0 { ON_PEDAL_DOWN } else { ON_PEDAL_UP })
        } else {
            None
        };
        if let (Some(sample), Some(trigger), false) = (sample, trigger, switched_off) {
            let dir = control.get("default_path").map(|d| d.trim().replace('\\', "/")).unwrap_or_default();
            // `key` gives all three of lokey, hikey and pitch_keycenter; one named beside it
            // stands (`key=a1 lokey=g1` plays G1 to A1).
            let key = get("key").and_then(sfz_note);
            let (lokey, hikey) = (get("lokey").and_then(sfz_note).or(key).unwrap_or(0), get("hikey").and_then(sfz_note).or(key).unwrap_or(127));
            // The note it was recorded at, or the one key it plays.
            let note = get("pitch_keycenter").and_then(sfz_note).or(key).or((lokey == hikey).then_some(lokey)).unwrap_or(60);
            let vel = |k: &str, d: u8| whole(k).map_or(d, |v| v.clamp(1.0, 127.0) as u8);
            let (lovel, hivel) = (vel("lovel", 1), vel("hivel", 127));
            let seq = whole("seq_position").map(|v| v.clamp(1.0, 63.0) as u8);
            let frames = |k: &str| whole(k).filter(|v| *v >= 0.0).map_or(0, |v| v.min(f64::from(u32::MAX)) as u32);
            let looped = get("loop_mode").is_some_and(|m| m.trim().starts_with("loop_")) && frames("loop_end") > frames("loop_start");
            let captured = whole("pantry_velocity").map(|v| v.clamp(1.0, 127.0) as u8);
            // Where the sustain pedal must be (`locc64`/`hicc64`): a piano recorded twice, with
            // the dampers down and lifted, plays one set or the other.
            let pedal = match (whole("locc64"), whole("hicc64")) {
                (Some(lo), _) if lo > 0.0 => PEDAL_DOWN,
                (_, Some(hi)) if hi < 127.0 => PEDAL_UP,
                _ => PEDAL_ANY,
            };
            // A velocity curve of its own (`amp_velcurve_N`): the level it has at the bottom of
            // its range against the top, kept as how far the velocity sets its level.
            let mut curve: Vec<(f64, f64)> = region
                .iter()
                .chain(group.iter())
                .chain(master.iter())
                .chain(global.iter())
                .filter_map(|(k, v)| Some((k.strip_prefix("amp_velcurve_")?.parse::<f64>().ok()?, v.trim().parse::<f64>().ok()?)))
                .filter(|(at, level)| (0.0..=128.0).contains(at) && (0.0..=1.0).contains(level))
                .collect();
            curve.sort_by(|a, b| a.0.total_cmp(&b.0));
            let shaped = match (curve.first(), curve.last()) {
                (Some(&(v0, g0)), Some(&(v1, g1))) if v1 > v0 && g1 > 0.0 => {
                    let top = v1.clamp(1.0, 127.0);
                    let fall = 1.0 - (v0.max(0.0) / top).powi(2);
                    Some((top as u8, if fall > 0.0 { ((1.0 - g0 / g1) / fall).clamp(0.0, 1.0) as f32 } else { 1.0 }))
                }
                _ => None,
            };
            let part = match get("pantry_part_name").or_else(|| get("master_label")).map(str::trim).filter(|n| !n.is_empty()) {
                Some(name) => name.to_string(),
                None => match trigger {
                    ON_KEY_DOWN if pedal == PEDAL_DOWN => "Notes pedal down",
                    ON_KEY_DOWN => "Notes",
                    ON_KEY_UP => "Release",
                    _ => "Pedal",
                }
                .to_string(),
            };
            let zone = ZoneSpec {
                lokey: lokey.min(hikey),
                hikey: lokey.max(hikey),
                root: note,
                lovel: lovel.min(hivel),
                hivel: lovel.max(hivel),
                seq: seq.unwrap_or(0),
                trigger,
                gain_db: whole("volume").map_or(0.0, |v| v.clamp(-144.0, 24.0) as f32),
                cents: (whole("tune").unwrap_or(0.0) + 100.0 * whole("transpose").unwrap_or(0.0)).clamp(-4800.0, 4800.0) as f32,
                start: frames("offset"),
                // (SFZ's `end` is the last frame played)
                end: whole("end").filter(|v| *v > 0.0).map_or(0, |v| (v.min(f64::from(u32::MAX - 1)) as u32).saturating_add(1)),
                loop_start: if looped { frames("loop_start") } else { 0 },
                loop_end: if looped { frames("loop_end").saturating_add(1) } else { 0 },
                xfade: if looped { frames("pantry_xfade") } else { 0 },
                keytrack: whole("pitch_keytrack").is_none_or(|v| v != 0.0),
                veltrack: whole("amp_veltrack").map_or(shaped.map_or(1.0, |s| s.1), |v| (v / 100.0).clamp(0.0, 1.0) as f32),
                ref_vel: whole("pantry_ref").map_or(shaped.map_or(127, |s| s.0), |v| v.clamp(1.0, 127.0) as u8),
                pedal,
                ..ZoneSpec::default()
            };
            out.push(Region {
                sample: format!("{dir}{sample}"),
                note,
                // A file Pantry saved says what each captured sample was played at.
                velocity: captured.unwrap_or(hivel),
                robin: usize::from(seq.unwrap_or(1)),
                captured: captured.is_some(),
                zone,
                part,
            });
        }
        region.clear();
    };
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        for word in line.split_whitespace() {
            if let Some(header) = word.strip_prefix('<').and_then(|w| w.strip_suffix('>')) {
                if scope == "region" {
                    finish(&mut region, &control, &global, &master, &group, &mut out);
                }
                scope = header.to_ascii_lowercase();
                open = None;
                match scope.as_str() {
                    "global" => {
                        global.clear();
                        master.clear();
                        group.clear();
                    }
                    "master" => {
                        master.clear();
                        group.clear();
                    }
                    "group" => group.clear(),
                    _ => {}
                }
                continue;
            }
            let target = match scope.as_str() {
                "control" => &mut control,
                "global" => &mut global,
                "master" => &mut master,
                "group" => &mut group,
                "region" => &mut region,
                _ => continue,
            };
            match word.split_once('=') {
                Some((k, v)) if !k.is_empty() => {
                    let k = k.to_ascii_lowercase();
                    target.insert(k.clone(), v.to_string());
                    open = Some(k);
                }
                _ => {
                    if let Some(v) = open.as_ref().and_then(|k| target.get_mut(k)) {
                        v.push(' ');
                        v.push_str(word);
                    }
                }
            }
        }
    }
    if scope == "region" {
        finish(&mut region, &control, &global, &master, &group, &mut out);
    }
    out
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.multisample_clear";
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    let ins = multisampler_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not the Multisampler"))?;
    let cleared = ins.samples.len();
    ins.samples.clear();
    Ok(json!({"track": t, "cleared": cleared}))
}

fn multisampler_mut(e: &mut Engine, t: TrackId) -> Option<&mut Insert> {
    e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()).filter(|i| i.plugin == MULTISAMPLER)
}

/// Largest sample read out of a library archive, in bytes.
const MAX_PACKED_SAMPLE_BYTES: u64 = 1 << 30;
/// How deep SFZ files may include one another.
const MAX_SFZ_INCLUDES: usize = 8;

/// The SFZ file at `path` with its `#include`s (paths relative to `root`, the main file's
/// folder) put in place and its `#define`d variables (`$NAME`) filled in, appended to `out`.
/// `budget` is the bytes that may still be read.
fn expand_sfz(path: &Path, root: &Path, depth: usize, defines: &mut Vec<(String, String)>, out: &mut String, budget: &mut usize) -> Result<()> {
    let bytes = crate::io::read_file_max(path, *budget as u64)
        .map_err(|_| EngineError::Io(format!("{}: missing, or too large for an SFZ file", path.display())))?;
    *budget = budget.saturating_sub(bytes.len());
    for line in String::from_utf8_lossy(&bytes).lines() {
        // The settings of a file Pantry saved are in a comment: kept for `load` to read.
        if line.trim_start().starts_with(PARAMS_LINE) {
            out.push_str(line.trim_start());
            out.push('\n');
            continue;
        }
        // Directives may stand anywhere on a line (`<group> #include "a" lovel=1 #include "b"`).
        let mut rest = line.split("//").next().unwrap_or("");
        loop {
            let (before, include) = match rest.find("#include") {
                Some(at) => (rest.get(..at).unwrap_or(""), rest.get(at + "#include".len()..)),
                None => (rest, None),
            };
            if let Some(define) = before.trim().strip_prefix("#define") {
                let mut words = define.split_whitespace();
                if let (Some(var), Some(value)) = (words.next().filter(|v| v.starts_with('$') && v.len() > 1), words.next()) {
                    defines.retain(|(k, _)| k != var);
                    defines.push((var.to_string(), value.to_string()));
                    // The longest name first, so `$VELTRACK` is not read as `$VEL` and `TRACK`.
                    defines.sort_by_key(|d| std::cmp::Reverse(d.0.len()));
                }
            } else if !before.trim().is_empty() {
                let mut text = before.to_string();
                if text.contains('$') {
                    for (var, value) in defines.iter() {
                        text = text.replace(var.as_str(), value);
                    }
                }
                out.push_str(&text);
                out.push(' ');
            }
            let Some(after) = include else { break };
            // The file's name in quotes, then the rest of the line.
            let after = after.trim_start();
            let (file, tail) = match after.strip_prefix('"').and_then(|a| a.split_once('"')) {
                Some((file, tail)) => (file, tail),
                None => (after.split_whitespace().next().unwrap_or(""), ""),
            };
            let file = file.replace('\\', "/");
            if !file.is_empty() && depth < MAX_SFZ_INCLUDES {
                expand_sfz(&root.join(&file), root, depth + 1, defines, out, budget)?;
            }
            rest = tail;
        }
        out.push('\n');
    }
    Ok(())
}

/// `sample` (as a preset inside a library archive names it) relative to the preset's folder
/// `base`, as an entry name: forward slashes, `.` and `..` resolved.
fn archive_path(base: &str, sample: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let joined = format!("{base}/{}", sample.replace('\\', "/"));
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// A Decent Sampler preset's samples (`.dspreset`), in file order, and the levels its controls
/// give its parts. A `<sample>` takes a value it lacks from its `<group>`, then from `<groups>`;
/// levels and tunings add up down that chain. A part is a group's first tag (else its name), and
/// its level the value of the control bound to that tag's volume. Only the mapping is read: no
/// interface, effects or envelopes.
fn parse_dspreset(text: &str) -> std::result::Result<(Vec<Region>, BTreeMap<String, f32>), String> {
    use crate::dawproject::xml::Element;
    fn walk(e: &Element, inherited: &[&Element], out: &mut Vec<Region>, levels: &mut BTreeMap<String, f32>) {
        // A control bound to a tag's volume: that part's level.
        if e.name == "control" {
            let bound = e.children_named("binding").find(|b| b.attr("level") == Some("tag") && b.attr("parameter") == Some("AMP_VOLUME"));
            if let (Some(tag), Some(db)) = (bound.and_then(|b| b.attr("identifier")), e.attr("value").and_then(level_db)) {
                levels.insert(tag.trim().to_string(), db);
            }
            return;
        }
        if e.name == "sample" {
            let get = |k: &str| e.attr(k).or_else(|| inherited.iter().rev().find_map(|g| g.attr(k)));
            // A value at every level of the chain (levels and tunings add up).
            let every = |k: &str| -> Vec<&str> { inherited.iter().copied().chain([e]).filter_map(|g| g.attr(k)).collect() };
            let whole = |k: &str| get(k).and_then(|v| v.trim().parse::<f64>().ok());
            let note = |k: &str| get(k).and_then(sfz_note);
            let enabled = get("enabled").is_none_or(|v| !v.trim().eq_ignore_ascii_case("false"));
            let pedal = whole("onLoCC64").or(whole("onHiCC64")).is_some();
            let trigger = match get("trigger").map(|t| t.trim().to_ascii_lowercase()).as_deref() {
                _ if pedal => Some(if whole("onLoCC64").unwrap_or(0.0) >= 64.0 { ON_PEDAL_DOWN } else { ON_PEDAL_UP }),
                None | Some("attack") => Some(ON_KEY_DOWN),
                Some("release") => Some(ON_KEY_UP),
                _ => None,
            };
            let path = e.attr("path").map(|p| p.trim().replace('\\', "/")).filter(|p| !p.is_empty());
            let (Some(path), Some(trigger), true) = (path, trigger, enabled) else { return };
            let (lokey, hikey) = (note("loNote").unwrap_or(0), note("hiNote").unwrap_or(127));
            let root = note("rootNote").or((lokey == hikey).then_some(lokey)).unwrap_or(60);
            let vel = |k: &str, d: u8| whole(k).map_or(d, |v| v.round().clamp(1.0, 127.0) as u8);
            let (lovel, hivel) = (vel("loVel", 1), vel("hiVel", 127));
            let seq = whole("seqPosition").map(|v| v.round().clamp(1.0, 63.0) as u8);
            let frames = |k: &str| whole(k).filter(|v| *v >= 0.0).map_or(0, |v| v.min(f64::from(u32::MAX - 1)) as u32);
            let looped = get("loopEnabled").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) && frames("loopEnd") > frames("loopStart");
            // The group's first tag names the part; a group without one, its own name.
            let group = inherited.iter().rev().find(|g| g.name == "group");
            let tag = group.and_then(|g| g.attr("tags")).and_then(|t| t.split(',').next()).map(str::trim).filter(|t| !t.is_empty());
            let part = tag.or_else(|| group.and_then(|g| g.attr("name")).map(str::trim).filter(|n| !n.is_empty())).unwrap_or("Notes").to_string();
            let zone = ZoneSpec {
                lokey: lokey.min(hikey),
                hikey: lokey.max(hikey),
                root,
                lovel: lovel.min(hivel),
                hivel: lovel.max(hivel),
                seq: seq.unwrap_or(0),
                trigger,
                gain_db: every("volume").into_iter().filter_map(level_db).sum::<f32>().clamp(-144.0, 24.0),
                cents: (100.0 * every("tuning").into_iter().filter_map(|v| v.trim().parse::<f32>().ok()).sum::<f32>()).clamp(-4800.0, 4800.0),
                start: frames("start"),
                end: frames("end").saturating_add(u32::from(frames("end") > 0)),
                loop_start: if looped { frames("loopStart") } else { 0 },
                loop_end: if looped { frames("loopEnd").saturating_add(1) } else { 0 },
                xfade: if looped { frames("loopCrossfade") } else { 0 },
                keytrack: whole("pitchKeyTrack").is_none_or(|v| v != 0.0),
                veltrack: whole("ampVelTrack").map_or(1.0, |v| v.clamp(0.0, 1.0) as f32),
                ..ZoneSpec::default()
            };
            out.push(Region { sample: path, note: root, velocity: hivel, robin: usize::from(seq.unwrap_or(1)), captured: false, zone, part });
            return;
        }
        let mut chain = inherited.to_vec();
        if e.name == "groups" || e.name == "group" {
            chain.push(e);
        }
        for c in &e.children {
            walk(c, &chain, out, levels);
        }
    }
    // Presets are written by hand: Decent Sampler opens ones a strict reader would refuse.
    let root = Element::parse_lenient(text)?;
    let (mut out, mut levels) = (Vec::new(), BTreeMap::new());
    walk(&root, &[], &mut out, &mut levels);
    Ok((out, levels))
}

/// Puts an instrument file's `regions` in track `t`'s Pantry in place of what it holds: the
/// captured samples of a file Pantry saved by note, velocity and take ([`attach`]), every other
/// one as a zone of its own, in parts named as the file names them (eight at most, the rest
/// joining the last) at the `levels` it gives them. `dir` is where the samples are: a folder, or
/// with `library` a folder inside that archive. Returns the samples and zones placed, the notes
/// and most layers a note has, what did not fit, and the parts' names.
fn attach_regions(
    e: &mut Engine,
    id: &str,
    t: TrackId,
    regions: &[Region],
    levels: &BTreeMap<String, f32>,
    dir: &Path,
    mut library: Option<&mut Library>,
    what: &str,
    written: &mut Vec<PathBuf>,
) -> Result<(usize, usize, usize, usize, Vec<String>)> {
    let packed = library.is_some();
    let place = |sample: &str| if packed { PathBuf::from(archive_path(&dir.to_string_lossy(), sample)) } else { dir.join(sample) };
    let captured: Vec<Wanted> = regions
        .iter()
        .filter(|r| r.captured)
        .map(|r| Wanted { note: r.note, velocity: r.velocity, robin: r.robin, file: place(&r.sample), packed })
        .collect();
    let (mut samples, mut notes, mut layers, mut skipped) = attach(e, id, t, &captured, true, what, library.as_deref_mut(), written)?;
    let zones: Vec<&Region> = regions.iter().filter(|r| !r.captured).collect();
    let mut parts: Vec<String> = Vec::new();
    for r in &zones {
        if !parts.contains(&r.part) && parts.len() < PARTS {
            parts.push(r.part.clone());
        }
    }
    let rate = e.session().sample_rate.hz();
    // A file several zones play is decoded once: its source, and its own sample rate.
    let mut decoded: BTreeMap<PathBuf, (soundcraft_model::SourceId, u32)> = BTreeMap::new();
    let mut total = 0usize;
    // Each part's layers of each note it holds.
    let mut ranges: BTreeMap<(usize, u8), BTreeSet<(u8, u8)>> = BTreeMap::new();
    for (index, r) in zones.iter().enumerate() {
        if index >= ZONES {
            skipped += 1;
            continue;
        }
        let file = place(&r.sample);
        let (src, file_rate) = match decoded.get(&file) {
            Some(found) => *found,
            None => {
                let (src, file_rate) = add_sample(e, &file, packed, library.as_deref_mut(), what, &mut total, written)?;
                decoded.insert(file.clone(), (src, file_rate));
                (src, file_rate)
            }
        };
        // The file counts its frames at its own rate; the session holds it at the session's.
        let scale = |f: u32| (f64::from(f) * f64::from(rate) / f64::from(file_rate)).round().min(f64::from(u32::MAX)) as u32;
        let part = parts.iter().position(|p| *p == r.part).unwrap_or(parts.len().saturating_sub(1));
        let spec = ZoneSpec {
            index,
            part,
            part_name: parts.get(part).cloned().unwrap_or_default(),
            start: scale(r.zone.start),
            end: scale(r.zone.end),
            loop_start: scale(r.zone.loop_start),
            loop_end: scale(r.zone.loop_end),
            xfade: scale(r.zone.xfade),
            ..r.zone.clone()
        };
        if spec.trigger == ON_KEY_DOWN {
            ranges.entry((spec.part, spec.root)).or_default().insert((spec.lovel, spec.hivel));
        }
        let ins = multisampler_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not the Multisampler"))?;
        ins.samples.insert(spec.key(), src);
        samples += 1;
    }
    if !zones.is_empty() {
        notes += ranges.keys().map(|(_, root)| *root).collect::<BTreeSet<_>>().len();
        layers = layers.max(ranges.values().map(BTreeSet::len).max().unwrap_or(0));
        // A library's levels follow the velocity from full scale, and its layers switch as it
        // was made to; its parts start at the levels its own controls give them.
        if let Some(ins) = multisampler_mut(e, t) {
            ins.params.insert("velocity_curve".into(), 2.0);
            ins.params.insert("layer_blend".into(), 0.0);
            for i in 0..PARTS {
                let level = parts.get(i).and_then(|name| levels.get(name)).copied().unwrap_or(0.0);
                ins.params.insert(format!("part{}_level", i + 1), level.clamp(PART_OFF_DB, 12.0));
            }
        }
    }
    Ok((samples, notes, layers, skipped, parts))
}

/// Adds the sample at `file` (with `packed`, an entry of `library`) to the session as a source
/// and returns it with the file's own sample rate. A WAV at the session's rate is played from
/// where it is, nothing of it decoded now; any other file is decoded (and brought to the
/// session's rate), then written out as a WAV to play from ([`crate::spill`]) when the session
/// has somewhere to write, its file added to `written`. `kept` counts what stays in memory.
fn add_sample(
    e: &mut Engine,
    file: &Path,
    packed: bool,
    library: Option<&mut Library>,
    what: &str,
    kept: &mut usize,
    written: &mut Vec<PathBuf>,
) -> Result<(soundcraft_model::SourceId, u32)> {
    let rate = e.session().sample_rate.hz();
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string();
    let too_long = || EngineError::Io(format!("{name}: too long for one note (two minutes at most)"));
    if !packed && let Some(f) = crate::io::streamable(file, rate) {
        if f.frames().saturating_mul(f.channels()) > MAX_SAMPLE_VALUES {
            return Err(too_long());
        }
        let audio = crate::io::streamed_audio(f, None, file, rate);
        let s = e.session_mut();
        let src = crate::io::insert_source(s, &name, audio, Some(&file.to_string_lossy()), FileFormat::Wav);
        if let Some(x) = s.sources.last_mut() {
            x.unsaved = false;
        }
        return Ok((src, rate));
    }
    let ext = file.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let bytes = match library.filter(|_| packed) {
        Some(lib) => lib.read(&file.to_string_lossy(), MAX_PACKED_SAMPLE_BYTES).map_err(EngineError::Io)?,
        None => crate::io::read_file(file)?,
    };
    let (format, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    if buf.frames() == 0 || buf.num_channels() == 0 {
        return Err(EngineError::Io(format!("{name}: no audio")));
    }
    let file_rate = buf.sample_rate.max(1);
    let buf = crate::io::at_rate(buf, rate, MAX_SAMPLE_VALUES).ok_or_else(too_long)?;
    let size = buf.frames().saturating_mul(buf.num_channels());
    // A sample out of an archive has no file of its own: it is saved with the session.
    let from = (!packed).then(|| file.to_string_lossy().into_owned());
    let src = crate::io::add_source(e.session_mut(), &name, buf, from.as_deref(), format.format);
    written.extend(crate::spill::spill_sources(e, "sample", Some(&[src])));
    if e.session().pool.get(src).is_some_and(|a| a.in_memory().is_some()) {
        *kept = kept.saturating_add(size);
        if *kept > crate::io::MAX_IMPORT_SAMPLES {
            return Err(EngineError::Io(format!("{what}: too much audio to hold in memory")));
        }
    }
    Ok((src, file_rate))
}

/// Removes the files a load that failed wrote for its samples (the session goes back to what
/// it was, so nothing names them).
fn discard(written: &[PathBuf]) {
    for f in written {
        let _ = crate::io::remove_path(f);
    }
}

/// A sample to put in a Pantry: the note it was recorded at, its velocity, its take (from 1) and
/// its file.
struct Wanted {
    note: u8,
    velocity: u8,
    robin: usize,
    /// The sample's file, or with `packed` its entry in the library archive being loaded.
    file: PathBuf,
    packed: bool,
}

/// A Decent Sampler library file (`.dslibrary`): a zip archive of presets and their samples.
type Library = crate::io::FileArchive;

/// Decodes `wanted` into track `t`'s Pantry (in place of what it holds with `replace`, else
/// beside it): a note keeps 16 velocities and a layer 4 takes, and a second sample for a taken
/// place is the layer's next take; what does not fit is skipped. Returns the samples placed, the
/// notes and most layers the Pantry then holds, and the samples skipped. A missing or broken
/// file fails the command, which leaves the session as it was.
fn attach(
    e: &mut Engine,
    id: &str,
    t: TrackId,
    wanted: &[Wanted],
    replace: bool,
    what: &str,
    mut library: Option<&mut Library>,
    written: &mut Vec<PathBuf>,
) -> Result<(usize, usize, usize, usize)> {
    let held: Vec<String> = match multisampler_mut(e, t) {
        Some(ins) if replace => {
            ins.samples.clear();
            Vec::new()
        }
        Some(ins) => ins.samples.keys().cloned().collect(),
        None => return Err(bad(id, "the track's instrument is not the Multisampler")),
    };
    let mut taken: BTreeSet<String> = held.iter().cloned().collect();
    let mut layers: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    for (note, velocity, _) in held.iter().filter_map(|k| parse_sample_key(k)) {
        layers.entry(note as u8).or_default().insert(velocity);
    }
    let mut placed: Vec<(String, &Wanted)> = Vec::new();
    let mut skipped = 0usize;
    for w in wanted {
        let of_note = layers.entry(w.note).or_default();
        if !of_note.contains(&w.velocity) && of_note.len() >= LAYERS {
            skipped += 1;
            continue;
        }
        let free = (w.robin.max(1)..=ROBINS).map(|robin| sample_key(w.note, w.velocity, robin)).find(|k| !taken.contains(k));
        match free {
            Some(key) if taken.len() < NOTES * LAYERS * ROBINS => {
                of_note.insert(w.velocity);
                taken.insert(key.clone());
                placed.push((key, w));
            }
            _ => skipped += 1,
        }
    }
    let mut total = 0usize;
    for (key, w) in &placed {
        let (src, _) = add_sample(e, &w.file, w.packed, library.as_deref_mut().filter(|_| w.packed), what, &mut total, written)?;
        let ins = multisampler_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not the Multisampler"))?;
        ins.samples.insert(key.clone(), src);
    }
    let notes = layers.values().filter(|v| !v.is_empty()).count();
    Ok((placed.len(), notes, layers.values().map(BTreeSet::len).max().unwrap_or(0), skipped))
}

/// The line of a saved file that carries the instrument's parameters, as JSON after it.
const PARAMS_LINE: &str = "//pantry-params ";

fn load(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.multisample_load";
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    if multisampler_mut(e, t).is_none() {
        return Err(bad(id, "the track's instrument is not the Multisampler"));
    }
    let path = Path::new(str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?).to_path_buf();
    let kind = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let shown = path.display().to_string();
    let none = || EngineError::Io(format!("{shown}: no samples to play (its key-down regions)"));
    let mut text = String::new();
    let mut library: Option<Library> = None;
    let mut presets: Vec<String> = Vec::new();
    let mut name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("Multisample").to_string();
    let mut levels = BTreeMap::new();
    let mut inside = dir.clone();
    let regions: Vec<Region> = match kind.as_str() {
        // A Decent Sampler library: one of its presets, the samples read out of the archive.
        "dslibrary" => {
            let mut lib = Library::new(crate::io::open_read(&path)?)
                .map_err(|_| EngineError::Io(format!("{shown}: not a Decent Sampler library (a zip archive)")))?;
            presets = lib.names().iter().filter(|n| n.to_ascii_lowercase().ends_with(".dspreset") && !n.starts_with("__MACOSX")).cloned().collect();
            presets.sort();
            let stem = |n: &str| Path::new(n).file_stem().and_then(|x| x.to_str()).unwrap_or(n).to_string();
            let entry = match str_param(p, "preset") {
                Some(want) => presets.iter().find(|n| stem(n).eq_ignore_ascii_case(want) || n.eq_ignore_ascii_case(want)),
                None => presets.first(),
            }
            .cloned()
            .ok_or_else(|| {
                EngineError::Io(format!(
                    "{shown}: no such preset in the library ({})",
                    presets.iter().map(|n| stem(n)).collect::<Vec<_>>().join(", ")
                ))
            })?;
            let xml = lib.read(&entry, MAX_SFZ_BYTES as u64).map_err(EngineError::Io)?;
            let (regions, found) = parse_dspreset(&String::from_utf8_lossy(&xml)).map_err(|err| EngineError::Io(format!("{entry}: {err}")))?;
            levels = found;
            inside = Path::new(&entry).parent().map(Path::to_path_buf).unwrap_or_default();
            name = stem(&entry);
            presets = presets.iter().map(|n| stem(n)).collect();
            library = Some(lib);
            regions
        }
        "dspreset" => {
            let xml = crate::io::read_file_max(&path, MAX_SFZ_BYTES as u64)?;
            let (regions, found) = parse_dspreset(&String::from_utf8_lossy(&xml)).map_err(|err| EngineError::Io(format!("{shown}: {err}")))?;
            levels = found;
            regions
        }
        _ => {
            let mut budget = MAX_SFZ_BYTES;
            expand_sfz(&path, &dir, 0, &mut Vec::new(), &mut text, &mut budget)?;
            parse_sfz(&text)
        }
    };
    if regions.is_empty() {
        return Err(none());
    }
    let mut written = Vec::new();
    let attached = attach_regions(e, id, t, &regions, &levels, &inside, library.as_mut(), &shown, &mut written);
    let (samples, notes, layers, skipped, parts) = attached.inspect_err(|_| discard(&written))?;
    // A file Pantry saved brings the instrument's settings with it.
    let saved: Option<BTreeMap<String, f64>> =
        text.lines().find_map(|l| l.trim().strip_prefix(PARAMS_LINE)).and_then(|j| serde_json::from_str(j).ok());
    if let (Some(saved), Some(info)) = (saved, soundcraft_dsp::plugin_info(MULTISAMPLER))
        && let Some(ins) = multisampler_mut(e, t)
    {
        for (k, v) in saved {
            if let Some(param) = info.param(&k) {
                ins.params.insert(k, param.clamp(v as f32));
            }
        }
    }
    Ok(
        json!({"track": t.0, "name": name, "samples": samples, "notes": notes, "layers": layers, "skipped": skipped, "presets": presets, "parts": parts}),
    )
}

/// File extensions a sample is imported from (what the audio decoder reads).
const AUDIO_EXTS: &[&str] = &["wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "aac", "mp4", "caf"];
/// Most files of a folder looked at for an import (a folder is user-controlled input).
const MAX_IMPORT_FILES: usize = 20_000;

/// A note by the name the app shows (60 is C3), a sharp written `#` or, as in a file name, `_`.
fn shown_note(text: &str) -> Option<u8> {
    let mut chars = text.chars();
    let class: i64 = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (class, rest) = match rest.chars().next()? {
        '#' | '_' => (class + 1, rest.get(1..)?),
        _ => (class, rest),
    };
    let octave: i64 = rest.parse().ok()?;
    u8::try_from(class.checked_add(octave.checked_add(2)?.checked_mul(12)?)?).ok().filter(|n| *n <= 127)
}

/// Where a sample goes by its file name (without the extension): the note it was recorded at,
/// its velocity and its take (from 1). Reads the names capture gives its samples
/// (`Piano Pantry D#1 v95 rr2`, as a file `Piano Pantry D_1 v95 rr2 747`), the ones Save to
/// Library and `tools/sampling/sampling.py` write (`039_D#1_v095_rr2`: the leading number is the
/// note), and anything of either shape: a `v` and a velocity, an optional `rr` and a take, and
/// before the velocity a note by name, or a MIDI note number first of all.
pub fn place_from_name(stem: &str) -> Option<(u8, u8, usize)> {
    let tagged = |t: &str, tag: &str| {
        t.strip_prefix(tag).filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())).and_then(|d| d.parse::<usize>().ok())
    };
    let read = |tokens: &[&str]| -> Option<(u8, u8, usize)> {
        let at = tokens.iter().position(|t| tagged(&t.to_ascii_lowercase(), "v").is_some())?;
        let velocity = u8::try_from(tagged(&tokens.get(at)?.to_ascii_lowercase(), "v")?).ok().filter(|v| (1..=127).contains(v))?;
        let robin = tokens.iter().find_map(|t| tagged(&t.to_ascii_lowercase(), "rr")).unwrap_or(1);
        let numbered =
            tokens.first().filter(|t| t.len() <= 3 && t.chars().all(|c| c.is_ascii_digit())).and_then(|t| t.parse::<u8>().ok()).filter(|n| *n <= 127);
        let note = numbered.or_else(|| at.checked_sub(1).and_then(|i| tokens.get(i)).and_then(|t| shown_note(t)))?;
        (1..=ROBINS).contains(&robin).then_some((note, velocity, robin))
    };
    // Words apart by spaces keep a sharp's `_` (`D_1`); a name joined by `_` has the number first.
    let spaced: Vec<&str> = stem.split_whitespace().collect();
    read(&spaced).or_else(|| read(&stem.split(['_', ' ']).filter(|t| !t.is_empty()).collect::<Vec<_>>()))
}

fn import(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.multisample_import";
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    if multisampler_mut(e, t).is_none() {
        return Err(bad(id, "the track's instrument is not the Multisampler"));
    }
    let mut files: Vec<PathBuf> = match p.get("paths") {
        None => Vec::new(),
        Some(v) => v
            .as_array()
            .and_then(|a| a.iter().map(|x| x.as_str().map(PathBuf::from)).collect::<Option<Vec<_>>>())
            .ok_or_else(|| bad(id, "`paths` must be a list of files"))?,
    };
    if let Some(folder) = str_param(p, "folder") {
        let mut found: Vec<PathBuf> = crate::io::list_dir(folder).into_iter().take(MAX_IMPORT_FILES).collect();
        if found.is_empty() {
            return Err(EngineError::Io(format!("{folder}: no files there")));
        }
        found.sort();
        files.extend(found);
    }
    if files.is_empty() {
        return Err(bad(id, "`folder` or `paths` required"));
    }
    let audio = |f: &PathBuf| f.extension().and_then(|x| x.to_str()).is_some_and(|x| AUDIO_EXTS.contains(&x.to_ascii_lowercase().as_str()));
    let mut unnamed = 0usize;
    let mut wanted = Vec::new();
    for file in files.into_iter().filter(audio) {
        match file.file_stem().and_then(|n| n.to_str()).and_then(place_from_name) {
            Some((note, velocity, robin)) => wanted.push(Wanted { note, velocity, robin, file, packed: false }),
            None => unnamed += 1,
        }
    }
    if wanted.is_empty() {
        return Err(bad(id, "no sample there says its note and velocity in its name (as `Piano C3 v95.wav` or `060_C3_v095_rr1.wav`)"));
    }
    let mut written = Vec::new();
    let attached = attach(e, id, t, &wanted, bool_or(p, "replace", false), "the samples", None, &mut written);
    let (samples, notes, layers, skipped) = attached.inspect_err(|_| discard(&written))?;
    Ok(json!({"track": t.0, "samples": samples, "notes": notes, "layers": layers, "skipped": skipped, "unnamed": unnamed}))
}

fn save(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.multisample_save";
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    let s = e.session_arc();
    let ins = s
        .track(t)
        .and_then(|tr| tr.instrument.as_ref())
        .filter(|i| i.plugin == MULTISAMPLER)
        .ok_or_else(|| bad(id, "the track's instrument is not the Multisampler"))?;
    let path = Path::new(str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?).to_path_buf();
    let path = if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("sfz")) { path } else { path.with_extension("sfz") };
    let stem = path.file_stem().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).ok_or_else(|| bad(id, "`path` needs a file name"))?.to_string();
    if crate::io::path_taken(&path) && !bool_or(p, "replace", false) {
        return Err(EngineError::Io(format!("{} already exists", path.display())));
    }
    // What it holds, by note, then velocity, then take.
    let mut held: Vec<(u8, u8, usize, soundcraft_model::SourceId)> =
        ins.samples.iter().filter_map(|(k, src)| parse_sample_key(k).map(|(n, v, r)| (n as u8, v, r + 1, *src))).collect();
    held.sort();
    let mut zones: Vec<(ZoneSpec, soundcraft_model::SourceId)> =
        ins.samples.iter().filter_map(|(k, src)| ZoneSpec::parse(k).map(|z| (z, *src))).collect();
    zones.sort_by_key(|(z, _)| z.index);
    if held.is_empty() && zones.is_empty() {
        return Err(bad(id, "the Multisampler holds no samples to save"));
    }
    let notes: Vec<u8> = held.iter().map(|h| h.0).collect::<BTreeSet<_>>().into_iter().collect();
    let folder = path.with_extension("");
    crate::file_access::check_write(&path).map_err(EngineError::Io)?;
    crate::io::make_dir(&folder)?;
    let params: BTreeMap<&str, f32> = ins.params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let release = ins.params.get("release").copied().unwrap_or(250.0) / 1000.0;
    let mut text = format!(
        "// {stem}: a Pantry instrument saved by FreeDAWm. Each sample is as it was captured, so\n// velocity does not scale the layers again.\n{PARAMS_LINE}{}\n<control> default_path={stem}/\n<global> amp_veltrack=0 ampeg_release={release:.3}\n",
        serde_json::to_string(&params).unwrap_or_else(|_| "{}".into())
    );
    let mut bytes = 0usize;
    for &(note, velocity, robin, src) in &held {
        let audio = s.pool.get(src).ok_or_else(|| EngineError::Io(format!("{}: a sample's audio is missing", note_name(note))))?;
        let file = format!("{note:03}_{}_v{velocity:03}_rr{robin}.wav", note_name(note).replace('#', "s"));
        let wav = crate::io::write_source_file(&folder.join(&file), audio)?;
        bytes = bytes.saturating_add(usize::try_from(wav).unwrap_or(usize::MAX));
        // The keys nearest the note, and the velocities from just above the layer below.
        let at = notes.iter().position(|n| *n == note).unwrap_or(0);
        let lokey = if at == 0 { 0 } else { notes.get(at - 1).map_or(0, |p| (u16::from(*p) + u16::from(note)) / 2 + 1) };
        let hikey = notes.get(at + 1).map_or(127, |n| (u16::from(note) + u16::from(*n)) / 2);
        let of_note: Vec<u8> = held.iter().filter(|h| h.0 == note).map(|h| h.1).collect::<BTreeSet<_>>().into_iter().collect();
        let layer = of_note.iter().position(|v| *v == velocity).unwrap_or(0);
        let lovel = if layer == 0 { 1 } else { of_note.get(layer - 1).map_or(1, |v| u16::from(*v) + 1) };
        let hivel = if layer + 1 == of_note.len() { 127 } else { velocity };
        let takes = held.iter().filter(|h| h.0 == note && h.1 == velocity).count();
        let seq = if takes > 1 {
            format!(" seq_length={takes} seq_position={}", held.iter().filter(|h| h.0 == note && h.1 == velocity && h.2 <= robin).count())
        } else {
            String::new()
        };
        text.push_str(&format!(
            "<region> sample={file} pitch_keycenter={note} lokey={lokey} hikey={hikey} lovel={lovel} hivel={hivel} pantry_velocity={velocity}{seq}\n"
        ));
    }
    // A library's zones, each spelled out; a file several play is written once.
    let mut written: BTreeMap<soundcraft_model::SourceId, String> = BTreeMap::new();
    for (z, src) in &zones {
        let file = match written.get(src) {
            Some(file) => file.clone(),
            None => {
                let audio = s.pool.get(*src).ok_or_else(|| EngineError::Io(format!("zone {}: its audio is missing", z.index)))?;
                let file = format!("z{:04}_{}.wav", z.index, note_name(z.root).replace('#', "s"));
                let wav = crate::io::write_source_file(&folder.join(&file), audio)?;
                bytes = bytes.saturating_add(usize::try_from(wav).unwrap_or(usize::MAX));
                written.insert(*src, file.clone());
                file
            }
        };
        let mut line = format!(
            "<region> sample={file} lokey={} hikey={} pitch_keycenter={} lovel={} hivel={} volume={:.1} tune={:.0} amp_veltrack={:.0} pantry_ref={} pantry_part_name={}",
            z.lokey,
            z.hikey,
            z.root,
            z.lovel,
            z.hivel,
            z.gain_db,
            z.cents,
            z.veltrack * 100.0,
            z.ref_vel,
            if z.part_name.is_empty() { format!("Part {}", z.part + 1) } else { z.part_name.clone() },
        );
        let mut add = |text: String| {
            line.push(' ');
            line.push_str(&text);
        };
        match z.pedal {
            PEDAL_UP => add("locc64=0 hicc64=63".into()),
            PEDAL_DOWN => add("locc64=64 hicc64=127".into()),
            _ => {}
        }
        match z.trigger {
            ON_KEY_UP => add("trigger=release".into()),
            ON_PEDAL_DOWN => add("on_locc64=64 on_hicc64=127".into()),
            ON_PEDAL_UP => add("on_locc64=0 on_hicc64=63".into()),
            _ => {}
        }
        if z.seq > 0 {
            add(format!("seq_position={}", z.seq));
        }
        if z.start > 0 {
            add(format!("offset={}", z.start));
        }
        if z.end > 0 {
            add(format!("end={}", z.end - 1));
        }
        if z.loop_end > 0 {
            add(format!("loop_mode=loop_continuous loop_start={} loop_end={} pantry_xfade={}", z.loop_start, z.loop_end - 1, z.xfade));
        }
        if !z.keytrack {
            add("pitch_keytrack=0".into());
        }
        text.push_str(&line);
        text.push('\n');
    }
    crate::io::write_file(&path.to_string_lossy(), text.as_bytes())?;
    Ok(json!({"path": path.to_string_lossy(), "name": stem, "samples": held.len() + zones.len(), "bytes": bytes.saturating_add(text.len())}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-multi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A generated tone (no recorded or third-party audio) as a 24-bit WAV.
    fn tone_wav(path: &Path, rate: u32) {
        let x: Vec<f32> = (0..rate as usize / 10).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
        let buf = AudioBuffer { sample_rate: rate, channels: vec![x] };
        let bytes =
            soundcraft_audio_io::encode(&buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, ..Default::default() }).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn track(e: &mut Engine, instrument: &str) -> TrackId {
        let v = e.execute("track.new", &json!({"kind": "instrument", "instrument": instrument})).unwrap();
        TrackId(v["tracks"][0].as_u64().unwrap())
    }

    /// A key-down sample in brief: its file, the note it was recorded at, the top of its
    /// velocity range and its take.
    fn region(sample: &str, note: u8, velocity: u8, robin: usize) -> (String, u8, u8, usize) {
        (sample.into(), note, velocity, robin)
    }

    fn key_down(regions: &[Region]) -> Vec<(String, u8, u8, usize)> {
        regions.iter().filter(|r| r.zone.trigger == ON_KEY_DOWN).map(|r| (r.sample.clone(), r.note, r.velocity, r.robin)).collect()
    }

    /// The zones a track's Pantry holds, by index.
    fn zones(e: &Engine, t: TrackId) -> Vec<ZoneSpec> {
        let ins = e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        let mut z: Vec<ZoneSpec> = ins.samples.keys().filter_map(|k| ZoneSpec::parse(k)).collect();
        z.sort_by_key(|z| z.index);
        z
    }

    #[test]
    fn notes_read_as_numbers_or_names() {
        assert_eq!(
            [sfz_note("60"), sfz_note("c4"), sfz_note("C#4"), sfz_note("db4"), sfz_note("a0"), sfz_note("c-1")],
            [Some(60), Some(60), Some(61), Some(61), Some(21), Some(0)]
        );
        assert_eq!([sfz_note("128"), sfz_note("-1"), sfz_note("h2"), sfz_note("c"), sfz_note("c99"), sfz_note("")], [None; 6]);
    }

    #[test]
    fn an_sfz_file_gives_its_key_down_regions() {
        let text = "// a comment\n<control> default_path=Samples\\\n<global> amp_veltrack=0 ampeg_release=0.4\n\
            <region> sample=060_C4_v040_rr1.wav pitch_keycenter=60 lokey=58 hikey=61 lovel=1 hivel=40 seq_length=2 seq_position=1\n\
            <region> sample=060_C4_v040_rr2.wav pitch_keycenter=60 lovel=1 hivel=40 seq_length=2 seq_position=2 // second take\n\
            <group> key=c5 hivel=90\n<region> sample=Grand Piano C5 soft.flac lovel=1\n<region> sample=loud.wav hivel=127\n\
            <region> sample=release.wav trigger=release\n<region> sample=*sine\n<region> lokey=1\n\
            <group>\n<region>\nsample=late.wav\nkey=72";
        assert_eq!(
            key_down(&parse_sfz(text)),
            [
                region("Samples/060_C4_v040_rr1.wav", 60, 40, 1),
                region("Samples/060_C4_v040_rr2.wav", 60, 40, 2),
                region("Samples/Grand Piano C5 soft.flac", 72, 90, 1),
                region("Samples/loud.wav", 72, 127, 1),
                region("Samples/late.wav", 72, 127, 1),
            ]
        );
        assert!(parse_sfz("").is_empty() && parse_sfz("sample=a.wav <nonsense> = == <region>").is_empty());
    }

    #[test]
    fn an_sfz_piano_recorded_pedal_up_and_down_maps_as_it_was_made() {
        // As a real library writes it: `key` with a lower `lokey` beside it, the two pedal
        // positions as masters, a velocity curve per layer.
        let text = "<master> locc64=0 hicc64=64\n<group> lovel=40 hivel=55 amp_velcurve_40=.68 amp_velcurve_55=1\n\
                    <region> sample=up a.wav key=a1 lokey=g1\n<region> sample=up c.wav key=c2 lokey=a#1\n\
                    <master> locc64=16 hicc64=127\n<group> lovel=40 hivel=55\n<region> sample=down a.wav key=a1 lokey=g1\n";
        let r = parse_sfz(text);
        let z: Vec<_> = r.iter().map(|r| (r.zone.lokey, r.zone.hikey, r.zone.root, r.zone.pedal, r.part.as_str())).collect();
        assert_eq!(z, [(31, 33, 33, PEDAL_UP, "Notes"), (34, 36, 36, PEDAL_UP, "Notes"), (31, 33, 33, PEDAL_DOWN, "Notes pedal down")]);
        // The layer is 0.68 as loud at its softest as at its hardest, and full at 55.
        let up = &r[0].zone;
        assert_eq!(up.ref_vel, 55);
        let at = |vel: f32| 1.0 - up.veltrack + up.veltrack * (vel / 55.0).powi(2);
        assert!((at(40.0) - 0.68).abs() < 0.01 && (at(55.0) - 1.0).abs() < 1e-6, "{}", up.veltrack);
        assert_eq!(r[2].zone.ref_vel, 127);
        // A zone's key spells its pedal position and reads back.
        assert_eq!(ZoneSpec::parse(&r[2].zone.key()).map(|z| z.pedal), Some(PEDAL_DOWN));
    }

    #[test]
    fn loading_an_sfz_file_gives_the_multisampler_its_samples() {
        let d = dir("load");
        for f in ["a.wav", "b.wav", "c.wav", "d.wav"] {
            tone_wav(&d.join(f), 44_100);
        }
        let sfz = d.join("Tones.sfz");
        std::fs::write(
            &sfz,
            "<region> sample=a.wav pitch_keycenter=48 hivel=60\n<region> sample=b.wav pitch_keycenter=48 hivel=127\n\
             <region> sample=c.wav key=60 seq_position=1\n<region> sample=d.wav key=60 seq_position=1\n",
        )
        .unwrap();
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!(
            (v["name"].as_str(), v["samples"].as_u64(), v["notes"].as_u64(), v["layers"].as_u64(), v["skipped"].as_u64()),
            (Some("Tones"), Some(4), Some(2), Some(2), Some(0))
        );
        let ins = e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        // Each region is a zone of its own: two layers of one note, and two samples that sound
        // together on another.
        let z = zones(&e, t);
        assert_eq!(
            z.iter().map(|z| (z.root, z.lovel, z.hivel, z.seq)).collect::<Vec<_>>(),
            [(48, 1, 60, 0), (48, 1, 127, 0), (60, 1, 127, 1), (60, 1, 127, 1)]
        );
        assert!(z.iter().all(|z| z.part == 0 && z.part_name == "Notes" && z.ref_vel == 127));
        assert_eq!(ins.params.get("velocity_curve"), Some(&2.0));
        assert!(ins.samples.values().all(|s| e.session().source(*s).is_some()));
        // Loading again replaces what was there; one undo step takes a load back.
        std::fs::write(&sfz, "<region> sample=a.wav key=50\n").unwrap();
        e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        let keys = |e: &Engine| e.session().track(t).and_then(|tr| tr.instrument.as_ref()).map(|i| i.samples.len());
        assert_eq!(keys(&e), Some(1));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(keys(&e), Some(4));
    }

    #[test]
    fn loaded_samples_play_from_disk_not_from_memory() {
        if !soundcraft_model::disk::streaming_enabled() {
            return;
        }
        let d = dir("stream");
        let long = |path: &Path, rate: u32| {
            let x: Vec<f32> = (0..rate as usize * 2).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
            let buf = AudioBuffer { sample_rate: rate, channels: vec![x] };
            let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, ..Default::default() };
            std::fs::write(path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
        };
        let mut e = Engine { unsaved_audio_root: Some(d.join("unsaved")), ..Default::default() };
        let rate = e.session().sample_rate.hz();
        let other = if rate == 44_100 { 48_000 } else { 44_100 };
        long(&d.join("native.wav"), rate);
        long(&d.join("other.wav"), other);
        tone_wav(&d.join("short.wav"), rate);
        let sfz = d.join("Stream.sfz");
        std::fs::write(&sfz, "<region> sample=native.wav key=48\n<region> sample=other.wav key=50\n<region> sample=short.wav key=52\n").unwrap();
        let t = track(&mut e, MULTISAMPLER);
        e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        let s = e.session();
        let ins = s.track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        let of = |root: u8| {
            let key = ins.samples.keys().find(|k| ZoneSpec::parse(k).is_some_and(|z| z.root == root)).unwrap();
            let id = ins.samples[key];
            (s.source(id).unwrap().clone(), std::sync::Arc::clone(s.pool.get(id).unwrap()))
        };
        // A WAV at the session's rate plays from where it is.
        let (src, audio) = of(48);
        assert_eq!(Path::new(&src.path), d.join("native.wav"));
        assert!(audio.in_memory().is_none() && audio.has_head() && !src.unsaved);
        assert_eq!(audio.frames(), rate as usize * 2);
        // One at another rate is brought to the session's and written out to play from.
        let (src, audio) = of(50);
        assert!(Path::new(&src.path).starts_with(d.join("unsaved")), "{}", src.path);
        assert!(audio.in_memory().is_none() && audio.has_head());
        assert_eq!(audio.frames(), rate as usize * 2);
        let mut head = vec![0.0f32; 64];
        assert!(audio.read_ready(0, 0, &mut head));
        assert!(head.iter().fold(0.0f32, |m, v| m.max(v.abs())) > 0.2);
        // A short one is not worth a file: it stays in memory.
        assert!(of(52).1.in_memory().is_some());
        // An offline render plays them as it plays any sample: capturing this Pantry's notes
        // (the three it holds, off disk and out of memory) gives three that sound.
        let v = e
            .execute(
                "track.capture_multisample",
                &json!({"track": t.0, "low": 48, "high": 52, "step": 2, "layers": 1, "hold": 0.3, "tail": 0.3, "release": false, "pedal": false}),
            )
            .unwrap();
        assert_eq!((v["samples"].as_u64(), v["silent"].as_u64()), (Some(3), Some(0)));
        // A load that fails leaves no files behind.
        let before = std::fs::read_dir(d.join("unsaved"))
            .unwrap()
            .flatten()
            .flat_map(|x| std::fs::read_dir(x.path().join("Audio Files")).unwrap().flatten())
            .count();
        std::fs::write(&sfz, "<region> sample=other.wav key=50\n<region> sample=missing.wav key=52\n").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).is_err());
        let after = std::fs::read_dir(d.join("unsaved"))
            .unwrap()
            .flatten()
            .flat_map(|x| std::fs::read_dir(x.path().join("Audio Files")).unwrap().flatten())
            .count();
        assert_eq!(before, after);
    }

    #[test]
    fn a_bad_load_changes_nothing() {
        let d = dir("bad");
        tone_wav(&d.join("a.wav"), 48_000);
        let sfz = d.join("x.sfz");
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        std::fs::write(&sfz, "<region> sample=a.wav key=50\n").unwrap();
        e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        let before = e.session().track(t).and_then(|tr| tr.instrument.clone());
        // A missing sample, a file with no regions, and a track playing something else.
        std::fs::write(&sfz, "<region> sample=a.wav key=50\n<region> sample=missing.wav key=52\n").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).is_err());
        std::fs::write(&sfz, "<global> volume=3\n").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).is_err());
        assert_eq!(e.session().track(t).and_then(|tr| tr.instrument.clone()), before);
        let other = track(&mut e, "sampler");
        std::fs::write(&sfz, "<region> sample=a.wav key=50\n").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": other.0, "path": sfz.to_string_lossy()})).is_err());
    }

    #[test]
    fn capturing_a_track_makes_a_pantry_of_its_notes() {
        let mut e = Engine::default();
        let synth = track(&mut e, "subtractive_synth");
        let v = e
            .execute(
                "track.capture_multisample",
                &json!({"track": synth.0, "low": 48, "high": 60, "step": 6, "layers": 2, "robins": 2, "hold": 0.2, "tail": 0.6, "name": "Keys"}),
            )
            .unwrap();
        assert_eq!((v["samples"].as_u64(), v["notes"].as_u64(), v["layers"].as_u64(), v["silent"].as_u64()), (Some(12), Some(3), Some(2), Some(0)));
        let made = TrackId(v["track"].as_u64().unwrap());
        let s = e.session();
        let tr = s.track(made).unwrap();
        let ins = tr.instrument.as_ref().unwrap();
        assert_eq!((tr.name.as_str(), ins.plugin.as_str(), tr.kind), ("Keys", MULTISAMPLER, TrackKind::Instrument));
        assert_eq!(ins.params.len(), soundcraft_dsp::plugin_info(MULTISAMPLER).unwrap().params.len());
        let keys: Vec<&str> = ins.samples.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "n48.v127.r1",
                "n48.v127.r2",
                "n48.v64.r1",
                "n48.v64.r2",
                "n54.v127.r1",
                "n54.v127.r2",
                "n54.v64.r1",
                "n54.v64.r2",
                "n60.v127.r1",
                "n60.v127.r2",
                "n60.v64.r1",
                "n60.v64.r2"
            ]
        );
        // Each note starts with its sound, holds no more than its slot and ends in silence.
        let sr = s.sample_rate.hz() as usize;
        for src in ins.samples.values() {
            let audio = s.pool.get(*src).unwrap();
            let ch = audio.channel(0).unwrap();
            let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(ch.len() > sr / 10 && ch.len() <= sr * 8 / 10, "{}", ch.len());
            assert!(peak(&ch[..sr / 20]) > 0.01 && ch.last().copied().unwrap().abs() < 1.0e-4);
        }
        // The harder strike is the louder sample.
        let level = |key: &str| {
            let audio = s.pool.get(ins.samples[key]).unwrap();
            audio.channel(0).unwrap().iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        assert!(level("n60.v127.r1") > level("n60.v64.r1") * 1.2);
        // The new track plays what was captured: capturing it in turn gives the same notes.
        let again =
            e.execute("track.capture_multisample", &json!({"track": made.0, "low": 50, "high": 50, "layers": 1, "hold": 0.2, "tail": 0.6})).unwrap();
        assert_eq!((again["samples"].as_u64(), again["silent"].as_u64()), (Some(1), Some(0)));
        e.execute("edit.undo", &json!({})).unwrap();
        // Cleared, it holds nothing; undo gives the samples back.
        assert_eq!(e.execute("mix.multisample_clear", &json!({"track": made.0})).unwrap()["cleared"], 12);
        assert!(e.session().track(made).unwrap().instrument.as_ref().unwrap().samples.is_empty());
        e.execute("edit.undo", &json!({})).unwrap();
        let s = e.session();
        // The source track is as it was, and one undo step takes the capture back.
        assert!(s.track(synth).unwrap().clips().is_empty());
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().track(made).is_none());
    }

    #[test]
    fn a_capture_job_runs_in_slices_and_the_command_takes_what_it_made() {
        let mut e = Engine::default();
        let synth = track(&mut e, "subtractive_synth");
        let p = json!({"track": synth.0, "low": 48, "high": 54, "step": 3, "layers": 2, "hold": 0.2, "tail": 0.4});
        let mut job = CaptureJob::new(&e, &p).unwrap();
        assert_eq!((job.counts(), job.note().as_deref(), job.progress()), ((0, 3), Some("C2"), 0.0));
        // A block at a time: progress only grows, and the session is untouched meanwhile.
        let (mut steps, mut last) = (0, 0.0);
        while !job.step(|| false).unwrap() {
            assert!(job.progress() >= last);
            last = job.progress();
            steps += 1;
        }
        assert!(steps > 10 && job.counts() == (3, 3) && job.note().is_none());
        assert_eq!(e.session().tracks.len(), 1);
        assert_eq!(job.command(), ("track.capture_multisample", &p));
        e.offer_capture(job.finish().unwrap());
        let v = e.execute("track.capture_multisample", &p).unwrap();
        assert_eq!((v["samples"].as_u64(), v["notes"].as_u64(), v["layers"].as_u64()), (Some(6), Some(3), Some(2)));
        // A capture offered for other parameters is not used (the command renders its own).
        let mut other = CaptureJob::new(&e, &p).unwrap();
        while !other.step(|| true).unwrap() {}
        e.offer_capture(other.finish().unwrap());
        let q = json!({"track": synth.0, "low": 60, "high": 60, "layers": 1, "hold": 0.2, "tail": 0.4});
        assert_eq!(e.execute("track.capture_multisample", &q).unwrap()["samples"], 1);
        // What was offered and not used is gone with the command.
        assert!(e.take_capture().is_none());
    }

    #[test]
    fn capture_refuses_what_it_cannot_play() {
        let mut e = Engine::default();
        let audio = TrackId(e.execute("track.new", &json!({})).unwrap()["tracks"][0].as_u64().unwrap());
        assert!(e.execute("track.capture_multisample", &json!({"track": audio.0})).is_err(), "no instrument");
        let synth = track(&mut e, "subtractive_synth");
        for bad in [
            json!({"low": 70, "high": 60}),
            json!({"step": 0}),
            json!({"layers": 17}),
            json!({"robins": 5}),
            json!({"velocities": [0]}),
            json!({"hold": 0}),
            json!({"tail": 99}),
        ] {
            let mut p = bad.clone();
            p["track"] = json!(synth.0);
            assert!(e.execute("track.capture_multisample", &p).is_err(), "{bad}");
        }
        // An empty Pantry makes no sound.
        let empty = track(&mut e, MULTISAMPLER);
        assert!(e.execute("track.capture_multisample", &json!({"track": empty.0, "low": 60, "high": 60, "hold": 0.1, "tail": 0.1})).is_err());
    }

    #[test]
    fn samples_say_where_they_go_in_their_names() {
        // As capture names them, as a saved session's files, and with takes.
        assert_eq!(place_from_name("Natural Liberty A0 v11 721"), Some((33, 11, 1)));
        assert_eq!(place_from_name("Natural Liberty D_1 v127 756"), Some((39, 127, 1)));
        assert_eq!(place_from_name("Piano Pantry D#1 v95 rr2"), Some((39, 95, 2)));
        assert_eq!(place_from_name("Keys A-1 v64"), Some((21, 64, 1)));
        assert_eq!(place_from_name("Keys C_-2 v1"), Some((1, 1, 1)));
        // As Save to Library and the sampling tool write them: the number is the note.
        assert_eq!(place_from_name("051_Ds1_v095_rr2"), Some((51, 95, 2)));
        assert_eq!(place_from_name("060_C4_v100_rr1"), Some((60, 100, 1)));
        assert_eq!(place_from_name("C3_v64"), Some((60, 64, 1)));
        for bad in
            ["", "Piano", "Piano C3", "Piano v95", "Piano C3 v0", "Piano C3 v128", "Piano H3 v64", "Piano C3 v64 rr5", "Piano C99 v64", "200_x_v64"]
        {
            assert_eq!(place_from_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_saved_pantry_loads_back_as_it_was_and_loose_samples_import_by_name() {
        let d = dir("save");
        let mut e = Engine::default();
        let synth = track(&mut e, "subtractive_synth");
        let made = e
            .execute(
                "track.capture_multisample",
                &json!({"track": synth.0, "low": 48, "high": 51, "step": 3, "velocities": [40, 100], "robins": 2, "hold": 0.2, "tail": 0.4}),
            )
            .unwrap();
        let made = TrackId(made["track"].as_u64().unwrap());
        e.execute("mix.instrument_param", &json!({"track": made.0, "param": "layer_blend", "value": 80.0})).unwrap();
        let pantry = |e: &Engine, t: TrackId| e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        let sfz = d.join("Keys.sfz");
        let v = e.execute("mix.multisample_save", &json!({"track": made.0, "path": d.join("Keys").to_string_lossy()})).unwrap();
        assert_eq!((v["samples"].as_u64(), v["name"].as_str(), v["path"].as_str()), (Some(8), Some("Keys"), Some(&*sfz.to_string_lossy())));
        let text = std::fs::read_to_string(&sfz).unwrap();
        assert!(text.contains(
            "<region> sample=048_C2_v040_rr1.wav pitch_keycenter=48 lokey=0 hikey=49 lovel=1 hivel=40 pantry_velocity=40 seq_length=2 seq_position=1"
        ));
        assert!(text.contains(
            "sample=051_Ds2_v100_rr2.wav pitch_keycenter=51 lokey=50 hikey=127 lovel=41 hivel=127 pantry_velocity=100 seq_length=2 seq_position=2"
        ));
        assert!(d.join("Keys").join("051_Ds2_v100_rr2.wav").is_file());
        // A second save needs `replace`; nothing was undoable about the first.
        assert!(e.execute("mix.multisample_save", &json!({"track": made.0, "path": sfz.to_string_lossy()})).is_err());
        e.execute("mix.multisample_save", &json!({"track": made.0, "path": sfz.to_string_lossy(), "replace": true})).unwrap();
        // Loaded into another Pantry: the same places, the same audio, the same settings.
        let other = track(&mut e, MULTISAMPLER);
        let v = e.execute("mix.multisample_load", &json!({"track": other.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!((v["samples"].as_u64(), v["skipped"].as_u64()), (Some(8), Some(0)));
        let (a, b) = (pantry(&e, made), pantry(&e, other));
        assert_eq!(a.samples.keys().collect::<Vec<_>>(), b.samples.keys().collect::<Vec<_>>());
        assert_eq!(b.params.get("layer_blend"), Some(&80.0));
        let s = e.session();
        for (key, src) in &a.samples {
            let (x, y) = (s.pool.get(*src).unwrap(), s.pool.get(b.samples[key]).unwrap());
            assert_eq!(x.channel(0), y.channel(0), "{key}");
        }
        // The saved samples as loose files: imported by name beside what is there, or in its place.
        let third = track(&mut e, MULTISAMPLER);
        std::fs::write(d.join("Keys").join("notes.txt"), "x").unwrap();
        std::fs::copy(d.join("Keys").join("048_C2_v040_rr1.wav"), d.join("Keys").join("stray.wav")).unwrap();
        let v = e.execute("mix.multisample_import", &json!({"track": third.0, "folder": d.join("Keys").to_string_lossy()})).unwrap();
        assert_eq!((v["samples"].as_u64(), v["notes"].as_u64(), v["layers"].as_u64(), v["unnamed"].as_u64()), (Some(8), Some(2), Some(2), Some(1)));
        assert_eq!(pantry(&e, third).samples.keys().collect::<Vec<_>>(), a.samples.keys().collect::<Vec<_>>());
        let one = d.join("Keys").join("Grand F_2 v77 12.wav");
        std::fs::copy(d.join("Keys").join("048_C2_v040_rr1.wav"), &one).unwrap();
        let v = e.execute("mix.multisample_import", &json!({"track": third.0, "paths": [one.to_string_lossy()]})).unwrap();
        assert_eq!((v["samples"].as_u64(), v["notes"].as_u64()), (Some(1), Some(3)));
        assert!(pantry(&e, third).samples.contains_key("n54.v77.r1"));
        let v = e.execute("mix.multisample_import", &json!({"track": third.0, "paths": [one.to_string_lossy()], "replace": true})).unwrap();
        assert_eq!((v["samples"].as_u64(), pantry(&e, third).samples.len()), (Some(1), 1));
        // Nothing named, nothing given, nothing held.
        assert!(
            e.execute("mix.multisample_import", &json!({"track": third.0, "paths": [d.join("Keys").join("stray.wav").to_string_lossy()]})).is_err()
        );
        assert!(e.execute("mix.multisample_import", &json!({"track": third.0})).is_err());
        let empty = track(&mut e, MULTISAMPLER);
        assert!(e.execute("mix.multisample_save", &json!({"track": empty.0, "path": d.join("Empty").to_string_lossy()})).is_err());
    }

    #[test]
    fn sfz_files_include_one_another_and_fill_in_their_variables() {
        let d = dir("include");
        std::fs::create_dir_all(d.join("maps")).unwrap();
        for f in ["A_soft.wav", "A_hard.wav", "pedal.wav", "rel.wav"] {
            tone_wav(&d.join(f), 48_000);
        }
        let sfz = d.join("Main.sfz");
        std::fs::write(&sfz, "#define $SOFT 50 // the soft layer's top\n#define $NAME A\n<global> ampeg_release=1\n#include \"maps/notes.sfz\"\n<group> on_locc64=64\n<region> sample=pedal.wav key=60\n").unwrap();
        std::fs::write(
            d.join("maps").join("notes.sfz"),
            "<group> lokey=57 hikey=57\n<region> sample=$NAME_soft.wav hivel=$SOFT\n<region> sample=$NAME_hard.wav lovel=51\n<region> sample=rel.wav trigger=release\n",
        )
        .unwrap();
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!((v["samples"].as_u64(), v["notes"].as_u64(), v["layers"].as_u64()), (Some(4), Some(1), Some(2)));
        // The two layers of the note, its release sample and the pedal's noise, each in its part.
        let z = zones(&e, t);
        assert_eq!(
            z.iter().map(|z| (z.trigger, z.lovel, z.hivel)).collect::<Vec<_>>(),
            [(ON_KEY_DOWN, 1, 50), (ON_KEY_DOWN, 51, 127), (ON_KEY_UP, 1, 127), (ON_PEDAL_DOWN, 1, 127)]
        );
        assert_eq!(v["parts"], json!(["Notes", "Release", "Pedal"]));
        // Includes in the middle of a line, a variable that starts like another, and a keyswitched
        // second version of the same notes, which is left out.
        std::fs::write(d.join("maps").join("v1.sfz"), "#define $VEL soft\n#define $VELTRACK 99\n").unwrap();
        std::fs::write(d.join("maps").join("r.sfz"), "<region> sample=A_$VEL.wav key=57 amp_veltrack_oncc$VELTRACK=1\n").unwrap();
        std::fs::write(
            &sfz,
            "<global> sw_default=c0\n<master> sw_last=c0\n<group> #include \"maps/v1.sfz\" hivel=40 #include \"maps/r.sfz\"\n<master> sw_last=c#0\n<group> hivel=40 #include \"maps/r.sfz\"\n",
        )
        .unwrap();
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!((v["samples"].as_u64(), v["skipped"].as_u64()), (Some(1), Some(0)));
        // A file that includes itself, or one that is not there, fails without hanging.
        std::fs::write(&sfz, "#include \"Main.sfz\"\n<region> sample=A_soft.wav key=57\n").unwrap();
        // (nine copies of its one region)
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!((v["samples"].as_u64(), v["skipped"].as_u64()), (Some(9), Some(0)));
        std::fs::write(&sfz, "#include \"gone.sfz\"\n").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).is_err());
    }

    const DSPRESET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<DecentSampler minVersion="1.0.0">
  <ui><tab><labeled-knob label="Tone"/></tab></ui>
  <groups attack="0.0" release="0.4" hiVel="127">
    <group loVel="1" hiVel="60">
      <sample path="Samples/c_soft.wav" rootNote="48" loNote="46" hiNote="50"/>
      <sample path="Samples\e_soft.wav" loNote="52" hiNote="52"/>
    </group>
    <group loVel="61" seqMode="round_robin">
      <sample path="Samples/c_hard_1.wav" rootNote="48" seqPosition="1"/>
      <sample path="Samples/c_hard_2.wav" rootNote="48" seqPosition="2"/>
    </group>
    <group trigger="release"><sample path="Samples/c_rel.wav" rootNote="48"/></group>
    <group enabled="false"><sample path="Samples/c_off.wav" rootNote="48"/></group>
  </groups>
</DecentSampler>"#;

    #[test]
    fn a_decent_sampler_preset_gives_its_key_down_samples() {
        assert_eq!(
            key_down(&parse_dspreset(DSPRESET).unwrap().0),
            [
                region("Samples/c_soft.wav", 48, 60, 1),
                region("Samples/e_soft.wav", 52, 60, 1),
                region("Samples/c_hard_1.wav", 48, 127, 1),
                region("Samples/c_hard_2.wav", 48, 127, 2),
            ]
        );
        assert!(parse_dspreset("<DecentSampler><groups/></DecentSampler>").unwrap().0.is_empty());
        assert!(parse_dspreset("not xml <").is_err());
        assert_eq!(archive_path("Piano/Presets", "../Samples/./a.wav"), "Piano/Samples/a.wav");
        assert_eq!(archive_path("", "Samples\\a.wav"), "Samples/a.wav");
        assert_eq!(archive_path("", "../../a.wav"), "a.wav");
    }

    /// A preset with what a sampled piano's has: a looped pad under the notes, layers with their
    /// own levels and tunings, a start past the file's lead-in, release samples, a pedal noise,
    /// and controls that set each part's level.
    const LAYERED: &str = r#"<DecentSampler>
  <ui><tab>
    <control parameterName="Notes" value="0.5"><binding type="amp" level="tag" identifier="notes" parameter="AMP_VOLUME"/></control>
    <control parameterName="Texture" value="0"><binding type="amp" level="tag" identifier="pad" parameter="AMP_VOLUME"/></control>
    <control parameterName="Cutoff" value="22000" style="rotary" width="80" style="linear_bar_vertical"><binding type="effect" level="instrument" position="0" parameter="FX_FILTER_FREQUENCY"/></control>
  </tab></ui>
  <groups volume="-3dB">
    <group name="PAD" tags="pad" volume="3.0dB">
      <sample path="Samples/c_soft.wav" rootNote="48" loNote="40" hiNote="52" start="441" loopEnabled="true" loopStart="882" loopEnd="2204" loopCrossfade="44"/>
    </group>
    <group name="SOFT" tags="notes" tuning="0.5">
      <sample path="Samples/c_soft.wav" volume="2.0dB" tuning="0.25" rootNote="48" loNote="44" hiNote="48" loVel="1" hiVel="70"/>
    </group>
    <group name="HARD" tags="notes">
      <sample path="Samples/c_hard_1.wav" rootNote="48" loNote="44" hiNote="48" loVel="71" hiVel="127" end="2204"/>
    </group>
    <group name="RT" tags="release" trigger="release"><sample path="Samples/c_rel.wav" rootNote="48" loNote="44" hiNote="48"/></group>
    <group name="PEDAL" tags="pedal">
      <sample path="Samples/e_soft.wav" rootNote="0" loNote="127" hiNote="127" onLoCC64="90" onHiCC64="127" pitchKeyTrack="0"/>
    </group>
  </groups>
</DecentSampler>"#;

    #[test]
    fn a_layered_preset_loads_as_parts_with_their_levels_and_saves_and_loads_back() {
        let d = dir("layered");
        std::fs::create_dir_all(d.join("Samples")).unwrap();
        for f in ["c_soft.wav", "e_soft.wav", "c_hard_1.wav", "c_rel.wav"] {
            tone_wav(&d.join("Samples").join(f), 44_100);
        }
        let preset = d.join("Layered.dspreset");
        std::fs::write(&preset, LAYERED).unwrap();
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": preset.to_string_lossy()})).unwrap();
        assert_eq!(
            (v["samples"].as_u64(), v["notes"].as_u64(), v["layers"].as_u64(), &v["parts"]),
            (Some(5), Some(1), Some(2), &json!(["pad", "notes", "release", "pedal"]))
        );
        let z = zones(&e, t);
        let pantry = |e: &Engine, t: TrackId| e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        let ins = pantry(&e, t);
        // The pad: its part, its keys, and (at the session's 48 kHz, from the file's 44.1) where
        // it starts and loops; the group's and the instrument's levels add up.
        let pad = &z[0];
        assert_eq!((pad.part, pad.part_name.as_str(), pad.lokey, pad.hikey, pad.root), (0, "pad", 40, 52, 48));
        assert_eq!((pad.start, pad.loop_start, pad.loop_end, pad.xfade, pad.gain_db), (480, 960, 2400, 48, 0.0));
        // The soft layer: 2 dB over the instrument's -3, the group's and its own tuning together.
        assert_eq!((z[1].part, z[1].lovel, z[1].hivel, z[1].gain_db, z[1].cents), (1, 1, 70, -1.0, 75.0));
        assert_eq!((z[2].part, z[2].lovel, z[2].hivel, z[2].end), (1, 71, 127, 2400));
        assert_eq!((z[3].part, z[3].trigger), (2, ON_KEY_UP));
        assert_eq!((z[4].part, z[4].trigger, z[4].keytrack), (3, ON_PEDAL_DOWN, false));
        // The file the pad and the soft layer both play is one source.
        assert_eq!(ins.samples[&z[0].key()], ins.samples[&z[1].key()]);
        assert_eq!(ins.samples.values().collect::<BTreeSet<_>>().len(), 4);
        // Each part at the level its control gives it: the pad off, the notes at half.
        assert_eq!(ins.params["part1_level"], PART_OFF_DB);
        assert!((ins.params["part2_level"] + 6.02).abs() < 0.01);
        assert_eq!((ins.params["part3_level"], ins.params["layer_blend"], ins.params["velocity_curve"]), (0.0, 0.0, 2.0));
        // Saved to a file and loaded into another Pantry: the same zones, parts and levels.
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "part1_level", "value": -12.0})).unwrap();
        let sfz = d.join("Kept.sfz");
        let v = e.execute("mix.multisample_save", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!(v["samples"], 5);
        let other = track(&mut e, MULTISAMPLER);
        let v = e.execute("mix.multisample_load", &json!({"track": other.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!((v["samples"].as_u64(), &v["parts"]), (Some(5), &json!(["pad", "notes", "release", "pedal"])));
        assert_eq!(zones(&e, other), z);
        let back = pantry(&e, other);
        assert_eq!((back.params["part1_level"], back.params["layer_blend"]), (-12.0, 0.0));
        assert_eq!(back.samples.values().collect::<BTreeSet<_>>().len(), 4);
        // It plays: the session's renderer hears the notes.
        let made = e
            .execute("track.capture_multisample", &json!({"track": other.0, "low": 46, "high": 46, "layers": 1, "hold": 0.05, "tail": 0.1}))
            .unwrap();
        assert_eq!(
            (made["samples"].as_u64(), made["silent"].as_u64(), made["release"].as_u64(), made["pedal"].as_u64()),
            (Some(1), Some(0), Some(0), Some(0))
        );
        // Captured with its releases and its pedal: what sounds when a key comes up (a soft and
        // a hard one for each note) and when the pedal goes down (this one is silent coming up).
        for (param, value) in [("part1_level", PART_OFF_DB), ("release", 5.0)] {
            e.execute("mix.instrument_param", &json!({"track": other.0, "param": param, "value": value})).unwrap();
        }
        let params = json!({"track": other.0, "low": 44, "high": 48, "step": 4, "layers": 2, "hold": 0.05, "tail": 0.3, "release": true, "pedal": true, "name": "Again"});
        let made = e.execute("track.capture_multisample", &params).unwrap();
        assert_eq!(
            (made["samples"].as_u64(), made["release"].as_u64(), made["pedal"].as_u64(), made["silent"].as_u64()),
            (Some(4), Some(4), Some(1), Some(0))
        );
        let z = zones(&e, TrackId(made["track"].as_u64().unwrap()));
        assert_eq!(
            z.iter().map(|z| (z.part, z.part_name.as_str(), z.trigger, z.lokey, z.hikey, z.root, z.lovel, z.hivel, z.ref_vel)).collect::<Vec<_>>(),
            [
                (1, "Release", ON_KEY_UP, 0, 46, 44, 1, 64, 64),
                (1, "Release", ON_KEY_UP, 0, 46, 44, 65, 127, 127),
                (1, "Release", ON_KEY_UP, 47, 127, 48, 1, 64, 64),
                (1, "Release", ON_KEY_UP, 47, 127, 48, 65, 127, 127),
                (2, "Pedal", ON_PEDAL_DOWN, 0, 127, 60, 1, 127, 127),
            ]
        );
        assert!(!z[4].keytrack && z[4].veltrack == 0.0);
        // A job says when it is on the pedal.
        let mut job = CaptureJob::new(&e, &params).unwrap();
        assert_eq!(job.counts(), (0, 3));
        while job.counts().0 < 2 {
            job.step(|| false).unwrap();
        }
        assert_eq!(job.note().as_deref(), Some("Pedal"));
    }

    #[test]
    fn decent_sampler_presets_and_libraries_load() {
        let d = dir("decent");
        std::fs::create_dir_all(d.join("Samples")).unwrap();
        let names = ["c_soft.wav", "e_soft.wav", "c_hard_1.wav", "c_hard_2.wav", "c_rel.wav"];
        for f in names {
            tone_wav(&d.join("Samples").join(f), 44_100);
        }
        let preset = d.join("Upright.dspreset");
        std::fs::write(&preset, DSPRESET).unwrap();
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        let keys = |e: &Engine| e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap().samples.keys().cloned().collect::<Vec<_>>();
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": preset.to_string_lossy()})).unwrap();
        assert_eq!(
            (v["name"].as_str(), v["samples"].as_u64(), v["notes"].as_u64(), v["layers"].as_u64()),
            (Some("Upright"), Some(5), Some(2), Some(2))
        );
        let z = zones(&e, t);
        assert_eq!(
            z.iter().map(|z| (z.root, z.lokey, z.hikey, z.hivel, z.seq, z.trigger)).collect::<Vec<_>>(),
            [
                (48, 46, 50, 60, 0, ON_KEY_DOWN),
                (52, 52, 52, 60, 0, ON_KEY_DOWN),
                (48, 0, 127, 127, 1, ON_KEY_DOWN),
                (48, 0, 127, 127, 2, ON_KEY_DOWN),
                (48, 0, 127, 127, 0, ON_KEY_UP)
            ]
        );
        // The same instrument packed as a library, with a second preset beside it.
        let mut w = crate::dawproject::archive::ArchiveWriter::new(std::io::Cursor::new(Vec::new()));
        w.add("Upright/Upright Close.dspreset", DSPRESET.as_bytes()).unwrap();
        w.add("Upright/Upright Far.dspreset", DSPRESET.replace("c_soft", "c_hard_1").as_bytes()).unwrap();
        for f in names {
            w.add(&format!("Upright/Samples/{f}"), &std::fs::read(d.join("Samples").join(f)).unwrap()).unwrap();
        }
        let lib = d.join("Upright.dslibrary");
        std::fs::write(&lib, w.finish().unwrap().into_inner()).unwrap();
        e.execute("mix.multisample_clear", &json!({"track": t.0})).unwrap();
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": lib.to_string_lossy()})).unwrap();
        assert_eq!(
            (v["name"].as_str(), v["samples"].as_u64(), &v["presets"]),
            (Some("Upright Close"), Some(5), &json!(["Upright Close", "Upright Far"]))
        );
        assert_eq!(keys(&e).len(), 5);
        // Its samples have no files of their own: they are the session's to save.
        let s = e.session();
        let ins = s.track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        assert!(ins.samples.values().all(|src| s.source(*src).is_some_and(|x| x.unsaved && x.path.starts_with("Audio Files/"))));
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": lib.to_string_lossy(), "preset": "upright far"})).unwrap();
        assert_eq!(v["name"], "Upright Far");
        assert!(e.execute("mix.multisample_load", &json!({"track": t.0, "path": lib.to_string_lossy(), "preset": "Nope"})).is_err());
        // Not an archive, and an archive with no preset.
        std::fs::write(&lib, b"nope").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": t.0, "path": lib.to_string_lossy()})).is_err());
    }

    /// `FREEDAWM_TEST_SFZ=<an .sfz file> cargo test -p soundcraft-engine --lib -- --ignored a_real_sfz_file_maps`
    /// prints how a library's file maps (its samples need not be there).
    #[test]
    #[ignore = "needs FREEDAWM_TEST_SFZ: an SFZ file of a real library"]
    fn a_real_sfz_file_maps() {
        let path = PathBuf::from(std::env::var("FREEDAWM_TEST_SFZ").unwrap());
        let (mut text, mut budget) = (String::new(), MAX_SFZ_BYTES);
        expand_sfz(&path, path.parent().unwrap(), 0, &mut Vec::new(), &mut text, &mut budget).unwrap();
        let regions = parse_sfz(&text);
        let notes: BTreeSet<u8> = regions.iter().map(|r| r.note).collect();
        let velocities: BTreeSet<u8> = regions.iter().map(|r| r.velocity).collect();
        println!("{} regions, {} notes {:?}, {} velocities {:?}", regions.len(), notes.len(), notes, velocities.len(), velocities);
        for r in regions.iter().take(3).chain(regions.iter().rev().take(2)) {
            println!("  {r:?}");
        }
        // The keys no key-down zone answers with the pedal up, between the lowest and highest.
        let up: Vec<&Region> = regions.iter().filter(|r| r.zone.trigger == ON_KEY_DOWN && r.zone.pedal != PEDAL_DOWN).collect();
        let (lo, hi) = (up.iter().map(|r| r.zone.lokey).min().unwrap_or(0), up.iter().map(|r| r.zone.hikey).max().unwrap_or(0));
        let gaps: Vec<u8> = (lo..=hi).filter(|k| !up.iter().any(|r| r.zone.lokey <= *k && *k <= r.zone.hikey)).collect();
        let parts: BTreeSet<&str> = regions.iter().map(|r| r.part.as_str()).collect();
        println!("keys {lo}..={hi}, silent keys {gaps:?}, parts {parts:?}");
        assert!(!regions.is_empty());
    }

    /// `FREEDAWM_TEST_DSPRESET=<a .dspreset file>`: the same for a Decent Sampler preset.
    #[test]
    #[ignore = "needs FREEDAWM_TEST_DSPRESET: a Decent Sampler preset of a real library"]
    fn a_real_decent_sampler_preset_maps() {
        let path = PathBuf::from(std::env::var("FREEDAWM_TEST_DSPRESET").unwrap());
        let (regions, levels) = parse_dspreset(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut parts: BTreeMap<&str, Vec<&Region>> = BTreeMap::new();
        for r in &regions {
            parts.entry(r.part.as_str()).or_default().push(r);
        }
        println!("{} samples in {} parts", regions.len(), parts.len());
        for (name, of) in &parts {
            let notes: BTreeSet<u8> = of.iter().map(|r| r.note).collect();
            let ranges: BTreeSet<(u8, u8)> = of.iter().map(|r| (r.zone.lovel, r.zone.hivel)).collect();
            let triggers: BTreeSet<u8> = of.iter().map(|r| r.zone.trigger).collect();
            let loops = of.iter().filter(|r| r.zone.loop_end > 0).count();
            println!(
                "  {name}: {} samples, {} notes, velocities {ranges:?}, triggers {triggers:?}, {loops} looped, level {:?} dB",
                of.len(),
                notes.len(),
                levels.get(*name)
            );
        }
        if let Some(r) = regions.first() {
            println!("  first: {} {:?}", r.sample, r.zone);
        }
        assert!(!regions.is_empty());
    }

    #[test]
    fn layers_and_takes_past_the_limits_are_skipped() {
        let d = dir("limits");
        tone_wav(&d.join("a.wav"), 48_000);
        let mut text = String::new();
        for v in 1..=18 {
            text.push_str(&format!("<region> sample=a.wav key=60 hivel={v}\n"));
        }
        for _ in 0..6 {
            text.push_str("<region> sample=a.wav key=62\n");
        }
        let sfz = d.join("many.sfz");
        std::fs::write(&sfz, text).unwrap();
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        // A library's regions are zones of their own, whatever their number.
        assert_eq!((v["samples"].as_u64(), v["layers"].as_u64(), v["skipped"].as_u64()), (Some(24), Some(18), Some(0)));
    }
}

#[cfg(test)]
#[path = "multisample_pitch_tests.rs"]
mod pitch_tests;
