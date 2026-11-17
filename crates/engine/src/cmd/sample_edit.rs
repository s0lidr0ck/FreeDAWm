//! The sample editor's edits (Clip › Sample Editor, and the Pencil on audio at sample zoom): the
//! samples of one clip's file redrawn, silenced or turned over inside the clip's window. Each
//! writes a copy of the file with the change and points the clip at it (as Process does), so
//! the original file stays as it was for every other clip, and Undo puts the clip back. One
//! undo step per stroke or edit.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, ClipContent, ClipId};

/// Most samples one pencil stroke writes (ten seconds at 48 kHz: a stroke is drawn at sample zoom).
pub const MAX_DRAWN: usize = 480_000;

/// Samples at each end of a stroke over which the drawn line blends in from (and back out to)
/// the file, so the stroke's edges make no click.
const BLEND: usize = 8;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.draw_samples",
            "Draw Samples",
            [],
            None,
            "{clip, at: timeline position of the first value, values: [f32 -1..1], channel?: n (default: every channel)} — the Pencil at sample level: the clip's file samples from `at` are the values drawn (blended over the first and last 8), written to a copy of the file the clip then plays",
            always,
            draw
        ),
        cmd!(
            "clip.sample_edit",
            "Sample Edit",
            [],
            None,
            "{clip, op: silence | reverse | invert | gain, start, end (timeline), db?, channel?} — changes the clip's file samples between start and end (inside the clip), in a copy of the file the clip then plays",
            always,
            sample_edit
        ),
    ]
}

/// The clip `p` names, when its file maps straight onto the timeline (no stretch, warp, loop or
/// reverse): the samples drawn over a timeline position are the file's at the same place.
fn plain_audio_clip(e: &Engine, p: &Value, id: &str) -> Result<Clip> {
    let cid = p.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(id, "`clip` required"))?;
    let (_, c) = e.session().find_clip(cid).ok_or_else(|| bad(id, "no such clip"))?;
    if !c.is_audio() {
        return Err(bad(id, "not an audio clip"));
    }
    if c.stretch != 1.0 || !c.warp.is_empty() || c.loop_len != 0 || c.reversed {
        return Err(bad(id, "the clip is stretched, looped or reversed: commit it first (Clip › Consolidate)"));
    }
    if e.session().pool.get(c.source().unwrap_or(soundcraft_model::SourceId(0))).is_none() {
        return Err(bad(id, "the clip's file is missing"));
    }
    Ok(c.clone())
}

/// The channel `p` names (`None`: every one).
fn channel(p: &Value, channels: usize, id: &str) -> Result<Option<usize>> {
    match p.get("channel").and_then(Value::as_u64) {
        None => Ok(None),
        Some(c) => usize::try_from(c).ok().filter(|c| *c < channels).map(Some).ok_or_else(|| bad(id, format!("the file has {channels} channel(s)"))),
    }
}

/// Write `buf` (the clip's whole file, changed) as a new file, and point clip `c` at it.
fn replace_file(e: &mut Engine, c: &Clip, buf: Vec<Vec<f32>>, what: &str) {
    let s = e.session_mut();
    let rate = s.sample_rate.hz();
    let base = c.source().and_then(|x| s.source(x)).map_or_else(|| c.name.clone(), |x| x.name.clone());
    let name = format!("{base}-{what}");
    let src = crate::io::add_source(s, &name, AudioBuffer { sample_rate: rate, channels: buf }, None, FileFormat::Wav);
    if let Some(cl) = s.find_clip_mut(c.id)
        && let ClipContent::Audio { source, .. } = &mut cl.content
    {
        *source = src;
    }
}

/// The clip's file, whole, one buffer per channel.
fn whole_file(e: &Engine, c: &Clip) -> Vec<Vec<f32>> {
    c.source().and_then(|x| e.session().pool.get(x)).map(|a| a.buffer().into_owned().channels).unwrap_or_default()
}

/// File frame of timeline position `at` in clip `c` (plain: no stretch).
fn file_frame(c: &Clip, at: Samples) -> Samples {
    c.source_offset().saturating_add(at.saturating_sub(c.start))
}

fn draw(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.draw_samples";
    let c = plain_audio_clip(e, p, C)?;
    let at = p.get("at").and_then(Value::as_i64).ok_or_else(|| bad(C, "`at` required"))?;
    let values: Vec<f32> = p
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(C, "`values` required"))?
        .iter()
        .map(|v| v.as_f64().filter(|x| x.is_finite()).map_or(0.0, |x| x.clamp(-1.0, 1.0) as f32))
        .collect();
    if values.is_empty() {
        return Err(bad(C, "nothing drawn"));
    }
    if values.len() > MAX_DRAWN {
        return Err(bad(C, format!("a stroke draws at most {MAX_DRAWN} samples: zoom in")));
    }
    // Only inside the clip.
    let n = i64::try_from(values.len()).unwrap_or(i64::MAX);
    let (from, to) = (at.max(c.start), at.saturating_add(n).min(c.end()));
    if to <= from {
        return Err(bad(C, "the stroke is outside the clip"));
    }
    let mut buf = whole_file(e, &c);
    let ch = channel(p, buf.len(), C)?;
    let skip = usize::try_from(from - at).unwrap_or(0);
    let count = usize::try_from(to - from).unwrap_or(0);
    let first = usize::try_from(file_frame(&c, from)).unwrap_or(0);
    let blend = BLEND.min(count / 4);
    for (k, chan) in buf.iter_mut().enumerate() {
        if ch.is_some_and(|x| x != k) {
            continue;
        }
        for i in 0..count {
            let (Some(o), Some(v)) = (chan.get_mut(first + i), values.get(skip + i)) else { continue };
            // Blend in over the first samples and out over the last.
            let edge = i.min(count - 1 - i);
            let w = if blend == 0 || edge >= blend { 1.0 } else { (edge as f32 + 1.0) / (blend as f32 + 1.0) };
            *o = *o * (1.0 - w) + v * w;
        }
    }
    replace_file(e, &c, buf, "pencil");
    Ok(json!({"clip": c.id.0, "drawn": count}))
}

