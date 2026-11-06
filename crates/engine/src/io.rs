//! Import, export, save/load, rendering helpers (bounce, consolidate, commit) and analysis.

use crate::{Engine, EngineError, Result};
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, EncodeOptions, FileFormat};
use soundcraft_model::{BusId, ChannelFormat, Clip, ClipContent, Session, Source, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Largest import we accept (samples per channel × channels), ~4 GiB of f32.
pub(crate) const MAX_IMPORT_SAMPLES: usize = 1 << 30;

/// `buf` converted to `rate`, unless it would then hold more than `limit` samples: a file with a
/// tiny sample rate grows by orders of magnitude.
pub(crate) fn at_rate(mut buf: AudioBuffer, rate: u32, limit: usize) -> Option<AudioBuffer> {
    let from = buf.sample_rate.max(1);
    if buf.frames() as f64 * buf.num_channels() as f64 * f64::from(rate) / f64::from(from) > limit as f64 {
        return None;
    }
    if from != rate {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, from, rate);
        buf.sample_rate = rate;
    }
    Some(buf)
}

// ─── Checked file access ──────────────────────────────────────────────────────────────────────
//
// Every file a command names by path is read and written through the helpers below, so the
// automation roots (`crate::file_access`, `freedawm-cli mcp --automation-read-root` and
// `--automation-write-root`) are checked in one place. A test in `file_access` keeps the rest of
// the engine from calling `std::fs` itself. Audio is decoded into the session's pool the moment it
// enters it (import, open, sampler load), so playback and bounce read nothing from disk and
// need no check of their own.

/// The bytes of the file at `path`, which must be inside the automation read root when there is
/// one. Only a regular file qualifies: a device or a pipe named by a session or a command would
/// never end or never answer. Errors name the file.
pub fn read_file(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(EngineError::Io)?;
    let err = |err: std::io::Error| EngineError::Io(format!("{}: {err}", path.display()));
    if !std::fs::metadata(path).map_err(err)?.is_file() {
        return Err(EngineError::Io(format!("{}: not a regular file", path.display())));
    }
    std::fs::read(path).map_err(err)
}

/// [`read_file`] for a file that must not be larger than `max` bytes (checked before reading).
pub(crate) fn read_file_max(path: impl AsRef<Path>, max: u64) -> Result<Vec<u8>> {
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(EngineError::Io)?;
    let err = |err: std::io::Error| EngineError::Io(format!("{}: {err}", path.display()));
    let len = std::fs::metadata(path).map_err(err)?.len();
    if len > max {
        return Err(EngineError::Io(format!("{}: too large ({len} bytes; at most {max})", path.display())));
    }
    read_file(path)
}

/// [`read_file`] as UTF-8 text.
pub fn read_text(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    String::from_utf8(read_file(path)?).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))
}

/// Is there a file at `path` the automation read root lets us see? A path outside the root is
/// reported as not found, so a command can't probe for files beyond it.
pub fn file_found(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    crate::file_access::check_read(path).is_ok() && path.is_file()
}

/// The regular file at `path` (inside the read root) opened for reading, for readers that stream
/// (a DAWproject archive can be gigabytes). Errors name the file.
pub(crate) fn open_read(path: impl AsRef<Path>) -> Result<std::fs::File> {
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(EngineError::Io)?;
    let err = |err: std::io::Error| EngineError::Io(format!("{}: {err}", path.display()));
    if !std::fs::metadata(path).map_err(err)?.is_file() {
        return Err(EngineError::Io(format!("{}: not a regular file", path.display())));
    }
    std::fs::File::open(path).map_err(err)
}

/// Is there anything at `path` (inside the write root) a write would replace?
pub(crate) fn path_taken(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    crate::file_access::check_write(path).is_ok() && path.exists()
}

/// Writes `file` through `write` streaming into a temporary file beside it, then renames it into
/// place; the temporary file is removed when anything fails. Both paths must be inside the write
/// root.
pub(crate) fn write_streamed<T>(
    file: &Path,
    write: impl FnOnce(std::io::BufWriter<std::fs::File>) -> std::result::Result<(std::io::BufWriter<std::fs::File>, T), String>,
) -> Result<T> {
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    crate::file_access::check_write(file).map_err(EngineError::Io)?;
    crate::file_access::check_write(&tmp).map_err(EngineError::Io)?;
    let written = (|| -> std::result::Result<T, String> {
        let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let (w, done) = write(std::io::BufWriter::new(f))?;
        let f = w.into_inner().map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&tmp, file).map_err(|e| e.to_string())?;
        Ok(done)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(|e| EngineError::Io(format!("{}: {e}", file.display())))
}

/// The file a path stored in the session file `session` refers to, or why it is not opened.
///
/// Every path a session names (audio sources, movies) goes through here before it reaches the
/// file system. A relative path is taken from the session's folder, and only files on this
/// computer or on the network share the session itself is on are opened (`craft_docpath`).
/// Paths the person picks in this session (imports) are not stored paths and skip this.
pub fn stored_path(stored: &str, session: &Path) -> std::result::Result<PathBuf, String> {
    let session = std::path::absolute(session).unwrap_or_else(|_| session.to_path_buf());
    let path = craft_docpath::resolve(stored, Some(&session)).map_err(|why| format!("not opened: {why}"))?;
    // A path that is still relative would be taken from the working directory, which the session
    // does not choose.
    if path.is_relative() {
        return Err("not opened: the path is not absolute".into());
    }
    Ok(path)
}

/// Create `dir` and its parents, inside the automation write root when there is one.
pub(crate) fn make_dir(dir: impl AsRef<Path>) -> Result<()> {
    let dir = dir.as_ref();
    crate::file_access::check_write(dir).map_err(EngineError::Io)?;
    std::fs::create_dir_all(dir).map_err(|err| EngineError::Io(format!("{}: {err}", dir.display())))
}

/// Write `bytes` to the file at `path`, inside the automation write root when there is one.
pub(crate) fn write_path(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> Result<()> {
    let path = path.as_ref();
    crate::file_access::check_write(path).map_err(EngineError::Io)?;
    std::fs::write(path, bytes).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))
}

/// The entries of the folder `dir` (readable under the automation read root); a missing or
/// unreadable folder has none.
pub(crate) fn list_dir(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let dir = dir.as_ref();
    if crate::file_access::check_read(dir).is_err() {
        return Vec::new();
    }
    std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default()
}

/// Deletes the file at `path`, inside the automation write root when there is one.
pub(crate) fn remove_path(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    crate::file_access::check_write(path).map_err(EngineError::Io)?;
    std::fs::remove_file(path).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))
}

/// Moves the file `from` to `to`, both inside the automation write root when there is one.
pub(crate) fn rename_path(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    crate::file_access::check_write(from).map_err(EngineError::Io)?;
    crate::file_access::check_write(to).map_err(EngineError::Io)?;
    std::fs::rename(from, to).map_err(|err| EngineError::Io(format!("{}: {err}", to.display())))
}

/// A relative `Audio Files/` path no other source in `s` already uses.
fn unique_audio_rel(s: &Session, stem: &str) -> String {
    let stem = sanitize_name(stem);
    let (plain, prefix) = (format!("Audio Files/{stem}.wav"), format!("Audio Files/{stem}-"));
    let used: std::collections::HashSet<u64> = s
        .sources
        .iter()
        .filter_map(|src| {
            if src.path == plain {
                return Some(1);
            }
            let n = src.path.strip_prefix(&prefix)?.strip_suffix(".wav")?;
            n.parse().ok().filter(|i: &u64| *i > 1 && i.to_string() == n)
        })
        .collect();
    match (1..).find(|i| !used.contains(i)) {
        Some(n) if n > 1 => format!("{prefix}{n}.wav"),
        _ => plain,
    }
}

/// Register decoded audio as a new source. Resamples to the session rate if needed.
pub fn add_source(s: &mut Session, name: &str, mut buf: AudioBuffer, path: Option<&str>, format: FileFormat) -> SourceId {
    let sr = s.sample_rate.hz();
    if buf.sample_rate != sr && buf.sample_rate > 0 {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, sr);
        buf.sample_rate = sr;
    }
    insert_source(s, name, Arc::new(SourceAudio::new(buf)), path, format)
}

/// Register audio already decoded at the session rate, with its waveform overview built (Edit ›
/// Split into Files copies a piece ahead of the command): a new unsaved source written to
/// `Audio Files/` on save, as [`add_source`]'s.
pub fn add_source_audio(s: &mut Session, name: &str, audio: Arc<SourceAudio>, format: FileFormat) -> SourceId {
    insert_source(s, name, audio, None, format)
}

fn insert_source(s: &mut Session, name: &str, audio: Arc<SourceAudio>, path: Option<&str>, format: FileFormat) -> SourceId {
    let sr = s.sample_rate.hz();
    let id = SourceId(s.alloc());
    let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).unwrap_or(name).to_string();
    let stored = match path {
        Some(p) => p.to_string(),
        None => unique_audio_rel(s, &stem),
    };
    s.sources.push(Source {
        id,
        name: stem.clone(),
        path: stored,
        channels: u16::try_from(audio.num_channels()).unwrap_or(u16::MAX),
        frames: audio.frames() as u64,
        sample_rate: sr,
        format,
        time_reference: 0,
        unsaved: true,
    });
    s.pool.insert(id, audio);
    id
}

/// A BWF time stamp (samples at the file's rate) at the session's rate, as the audio is resampled.
fn stamp_at_rate(stamp: u64, file_rate: u32, session_rate: u32) -> u64 {
    if file_rate == 0 || file_rate == session_rate {
        return stamp;
    }
    let (from, to) = (u128::from(file_rate), u128::from(session_rate));
    u64::try_from((u128::from(stamp) * to + from / 2) / from).unwrap_or(u64::MAX)
}

