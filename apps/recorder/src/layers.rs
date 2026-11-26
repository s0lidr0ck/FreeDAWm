//! Layers: an idea played back on a loop while another part is recorded over it, like a simple
//! looper, every layer its own file.
//!
//! - **Names.** An idea `Chorus idea` is `Chorus idea.wav` / `.mid` (layer 1); its other layers
//!   are `Chorus idea (layer 2).wav` / `.mid`, `(layer 3)`… in the same folder, so they sort
//!   together and pair up ([`layer_stem`], [`split_layer`]). Files named that way by hand are
//!   layers too.
//! - **Alignment.** Every layer file starts at the idea's start (time zero) and is exactly the
//!   loop's length, so dropping all of an idea's files at the same position in any program lines
//!   them up. A layer is recorded as one take on the input while the idea loops (the take's frames
//!   are captured in the very output callbacks that play the loop, so they share one frame
//!   counter); at Stop it is cut into passes, each moved earlier by the record offset (the
//!   round trip the devices report, or the one **Line up** measured, plus the manual nudge) so
//!   it lines up with what was heard.
//! - **Passes.** Each time round the loop becomes its own layer (looper style): every file is
//!   the loop's length and loops cleanly. A last partial pass is padded with silence; a pass in
//!   which nothing was played (no sound above [`SILENT`] and no note) is not kept.
//! - **The sidecar** `<idea>.fdidea` (JSON, beside the files, so it moves with them) remembers
//!   each layer's mute and level and whether the idea loops ([`Sidecar`]). The WAVs and MIDI files
//!   never need it. While a layer records it also notes the take in progress ([`Pending`]): after
//!   a crash the take's file (written as it went, playable) is cut into its layers the next time
//!   the Recorder looks at the folder ([`recover`]). Earlier layers are never rewritten.
//!
//! Playback reads the idea's layers into memory (ideas are short: [`MAX_SECONDS`] at most) and
//! plays their mix through the player's synced preview, which reads it where the transport is, so
//! the loop and the take stay in step sample for sample.

use crate::files::TakeFiles;
use crate::rig::midi_file;
use serde::{Deserialize, Serialize};
use soundcraft_engine::midi_record::{MidiTake, RecordedEvent};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The sidecar's extension.
pub const SIDECAR_EXT: &str = "fdidea";

/// The longest idea that can be layered or played as a mix (its layers are read into memory:
/// about 23 MB a minute in stereo).
pub const MAX_SECONDS: f64 = 600.0;

/// A pass whose loudest sample stays under this (−45 dBFS) and which holds no note is "nothing
/// played" and is not kept.
pub const SILENT: f32 = 0.0056;

/// The file stem of layer `n` of idea `idea` (`n` ≤ 1: the idea's own).
pub fn layer_stem(idea: &str, n: u32) -> String {
    if n <= 1 { idea.to_string() } else { format!("{idea} (layer {n})") }
}

/// The idea a file stem belongs to and its layer number (`Chorus (layer 3)` → `("Chorus", 3)`;
/// any other stem is layer 1 of itself).
pub fn split_layer(stem: &str) -> (String, u32) {
    if let Some(head) = stem.strip_suffix(')')
        && let Some(i) = head.rfind(" (layer ")
        && let Some(n) = head.get(i + 8..).and_then(|n| n.parse::<u32>().ok()).filter(|n| *n >= 2)
        && let Some(idea) = head.get(..i).filter(|s| !s.trim().is_empty())
    {
        return (idea.to_string(), n);
    }
    (stem.to_string(), 1)
}

/// How a layer plays in the idea's mix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerMix {
    pub muted: bool,
    /// Linear gain, 0..=2 (1: as recorded).
    pub level: f32,
}

impl Default for LayerMix {
    fn default() -> Self {
        LayerMix { muted: false, level: 1.0 }
    }
}

/// A layer being recorded (in the sidecar while it records; see [`recover`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    /// The layer number its file has while it records (`<idea> (layer n).wav`).
    pub layer: u32,
    /// The loop's length and the rate, in frames.
    pub loop_frames: i64,
    pub rate: u32,
    /// The record offset when it started (frames).
    pub offset: i64,
}

/// `<idea>.fdidea`: how the idea's layers play together. Missing or damaged: every layer plays
/// at its own level, once (an idea gets looped when a layer is added).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidecar {
    pub version: u32,
    /// Play the idea round and round (else once).
    pub looped: bool,
    /// By layer number; a layer not here plays at its own level.
    pub layers: BTreeMap<u32, LayerMix>,
    pub pending: Option<Pending>,
    /// The tempo and beats a bar it was recorded to, when the click was on.
    pub tempo: Option<crate::rig::Click>,
}

impl Default for Sidecar {
    fn default() -> Self {
        Sidecar { version: 1, looped: false, layers: BTreeMap::new(), pending: None, tempo: None }
    }
}

