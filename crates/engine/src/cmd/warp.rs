//! Warp markers on an audio clip: add, move and delete them, the material between two markers
//! stretching to fit, by the track's Elastic algorithm.
//!
//! A warped clip plays a render of its original material (`elastic.<clip>.src/off/len` in the
//! edit values, as Elastic Properties and Straighten keep it). [`soundcraft_model::Clip::warp`]
//! holds `(position in the original material, position in the render)` pairs. The render's first
//! sample sits at timeline `clip.start − (offset − roff)`, where `roff` is the clip's offset into
//! the render when it was made (`elastic.<clip>.roff`; Straighten's clips share one render), so a
//! trim or a move keeps the markers on their audio. Every marker edit renders again from the
//! original, so warps never stack.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ClipContent, ClipId, Session, SourceId};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "clip.warp_marker",
        "Warp Marker",
        [],
        None,
        "{clip, op: \"add\"|\"move\"|\"delete\", at?: position (add), index?: n (move, delete), to?: position (move)} — warp markers on an audio clip (the Warp track view): `add` puts one at a timeline position without changing the sound; `move` takes marker `index` to a timeline position and the material on each side stretches to fit (the first marker stays; the last one sets the clip's end); `delete` removes one and the material re-stretches between its neighbours. The stretch uses the track's Elastic algorithm (pitch-preserving on a plain track, speed on Varispeed)",
        always,
        warp_marker
    )]
}

/// The fewest samples between two warp markers.
pub const MIN_GAP: Samples = 32;

/// A clip's warp as it stands: its original material, its markers and the render's offset.
#[derive(Debug, Clone, PartialEq)]
pub struct WarpState {
    pub source: SourceId,
    pub offset: Samples,
    pub len: Samples,
    pub marks: Vec<(Samples, Samples)>,
    pub roff: Samples,
}

impl WarpState {
    /// Where the render's first sample is on the timeline, for `clip`.
    pub fn anchor(&self, clip: &soundcraft_model::Clip) -> Samples {
        clip.start.saturating_sub(clip.source_offset().saturating_sub(self.roff))
    }

    /// The timeline positions of the markers, for `clip`.
    pub fn positions(&self, clip: &soundcraft_model::Clip) -> Vec<Samples> {
        let a = self.anchor(clip);
        self.marks.iter().map(|m| a.saturating_add(m.1)).collect()
    }
}

/// The warp of audio clip `id`: its markers, or for a clip without any the identity warp of
/// what it plays (an Elastic render's whole render, or a plain clip's own window). `None` for a
/// clip that is not audio or whose original material is gone.
pub fn warp_state(s: &Session, id: ClipId) -> Option<WarpState> {
    let (_, c) = s.find_clip(id)?;
    let ClipContent::Audio { source, offset } = c.content else { return None };
    let v = |k: &str| s.edit.values.get(&format!("elastic.{}.{k}", id.0)).copied().filter(|x| x.is_finite());
    match super::clip::warp_original(s, id) {
        Some((src, off, len)) if !c.warp.is_empty() => {
            Some(WarpState { source: src, offset: off, len, marks: c.warp.clone(), roff: v("roff").map_or(offset, |x| x as Samples) })
        }
        Some((src, off, len)) => {
            // An Elastic render: the whole render from offset 0 stands for the whole original.
            let rendered = s.source(source).map(|x| i64::try_from(x.frames).unwrap_or(0))?.max(1);
            Some(WarpState { source: src, offset: off, len, marks: vec![(0, 0), (len, rendered)], roff: 0 })
        }
        None => {
            let material = c.source_span(c.length).max(1);
            Some(WarpState { source, offset, len: material, marks: vec![(0, 0), (material, c.length)], roff: offset })
        }
    }
}

