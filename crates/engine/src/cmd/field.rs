//! Field recordings (parity section 8, the field-recorder workflow, steps 2 and 3).
//!
//! A clip of the edit (often the picture editor's mixed-down dialogue) keeps the timecode of the
//! location recording it came from (its file's BWF time reference) and, from step 1, the scene
//! and take of its iXML ([`Session::field_info`]). From these:
//!
//! - **Expand to Location Channels** (`clip.expand_channels`) finds the recordings of the same
//!   moment: files in the session or in a folder (by default the folder of the clip's file) whose
//!   timecode overlaps the clip and whose scene and take match its file's, and the other
//!   channels of the clip's own file when it holds several. Each channel goes on a new mono
//!   track below the clip's, named by the recorder's channel name, cut to the part of the clip
//!   it covers. Files from the folder are read only when they match (their headers are read to
//!   decide), and join the session as imported files do.
//! - **Alternate Takes** (`clip.alternate_takes`, `clip.use_alternate_take`): the other takes of
//!   the clip's scene, and the clip playing one of them instead from the same place in the file.
//!   Where a recorder writes one file per channel, the take's file with the same channel names
//!   is chosen.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::FileFormat;
use soundcraft_audio_io::ixml::{IxmlInfo, read_field_header};
use soundcraft_model::{ChannelFormat, Clip, ClipContent, Session, SourceId, TrackKind};
use std::path::{Path, PathBuf};

const EXPAND: &str = "clip.expand_channels";
const ALTERNATES: &str = "clip.alternate_takes";
const USE_ALTERNATE: &str = "clip.use_alternate_take";

/// The most files of a folder looked at.
const MAX_FILES: usize = 4096;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.expand_channels",
            "Expand to Location Channels",
            [],
            None,
            "{clip?: id (default: the selected audio clip), folder?: path (default: the folder of the clip's file)} — each channel of the location recordings of the clip's moment (files in the session or the folder whose timecode overlaps the clip and whose scene and take match its file's, and the other channels of its own file) goes on a new mono track below the clip's, cut to the clip -> {tracks, clips, files}",
            always,
            expand
        ),
        cmd!(
            query "clip.alternate_takes",
            "Alternate Takes",
            [],
            None,
            "{clip?, folder?} — the other takes of the clip's scene (files in the session or the folder with the same scene and another take) -> {scene, take, takes: [{take, name, path?, source?}]}",
            always,
            alternates
        ),
        cmd!(
            "clip.use_alternate_take",
            "Use Alternate Take",
            [],
            None,
            "{clip?, take, folder?} — the clip plays that take of its scene instead, from the same place in the file (shorter when the take is) -> {source, name}",
            always,
            use_alternate
        ),
    ]
}