impl Sidecar {
    pub fn load(path: &Path) -> Sidecar {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// The idea's sidecar (the defaults without one).
    pub fn of(take: &TakeFiles) -> Sidecar {
        take.sidecar.as_deref().or(take.sidecar_path().as_deref()).map(Sidecar::load).unwrap_or_default()
    }

    /// Writes `path` through a temporary file (a crash never leaves half of it).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension(format!("{SIDECAR_EXT}.tmp"));
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn mix(&self, n: u32) -> LayerMix {
        self.layers.get(&n).copied().unwrap_or_default()
    }
}

/// An idea's layers mixed for playback: stereo at `rate`, `frames` long (the loop).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mix {
    pub channels: Vec<Vec<f32>>,
    pub rate: u32,
    pub frames: usize,
}

/// A MIDI file's length in seconds (its last note's end, at its own first tempo).
pub fn midi_seconds(path: &Path) -> Option<f64> {
    let smf = soundcraft_midi::read_smf(&std::fs::read(path).ok()?).ok()?;
    let bpm = smf.tempos.first().map_or(120.0, |t| t.1).max(1.0);
    let end = smf.tracks.iter().flat_map(|t| t.sequence.notes.iter()).map(|n| n.start.saturating_add(n.length)).max()?;
    Some(end as f64 / f64::from(soundcraft_midi::PPQ) * 60.0 / bpm)
}

/// The idea's loop length at `rate` (frames): its longest layer (a WAV's length, a MIDI file's
/// last note).
pub fn loop_frames(take: &TakeFiles, rate: u32) -> Option<usize> {
    let secs = take
        .all_layers()
        .iter()
        .filter_map(|l| l.wav.as_deref().and_then(crate::files::wav_seconds).or_else(|| l.mid.as_deref().and_then(midi_seconds)))
        .reduce(f64::max)?;
    let f = (secs * f64::from(rate)).round();
    (f >= 1.0).then_some(f as usize)
}

/// Reads a WAV as planar audio at `rate` (linear interpolation when its rate differs).
fn read_at(path: &Path, rate: u32) -> Result<Vec<Vec<f32>>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (_, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|e| format!("{}: {e}", path.display()))?;
    if buf.sample_rate == rate || buf.sample_rate == 0 {
        return Ok(buf.channels);
    }
    let step = f64::from(buf.sample_rate) / f64::from(rate.max(1));
    Ok(buf
        .channels
        .iter()
        .map(|c| {
            let n = (c.len() as f64 / step).floor() as usize;
            (0..n)
                .map(|i| {
                    let x = i as f64 * step;
                    let k = x as usize;
                    let f = (x - k as f64) as f32;
                    let a = c.get(k).copied().unwrap_or(0.0);
                    let b = c.get(k + 1).copied().unwrap_or(a);
                    a + (b - a) * f
                })
                .collect()
        })
        .collect())
}

/// The idea's unmuted layers' WAVs mixed (each at its level) at `rate`, the loop's length. A
/// mono layer plays on both sides.
pub fn mix(take: &TakeFiles, side: &Sidecar, rate: u32) -> Result<Mix, String> {
    let frames = loop_frames(take, rate).ok_or("This idea has nothing in it to play")?;
    if frames as f64 > MAX_SECONDS * f64::from(rate.max(1)) {
        return Err(format!("Layers work on ideas up to {} minutes long", (MAX_SECONDS / 60.0) as u32));
    }
    let mut out = vec![vec![0.0f32; frames]; 2];
    for l in take.all_layers() {
        let m = side.mix(l.n);
        let Some(wav) = l.wav.as_deref().filter(|_| !m.muted && m.level > 0.0) else { continue };
        let ch = read_at(wav, rate)?;
        for (c, o) in out.iter_mut().enumerate() {
            let Some(src) = ch.get(c).or_else(|| ch.first()) else { continue };
            for (o, x) in o.iter_mut().zip(src) {
                *o += x * m.level.clamp(0.0, 2.0);
            }
        }
    }
    Ok(Mix { channels: out, rate, frames })
}

/// A layer take as it stopped: what [`finish`] cuts into layers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerRaw {
    /// The idea and the layer number its take was recorded as.
    pub idea: String,
    pub first: u32,
    pub folder: PathBuf,
    /// The take's WAV (`None`: MIDI only).
    pub wav: Option<PathBuf>,
    /// The timeline position of the take's first frame (in `0..loop_frames`).
    pub start: i64,
    /// The record offset (frames): take frame `k` was heard at `start + k − offset`.
    pub offset: i64,
    pub loop_frames: i64,
    pub rate: u32,
    /// The MIDI played (`at` within the loop, `pass` the time round), and how much earlier it
    /// was played than heard.
    pub events: Vec<RecordedEvent>,
    pub midi_latency: i64,
    /// The click it was recorded with (its MIDI is written at that tempo).
    pub click: Option<crate::rig::Click>,
}