/// Decode and place an audio file. With no target track, a new track of matching width is made.
pub fn import_audio_bytes(e: &mut Engine, name: &str, bytes: &[u8], path: Option<&str>, target: Option<TrackId>, at: Samples) -> Result<Value> {
    let ext = Path::new(name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let (info, buf) = soundcraft_audio_io::decode(bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    let buf = at_rate(buf, e.session().sample_rate.hz(), MAX_IMPORT_SAMPLES)
        .ok_or_else(|| EngineError::Io(format!("{name}: file is too large to import")))?;
    if buf.frames() == 0 {
        return Err(EngineError::Io(format!("{name}: no audio")));
    }
    let channels = buf.num_channels();
    let file_rate = info.sample_rate;
    let copy = e.copy_imported_audio;
    let s = e.session_mut();
    let session_rate = s.sample_rate.hz();
    let src = add_source(s, name, buf, path, info.format);
    // Not copied: a file that holds exactly this audio (the session's rate) is referred to where
    // it is. Resampled audio is written to the project all the same.
    if !copy
        && path.is_some_and(|p| Path::new(p).is_absolute())
        && file_rate == session_rate
        && let Some(sx) = s.sources.last_mut()
    {
        sx.unsaved = false;
    }
    if let (Some(bwf), Some(sx)) = (info.bwf.as_ref(), s.sources.last_mut()) {
        sx.time_reference = stamp_at_rate(bwf.time_reference, file_rate, session_rate);
    }
    let frames = s.source(src).map_or(0, |x| i64::try_from(x.frames).unwrap_or(0));
    let track = match target {
        Some(t) => t,
        None => {
            let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).unwrap_or("Audio").to_string();
            s.add_track(TrackKind::Audio, ChannelFormat::for_channels(channels), Some(&stem))
        }
    };
    let cid = s.new_clip_id();
    let stem = s.source(src).map(|x| x.name.clone()).unwrap_or_default();
    crate::edit::place_clip(s, track, Clip::audio(cid, stem, src, 0, at, frames));
    Ok(json!({"track": track, "clip": cid, "source": src, "frames": frames, "channels": channels}))
}

/// Loads an audio file's bytes into an instrument track's Sampler as one undo step ("Load
/// Sampler Sample"): for files that have no path, such as one dropped into the browser. The same
/// checks as `mix.sampler_load`; not journaled, since the bytes can be large.
pub fn load_sampler_bytes(e: &mut Engine, track: TrackId, name: &str, bytes: &[u8]) -> Result<Value> {
    let before = e.session_arc();
    let r = crate::cmd::sampler::attach(e, track, name, bytes, None, Vec::new())?;
    e.push_undo("Load Sampler Sample", before);
    Ok(r)
}

/// Loads a dropped file's bytes into slot `(pad, layer, robin)` (each from 0) of `track`'s Juke,
/// as one undo step.
pub fn load_kit_bytes(e: &mut Engine, track: TrackId, slot: (usize, usize, usize), name: &str, bytes: &[u8]) -> Result<Value> {
    let before = e.session_arc();
    let r = crate::cmd::juke::attach_sample(e, track, slot, name, bytes, None)?;
    e.push_undo("Load Pad Sample", before);
    Ok(r)
}

/// Import a movie: a Video track clip spanning the picture (the movie is referenced by `path`,
/// never copied) plus its audio on a new audio track when it has a usable audio stream. Fails
/// only when the file has neither a readable picture nor audio.
pub fn import_video_bytes(e: &mut Engine, path: &str, bytes: Vec<u8>, at: Samples) -> Result<Value> {
    let stem = Path::new(path).file_stem().and_then(|n| n.to_str()).unwrap_or("Video").to_string();
    let ext = Path::new(path).extension().and_then(|x| x.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let bytes: Arc<[u8]> = bytes.into();
    let movie = soundcraft_video::Movie::open(Arc::clone(&bytes)).map(|m| m.info().clone());
    let audio_name = format!("{stem} audio.{}", if ext == "mov" { "mp4" } else { ext.as_str() });
    let audio = import_audio_bytes(e, &audio_name, &bytes, Some(path), None, at);
    let info = match (movie, &audio) {
        (Ok(info), _) => info,
        (Err(verr), Ok(a)) => {
            e.message(format!("{stem}: imported the audio; no picture ({verr})"));
            return Ok(json!({"audio": a, "video": Value::Null, "video_error": verr.to_string()}));
        }
        (Err(verr), Err(aerr)) => return Err(EngineError::Io(format!("{path}: {verr}; no usable audio stream ({aerr})"))),
    };
    let abs = std::fs::canonicalize(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned());
    let s = e.session_mut();
    let sr = s.sample_rate.as_f64();
    let len = (info.duration * sr).round();
    let len = if len.is_finite() { (len as Samples).clamp(1, sr as Samples * 3600 * 48) } else { 1 };
    let vid = SourceId(s.alloc());
    s.videos.push(soundcraft_model::VideoSource {
        id: vid,
        name: Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("movie").to_string(),
        path: abs,
        width: info.width,
        height: info.height,
        frame_rate: info.frame_rate,
        duration: info.duration,
        codec: info.codec.clone(),
        offline: None,
    });
    // Reuse the first Video track when the new clip does not overlap its clips; else add one at the top.
    let range = Range::new(at, at.saturating_add(len));
    let track = match s.tracks.iter().find(|t| t.kind == TrackKind::Video) {
        Some(t) if !t.clips().iter().any(|c| c.range().overlaps(&range)) => t.id,
        _ => {
            let id = s.add_track(TrackKind::Video, ChannelFormat::Mono, Some(&stem));
            if let Some(i) = s.track_index(id) {
                let t = s.tracks.remove(i);
                s.tracks.insert(0, t);
            }
            id
        }
    };
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, track, Clip::video(cid, stem.clone(), vid, 0, at, len));
    if !info.decodable {
        e.message(format!("{stem}: {} pictures cannot be shown yet; the Video track shows the clip only", info.codec_detail));
    }
    if let Err(err) = &audio {
        e.message(format!("{stem}: no usable audio stream ({err})"));
    }
    Ok(json!({
        "video": {"track": track, "clip": cid, "source": vid, "length": len, "info": info},
        "audio": audio.as_ref().ok(),
        "audio_error": audio.as_ref().err().map(ToString::to_string),
    }))
}

/// Resolve each movie referenced by the session file `session` ([`stored_path`]): keep the
/// resolved path when its file exists, else look for a file of the same name next to the session
/// (moved projects) and relink. A movie whose stored path is refused and has no such copy is
/// marked offline. Returns the movies still missing.
pub fn relink_videos(s: &mut Session, session: &Path) -> Vec<String> {
    let session_dir = session.parent().unwrap_or(Path::new(""));
    let mut missing = Vec::new();
    for v in &mut s.videos {
        v.offline = None;
        let p = stored_path(&v.path, session);
        // The UI opens `path` as it is: keep the checked path, not one relative to the working
        // directory.
        if let Ok(found) = &p
            && file_found(found)
        {
            v.path = found.to_string_lossy().into_owned();
            continue;
        }
        let name = Path::new(&v.path).file_name().map(PathBuf::from).unwrap_or_default();
        let near = [session_dir.join(&name), session_dir.join("Video Files").join(&name)]
            .into_iter()
            .find(|c| !name.as_os_str().is_empty() && file_found(c));
        match (near, p) {
            (Some(c), _) => v.path = c.to_string_lossy().into_owned(),
            (None, Ok(p)) => missing.push(format!("{} (video)", p.display())),
            (None, Err(why)) => {
                missing.push(format!("{} (video): {why}", v.path));
                v.offline = Some(why);
            }
        }
    }
    missing
}

/// Most meter changes a MIDI file may bring into the tempo map; scores have a handful.
const MAX_MIDI_METERS: usize = 10_000;

/// Import a Standard MIDI File as MIDI tracks (one per SMF track).
pub fn import_midi_bytes(e: &mut Engine, bytes: &[u8], at: Samples, tempo_map: bool) -> Result<Vec<TrackId>> {
    let smf = soundcraft_midi::read_smf(bytes).map_err(|err| EngineError::Io(err.to_string()))?;
    let s = e.session_mut();
    if tempo_map {
        // Each meter is placed by walking the ones before it.
        if smf.meters.len() > MAX_MIDI_METERS {
            return Err(EngineError::Io(format!("MIDI file: more than {MAX_MIDI_METERS} meter changes")));
        }
        for (tick, bpm) in &smf.tempos {
            let _ = s.tempo.set_tempo(*tick, *bpm);
        }
        for (tick, n, d) in &smf.meters {
            let _ = s.tempo.set_meter(*tick, u32::from(*n), u32::from(*d));
        }
    }
    let base_tick = s.tempo.samples_to_ticks(at, s.sample_rate);
    let mut out = Vec::new();
    for (i, tr) in smf.tracks.iter().enumerate() {
        let lyrics: Vec<(i64, &str)> = smf.lyrics.iter().filter(|l| l.0 == i).map(|l| (l.1, l.2.as_str())).collect();
        if tr.sequence.notes.is_empty() && tr.sequence.ctrls.is_empty() && lyrics.is_empty() {
            continue;
        }
        let mut name = if tr.name.trim().is_empty() { format!("MIDI {}", i + 1) } else { tr.name.clone() };
        soundcraft_model::truncate_chars(&mut name, soundcraft_model::MAX_NAME_CHARS);
        let id = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
        let drums = !tr.sequence.notes.is_empty() && tr.sequence.notes.iter().all(|n| n.channel == 9);
        if let Some(t) = s.track_mut(id) {
            t.instrument = Some(soundcraft_model::Insert::new(if drums { "drum_synth" } else { "subtractive_synth" }));
        }
        // A controller on the clip's last tick would not play, so the clip runs one tick past the last one.
        let past_ctrls =
            tr.sequence.ctrls.iter().map(|c| c.tick.saturating_add(1)).chain(lyrics.iter().map(|l| l.0.saturating_add(480))).max().unwrap_or(0);
        let end_tick = base_tick.saturating_add(tr.sequence.end_tick().max(past_ctrls).max(1));
        let len = s.tempo.tick_to_samples(end_tick, s.sample_rate).saturating_sub(at);
        if len > s.sample_rate.samples(86_400.0) {
            return Err(EngineError::Io(format!("MIDI track {}: longer than 24 hours", i + 1)));
        }
        let cid = s.new_clip_id();
        let mut clip = Clip::midi(cid, name, at, len.max(1), tr.sequence.clone());
        clip.lyrics = midi_lyric_lines(s, base_tick, at, &lyrics);
        crate::edit::place_clip(s, id, clip);
        out.push(id);
    }
    Ok(out)
}

/// A MIDI file's lyric events (`(tick, text)` from the file's start) as a MIDI clip's lyric lines
/// (samples from the clip start at `at`, `base_tick`). Karaoke markers start lines: a text
/// beginning with `/` or `\`, or a line break after the previous text; without any, a gap of 2 s
/// or more does. Each word lasts until the next (at most 2 s).
fn midi_lyric_lines(s: &soundcraft_model::Session, base_tick: i64, at: Samples, lyrics: &[(i64, &str)]) -> Vec<soundcraft_model::LyricLine> {
    use soundcraft_model::{LyricLine, LyricWord};
    let pos = |tick: i64| s.tempo.tick_to_samples(base_tick.saturating_add(tick.max(0)), s.sample_rate).saturating_sub(at);
    let gap = s.sample_rate.samples(2.0);
    const MARKS: [char; 2] = ['/', '\\'];
    const BREAKS: [char; 2] = ['\r', '\n'];
    let marked = lyrics.iter().any(|(_, t)| t.starts_with(MARKS) || t.ends_with(BREAKS));
    let mut sorted: Vec<(i64, &str)> = lyrics.to_vec();
    sorted.sort_by_key(|l| l.0);
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut new_line = true;
    for (k, (tick, raw)) in sorted.iter().enumerate() {
        let text = raw.trim_start_matches(MARKS).trim_end_matches(BREAKS).trim();
        let start = pos(*tick);
        let next = sorted.get(k + 1).map_or(start.saturating_add(gap), |n| pos(n.0));
        let breaks = if marked {
            new_line || raw.starts_with(MARKS)
        } else {
            lines.last().and_then(|l| l.words.last()).is_none_or(|w| start - w.start >= gap)
        };
        new_line = raw.ends_with(BREAKS);
        if text.is_empty() {
            continue;
        }
        let word = LyricWord { start, end: next.min(start.saturating_add(gap)).max(start), text: text.to_string() };
        match lines.last_mut() {
            Some(l) if !breaks => l.words.push(word),
            _ => lines.push(LyricLine { start, end: start, words: vec![word] }),
        }
    }
    soundcraft_model::sanitize_lyrics(&mut lines);
    lines
}

/// The session's tempo map as SMF tempo and meter events, and the tick offset everything else
/// takes. An SMF has no tempo ramps: a ramp is written in sixteenth-note steps, each at the tempo
/// that lasts exactly as long as the ramp does over it. Its tick 0 is the file's start: when bar
/// 1 (the map's origin) is later than the session's start, a pickup bar of whole quarters fills
/// the time before it, at the tempo that lasts exactly that long, so every event keeps its time
/// and the session's bars stay bars. Returns the tempos, the meters, and the file tick of each
/// session tick.
pub(crate) fn smf_tempo_map(s: &Session) -> (Vec<(i64, f64)>, Vec<(i64, u8, u8)>, impl Fn(i64) -> i64) {
    let (map, sr) = (&s.tempo, s.sample_rate);
    let q = i64::from(soundcraft_midi::PPQ);
    let mut tempos: Vec<(i64, f64)> = Vec::new();
    let mut meters: Vec<(i64, u8, u8)> = Vec::new();
    let first = map.tempos().first().map_or(120.0, |t| t.bpm);
    let origin_s = map.origin() as f64 / sr.as_f64();
    let (off, pickup) = if origin_s > 0.0 {
        let n = (origin_s * first / 60.0).round().clamp(1.0, 255.0);
        tempos.push((0, n * 60.0 / origin_s));
        meters.push((0, n as u8, 4));
        (n as i64 * q, n * 60.0 / origin_s)
    } else {
        (0, first)
    };
    let events = map.tempos();
    for (k, t) in events.iter().enumerate() {
        tempos.push((t.tick + off, t.bpm));
        let Some(next) = events.get(k + 1).filter(|n| t.ramp && n.tick > t.tick) else { continue };
        // The ramp in steps (the first replaces the event's own tempo).
        tempos.pop();
        let step = q / 4;
        let mut a = t.tick;
        while a < next.tick {
            let b = (a + step).min(next.tick);
            let secs = (map.tick_to_samples(b, sr) - map.tick_to_samples(a, sr)) as f64 / sr.as_f64();
            let bpm = if secs > 0.0 { (b - a) as f64 / q as f64 * 60.0 / secs } else { map.tempo_at_tick(a) };
            tempos.push((a + off, bpm));
            a = b;
        }
    }
    meters.extend(map.meters().iter().map(|m| (m.tick + off, u8::try_from(m.numerator).unwrap_or(4), u8::try_from(m.denominator).unwrap_or(4))));
    // Before bar 1 the session runs at its first tempo, the file at the pickup's.
    let file_tick = move |tick: i64| if tick < 0 { (off + (tick as f64 * pickup / first).round() as i64).max(0) } else { tick + off };
    (tempos, meters, file_tick)
}

/// Export MIDI clips as a format-1 SMF. `only` limits the file to those tracks.
pub fn export_midi(e: &Engine, only: Option<&[TrackId]>) -> Result<Vec<u8>> {
    let s = e.session();
    let (tempos, meters, file_tick) = smf_tempo_map(s);
    let mut smf = soundcraft_midi::Smf {
        format: 1,
        tracks: Vec::new(),
        tempos,
        meters,
        markers: s.markers.iter().map(|m| (file_tick(s.tempo.samples_to_ticks(m.start, s.sample_rate)), m.name.clone())).collect(),
        key_sigs: Vec::new(),
        lyrics: Vec::new(),
    };
    let chosen: Vec<_> = s.tracks.iter().filter(|t| t.kind.is_midi() && only.is_none_or(|ids| ids.contains(&t.id))).collect();
    // Tracks without MIDI that hold lyrics (a transcribed vocal) export as lyrics-only tracks.
    let sung: Vec<_> = s
        .tracks
        .iter()
        .filter(|t| !t.kind.is_midi() && soundcraft_model::track_has_lyrics(t) && only.is_none_or(|ids| ids.contains(&t.id)))
        .collect();
    if only.is_some() && chosen.is_empty() && sung.is_empty() {
        return Err(EngineError::BadParams("file.export_midi".into(), "none of the requested tracks hold MIDI".into()));
    }
    for t in chosen {
        let mut seq = soundcraft_midi::Sequence::default();
        for c in t.clips() {
            if let ClipContent::Midi { sequence } = &c.content {
                let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
                for n in &sequence.notes {
                    let mut n = *n;
                    let (a, b) = (file_tick(n.start.saturating_add(base)), file_tick(n.end().saturating_add(base)));
                    n.start = a;
                    n.length = (b - a).max(1);
                    seq.notes.push(n);
                }
                for ev in &sequence.ctrls {
                    let mut ev = *ev;
                    ev.tick = file_tick(ev.tick.saturating_add(base));
                    seq.ctrls.push(ev);
                }
            }
        }
        seq.sort();
        let index = smf.tracks.len();
        smf.lyrics.extend(crate::lyrics::midi_lyrics(s, t.id).into_iter().map(|(tick, text)| (index, file_tick(tick), text)));
        smf.tracks.push(soundcraft_midi::SmfTrack { name: t.name.clone(), sequence: seq });
    }
    for t in sung {
        let index = smf.tracks.len();
        smf.lyrics.extend(crate::lyrics::midi_lyrics(s, t.id).into_iter().map(|(tick, text)| (index, file_tick(tick), text)));
        smf.tracks.push(soundcraft_midi::SmfTrack { name: format!("{} Lyrics", t.name), sequence: soundcraft_midi::Sequence::default() });
    }
    soundcraft_midi::write_smf(&smf).map_err(|err| EngineError::Io(err.to_string()))
}

/// A bounce of the main mix, or of one bus, over a range, written to its file block by block so no
/// bounce has to fit in memory, and rendered a slice at a time ([`Bounce::step`]) so a UI can show
/// progress. The file appears at its path only once complete; a bounce that fails, or is dropped
/// unfinished, leaves any older file there alone. A host renders it through its live plugin
/// instances ([`Bounce::through`]); without them the render makes instances of its own from the
/// plugins' stored state, when the first block is rendered.
pub struct Bounce {
    session: Arc<Session>,
    path: PathBuf,
    part: PathBuf,
    range: Range,
    bus: Option<BusId>,
    fold: Option<ChannelFormat>,
    normalize: bool,
    /// Measuring the peak to normalize to, the first of two passes.
    measuring: bool,
    /// Made at the first [`Bounce::step`] (not before: a host may lend its instances first).
    render: Option<soundcraft_mix::RangeRender>,
    /// The host's live instances, until the render takes them ([`Bounce::through`]).
    live: Option<soundcraft_mix::LiveInstances>,
    /// The Library the band plays from, when the session's band plays into the bounce.
    band: Option<Arc<soundcraft_band::library::LibraryData>>,
    block: Vec<Vec<f32>>,
    done: usize,
    gain: f32,
    peak: f32,
    meter: soundcraft_dsp::meter::LoudnessMeter,
    enc: Option<soundcraft_audio_io::FileEncoder<Out>>,
}

/// A finished bounce: the size of its file and its levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounced {
    pub bytes: u64,
    pub peak_db: f32,
    pub lufs: f32,
    pub true_peak_db: f32,
}

impl Bounce {
    /// Starts a bounce of `s`'s main mix, or of `bus`, over `r` to `path`: every channel (SMPTE/WAV
    /// order, with the WAV speaker mask of its format), or an ITU stereo fold-down when
    /// `fold_stereo`, peak-normalized to -0.1 dBFS when `normalize` (which renders twice).
    pub fn new(
        s: Arc<Session>,
        path: &Path,
        r: Range,
        opts: &EncodeOptions,
        bus: Option<BusId>,
        normalize: bool,
        fold_stereo: bool,
    ) -> Result<Bounce> {
        if r.len() > s.sample_rate.samples(MAX_BOUNCE_SECS) {
            return Err(EngineError::BadParams("file.bounce_mix".into(), "bounces are limited to 4 hours".into()));
        }
        let (fmt, channels, fold) = bounce_layout(&s, bus, fold_stereo);
        let name = path.file_name().map_or_else(|| "bounce".into(), |n| n.to_string_lossy().into_owned());
        let part = path.with_file_name(format!("{name}.part"));
        crate::file_access::check_write(path).map_err(EngineError::Io)?;
        crate::file_access::check_write(&part).map_err(EngineError::Io)?;
        // A bounce into the project's `Bounced Files`, not made yet, makes it (any other missing
        // folder is an error: a mistyped destination is not created).
        if let Some(dir) =
            path.parent().filter(|d| d.file_name().is_some_and(|n| n == "Bounced Files") && !d.is_dir() && d.parent().is_some_and(Path::is_dir))
        {
            make_dir(dir)?;
        }
        let enc = (|| -> std::result::Result<_, soundcraft_audio_io::AudioError> {
            let enc = soundcraft_audio_io::FileEncoder::new(Out::create(&part)?, opts, channels, s.sample_rate.hz(), Some(fmt.channel_mask()))?;
            enc.check_room(u64::try_from(r.len()).unwrap_or(0))?;
            Ok(enc)
        })()
        .map_err(|err| {
            let _ = std::fs::remove_file(&part);
            EngineError::Io(format!("{}: {err}", path.display()))
        })?;
        let meter = soundcraft_dsp::meter::LoudnessMeter::new(s.sample_rate.as_f64() as f32, channels);
        Ok(Bounce {
            session: s,
            path: path.to_path_buf(),
            part,
            range: r,
            bus,
            fold,
            normalize,
            measuring: normalize,
            render: None,
            live: None,
            band: None,
            block: Vec::new(),
            done: 0,
            gain: 1.0,
            peak: 0.0,
            meter,
            enc: Some(enc),
        })
    }

    /// The session's band plays into the bounce (tracks whose input is a musician), from `lib`.
    pub fn with_band(mut self, lib: Arc<soundcraft_band::library::LibraryData>) -> Bounce {
        if let Some(r) = &mut self.render {
            r.adopt_band_library(&self.session, &lib);
        }
        self.band = Some(lib);
        self
    }

    /// Renders through the host's live plugin instances (see
    /// [`soundcraft_mix::LiveInstances`]): what the person hears is what is bounced. Taken by
    /// the next [`Bounce::step`]; given after the render started, the instances go straight back.
    pub fn through(mut self, live: soundcraft_mix::LiveInstances) -> Bounce {
        if self.render.is_none() {
            self.live = Some(live);
        }
        self
    }

    /// Whether the bounce renders through the host's live instances.
    pub fn is_live(&self) -> bool {
        self.live.is_some() || self.render.as_ref().is_some_and(soundcraft_mix::RangeRender::is_live)
    }

    /// The render, made at the first block: through the live instances when the host lent them.
    fn render(&mut self) -> &mut soundcraft_mix::RangeRender {
        let (live, band, session, bus, range) = (&mut self.live, &self.band, &self.session, self.bus, self.range);
        self.render.get_or_insert_with(|| {
            let mut r = match live.take() {
                Some(live) => soundcraft_mix::RangeRender::with_instances(session, bus, range, live),
                None => soundcraft_mix::RangeRender::new(session, bus, range, 1024),
            };
            if let Some(lib) = band {
                r.adopt_band_library(session, lib);
            }
            r
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn range(&self) -> Range {
        self.range
    }

    /// How far along the bounce is, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let pass = (self.done as f64 / self.range.len().max(1) as f64).min(1.0) as f32;
        match (self.normalize, self.measuring) {
            (false, _) => pass,
            (true, true) => pass / 2.0,
            (true, false) => 0.5 + pass / 2.0,
        }
    }

    /// Renders blocks until `more` returns false (at least one per call). Returns the finished
    /// bounce once its file is complete.
    pub fn step(&mut self, mut more: impl FnMut() -> bool) -> Result<Option<Bounced>> {
        loop {
            self.render();
            let Some(render) = self.render.as_mut() else { return Ok(None) };
            match render.next_block(&self.session) {
                Some((chunk, n)) => {
                    fill(&mut self.block, chunk, n, self.fold, self.gain);
                    self.done += n;
                    self.peak = self.peak.max(soundcraft_dsp::offline::peak(&self.block));
                    if !self.measuring {
                        self.meter.process(&self.block, n);
                        if let Some(enc) = &mut self.enc {
                            enc.write(&self.block, n).map_err(|err| EngineError::Io(format!("{}: {err}", self.path.display())))?;
                        }
                    }
                }
                None if self.measuring => {
                    self.gain = if self.peak > 0.0 { soundcraft_dsp::db_to_gain(-0.1) / self.peak } else { 1.0 };
                    self.peak = 0.0;
                    self.done = 0;
                    self.measuring = false;
                    // The second pass renders again from the start, through the same instances.
                    self.live = self.render.take().and_then(soundcraft_mix::RangeRender::into_live);
                }
                None => return self.finish().map(Some),
            }
            if !more() {
                return Ok(None);
            }
        }
    }

    /// Renders the whole bounce (see [`Bounce::step`]).
    pub fn run(mut self) -> Result<Bounced> {
        loop {
            if let Some(done) = self.step(|| true)? {
                return Ok(done);
            }
        }
    }

    fn finish(&mut self) -> Result<Bounced> {
        let bytes = match self.enc.take() {
            Some(enc) => enc.finish().map_err(|err| EngineError::Io(format!("{}: {err}", self.path.display())))?.complete(&self.part, &self.path)?,
            None => 0,
        };
        Ok(Bounced {
            bytes,
            peak_db: soundcraft_dsp::gain_to_db(self.peak),
            lufs: self.meter.integrated_lufs(),
            true_peak_db: self.meter.true_peak_db(),
        })
    }
}

impl Drop for Bounce {
    /// Removes the part file unless the bounce completed (which renamed it).
    fn drop(&mut self) {
        drop(self.enc.take());
        let _ = std::fs::remove_file(&self.part);
    }
}

/// Where a bounce is written: its part file, or memory in the browser, which has no file system.
enum Out {
    File(std::io::BufWriter<std::fs::File>),
    Memory(std::io::Cursor<Vec<u8>>),
}

impl Out {
    fn create(part: &Path) -> std::io::Result<Out> {
        if cfg!(target_arch = "wasm32") {
            return Ok(Out::Memory(std::io::Cursor::new(Vec::new())));
        }
        Ok(Out::File(std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(part)?)))
    }

    /// Puts the finished file at `path` and returns its size: the part file is synced and renamed
    /// over it; in the browser the bytes are delivered ([`write_file`]).
    fn complete(self, part: &Path, path: &Path) -> Result<u64> {
        match self {
            Out::File(out) => {
                let mut len = 0;
                let synced = out.into_inner().map_err(std::io::IntoInnerError::into_error).and_then(|f| {
                    f.sync_all()?;
                    len = f.metadata()?.len();
                    Ok(())
                });
                replace_with(part, path, synced).map(|()| len)
            }
            Out::Memory(m) => {
                let bytes = m.into_inner();
                write_file(&path.to_string_lossy(), &bytes)?;
                Ok(bytes.len() as u64)
            }
        }
    }
}

impl std::io::Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Out::File(f) => f.write(buf),
            Out::Memory(m) => {
                // A long bounce may not fit in the browser's memory: fail instead of aborting.
                let end = usize::try_from(m.position()).unwrap_or(usize::MAX).saturating_add(buf.len());
                let v = m.get_mut();
                v.try_reserve(end.saturating_sub(v.len())).map_err(|_| std::io::ErrorKind::OutOfMemory)?;
                m.write(buf)
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Out::File(f) => f.flush(),
            Out::Memory(_) => Ok(()),
        }
    }
}