/// A location recording: a file of the session or of the folder.
#[derive(Debug, Clone)]
struct Take {
    name: String,
    /// Its id when the session has it.
    source: Option<SourceId>,
    path: Option<PathBuf>,
    channels: u16,
    /// The file's own rate (the session's for a source).
    file_rate: u32,
    /// Length and timecode (BWF time reference; 0 = none), at the session's rate.
    frames: i64,
    tc: i64,
    info: IxmlInfo,
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn i64_of(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// The file a session source's stored path names, when it can be found.
fn source_file(e: &Engine, stored: &str) -> Option<PathBuf> {
    match e.path.as_deref() {
        Some(session) => crate::io::stored_path(stored, Path::new(session)).ok(),
        None => Some(PathBuf::from(stored)).filter(|p| p.is_absolute()),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

fn source_take(e: &Engine, s: &Session, id: SourceId) -> Option<Take> {
    let src = s.source(id)?;
    Some(Take {
        name: src.name.clone(),
        source: Some(id),
        path: source_file(e, &src.path),
        channels: src.channels,
        file_rate: s.sample_rate.hz(),
        frames: i64_of(src.frames),
        tc: i64_of(src.time_reference),
        info: s.field_info.get(&id.0).cloned().unwrap_or_default(),
    })
}

/// Every location recording known: the session's files that have a timecode or iXML (and whose
/// audio is there), then the folder's WAV files the session does not have.
fn takes(e: &Engine, folder: Option<&Path>) -> Vec<Take> {
    let s = e.session();
    let rate = s.sample_rate.hz();
    let mut out: Vec<Take> = s
        .sources
        .iter()
        .filter(|src| s.pool.get(src.id).is_some() && (src.time_reference > 0 || s.field_info.contains_key(&src.id.0)))
        .filter_map(|src| source_take(e, s, src.id))
        .collect();
    let Some(dir) = folder else { return out };
    let mut files: Vec<PathBuf> = crate::io::list_dir(dir)
        .into_iter()
        .filter(|f| f.extension().and_then(|x| x.to_str()).is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "wav" | "bwf")))
        .collect();
    files.sort();
    files.truncate(MAX_FILES);
    for f in files {
        if crate::file_access::check_read(&f).is_err() || out.iter().any(|t| t.path.as_deref().is_some_and(|p| same_file(p, &f))) {
            continue;
        }
        let Some(h) = read_field_header(&f) else { continue };
        if h.frames == 0 {
            continue;
        }
        out.push(Take {
            name: f.file_stem().and_then(|x| x.to_str()).unwrap_or("Take").to_string(),
            source: None,
            channels: h.channels,
            file_rate: h.sample_rate,
            frames: i64_of(crate::io::stamp_at_rate(h.frames, h.sample_rate, rate)),
            tc: i64_of(crate::io::stamp_at_rate(h.time_reference, h.sample_rate, rate)),
            info: h.ixml.unwrap_or_default(),
            path: Some(f),
        });
    }
    out
}

/// The clip a command acts on (`clip`, else the first selected audio clip), its track, its
/// file as a [`Take`], and the folder to look in (`folder`, else its file's).
fn origin(e: &Engine, p: &Value, cmd: &str) -> Result<(TrackId, Clip, Take, Option<PathBuf>)> {
    let s = e.session();
    let id = match p.get("clip") {
        Some(_) => clip_id(p, cmd)?,
        None => s
            .edit
            .selected_clips
            .iter()
            .copied()
            .find(|c| s.find_clip(*c).is_some_and(|(_, x)| x.is_audio()))
            .ok_or_else(|| bad(cmd, "select an audio clip, or pass `clip`"))?,
    };
    let (track, clip) = s.find_clip(id).map(|(t, c)| (t, c.clone())).ok_or_else(|| bad(cmd, format!("no clip with id {}", id.0)))?;
    let src = clip.source().filter(|_| clip.is_audio()).ok_or_else(|| bad(cmd, "not an audio clip"))?;
    let own = source_take(e, s, src).ok_or_else(|| bad(cmd, "the clip's audio file is not in the session"))?;
    let folder = match str_param(p, "folder").map(str::trim).filter(|x| !x.is_empty()) {
        Some(f) => Some(PathBuf::from(f)),
        None => own.path.as_deref().and_then(Path::parent).filter(|d| d.is_dir()).map(Path::to_path_buf),
    };
    Ok((track, clip, own, folder))
}

/// The source of `t`: the session's, or the file read and added as an import adds it.
fn load(e: &mut Engine, cmd: &str, t: &Take) -> Result<(SourceId, u16, i64)> {
    if let Some(id) = t.source {
        return Ok((id, t.channels, t.frames));
    }
    let path = t.path.clone().ok_or_else(|| bad(cmd, format!("`{}` has no file", t.name)))?;
    let bytes = crate::io::read_file(&path)?;
    let rate = e.session().sample_rate.hz();
    let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let buf = crate::io::decode_buffer(&bytes, ext.as_deref(), rate).map_err(|err| bad(cmd, format!("{}: {err}", path.display())))?;
    if buf.frames() == 0 {
        return Err(bad(cmd, format!("{}: no audio", path.display())));
    }
    let copy = e.copy_imported_audio;
    let s = e.session_mut();
    let file = path.file_name().and_then(|x| x.to_str()).unwrap_or("take.wav").to_string();
    let id = crate::io::add_source(s, &file, buf, Some(&path.to_string_lossy()), FileFormat::Wav);
    if let Some(src) = s.sources.iter_mut().find(|x| x.id == id) {
        src.time_reference = u64::try_from(t.tc).unwrap_or(0);
        // Not copied: the file is referred to where it is when it holds the session's rate.
        if !copy && t.file_rate == rate {
            src.unsaved = false;
        }
    }
    if t.info != IxmlInfo::default() {
        s.field_info.insert(id.0, t.info.clone());
    }
    let (channels, frames) = s.source(id).map_or((t.channels, t.frames), |x| (x.channels, i64_of(x.frames)));
    Ok((id, channels, frames))
}

/// Whether `t`'s scene and take are those of `own` (each only when `own` says it).
fn same_scene_take(own: &IxmlInfo, t: &IxmlInfo) -> bool {
    (own.scene.trim().is_empty() || same_text(&own.scene, &t.scene)) && (own.take.trim().is_empty() || same_text(&own.take, &t.take))
}

fn expand(e: &mut Engine, p: &Value) -> Result<Value> {
    let (track, clip, own, folder) = origin(e, p, EXPAND)?;
    if clip.stretch != 1.0 || !clip.warp.is_empty() {
        return Err(bad(EXPAND, "a stretched or warped clip no longer lines up with its timecode"));
    }
    if own.tc <= 0 {
        return Err(bad(EXPAND, format!("`{}` has no timecode (a BWF time reference) to match recordings by", own.name)));
    }
    let a = own.tc.saturating_add(clip.source_offset());
    let b = a.saturating_add(clip.length);
    let mut found: Vec<Take> = Vec::new();
    // The clip's own file's other channels first.
    if own.channels >= 2 {
        found.push(own.clone());
    }
    let mut others: Vec<Take> = takes(e, folder.as_deref())
        .into_iter()
        .filter(|t| t.source != own.source && t.tc > 0 && t.tc < b && t.tc.saturating_add(t.frames) > a && same_scene_take(&own.info, &t.info))
        .collect();
    others.sort_by(|x, y| x.name.cmp(&y.name));
    found.extend(others);
    if found.is_empty() {
        let place = folder.as_ref().map_or("the session".to_string(), |f| format!("the session or {}", f.display()));
        return Err(bad(EXPAND, format!("no location recordings of this clip's timecode, scene and take found in {place}")));
    }
    let mut tracks = Vec::new();
    let mut clips = Vec::new();
    let mut files = Vec::new();
    for t in &found {
        let (src, channels, frames) = load(e, EXPAND, t)?;
        let (o0, o1) = (a.max(t.tc), b.min(t.tc.saturating_add(frames)));
        if o1 <= o0 {
            continue;
        }
        files.push(t.name.clone());
        let s = e.session_mut();
        for ch in 0..channels {
            let named = t.info.channels.get(usize::from(ch)).map(|x| x.trim()).filter(|x| !x.is_empty());
            let name = match named {
                Some(n) => n.to_string(),
                None if channels == 1 => t.name.clone(),
                None => format!("{} {}", t.name, ch + 1),
            };
            let id = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&name));
            // Below the clip's track, in the order found.
            if let (Some(at), Some(from)) = (s.track_index(track), s.track_index(id)) {
                let moved = s.tracks.remove(from);
                s.tracks.insert((at + 1 + tracks.len()).min(s.tracks.len()), moved);
            }
            let cid = s.new_clip_id();
            let mut c = Clip::audio(cid, name, src, o0 - t.tc, clip.start.saturating_add(o0 - a), o1 - o0);
            c.source_channel = (channels > 1).then_some(ch);
            if (o0, o1) == (a, b) {
                (c.fade_in, c.fade_out) = (clip.fade_in, clip.fade_out);
            }
            crate::edit::place_clip(s, id, c);
            tracks.push(id);
            clips.push(cid);
        }
    }
    Ok(json!({"tracks": tracks, "clips": clips, "files": files}))
}