/// What a layer take became.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerResult {
    /// The layer numbers made, in order.
    pub made: Vec<u32>,
    /// Times round with nothing played (not kept).
    pub empty: usize,
    pub problem: Option<String>,
}

/// `frames` frames of `ch` from frame `k0` on (silence outside it).
fn slice(ch: &[Vec<f32>], k0: i64, frames: usize) -> Vec<Vec<f32>> {
    ch.iter()
        .map(|c| {
            (0..frames)
                .map(|j| {
                    let k = k0.saturating_add(j as i64);
                    usize::try_from(k).ok().and_then(|k| c.get(k)).copied().unwrap_or(0.0)
                })
                .collect()
        })
        .collect()
}

/// Cuts a layer take into its layers (see the module docs): each time round the loop becomes
/// layer `first`, `first + 1`… (empty passes left out, the numbers kept consecutive), each a WAV
/// of exactly the loop's length from the idea's start and, when notes were played, a MIDI file.
/// The take's own file is replaced by them; nothing else is touched.
pub fn finish(raw: &LayerRaw) -> LayerResult {
    let mut res = LayerResult::default();
    let n = raw.loop_frames.max(1);
    let frames = usize::try_from(n).unwrap_or(0);
    // The take's audio (an empty or unreadable file: none).
    let audio: Option<Vec<Vec<f32>>> = raw
        .wav
        .as_deref()
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| soundcraft_audio_io::decode(&b, Some("wav")).ok())
        .map(|(_, b)| b.channels)
        .filter(|c| c.first().is_some_and(|c| !c.is_empty()));
    let total = audio.as_ref().and_then(|c| c.first()).map_or(0, Vec::len) as i64;
    // Heard at `start + k − offset`: time round `p` holds take frames from `p·n − start + offset`.
    let first_k = |p: i64| p.saturating_mul(n).saturating_sub(raw.start).saturating_add(raw.offset);
    let mut passes = if total > 0 { (total.saturating_add(raw.start).saturating_sub(raw.offset)).max(0).saturating_add(n - 1) / n } else { 0 };
    // The MIDI by time round, `at` from the idea's start.
    let mut midi: BTreeMap<i64, Vec<RecordedEvent>> = BTreeMap::new();
    for e in &raw.events {
        let u = i64::from(e.pass).saturating_mul(n).saturating_add(e.at).saturating_sub(raw.midi_latency).max(0);
        let p = u / n;
        midi.entry(p).or_default().push(RecordedEvent { at: u - p * n, pass: 0, ..e.clone() });
    }
    passes = passes.max(midi.keys().next_back().map_or(0, |p| p + 1));
    let mut writes: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut next = raw.first;
    for p in 0..passes {
        let sound = audio.as_ref().map(|ch| slice(ch, first_k(p), frames));
        let loud = sound.as_ref().is_some_and(|s| s.iter().flatten().any(|x| x.is_finite() && x.abs() >= SILENT));
        let events = midi.get(&p).cloned().unwrap_or_default();
        let take = MidiTake { start: 0, end: n, looped: None, punch: None, latency: 0 };
        let notes =
            if events.iter().any(|e| e.msg[0] & 0xF0 == 0x90 && e.msg[2] > 0) { midi_file(raw.rate, &take, &events, raw.click) } else { None };
        if !loud && notes.is_none() {
            res.empty += 1;
            continue;
        }
        let stem = layer_stem(&raw.idea, next);
        if let Some(s) = sound.filter(|_| loud) {
            let buf = soundcraft_audio_io::AudioBuffer { sample_rate: raw.rate, channels: s };
            let opts = soundcraft_audio_io::EncodeOptions::default();
            match soundcraft_audio_io::encode(&buf, &opts) {
                Ok(b) => writes.push((raw.folder.join(format!("{stem}.wav")), b)),
                Err(e) => res.problem = Some(format!("A layer could not be written: {e}")),
            }
        }
        if let Some((bytes, _)) = notes {
            writes.push((raw.folder.join(format!("{stem}.mid")), bytes));
        }
        res.made.push(next);
        next = next.saturating_add(1);
    }
    // Written beside, then the take's own file goes and they take their names.
    let mut parts = Vec::new();
    for (to, bytes) in writes {
        let part = to.with_extension(format!("{}.part", to.extension().and_then(|x| x.to_str()).unwrap_or("wav")));
        match std::fs::write(&part, bytes) {
            Ok(()) => parts.push((part, to)),
            Err(e) => res.problem = Some(format!("A layer could not be written: {e}")),
        }
    }
    if res.problem.is_some() {
        // The take's file stays as it is (it plays); the parts go.
        for (part, _) in parts {
            let _ = std::fs::remove_file(part);
        }
        res.made.clear();
        return res;
    }
    if let Some(w) = &raw.wav {
        let _ = std::fs::remove_file(w);
    }
    for (part, to) in parts {
        if let Err(e) = std::fs::rename(&part, &to) {
            res.problem = Some(format!("A layer could not be written: {e}"));
        }
    }
    res
}

