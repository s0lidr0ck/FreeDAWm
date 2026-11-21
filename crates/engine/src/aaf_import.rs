//! File › Import › AAF... (`file.import_aaf`): the audio of an AAF file from Pro Tools, Media
//! Composer, Premiere or Resolve as new tracks in the open session, one undo step.
//!
//! The file is read by `soundcraft-aaf` into its neutral description (tracks, clips in samples
//! at the session's rate, fades, crossfades, clip gain, track volume/pan/mute/solo, markers,
//! media); this module places it. Every file it reads goes through [`crate::io`]'s checked
//! helpers. Audio embedded in the AAF is decoded into the session like a recording (written into
//! its Audio Files folder on the first save). External audio is looked for where the AAF says
//! and beside the AAF; found, it is imported as File › Import › Audio would (copied unless the
//! session refers to imported files where they are). Not found or not readable, its clips still
//! come in, playing nothing, with their source named and its last known path kept (relink it
//! with Clip › Relink Audio...), as a session whose audio went missing opens.
//!
//! Positions: the composition starts at its start timecode. In a session with no clips yet the
//! session's own timecode start (and frame rate, when it is one FreeDAWm has) become the
//! composition's, so its clips sit where they sat; in a session with clips, they are placed by
//! timecode against the session's. `at` places the composition's start anywhere instead.

use crate::{Engine, EngineError, Result};
use serde_json::{Value, json};
use soundcraft_aaf::desc::{ChannelRef, Clip as AClip, Curve, Import, MediaData};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{AutoParam, ChannelFormat, Clip, Fade, FadeShape, MarkerKind, Session, Source, SourceId, TrackId, TrackKind};
use soundcraft_time::{FrameRate, Samples};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// The largest AAF file read (its embedded audio included).
pub const MAX_AAF_BYTES: u64 = 4 << 30;
/// The largest external audio file read.
const MAX_MEDIA_BYTES: u64 = 4 << 30;

/// Reads the AAF file at `path` and imports it ([`import_bytes`]).
pub fn import_path(e: &mut Engine, path: &str, at: Option<Samples>, rendered_fades: bool) -> Result<Value> {
    let bytes = crate::io::read_file_max(path, MAX_AAF_BYTES)?;
    let p = Path::new(path);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    import_bytes(e, &bytes, &name, p.parent(), at, rendered_fades)
}

/// A media file of the AAF, as far as it could be read.
enum Got {
    /// Decoded at the session rate, with the path it came from (external) and its format.
    Audio { buf: Arc<AudioBuffer>, path: Option<String>, format: FileFormat, file_rate: u32 },
    /// Not readable: why, and the path it was last at (or its name).
    Offline { why: String, path: String },
}

/// Imports an AAF file held in memory; external media is looked for beside `dir`.
pub fn import_bytes(e: &mut Engine, bytes: &[u8], file: &str, dir: Option<&Path>, at: Option<Samples>, rendered_fades: bool) -> Result<Value> {
    let sr = e.session().sample_rate.hz();
    let opts = soundcraft_aaf::Options { sample_rate: sr, pt_auto: !rendered_fades, max_embedded: MAX_AAF_BYTES, ..Default::default() };
    let imp = soundcraft_aaf::read(bytes, &opts).map_err(|err| EngineError::Io(format!("{file}: {err}")))?;
    let got = (0..imp.media.len()).map(|_| None).collect();
    let copy = e.copy_imported_audio;
    let s = e.session_mut();
    let mut placer = Placer {
        s,
        imp: &imp,
        dir: dir.map(Path::to_path_buf),
        got,
        sources: HashMap::new(),
        media_sources: HashMap::new(),
        copy,
        offline: Vec::new(),
        originals: Vec::new(),
    };
    placer.place(at, file)
}

