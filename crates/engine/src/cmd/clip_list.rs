//! Clip List commands.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Clip, ClipContent, Session, SourceId};
use std::collections::BTreeSet;

const CLEAR: &str = "clip.clear";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.relink_audio",
            "Relink Audio File...",
            [],
            None,
            "{source: id, path} — the session's audio file `source` (a missing one, or any) is read from `path` instead; its clips play it from where they were -> {frames, warning?}",
            always,
            relink
        ),
        cmd!(
            "clip.clear",
            "Clear Clips",
            [],
            Some("Cmd+Shift+B"),
            "{clips?: [id], sources?: [id], with_clips?: bool} — removes clips and audio files from the session (files stay on disk); default: the Clip List selection. A file still used on the timeline needs `with_clips: true`, which removes those clips too",
            has_list_selection,
            clear
        ),
        cmd!(
            "file.compact",
            "Compact Audio",
            [],
            None,
            "{sources?: [id], handle_ms?: 1000} — each audio file (default: every one) is made again holding only the parts its clips use, `handle_ms` either side, and its clips point at it; the old file leaves the session (it stays on disk). Files of stretched, looped, reversed, tuned or cleaned clips, frozen tracks and instruments are left whole -> {compacted, frames_before, frames_after}",
            always,
            compact
        ),
    ]
}

/// Every clip of the session: on every playlist, kept in the Clip List, and the clips process
/// histories keep ([`soundcraft_model::ProcessStep::before`]).
fn each_clip_mut(s: &mut Session, f: &mut impl FnMut(&mut Clip)) {
    fn with_history(c: &mut Clip, f: &mut impl FnMut(&mut Clip)) {
        for step in &mut c.processes {
            f(&mut step.before);
        }
        f(c);
    }
    for c in s.tracks.iter_mut().flat_map(|t| t.playlists.iter_mut()).flat_map(|p| p.clips.iter_mut()) {
        with_history(c, f);
    }
    for c in &mut s.captured {
        with_history(c, f);
    }
}

/// The audio files compacting leaves whole: used by a frozen track, an insert or an instrument,
/// remembered by a stretch (Remove Warp's original) or being stretched now, or a cleaned
/// version of a clip.
fn kept_whole(s: &Session) -> BTreeSet<SourceId> {
    let mut out: BTreeSet<SourceId> = BTreeSet::new();
    for t in &s.tracks {
        out.extend(t.freeze.iter().map(|f| f.source));
        out.extend(t.mixer.inserts.iter().flat_map(|i| i.sources()));
        out.extend(t.instrument.iter().flat_map(|i| i.sources()));
    }
    out.extend(s.edit.values.iter().filter(|(k, _)| k.starts_with("elastic.") && k.ends_with(".src")).map(|(_, v)| SourceId(*v as u64)));
    out.extend(s.live_stretch.keys().copied());
    out.extend(s.all_clips().filter_map(|c| c.clean_alt().map(|k| k.source)));
    out
}

/// Whether compacting can map `c`'s window straight into a new file: plain audio, no stretch,
/// loop, reverse, tuning, cleaned version or lyrics tied to file positions.
fn plain(c: &Clip) -> bool {
    c.stretch == 1.0 && c.warp.is_empty() && c.loop_len == 0 && !c.reversed && c.tune.is_none() && c.clean.is_none() && c.lyrics.is_empty()
}

/// A compacted file keeps at least this share less than the file, or it is left as it is.
const COMPACT_SAVES_AT_LEAST: f64 = 0.1;