impl std::io::Seek for Out {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Out::File(f) => f.seek(pos),
            Out::Memory(m) => m.seek(pos),
        }
    }
}

const MAX_BOUNCE_SECS: f64 = 4.0 * 3600.0;

/// The format, channel count and fold-down source of a bounce of the main mix or `bus`: every
/// channel (a mono or stereo main mix as stereo), or an ITU stereo fold-down of a surround source
/// when `fold_stereo`.
fn bounce_layout(s: &Session, bus: Option<BusId>, fold_stereo: bool) -> (ChannelFormat, usize, Option<ChannelFormat>) {
    let nch = soundcraft_mix::source_channels(s, bus);
    let fmt = match bus {
        Some(id) => s.bus(id).map_or(ChannelFormat::Stereo, |b| b.format),
        None if nch <= 2 => ChannelFormat::Stereo,
        None => s.main_format(),
    };
    if fold_stereo && nch > 2 { (ChannelFormat::Stereo, 2, Some(fmt)) } else { (fmt, nch, None) }
}

/// The first `n` frames of `chunk`, folded down from `fold` when set and scaled by `gain`.
fn fill(block: &mut Vec<Vec<f32>>, chunk: &[Vec<f32>], n: usize, fold: Option<ChannelFormat>, gain: f32) {
    let folded = fold.map(|from| soundcraft_mix::fold_down_stereo(from, chunk));
    let src = folded.as_deref().unwrap_or(chunk);
    block.resize_with(src.len(), Vec::new);
    for (d, c) in block.iter_mut().zip(src) {
        d.clear();
        d.extend(c.iter().take(n).map(|v| v * gain));
    }
}

/// Bounce one file per audio/instrument track (each soloed, so its sends and auxes are included)
/// into `dir`. Returns the written paths.
pub fn bounce_stems(e: &Engine, dir: &str, r: Range, opts: &EncodeOptions) -> Result<Vec<String>> {
    bounce_stems_with(e, dir, r, opts, false)
}

/// [`bounce_stems`] in the main format, or folded down to stereo when `fold_stereo`.
pub fn bounce_stems_with(e: &Engine, dir: &str, r: Range, opts: &EncodeOptions, fold_stereo: bool) -> Result<Vec<String>> {
    let s = e.session();
    if r.len() > s.sample_rate.samples(MAX_BOUNCE_SECS) {
        return Err(EngineError::BadParams("file.bounce_stems".into(), "bounces are limited to 4 hours".into()));
    }
    make_dir(dir)?;
    let ext = soundcraft_audio_io::encode_extension(opts.format);
    let mut out = Vec::new();
    let stems: Vec<(TrackId, String)> = s
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi) && !t.inactive && !t.mixer.mute)
        .map(|t| (t.id, t.name.clone()))
        .collect();
    for (id, name) in stems {
        let mut solo = s.clone();
        for t in &mut solo.tracks {
            t.mixer.solo = t.id == id;
        }
        let path = Path::new(dir).join(format!("{} - {}.{ext}", sanitize_name(&s.name), sanitize_name(&name)));
        Bounce::new(Arc::new(solo), &path, r, opts, None, false, fold_stereo)?.with_band(e.band_library()).run()?;
        out.push(path.to_string_lossy().into_owned());
    }
    Ok(out)
}

/// Limit a range used for rendering to the track's material (plus a second), so a huge
/// selection never renders hours of silence.
pub fn bounded_to_track(s: &Session, t: TrackId, r: Range) -> Range {
    let Some(tr) = s.track(t) else { return Range::point(r.start) };
    let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0).saturating_add(s.sample_rate.samples(1.0));
    let start = r.start.max(0).min(end);
    Range::new(start, r.end.min(end).max(start))
}

/// Transient positions (absolute) in a track's clips within `r`.
pub fn transients_in(s: &Session, t: TrackId, r: Range, sens: f32) -> Vec<Samples> {
    let r = if r.is_empty() { Range::new(0, s.content_end()) } else { r };
    let r = bounded_to_track(s, t, r);
    let audio = soundcraft_mix::render_clips(s, t, r);
    soundcraft_dsp::offline::detect_transients(&audio, s.sample_rate.as_f64() as f32, sens).into_iter().map(|i| r.start + i as i64).collect()
}

/// Strip Silence: keep only the non-silent ranges of clips inside `r`.
pub fn strip_silence(s: &mut Session, t: TrackId, r: Range, thr: f32, min_len: Samples, pre: Samples, post: Samples) -> Result<usize> {
    let r = bounded_to_track(s, t, r);
    let keep = non_silent(s, t, r, thr, min_len, pre, post)?;
    let mut cut_points = vec![r.start];
    for (a, b) in &keep {
        cut_points.push(*a);
        cut_points.push(*b);
    }
    cut_points.push(r.end);
    for p in &cut_points {
        crate::edit::separate_at(s, t, *p);
    }
    // Remove pieces that fall in the gaps.
    let mut gaps = Vec::new();
    let mut prev = r.start;
    for (a, b) in &keep {
        if *a > prev {
            gaps.push(Range::new(prev, *a));
        }
        prev = *b;
    }
    if prev < r.end {
        gaps.push(Range::new(prev, r.end));
    }
    if let Some(pl) = s.track_mut(t).and_then(|x| x.playlist_mut()) {
        pl.clips.retain(|c| !gaps.iter().any(|g| c.start >= g.start && c.end() <= g.end));
    }
    Ok(keep.len())
}

/// Most samples (over all channels) Strip Silence renders at once.
const STRIP_WINDOW_SAMPLES: usize = 1 << 24;

/// The non-silent ranges of `t`'s clips in `r`. The audio is rendered a window at a time where
/// audio clips play, so all of `r` is scanned however long it is (one render stops at
/// `MAX_RENDER_SAMPLES`); at most a day of clips, Pro Tools' longest session.
fn non_silent(s: &Session, t: TrackId, r: Range, thr: f32, min_len: Samples, pre: Samples, post: Samples) -> Result<Vec<(Samples, Samples)>> {
    let Some(tr) = s.track(t) else { return Ok(Vec::new()) };
    let mut clips: Vec<Range> = tr.clips().iter().filter(|c| c.is_audio() && !c.muted).filter_map(|c| c.range().intersect(&r)).collect();
    clips.sort_by_key(|x| x.start);
    let mut spans: Vec<Range> = Vec::new();
    for x in clips {
        match spans.last_mut() {
            Some(last) if x.start <= last.end => last.end = last.end.max(x.end),
            _ => spans.push(x),
        }
    }
    if spans.iter().map(Range::len).fold(0, i64::saturating_add) > s.sample_rate.samples(86_400.0) {
        return Err(EngineError::BadParams("edit.strip_silence".into(), format!("`{}` has more than a day of clips to scan", tr.name)));
    }
    let window = i64::try_from(STRIP_WINDOW_SAMPLES / tr.channels().max(1)).unwrap_or(i64::MAX);
    let len = |n: Samples| u64::try_from(n).unwrap_or(0);
    let mut scan = soundcraft_dsp::offline::SilenceScan::new(thr, len(min_len));
    let mut at = r.start;
    for x in spans {
        scan.skip(len(x.start - at));
        at = x.start;
        while at < x.end {
            let end = at.saturating_add(window).min(x.end);
            scan.push(&soundcraft_mix::render_clips(s, t, Range::new(at, end)));
            at = end;
        }
    }
    scan.skip(len(r.end - at));
    Ok(scan.finish(len(pre), len(post)).into_iter().map(|(a, b)| (r.start.saturating_add_unsigned(a), r.start.saturating_add_unsigned(b))).collect())
}

/// Consolidate `r` on a track into one new clip backed by a rendered source. The clip covers the
/// whole range, silence included; a range too long to render is refused rather than shortened.
pub fn consolidate(e: &mut Engine, cmd: &str, t: TrackId, r: Range) -> Result<bool> {
    let s = e.session();
    let Some(tr) = s.track(t).filter(|tr| tr.kind.has_playlist() && !tr.kind.is_midi()) else { return Ok(false) };
    let start = r.start.max(0);
    let len = r.end.saturating_sub(start);
    if len <= 0 {
        return Ok(false);
    }
    let max = soundcraft_mix::MAX_RENDER_SAMPLES / tr.channels().max(1);
    if usize::try_from(len).map_or(true, |n| n > max) {
        let secs = max as f64 / s.sample_rate.as_f64();
        return Err(crate::cmd::bad(cmd, format!("selection is too long to consolidate on `{}` (at most {secs:.0} s)", tr.name)));
    }
    let r = Range::new(start, start + len);
    let audio = soundcraft_mix::render_clips(s, t, r);
    if audio.iter().any(|ch| ch.len() as i64 != len) {
        return Err(EngineError::Internal(cmd.to_string(), "consolidate rendered the wrong length".into()));
    }
    let name = format!("{}_consolidated", tr.name);
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &name, buf, None, FileFormat::Wav);
    crate::edit::clear_range(s, t, r, false);
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::audio(cid, name, src, 0, r.start, r.len()));
    Ok(true)
}

/// The most frames of `channels`-channel audio one offline render holds.
pub fn render_limit(channels: usize) -> usize {
    soundcraft_mix::MAX_RENDER_SAMPLES / channels.max(1)
}

/// Refuses `len` frames of `name` past `max`, where a render would cut the audio short.
pub fn refuse_longer(s: &Session, cmd: &str, name: &str, verb: &str, len: Samples, max: usize) -> Result<()> {
    if len <= Samples::try_from(max).unwrap_or(Samples::MAX) {
        return Ok(());
    }
    let secs = max as f64 / s.sample_rate.as_f64();
    Err(crate::cmd::bad(cmd, format!("`{name}` is too long to {verb} (at most {secs:.0} s)")))
}

/// Render clip gain into a new source and reset the clip's gain.
pub fn render_clip_gain(e: &mut Engine, cmd: &str, id: soundcraft_model::ClipId) -> Result<bool> {
    let s = e.session();
    let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { return Ok(false) };
    if !c.is_audio() {
        return Ok(false);
    }
    refuse_longer(s, cmd, &c.name, "render", c.length, render_limit(s.track(t).map_or(1, |x| x.channels())))?;
    let mut c2 = c.clone();
    c2.fade_in = Default::default();
    c2.fade_out = Default::default();
    let mut tmp = s.clone();
    if let Some(pl) = tmp.track_mut(t).and_then(|x| x.playlist_mut()) {
        pl.clips = vec![c2];
    }
    let audio = soundcraft_mix::render_clips(&tmp, t, c.range());
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &format!("{}_gain", c.name), buf, None, FileFormat::Wav);
    if let Some(cl) = s.find_clip_mut(id) {
        cl.content = ClipContent::Audio { source: src, offset: 0 };
        soundcraft_model::rebase_lyrics(cl, c.source_offset());
        cl.gain_db = 0.0;
        cl.gain_env.clear();
        // The new source is the clip as it already sounds, stretch included.
        // Keeping the old ratio would play that audio through the stretch again.
        cl.stretch = 1.0;
    }
    Ok(true)
}

/// Track › Freeze: renders the track `t`'s clips (to `end`; on a MIDI or instrument track, its
/// instrument playing them, with the release tail) through its module chain (down
/// to and including the module `upto`; the whole chain with `None`), pre-fader, plus the tail,
/// into a new source of the session (written to `Audio Files/` when the session is saved, as
/// Commit's are). Returns the source and its length in frames.
pub fn freeze_track(e: &mut Engine, t: TrackId, end: Samples, upto: Option<soundcraft_model::ModuleId>) -> Result<(SourceId, Samples)> {
    let s = e.session_arc();
    let Some(tr) = s.track(t) else { return Err(crate::cmd::bad("track.freeze", "no such track")) };
    check_freeze_length(&s, tr, end)?;
    // A host may have rendered it already, through its live instances (`RenderJob`).
    let kind = crate::render_job::RenderKind::Freeze { end, upto };
    let audio = match e.take_prepared(t, &kind) {
        Some(audio) => audio,
        None => soundcraft_mix::render_track_frozen(&s, t, end, upto),
    };
    let frames = audio.first().map_or(0, Vec::len);
    if frames == 0 {
        return Err(crate::cmd::bad("track.freeze", format!("`{}` has nothing audible to freeze", tr.name)));
    }
    let name = format!("{} Freeze", tr.name);
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let src = add_source(e.session_mut(), &name, buf, None, FileFormat::Wav);
    Ok((src, Samples::try_from(frames).unwrap_or(Samples::MAX)))
}

/// Refuses a freeze of `tr` to `end` that would cut the render short (see [`refuse_longer`]).
pub(crate) fn check_freeze_length(s: &Session, tr: &soundcraft_model::Track, end: Samples) -> Result<()> {
    let longest = end.saturating_add(s.sample_rate.samples(soundcraft_mix::FREEZE_TAIL_MAX_SECS));
    refuse_longer(s, "track.freeze", &tr.name, "freeze", longest, render_limit(tr.channels().max(2)))
}

/// What Track › Commit renders of `tr`: from the session start to its last clip plus two seconds
/// of tail. `None` when there is nothing to commit (not an audio path, the master, no clips).
/// Refuses a range that would cut the render short.
pub(crate) fn commit_range(s: &Session, tr: &soundcraft_model::Track) -> Result<Option<Range>> {
    if !tr.kind.is_audio_path() || tr.kind == TrackKind::Master {
        return Ok(None);
    }
    let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0);
    if end <= 0 {
        return Ok(None);
    }
    let range = Range::new(0, end + s.sample_rate.samples(2.0));
    // The track renders through a main mix of at least two channels.
    refuse_longer(s, "track.commit", &tr.name, "commit", range.len(), render_limit(tr.channels().max(2)))?;
    Ok(Some(range))
}

/// Thaws every frozen track of `s` whose render didn't load (or that names none, as sessions
/// from before Freeze rendered could): it plays live again. Returns what to tell the person.
pub fn thaw_missing_renders(s: &mut Session) -> Vec<String> {
    let mut notes = Vec::new();
    let pool = &s.pool;
    for t in &mut s.tracks {
        let loaded = t.freeze.is_some_and(|f| pool.get(f.source).is_some());
        // A freeze point that names no module of the chain: the render matches nothing live.
        let lost_point = t.freeze.and_then(|f| f.upto).is_some_and(|m| t.mixer.module_index(m).is_none());
        if (t.frozen || t.freeze.is_some()) && (!loaded || lost_point) {
            if lost_point && loaded {
                notes.push(format!("`{}`: the module it was frozen up to is missing, so the track was unfrozen and plays live", t.name));
            } else if t.freeze.is_some() {
                notes.push(format!("`{}`: its frozen audio is missing, so the track was unfrozen and plays live", t.name));
            }
            t.frozen = false;
            t.freeze = None;
        }
    }
    notes
}

/// Commit a track's inserts into a new audio track (Track › Commit).
pub fn commit_track(e: &mut Engine, t: TrackId) -> Result<Option<TrackId>> {
    let s = e.session_arc();
    let Some(tr) = s.track(t) else { return Ok(None) };
    let Some(range) = commit_range(&s, tr)? else { return Ok(None) };
    // A host may have rendered it already, through its live instances (`RenderJob`).
    let audio = match e.take_prepared(t, &crate::render_job::RenderKind::Commit { range }) {
        Some(audio) => audio,
        None => soundcraft_mix::render_track_pre_fader(&s, t, range),
    };
    let name = format!("{}.cm", tr.name);
    let fmt = ChannelFormat::for_channels(audio.len().clamp(1, 2));
    let mixer = tr.mixer.clone();
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &name, buf, None, FileFormat::Wav);
    let nt = s.add_track(TrackKind::Audio, fmt, Some(&name));
    if let (Some(idx), Some(new_idx)) = (s.track_index(t), s.track_index(nt)) {
        let tr = s.tracks.remove(new_idx);
        s.tracks.insert((idx + 1).min(s.tracks.len()), tr);
    }
    let cid = s.new_clip_id();
    if let Some(ntr) = s.track_mut(nt) {
        ntr.mixer.volume_db = mixer.volume_db;
        ntr.mixer.output = mixer.output.clone();
        ntr.mixer.sends = mixer.sends.clone();
        if let Some(pl) = ntr.playlist_mut() {
            pl.clips.push(Clip::audio(cid, name, src, 0, 0, range.len()));
        }
    }
    if let Some(old) = s.track_mut(t) {
        old.inactive = true;
        old.hidden = true;
    }
    Ok(Some(nt))
}

pub fn export_clips(e: &Engine, ids: &[soundcraft_model::ClipId], dir: &str) -> Result<usize> {
    let s = e.session();
    make_dir(dir)?;
    let mut n = 0;
    for id in ids {
        let Some((t, c)) = s.find_clip(*id) else { continue };
        let audio = soundcraft_mix::render_clips(s, t, c.range());
        let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).map_err(|err| EngineError::Io(err.to_string()))?;
        let stem = sanitize_name(&c.name);
        let mut path = Path::new(dir).join(format!("{stem}.wav"));
        let mut k = 2u32;
        while path.exists() {
            path = Path::new(dir).join(format!("{stem}-{k}.wav"));
            k = k.saturating_add(1);
            if k > 10_000 {
                return Err(EngineError::Io(format!("{dir}: too many files named {stem}")));
            }
        }
        write_path(&path, bytes)?;
        n += 1;
    }
    Ok(n)
}

pub fn sanitize_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "untitled".into() } else { s.chars().take(100).collect() }
}