fn media(m: &soundcraft_aaf::desc::Media, dir: Option<&Path>, sr: u32) -> Got {
    let decode = |bytes: &[u8], ext: Option<&str>, path: Option<String>| -> std::result::Result<Got, String> {
        let (info, buf) = soundcraft_audio_io::decode(bytes, ext).map_err(|e| e.to_string())?;
        if buf.frames() == 0 {
            return Err("no audio".into());
        }
        let buf = crate::io::at_rate(buf, sr, crate::io::MAX_IMPORT_SAMPLES).ok_or("too large to import")?;
        Ok(Got::Audio { buf: Arc::new(buf), path, format: info.format, file_rate: info.sample_rate })
    };
    match &m.data {
        MediaData::Embedded { bytes, ext } => {
            decode(bytes, Some(ext), None).unwrap_or_else(|why| Got::Offline { why: format!("embedded audio: {why}"), path: m.name.clone() })
        }
        MediaData::External { locations } => {
            let mut why = "not found".to_string();
            let mut last = locations.first().map(|l| soundcraft_aaf::desc::url_to_path(l)).unwrap_or_else(|| m.name.clone());
            for loc in locations {
                for cand in soundcraft_aaf::desc::candidate_paths(loc, dir) {
                    if !crate::io::file_found(&cand) {
                        continue;
                    }
                    let shown = cand.to_string_lossy().into_owned();
                    let ext = cand.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
                    match crate::io::read_file_max(&cand, MAX_MEDIA_BYTES)
                        .map_err(|e| e.to_string())
                        .and_then(|b| decode(&b, ext.as_deref(), Some(shown.clone())))
                    {
                        Ok(g) => return g,
                        Err(w) => {
                            why = w;
                            last = shown;
                        }
                    }
                }
            }
            Got::Offline { why, path: last }
        }
        MediaData::Missing => Got::Offline { why: "the AAF does not say where its audio is".into(), path: m.name.clone() },
    }
}

struct Placer<'a> {
    s: &'a mut Session,
    imp: &'a Import,
    /// The AAF's folder (external media is looked for beside it).
    dir: Option<std::path::PathBuf>,
    /// Each medium once read (only media a clip plays are read).
    got: Vec<Option<Got>>,
    /// A clip's channel layout (each channel: media, channel, offset past the clip's) → source.
    sources: HashMap<Vec<Option<(usize, u16, i64)>>, SourceId>,
    /// Media index → the source playing it as it is.
    media_sources: HashMap<usize, SourceId>,
    copy: bool,
    /// Media that could not be read: (name, why).
    offline: Vec<(String, String)>,
    /// Media played from the file it was imported from: (name, that file).
    originals: Vec<(String, String)>,
}

fn shape(c: Curve) -> FadeShape {
    match c {
        Curve::Linear => FadeShape::Linear,
        Curve::Power => FadeShape::EqualPower,
        Curve::Log => FadeShape::Logarithmic,
        Curve::Exp => FadeShape::Exponential,
    }
}

fn db(factor: f64) -> f32 {
    if factor > 0.0 && factor.is_finite() { (20.0 * factor.log10()).clamp(-144.0, 24.0) as f32 } else { -144.0 }
}

/// The FreeDAWm frame rate for an AAF timecode, if there is one.
fn frame_rate(tc: &soundcraft_aaf::desc::Timecode) -> Option<FrameRate> {
    let (n, d) = (f64::from(tc.rate.0), f64::from(tc.rate.1));
    if n <= 0.0 || d <= 0.0 {
        return None;
    }
    let want = n / d;
    let close = |r: FrameRate| {
        let (a, b) = r.rational();
        ((a as f64 / b as f64) - want).abs() < want * 2e-4
    };
    FrameRate::ALL
        .into_iter()
        .find(|r| close(*r) && r.is_drop() == tc.drop)
        .or_else(|| FrameRate::ALL.into_iter().find(|r| close(*r) && !r.is_drop()))
}