fn warp_marker(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.warp_marker";
    let id = clip_id(p, C)?;
    let op = str_param(p, "op").ok_or_else(|| bad(C, "`op` required: add, move or delete"))?;
    let (track, clip) = e.session().find_clip(id).map(|(t, c)| (t, c.clone())).ok_or_else(|| bad(C, "no such clip"))?;
    let st = warp_state(e.session(), id).ok_or_else(|| bad(C, "warp markers go on audio clips whose audio is in the session"))?;
    let anchor = st.anchor(&clip);
    let index = || -> Result<usize> {
        let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad(C, "`index` required"))?;
        usize::try_from(i).ok().filter(|i| *i < st.marks.len()).ok_or_else(|| bad(C, "no such marker"))
    };
    match op {
        "add" => {
            let at = position_param(e, C, p, "at")?.ok_or_else(|| bad(C, "`at` required"))?;
            let pos = at.saturating_sub(anchor);
            let i = st.marks.iter().position(|m| m.1 >= pos).unwrap_or(st.marks.len());
            let (Some(a), Some(b)) = (i.checked_sub(1).and_then(|j| st.marks.get(j)), st.marks.get(i)) else {
                return Err(bad(C, "the position is not inside the warped material"));
            };
            if pos - a.1 < MIN_GAP || b.1 - pos < MIN_GAP {
                return Err(bad(C, "too close to another marker"));
            }
            let orig = a.0 + soundcraft_time::to_samples((pos - a.1) as f64 * (b.0 - a.0) as f64 / (b.1 - a.1).max(1) as f64);
            if orig <= a.0 || orig >= b.0 {
                return Err(bad(C, "too close to another marker"));
            }
            let mut marks = st.marks.clone();
            marks.insert(i, (orig, pos));
            // The sound does not change: only the markers (and, for a clip warped for the first
            // time, where its original material is) are kept.
            let s = e.session_mut();
            remember_original(s, id, &st);
            if let Some(c) = s.find_clip_mut(id) {
                c.warp = marks;
            }
            Ok(json!({"index": i, "markers": i_marks(s, id)}))
        }
        "move" => {
            let i = index()?;
            if i == 0 {
                return Err(bad(C, "the first marker holds the clip in place: move the clip instead"));
            }
            let to = position_param(e, C, p, "to")?.ok_or_else(|| bad(C, "`to` required"))?;
            let lo = st.marks.get(i - 1).map_or(0, |m| m.1 + MIN_GAP);
            let hi = st.marks.get(i + 1).map_or(Samples::MAX / 4, |m| m.1 - MIN_GAP);
            let pos = to.saturating_sub(anchor).clamp(lo, hi.max(lo));
            let mut marks = st.marks.clone();
            if let Some(m) = marks.get_mut(i) {
                m.1 = pos;
            }
            apply(e, track, id, &st, marks)?;
            Ok(json!({"index": i, "markers": i_marks(e.session(), id)}))
        }
        "delete" => {
            let i = index()?;
            if i == 0 || i + 1 == st.marks.len() {
                return Err(bad(C, "the first and last markers stay"));
            }
            let mut marks = st.marks.clone();
            marks.remove(i);
            apply(e, track, id, &st, marks)?;
            Ok(json!({"markers": i_marks(e.session(), id)}))
        }
        _ => Err(bad(C, "`op` is add, move or delete")),
    }
}

/// The markers' timeline positions now.
fn i_marks(s: &Session, id: ClipId) -> Vec<Samples> {
    match (warp_state(s, id), s.find_clip(id)) {
        (Some(st), Some((_, c))) => st.positions(c),
        _ => Vec::new(),
    }
}

/// Keep where clip `id`'s original material is, the first time it is warped.
fn remember_original(s: &mut Session, id: ClipId, st: &WarpState) {
    let key = |k: &str| format!("elastic.{}.{k}", id.0);
    if !s.edit.values.contains_key(&key("src")) {
        s.edit.values.insert(key("src"), st.source.0 as f64);
        s.edit.values.insert(key("off"), st.offset as f64);
        s.edit.values.insert(key("len"), st.len as f64);
    }
    s.edit.values.insert(key("roff"), st.roff as f64);
}