/// Save the session to `path` (a `.fdawm` file, or the `.scraft` it was opened from). Audio that only exists in memory is written
/// as 32-bit float WAV into `Audio Files/` next to it. Returns how many audio files were written.
///
/// In the browser there are no folders: the session and all its audio go out as one bundle file
/// (`<name>.fdawmz`, see [`session_bundle`]) through the host's delivery hook ([`set_deliver`]).
pub fn save_session(e: &mut Engine, path: &str) -> Result<usize> {
    if cfg!(target_arch = "wasm32") {
        return save_session_bundle(e, path);
    }
    let (fresh, written, p) = write_session(e.session(), path, false, e.copy_imported_audio)?;
    // Keep the in-memory document in sync (paths, unsaved flags) without an undo step.
    *e.session_mut() = fresh;
    // What was just written (or was there already) now plays from its file.
    stream_saved(e, &p);
    e.path = Some(p.to_string_lossy().into_owned());
    e.mark_clean();
    // Generated files neither the saved session nor an undo or redo step uses are deleted.
    if let Some(dir) = p.parent() {
        let keep = e.history_sessions();
        let gone = crate::spill::clean_unused(dir, &keep, e.unsaved_audio_root.as_deref());
        if gone > 0 {
            log::info!("{}: removed {gone} generated audio files nothing uses", dir.display());
        }
    }
    Ok(written)
}

/// Every saved source still decoded in memory (imported or made before the first save) is
/// swapped for its file in the project, in the session and its undo history. Sampler samples and
/// short sources stay in memory.
fn stream_saved(e: &mut Engine, file: &Path) {
    let s = e.session();
    let rate = s.sample_rate.hz();
    let sampled: std::collections::HashSet<SourceId> =
        s.tracks.iter().flat_map(|t| t.mixer.inserts.iter().chain(t.instrument.as_ref())).flat_map(|i| i.sources()).collect();
    let mut swaps: Vec<(Arc<SourceAudio>, Arc<SourceAudio>)> = Vec::new();
    for src in s.sources.iter().filter(|src| !src.unsaved && !sampled.contains(&src.id)) {
        let Some(audio) = s.pool.get(src.id) else { continue };
        if audio.in_memory().is_none()
            || audio.frames().saturating_mul(audio.num_channels()) < crate::spill::MIN_SAMPLES
            || swaps.iter().any(|(old, _)| Arc::ptr_eq(old, audio))
        {
            continue;
        }
        let Ok(p) = stored_path(&src.path, file) else { continue };
        let Some(f) = streamable(&p, rate) else { continue };
        if f.channels() != audio.num_channels() || f.frames() != audio.frames() {
            continue;
        }
        if let Ok(streamed) = SourceAudio::from_disk(f, audio.peaks.clone()) {
            swaps.push((Arc::clone(audio), Arc::new(streamed)));
        }
    }
    e.swap_audio(&swaps);
}

/// Write a copy of the session (and any audio it needs) without touching the open document —
/// used for autosave / crash recovery. Audio already written for a source is reused.
pub fn save_copy(e: &Engine, path: &str) -> Result<usize> {
    write_session(e.session(), path, true, true).map(|(_, n, _)| n)
}

/// File › Save Session Copy In: a normal session folder that does not rename, retarget or
/// clean the session that stays open.
pub fn save_session_copy(e: &Engine, path: &str) -> Result<usize> {
    write_session(e.session(), path, false, e.copy_imported_audio).map(|(_, n, _)| n)
}

/// Write `session` to `path`; returns the session with updated media paths, the number of audio
/// files written and the final path. Audio the session refers to outside the project (an
/// imported file, a file of the folder it was saved in before) is copied into `Audio Files`, so
/// the project folder holds everything it needs, unless `copy_refs` is off: then a source naming
/// an existing file by its absolute path, with nothing unsaved, stays a reference.
fn write_session(session: &Session, path: &str, overwrite: bool, copy_refs: bool) -> Result<(Session, usize, PathBuf)> {
    use std::collections::{HashMap, HashSet};
    let p = session_file(path);
    // Before any audio is written: a session saved outside the write root changes nothing.
    crate::file_access::check_write(&p).map_err(EngineError::Io)?;
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let audio_dir = dir.join("Audio Files");
    let mut s = session.clone();
    let from = s.folder.clone();
    let same_dir = from.as_deref().is_some_and(|f| same_path(f, &dir));
    let mut written = 0;
    // A file can stay put for all the sources that name it, unless their audio differs.
    let shared_paths: Vec<bool> = {
        let mut audio_by_path: HashMap<&str, HashSet<Option<*const SourceAudio>>> = HashMap::new();
        for src in &s.sources {
            audio_by_path.entry(&src.path).or_default().insert(s.pool.get(src.id).map(Arc::as_ptr));
        }
        s.sources.iter().map(|src| audio_by_path.get(src.path.as_str()).is_some_and(|a| a.len() > 1)).collect()
    };
    // Files written for audio during this save: sources playing the same audio share one.
    let mut files: HashMap<*const SourceAudio, String> = HashMap::new();
    for (i, src) in s.sources.iter_mut().enumerate() {
        if !same_dir
            && s.pool.get(src.id).is_none()
            && Path::new(&src.path).is_relative()
            && let Some(f) = &from
        {
            src.path = f.join(&src.path).to_string_lossy().into_owned();
        }
        if !copy_refs && !src.unsaved && Path::new(&src.path).is_absolute() && Path::new(&src.path).is_file() {
            continue;
        }
        let exists_rel = same_dir && !Path::new(&src.path).is_absolute() && dir.join(&src.path).exists();
        let shared = shared_paths.get(i).copied().unwrap_or(false);
        if exists_rel && !src.unsaved && !shared {
            continue;
        }
        let in_folder = Path::new(&src.path).strip_prefix(&audio_dir).ok().and_then(Path::to_str).map(str::to_string);
        if let Some(f) = in_folder.filter(|_| !src.unsaved && Path::new(&src.path).is_file()) {
            src.path = format!("Audio Files/{f}");
            continue;
        }
        let Some(audio) = s.pool.get(src.id) else { continue };
        if let Some(path) = files.get(&Arc::as_ptr(audio)) {
            src.path.clone_from(path);
            src.unsaved = false;
            src.format = FileFormat::Wav;
            continue;
        }
        // Generated audio keeps its subfolder (`Audio Files/Stems/…`).
        let (into, rel) = match crate::spill::subfolder_of(&src.path) {
            Some(sub) => (audio_dir.join(sub), format!("Audio Files/{sub}/")),
            None => (audio_dir.clone(), "Audio Files/".to_string()),
        };
        make_dir(&into)?;
        let mut stem = sanitize_name(&src.name);
        if overwrite {
            // One file per source id so autosaves never collide or pile up.
            stem = format!("{stem}_{}", src.id.0);
            if into.join(format!("{stem}.wav")).exists() {
                src.path = format!("{rel}{stem}.wav");
                src.unsaved = false;
                files.insert(Arc::as_ptr(audio), src.path.clone());
                continue;
            }
        }
        // A float WAV played from disk is copied as it is (never read whole into memory).
        let copied = audio.disk_file().filter(|f| f.is_float32()).and_then(|f| copy_wav_new(f.path(), &into, &stem));
        let (name, new) = match copied {
            Some(name) => (name, true),
            None => {
                let opts = EncodeOptions {
                    format: FileFormat::Wav,
                    bit_depth: soundcraft_audio_io::BitDepth::Float32,
                    dither: false,
                    bwf: None,
                    bitrate_kbps: 0,
                };
                let bytes = soundcraft_audio_io::encode(&audio.buffer(), &opts).map_err(|err| EngineError::Io(err.to_string()))?;
                store_wav(&into, &stem, &bytes)?
            }
        };
        src.path = format!("{rel}{name}");
        src.unsaved = false;
        src.format = FileFormat::Wav;
        files.insert(Arc::as_ptr(audio), src.path.clone());
        written += usize::from(new);
    }
    if written > 0 {
        sync_dir(&audio_dir);
    }
    // Files the session's modules play from (an Amp cabinet IR, LevelUp's reference tracks):
    // copied into `Audio Files` with the audio when they live outside the project.
    if copy_refs && !overwrite {
        copy_module_files(&mut s, &dir, &audio_dir, from.as_deref(), same_dir);
    }
    // The waveforms of the project's audio, for a fast reopen (`peak_cache`); not for autosaves.
    if !overwrite {
        let rate = s.sample_rate.hz();
        let mut done = std::collections::HashSet::new();
        for src in &s.sources {
            let Some(audio) = s.pool.get(src.id) else { continue };
            if Path::new(&src.path).is_absolute() || !done.insert(src.path.clone()) {
                continue;
            }
            let file = dir.join(&src.path);
            if !crate::peak_cache::fresh(&dir, &file, rate, audio.peaks.len()) {
                crate::peak_cache::store(&dir, &file, rate, &audio.peaks);
            }
        }
    }
    s.folder = Some(dir.clone());
    if !overwrite && let Some(stem) = p.file_stem().and_then(|x| x.to_str()) {
        s.name = stem.to_string();
    }
    let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
    make_dir(&dir)?;
    // Atomic-ish write: temp file then rename.
    let tmp = p.with_extension(SESSION_TMP_EXTENSION);
    if !overwrite && read_file(&p).is_ok_and(|old| old != text.as_bytes()) {
        back_up(&p);
    }
    replace_file(&tmp, &p, text.as_bytes())?;
    Ok((s, written, p))
}

/// The module asset files of `s` (not bundled ones) that live outside the project folder `dir`
/// are copied into `audio_dir` and named from there (`Audio Files/<name>`); a reference relative
/// to the folder the session was saved in before (`from`) is followed there first. The prepared
/// assets are kept under the new names, so nothing reloads. A file that can't be copied stays
/// referred to where it is.
fn copy_module_files(s: &mut Session, dir: &Path, audio_dir: &Path, from: Option<&Path>, same_dir: bool) {
    let rate = s.sample_rate.hz();
    let mut renamed: Vec<(String, String, String)> = Vec::new();
    let mut copied: std::collections::HashMap<PathBuf, String> = std::collections::HashMap::new();
    for t in &mut s.tracks {
        for ins in &mut t.mixer.inserts {
            for (key, r) in &mut ins.assets {
                if r.starts_with(crate::cmd::module_asset::BUNDLED) {
                    continue;
                }
                let file = if Path::new(r.as_str()).is_absolute() {
                    PathBuf::from(r.as_str())
                } else if same_dir {
                    continue;
                } else if let Some(f) = from {
                    f.join(r.as_str())
                } else {
                    continue;
                };
                if file.starts_with(dir) || crate::file_access::check_read(&file).is_err() || !file.is_file() {
                    continue;
                }
                let stored = match copied.get(&file) {
                    Some(n) => n.clone(),
                    None => match copy_in(audio_dir, &file) {
                        Ok(name) => {
                            let rel = format!("Audio Files/{name}");
                            copied.insert(file.clone(), rel.clone());
                            rel
                        }
                        Err(err) => {
                            log::warn!("{}: not copied into the project: {err}", file.display());
                            continue;
                        }
                    },
                };
                renamed.push((key.clone(), r.clone(), stored.clone()));
                *r = stored;
            }
        }
    }
    for (key, old, new) in renamed {
        if let Some(data) = s.assets.get_for(&key, &old, rate).cloned() {
            s.assets.insert_for(&key, &new, rate, data);
        }
    }
}

/// Copies `file` into `dir` as the first of `<name>`, `<stem>_01.<ext>`, … that is free or already
/// holds the same bytes; returns the name.
fn copy_in(dir: &Path, file: &Path) -> std::io::Result<String> {
    let stem = sanitize_name(&file.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default());
    let ext = file.extension().map(|x| format!(".{}", x.to_string_lossy())).unwrap_or_default();
    crate::file_access::check_write(dir).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    let len = std::fs::metadata(file)?.len();
    for k in 0..10_000u32 {
        let name = if k == 0 { format!("{stem}{ext}") } else { format!("{stem}_{k:02}{ext}") };
        let to = dir.join(&name);
        match copy_new(file, &to) {
            Ok(()) => return Ok(name),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::metadata(&to).is_ok_and(|m| m.len() == len) && std::fs::read(&to).ok() == std::fs::read(file).ok() {
                    return Ok(name);
                }
            }
            Err(err) => return Err(err),
        }
    }
    Err(std::io::Error::other("no free file name"))
}

/// Write `bytes` to `tmp`, flush it to disk and rename it over `file`, so `file` holds either the
/// old or the new contents, never a mix.
pub fn replace_file(tmp: &Path, file: &Path, bytes: &[u8]) -> Result<()> {
    crate::file_access::check_write(file).map_err(EngineError::Io)?;
    crate::file_access::check_write(tmp).map_err(EngineError::Io)?;
    replace_with(tmp, file, write_synced(tmp, bytes))
}

/// Renames `tmp` over `file` once it is `written`, or removes it when writing or renaming failed.
/// The caller has checked both paths ([`crate::file_access::check_write`]).
fn replace_with(tmp: &Path, file: &Path, written: std::io::Result<()>) -> Result<()> {
    if let Err(err) = written.and_then(|()| std::fs::rename(tmp, file)) {
        let _ = std::fs::remove_file(tmp);
        return Err(EngineError::Io(format!("{}: {err}", file.display())));
    }
    sync_dir(file.parent().unwrap_or(Path::new("")));
    Ok(())
}

fn sync_dir(dir: &Path) {
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Copies `from` to `to`, which must not exist yet; an existing entry, even a link, is never followed.
/// Copies the WAV at `from` into `dir` as `<stem>.wav` (or `<stem>_NN.wav`, a name not taken);
/// returns the name.
fn copy_wav_new(from: &Path, dir: &Path, stem: &str) -> Option<String> {
    crate::file_access::check_read(from).ok()?;
    crate::file_access::check_write(dir).ok()?;
    (0..10_000u32).find_map(|k| {
        let name = if k == 0 { format!("{stem}.wav") } else { format!("{stem}_{k:02}.wav") };
        let to = dir.join(&name);
        (!to.exists() && copy_new(from, &to).is_ok()).then_some(name)
    })
}

fn copy_new(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut src = std::fs::File::open(from)?;
    let mut dst = std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    if let Err(err) = std::io::copy(&mut src, &mut dst) {
        drop(dst);
        let _ = std::fs::remove_file(to);
        return Err(err);
    }
    Ok(())
}

/// Copy the session file about to be replaced to `Backups/<name> backup <n>.<ext>`, keeping the
/// newest ten.
fn back_up(file: &Path) {
    let (Some(dir), Some(stem), Some(ext)) = (file.parent(), file.file_stem().and_then(|x| x.to_str()), file.extension().and_then(|x| x.to_str()))
    else {
        return;
    };
    let backups = dir.join("Backups");
    if let Err(err) = crate::file_access::check_write(&backups) {
        log::warn!("no backup of {}: {err}", file.display());
        return;
    }
    let prefix = format!("{stem} backup ");
    let suffix = format!(".{ext}");
    let numbers: Vec<u32> = std::fs::read_dir(&backups)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|f| f.file_name().to_str()?.strip_prefix(&prefix)?.strip_suffix(&suffix)?.parse().ok())
        .collect();
    let Some(next) = numbers.iter().max().map_or(Some(1), |n| n.checked_add(1)) else {
        log::warn!("no backup of {}: backup numbers are exhausted", file.display());
        return;
    };
    let dest = backups.join(format!("{prefix}{next}{suffix}"));
    if let Err(err) = crate::file_access::check_write(&dest) {
        log::warn!("no backup of {}: {err}", file.display());
        return;
    }
    if let Err(err) = std::fs::create_dir_all(&backups).and_then(|()| copy_new(file, &dest)) {
        log::warn!("no backup of {}: {err}", file.display());
        return;
    }
    for n in numbers.into_iter().filter(|n| n.saturating_add(10) <= next) {
        let _ = std::fs::remove_file(backups.join(format!("{prefix}{n}{suffix}")));
    }
}

/// Extension of the temporary file a session or template is written to before it is renamed over
/// the real one.
pub const SESSION_TMP_EXTENSION: &str = "fdawm.tmp";

/// The file a session saved to `path` goes to: `path`, with `.fdawm` added when it has no
/// extension and isn't an existing file. A path that already has an extension (`.scraft`
/// included) is kept as it is.
pub fn session_file(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    // Only a path the write root allows is probed, so a refused one can't be used to test for files.
    let is_file = crate::file_access::check_write(&p).is_ok() && p.is_file();
    if p.extension().is_none() && !is_file { p.with_extension(soundcraft_model::SESSION_EXTENSION) } else { p }
}

/// True when `command` would write a session over the existing `file`. Save, Save As and Save
/// Copy In may write the open session's own file; other commands never ask.
pub fn replaces(e: &Engine, command: &str, file: &Path) -> bool {
    match command {
        "session.save" | "session.save_as" | "session.save_copy" => {
            file.exists() && !e.path.as_deref().is_some_and(|own| same_path(Path::new(own), file))
        }
        "session.save_template" | "file.export_selected_tracks_as_session" => file.exists(),
        _ => false,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let full = |p: &Path| if p.as_os_str().is_empty() { Path::new(".").canonicalize() } else { p.canonicalize() };
    a == b || matches!((full(a), full(b)), (Ok(x), Ok(y)) if x == y)
}

/// Store `bytes` as the first of `<stem>.wav`, `<stem>_01.wav`, … in `dir` that is free or already
/// holds exactly these bytes, so saving never replaces audio; returns the name and whether it was
/// written.
fn store_wav(dir: &Path, stem: &str, bytes: &[u8]) -> Result<(String, bool)> {
    use std::io::Write;
    for k in 0..10_000u32 {
        let name = if k == 0 { format!("{stem}.wav") } else { format!("{stem}_{k:02}.wav") };
        let path = dir.join(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(err) = f.write_all(bytes).and_then(|()| f.sync_all()) {
                    let _ = std::fs::remove_file(&path);
                    return Err(EngineError::Io(format!("{}: {err}", path.display())));
                }
                return Ok((name, true));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) && std::fs::read(&path).is_ok_and(|b| b == bytes) {
                    return Ok((name, false));
                }
            }
            Err(err) => return Err(EngineError::Io(format!("{}: {err}", path.display()))),
        }
    }
    Err(EngineError::Io(format!("{}: no free file name for {stem}", dir.display())))
}

