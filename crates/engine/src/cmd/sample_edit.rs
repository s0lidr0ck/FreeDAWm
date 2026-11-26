//! The sample editor's edits (Clip › Sample Editor, and the Pencil on audio at sample zoom): the
//! samples of one clip's file redrawn, silenced or turned over inside the clip's window. Each
//! writes a copy of the file with the change and points the clip at it (as Process does), so
//! the original file stays as it was for every other clip, and Undo puts the clip back. One
//! undo step per stroke or edit.

use super::*;
use crate::TransportRequest;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, ClipContent, ClipId, SourceAudio};
use std::sync::Arc;

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
            "{clip, op: silence | reverse | invert | gain | cut | paste, start, end (timeline, inside the clip) or file_start, file_end (frames of the whole file), db?, channel?} — changes the clip's file samples in the span, in a copy of the file the clip then plays. `cut` removes the span (kept in the sample clipboard) and `paste` puts the sample clipboard in its place (at file_start when the span is empty), on every channel; the clip's window follows the audio it showed, growing or shrinking by the change inside it (up to the next clip)",
            always,
            sample_edit
        ),
        cmd!(
            noundo "clip.sample_copy",
            "Sample Copy",
            [],
            None,
            "{clip, start, end | file_start, file_end} — copies the span's samples (every channel) to the Sample Editor's clipboard → {frames}",
            always,
            sample_copy
        ),
        cmd!(
            noundo "clip.sample_loop",
            "Sample Loop",
            [],
            None,
            "{clip, start, end | file_start, file_end} — plays the span looped where the clip sits on the timeline (the part inside the clip's window) → {start, end}",
            always,
            sample_loop
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

/// A change to a clip's file: the new file is `old[..a]`, then `m` frames the edit makes, then
/// `old[b..]`.
struct Splice {
    a: usize,
    b: usize,
    m: usize,
}

/// What an edit makes of the middle of a [`Splice`]: `mid(ch, k, out)` fills `out` with its
/// frames from `k` on.
type Middle<'a> = dyn Fn(usize, usize, &mut [f32]) + 'a;

/// Frames `[start, start + out.len())` of channel `ch` of `audio` changed by `sp`.
fn read_spliced(audio: &SourceAudio, sp: &Splice, mid: &Middle<'_>, ch: usize, start: usize, out: &mut [f32]) {
    let mut i = 0usize;
    while i < out.len() {
        let x = start.saturating_add(i);
        let Some(rest) = out.get_mut(i..) else { return };
        let mid_end = sp.a.saturating_add(sp.m);
        let n = if x < sp.a {
            (sp.a - x).min(rest.len())
        } else if x < mid_end {
            (mid_end - x).min(rest.len())
        } else {
            rest.len()
        };
        let Some(dst) = rest.get_mut(..n) else { return };
        if x < sp.a {
            audio.read(ch, x, dst);
        } else if x < mid_end {
            mid(ch, x - sp.a, dst);
        } else {
            audio.read(ch, (x - mid_end).saturating_add(sp.b), dst);
        }
        i += n.max(1);
    }
}

/// Writes the clip's file changed by `sp` as a new file and points clip `c` at it: straight to
/// disk a block at a time where the session has a folder for generated audio (a long file is
/// never copied into memory), else in memory as before.
fn replace_file(e: &mut Engine, c: &Clip, sp: &Splice, mid: &Middle<'_>, what: &str) -> Result<()> {
    let audio = c.source().and_then(|x| e.session().pool.get(x)).cloned().ok_or_else(|| bad("clip.sample_edit", "the clip's file is missing"))?;
    let channels = audio.num_channels();
    let frames = audio.frames().saturating_sub(sp.b.saturating_sub(sp.a)).saturating_add(sp.m);
    let s = e.session();
    let rate = s.sample_rate.hz();
    let base = c.source().and_then(|x| s.source(x)).map_or_else(|| c.name.clone(), |x| x.name.clone());
    let name = format!("{base}-{what}");
    let mut read = |at: usize, planar: &mut [Vec<f32>], n: usize| {
        for (ch, buf) in planar.iter_mut().enumerate() {
            if let Some(dst) = buf.get_mut(..n) {
                read_spliced(&audio, sp, mid, ch, at, dst);
            }
        }
    };
    let src = match crate::spill::write_generated(e, "Sample Edit", &name, (channels, frames), &mut read) {
        Some(src) => src,
        None => {
            let buf: Vec<Vec<f32>> = (0..channels)
                .map(|ch| {
                    let mut v = vec![0.0; frames];
                    read_spliced(&audio, sp, mid, ch, 0, &mut v);
                    v
                })
                .collect();
            crate::io::add_source(e.session_mut(), &name, AudioBuffer { sample_rate: rate, channels: buf }, None, FileFormat::Wav)
        }
    };
    if let Some(cl) = e.session_mut().find_clip_mut(c.id)
        && let ClipContent::Audio { source, .. } = &mut cl.content
    {
        *source = src;
    }
    Ok(())
}

/// The channel count and frames of the clip's file.
fn file_size(e: &Engine, c: &Clip) -> (usize, usize) {
    c.source().and_then(|x| e.session().pool.get(x)).map_or((0, 0), |a| (a.num_channels(), a.frames()))
}

/// The clip's file, read as it is (for an edit's middle).
fn old_audio(e: &Engine, c: &Clip) -> Option<Arc<SourceAudio>> {
    c.source().and_then(|x| e.session().pool.get(x)).cloned()
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
    let (channels, frames) = file_size(e, &c);
    let ch = channel(p, channels, C)?;
    let skip = usize::try_from(from - at).unwrap_or(0);
    let count = usize::try_from(to - from).unwrap_or(0);
    let first = usize::try_from(file_frame(&c, from)).unwrap_or(0).min(frames);
    let span = count.min(frames - first);
    let blend = BLEND.min(count / 4);
    let old = old_audio(e, &c).ok_or_else(|| bad(C, "the clip's file is missing"))?;
    let mid = |k: usize, i0: usize, out: &mut [f32]| {
        old.read(k, first + i0, out);
        if ch.is_some_and(|x| x != k) {
            return;
        }
        for (j, o) in out.iter_mut().enumerate() {
            let i = i0 + j;
            let Some(v) = values.get(skip + i) else { continue };
            // Blend in over the first samples and out over the last.
            let edge = i.min(count - 1 - i);
            let w = if blend == 0 || edge >= blend { 1.0 } else { (edge as f32 + 1.0) / (blend as f32 + 1.0) };
            *o = *o * (1.0 - w) + v * w;
        }
    };
    replace_file(e, &c, &Splice { a: first, b: first + span, m: span }, &mid, "pencil")?;
    Ok(json!({"clip": c.id.0, "drawn": count}))
}

/// The span `p` names, in frames of the clip's file: `file_start`/`file_end` anywhere in the
/// file, else timeline `start`/`end` inside the clip. Empty spans are allowed (a paste point).
fn file_span(e: &Engine, c: &Clip, p: &Value, id: &str) -> Result<(usize, usize)> {
    let frames = c.source().and_then(|x| e.session().source(x)).map_or(0, |x| i64::try_from(x.frames).unwrap_or(i64::MAX));
    let (a, b) = if let Some(a) = p.get("file_start").and_then(Value::as_i64) {
        let b = p.get("file_end").and_then(Value::as_i64).ok_or_else(|| bad(id, "`file_end` required with `file_start`"))?;
        (a.clamp(0, frames), b.clamp(0, frames))
    } else {
        let start = p.get("start").and_then(Value::as_i64).ok_or_else(|| bad(id, "`start` required"))?.max(c.start);
        let end = p.get("end").and_then(Value::as_i64).ok_or_else(|| bad(id, "`end` required"))?.min(c.end());
        (file_frame(c, start).clamp(0, frames), file_frame(c, end.max(start)).clamp(0, frames))
    };
    if b < a {
        return Err(bad(id, "the span ends before it starts"));
    }
    Ok((usize::try_from(a).unwrap_or(0), usize::try_from(b).unwrap_or(0)))
}

/// Where file frame `x` lands after `cut` frames are taken out at `at` and `ins` put in at `at`
/// (`end`: an edge that ends a window, which takes in material put exactly at it).
fn remap(x: i64, at: i64, cut: i64, ins: i64, end: bool) -> i64 {
    let x = if x <= at {
        x
    } else if x < at.saturating_add(cut) {
        at
    } else {
        x - cut
    };
    if x > at || (end && x == at) { x.saturating_add(ins) } else { x }
}

fn sample_edit(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.sample_edit";
    let c = plain_audio_clip(e, p, C)?;
    let op = str_param(p, "op").unwrap_or("");
    if !matches!(op, "silence" | "reverse" | "invert" | "gain" | "cut" | "paste") {
        return Err(bad(C, "`op` is silence, reverse, invert, gain, cut or paste"));
    }
    let (a, b) = file_span(e, &c, p, C)?;
    if b <= a && op != "paste" {
        return Err(bad(C, "select samples first"));
    }
    if matches!(op, "cut" | "paste") {
        return cut_paste(e, &c, op, a, b);
    }
    let gain = 10f32.powf(f64_or(p, "db", 0.0).clamp(-96.0, 24.0) as f32 / 20.0);
    let (channels, frames) = file_size(e, &c);
    let ch = channel(p, channels, C)?;
    let (a2, b2) = (a.min(frames), b.min(frames));
    let old = old_audio(e, &c).ok_or_else(|| bad(C, "the clip's file is missing"))?;
    let mid = |k: usize, i0: usize, out: &mut [f32]| {
        if ch.is_some_and(|x| x != k) {
            old.read(k, a2 + i0, out);
            return;
        }
        match op {
            "silence" => out.fill(0.0),
            "reverse" => {
                // Frame `i` of the span is frame `b - 1 - i` of the file.
                let end = b2.saturating_sub(i0);
                old.read(k, end.saturating_sub(out.len()), out);
                out.reverse();
            }
            "invert" => {
                old.read(k, a2 + i0, out);
                out.iter_mut().for_each(|x| *x = -*x);
            }
            _ => {
                old.read(k, a2 + i0, out);
                out.iter_mut().for_each(|x| *x = (*x * gain).clamp(-8.0, 8.0));
            }
        }
    };
    replace_file(e, &c, &Splice { a: a2, b: b2, m: b2 - a2 }, &mid, op)?;
    Ok(json!({"clip": c.id.0, "frames": b - a}))
}

/// Cut `a..b` out of the clip's file (into the sample clipboard), or paste the sample clipboard
/// in its place; the clip's window keeps the audio it showed, and grows or shrinks by what
/// changed inside it, never past the next clip on its track.
fn cut_paste(e: &mut Engine, c: &Clip, op: &str, a: usize, b: usize) -> Result<Value> {
    const C: &str = "clip.sample_edit";
    let (channels, frames) = file_size(e, c);
    let old = old_audio(e, c).ok_or_else(|| bad(C, "the clip's file is missing"))?;
    let (a2, b2) = (a.min(frames), b.min(frames));
    // What is cut goes to the sample clipboard (only then is it read).
    let cut: Vec<Vec<f32>> = if op == "cut" {
        (0..channels)
            .map(|ch| {
                let mut v = vec![0.0; b2 - a2];
                old.read(ch, a2, &mut v);
                v
            })
            .collect()
    } else {
        Vec::new()
    };
    let clip = if op == "paste" { e.sample_clipboard.clone() } else { Vec::new() };
    let ins = if op == "paste" {
        if clip.is_empty() || clip.iter().all(Vec::is_empty) {
            return Err(bad(C, "nothing copied: copy or cut samples first"));
        }
        clip.iter().map(Vec::len).max().unwrap_or(0)
    } else {
        0
    };
    if channels == 0 || frames.saturating_sub(b2 - a2).saturating_add(ins) == 0 {
        return Err(bad(C, "that would leave the file empty"));
    }
    let mid = |k: usize, i0: usize, out: &mut [f32]| {
        let from = clip.get(k % clip.len().max(1));
        for (j, o) in out.iter_mut().enumerate() {
            *o = from.and_then(|f| f.get(i0 + j)).copied().unwrap_or(0.0);
        }
    };
    let (at, cutn, insn) = (i64::try_from(a).unwrap_or(0), i64::try_from(b - a).unwrap_or(0), i64::try_from(ins).unwrap_or(0));
    let off = c.source_offset();
    let new_off = remap(off, at, cutn, insn, false);
    let new_end = remap(off.saturating_add(c.length), at, cutn, insn, true);
    if new_end <= new_off {
        return Err(bad(C, "that would leave the clip empty: select less, or delete the clip"));
    }
    // Never over the next clip on the track.
    let room = e
        .session()
        .find_clip(c.id)
        .and_then(|(t, _)| e.session().track(t))
        .and_then(|t| t.clips().iter().filter(|x| x.id != c.id && x.start > c.start).map(|x| x.start).min())
        .map_or(Samples::MAX, |next| next - c.start);
    let len = (new_end - new_off).min(room.max(1));
    replace_file(e, c, &Splice { a: a2, b: b2, m: ins }, &mid, op)?;
    if let Some(cl) = e.session_mut().find_clip_mut(c.id) {
        if let ClipContent::Audio { offset, .. } = &mut cl.content {
            *offset = new_off;
        }
        let start = cl.start;
        cl.trim_end_to(start.saturating_add(len));
    }
    if op == "cut" {
        e.sample_clipboard = cut;
    }
    Ok(json!({"clip": c.id.0, "frames": b - a, "inserted": ins, "length": len}))
}

fn sample_copy(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.sample_copy";
    let cid = p.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(C, "`clip` required"))?;
    let c = e.session().find_clip(cid).map(|(_, c)| c.clone()).filter(Clip::is_audio).ok_or_else(|| bad(C, "no such audio clip"))?;
    let (a, b) = file_span(e, &c, p, C)?;
    if b <= a {
        return Err(bad(C, "select samples first"));
    }
    let audio = c.source().and_then(|x| e.session().pool.get(x)).cloned().ok_or_else(|| bad(C, "the clip's file is missing"))?;
    let copied: Vec<Vec<f32>> = (0..audio.num_channels())
        .map(|ch| {
            let mut v = vec![0.0; b - a];
            audio.read(ch, a, &mut v);
            v
        })
        .collect();
    e.sample_clipboard = copied;
    Ok(json!({"frames": b - a}))
}