/// A take's number for ordering ("3" before "12"), then its text.
fn take_order(t: &str) -> (u64, String) {
    let digits: String = t.trim().chars().take_while(char::is_ascii_digit).collect();
    (digits.parse().unwrap_or(u64::MAX), t.trim().to_lowercase())
}

/// The other takes of the clip's scene: one file per take (the one with the clip's file's
/// channel names, else its channel count, else the first by name), in take order.
fn alternate_list(e: &Engine, p: &Value, cmd: &str) -> Result<(Clip, Take, Vec<Take>)> {
    let (_, clip, own, folder) = origin(e, p, cmd)?;
    if own.info.scene.trim().is_empty() {
        return Err(bad(cmd, format!("`{}` names no scene (no iXML from a field recorder)", own.name)));
    }
    let mut all: Vec<Take> = takes(e, folder.as_deref())
        .into_iter()
        .filter(|t| {
            t.source != own.source
                && same_text(&t.info.scene, &own.info.scene)
                && !t.info.take.trim().is_empty()
                && !same_text(&t.info.take, &own.info.take)
        })
        .collect();
    all.sort_by(|x, y| x.name.cmp(&y.name));
    let mut out: Vec<Take> = Vec::new();
    for t in all {
        let rank = |x: &Take| u8::from(x.info.channels != own.info.channels) * 2 + u8::from(x.channels != own.channels);
        match out.iter_mut().find(|x| same_text(&x.info.take, &t.info.take)) {
            Some(x) if rank(&t) < rank(x) => *x = t,
            Some(_) => {}
            None => out.push(t),
        }
    }
    out.sort_by_key(|t| take_order(&t.info.take));
    Ok((clip, own, out))
}