/// Writes `buf` as a new float WAV `<stem>.wav` (or `<stem>_NN.wav`) in `dir`, synced to disk.
pub(crate) fn write_wav_new(dir: &Path, stem: &str, buf: &soundcraft_audio_io::AudioBuffer) -> Option<(PathBuf, String)> {
    use std::io::Write;
    crate::file_access::check_write(dir).ok()?;
    std::fs::create_dir_all(dir).ok()?;
    let opts = soundcraft_audio_io::EncodeOptions {
        format: soundcraft_audio_io::FileFormat::Wav,
        bit_depth: soundcraft_audio_io::BitDepth::Float32,
        dither: false,
        bwf: None,
        bitrate_kbps: 0,
    };
    for k in 0..10_000u32 {
        let name = if k == 0 { format!("{stem}.wav") } else { format!("{stem}_{k:02}.wav") };
        let path = dir.join(&name);
        let Ok(f) = std::fs::OpenOptions::new().write(true).create_new(true).open(&path) else { continue };
        let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
        let ok = soundcraft_audio_io::FileEncoder::new(&mut w, &opts, buf.num_channels(), buf.sample_rate, None)
            .and_then(|mut enc| {
                for start in (0..buf.frames()).step_by(1 << 16) {
                    let n = (buf.frames() - start).min(1 << 16);
                    let block: Vec<Vec<f32>> = buf.channels.iter().map(|c| c.get(start..start + n).unwrap_or_default().to_vec()).collect();
                    enc.write(&block, n)?;
                }
                enc.finish().map(|_| ())
            })
            .is_ok()
            && w.flush().is_ok()
            && w.get_ref().sync_all().is_ok();
        if ok {
            return Some((path, name));
        }
        drop(w);
        let _ = std::fs::remove_file(&path);
        return None;
    }
    None
}

/// Open a `.fdawm` (or `.scraft`) session and decode its audio files (missing files are reported, not fatal).
pub fn open_session(e: &mut Engine, path: &str) -> Result<Vec<String>> {
    open_session_linked(e, path, Path::new(path))
}

/// [`open_session`], following the media paths in it as if they were stored in the session file
/// `links` (crash recovery: the session written beside the autosave, its paths from the original).
pub(crate) fn open_session_linked(e: &mut Engine, path: &str, links: &Path) -> Result<Vec<String>> {
    let (s, missing) = load_session_file(path, links, false)?;
    e.replace_session(s);
    e.path = Some(path.to_string());
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    Ok(missing)
}

/// Open the template at `path` as a new, untitled session named `name` (`session.new
/// {template: path}`): its media is found as the template stored it, but the session has no file
/// and no folder yet, so saving asks where and never writes over the template.
pub fn open_template(e: &mut Engine, path: &str, name: &str) -> Result<Vec<String>> {
    // Paths stored relative to the template are made absolute: the session has no folder.
    let (mut s, missing) = load_session_file(path, Path::new(path), true)?;
    s.name = name.to_string();
    s.folder = None;
    e.replace_session(s);
    e.path = None;
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    Ok(missing)
}

/// Read the session file at `path` and its media (paths followed as if stored in `links`); with
/// `absolute`, the stored paths are first made absolute (a template opened with no folder).
fn load_session_file(path: &str, links: &Path, absolute: bool) -> Result<(Session, Vec<String>)> {
    load_session_file_reporting(path, links, absolute, &mut |_| {})
}

/// [`load_session_file`], reporting each step to `progress` (see [`crate::open_job`]): the
/// session's audio files are decoded first ("Loading audio", one report per file, named), then
/// their waveforms are built ("Waveforms", one report per file). A file named by many sources is
/// read, decoded and overviewed once.
pub(crate) fn load_session_file_reporting(
    path: &str,
    links: &Path,
    absolute: bool,
    progress: &mut dyn FnMut(crate::open_job::OpenProgress),
) -> Result<(Session, Vec<String>)> {
    use crate::open_job::{OpenPhase, OpenProgress};
    let text = read_text(path)?;
    let mut s = Session::from_json(&text).map_err(|err| EngineError::Io(err.to_string()))?;
    if absolute {
        crate::cmd::module_asset::absolutize_paths(&mut s, links);
    }
    let dir = Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut missing = Vec::new();
    let rate = s.sample_rate.hz();
    // The files, each once, in the order the sources first name them.
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut by_source: Vec<std::result::Result<usize, String>> = Vec::with_capacity(s.sources.len());
    for src in &s.sources {
        let resolved = stored_path(&src.path, links).map_err(|why| format!("{}: {why}", src.path)).and_then(|p| {
            crate::file_access::check_read(&p)?;
            Ok(p)
        });
        by_source.push(resolved.map(|p| {
            let key = file_key(&p);
            match files.iter().position(|(k, _)| *k == key) {
                Some(i) => i,
                None => {
                    files.push((key, p.display().to_string()));
                    files.len() - 1
                }
            }
        }));
    }
    let total = files.len();
    // Files a sample-playing instrument plays are decoded into memory (it reads them whole);
    // every other native WAV at the session rate plays from disk: only its header is read now.
    let sampled: std::collections::HashSet<usize> = s
        .tracks
        .iter()
        .flat_map(|t| t.mixer.inserts.iter().chain(t.instrument.as_ref()))
        .flat_map(|i| i.sources())
        .filter_map(|id| s.sources.iter().position(|src| src.id == id))
        .filter_map(|k| by_source.get(k).and_then(|r| r.as_ref().ok()).copied())
        .collect();
    let mut decoded: Vec<std::result::Result<Loaded, String>> = Vec::with_capacity(total);
    for (i, (_, shown)) in files.iter().enumerate() {
        progress(OpenProgress::new(OpenPhase::Audio, i, total, file_name_of(shown)));
        if !sampled.contains(&i)
            && let Some(f) = streamable(Path::new(shown), rate)
        {
            decoded.push(Ok(Loaded::Disk(f)));
            continue;
        }
        decoded.push(read_file(shown).map_err(|_| shown.clone()).and_then(|bytes| {
            decode_buffer(&bytes, Path::new(shown).extension().and_then(|x| x.to_str()), rate)
                .map(Loaded::Memory)
                .map_err(|err| format!("{shown}: {err}"))
        }));
    }
    let mut audio: Vec<std::result::Result<Arc<SourceAudio>, String>> = Vec::with_capacity(total);
    // The waveforms come from the project's cache when it has them (`peak_cache`); a template
    // opened as a new session writes none into its folder.
    for (i, (buf, (_, shown))) in decoded.into_iter().zip(&files).enumerate() {
        progress(OpenProgress::new(OpenPhase::Waveforms, i, total, file_name_of(shown)));
        let file = Path::new(shown);
        let buf = match buf {
            Ok(Loaded::Disk(f)) => {
                audio.push(Ok(streamed_audio(f, (!absolute).then_some(dir.as_path()), file, rate)));
                continue;
            }
            Ok(Loaded::Memory(b)) => Ok(b),
            Err(e) => Err(e),
        };
        audio.push(buf.map(|b| {
            let cached = (!absolute).then(|| crate::peak_cache::load(&dir, file, rate, &b)).flatten();
            let made = match cached {
                Some(peaks) => SourceAudio::with_peaks(b, peaks),
                None => Err(b),
            };
            Arc::new(made.unwrap_or_else(|b| {
                let a = SourceAudio::new(b);
                if !absolute {
                    crate::peak_cache::store(&dir, file, rate, &a.peaks);
                }
                a
            }))
        }));
    }
    if total > 0 {
        progress(OpenProgress::new(OpenPhase::Waveforms, total, total, ""));
    }
    for (src, at) in s.sources.iter().zip(by_source) {
        match at.and_then(|i| audio.get(i).cloned().unwrap_or_else(|| Err(src.path.clone()))) {
            Ok(a) => s.pool.insert(src.id, a),
            Err(err) => missing.push(err),
        }
    }
    missing.extend(relink_videos(&mut s, links));
    missing.extend(crate::cmd::module_asset::load_session_assets(&mut s, Some(links)));
    missing.extend(s.clean_missing_note());
    s.folder = Some(dir);
    Ok((s, missing))
}

/// An audio file as the open reads it: played from disk, or decoded into memory.
enum Loaded {
    Disk(Arc<soundcraft_model::disk::DiskFile>),
    Memory(AudioBuffer),
}

/// The file at `p` opened to play from disk, when it can at `rate`: a native PCM/float WAV at
/// that rate (others are decoded into memory as before). `FREEDAWM_STREAM=0` turns it off.
pub(crate) fn streamable(p: &Path, rate: u32) -> Option<Arc<soundcraft_model::disk::DiskFile>> {
    if !soundcraft_model::disk::streaming_enabled() {
        return None;
    }
    crate::file_access::check_read(p).ok()?;
    soundcraft_model::disk::DiskFile::open(p).ok().filter(|f| f.sample_rate() == rate)
}

/// `f` as a source's audio, its waveform from the project's cache in `project` when it has one
/// for the file as it is, else made by reading the file through once (and kept in the cache).
pub(crate) fn streamed_audio(f: Arc<soundcraft_model::disk::DiskFile>, project: Option<&Path>, file: &Path, rate: u32) -> Arc<SourceAudio> {
    let cached = project.and_then(|d| crate::peak_cache::load_sized(d, file, rate, f.channels(), f.frames()));
    let made = match cached {
        Some(peaks) => SourceAudio::from_disk(f, peaks),
        None => Err(f),
    };
    Arc::new(made.unwrap_or_else(|f| {
        let a = SourceAudio::from_disk_scanned(f);
        if let Some(d) = project {
            crate::peak_cache::store(d, file, rate, &a.peaks);
        }
        a
    }))
}

/// The file name a progress line shows for a path.
fn file_name_of(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Decode one audio file's bytes at the session rate (no waveforms yet).
fn decode_buffer(bytes: &[u8], ext: Option<&str>, rate: u32) -> std::result::Result<AudioBuffer, String> {
    let (_, buf) = soundcraft_audio_io::decode(bytes, ext).map_err(|err| err.to_string())?;
    at_rate(buf, rate, MAX_IMPORT_SAMPLES).ok_or_else(|| "too large to load".to_string())
}

/// Decode one source's audio file at the session rate.
fn decode_source(bytes: &[u8], ext: Option<&str>, rate: u32) -> std::result::Result<Arc<SourceAudio>, String> {
    decode_buffer(bytes, ext, rate).map(|buf| Arc::new(SourceAudio::new(buf)))
}

/// Audio files decoded so far, by path: a session may name one file thousands of times.
/// The one name of the file at `p` (links followed), so the same file named two ways is read once.
pub(crate) fn file_key(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

pub(crate) type DecodedFiles = std::collections::HashMap<PathBuf, std::result::Result<Arc<SourceAudio>, String>>;

/// The audio of the file at `p` at `rate`, decoded the first time it is asked for. The error
/// names the file.
/// A file named in a session is hostile input too: a path that leaves the read root is not read.
pub(crate) fn decode_file(files: &mut DecodedFiles, p: &Path, rate: u32) -> std::result::Result<Arc<SourceAudio>, String> {
    crate::file_access::check_read(p)?;
    files
        .entry(file_key(p))
        .or_insert_with(|| {
            let bytes = read_file(p).map_err(|_| p.display().to_string())?;
            decode_source(&bytes, p.extension().and_then(|x| x.to_str()), rate).map_err(|err| format!("{}: {err}", p.display()))
        })
        .clone()
}

// ─── Hosts without a filesystem (the browser) ─────────────────────────────────────────────────

/// Extension of a session bundle: one zip file holding the `.fdawm` session and its
/// `Audio Files/`, the same layout as a saved session folder. The browser has no folders, so it
/// saves and opens sessions as bundles.
pub const BUNDLE_EXTENSION: &str = "fdawmz";
/// The bundle extension before the FreeDAWm rename: such bundles still open (holding a
/// `.scraft`), and one opened keeps its name.
pub const LEGACY_BUNDLE_EXTENSION: &str = "scraftz";
/// Every extension a session bundle may have, the current one first.
pub const BUNDLE_EXTENSIONS: [&str; 2] = [BUNDLE_EXTENSION, LEGACY_BUNDLE_EXTENSION];

/// Most entries and bytes a bundle may unpack to (bundles are hostile input).
const MAX_BUNDLE_ENTRIES: usize = 10_000;
const MAX_BUNDLE_BYTES: u64 = 1 << 30;

/// Hands a finished file (name, bytes) to the person on a host without a filesystem: the web app
/// installs one that starts a download. Native builds write files and never install it.
pub type DeliverHook = fn(&str, &[u8]) -> std::result::Result<(), String>;

static DELIVER: std::sync::OnceLock<DeliverHook> = std::sync::OnceLock::new();

/// Install the delivery hook (once, at start-up of the web app).
pub fn set_deliver(h: DeliverHook) {
    let _ = DELIVER.set(h);
}

/// Write one output file: to `path` natively; in the browser, delivered under its file name.
/// Without this, every export on the web failed with «operation not supported on this platform».
pub fn write_file(path: &str, bytes: &[u8]) -> Result<()> {
    if cfg!(target_arch = "wasm32") {
        let deliver = DELIVER.get().ok_or_else(|| EngineError::Io("saving files is not available in this build".into()))?;
        let name = Path::new(path).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("Untitled");
        return deliver(name, bytes).map_err(EngineError::Io);
    }
    crate::file_access::check_write(path).map_err(EngineError::Io)?;
    std::fs::write(path, bytes).map_err(|err| EngineError::Io(format!("{path}: {err}")))
}

/// The session as a bundle: `<name>.fdawm` plus every source's audio as 32-bit float WAV under
/// `Audio Files/` (stored, not compressed: audio barely compresses). Returns the bytes and the
/// session with its source paths pointing into the bundle.
pub fn session_bundle(session: &Session) -> Result<(Vec<u8>, Session)> {
    use std::io::Write as _;
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("bundle: {err}"));
    let ioerr = |err: std::io::Error| EngineError::Io(format!("bundle: {err}"));
    let mut s = session.clone();
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let mut taken = std::collections::HashSet::new();
    for src in &mut s.sources {
        let Some(audio) = s.pool.get(src.id) else { continue };
        let base = sanitize_name(&src.name);
        let mut target = format!("Audio Files/{base}.wav");
        let mut k = 1;
        while taken.contains(&target) && k <= 9_999 {
            target = format!("Audio Files/{base}_{k:02}.wav");
            k += 1;
        }
        let opts_wav =
            EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
        let bytes = soundcraft_audio_io::encode(&audio.buffer(), &opts_wav).map_err(|err| EngineError::Io(err.to_string()))?;
        w.start_file(target.as_str(), opts).map_err(zerr)?;
        w.write_all(&bytes).map_err(ioerr)?;
        taken.insert(target.clone());
        src.path = target;
        src.unsaved = false;
        src.format = FileFormat::Wav;
    }
    let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
    w.start_file(format!("{}.{}", sanitize_name(&s.name), soundcraft_model::SESSION_EXTENSION), opts).map_err(zerr)?;
    w.write_all(text.as_bytes()).map_err(ioerr)?;
    let bytes = w.finish().map_err(zerr)?.into_inner();
    Ok((bytes, s))
}

/// The bundle extension a session saved as `name` keeps: `.scraftz` for a bundle opened under the
/// old name (so it saves back to it), else `.fdawmz`.
pub fn bundle_extension_for(name: &str) -> &'static str {
    let legacy = Path::new(name).extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case(LEGACY_BUNDLE_EXTENSION));
    if legacy { LEGACY_BUNDLE_EXTENSION } else { BUNDLE_EXTENSION }
}

/// Save as a bundle named after `path`'s file name, delivered through the host ([`write_file`]).
fn save_session_bundle(e: &mut Engine, path: &str) -> Result<usize> {
    let stem = Path::new(path).file_stem().and_then(|x| x.to_str()).filter(|x| !x.is_empty()).map(str::to_string);
    let mut s = e.session().clone();
    if let Some(stem) = stem {
        s.name = stem;
    }
    let (bytes, fresh) = session_bundle(&s)?;
    let file = format!("{}.{}", sanitize_name(&fresh.name), bundle_extension_for(path));
    write_file(&file, &bytes)?;
    let written = fresh.sources.iter().filter(|x| fresh.pool.get(x.id).is_some()).count();
    *e.session_mut() = fresh;
    e.path = Some(file);
    e.mark_clean();
    Ok(written)
}

/// Open a session from bytes (the browser has no paths): a bundle ([`session_bundle`]) or a bare
/// `.fdawm` (or `.scraft`), whose audio is then reported missing. A `.scraftz` bundle keeps its
/// extension, so it saves back under the same name. Returns the missing media, like
/// [`open_session`].
pub fn open_session_bytes(e: &mut Engine, name: &str, bytes: &[u8]) -> Result<Vec<String>> {
    let (text, mut files) = if bytes.starts_with(b"PK\x03\x04") {
        read_bundle(bytes)?
    } else {
        (String::from_utf8(bytes.to_vec()).map_err(|_| EngineError::Io(format!("{name}: not a FreeDAWm session")))?, std::collections::HashMap::new())
    };
    let mut s = Session::from_json(&text).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    let mut missing = Vec::new();
    // Several sources may share one file (native sessions do): decode each path once and share the result.
    let mut decoded: std::collections::HashMap<String, Option<Arc<SourceAudio>>> = std::collections::HashMap::new();
    for src in &s.sources {
        if let Some(audio) = decoded.get(&src.path) {
            if let Some(audio) = audio {
                s.pool.insert(src.id, Arc::clone(audio));
            }
            continue;
        }
        let audio = match files.remove(&src.path) {
            Some(bytes) => match decode_source(&bytes, Path::new(&src.path).extension().and_then(|x| x.to_str()), s.sample_rate.hz()) {
                Ok(audio) => Some(audio),
                Err(err) => {
                    missing.push(format!("{}: {err}", src.path));
                    None
                }
            },
            None => {
                missing.push(src.path.clone());
                None
            }
        };
        if let Some(audio) = &audio {
            s.pool.insert(src.id, Arc::clone(audio));
        }
        decoded.insert(src.path.clone(), audio);
    }
    // Movies are referenced by path, never copied into a session: none can be found here.
    missing.extend(s.videos.iter().map(|v| v.path.clone()));
    // Bundled IRs load; a file named by path cannot be read here.
    missing.extend(crate::cmd::module_asset::load_session_assets(&mut s, None));
    missing.extend(s.clean_missing_note());
    let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).filter(|x| !x.is_empty()).unwrap_or("Untitled").to_string();
    e.replace_session(s);
    e.path = Some(format!("{}.{}", sanitize_name(&stem), bundle_extension_for(name)));
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    Ok(missing)
}