fn sample_loop(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.sample_loop";
    let cid = p.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(C, "`clip` required"))?;
    let c = plain_audio_clip(e, &json!({"clip": cid.0}), C)?;
    let (a, b) = file_span(e, &c, p, C)?;
    let at = |x: usize| c.start.saturating_add(i64::try_from(x).unwrap_or(i64::MAX).saturating_sub(c.source_offset()));
    let (start, end) = (at(a).max(c.start), at(b).min(c.end()));
    if end <= start {
        return Err(bad(C, "the selection is outside the clip's window on the timeline"));
    }
    let r = soundcraft_time::Range::new(start, end);
    e.transport_requests.push(TransportRequest::PlayRange { from: start, end: None, looped: Some(r) });
    Ok(json!({"start": start, "end": end}))
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

    /// A file of 2 000 frames counting up (frame i holds i/10 000), the clip over 100..1 100 at 1 000.
    fn ramp() -> (Engine, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![(0..2_000).map(|i| i as f32 / 10_000.0).collect()] };
        let src = crate::io::add_source(&mut s, "ramp", buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(id, "ramp", src, 100, 1_000, 1_000));
        (Engine::new(s), id)
    }

    fn at(f: &[f32], i: usize) -> i32 {
        (f[i] * 10_000.0).round() as i32
    }

    #[test]
    fn cut_takes_samples_out_and_the_clip_shrinks() {
        let (mut e, id) = ramp();
        // Timeline 1 100..1 300 = file 200..400.
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "cut", "start": 1_100, "end": 1_300})).unwrap();
        let (_, f) = file(&e, id);
        assert_eq!(f.len(), 1_800);
        assert_eq!((at(&f, 199), at(&f, 200)), (199, 400), "joined across the cut");
        let c = e.session().find_clip(id).unwrap().1.clone();
        assert_eq!((c.source_offset(), c.length), (100, 800), "the window keeps its audio, 200 shorter");
        assert_eq!(e.sample_clipboard[0].len(), 200);
        assert_eq!((e.sample_clipboard[0][0] * 10_000.0).round() as i32, 200);
        assert!(e.undo());
        assert_eq!(e.session().find_clip(id).unwrap().1.length, 1_000, "one undo step");
    }

    #[test]
    fn copy_and_paste_insert_and_replace() {
        let (mut e, id) = ramp();
        e.execute("clip.sample_copy", &json!({"clip": id.0, "file_start": 0, "file_end": 50})).unwrap();
        // Inserted at file 300 (an empty span): the clip grows by 50.
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "paste", "file_start": 300, "file_end": 300})).unwrap();
        let (_, f) = file(&e, id);
        assert_eq!(f.len(), 2_050);
        assert_eq!((at(&f, 299), at(&f, 300), at(&f, 349), at(&f, 350)), (299, 0, 49, 300));
        let c = e.session().find_clip(id).unwrap().1.clone();
        assert_eq!((c.source_offset(), c.length), (100, 1_050));
        // Replacing 10 frames with the 50 copied.
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "paste", "file_start": 400, "file_end": 410})).unwrap();
        let (_, f) = file(&e, id);
        assert_eq!(f.len(), 2_090);
        assert_eq!((at(&f, 399), at(&f, 400), at(&f, 449), at(&f, 450)), (349, 0, 49, 360));
        // Nothing copied in a fresh engine: refused.
        let (mut e2, id2) = ramp();
        assert!(e2.execute("clip.sample_edit", &json!({"clip": id2.0, "op": "paste", "file_start": 0, "file_end": 0})).is_err());
    }

    #[test]
    fn the_whole_file_is_editable_beyond_the_window() {
        let (mut e, id) = ramp();
        // Before the window: frames 0..50 silenced, then cut 0..50: the window slides back.
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "silence", "file_start": 0, "file_end": 50})).unwrap();
        assert_eq!(at(&file(&e, id).1, 49), 0);
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "cut", "file_start": 0, "file_end": 50})).unwrap();
        let c = e.session().find_clip(id).unwrap().1.clone();
        assert_eq!((c.source_offset(), c.length), (50, 1_000), "same audio, same length");
        assert_eq!(at(&file(&e, id).1, 50), 100, "the clip still starts on frame 100's sample");
        // After the window: the clip is unchanged.
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "cut", "file_start": 1_500, "file_end": 1_600})).unwrap();
        let c = e.session().find_clip(id).unwrap().1.clone();
        assert_eq!((c.source_offset(), c.length), (50, 1_000));
        // Cutting the whole window is refused.
        assert!(e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "cut", "file_start": 0, "file_end": 1_200})).is_err());
    }

    #[test]
    fn a_paste_stops_at_the_next_clip() {
        let (mut e, id) = ramp();
        let t = e.session().find_clip(id).unwrap().0;
        let s = e.session_mut();
        let src = s.find_clip(id).unwrap().1.source().unwrap();
        let next = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(next, "next", src, 0, 2_020, 100));
        e.execute("clip.sample_copy", &json!({"clip": id.0, "file_start": 0, "file_end": 100})).unwrap();
        e.execute("clip.sample_edit", &json!({"clip": id.0, "op": "paste", "file_start": 500, "file_end": 500})).unwrap();
        assert_eq!(e.session().find_clip(id).unwrap().1.end(), 2_020);
    }

    #[test]
    fn the_selection_loops_where_the_clip_sits() {
        let (mut e, id) = ramp();
        let r = e.execute("clip.sample_loop", &json!({"clip": id.0, "file_start": 50, "file_end": 300})).unwrap();
        assert_eq!((r["start"].as_i64(), r["end"].as_i64()), (Some(1_000), Some(1_200)), "cut to the window");
        assert_eq!(
            e.transport_requests.last(),
            Some(&TransportRequest::PlayRange { from: 1_000, end: None, looped: Some(soundcraft_time::Range::new(1_000, 1_200)) })
        );
        assert!(e.execute("clip.sample_loop", &json!({"clip": id.0, "file_start": 1_500, "file_end": 1_600})).is_err());
    }
}