fn compact(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "file.compact";
    let only: Option<BTreeSet<SourceId>> = p.get("sources").map(|v| ids_in::<Vec<SourceId>>(v, C)).transpose()?.map(|v| v.into_iter().collect());
    let handle_ms = p.get("handle_ms").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(1000.0).clamp(0.0, 60_000.0);
    let s = e.session();
    let handle = s.sample_rate.samples(handle_ms / 1000.0);
    let whole = kept_whole(s);
    // Each file's users, with the history's clips.
    let mut users: std::collections::BTreeMap<SourceId, Vec<Clip>> = std::collections::BTreeMap::new();
    let mut tmp = s.clone();
    each_clip_mut(&mut tmp, &mut |c| {
        if let Some(src) = c.source().filter(|_| c.is_audio()) {
            users.entry(src).or_default().push(c.clone());
        }
    });
    let mut plans: Vec<(SourceId, Vec<(Samples, Samples)>)> = Vec::new();
    let (mut before, mut after) = (0i64, 0i64);
    for src in &s.sources {
        let id = src.id;
        if only.as_ref().is_some_and(|o| !o.contains(&id)) || whole.contains(&id) {
            continue;
        }
        let (Some(clips), Some(audio)) = (users.get(&id), s.pool.get(id)) else { continue };
        if !clips.iter().all(plain) {
            continue;
        }
        let frames = i64::try_from(audio.frames()).unwrap_or(0);
        let mut spans: Vec<(Samples, Samples)> = clips
            .iter()
            .map(|c| (c.source_offset().saturating_sub(handle).max(0), c.source_offset().saturating_add(c.length).saturating_add(handle).min(frames)))
            .filter(|(a, b)| b > a)
            .collect();
        spans.sort_unstable();
        let mut merged: Vec<(Samples, Samples)> = Vec::new();
        for (a, b) in spans {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        let kept: i64 = merged.iter().map(|(a, b)| b - a).sum();
        if (kept as f64) > frames as f64 * (1.0 - COMPACT_SAVES_AT_LEAST) || kept <= 0 {
            continue;
        }
        before = before.saturating_add(frames);
        after = after.saturating_add(kept);
        plans.push((id, merged));
    }
    if plans.is_empty() {
        return Err(bad(C, "nothing to compact: every file's clips use most of it (or it is stretched, looped, frozen or an instrument's)"));
    }
    for (old, spans) in &plans {
        let s = e.session();
        let (Some(audio), Some(info)) = (s.pool.get(*old), s.source(*old)) else { continue };
        let name = format!("{} (compacted)", info.name);
        let channels = usize::from(info.channels).max(1);
        let total = usize::try_from(spans.iter().map(|(a, b)| b - a).sum::<i64>()).unwrap_or(0);
        let mut out = vec![vec![0.0f32; total]; channels];
        let mut at = 0usize;
        // Where each span starts in the new file.
        let mut bases = Vec::with_capacity(spans.len());
        for (a, b) in spans {
            let n = usize::try_from(b - a).unwrap_or(0);
            for (ch, buf) in out.iter_mut().enumerate() {
                if let Some(dst) = buf.get_mut(at..at + n) {
                    audio.read(ch, usize::try_from(*a).unwrap_or(0), dst);
                }
            }
            bases.push((*a, *b, i64::try_from(at).unwrap_or(0)));
            at += n;
        }
        let rate = s.sample_rate.hz();
        let s = e.session_mut();
        let new = crate::io::add_source(
            s,
            &name,
            soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels: out },
            None,
            soundcraft_audio_io::FileFormat::Wav,
        );
        each_clip_mut(s, &mut |c| {
            if let ClipContent::Audio { source, offset } = &mut c.content
                && *source == *old
                && let Some((a, _, base)) = bases.iter().find(|(a, b, _)| *offset >= *a && *offset < *b)
            {
                (*source, *offset) = (new, base.saturating_add(*offset - a));
            }
        });
        if !s.all_clips().any(|c| c.uses_source(*old)) {
            s.sources.retain(|x| x.id != *old);
            s.pool.remove(*old);
            s.edit.selected_sources.retain(|x| x != old);
        }
    }
    Ok(json!({"compacted": plans.len(), "frames_before": before, "frames_after": after}))
}

/// Relink: the file at `path` stands in for source `source`, decoded at the session rate, so
/// every clip of it plays again. A file of another length or width is taken, with a warning.
fn relink(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "file.relink_audio";
    let id = p.get("source").and_then(Value::as_u64).map(SourceId).ok_or_else(|| bad(C, "`source` id required"))?;
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(C, "`path` required"))?.to_string();
    let old = e.session().source(id).cloned().ok_or_else(|| bad(C, "no such audio file"))?;
    let bytes = crate::io::read_file(&path)?;
    let ext = std::path::Path::new(&path).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let rate = e.session().sample_rate.hz();
    let buf = crate::io::decode_buffer(&bytes, ext.as_deref(), rate).map_err(|err| bad(C, format!("{path}: {err}")))?;
    if buf.frames() == 0 {
        return Err(bad(C, format!("{path}: no audio")));
    }
    let (frames, channels) = (buf.frames() as u64, u16::try_from(buf.num_channels()).unwrap_or(u16::MAX));
    let s = e.session_mut();
    if let Some(src) = s.sources.iter_mut().find(|x| x.id == id) {
        src.path = path;
        (src.frames, src.channels, src.unsaved) = (frames, channels, false);
    }
    s.pool.insert(id, std::sync::Arc::new(soundcraft_model::SourceAudio::new(buf)));
    let mut out = json!({"frames": frames});
    if frames != old.frames || channels != old.channels {
        out["warning"] = json!(format!(
            "Relinked `{}`, but the new file has {frames} frames in {channels} channels (the old one {} in {})",
            old.name, old.frames, old.channels
        ));
    }
    Ok(out)
}