/// Unpack a bundle: the one session (`.fdawm`, or `.scraft` in an older bundle) at its top level
/// and the files beside it, by relative path.
fn read_bundle(bytes: &[u8]) -> Result<(String, std::collections::HashMap<String, Vec<u8>>)> {
    use std::io::Read as _;
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("bundle: {err}"));
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(zerr)?;
    if z.len() > MAX_BUNDLE_ENTRIES {
        return Err(EngineError::Io("bundle: too many files".into()));
    }
    let (mut text, mut files, mut total) = (None, std::collections::HashMap::new(), 0u64);
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(zerr)?;
        if f.is_dir() {
            continue;
        }
        let rel = f.name().map_err(zerr)?.to_string();
        let size = f.size();
        total = total.saturating_add(size);
        if total > MAX_BUNDLE_BYTES {
            return Err(EngineError::Io("bundle: too large".into()));
        }
        let mut data = Vec::new();
        // The declared size is hostile too: never read past it.
        f.by_ref().take(size).read_to_end(&mut data).map_err(|err| EngineError::Io(format!("bundle: {rel}: {err}")))?;
        let is_session = !rel.contains('/') && soundcraft_model::is_session_path(&rel);
        if is_session && text.is_none() {
            text = Some(String::from_utf8(data).map_err(|_| EngineError::Io(format!("bundle: {rel} is not text")))?);
        } else {
            files.insert(rel, data);
        }
    }
    let text = text.ok_or_else(|| EngineError::Io("bundle: no .fdawm session inside".into()))?;
    Ok((text, files))
}

/// The record-enabled audio tracks, each with the device input channels it records (resolved by
/// [`Session::input_channels`](soundcraft_model::Session::input_channels): a named input, or the
/// legacy "In 3" / "In 3-4"; a mono input feeds both sides of a stereo track) and a new take file
/// `<track>_<NN>.wav` in the `Audio Files` of the session's folder (`unsaved_dir` until it is saved).
/// An input that doesn't resolve, or needs a channel the device lacks (`input_channels`), is
/// refused by track and input name; it never wraps onto other channels. A track with no
/// hardware input records from channel 0 up, as before.
pub fn plan_takes(e: &Engine, unsaved_dir: &Path, input_channels: usize) -> std::result::Result<Vec<(TrackId, Vec<usize>, PathBuf)>, String> {
    let dir = e.path.as_deref().and_then(|p| Path::new(p).parent()).unwrap_or(unsaved_dir);
    let s = e.session();
    let take_no = s.sources.len() + 1;
    let mut plan = Vec::new();
    // A frozen track plays its render: a take on it would go unheard (and edit what was frozen).
    if let Some(t) = s.tracks.iter().find(|t| t.mixer.record_arm && t.frozen_audio().is_some()) {
        return Err(format!("`{}` is frozen. {}", t.name, crate::cmd::freeze::UNFREEZE_TO_RECORD));
    }
    for t in s.tracks.iter().filter(|t| t.mixer.record_arm && t.kind == TrackKind::Audio && !t.inactive) {
        let inputs: Vec<usize> = match &t.mixer.input {
            // Recorded on a remote recorder: its take lands here when it stops.
            soundcraft_model::Route::Hardware(name) if is_remote_input(name) => continue,
            soundcraft_model::Route::Hardware(name) => {
                let refuse = || format!("{}: input “{name}” is not available on this device ({input_channels} inputs)", t.name);
                let (first, width) = s.input_channels(name).ok_or_else(refuse)?;
                let chans: Vec<usize> = (0..t.channels()).map(|k| soundcraft_model::track_input_channel(k, first, width)).collect();
                if chans.iter().any(|c| *c >= input_channels) {
                    return Err(refuse());
                }
                chans
            }
            _ => (0..t.channels()).map(|k| k % input_channels.max(1)).collect(),
        };
        let file = |n: usize| dir.join("Audio Files").join(format!("{}_{n:02}.wav", sanitize_name(&t.name)));
        let path = (take_no..take_no + 1_000).map(file).find(|p| !p.exists()).unwrap_or_else(|| file(take_no));
        plan.push((t.id, inputs, path));
    }
    Ok(plan)
}

/// A remote recorder's input as a track input (`Remote: <PC> › <input>`, see `soundcraft-net`):
/// the track's takes are recorded on that PC and land here, so this PC's recorder skips it.
pub fn is_remote_input(name: &str) -> bool {
    name.starts_with("Remote: ")
}

/// Armed audio tracks recorded on a remote recorder ([`is_remote_input`]).
pub fn remote_armed(s: &soundcraft_model::Session) -> bool {
    s.tracks.iter().any(|t| {
        t.mixer.record_arm && t.kind == TrackKind::Audio && matches!(&t.mixer.input, soundcraft_model::Route::Hardware(n) if is_remote_input(n))
    })
}

/// A recording's plan, made when it starts: each record-enabled track's take file, where the
/// takes start on the timeline, the loop length they split into when loop recording, and the
/// punch range they're trimmed to.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Recording {
    pub start: Samples,
    pub pass: Option<Samples>,
    pub punch: Option<Range>,
    /// The input's sample rate, which the take files are written at.
    pub rate: u32,
    pub takes: Vec<Take>,
    /// The round trip in samples: take frame `k` belongs at timeline `start + k - latency`.
    /// Plans saved before it existed load as 0.
    #[serde(default)]
    pub latency: Samples,
}

/// One track's take file.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Take {
    pub track: TrackId,
    pub path: PathBuf,
    pub channels: usize,
}

/// Place recorded take files as `plan` says, as one undo step ("Record").
pub fn place_recording(e: &mut Engine, plan: &Recording, takes: Vec<(TrackId, PathBuf)>) -> Vec<soundcraft_model::ClipId> {
    add_recording_in(e, plan.start, takes, plan.pass, plan.punch, plan.latency)
}

/// Turn recorded take files into clips at `start`, as one undo step ("Record"); each file is the
/// source of its clips. With `pass` (loop recording), a take becomes one clip per pass (at most
/// 1000), each pass after the first on a new playlist.
pub fn add_recording(e: &mut Engine, start: Samples, takes: Vec<(TrackId, PathBuf)>, pass: Option<Samples>) -> Vec<soundcraft_model::ClipId> {
    add_recording_in(e, start, takes, pass, None, 0)
}

/// [`add_recording`] limited to the `punch` range (each clip is cut to it before it's placed, so
/// the material around the punch isn't overwritten) and shifted by the recording `latency`: take
/// frame `k` belongs at timeline `start + k - latency`, so the first `latency` frames are skipped
/// (a negative latency places the take later instead). Loop passes split on that shifted
/// timeline: pass `i` holds frames `i * pass + latency ..`.
fn add_recording_in(
    e: &mut Engine,
    start: Samples,
    takes: Vec<(TrackId, PathBuf)>,
    pass: Option<Samples>,
    punch: Option<Range>,
    latency: Samples,
) -> Vec<soundcraft_model::ClipId> {
    let before = e.session_arc();
    let mut out = Vec::new();
    for (tid, path) in takes {
        if e.session().track(tid).is_none_or(|t| t.frozen_audio().is_some()) {
            continue;
        }
        let name = path.file_stem().and_then(|x| x.to_str()).unwrap_or("Take");
        // A take at the session rate plays from its file (its waveform read from it once).
        let rate = e.session().sample_rate.hz();
        if let Some(f) = streamable(&path, rate) {
            let audio = streamed_audio(f, None, &path, rate);
            let s = e.session_mut();
            let src = insert_source(s, name, audio, Some(&path.to_string_lossy()), FileFormat::Wav);
            if let Some(x) = s.sources.last_mut() {
                x.unsaved = false;
            }
            place_take(s, tid, src, name, start, pass, punch, latency, &mut out);
            continue;
        }
        let read =
            read_file(&path).and_then(|b| soundcraft_audio_io::decode(&b, None).map_err(|err| EngineError::Io(format!("{}: {err}", path.display()))));
        let Ok((_, buf)) = read.inspect_err(|err| e.message(err.to_string())) else { continue };
        let s = e.session_mut();
        let resampled = buf.sample_rate != s.sample_rate.hz();
        let src = add_source(s, name, buf, Some(&path.to_string_lossy()), FileFormat::Wav);
        if let Some(x) = s.sources.last_mut() {
            x.unsaved = resampled;
        }
        place_take(s, tid, src, name, start, pass, punch, latency, &mut out);
    }
    if !out.is_empty() {
        e.push_undo("Record", before);
    }
    out
}

/// Places the clips of take `src` on track `tid` (see [`add_recording_in`]).
#[allow(clippy::too_many_arguments)]
fn place_take(
    s: &mut Session,
    tid: TrackId,
    src: SourceId,
    name: &str,
    start: Samples,
    pass: Option<Samples>,
    punch: Option<Range>,
    latency: Samples,
    out: &mut Vec<soundcraft_model::ClipId>,
) {
    {
        let Some(x) = s.sources.iter().find(|x| x.id == src) else { return };
        let len = i64::try_from(x.frames).unwrap_or(0);
        // Without loop recording the whole take is one pass.
        let step = pass.map_or(Samples::MAX, |p| p.max(1));
        for i in 0..1_000i64 {
            // Pass `i` starts at timeline `start` and holds take frames `origin ..`; the frames
            // before 0 (a negative latency) or past the take's end are not there.
            let origin = i.saturating_mul(step).saturating_add(latency);
            let from = origin.max(0);
            if from >= len {
                break;
            }
            let n = origin.saturating_add(step).min(len).saturating_sub(from);
            if n <= 0 {
                continue;
            }
            if i > 0 && n < step / 8 {
                break;
            }
            let at = start.saturating_add(from.saturating_sub(origin));
            let (clip_start, cut, clip_len) = match punch {
                Some(p) => {
                    let Some(kept) = Range::new(at, at.saturating_add(n)).intersect(&p) else { continue };
                    (kept.start, kept.start.saturating_sub(at), kept.len())
                }
                None => (at, 0, n),
            };
            // A comping track (layers open, a record target, or take layers): each pass goes on a
            // take layer and into the comp.
            if s.track(tid).is_some_and(crate::cmd::comp::comping) {
                let cid = s.new_clip_id();
                out.push(crate::cmd::comp::record_pass(s, tid, Clip::audio(cid, name, src, from.saturating_add(cut), clip_start, clip_len)));
                continue;
            }
            if i > 0
                && let Some(t) = s.track_mut(tid)
            {
                t.playlists.push(soundcraft_model::Playlist::new(format!("{}.{:02}", t.name, t.playlists.len() + 1)));
                t.active_playlist = t.playlists.len() - 1;
            }
            let cid = s.new_clip_id();
            crate::edit::place_clip(s, tid, Clip::audio(cid, name, src, from.saturating_add(cut), clip_start, clip_len));
            out.push(cid);
        }
    }
}

/// Where a take recording from `start` shows while it holds `len` samples, as it will be placed
/// (see [`add_recording_in`]): frame `k` at `start + k - latency`, just the pass under way when
/// loop recording in passes of `pass` (split on that shifted timeline), within the `punch`
/// range. Returns the timeline position of its first sample and the span it covers; `None` while
/// nothing of it shows yet.
pub fn live_span(start: Samples, pass: Option<Samples>, punch: Option<Range>, len: Samples, latency: Samples) -> Option<(Samples, Range)> {
    let placed = len.saturating_sub(latency).max(0);
    let done = pass.filter(|p| *p > 0).map_or(0, |p| placed / p * p);
    let origin = start.saturating_sub(latency).saturating_sub(done);
    let (from, end) = (start.max(origin), origin.saturating_add(len));
    if end <= from {
        return None;
    }
    let span = Range::new(from, end);
    Some((origin, span.intersect(&punch.unwrap_or(span))?))
}

#[cfg(test)]
mod record_tests {
    use super::*;
    use serde_json::json;