impl Placer<'_> {
    /// Medium `m`, read on first use.
    fn got(&mut self, m: usize) -> Option<&Got> {
        let sr = self.s.sample_rate.hz();
        let slot = self.got.get_mut(m)?;
        if slot.is_none() {
            *slot = Some(media(self.imp.media.get(m)?, self.dir.as_deref(), sr));
        }
        self.got.get(m)?.as_ref()
    }

    /// Medium `m`'s audio, if it could be read.
    fn audio(&mut self, m: usize) -> Option<Arc<AudioBuffer>> {
        match self.got(m)? {
            Got::Audio { buf, .. } => Some(Arc::clone(buf)),
            Got::Offline { .. } => None,
        }
    }

    /// What a clip channel plays: its medium, or when that cannot be read and the AAF names the
    /// file it was made from, that file.
    fn effective(&mut self, ch: ChannelRef) -> (usize, u16, i64) {
        if self.audio(ch.media).is_none()
            && let Some((om, oc, oo)) = ch.original
            && let Some(buf) = self.audio(om)
        {
            let name = self.imp.media.get(ch.media).map(|m| m.name.clone()).unwrap_or_default();
            let from = match self.got(om) {
                Some(Got::Audio { path: Some(p), .. }) => p.clone(),
                _ => String::new(),
            };
            if !self.originals.iter().any(|(n, f)| *n == name && *f == from) {
                self.originals.push((name, from));
            }
            let c = oc.min(u16::try_from(buf.num_channels().saturating_sub(1)).unwrap_or(0));
            return (om, c, oo);
        }
        (ch.media, ch.channel, ch.offset)
    }

    fn place(&mut self, at: Option<Samples>, file: &str) -> Result<Value> {
        let imp = self.imp;
        let min_start = imp.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.start)).chain(imp.markers.iter().map(|m| m.start)).min().unwrap_or(0);
        let empty = self.s.tracks.iter().all(|t| t.clips().is_empty());
        let mut notes: Vec<String> = Vec::new();
        let mut shift = match at {
            Some(a) => a,
            None if empty => {
                self.s.timecode_start = imp.start;
                if let Some(tc) = &imp.timecode {
                    match frame_rate(tc) {
                        Some(r) => self.s.frame_rate = r,
                        None => notes.push(format!("timecode at {}/{} frames a second: kept the session's frame rate", tc.rate.0, tc.rate.1)),
                    }
                }
                0
            }
            None => imp.start - self.s.timecode_start,
        };
        if shift + min_start < 0 {
            notes.push("the composition starts before the session: moved to the session start".into());
            shift = -min_start;
        }
        let mut tracks = Vec::new();
        let mut clips = 0usize;
        for t in &imp.tracks {
            let channels = usize::from(t.channels.max(1));
            let format = ChannelFormat::for_channels(channels);
            let id = self.s.add_track(TrackKind::Audio, format, Some(&t.name));
            tracks.push(id);
            if format.channels() != channels {
                notes.push(format!("\"{}\": {channels} channels, imported as {}", t.name, format.channels()));
            }
            let mut placed = Vec::new();
            for c in &t.clips {
                if let Some(clip) = self.clip(c, shift) {
                    placed.push(clip);
                }
            }
            clips += placed.len();
            self.mixer(id, t, shift);
            if let Some(pl) = self.s.track_mut(id).and_then(|tr| tr.playlist_mut()) {
                pl.clips.extend(placed);
                pl.sort();
            }
        }
        for m in &imp.markers {
            let start = (m.start + shift).max(0);
            let kind = if m.len > 0 { MarkerKind::Selection } else { MarkerKind::Marker };
            let id = self.s.add_marker(&m.name, kind, start, start + m.len.max(0));
            if let Some(loc) = self.s.markers.iter_mut().find(|l| l.id == id) {
                if m.comment != m.name {
                    loc.comments = m.comment.clone();
                }
                loc.color = m.color;
            }
        }
        // The summary, and the report in the Notes tab.
        let mut skipped: Vec<String> = imp.skipped.clone();
        skipped.extend(notes);
        let offline: Vec<String> = self.offline.iter().map(|(n, why)| format!("{n}: {why}")).collect();
        for (n, f) in &self.originals {
            skipped.push(format!("{n}: its own media is not read, played from the file it came from ({f})"));
        }
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut summary = format!("AAF {file}: {}, {}", plural(tracks.len(), "track", "tracks"), plural(clips, "clip", "clips"));
        if !imp.markers.is_empty() {
            summary.push_str(&format!(", {}", plural(imp.markers.len(), "marker", "markers")));
        }
        if !offline.is_empty() {
            summary.push_str(&format!("; {} offline", plural(offline.len(), "file", "files")));
        }
        if !skipped.is_empty() {
            summary.push_str(&format!("; {} not carried (see Notes)", plural(skipped.len(), "thing", "things")));
        }
        let mut report = vec![summary.clone()];
        if !imp.writer.is_empty() {
            report.push(format!("Written by {}.", imp.writer));
        }
        if let Some(tc) = &imp.timecode {
            report.push(format!("Starts at timecode frame {} ({} fps{}).", tc.frames, tc.fps, if tc.drop { " drop" } else { "" }));
        }
        if let Some(r) = imp.media_rate.filter(|r| *r != imp.sample_rate) {
            report.push(format!("Media at {r} Hz, converted to the session's {} Hz.", imp.sample_rate));
        }
        for o in &offline {
            report.push(format!("Offline: {o}"));
        }
        for k in &skipped {
            report.push(format!("Not carried: {k}"));
        }
        let text = report.join("\n");
        let id = self.s.alloc();
        let mut note: String = text.chars().take(soundcraft_model::MAX_NOTE_CHARS.saturating_sub(1)).collect();
        if note.len() < text.len() {
            note.push('…');
        }
        self.s.notes.push(soundcraft_model::Note { id, text: note, done: false, tags: vec!["aaf".into()], at: None, created: String::new() });
        soundcraft_model::sanitize_notes(&mut self.s.notes);
        self.s.sanitize();
        Ok(json!({
            "name": imp.name,
            "writer": imp.writer,
            "tracks": tracks,
            "clips": clips,
            "markers": imp.markers.len(),
            "offline": offline,
            "skipped": skipped,
            "summary": summary,
            "report_text": text,
        }))
    }

    fn mixer(&mut self, id: TrackId, t: &soundcraft_aaf::desc::Track, shift: Samples) {
        let Some(tr) = self.s.track_mut(id) else { return };
        tr.mixer.mute = t.mute;
        tr.mixer.solo = t.solo;
        if let Some(g) = t.gain_db {
            tr.mixer.volume_db = g.clamp(-144.0, 12.0);
        }
        if let Some(p) = t.pan {
            let p = if p.is_finite() { p.clamp(-1.0, 1.0) } else { 0.0 };
            match tr.mixer.pan.len() {
                1 => tr.mixer.pan = vec![p],
                2 => tr.mixer.pan = if p >= 0.0 { vec![-1.0 + 2.0 * p, 1.0] } else { vec![-1.0, 1.0 + 2.0 * p] },
                _ => {}
            }
        }
        let mut lanes = Vec::new();
        if !t.gain_auto.is_empty() {
            let lane = tr.lane_mut(&AutoParam::Volume);
            lane.points.clear();
            for (at, v) in &t.gain_auto {
                lane.set_point((at + shift).max(0), db(*v).clamp(-144.0, 12.0));
            }
            lanes.push(AutoParam::Volume.id());
        }
        if !t.pan_auto.is_empty() && tr.mixer.pan.len() == 1 {
            let lane = tr.lane_mut(&AutoParam::Pan(0));
            lane.points.clear();
            for (at, v) in &t.pan_auto {
                lane.set_point((at + shift).max(0), v.clamp(-1.0, 1.0));
            }
            lanes.push(AutoParam::Pan(0).id());
        }
        for l in lanes {
            if !tr.auto_lanes.contains(&l) {
                tr.auto_lanes.push(l);
            }
        }
    }

    fn clip(&mut self, c: &AClip, shift: Samples) -> Option<Clip> {
        let len = c.len.max(1);
        let start = (c.start + shift).max(0);
        let (src, offset, source_channel) = self.source_for(c)?;
        let cid = self.s.new_clip_id();
        let mut clip = Clip::audio(cid, c.name.clone(), src, offset, start, len);
        clip.source_channel = source_channel;
        if let Some(f) = c.fade_in {
            clip.fade_in = Fade { len: f.len.clamp(0, len), shape: shape(f.curve), curvature: 0.0 };
        }
        if let Some(f) = c.fade_out {
            clip.fade_out = Fade { len: f.len.clamp(0, len), shape: shape(f.curve), curvature: 0.0 };
        }
        if let Some(g) = c.gain_db {
            clip.gain_db = g.clamp(-144.0, 24.0);
        }
        clip.gain_env = c.gain_env.iter().map(|(t, v)| ((*t).clamp(0, len), db(*v))).collect();
        clip.muted = c.muted;
        Some(clip)
    }

    /// The source a clip plays, its offset in it and the one channel it takes (a mono clip of a
    /// multichannel file).
    fn source_for(&mut self, c: &AClip) -> Option<(SourceId, Samples, Option<u16>)> {
        let chans: Vec<Option<(usize, u16, i64)>> = c.channels.iter().map(|ch| ch.map(|x| self.effective(x))).collect();
        let width = |p: &mut Self, m: usize| {
            p.audio(m).map(|b| b.num_channels()).unwrap_or_else(|| p.imp.media.get(m).map_or(1, |x| usize::from(x.channels.max(1))))
        };
        // One medium played as it is: its own source (one channel of it for a mono clip of a
        // multichannel file).
        if let [Some((m, ch, off))] = chans.as_slice() {
            let (m, ch, off) = (*m, *ch, *off);
            let w = width(self, m);
            let src = self.media_source(m)?;
            return Some((src, off.max(0), (w > 1).then_some(ch)));
        }
        if let Some(Some((m, _, off))) = chans.first() {
            let (m, off) = (*m, *off);
            let in_order = chans.len() == width(self, m)
                && chans.iter().enumerate().all(|(i, ch)| ch.is_some_and(|(cm, cc, co)| cm == m && usize::from(cc) == i && co == off));
            if in_order {
                return Some((self.media_source(m)?, off.max(0), None));
            }
        }
        // Several media (Pro Tools' and Media Composer's multi-mono files) as one source.
        let key: Vec<Option<(usize, u16, i64)>> = chans.iter().map(|ch| ch.map(|(m, cc, o)| (m, cc, o - c.offset))).collect();
        if let Some(&src) = self.sources.get(&key) {
            return Some((src, c.offset.max(0), None));
        }
        let src = self.combined(c, &key)?;
        self.sources.insert(key, src);
        Some((src, c.offset.max(0), None))
    }

    fn media_source(&mut self, m: usize) -> Option<SourceId> {
        if let Some(&s) = self.media_sources.get(&m) {
            return Some(s);
        }
        let info = self.imp.media.get(m)?;
        let sr = self.s.sample_rate.hz();
        let got = match self.got(m)? {
            Got::Audio { buf, path, format, file_rate } => {
                Got::Audio { buf: Arc::clone(buf), path: path.clone(), format: *format, file_rate: *file_rate }
            }
            Got::Offline { why, path } => Got::Offline { why: why.clone(), path: path.clone() },
        };
        let src = match &got {
            Got::Audio { buf, path, format, file_rate } => {
                let audio = Arc::new(soundcraft_model::SourceAudio::new((**buf).clone()));
                let id = crate::io::insert_source(self.s, &info.name, audio, path.as_deref(), *format);
                // A file at the session's rate is referred to where it is when the session does
                // not copy imported audio (as File › Import › Audio).
                if !self.copy
                    && path.as_deref().is_some_and(|p| Path::new(p).is_absolute())
                    && *file_rate == sr
                    && let Some(sx) = self.s.sources.last_mut()
                {
                    sx.unsaved = false;
                }
                id
            }
            Got::Offline { why, path } => {
                self.offline.push((info.name.clone(), why.clone()));
                let frames = if info.sample_rate > 0 { (info.frames as f64 * f64::from(sr) / f64::from(info.sample_rate)).round() as u64 } else { 0 };
                self.offline_source(&info.name, path, info.channels.max(1), frames)
            }
        };
        self.media_sources.insert(m, src);
        Some(src)
    }

    /// A source with no audio: listed with its name and last known path, as missing audio is
    /// when a session opens without it.
    fn offline_source(&mut self, name: &str, path: &str, channels: u16, frames: u64) -> SourceId {
        let id = SourceId(self.s.alloc());
        let sr = self.s.sample_rate.hz();
        let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).unwrap_or(name).to_string();
        self.s.sources.push(Source {
            id,
            name: stem,
            path: path.to_string(),
            channels,
            frames,
            sample_rate: sr,
            format: FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        id
    }

    /// One source holding each of a clip's channels (silence where a channel's medium is
    /// offline), each lined up so the clip's offset reads them all.
    fn combined(&mut self, c: &AClip, key: &[Option<(usize, u16, i64)>]) -> Option<SourceId> {
        let sr = self.s.sample_rate.hz();
        let mut bufs: Vec<Option<(Arc<AudioBuffer>, u16, i64)>> = Vec::with_capacity(key.len());
        for k in key {
            let b = k.and_then(|(m, ch, d)| self.audio(m).map(|buf| (buf, ch, d)));
            if b.is_none()
                && let Some((m, _, _)) = k
            {
                // Its own source records it offline (and in the summary).
                let _ = self.media_source(*m);
            }
            bufs.push(b);
        }
        if bufs.iter().all(Option::is_none) {
            let first = key.iter().flatten().next().map(|(m, _, _)| *m);
            let path = first.and_then(|m| match self.got(m) {
                Some(Got::Offline { path, .. }) => Some(path.clone()),
                _ => None,
            });
            let frames = u64::try_from(c.offset.max(0) + c.len).unwrap_or(0);
            return Some(self.offline_source(&c.name, &path.unwrap_or_else(|| c.name.clone()), u16::try_from(key.len()).unwrap_or(1), frames));
        }
        // Frames: as long as the longest channel reaches.
        let frames = bufs.iter().flatten().map(|(b, _, d)| (b.frames() as i64 - d).max(0)).max().unwrap_or(0);
        if frames <= 0 || frames as usize * key.len() > crate::io::MAX_IMPORT_SAMPLES {
            return None;
        }
        let mut channels = Vec::with_capacity(key.len());
        for b in &bufs {
            let mut ch = vec![0.0f32; frames as usize];
            if let Some((buf, which, d)) = b
                && let Some(data) = buf.channels.get(usize::from(*which)).or(buf.channels.first())
            {
                for (i, x) in ch.iter_mut().enumerate() {
                    let at = i as i64 + d;
                    if at >= 0
                        && let Some(v) = data.get(at as usize)
                    {
                        *x = *v;
                    }
                }
            }
            channels.push(ch);
        }
        let buf = AudioBuffer { sample_rate: sr, channels };
        Some(crate::io::add_source(self.s, &c.name, buf, None, FileFormat::Wav))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_aaf::testwrite::{Item, MediaSpec, Spec, ramp, session};

    fn spec() -> Spec {
        Spec {
            name: "Reel 1".into(),
            rate: (25, 1),
            tc_start: Some(90_000),
            tracks: vec![
                (
                    "Dialog".into(),
                    vec![
                        Item::Filler(25),
                        Item::Clip { media: 0, start: 5, len: 50, gain: Some(0.5) },
                        Item::Transition(10),
                        Item::Clip { media: 1, start: 0, len: 40, gain: None },
                        Item::Transition(5),
                        Item::Filler(20),
                    ],
                ),
                ("Music".into(), vec![Item::Stereo { left: 0, right: 1, start: 0, len: 25 }]),
            ],
            media: vec![
                MediaSpec { name: "tone".into(), rate: 48_000, samples: ramp(48_000 * 3), url: None },
                MediaSpec { name: "voice".into(), rate: 48_000, samples: ramp(48_000 * 2), url: None },
            ],
            markers: vec![(50, "Scene 2".into())],
        }
    }

    #[test]
    fn an_aaf_comes_in_as_tracks_with_clips_fades_and_gain_in_one_undo_step() {
        let mut e = Engine::default();
        let steps = e.undo_history().len();
        let bytes = session(&spec(), 512);
        let dir = std::env::temp_dir().join(format!("freedawm-aaf-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("reel1.aaf");
        std::fs::write(&path, &bytes).unwrap();
        let v = e.execute("file.import_aaf", &json!({"path": path.to_string_lossy()})).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(e.undo_history().len(), steps + 1, "one undo step");
        assert_eq!(v["clips"], 3, "{v}");
        assert!(v["summary"].as_str().unwrap().contains("2 tracks, 3 clips, 1 marker"), "{v}");
        let s = e.session();
        // An empty session takes the composition's timecode: 01:00:00:00 at 25 fps.
        assert_eq!(s.timecode_start, 90_000 * 1920);
        assert_eq!(s.frame_rate, FrameRate::Fps25);
        let dialog = s.tracks.iter().find(|t| t.name == "Dialog").unwrap();
        let clips = dialog.clips();
        assert_eq!(clips.len(), 2);
        let (a, b) = (&clips[0], &clips[1]);
        assert_eq!((a.start, a.length), (25 * 1920, 50 * 1920));
        assert!(matches!(a.content, soundcraft_model::ClipContent::Audio { offset, .. } if offset == 5 * 1920));
        assert!((a.gain_db + 6.02).abs() < 0.05, "{}", a.gain_db);
        assert_eq!(a.fade_out.len, 10 * 1920, "a crossfade: the clips overlap");
        assert_eq!(b.fade_in.len, 10 * 1920);
        assert_eq!(b.start, 65 * 1920);
        assert_eq!(b.fade_out.len, 5 * 1920);
        assert_eq!(a.fade_out.shape, FadeShape::Linear);
        // The embedded audio is in the session, unsaved until the session is saved.
        let src = a.source().unwrap();
        assert!(s.source(src).unwrap().unsaved);
        assert!(s.pool.get(src).is_some());
        let music = s.tracks.iter().find(|t| t.name == "Music").unwrap();
        assert_eq!(music.format, ChannelFormat::Stereo);
        let m = &music.clips()[0];
        assert_eq!(s.source(m.source().unwrap()).unwrap().channels, 2, "two mono media combined");
        assert_eq!(s.markers.len(), 1);
        assert_eq!(s.markers[0].name, "Scene 2");
        assert_eq!(s.markers[0].start, 50 * 1920);
        assert!(s.notes.iter().any(|n| n.tags.contains(&"aaf".to_string())));
        // Undo takes all of it away.
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().tracks.iter().all(|t| t.name != "Dialog"));
    }

    #[test]
    fn a_session_with_clips_places_the_composition_by_timecode() {
        let mut e = crate::demo::demo_engine();
        let before = e.session().timecode_start;
        let v = import_bytes(&mut e, &session(&spec(), 4096), "x.aaf", None, None, false).unwrap();
        assert_eq!(e.session().timecode_start, before, "the session's timecode is kept");
        let t = TrackId(v["tracks"][0].as_u64().unwrap());
        let first = e.session().track(t).unwrap().clips()[0].start;
        assert_eq!(first, 90_000 * 1920 + 25 * 1920 - before);
        // `at` puts the composition's start where asked.
        let v = import_bytes(&mut e, &session(&spec(), 512), "x.aaf", None, Some(1000), false).unwrap();
        let t = TrackId(v["tracks"][0].as_u64().unwrap());
        assert_eq!(e.session().track(t).unwrap().clips()[0].start, 1000 + 25 * 1920);
    }

    #[test]
    fn missing_external_media_imports_offline_with_its_name() {
        let mut sp = spec();
        sp.media[1].url = Some("file:///nowhere/at/all/voice.wav".into());
        let mut e = Engine::default();
        let v = import_bytes(&mut e, &session(&sp, 512), "x.aaf", None, None, false).unwrap();
        assert_eq!(v["offline"].as_array().unwrap().len(), 1, "{v}");
        assert!(v["summary"].as_str().unwrap().contains("1 file offline"));
        let s = e.session();
        let dialog = s.tracks.iter().find(|t| t.name == "Dialog").unwrap();
        let b = &dialog.clips()[1];
        let src = s.source(b.source().unwrap()).unwrap();
        assert_eq!(src.name, "voice");
        assert_eq!(src.path, "/nowhere/at/all/voice.wav");
        assert!(s.pool.get(src.id).is_none(), "no audio: offline");
        assert!(!src.unsaved);
    }

    #[test]
    fn external_media_beside_the_aaf_is_found() {
        let dir = std::env::temp_dir().join(format!("freedawm-aaf-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Audio")).unwrap();
        let pcm: Vec<u8> = ramp(96_000).iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(dir.join("Audio").join("voice.wav"), soundcraft_aaf::walk::pcm_wav(&pcm, 1, 48_000, 16)).unwrap();
        let mut sp = spec();
        sp.media[1].url = Some("file:///C:/Editor/Project/Audio/voice.wav".into());
        let mut e = Engine::default();
        let v = import_bytes(&mut e, &session(&sp, 512), "x.aaf", Some(&dir), None, false).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(v["offline"].as_array().unwrap().is_empty(), "{v}");
        let s = e.session();
        let dialog = s.tracks.iter().find(|t| t.name == "Dialog").unwrap();
        let src = dialog.clips()[1].source().unwrap();
        assert!(s.pool.get(src).is_some());
        assert!(s.source(src).unwrap().path.ends_with("voice.wav"));
    }

    /// Every sample file (`FREEDAWM_AAF_SAMPLES`, see `crates/aaf/tests/samples.rs`) imports into
    /// an empty session without an error, with as many tracks and clips as the reader finds.
    #[test]
    fn every_sample_file_imports() {
        let Some(dir) = std::env::var_os("FREEDAWM_AAF_SAMPLES") else {
            println!("FREEDAWM_AAF_SAMPLES is not set: the AAF sample import is skipped");
            return;
        };
        let dir = std::path::Path::new(&dir).join("aaf");
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
        files.retain(|p| p.extension().is_some_and(|x| x == "aaf"));
        files.sort();
        let mut lines = Vec::new();
        for f in &files {
            let mut e = Engine::default();
            let v = e.execute("file.import_aaf", &json!({"path": f.to_string_lossy()})).unwrap_or_else(|err| panic!("{}: {err}", f.display()));
            let clips: usize = e.session().tracks.iter().map(|t| t.clips().len()).sum();
            assert_eq!(clips, v["clips"].as_u64().unwrap() as usize);
            lines.push(format!(
                "{}: {} tracks, {clips} clips, {} offline",
                f.file_name().unwrap().to_string_lossy(),
                v["tracks"].as_array().unwrap().len(),
                v["offline"].as_array().unwrap().len()
            ));
        }
        println!("{}", lines.join("\n"));
    }

    #[test]
    fn bad_files_are_errors_and_change_nothing() {
        let mut e = Engine::default();
        let good = session(&spec(), 512);
        for bad in [&b""[..], b"not an aaf", &good[..good.len() / 3]] {
            let before = e.session().clone();
            assert!(import_bytes(&mut e, bad, "bad.aaf", None, None, false).is_err() || e.session().tracks.len() <= 2);
            let _ = before;
        }
        let steps = e.undo_history().len();
        assert!(e.execute("file.import_aaf", &json!({"path": "/no/such/file.aaf"})).is_err());
        assert_eq!(e.undo_history().len(), steps);
        // Corrupted copies never panic.
        let mut x: u32 = 7;
        for _ in 0..300 {
            let mut b = good.clone();
            for _ in 0..3 {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let at = (x as usize >> 3) % b.len();
                b[at] ^= (x >> 23) as u8 | 1;
            }
            let mut e = Engine::default();
            let _ = import_bytes(&mut e, &b, "c.aaf", None, None, false);
        }
    }
}