/// Makes a new idea's WAV a whole number of bars long (`bar` frames each; at least one): the
/// nearest, filled out with silence or cut, the last 10 ms faded out when cut. Returns the bars.
/// Written beside and renamed over, so a crash leaves the old file or the new one.
pub fn round_to_bars(wav: &Path, bar: f64) -> Result<u32, String> {
    let bytes = std::fs::read(wav).map_err(|e| e.to_string())?;
    let (_, mut buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|e| e.to_string())?;
    let frames = buf.channels.first().map_or(0, Vec::len);
    if !(bar.is_finite() && bar >= 1.0) || frames == 0 {
        return Err("nothing to round".into());
    }
    let bars = ((frames as f64 / bar).round() as u32).max(1);
    let target = (f64::from(bars) * bar).round() as usize;
    if target < frames {
        let fade = (f64::from(buf.sample_rate) * 0.01) as usize;
        for c in &mut buf.channels {
            c.truncate(target);
            let n = c.len();
            for (k, x) in c.iter_mut().skip(n.saturating_sub(fade)).enumerate() {
                *x *= 1.0 - (k as f32 + 1.0) / fade.max(1) as f32;
            }
        }
    } else {
        for c in &mut buf.channels {
            c.resize(target, 0.0);
        }
    }
    let out = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).map_err(|e| e.to_string())?;
    let part = wav.with_extension("wav.part");
    std::fs::write(&part, out).map_err(|e| e.to_string())?;
    std::fs::rename(&part, wav).map_err(|e| e.to_string())?;
    Ok(bars)
}

/// Finishes layer takes a crash left behind in `dir` (their sidecars still say one was
/// recording): each is cut into its layers as Stop would have (its MIDI, kept in memory, is
/// lost). `skip`: the idea recording now.
pub fn recover(dir: &Path, skip: Option<&str>) -> Vec<(String, LayerResult)> {
    let mut out = Vec::new();
    for t in crate::files::recent(dir, usize::MAX) {
        if Some(t.stem.as_str()) == skip {
            continue;
        }
        let Some(path) = t.sidecar.clone() else { continue };
        let mut side = Sidecar::load(&path);
        let Some(p) = side.pending.take() else { continue };
        let wav = dir.join(format!("{}.wav", layer_stem(&t.stem, p.layer)));
        let raw = LayerRaw {
            idea: t.stem.clone(),
            first: p.layer,
            folder: dir.to_path_buf(),
            wav: Some(wav),
            start: 0,
            offset: p.offset,
            loop_frames: p.loop_frames,
            rate: p.rate,
            events: Vec::new(),
            midi_latency: 0,
            click: side.tempo,
        };
        let res = finish(&raw);
        if res.problem.is_none() {
            let _ = side.save(&path);
        }
        out.push((t.stem, res));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_names_pair_up() {
        assert_eq!(layer_stem("Chorus idea", 1), "Chorus idea");
        assert_eq!(layer_stem("Chorus idea", 2), "Chorus idea (layer 2)");
        assert_eq!(split_layer("Chorus idea (layer 2)"), ("Chorus idea".to_string(), 2));
        assert_eq!(split_layer("Chorus idea (layer 12)"), ("Chorus idea".to_string(), 12));
        assert_eq!(split_layer("Chorus idea"), ("Chorus idea".to_string(), 1));
        assert_eq!(split_layer("Idea 2026-11-22 14-03-10 (2)"), ("Idea 2026-11-22 14-03-10 (2)".to_string(), 1));
        assert_eq!(split_layer("(layer 2)"), ("(layer 2)".to_string(), 1));
        assert_eq!(split_layer("A (layer 1)"), ("A (layer 1)".to_string(), 1), "layer 1 is the idea's own name");
        assert_eq!(split_layer("A (layer x)"), ("A (layer x)".to_string(), 1));
    }

    #[test]
    fn the_sidecar_defaults_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("freedawm-recorder-sidecar-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("A.fdidea");
        assert_eq!(Sidecar::load(&p), Sidecar::default());
        assert!(!Sidecar::default().looped);
        let mut s = Sidecar::default();
        s.layers.insert(2, LayerMix { muted: true, level: 0.5 });
        s.save(&p).unwrap();
        assert_eq!(Sidecar::load(&p), s);
        std::fs::write(&p, b"{not json").unwrap();
        assert_eq!(Sidecar::load(&p), Sidecar::default(), "damaged: the defaults");
        let _ = std::fs::remove_dir_all(dir);
    }
}