    fn armed(e: &mut Engine, name: &str, format: &str, input: &str) -> TrackId {
        let id = e.execute("track.new", &json!({"name": name, "format": format})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [id], "input": input})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
        TrackId(id)
    }

    fn write_take(path: &Path, frames: usize) {
        let mut bytes = soundcraft_audio_io::wav_float_header(1, 48_000, frames as u64).unwrap();
        bytes.extend((0..frames).flat_map(|i| (i as f32 / frames as f32 - 0.5).to_le_bytes()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn armed_tracks_get_new_take_files_used_in_place() {
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 2");
        let gtr = armed(&mut e, "Gtr", "stereo", "In 3-4");
        write_take(&dir.join("Audio Files/Vox_01.wav"), 10);
        let plan = plan_takes(&e, &dir, 4).unwrap();
        assert_eq!(plan, vec![(vox, vec![1], dir.join("Audio Files/Vox_02.wav")), (gtr, vec![2, 3], dir.join("Audio Files/Gtr_01.wav"))]);
        // A channel the device lacks is refused, never wrapped onto channels 1-2.
        assert!(plan_takes(&e, &dir, 2).is_err());
        // A track on a remote recorder's input is recorded there: skipped here.
        let remote = armed(&mut e, "Guitar", "mono", "Remote: Guitar PC › In 1");
        assert_eq!(e.session().track(remote).unwrap().mixer.input, soundcraft_model::Route::Hardware("Remote: Guitar PC › In 1".into()));
        assert!(remote_armed(e.session()));
        assert_eq!(plan_takes(&e, &dir, 4).unwrap().len(), 2);
        e.execute("mix.record_arm", &json!({"tracks": [remote.0], "value": false})).unwrap();

        let session = dir.join("Song.scraft").to_string_lossy().into_owned();
        e.execute("session.save_as", &json!({"path": session})).unwrap();
        write_take(&plan[0].2, 4_800);
        let clips = add_recording(&mut e, 96_000, vec![(vox, plan[0].2.clone())], None);
        let clip = e.session().find_clip(clips[0]).unwrap().1.clone();
        assert_eq!((clip.start, clip.length), (96_000, 4_800));
        assert_eq!(save_session(&mut e, &session).unwrap(), 0);
        let src = e.session().sources.last().unwrap().clone();
        assert_eq!((src.name.as_str(), src.path.as_str(), src.unsaved, src.frames), ("Vox_02", "Audio Files/Vox_02.wav", false, 4_800));
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(clips[0]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_loop_take_becomes_one_clip_per_pass_and_autosaves_in_place() {
        let dir = std::env::temp_dir().join(format!("soundcraft-loop-take-{}", std::process::id()));
        let path = dir.join("Audio Files/Vox_01.wav");
        write_take(&path, 1_050);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        assert_eq!(add_recording(&mut e, 1_000, vec![(vox, path.clone())], Some(400)).len(), 3);
        assert_eq!(save_copy(&e, &dir.join("Untitled.scraft").to_string_lossy()).unwrap(), 0);
        let t = e.session().track(vox).unwrap();
        let passes: Vec<_> = t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.source_offset(), c.start, c.length))).collect();
        assert_eq!(passes, vec![(0, 1_000, 400), (400, 1_000, 400), (800, 1_000, 250)]);
        assert_eq!((t.active_playlist, t.playlists[2].name.as_str()), (2, "Vox.03"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_comping_track_records_each_pass_on_a_take_layer() {
        let dir = test_dir("comp-takes");
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        let one = dir.join("Audio Files/Vox_01.wav");
        write_take(&one, 1_000);
        // A plain track keeps one playlist for a first take.
        add_recording(&mut e, 0, vec![(vox, one.clone())], None);
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 1);
        // New Layer on a track with audio: that audio becomes Take 1, the blank Take 2 is the
        // record target; the next take goes there and into the comp.
        e.execute("comp.layer_new", &json!({"track": vox.0})).unwrap();
        let t = e.session().track(vox).unwrap();
        assert_eq!((t.playlists.len(), t.record_layer), (3, Some(2)));
        let two = dir.join("Audio Files/Vox_02.wav");
        write_take(&two, 500);
        add_recording(&mut e, 200, vec![(vox, two)], None);
        let t = e.session().track(vox).unwrap();
        let names: Vec<_> = t.playlists.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Vox", "Take 1", "Take 2"]);
        assert_eq!(t.record_layer, None, "the target is used once");
        assert_eq!((t.playlists[1].clips.len(), t.playlists[2].clips.len()), (1, 1));
        let comp: Vec<_> = t.clips().iter().map(|c| (c.start, c.end(), c.from_layer)).collect();
        assert_eq!(comp, [(0, 200, Some(1)), (200, 700, Some(2)), (700, 1_000, Some(1))]);
        // Loop passes: a new layer each, the comp ends on the last.
        let three = dir.join("Audio Files/Vox_03.wav");
        write_take(&three, 1_050);
        add_recording(&mut e, 0, vec![(vox, three)], Some(400));
        let t = e.session().track(vox).unwrap();
        assert_eq!(t.playlists.len(), 6);
        assert_eq!(t.clips().iter().find(|c| c.start == 0).and_then(|c| c.from_layer), Some(5));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn punching_in_keeps_the_material_around_the_punch_range() {
        let dir = std::env::temp_dir().join(format!("soundcraft-punch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        let base = dir.join("Audio Files/Base.wav");
        let take = dir.join("Audio Files/Take.wav");
        write_take(&base, 3_000);
        write_take(&take, 1_000);
        add_recording(&mut e, 0, vec![(vox, base)], None);
        let plan = Recording {
            start: 1_000,
            pass: None,
            punch: Some(Range::new(1_400, 1_600)),
            rate: 48_000,
            takes: vec![Take { track: vox, path: take.clone(), channels: 1 }],
            latency: 0,
        };
        place_recording(&mut e, &plan, vec![(vox, take)]);
        let clips: Vec<_> = e.session().track(vox).unwrap().clips().iter().map(|c| (c.start, c.length, c.source_offset())).collect();
        assert_eq!(clips, vec![(0, 1_400, 0), (1_400, 200, 400), (1_600, 1_400, 1_600)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write_samples(path: &Path, samples: &[f32]) {
        let mut bytes = soundcraft_audio_io::wav_float_header(1, 48_000, samples.len() as u64).unwrap();
        bytes.extend(samples.iter().flat_map(|x| x.to_le_bytes()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soundcraft-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn clip_rows(e: &Engine, t: TrackId) -> Vec<(Samples, Samples, Samples)> {
        e.session().track(t).unwrap().playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.start, c.length, c.source_offset()))).collect()
    }

    #[test]
    fn arming_an_unavailable_input_is_refused_by_name() {
        let mut e = Engine::default();
        e.execute("setup.io", &json!({"action": "create_input", "name": "X32 17", "format": "mono", "first_channel": 16})).unwrap();
        armed(&mut e, "Vox", "mono", "X32 17");
        let err = plan_takes(&e, &test_dir("unavailable"), 2).unwrap_err();
        assert!(err.contains("Vox") && err.contains("X32 17"), "{err}");
        assert!(err.contains("(2 inputs)"), "{err}");
        assert_eq!(plan_takes(&e, &test_dir("unavailable"), 17).unwrap()[0].1, vec![16]);
        // A name that resolves to nothing is refused too.
        let mut e = Engine::default();
        armed(&mut e, "Gtr", "mono", "In 0");
        assert!(plan_takes(&e, &test_dir("unavailable"), 8).unwrap_err().contains("Gtr"));
    }

    #[test]
    fn a_stereo_track_on_a_mono_input_records_it_twice() {
        let mut e = Engine::default();
        e.execute("setup.io", &json!({"action": "create_input", "name": "Vox Mic", "format": "mono", "first_channel": 4})).unwrap();
        armed(&mut e, "Dbl", "stereo", "Vox Mic");
        assert_eq!(plan_takes(&e, &test_dir("mono-twice"), 8).unwrap()[0].1, vec![4, 4]);
        // No input keeps today's default: from channel 0.
        let mut e = Engine::default();
        armed(&mut e, "Pad", "stereo", "none");
        assert_eq!(plan_takes(&e, &test_dir("mono-twice"), 8).unwrap()[0].1, vec![0, 1]);
    }

    #[test]
    fn takes_land_earlier_by_the_latency() {
        let dir = test_dir("latency");
        let path = dir.join("Audio Files/Vox_01.wav");
        let mut take = vec![0.0f32; 1_000];
        take[100] = 1.0;
        write_samples(&path, &take);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        add_recording_in(&mut e, 1_000, vec![(vox, path)], None, None, 100);
        assert_eq!(clip_rows(&e, vox), vec![(1_000, 900, 100)]);
        let clip = e.session().track(vox).unwrap().clips()[0].clone();
        let buf = &e.session().pool.get(clip.source().unwrap()).unwrap().buffer();
        // The clip's first sample (timeline 1000) is the impulse.
        assert_eq!(buf.channels[0][usize::try_from(clip.source_offset()).unwrap()], 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loop_passes_shift_by_the_latency() {
        let dir = test_dir("loop-latency");
        let path = dir.join("Audio Files/Vox_01.wav");
        write_samples(&path, &vec![0.25; 2_550]);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        add_recording_in(&mut e, 4_000, vec![(vox, path)], Some(1_000), None, 50);
        assert_eq!(clip_rows(&e, vox), vec![(4_000, 1_000, 50), (4_000, 1_000, 1_050), (4_000, 500, 2_050)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn punch_trims_on_the_shifted_timeline() {
        let dir = test_dir("punch-latency");
        let path = dir.join("Audio Files/Vox_01.wav");
        write_samples(&path, &vec![0.25; 1_000]);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        // Frame k is at timeline 1000 + k - 100; the punch keeps 1400..1600 = frames 500..700.
        add_recording_in(&mut e, 1_000, vec![(vox, path)], None, Some(Range::new(1_400, 1_600)), 100);
        assert_eq!(clip_rows(&e, vox), vec![(1_400, 200, 500)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_negative_latency_places_the_take_later() {
        let dir = test_dir("negative-latency");
        let path = dir.join("Audio Files/Vox_01.wav");
        write_samples(&path, &vec![0.25; 1_000]);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        add_recording_in(&mut e, 1_000, vec![(vox, path.clone())], None, None, -30);
        assert_eq!(clip_rows(&e, vox), vec![(1_030, 1_000, 0)]);
        // A hostile latency places nothing rather than overflowing.
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        assert!(add_recording_in(&mut e, 1_000, vec![(vox, path.clone())], None, None, Samples::MAX).is_empty());
        add_recording_in(&mut e, 1_000, vec![(vox, path)], Some(400), None, Samples::MIN);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_recording_plans_load_with_zero_latency() {
        let plan: Recording = serde_json::from_value(json!({"start": 5, "pass": null, "punch": null, "rate": 48_000, "takes": []})).unwrap();
        assert_eq!(plan.latency, 0);
    }

    #[test]
    fn a_take_shows_the_pass_under_way_within_the_punch_while_it_records() {
        assert_eq!(live_span(1_000, None, None, 0, 0), None);
        assert_eq!(live_span(1_000, None, None, 500, 0), Some((1_000, Range::new(1_000, 1_500))));
        assert_eq!(live_span(1_000, Some(400), None, 900, 0), Some((200, Range::new(1_000, 1_100))));
        assert_eq!(live_span(1_000, Some(400), None, 800, 0), None);
        let punch = Some(Range::new(1_200, 1_400));
        assert_eq!(live_span(1_000, None, punch, 150, 0), None);
        assert_eq!(live_span(1_000, None, punch, 500, 0), Some((1_000, Range::new(1_200, 1_400))));
    }

    #[test]
    fn a_live_take_shows_where_it_will_land_after_the_latency() {
        // Frame k shows at start + k - latency, as `add_recording_in` places it.
        assert_eq!(live_span(1_000, None, None, 100, 100), None, "only the round trip so far");
        assert_eq!(live_span(1_000, None, None, 500, 100), Some((900, Range::new(1_000, 1_400))));
        // Loop passes split on the shifted timeline: frames 450.. are pass 2 (pass 400, latency 50).
        assert_eq!(live_span(1_000, Some(400), None, 449, 50), Some((950, Range::new(1_000, 1_399))));
        assert_eq!(live_span(1_000, Some(400), None, 500, 50), Some((550, Range::new(1_000, 1_050))));
        // A negative latency shows the take later.
        assert_eq!(live_span(1_000, None, None, 500, -100), Some((1_100, Range::new(1_100, 1_600))));
        let punch = Some(Range::new(1_200, 1_400));
        assert_eq!(live_span(1_000, None, punch, 500, 100), Some((900, Range::new(1_200, 1_400))));
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[test]
    fn autosave_copy_leaves_document_untouched_and_reuses_audio() {
        let mut e = crate::demo::demo_engine();
        e.execute("mix.volume", &serde_json::json!({"track": "Kick", "db": -7.0})).unwrap();
        let dir = std::env::temp_dir().join(format!("soundcraft-autosave-{}", std::process::id()));
        let path = dir.join("Midnight Groove.scraft");
        let p = path.to_string_lossy().into_owned();
        let n1 = save_copy(&e, &p).unwrap();
        let n2 = save_copy(&e, &p).unwrap();
        assert!(n1 >= 5);
        assert_eq!(n2, 0, "second autosave must not rewrite audio");
        assert!(e.is_dirty() && e.path.is_none());
        let mut r = Engine::default();
        open_session(&mut r, &p).unwrap();
        assert_eq!(r.session().track_by_name("Kick").unwrap().mixer.volume_db, -7.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn with_take(level: f32) -> Engine {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let src = add_source(s, "Audio 1_02", AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; 4_800]] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(id, "Audio 1_02", src, 0, 0, 4_800));
        e
    }

    fn reopen(path: &str) -> Engine {
        let mut r = Engine::default();
        assert_eq!(open_session(&mut r, path).unwrap(), Vec::<String>::new());
        r
    }

    #[test]
    fn imported_audio_is_copied_into_the_project_unless_the_preference_is_off() {
        let dir = std::env::temp_dir().join(format!("soundcraft-import-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Elsewhere")).unwrap();
        let original = dir.join("Elsewhere").join("Loop.wav");
        let opts =
            EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Int16, dither: false, bwf: None, bitrate_kbps: 0 };
        let bytes = soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4_800]] }, &opts).unwrap();
        std::fs::write(&original, &bytes).unwrap();
        for (copy, project) in [(true, "On"), (false, "Off")] {
            let mut e = Engine::default();
            e.execute("setup.copy_imported_audio", &json!({"value": copy})).unwrap();
            e.execute("file.import_audio", &json!({"path": original.to_string_lossy()})).unwrap();
            let path = dir.join(project).join(format!("{project}.fdawm"));
            save_session(&mut e, &path.to_string_lossy()).unwrap();
            let stored = e.session().sources[0].path.clone();
            if copy {
                assert_eq!(stored, "Audio Files/Loop.wav");
                assert!(dir.join(project).join("Audio Files").join("Loop.wav").is_file());
            } else {
                assert_eq!(Path::new(&stored), original.as_path(), "a reference to the original");
                assert!(!dir.join(project).join("Audio Files").exists());
            }
            // Reopened, it plays the same audio.
            let r = reopen(&path.to_string_lossy());
            assert_eq!(r.session().pool.get(r.session().sources[0].id).unwrap().buffer().channels[0][100], 0.25);
        }
        // The original is never moved or changed.
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_referring_to_audio_elsewhere_is_made_whole_by_save_as() {
        // An older project that names a file outside its folder: saved as a new project, the file
        // is copied in (with the preference on, the default).
        let dir = std::env::temp_dir().join(format!("soundcraft-save-as-whole-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        e.execute("setup.copy_imported_audio", &json!({"value": false})).unwrap();
        std::fs::create_dir_all(dir.join("Elsewhere")).unwrap();
        let original = dir.join("Elsewhere").join("Pad.wav");
        let opts =
            EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
        std::fs::write(&original, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 480]] }, &opts).unwrap())
            .unwrap();
        e.execute("file.import_audio", &json!({"path": original.to_string_lossy()})).unwrap();
        save_session(&mut e, &dir.join("Old").join("Old.fdawm").to_string_lossy()).unwrap();
        let mut r = reopen(&dir.join("Old").join("Old.fdawm").to_string_lossy());
        assert!(Path::new(&r.session().sources[0].path).is_absolute());
        let new = dir.join("New").join("New.fdawm");
        r.execute("session.save_as", &json!({"path": new.to_string_lossy()})).unwrap();
        assert_eq!(r.session().sources[0].path, "Audio Files/Pad.wav");
        assert!(dir.join("New").join("Audio Files").join("Pad.wav").is_file());
        assert!(dir.join("New").join("Waveforms").join("Pad.wav.peaks").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_projects_waveforms_are_cached_and_read_back_on_open() {
        let dir = std::env::temp_dir().join(format!("soundcraft-peak-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.fdawm").to_string_lossy().into_owned();
        save_session(&mut with_take(0.5), &path).unwrap();
        let wav = dir.join("Audio Files").join("Audio 1_02.wav");
        let cache = dir.join("Waveforms").join("Audio 1_02.wav.peaks");
        assert!(cache.is_file(), "written on save");
        // Reopened, the overview comes from the cache: a doctored cache shows.
        let fake = vec![soundcraft_audio_io::peaks::Peaks::build(&[0.125; 4_800])];
        crate::peak_cache::store(&dir, &wav, 48_000, &fake);
        let r = reopen(&path);
        assert_eq!(r.session().pool.get(r.session().sources[0].id).unwrap().peaks, fake);
        // Deleted, it is made again from the audio on open.
        std::fs::remove_dir_all(dir.join("Waveforms")).unwrap();
        let r = reopen(&path);
        assert_eq!(r.session().pool.get(r.session().sources[0].id).unwrap().peaks, vec![soundcraft_audio_io::peaks::Peaks::build(&[0.5; 4_800])]);
        assert!(cache.is_file(), "made again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_never_replaces_audio_of_another_session() {
        let dir = std::env::temp_dir().join(format!("soundcraft-no-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = |p: &str| dir.join(p).to_string_lossy().into_owned();
        save_session(&mut with_take(0.5), &path("A/A.scraft")).unwrap();
        save_session(&mut with_take(-0.25), &path("A/A v2.scraft")).unwrap();
        save_session(&mut with_take(0.75), &path("B/B.scraft")).unwrap();
        save_session(&mut reopen(&path("B/B.scraft")), &path("A/B.scraft")).unwrap();
        let levels = ["A/A.scraft", "A/A v2.scraft", "B/B.scraft", "A/B.scraft"].map(|p| {
            let r = reopen(&path(p));
            r.session().pool.get(r.session().sources[0].id).unwrap().buffer().channels[0][0]
        });
        assert_eq!(levels, [0.5, -0.25, 0.75, 0.75]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn consolidating_twice_keeps_both_renders() {
        let dir = std::env::temp_dir().join(format!("soundcraft-consolidate-twice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().id;
        let end = e.session().content_end();
        let render = |e: &Engine| soundcraft_mix::render_clips(e.session(), kick, Range::new(0, end));
        e.execute("edit.consolidate", &json!({"tracks": [kick.0], "start": 500_000, "end": 560_000})).unwrap();
        save_session(&mut e, &path).unwrap();
        e.execute("edit.consolidate", &json!({"tracks": [kick.0], "start": 1_400_000, "end": 1_500_000})).unwrap();
        save_session(&mut e, &path).unwrap();
        assert!(render(&reopen(&path)) == render(&e));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_after_undo_reuses_the_saved_audio() {
        let dir = std::env::temp_dir().join(format!("soundcraft-undo-resave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        let mut e = with_take(0.5);
        e.execute("track.new", &json!({})).unwrap();
        assert_eq!(save_session(&mut e, &path).unwrap(), 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(save_session(&mut e, &path).unwrap(), 0);
        assert_eq!(std::fs::read_dir(dir.join("Audio Files")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_audio_is_trusted_only_in_the_folder_it_was_saved_in() {
        let dir = std::env::temp_dir().join(format!("soundcraft-own-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = |p: &str| dir.join(p).to_string_lossy().into_owned();
        let level = |r: &Engine| r.session().pool.get(r.session().sources[0].id).unwrap().buffer().channels[0][0];
        save_session(&mut with_take(-0.25), &path("B/Other.scraft")).unwrap();
        let mut e = with_take(0.5);
        save_session(&mut e, &path("A/A.scraft")).unwrap();
        e.execute("track.new", &json!({})).unwrap();
        save_session(&mut e, &path("B/B.scraft")).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("session.save", &json!({})).unwrap();
        assert_eq!(level(&reopen(&path("B/B.scraft"))), 0.5);
        e.execute("session.save_copy", &json!({"path": path("A/Copy.scraft")})).unwrap();
        e.execute("session.save", &json!({})).unwrap();
        assert_eq!(level(&reopen(&path("B/B.scraft"))), 0.5);
        std::fs::remove_file(dir.join("A/Audio Files/Audio 1_02.wav")).unwrap();
        let mut missing = Engine::default();
        open_session(&mut missing, &path("A/A.scraft")).unwrap();
        save_session(&mut missing, &path("B/Moved.scraft")).unwrap();
        let mut r = Engine::default();
        assert_eq!(
            open_session(&mut r, &path("B/Moved.scraft")).unwrap(),
            // Joined component by component: the engine reports the platform's separators.
            vec![dir.join("A").join("Audio Files").join("Audio 1_02.wav").display().to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_over_another_session_needs_replace() {
        let dir = std::env::temp_dir().join(format!("soundcraft-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        let mut first = with_take(0.5);
        first.execute("session.save_as", &json!({"path": path})).unwrap();
        first.execute("session.save", &json!({})).unwrap();
        first.execute("session.save_as", &json!({"path": dir.join("Song")})).unwrap();
        assert!(first.execute("session.save_template", &json!({"path": path})).is_err());
        let mut second = with_take(-0.25);
        let track = second.session().tracks[0].id.0;
        for id in ["session.save", "session.save_as", "session.save_copy", "session.save_template", "file.export_selected_tracks_as_session"] {
            let err = second.execute(id, &json!({"path": path, "tracks": [track]})).unwrap_err();
            assert!(err.to_string().contains("`replace: true`"), "{id}: {err}");
        }
        assert_eq!(std::fs::read_dir(dir.join("Audio Files")).unwrap().count(), 1);
        Engine::default().execute("session.save_as", &json!({"path": dir.join("New/New.scraft")})).unwrap();
        std::fs::copy(&path, dir.join("Song")).unwrap();
        let mut bare = Engine::default();
        open_session(&mut bare, &dir.join("Song").to_string_lossy()).unwrap();
        bare.execute("session.save", &json!({})).unwrap();
        second.execute("session.save_as", &json!({"path": path, "replace": true})).unwrap();
        let r = reopen(&path);
        assert_eq!(r.session().pool.get(r.session().sources[0].id).unwrap().buffer().channels[0][0], -0.25);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_keeps_the_last_ten_versions_as_backups() {
        let dir = std::env::temp_dir().join(format!("soundcraft-backups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Song.scraft");
        let mut e = Engine::default();
        let mut versions = Vec::new();
        for i in 0..12 {
            e.execute("track.new", &json!({"name": format!("T{i}")})).unwrap();
            save_session(&mut e, &path.to_string_lossy()).unwrap();
            save_session(&mut e, &path.to_string_lossy()).unwrap();
            versions.push(std::fs::read(&path).unwrap());
        }
        let backup = |n: usize| std::fs::read(dir.join(format!("Backups/Song backup {n}.scraft"))).ok();
        assert_eq!(std::fs::read_dir(dir.join("Backups")).unwrap().count(), 10);
        assert_eq!((backup(1), backup(2), backup(11)), (None, Some(versions[1].clone()), Some(versions[10].clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_never_overwrites_an_existing_entry_when_numbers_run_out() {
        let dir = std::env::temp_dir().join(format!("soundcraft-backups-full-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Backups")).unwrap();
        let path = dir.join("Song.scraft");
        std::fs::write(&path, b"session").unwrap();
        let last = dir.join(format!("Backups/Song backup {}.scraft", u32::MAX));
        std::fs::write(&last, b"sentinel").unwrap();
        back_up(&path);
        assert_eq!(std::fs::read(&last).unwrap(), b"sentinel");
        let dest = dir.join("Backups/Song backup 1.scraft");
        std::fs::write(&dest, b"taken").unwrap();
        assert!(copy_new(&path, &dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"taken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn backup_never_writes_through_a_link_to_an_outside_file() {
        let dir = std::env::temp_dir().join(format!("soundcraft-backups-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Project/Backups")).unwrap();
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, b"sentinel").unwrap();
        let path = dir.join("Project/Song.scraft");
        std::fs::write(&path, b"session").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(format!("Project/Backups/Song backup {}.scraft", u32::MAX))).unwrap();
        back_up(&path);
        let link = dir.join("Project/Backups/Song backup 1.scraft");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(copy_new(&path, &link).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"sentinel");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_gives_sources_that_share_a_path_their_own_files() {
        let mut s = Session::default();
        let loud = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 64]] };
        let quiet = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 64]] };
        add_source(&mut s, "tone", loud, Some("Audio Files/tone.wav"), FileFormat::Wav);
        add_source(&mut s, "tone", quiet, Some("Audio Files/tone.wav"), FileFormat::Wav);
        let mut e = Engine::new(s);
        let dir = std::env::temp_dir().join(format!("soundcraft-shared-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Shared.scraft");
        save_session(&mut e, &path.to_string_lossy()).unwrap();
        let paths: Vec<_> = e.session().sources.iter().map(|src| src.path.clone()).collect();
        assert_eq!(paths.len(), 2);
        assert_ne!(paths[0], paths[1], "{paths:?}");
        let a = std::fs::read(dir.join(&paths[0])).unwrap();
        let b = std::fs::read(dir.join(&paths[1])).unwrap();
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    #[test]
    fn a_tiny_sample_rate_is_refused_before_it_is_converted() {
        let buf = |rate| AudioBuffer { sample_rate: rate, channels: vec![vec![0.5; 100]; 2] };
        assert!(at_rate(buf(1), 48_000, 1_000_000).is_none());
        let ok = at_rate(buf(8_000), 48_000, 1_000_000).unwrap();
        assert_eq!((ok.sample_rate, ok.frames()), (48_000, 600));
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;

    #[test]
    fn a_file_named_by_many_sources_is_decoded_once() {
        let dir = std::env::temp_dir().join(format!("soundcraft-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] };
        std::fs::write(dir.join("k.wav"), soundcraft_audio_io::encode(&audio, &EncodeOptions::default()).unwrap()).unwrap();
        let mut s = Session::default();
        for _ in 0..3 {
            add_source(&mut s, "k", audio.clone(), Some("k.wav"), FileFormat::Wav);
        }
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        std::fs::write(&path, s.to_json().unwrap()).unwrap();
        let mut e = Engine::default();
        assert!(open_session(&mut e, &path).unwrap().is_empty());
        let (sources, pool) = (&e.session().sources, &e.session().pool);
        let first = pool.get(sources[0].id).unwrap();
        assert!(sources.iter().all(|src| pool.get(src.id).is_some_and(|a| Arc::ptr_eq(a, first))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sources_playing_one_file_share_it_when_saved() {
        let dir = std::env::temp_dir().join(format!("soundcraft-shared-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Audio Files")).unwrap();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] };
        std::fs::write(dir.join("Audio Files/k.wav"), soundcraft_audio_io::encode(&audio, &EncodeOptions::default()).unwrap()).unwrap();
        let mut s = Session::default();
        for _ in 0..3 {
            add_source(&mut s, "k", audio.clone(), Some("Audio Files/k.wav"), FileFormat::Wav);
        }
        s.sources.iter_mut().for_each(|src| src.unsaved = false);
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        std::fs::write(&path, s.to_json().unwrap()).unwrap();
        let mut e = Engine::default();
        open_session(&mut e, &path).unwrap();
        assert_eq!(save_session(&mut e, &path).unwrap(), 0);
        assert!(e.session().sources.iter().all(|src| src.path == "Audio Files/k.wav"));
        assert_eq!(save_copy(&e, &dir.join("copy/Song.scraft").to_string_lossy()).unwrap(), 1);
        assert_eq!(std::fs::read_dir(dir.join("copy/Audio Files")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn stored_media_paths_are_followed_only_on_this_computer() {
        let dir = std::env::temp_dir().join(format!("soundcraft-stored-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] };
        std::fs::write(dir.join("k.wav"), soundcraft_audio_io::encode(&audio, &EncodeOptions::default()).unwrap()).unwrap();
        let mut saved = Session::default();
        let t = saved.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let src = add_source(&mut saved, "k", audio, Some("../k.wav"), FileFormat::Wav);
        let id = saved.new_clip_id();
        crate::edit::place_clip(&mut saved, t, Clip::audio(id, "k", src, 0, 0, 480));
        // In a folder of its own, so a movie named `k.wav` is not relinked to a file beside it.
        std::fs::create_dir_all(dir.join("s")).unwrap();
        let path = dir.join("s").join("Song.scraft").to_string_lossy().into_owned();
        let mut stored = vec![r"\\192.0.2.1\share\x.wav".to_string(), "file://192.0.2.1/s/x.wav".to_string()];
        // Linux reads `//tmp/…` as the local `/tmp/…`, so only the path check keeps it from being read.
        if cfg!(unix) {
            stored.push(format!("/{}", dir.join("k.wav").display()));
        }
        let refused = |m: &str| m.contains("not opened: the path is on another computer");
        for p in stored {
            let mut s = saved.clone();
            s.sources[0].path.clone_from(&p);
            let id = SourceId(s.alloc());
            s.videos.push(soundcraft_model::VideoSource {
                id,
                name: "x".into(),
                path: p.clone(),
                width: 0,
                height: 0,
                frame_rate: 0.0,
                duration: 0.0,
                codec: String::new(),
                offline: None,
            });
            std::fs::write(&path, s.to_json().unwrap()).unwrap();
            let mut r = Engine::default();
            let missing = open_session(&mut r, &path).unwrap();
            assert_eq!(missing.iter().filter(|m| refused(m)).count(), 2, "{p}: {missing:?}");
            assert!(r.session().pool.get(r.session().sources[0].id).is_none(), "{p}");
            assert!(r.session().videos[0].offline.is_some(), "{p}");
            let mut into = Engine::default();
            let imported = into.execute("file.import_session_data", &json!({"path": path})).unwrap();
            assert!(imported["missing"].as_array().unwrap().iter().any(|m| m.as_str().is_some_and(refused)), "{p}: {imported}");
            assert!(into.session().pool.get(into.session().sources[0].id).is_none(), "{p}");
        }
        // A path relative to the session opens.
        std::fs::write(&path, saved.to_json().unwrap()).unwrap();
        let mut r = Engine::default();
        assert!(open_session(&mut r, &path).unwrap().is_empty());
        assert!(r.session().pool.get(r.session().sources[0].id).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn a_midi_track_name_is_cut_to_a_length_that_can_be_drawn() {
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let smf = soundcraft_midi::Smf { tracks: vec![soundcraft_midi::SmfTrack { name: "n".repeat(1_000_000), sequence }], ..Default::default() };
        let mut e = Engine::default();
        let ids = import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf).unwrap(), 0, false).unwrap();
        let track = e.session().track(ids[0]).unwrap();
        assert_eq!((track.name.len(), track.clips()[0].name.len()), (soundcraft_model::MAX_NAME_CHARS, soundcraft_model::MAX_NAME_CHARS));
    }

    #[test]
    fn a_controller_only_midi_track_is_imported_with_its_events() {
        let ctrls = vec![
            soundcraft_midi::CtrlEvent { tick: 0, channel: 0, kind: soundcraft_midi::CtrlKind::Cc(7), value: 100 },
            soundcraft_midi::CtrlEvent { tick: 480, channel: 0, kind: soundcraft_midi::CtrlKind::Cc(7), value: 64 },
            soundcraft_midi::CtrlEvent { tick: 960, channel: 0, kind: soundcraft_midi::CtrlKind::Cc(7), value: 20 },
        ];
        let sequence = soundcraft_midi::Sequence { ctrls: ctrls.clone(), ..Default::default() };
        let smf = soundcraft_midi::Smf { tracks: vec![soundcraft_midi::SmfTrack { name: "Vol".into(), sequence }], ..Default::default() };
        let mut e = Engine::default();
        let ids = import_midi_bytes(&mut e, &soundcraft_midi::write_smf(&smf).unwrap(), 0, false).unwrap();
        assert_eq!(ids.len(), 1);
        let track = e.session().track(ids[0]).unwrap();
        assert_eq!(track.instrument.as_ref().map(|i| i.plugin.as_str()), Some("subtractive_synth"));
        // The clip must reach past the last event, or playback skips it.
        assert!(track.clips()[0].end() > e.session().tempo.tick_to_samples(960, e.session().sample_rate));
        match &track.clips()[0].content {
            ClipContent::Midi { sequence } => assert_eq!(sequence.ctrls, ctrls),
            _ => panic!("expected a MIDI clip"),
        }
    }

    #[test]
    fn midi_longer_than_a_day_is_refused() {
        let held = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: (1 << 28) - 1 };
        let sequence = soundcraft_midi::Sequence { notes: vec![held], ..Default::default() };
        let smf = soundcraft_midi::Smf { tracks: vec![soundcraft_midi::SmfTrack { name: String::new(), sequence }], ..Default::default() };
        let err = import_midi_bytes(&mut Engine::default(), &soundcraft_midi::write_smf(&smf).unwrap(), 0, false).unwrap_err();
        assert!(err.to_string().contains("longer than 24 hours"), "{err}");
    }

    #[test]
    fn midi_with_more_meter_changes_than_scores_have_is_refused() {
        let note = soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 };
        let sequence = soundcraft_midi::Sequence { notes: vec![note], ..Default::default() };
        let meters = (0..=MAX_MIDI_METERS as i64).map(|bar| (bar * 3840, 4, 4)).collect();
        let smf = soundcraft_midi::Smf { tracks: vec![soundcraft_midi::SmfTrack { name: String::new(), sequence }], meters, ..Default::default() };
        let err = import_midi_bytes(&mut Engine::default(), &soundcraft_midi::write_smf(&smf).unwrap(), 0, true).unwrap_err();
        assert!(err.to_string().contains("meter changes"), "{err}");
    }

    #[test]
    fn new_sources_take_the_first_free_file_name() {
        let mut s = Session::default();
        let buf = || AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 4]] };
        add_source(&mut s, "k", buf(), Some("Audio Files/k-02.wav"), FileFormat::Wav);
        let mut path = || {
            let id = add_source(&mut s, "k", buf(), None, FileFormat::Wav);
            s.source(id).map(|x| x.path.clone()).unwrap_or_default()
        };
        assert_eq!([path(), path(), path()], ["Audio Files/k.wav", "Audio Files/k-2.wav", "Audio Files/k-3.wav"]);
    }
}

#[cfg(all(test, unix))]
mod pipe_tests {
    use super::*;

    #[test]
    fn a_pipe_named_as_media_is_refused_not_waited_on() {
        let dir = std::env::temp_dir().join(format!("soundcraft-pipe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe.wav");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let mut s = Session::default();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 10]] };
        add_source(&mut s, "pipe", audio, Some(&fifo.to_string_lossy()), FileFormat::Wav);
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        std::fs::write(&path, s.to_json().unwrap()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let pipe = fifo.to_string_lossy().into_owned();
        std::thread::spawn(move || {
            let mut e = Engine::default();
            let imported = e.execute("file.import_audio", &json!({"path": pipe})).is_ok();
            let _ = tx.send((imported, open_session(&mut e, &path).map_err(|err| err.to_string())));
        });
        let (imported, opened) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(!imported);
        assert_eq!(opened.unwrap(), [fifo.display().to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod stem_tests {
    #[test]
    fn stems_one_file_per_track() {
        let mut e = crate::demo::demo_engine();
        let dir = std::env::temp_dir().join(format!("soundcraft-stems-{}", std::process::id()));
        let r = e
            .execute("file.bounce_stems", &serde_json::json!({"dir": dir.to_string_lossy(), "start": {"seconds": 10.0}, "end": {"seconds": 11.0}}))
            .unwrap();
        let n = r["files"].as_array().map_or(0, Vec::len);
        assert_eq!(n, 7, "{r}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod strip_tests {
    use super::*;

    /// A stereo track with `length`-long clips at `starts`, each playing a second of steady tone.
    fn tones(starts: &[Samples], length: Samples) -> (Engine, TrackId) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id;
        let s = e.session_mut();
        let tone = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]; 2] };
        let src = add_source(s, "tone", tone, None, FileFormat::Wav);
        for at in starts {
            let id = s.new_clip_id();
            crate::edit::place_clip(s, t, Clip::audio(id, "tone", src, 0, *at, length));
        }
        (e, t)
    }

    #[test]
    fn clips_past_what_one_render_holds_are_kept() {
        // 100 minutes in: a stereo render stops at 2^28 frames, about 93 minutes.
        let far = 48_000 * 6000;
        let (mut e, t) = tones(&[0, far], 48_000);
        let r = e.execute("edit.strip_silence", &json!({"track": t.0, "start": 0, "end": far + 96_000})).unwrap();
        assert_eq!(r["clips"], 2);
        assert!(e.session().track(t).unwrap().clips().iter().any(|c| c.start >= far));
    }

    #[test]
    fn more_than_a_day_of_clips_is_refused() {
        let (mut e, t) = tones(&[0], 48_000 * 86_401);
        assert!(e.execute("edit.strip_silence", &json!({"track": t.0, "start": 0, "end": 48_000i64 * 86_402})).is_err());
        assert_eq!(e.session().track(t).unwrap().clips().len(), 1);
    }
}

#[cfg(test)]
mod bundle_tests {
    use super::*;

    #[test]
    fn bundle_round_trips_session_and_audio() {
        let mut e = crate::demo::demo_engine();
        e.execute("mix.volume", &serde_json::json!({"track": "Kick", "db": -7.0})).unwrap();
        let (bytes, fresh) = session_bundle(e.session()).unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"));
        assert!(fresh.sources.iter().all(|s| s.path.starts_with("Audio Files/") && !s.unsaved));
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Midnight Groove.scraftz", &bytes).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(r.session().track_by_name("Kick").unwrap().mixer.volume_db, -7.0);
        assert_eq!(r.session().tracks.len(), e.session().tracks.len());
        for src in &e.session().sources {
            let (a, b) = (e.session().pool.get(src.id).unwrap(), r.session().pool.get(src.id).unwrap());
            assert_eq!(a.buffer().channels, b.buffer().channels, "{}: float WAV keeps every sample", src.name);
        }
        assert!(!r.is_dirty());
        assert_eq!(r.path.as_deref(), Some("Midnight Groove.scraftz"));
    }

    #[test]
    fn sources_sharing_one_file_all_load_from_a_bundle() {
        use std::io::Write as _;
        let e = crate::demo::demo_engine();
        let (bytes, _) = session_bundle(e.session()).unwrap();
        let (text, files) = read_bundle(&bytes).unwrap();
        let mut s = Session::from_json(&text).unwrap();
        s.sources[1].path = s.sources[0].path.clone();
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for path in s.sources.iter().map(|x| &x.path).collect::<std::collections::BTreeSet<_>>() {
            w.start_file(path.as_str(), opts).unwrap();
            w.write_all(&files[path]).unwrap();
        }
        w.start_file("Song.scraft", opts).unwrap();
        w.write_all(s.to_json().unwrap().as_bytes()).unwrap();
        let bundle = w.finish().unwrap().into_inner();
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Song.scraftz", &bundle).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        for src in &r.session().sources {
            assert!(r.session().pool.get(src.id).is_some(), "{} has no audio", src.name);
        }
    }

    #[test]
    fn same_named_sources_get_distinct_files() {
        let mut e = crate::demo::demo_engine();
        let name = e.session().sources[0].name.clone();
        let s = e.session_mut();
        let n = s.sources.len();
        s.sources[1].name = name;
        let (bytes, fresh) = session_bundle(e.session()).unwrap();
        let paths: std::collections::HashSet<_> = fresh.sources.iter().map(|s| s.path.clone()).collect();
        assert_eq!(paths.len(), n);
        let mut r = Engine::default();
        assert!(open_session_bytes(&mut r, "x.scraftz", &bytes).unwrap().is_empty());
    }

    #[test]
    fn bare_session_opens_with_its_audio_reported_missing() {
        let e = crate::demo::demo_engine();
        let (_, fresh) = session_bundle(e.session()).unwrap();
        let text = fresh.to_json().unwrap();
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Song.scraft", text.as_bytes()).unwrap();
        assert_eq!(missing.len(), fresh.sources.len());
        assert_eq!(r.session().tracks.len(), fresh.tracks.len());
    }

    #[test]
    fn hostile_bundles_are_errors_not_panics() {
        let mut r = Engine::default();
        for bad in [&b"PK\x03\x04 not really a zip"[..], b"", b"{ not json", b"\xff\xfe\x00"] {
            assert!(open_session_bytes(&mut r, "bad.scraftz", bad).is_err());
        }
        // A valid zip without a session inside.
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("Audio Files/a.wav", zip::write::SimpleFileOptions::default()).unwrap();
        let only_audio = w.finish().unwrap().into_inner();
        assert!(open_session_bytes(&mut r, "a.scraftz", &only_audio).is_err());
        assert_eq!(r.session().tracks.len(), 0, "a failed open leaves the document alone");
    }

    #[test]
    fn write_file_writes_to_disk_natively() {
        let p = std::env::temp_dir().join(format!("soundcraft-write-file-{}.txt", std::process::id()));
        write_file(&p.to_string_lossy(), b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        let _ = std::fs::remove_file(&p);
    }
}

#[cfg(test)]
mod smf_tempo_tests {
    use super::*;
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_model::{ChannelFormat, TrackKind};

    /// Seconds from the file's start to `tick`, by the file's tempo events.
    fn file_secs(tempos: &[(i64, f64)], tick: i64) -> f64 {
        let q = f64::from(soundcraft_midi::PPQ);
        let mut secs = 0.0;
        for (k, (t, bpm)) in tempos.iter().enumerate() {
            let end = tempos.get(k + 1).map_or(i64::MAX, |n| n.0).min(tick);
            if end > *t {
                secs += (end - t) as f64 / q * 60.0 / bpm;
            }
        }
        secs
    }

    #[test]
    fn smf_export_keeps_every_note_time_through_a_ramp_and_a_late_bar_1() {
        let mut s = Session::default();
        let sr = s.sample_rate;
        // Bar 1 at 1.5 s; 120 BPM ramping to 180 over two bars, then 180.
        s.tempo.set_origin(72_000);
        s.tempo.set_tempo(0, 120.0).unwrap();
        s.tempo.set_tempo(8 * 960, 180.0).unwrap();
        assert!(s.tempo.set_ramp(0, true));
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("Keys"));
        // A note every half second from 0.25 s (before bar 1 too), at the clip from 0.
        let times: Vec<i64> = (0..12).map(|k| 12_000 + k * 24_000).collect();
        let notes = times
            .iter()
            .map(|at| {
                let start = s.tempo.samples_to_ticks(*at, sr) - s.tempo.samples_to_ticks(0, sr);
                Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start, length: 240 }
            })
            .collect();
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::midi(cid, "keys", 0, 12 * 24_000, Sequence { notes, ctrls: Vec::new() }));
        let e = Engine::new(s);
        let smf = soundcraft_midi::read_smf(&export_midi(&e, None).unwrap()).unwrap();
        assert!(smf.tempos.len() > 8, "the ramp in steps: {:?}", smf.tempos);
        let got: Vec<f64> = smf.tracks[0].sequence.notes.iter().map(|n| file_secs(&smf.tempos, n.start)).collect();
        assert_eq!(got.len(), times.len());
        for (g, at) in got.iter().zip(&times) {
            let want = *at as f64 / 48_000.0;
            assert!((g - want).abs() < 0.002, "a note at {want:.3} s is at {g:.3} s in the file");
        }
        // Bar 1 is a bar line in the file: the pickup is whole quarters, then the session's meter.
        let (pickup, n, d) = smf.meters[0];
        assert_eq!((pickup, d), (0, 4));
        assert_eq!(smf.meters[1].0, i64::from(n) * 960);
        assert!((file_secs(&smf.tempos, smf.meters[1].0) - 1.5).abs() < 1e-3);
    }
}
