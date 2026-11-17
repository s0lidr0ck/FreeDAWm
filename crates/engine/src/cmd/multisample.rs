//! Pantry, the multisampler: instruments recorded note by note on instrument tracks.
//!
//! `track.capture_multisample` makes one from any instrument track: it plays the track's
//! instrument a note at a time, at several velocities, through the track's module chain
//! (offline, as Freeze renders it), cuts each note where it has died away, and puts the notes in
//! a new track's Pantry.
//! `mix.multisample_load` reads an SFZ file's key-down regions into the track's Multisampler.
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
use soundcraft_dsp::plugins::multisampler::{LAYERS, NOTES, ROBINS, parse_sample_key, sample_key};
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
            "{track?, low?: 21, high?: 108 (MIDI notes), step?: 3 (semitones between captured notes, 1-12), layers?: 12 (evenly spaced velocities up to 127, 1-16) or velocities?: [1-127…], robins?: 1 (strikes of each, 1-4), hold?: 8 (seconds the lowest note is held), hold_high?: 3 (the highest; the ones between shorten evenly), tail?: 4 (seconds left to ring after each), silence?: -72 (dBFS a note has died away at), mono?: false, name?} -> {track, samples, notes, layers, seconds, silent, ringing}: plays the track's instrument a note at a time through its modules (pre-fader, offline) and makes a new instrument track whose Pantry plays the captured notes; `silent` notes made no sound and are left out, `ringing` ones were still sounding when the next began",
            has_selection,
            capture
        ),
        cmd!(
            "mix.multisample_load",
            "Load Multisample",
            [],
            None,
            "{track?, path: an .sfz file} -> {track, name, samples, notes, layers, skipped}: the track's Multisampler plays the file's samples in place of its own (the mapping only: recorded note, velocity layer and round-robin of each key-down region; at most 16 layers a note and 4 round-robins a layer, the rest skipped)",
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
    /// Each sample's key, source name and audio.
    samples: Vec<(String, String, AudioBuffer)>,
    silent: usize,
    ringing: usize,
}

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
    /// The note being rendered (an index into `notes`), its render and its strikes' starts.
    at: usize,
    render: Option<(soundcraft_mix::TrackRender, Vec<(u8, usize, usize)>)>,
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
            scratch: (*s).clone(),
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

    /// Notes rendered so far, and how many there are.
    pub fn counts(&self) -> (usize, usize) {
        (self.at.min(self.notes.len()), self.notes.len())
    }

    /// The note being rendered, by name (`None` once all are done).
    pub fn note(&self) -> Option<String> {
        self.notes.get(self.at).map(|n| note_name(*n))
    }

    /// How far along, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let within = self.render.as_ref().map_or(0.0, |(r, _)| r.progress());
        ((self.at as f32 + within) / self.notes.len().max(1) as f32).clamp(0.0, 1.0)
    }

    /// Starts the render of note `self.at`: half a second of silence, then its strikes.
    fn begin(&mut self) -> Result<()> {
        let Some(&note) = self.notes.get(self.at) else { return Ok(()) };
        let sr = self.scratch.sample_rate;
        let along = if self.high > self.low { (i64::from(note) - self.low) as f64 / (self.high - self.low) as f64 } else { 0.0 };
        let held = self.hold + (self.hold_high - self.hold) * along;
        let (hold_len, slot) = (sr.samples(held).max(1), sr.samples(held + self.tail).max(1));
        let mut seq = Sequence::default();
        let mut starts = Vec::with_capacity(self.velocities.len() * self.robins);
        let mut at = sr.samples(CAPTURE_LEAD_SECS);
        for &velocity in &self.velocities {
            for robin in 1..=self.robins {
                let on = self.scratch.tempo.samples_to_ticks(at, sr);
                let off = self.scratch.tempo.samples_to_ticks(at.saturating_add(hold_len), sr);
                seq.notes.push(Note { pitch: note, velocity, release_velocity: 64, channel: 0, start: on, length: off.saturating_sub(on).max(1) });
                let sounds = self.scratch.tempo.tick_to_samples(on, sr).saturating_add(self.lag);
                starts.push((velocity, robin, usize::try_from(sounds.max(0)).unwrap_or(0)));
                at = at.saturating_add(slot);
            }
        }
        seq.sort();
        let clip = Clip::midi(self.scratch.new_clip_id(), "Capture", 0, at, seq);
        match self.scratch.track_mut(self.track).and_then(|tr| tr.playlist_mut()) {
            Some(pl) => pl.clips = vec![clip],
            None => return Err(bad(self.id, "the track has no playlist to play from")),
        }
        self.render = Some((soundcraft_mix::TrackRender::frozen(&self.scratch, self.track, at, None, None), starts));
        Ok(())
    }

    /// Cuts the strikes of the note just rendered out of `audio`.
    fn cut(&mut self, audio: &[Vec<f32>], starts: &[(u8, usize, usize)]) -> Result<()> {
        let Some(&note) = self.notes.get(self.at) else { return Ok(()) };
        let sr = self.scratch.sample_rate;
        let frames = |secs: f64| usize::try_from(sr.samples(secs).max(0)).unwrap_or(0);
        let rendered = audio.iter().map(Vec::len).min().unwrap_or(0);
        // What counts as died away: the level asked for, or just above the chain's own noise.
        let quiet = self.quiet_at.max(peak_in(audio, 0, frames(CAPTURE_LEAD_SECS)) * 1.5);
        let block = frames(0.005).max(1);
        for (i, &(velocity, robin, start)) in starts.iter().enumerate() {
            let last_strike = i + 1 == starts.len();
            let limit = starts.get(i + 1).map_or(rendered, |next| next.2.min(rendered));
            if start >= limit || peak_in(audio, start, limit) <= quiet {
                self.out.silent += 1;
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
            if loud >= limit && !last_strike {
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
            let file =
                format!("{} {} v{velocity}{}", self.name, note_name(note), if self.robins > 1 { format!(" rr{robin}") } else { String::new() });
            self.out.samples.push((sample_key(note, velocity, robin), file, AudioBuffer { sample_rate: sr.hz(), channels }));
        }
        Ok(())
    }

    /// Renders while `more` says so (asked after every block). `Ok(true)` once every note is
    /// done; an error ends the job.
    pub fn step(&mut self, mut more: impl FnMut() -> bool) -> Result<bool> {
        loop {
            if self.at >= self.notes.len() {
                return Ok(true);
            }
            if self.render.is_none() {
                self.begin()?;
            }
            let done = self.render.as_mut().is_none_or(|(r, _)| r.step());
            if done && let Some((render, starts)) = self.render.take() {
                let (audio, _) = render.finish();
                self.cut(&audio, &starts)?;
                self.at += 1;
            }
            if !more() {
                return Ok(self.at >= self.notes.len());
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
    let seconds = sr.seconds(Samples::try_from(made.samples.iter().map(|(_, _, b)| b.frames()).sum::<usize>()).unwrap_or(Samples::MAX));
    let notes: BTreeSet<&str> = made.samples.iter().filter_map(|(k, _, _)| k.split('.').next()).collect();
    let layers: BTreeSet<&str> = made.samples.iter().filter_map(|(k, _, _)| k.split('.').nth(1)).collect();
    let (count, notes, layers) = (made.samples.len(), notes.len(), layers.len());
    let mut ins = pantry_insert();
    let session = e.session_mut();
    for (key, file, buf) in made.samples {
        ins.samples.insert(key, crate::io::add_source(session, &file, buf, None, FileFormat::Wav));
    }
    let track = session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
    if let Some(tr) = session.track_mut(track) {
        tr.instrument = Some(ins);
        tr.mixer.input = Route::Midi(soundcraft_model::MidiIn::all());
    }
    session.edit.selected_tracks = vec![track];
    Ok(
        json!({"track": track, "samples": count, "notes": notes, "layers": layers, "seconds": seconds, "silent": made.silent, "ringing": made.ringing}),
    )
}

/// A key-down region of an SFZ file.
#[derive(Debug, Clone, PartialEq)]
struct Region {
    /// The sample's path as the file gives it (after `default_path`), with `/` separators.
    sample: String,
    note: u8,
    velocity: u8,
    /// Round-robin take, from 1.
    robin: usize,
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
    let finish = |region: &mut Opcodes, control: &Opcodes, global: &Opcodes, master: &Opcodes, group: &Opcodes, out: &mut Vec<Region>| {
        let get = |k: &str| region.get(k).or_else(|| group.get(k)).or_else(|| master.get(k)).or_else(|| global.get(k)).map(String::as_str);
        let sample = get("sample").map(|s| s.trim().replace('\\', "/")).filter(|s| !s.is_empty() && !s.starts_with('*'));
        let key_down = get("trigger").is_none_or(|t| t.trim().eq_ignore_ascii_case("attack"));
        if let Some(sample) = sample.filter(|_| key_down) {
            let dir = control.get("default_path").map(|d| d.trim().replace('\\', "/")).unwrap_or_default();
            let note = get("pitch_keycenter").or_else(|| get("key")).and_then(sfz_note).unwrap_or(60);
            // A file Pantry saved says what each sample was played at; else the top of its range.
            let velocity =
                get("pantry_velocity").or_else(|| get("hivel")).and_then(|v| v.trim().parse::<i64>().ok()).map_or(127, |v| v.clamp(1, 127) as u8);
            let robin = get("seq_position").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).max(1);
            out.push(Region { sample: format!("{dir}{sample}"), note, velocity, robin });
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

/// A sample to put in a Pantry: the note it was recorded at, its velocity, its take (from 1) and
/// its file.
struct Wanted {
    note: u8,
    velocity: u8,
    robin: usize,
    file: PathBuf,
}

/// Decodes `wanted` into track `t`'s Pantry (in place of what it holds with `replace`, else
/// beside it): a note keeps 16 velocities and a layer 4 takes, and a second sample for a taken
/// place is the layer's next take; what does not fit is skipped. Returns the samples placed, the
/// notes and most layers the Pantry then holds, and the samples skipped. A missing or broken
/// file fails the command, which leaves the session as it was.
fn attach(e: &mut Engine, id: &str, t: TrackId, wanted: &[Wanted], replace: bool, what: &str) -> Result<(usize, usize, usize, usize)> {
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
    let rate = e.session().sample_rate.hz();
    let mut total = 0usize;
    for (key, w) in &placed {
        let name = w.file.file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string();
        let ext = w.file.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let bytes = crate::io::read_file(&w.file)?;
        let (format, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
        if buf.frames() == 0 || buf.num_channels() == 0 {
            return Err(EngineError::Io(format!("{name}: no audio")));
        }
        let buf = crate::io::at_rate(buf, rate, MAX_SAMPLE_VALUES)
            .ok_or_else(|| EngineError::Io(format!("{name}: too long for one note (two minutes at most)")))?;
        total = total.saturating_add(buf.frames().saturating_mul(buf.num_channels()));
        if total > crate::io::MAX_IMPORT_SAMPLES {
            return Err(EngineError::Io(format!("{what}: too much audio to hold in memory")));
        }
        let src = crate::io::add_source(e.session_mut(), &name, buf, Some(&w.file.to_string_lossy()), format.format);
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
    let bytes = crate::io::read_file(&path)?;
    if bytes.len() > MAX_SFZ_BYTES {
        return Err(EngineError::Io(format!("{}: too large for an SFZ file", path.display())));
    }
    let text = String::from_utf8_lossy(&bytes);
    let regions = parse_sfz(&text);
    if regions.is_empty() {
        return Err(EngineError::Io(format!("{}: no samples to play (an SFZ file's key-down regions)", path.display())));
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let wanted: Vec<Wanted> =
        regions.iter().map(|r| Wanted { note: r.note, velocity: r.velocity, robin: r.robin, file: dir.join(&r.sample) }).collect();
    let (samples, notes, layers, skipped) = attach(e, id, t, &wanted, true, &path.display().to_string())?;
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
    let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("Multisample");
    Ok(json!({"track": t.0, "name": name, "samples": samples, "notes": notes, "layers": layers, "skipped": skipped}))
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
            Some((note, velocity, robin)) => wanted.push(Wanted { note, velocity, robin, file }),
            None => unnamed += 1,
        }
    }
    if wanted.is_empty() {
        return Err(bad(id, "no sample there says its note and velocity in its name (as `Piano C3 v95.wav` or `060_C3_v095_rr1.wav`)"));
    }
    let (samples, notes, layers, skipped) = attach(e, id, t, &wanted, bool_or(p, "replace", false), "the samples")?;
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
    if held.is_empty() {
        return Err(bad(id, "the Multisampler holds no samples to save"));
    }
    let notes: Vec<u8> = held.iter().map(|h| h.0).collect::<BTreeSet<_>>().into_iter().collect();
    let folder = path.with_extension("");
    crate::file_access::check_write(&path).map_err(EngineError::Io)?;
    if !cfg!(target_arch = "wasm32") {
        crate::io::make_dir(&folder)?;
    }
    let params: BTreeMap<&str, f32> = ins.params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let release = ins.params.get("release").copied().unwrap_or(250.0) / 1000.0;
    let mut text = format!(
        "// {stem}: a Pantry instrument saved by FreeDAWm. Each sample is as it was captured, so\n// velocity does not scale the layers again.\n{PARAMS_LINE}{}\n<control> default_path={stem}/\n<global> amp_veltrack=0 ampeg_release={release:.3}\n",
        serde_json::to_string(&params).unwrap_or_else(|_| "{}".into())
    );
    let options =
        soundcraft_audio_io::EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Float32, ..Default::default() };
    let mut bytes = 0usize;
    for &(note, velocity, robin, src) in &held {
        let audio = s.pool.get(src).ok_or_else(|| EngineError::Io(format!("{}: a sample's audio is missing", note_name(note))))?;
        let file = format!("{note:03}_{}_v{velocity:03}_rr{robin}.wav", note_name(note).replace('#', "s"));
        let wav = soundcraft_audio_io::encode(&audio.buffer(), &options).map_err(|err| EngineError::Io(format!("{file}: {err}")))?;
        bytes = bytes.saturating_add(wav.len());
        crate::io::write_file(&folder.join(&file).to_string_lossy(), &wav)?;
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
    crate::io::write_file(&path.to_string_lossy(), text.as_bytes())?;
    Ok(json!({"path": path.to_string_lossy(), "name": stem, "samples": held.len(), "bytes": bytes.saturating_add(text.len())}))
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

    fn region(sample: &str, note: u8, velocity: u8, robin: usize) -> Region {
        Region { sample: sample.into(), note, velocity, robin }
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
            parse_sfz(text),
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
        // The second sample for one place is that layer's next take.
        assert_eq!(ins.samples.keys().map(String::as_str).collect::<Vec<_>>(), ["n48.v127.r1", "n48.v60.r1", "n60.v127.r1", "n60.v127.r2"]);
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
        assert_eq!((v["samples"].as_u64(), v["layers"].as_u64(), v["skipped"].as_u64()), (Some(20), Some(16), Some(4)));
    }
}