/// Render clip `id`'s original material through `marks` and play the render, keeping the clip's
/// start and (unless it reached the old end, when it follows the last marker) its length.
fn apply(e: &mut Engine, track: soundcraft_model::TrackId, id: ClipId, st: &WarpState, marks: Vec<(Samples, Samples)>) -> Result<()> {
    const C: &str = "clip.warp_marker";
    for w in marks.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        let r = (b.1 - a.1) as f64 / (b.0 - a.0).max(1) as f64;
        if b.0 <= a.0 || b.1 <= a.1 || !(0.1..=10.0).contains(&r) {
            return Err(bad(C, "a stretch between two markers must stay between 0.1 and 10 times"));
        }
    }
    let s = e.session_mut();
    let clip = s.find_clip(id).map(|(_, c)| c.clone()).ok_or_else(|| bad(C, "no such clip"))?;
    let out_len = marks.last().map_or(0, |m| m.1);
    let max = super::more_util::max_render(s);
    if out_len <= 0 || out_len > max {
        return Err(bad(C, "the warped clip would be too long"));
    }
    let mut base = clip.clone();
    base.content = ClipContent::Audio { source: st.source, offset: st.offset };
    base.stretch = 1.0;
    base.length = st.len.max(1);
    base.warp.clear();
    let n = usize::try_from(st.len).unwrap_or(0);
    let ch = soundcraft_mix::clip_source_window(s, &base, st.offset.max(0), n).ok_or_else(|| bad(C, "the clip's original audio is missing"))?;
    // The track's Elastic algorithm; Varispeed reads at speed; a plain track keeps the pitch.
    let algo = s.track(track).and_then(|t| t.elastic.clone());
    let mode = match algo.as_deref() {
        Some("varispeed") => None,
        Some(a) => Some(soundcraft_dsp::offline::StretchMode::from_id(a).unwrap_or_default()),
        None => Some(soundcraft_dsp::offline::StretchMode::Polyphonic),
    };
    let name = format!("{}-Warp", clip.name);
    let src = match mode {
        // Pitch-preserving: rendered now, or (deferred) heard through the real-time stretcher
        // until a worker has rendered it.
        Some(_) => {
            let kind = soundcraft_model::StretchKind::Warp { marks: marks.clone(), mode: algo.clone() };
            crate::stretch_jobs::stretch_source(s, &name, &base, st.offset.max(0), n, kind)
                .ok_or_else(|| bad(C, "the clip's original audio is missing"))?
        }
        None => {
            let um: Vec<(usize, usize)> = marks.iter().map(|m| (usize::try_from(m.0).unwrap_or(0), usize::try_from(m.1).unwrap_or(0))).collect();
            let out = soundcraft_dsp::offline::warp_render(&ch, &um, s.sample_rate.as_f64() as f32, mode);
            let buf = soundcraft_audio_io::AudioBuffer { sample_rate: s.sample_rate.hz(), channels: out };
            crate::io::add_source(s, &name, buf, None, soundcraft_audio_io::FileFormat::Wav)
        }
    };
    remember_original(s, id, &WarpState { roff: 0, ..st.clone() });
    s.edit.values.remove(&format!("elastic.{id}.ratio", id = id.0));
    let old_end = st.marks.last().map_or(0, |m| m.1);
    let e0 = clip.source_offset().saturating_sub(st.roff).max(0);
    let mut nc = clip.clone();
    nc.content = ClipContent::Audio { source: src, offset: e0 };
    nc.stretch = 1.0;
    nc.warp = marks;
    let room = (out_len - e0).max(1);
    nc.length = if e0 + clip.length >= old_end { room } else { clip.length.min(room) };
    nc.clamp_fades();
    super::more_util::replace_clip(s, track, id, nc);
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackId, TrackKind};

    /// A mono clip of 4 s with a burst every second, on a track with `algo`.
    fn hits(algo: Option<&str>) -> (crate::Engine, TrackId, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Drums"));
        let mut v = vec![0.0f32; 192_000];
        for a in [0usize, 48_000, 96_000, 144_000] {
            for k in 0..512 {
                v[a + k] = (1.0 - k as f32 / 512.0) * (k as f32 * 0.13).sin();
            }
        }
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![v] };
        let src = crate::io::add_source(&mut s, "Hits", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(cid, "Hits", src, 0, 10_000, 192_000));
        let mut e = crate::Engine::new(s);
        if let Some(a) = algo {
            e.execute("track.elastic", &json!({"track": t.0, "algorithm": a})).unwrap();
        }
        (e, t, cid)
    }

    /// Timeline positions where a burst starts on the track.
    fn onsets(e: &crate::Engine, t: TrackId) -> Vec<i64> {
        let r = soundcraft_time::Range { start: 0, end: 260_000 };
        let out = soundcraft_mix::render_clips(e.session(), t, r);
        let mut found = Vec::new();
        let mut quiet = usize::MAX / 2;
        for (i, x) in out[0].iter().enumerate() {
            if x.abs() > 0.05 && quiet > 4000 {
                found.push(i as i64);
            }
            quiet = if x.abs() > 0.05 { 0 } else { quiet + 1 };
        }
        found
    }

    #[test]
    fn dragging_a_warp_marker_moves_the_hit_and_keeps_the_others() {
        for algo in [None, Some("rhythmic"), Some("varispeed")] {
            let (mut e, t, cid) = hits(algo);
            // Markers on the second and third hits; the second goes 12 000 later.
            for at in [58_000, 106_000] {
                e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "add", "at": at})).unwrap();
            }
            assert_eq!(onsets(&e, t), vec![10_001, 58_001, 106_001, 154_001], "{algo:?}: adding changes nothing");
            let r = e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "move", "index": 1, "to": 70_000})).unwrap();
            assert_eq!(r["markers"], json!([10_000, 70_000, 106_000, 202_000]), "{algo:?}");
            let got = onsets(&e, t);
            assert_eq!(got.len(), 4, "{algo:?}: {got:?}");
            for (want, got) in [10_000i64, 70_000, 106_000, 154_000].iter().zip(&got) {
                assert!((want - got).abs() < 500, "{algo:?}: {got} for {want}");
            }
            // The clip keeps its place and length; deleting the marker puts the hit back.
            let c = e.session().find_clip(cid).unwrap().1.clone();
            assert_eq!((c.start, c.length), (10_000, 192_000));
            e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "delete", "index": 1})).unwrap();
            let got = onsets(&e, t);
            assert!((got[1] - 58_000).abs() < 500, "{algo:?}: {got:?}");
            // One undo step per edit.
            assert!(e.undo());
            assert_eq!(e.session().find_clip(cid).unwrap().1.warp.len(), 4);
        }
    }

    #[test]
    fn the_last_marker_sets_the_end_and_markers_survive_a_trim_and_a_move() {
        let (mut e, t, cid) = hits(None);
        let r = e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "move", "index": 1, "to": 10_000 + 144_000})).unwrap();
        assert_eq!(r["markers"], json!([10_000, 154_000]));
        assert_eq!(e.session().find_clip(cid).unwrap().1.length, 144_000, "three quarters as long");
        let got = onsets(&e, t);
        assert_eq!(got.len(), 4, "{got:?}");
        assert!((got[2] - 82_000).abs() < 500, "{got:?}");
        // Trim the start and move the clip: the markers stay on their audio.
        let st = super::warp_state(e.session(), cid).unwrap();
        let mut s = e.session().clone();
        s.find_clip_mut(cid).unwrap().trim_start_to(30_000);
        let c = s.find_clip(cid).unwrap().1.clone();
        assert_eq!(super::warp_state(&s, cid).unwrap().positions(&c), st.positions(e.session().find_clip(cid).unwrap().1));
        // Hostile input: a marker index that is not there, a stretch past 10 times.
        assert!(e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "move", "index": 9, "to": 0})).is_err());
        assert!(e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "move", "index": 1, "to": 10_050})).is_err());
        assert!(e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "delete", "index": 0})).is_err());
        assert!(e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "add", "at": 0})).is_err());
    }
}