fn alternates(e: &mut Engine, p: &Value) -> Result<Value> {
    let (_, own, list) = alternate_list(e, p, ALTERNATES)?;
    let takes: Vec<Value> = list
        .iter()
        .map(|t| json!({"take": t.info.take, "name": t.name, "path": t.path.as_ref().map(|x| x.display().to_string()), "source": t.source}))
        .collect();
    Ok(json!({"scene": own.info.scene, "take": own.info.take, "takes": takes}))
}

fn use_alternate(e: &mut Engine, p: &Value) -> Result<Value> {
    let want = str_param(p, "take").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(USE_ALTERNATE, "`take` required"))?.to_string();
    let (clip, _, list) = alternate_list(e, p, USE_ALTERNATE)?;
    let t =
        list.into_iter().find(|t| same_text(&t.info.take, &want)).ok_or_else(|| bad(USE_ALTERNATE, format!("no take {want} of this scene found")))?;
    let (src, channels, frames) = load(e, USE_ALTERNATE, &t)?;
    let s = e.session_mut();
    let c = s.find_clip_mut(clip.id).ok_or_else(|| bad(USE_ALTERNATE, "the clip is gone"))?;
    // The same place in the take as in the file it played; a shorter take shortens the clip.
    let offset = if c.source_offset() < frames { c.source_offset() } else { 0 };
    c.content = ClipContent::Audio { source: src, offset };
    c.length = c.length.min(frames - offset).max(1);
    c.name = t.name.clone();
    c.source_channel = c.source_channel.filter(|ch| *ch < channels);
    // What was made from the other file's audio no longer applies.
    (c.clean, c.tune) = (None, None);
    Ok(json!({"source": src, "name": t.name}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_audio_io::{AudioBuffer, BitDepth, BwfInfo, EncodeOptions};

    /// A WAV of `channels` channels, each a constant (0.1 × channel number + `base`), stamped
    /// at `tc` with the iXML of `scene`/`take` and channel names.
    fn field_wav(dir: &Path, name: &str, tc: u64, frames: usize, scene: &str, take: &str, names: &[&str], base: f32) -> PathBuf {
        let buf = AudioBuffer { sample_rate: 48_000, channels: (0..names.len()).map(|c| vec![base + 0.1 * (c as f32 + 1.0); frames]).collect() };
        let opts =
            EncodeOptions { bit_depth: BitDepth::Float32, bwf: Some(BwfInfo { time_reference: tc, ..Default::default() }), ..Default::default() };
        let mut bytes = soundcraft_audio_io::encode(&buf, &opts).unwrap();
        let tracks: String =
            names.iter().enumerate().map(|(i, n)| format!("<TRACK><CHANNEL_INDEX>{}</CHANNEL_INDEX><NAME>{n}</NAME></TRACK>", i + 1)).collect();
        let xml = format!("<BWFXML><SCENE>{scene}</SCENE><TAKE>{take}</TAKE><TRACK_LIST>{tracks}</TRACK_LIST></BWFXML>");
        bytes.extend_from_slice(b"iXML");
        bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
        bytes.extend_from_slice(xml.as_bytes());
        let riff = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    fn folder(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("field-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn import(e: &mut Engine, path: &Path) -> (TrackId, soundcraft_model::ClipId) {
        let bytes = std::fs::read(path).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        let v = crate::io::import_audio_bytes(e, name, &bytes, Some(&path.to_string_lossy()), None, 48_000).unwrap();
        (serde_json::from_value(v["track"].clone()).unwrap(), serde_json::from_value(v["clip"].clone()).unwrap())
    }

    #[test]
    fn a_clip_expands_to_the_location_channels_of_its_moment() {
        let dir = folder("expand");
        let tc = 48_000 * 3600;
        // The edit's clip: a mono mixdown of scene 14 take 2, stamped one second into the take.
        let mix = field_wav(&dir, "mix.wav", tc + 48_000, 24_000, "14", "2", &["Mix"], 0.0);
        // The recorder's polyphonic file of that take, another take, and another scene.
        field_wav(&dir, "14_2.wav", tc, 96_000, "14", "2", &["Boom", "Lav Anna"], 0.0);
        field_wav(&dir, "14_3.wav", tc + 200_000, 96_000, "14", "3", &["Boom", "Lav Anna"], 0.4);
        field_wav(&dir, "15_1.wav", tc, 96_000, "15", "1", &["Boom"], 0.7);
        let mut e = Engine::default();
        let (track, clip) = import(&mut e, &mix);
        let n_before = e.session().tracks.len();
        let out = e.execute(EXPAND, &json!({"clip": clip.0})).unwrap();
        assert_eq!(out["files"], json!(["14_2"]), "only the take of the clip's scene, take and timecode");
        let ids: Vec<TrackId> = serde_json::from_value(out["tracks"].clone()).unwrap();
        let s = e.session();
        assert_eq!(s.tracks.len(), n_before + 2);
        let at = s.track_index(track).unwrap();
        let names: Vec<&str> = ids.iter().map(|t| s.track(*t).unwrap().name.as_str()).collect();
        assert_eq!(names, ["Boom", "Lav Anna"], "named by the recorder's channel names");
        assert_eq!((s.track_index(ids[0]), s.track_index(ids[1])), (Some(at + 1), Some(at + 2)), "below the clip's track");
        for (k, t) in ids.iter().enumerate() {
            let c = &s.track(*t).unwrap().clips()[0];
            assert_eq!((c.start, c.length, c.source_offset()), (48_000, 24_000, 48_000), "cut to the clip, at its moment in the take");
            assert_eq!(c.source_channel, Some(k as u16));
            let heard = soundcraft_mix::render_clips(s, *t, soundcraft_time::Range::new(48_000, 48_100));
            assert!((heard[0][50] - 0.1 * (k as f32 + 1.0)).abs() < 1e-4, "channel {k} plays: {}", heard[0][50]);
        }
        let src = s.track(ids[0]).unwrap().clips()[0].source().unwrap();
        assert_eq!(s.source(src).unwrap().time_reference, tc);
        assert_eq!(s.field_info.get(&src.0).unwrap().take, "2", "the file joins the session with its iXML");
        // One undo step takes it all away.
        assert!(e.undo());
        assert_eq!(e.session().tracks.len(), n_before);
        // Without a timecode there is nothing to match.
        let mut plain = Engine::default();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 100]] };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).unwrap();
        let v = crate::io::import_audio_bytes(&mut plain, "x.wav", &bytes, None, None, 0).unwrap();
        assert!(plain.execute(EXPAND, &json!({"clip": v["clip"]})).is_err());
        // Splitting the polyphonic file's track names the tracks by its channels.
        let (poly, _) = import(&mut e, &dir.join("14_2.wav"));
        let out = e.execute("track.split_into_mono", &json!({"tracks": [poly.0]})).unwrap();
        let ids: Vec<TrackId> = serde_json::from_value(out["tracks"].clone()).unwrap();
        let names: Vec<&str> = ids.iter().map(|t| e.session().track(*t).unwrap().name.as_str()).collect();
        assert_eq!(names, ["Boom", "Lav Anna"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clip_switches_to_another_take_of_its_scene() {
        let dir = folder("alt");
        let tc = 48_000 * 7200;
        // A recorder writing one file per channel: boom and lav for takes 1, 2 and 12 of scene 9.
        let boom1 = field_wav(&dir, "9_1_boom.wav", tc, 48_000, "9", "1", &["Boom"], 0.0);
        field_wav(&dir, "9_1_lav.wav", tc, 48_000, "9", "1", &["Lav"], 0.1);
        field_wav(&dir, "9_12_boom.wav", tc + 900_000, 48_000, "9", "12", &["Boom"], 0.2);
        field_wav(&dir, "9_2_lav.wav", tc + 500_000, 30_000, "9", "2", &["Lav"], 0.3);
        field_wav(&dir, "9_2_boom.wav", tc + 500_000, 30_000, "9", "2", &["Boom"], 0.4);
        field_wav(&dir, "8_4_boom.wav", tc, 48_000, "8", "4", &["Boom"], 0.5);
        let mut e = Engine::default();
        let (track, clip) = import(&mut e, &boom1);
        let list = e.execute(ALTERNATES, &json!({"clip": clip.0})).unwrap();
        assert_eq!((list["scene"].as_str(), list["take"].as_str()), (Some("9"), Some("1")));
        let takes: Vec<(&str, &str)> =
            list["takes"].as_array().unwrap().iter().map(|t| (t["take"].as_str().unwrap(), t["name"].as_str().unwrap())).collect();
        assert_eq!(takes, [("2", "9_2_boom"), ("12", "9_12_boom")], "in take order, the boom's file of each");
        // Trim the clip to start 40,000 into its file and run 8,000: take 2 is 30,000 long.
        let s = e.session_mut();
        let c = s.find_clip_mut(clip).unwrap();
        c.content = ClipContent::Audio { source: c.source().unwrap(), offset: 20_000 };
        c.length = 15_000;
        let out = e.execute(USE_ALTERNATE, &json!({"clip": clip.0, "take": "2"})).unwrap();
        assert_eq!(out["name"], "9_2_boom");
        let s = e.session();
        let c = s.find_clip(clip).unwrap().1;
        assert_eq!((c.source_offset(), c.length, c.name.as_str()), (20_000, 10_000, "9_2_boom"), "the same place, shortened to the take");
        let heard = soundcraft_mix::render_clips(s, track, soundcraft_time::Range::new(48_000, 48_100));
        assert!((heard[0][50] - 0.5).abs() < 1e-4, "the boom of take 2 plays: {}", heard[0][50]);
        assert!(e.undo());
        assert_eq!(e.session().find_clip(clip).unwrap().1.length, 15_000);
        assert!(e.execute(USE_ALTERNATE, &json!({"clip": clip.0, "take": "7"})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