fn has_list_selection(e: &Engine) -> std::result::Result<(), String> {
    let ed = &e.session().edit;
    if ed.selected_clips.is_empty() && ed.selected_sources.is_empty() { Err("select clips or files in the Clip List".into()) } else { Ok(()) }
}

fn ids_in<'a, T: serde::Deserialize<'a>>(v: &'a Value, id: &str) -> Result<T> {
    T::deserialize(v).map_err(|_| bad(id, "`sources` must be an array of ids"))
}

fn ids<'a, T: serde::Deserialize<'a>>(p: &'a Value, key: &str) -> Result<Option<T>> {
    p.get(key).map(|v| T::deserialize(v).map_err(|_| bad(CLEAR, format!("`{key}` must be an array of ids")))).transpose()
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session();
    let (clips, sources): (BTreeSet<ClipId>, BTreeSet<SourceId>) = match (ids(p, "clips")?, ids(p, "sources")?) {
        (None, None) => (s.edit.selected_clips.iter().copied().collect(), s.edit.selected_sources.iter().copied().collect()),
        (c, f) => (c.unwrap_or_default(), f.unwrap_or_default()),
    };
    let known: BTreeSet<ClipId> = s.all_clips().map(|c| c.id).collect();
    if let Some(c) = clips.iter().find(|c| !known.contains(c)) {
        return Err(bad(CLEAR, format!("no clip with id {c}")));
    }
    let files = sources.iter().map(|f| s.source(*f).ok_or_else(|| bad(CLEAR, format!("no audio file with id {f}")))).collect::<Result<Vec<_>>>()?;
    if clips.is_empty() && files.is_empty() {
        return Err(bad(CLEAR, "nothing to clear: pass `clips` or `sources`, or select them in the Clip List"));
    }
    if !bool_or(p, "with_clips", false)
        && let Some((f, n)) = files.iter().map(|f| (f, s.clips_using(f.id).filter(|c| !clips.contains(&c.id)).count())).find(|(_, n)| *n > 0)
    {
        let plural = if n == 1 { "" } else { "s" };
        return Err(bad(CLEAR, format!("`{}` is used by {n} clip{plural} on the timeline; pass `with_clips: true` to remove them too", f.name)));
    }
    let gone: BTreeSet<ClipId> = clips.iter().copied().chain(sources.iter().flat_map(|f| s.clips_using(*f).map(|c| c.id))).collect();
    let s = e.session_mut();
    for pl in s.tracks.iter_mut().flat_map(|t| t.playlists.iter_mut()) {
        pl.clips.retain(|c| !gone.contains(&c.id));
    }
    s.sources.retain(|f| !sources.contains(&f.id));
    for f in &sources {
        s.pool.remove(*f);
    }
    s.edit.selected_clips.retain(|c| !gone.contains(c));
    s.edit.selected_sources.retain(|f| !sources.contains(f));
    Ok(json!({"clips": gone.len(), "sources": sources.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::{AudioBuffer, FileFormat};

    fn kick(e: &Engine) -> (SourceId, Vec<ClipId>) {
        let s = e.session();
        let f = s.sources.iter().find(|x| x.name == "Kick").map(|x| x.id).unwrap();
        (f, s.clips_using(f).map(|c| c.id).collect())
    }

    #[test]
    fn compact_keeps_only_what_the_clips_use_and_their_handles() {
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let ramp = AudioBuffer { sample_rate: 48_000, channels: vec![(0..100_000).map(|i| i as f32 / 100_000.0).collect()] };
        let src = crate::io::add_source(&mut s, "Ramp", ramp, None, FileFormat::Wav);
        for (off, start, len) in [(10_000, 0, 1_000), (60_000, 5_000, 2_000)] {
            let id = s.new_clip_id();
            crate::edit::place_clip(&mut s, t, Clip::audio(id, "r", src, off, start, len));
        }
        let mut e = Engine::new(s);
        // 10 ms handles: 480 frames either side.
        let r = e.execute("file.compact", &json!({"handle_ms": 10.0})).unwrap();
        assert_eq!(r, json!({"compacted": 1, "frames_before": 100_000, "frames_after": 1_960 + 2_960}));
        let s = e.session();
        assert!(s.source(src).is_none(), "the old file left the session");
        let clips: Vec<Clip> = s.tracks[0].clips().to_vec();
        assert_eq!(
            clips.iter().map(|c| (c.start, c.length, c.source_offset())).collect::<Vec<_>>(),
            vec![(0, 1_000, 480), (5_000, 2_000, 1_960 + 480)]
        );
        let new = s.pool.get(clips[0].source().unwrap()).unwrap().buffer();
        assert_eq!(new.channels[0].len(), 4_920);
        for (c, was) in clips.iter().zip([10_000, 60_000]) {
            let got = new.channels[0][usize::try_from(c.source_offset()).unwrap()];
            assert!((got - was as f32 / 100_000.0).abs() < 1e-6, "each clip plays what it played");
        }
        assert!(e.execute("file.compact", &json!({"handle_ms": 10.0})).is_err(), "nothing left to compact");
        assert!(e.undo());
        assert!(e.session().source(src).is_some(), "Undo brings the old file back");
    }

    #[test]
    fn clearing_a_clip_takes_it_off_the_timeline_and_keeps_its_file() {
        let mut e = crate::demo::demo_engine();
        let (f, clips) = kick(&e);
        let r = e.execute("clip.clear", &json!({"clips": [clips[0]]})).unwrap();
        assert_eq!(r, json!({"clips": 1, "sources": 0}));
        assert!(e.session().find_clip(clips[0]).is_none());
        assert!(e.session().source(f).is_some());
        assert!(e.undo());
        assert!(e.session().find_clip(clips[0]).is_some());
    }

    #[test]
    fn a_file_in_use_stays_unless_its_clips_go_too() {
        let mut e = crate::demo::demo_engine();
        e.execute("track.playlist_duplicate", &json!({"track": "Kick"})).unwrap();
        let (f, clips) = kick(&e);
        assert_eq!(clips.len(), 6);
        let err = e.execute("clip.clear", &json!({"sources": [f]})).unwrap_err();
        assert!(err.to_string().contains("used by 6 clips"), "{err}");
        let r = e.execute("clip.clear", &json!({"sources": [f], "with_clips": true})).unwrap();
        assert_eq!(r, json!({"clips": 6, "sources": 1}));
        let s = e.session();
        assert!(s.source(f).is_none() && !s.pool.contains(f));
        assert_eq!(s.clips_using(f).count(), 0);
        assert_eq!(e.undo_label(), Some("Clear Clips"));
        assert!(e.undo());
        assert!(e.session().pool.contains(f));
        assert_eq!(e.session().clips_using(f).count(), 6);
    }

    #[test]
    fn listing_a_files_clips_too_is_enough() {
        let mut e = crate::demo::demo_engine();
        let (f, clips) = kick(&e);
        let r = e.execute("clip.clear", &json!({"clips": clips, "sources": [f]})).unwrap();
        assert_eq!(r, json!({"clips": 3, "sources": 1}));
    }

    #[test]
    fn the_clip_list_selection_is_the_default_target() {
        let mut e = crate::demo::demo_engine();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4_800]] };
        let f = crate::io::add_source(e.session_mut(), "Spare", buf, None, FileFormat::Wav);
        let (_, clips) = kick(&e);
        e.execute("edit.select", &json!({"clips": [clips[0]]})).unwrap();
        let r = e.execute("edit.select", &json!({"sources": [f, f, 999_999]})).unwrap();
        assert_eq!(r["sources"], json!([f]));
        assert_eq!(r["clips"], json!([]));
        let r = e.execute("edit.select", &json!({"clips": [clips[0]]})).unwrap();
        assert_eq!(r["sources"], json!([]));
        e.execute("clip.clear", &json!({})).unwrap();
        assert!(e.session().find_clip(clips[0]).is_none());
        assert!(e.session().edit.selected_clips.is_empty());
        e.execute("edit.select", &json!({"sources": [f]})).unwrap();
        e.execute("clip.clear", &json!({})).unwrap();
        assert!(e.session().source(f).is_none());
        assert!(e.session().edit.selected_sources.is_empty());
    }

    #[test]
    fn bad_targets_are_errors() {
        let mut e = crate::demo::demo_engine();
        for p in [
            json!({}),
            json!({"clips": []}),
            json!({"clips": [999_999]}),
            json!({"sources": [999_999]}),
            json!({"sources": "Kick"}),
            json!({"clips": [-1]}),
        ] {
            assert!(e.execute("clip.clear", &p).is_err(), "{p}");
        }
    }

    #[test]
    fn deselect_all_clears_the_clip_list_selection() {
        let mut e = crate::demo::demo_engine();
        let (f, _) = kick(&e);
        e.execute("edit.select", &json!({"sources": [f]})).unwrap();
        e.execute("edit.select_none", &json!({})).unwrap();
        assert!(e.session().edit.selected_sources.is_empty());
    }
}