fn sample_edit(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.sample_edit";
    let c = plain_audio_clip(e, p, C)?;
    let op = str_param(p, "op").unwrap_or("");
    if !matches!(op, "silence" | "reverse" | "invert" | "gain") {
        return Err(bad(C, "`op` is silence, reverse, invert or gain"));
    }
    let start = p.get("start").and_then(Value::as_i64).ok_or_else(|| bad(C, "`start` required"))?.max(c.start);
    let end = p.get("end").and_then(Value::as_i64).ok_or_else(|| bad(C, "`end` required"))?.min(c.end());
    if end <= start {
        return Err(bad(C, "select samples inside the clip"));
    }
    let gain = 10f32.powf(f64_or(p, "db", 0.0).clamp(-96.0, 24.0) as f32 / 20.0);
    let mut buf = whole_file(e, &c);
    let ch = channel(p, buf.len(), C)?;
    let a = usize::try_from(file_frame(&c, start)).unwrap_or(0);
    let b = usize::try_from(file_frame(&c, end)).unwrap_or(0);
    for (k, chan) in buf.iter_mut().enumerate() {
        if ch.is_some_and(|x| x != k) {
            continue;
        }
        let end = b.min(chan.len());
        let Some(span) = chan.get_mut(a..end) else { continue };
        match op {
            "silence" => span.iter_mut().for_each(|x| *x = 0.0),
            "reverse" => span.reverse(),
            "invert" => span.iter_mut().for_each(|x| *x = -*x),
            _ => span.iter_mut().for_each(|x| *x = (*x * gain).clamp(-8.0, 8.0)),
        }
    }
    replace_file(e, &c, buf, op);
    Ok(json!({"clip": c.id.0, "frames": end - start}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Session, TrackKind};

    /// A clip at 1 000 over frames 100..1 100 of a file of 0.5s.
    fn engine() -> (Engine, ClipId, soundcraft_model::SourceId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 2_000]] };
        let src = crate::io::add_source(&mut s, "half", buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(id, "half", src, 100, 1_000, 1_000));
        (Engine::new(s), id, src)
    }

    fn file(e: &Engine, id: ClipId) -> (soundcraft_model::SourceId, Vec<f32>) {
        let (_, c) = e.session().find_clip(id).unwrap();
        let src = c.source().unwrap();
        (src, e.session().pool.get(src).unwrap().buffer().channels[0].clone())
    }

    #[test]
    fn the_pencil_redraws_samples_in_a_copy_of_the_file() {
        let (mut e, id, original) = engine();
        // 40 samples of -1 from timeline 1 010: file frames 110..150, blended at the ends.
        e.execute("clip.draw_samples", &json!({"clip": id.0, "at": 1_010, "values": vec![-1.0; 40]})).unwrap();
        let (src, f) = file(&e, id);
        assert_ne!(src, original, "a new file");
        assert_eq!(f[109], 0.5);
        assert!(f[110] > -1.0 && f[110] < 0.5, "blended in: {}", f[110]);
        assert_eq!(&f[118..142], &[-1.0; 24][..], "drawn");
        assert!(f[149] > -1.0, "blended out");
        assert_eq!(f[150], 0.5);
        assert!(e.session().pool.get(original).unwrap().buffer().channels[0].iter().all(|x| *x == 0.5), "the original is untouched");
        assert!(e.undo());
        assert_eq!(file(&e, id).0, original, "one undo step");
        // Outside the clip: refused; partly outside: cut to it.
        assert!(e.execute("clip.draw_samples", &json!({"clip": id.0, "at": 0, "values": [0.0, 0.0]})).is_err());
        let r = e.execute("clip.draw_samples", &json!({"clip": id.0, "at": 1_990, "values": vec![0.0; 40]})).unwrap();
        assert_eq!(r["drawn"], 10);
    }

    #[test]
    fn sample_edits_change_only_the_span() {
        let (mut e, id, _) = engine();
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "silence", "start": 1_100, "end": 1_200})).unwrap();
        let (_, f) = file(&e, id);
        assert_eq!((f[199], f[200], f[299], f[300]), (0.5, 0.0, 0.0, 0.5));
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "invert", "start": 1_000, "end": 1_010})).unwrap();
        assert_eq!(file(&e, id).1[100], -0.5);
        assert!(e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "explode", "start": 1_000, "end": 1_010})).is_err());
    }
}
