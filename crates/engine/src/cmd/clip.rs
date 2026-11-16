//! Clip menu commands.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("clip.edit_lock", "Edit Lock/Unlock", ["Clip"], Some("Cmd+L"), "{clips?}", has_selection, |e, p| flag(
            e,
            p,
            |c| c.edit_locked,
            |c, v| c.edit_locked = v
        )),
        cmd!("clip.time_lock", "Time Lock/Unlock", ["Clip"], Some("Ctrl+Alt+L"), "{clips?}", has_selection, |e, p| flag(
            e,
            p,
            |c| c.time_locked,
            |c, v| c.time_locked = v
        )),
        cmd!("clip.send_to_back", "Send to Back", ["Clip"], Some("Alt+Shift+B"), "{clips?}", has_selection, |e, p| layer(e, p, false)),
        cmd!("clip.bring_to_front", "Bring to Front", ["Clip"], Some("Alt+Shift+F"), "{clips?}", has_selection, |e, p| layer(e, p, true)),
        cmd!("clip.rating", "Rating", ["Clip", "Rating"], Some("Cmd+Alt+0-5"), "{clips?, rating: 0..5}", has_selection, |e, p| {
            let ids = clip_ids_param(e, p);
            let r = i64_or(p, "rating", 0).clamp(0, 5) as u8;
            let s = e.session_mut();
            for id in &ids {
                if let Some(c) = s.find_clip_mut(*id) {
                    c.rating = r
                }
            }
            Ok(json!({"rating": r}))
        }),
        cmd!("clip.group", "Group", ["Clip"], Some("Cmd+Alt+G"), "{clips?}", has_selection, |e, p| {
            let ids = clip_ids_param(e, p);
            if ids.len() < 2 {
                return Err(bad("clip.group", "select two or more clips"));
            }
            let s = e.session_mut();
            let g = s.alloc();
            for id in &ids {
                if let Some(c) = s.find_clip_mut(*id) {
                    c.group = Some(g)
                }
            }
            Ok(json!({"group": g}))
        }),
        cmd!("clip.ungroup", "Ungroup", ["Clip"], Some("Cmd+Alt+U"), "{clips?}", has_selection, |e, p| {
            let ids = clip_ids_param(e, p);
            let s = e.session_mut();
            for id in &ids {
                if let Some(c) = s.find_clip_mut(*id) {
                    c.group = None
                }
            }
            Ok(json!({}))
        }),
        cmd!("clip.ungroup_all", "Ungroup All", ["Clip"], None, "{}", always, |e, _| {
            for t in &mut e.session_mut().tracks {
                for pl in &mut t.playlists {
                    for c in &mut pl.clips {
                        c.group = None
                    }
                }
            }
            Ok(json!({}))
        }),
        cmd!("clip.loop", "Loop...", ["Clip"], Some("Cmd+Alt+L"), "{clips?, count?: n, length?: samples}", has_selection, loop_clips),
        cmd!("clip.rename", "Rename...", ["Clip"], Some("Cmd+Alt+Shift+R"), "{clip?, name}", has_selection, rename),
        cmd!("clip.gain", "Clip Gain", [], None, "{clips?, db | delta_db}", has_selection, gain),
        cmd!(
            "clip.gain_line",
            "Clip Gain Line",
            [],
            None,
            "{clip, points: [[offset, db], …]} — the clip's gain line: breakpoints at samples from the clip start, in dB over the clip gain (empty: a flat line)",
            always,
            gain_line
        ),
        cmd!("clip.gain_nudge_up", "Nudge Clip Gain Up", [], Some("Ctrl+Shift+Up"), "{clips?}", has_selection, |e, p| nudge_gain(e, p, 1.0)),
        cmd!("clip.gain_nudge_down", "Nudge Clip Gain Down", [], Some("Ctrl+Shift+Down"), "{clips?}", has_selection, |e, p| nudge_gain(e, p, -1.0)),
        cmd!("clip.gain_render", "Render", ["Clip", "Clip Gain"], None, "{clips?}", has_selection, |e, p| {
            let ids = clip_ids_param(e, p);
            let mut n = 0;
            for id in ids {
                if crate::io::render_clip_gain(e, "clip.gain_render", id)? {
                    n += 1
                }
            }
            Ok(json!({"rendered": n}))
        }),
        // A toggle: the gain and its envelope stay on the clip, unheard (it used to erase them).
        cmd!("clip.gain_bypass", "Bypass", ["Clip", "Clip Gain"], None, "{clips?, value?: bool}", has_selection, |e, p| {
            let ids = clip_ids_param(e, p);
            let s = e.session_mut();
            let on =
                p.get("value").and_then(Value::as_bool).unwrap_or_else(|| !ids.iter().all(|id| s.find_clip(*id).is_some_and(|(_, c)| c.gain_bypass)));
            for id in &ids {
                if let Some(c) = s.find_clip_mut(*id) {
                    c.gain_bypass = on;
                }
            }
            Ok(json!({"bypass": on, "clips": ids.len()}))
        }),
        cmd!("clip.identify_sync_point", "Identify Sync Point", ["Clip"], Some("Cmd+,"), "{clip?, at?}", has_selection, |e, p| {
            let at = position_param(e, "clip.identify_sync_point", p, "at")?.unwrap_or(e.session().edit.selection.start);
            let ids = clip_ids_param(e, p);
            let s = e.session_mut();
            for id in &ids {
                if let Some(c) = s.find_clip_mut(*id) {
                    c.sync_point = (at - c.start).clamp(0, c.length)
                }
            }
            Ok(json!({}))
        }),
        cmd!("clip.remove_sync_point", "Remove Sync Point", ["Clip"], None, "{clips?}", has_sync_point, remove_sync_point),
        cmd!(
            "clip.quantize_to_grid",
            "Quantize to Grid",
            ["Clip"],
            Some("Cmd+0"),
            "{clips?, strength?: 0..100 (100), swing?: 0..100 (0)} — moves each clip's sync point (else its start) that share of the way to the nearest grid line; swing delays every second line of a note grid",
            has_selection,
            quantize_to_grid
        ),
        cmd!("clip.color", "Clip Color", [], None, "{clips?, color: [r,g,b] | null}", has_selection, |e, p| {
            let ids = clip_ids_param(e, p);
            let col = p.get("color").and_then(Value::as_array).map(|a| {
                let g = |i: usize| a.get(i).and_then(Value::as_u64).unwrap_or(0).min(255) as u8;
                [g(0), g(1), g(2)]
            });
            let s = e.session_mut();
            for id in &ids {
                if let Some(c) = s.find_clip_mut(*id) {
                    c.color = col
                }
            }
            Ok(json!({}))
        }),
        cmd!(
            "clip.elastic_properties",
            "Elastic Properties",
            ["Clip"],
            Some("Alt+5"),
            "{clips?, ratio?: 1.0} — on Elastic Audio tracks (polyphonic/rhythmic/monophonic/x-form) the stretch is rendered pitch-preserving; elsewhere it is varispeed",
            has_selection,
            elastic
        ),
        cmd!("clip.remove_warp", "Remove Warp", ["Clip"], None, "{clips?}", has_selection, remove_warp),
        cmd!(
            "clip.place_source",
            "Place Audio File",
            [],
            None,
            "{source: id, track?: id|name (default: a new track), at?: position}",
            always,
            |e, p| {
                let src = p
                    .get("source")
                    .and_then(Value::as_u64)
                    .map(soundcraft_model::SourceId)
                    .ok_or_else(|| bad("clip.place_source", "`source` id required"))?;
                let at = position_param(e, "clip.place_source", p, "at")?.unwrap_or(e.session().edit.selection.start).max(0);
                let track = track_param(e, "clip.place_source", p, "track")?;
                let (name, frames, chans) = e
                    .session()
                    .source(src)
                    .map(|x| (x.name.clone(), i64::try_from(x.frames).unwrap_or(0), usize::from(x.channels)))
                    .ok_or_else(|| bad("clip.place_source", "no such audio file"))?;
                let s = e.session_mut();
                let t = match track {
                    Some(t) if s.track(t).is_some_and(|x| x.kind == soundcraft_model::TrackKind::Audio) => t,
                    Some(_) => return Err(bad("clip.place_source", "audio files go on audio tracks")),
                    None => s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::for_channels(chans.min(2)), Some(&name)),
                };
                let (sh, len) = (s.edit.edit_mode == soundcraft_model::EditMode::Shuffle, frames.max(1));
                let at = if sh { crate::edit::shuffle_slot(s, t, at, &[]) } else { at };
                if sh {
                    crate::edit::make_room(s, t, at, len).map_err(|why| bad("clip.place_source", why))?;
                }
                let id = s.new_clip_id();
                crate::edit::place_clip(s, t, soundcraft_model::Clip::audio(id, name, src, 0, at, len));
                Ok(json!({"clip": id, "track": t}))
            }
        ),
        cmd!("clip.capture", "Capture...", ["Clip"], Some("Cmd+R"), "{name?}", has_range, |e, p| {
            // Captures the selection as a new whole-file-referencing clip in the clip list (we keep it on the track).
            let name = str_param(p, "name").unwrap_or("Captured").to_string();
            let ids = clip_ids_param(e, p);
            let s = e.session_mut();
            for id in &ids {
                if let Some(c) = s.find_clip_mut(*id) {
                    c.name = name.clone()
                }
            }
            Ok(json!({"clips": ids.len()}))
        }),
    ]
}

/// Warp clips. On Elastic tracks the material is rendered with a pitch-preserving time stretch
/// (the original source is remembered in `edit.values` so Remove Warp restores it).
pub(crate) fn elastic(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let ratio = f64_or(p, "ratio", 1.0).clamp(0.05, 20.0);
    let s = e.session_mut();
    let mut rendered = 0;
    for id in &ids {
        let Some((t, c)) = s.find_clip(*id).map(|(t, c)| (t, c.clone())) else { continue };
        let elastic = s.track(t).and_then(|x| x.elastic.clone()).filter(|a| a != "varispeed");
        // Start from the unwarped original if this clip was warped before.
        let key = |k: &str| format!("elastic.{}.{k}", id.0);
        let orig = warp_original(s, *id);
        let mut base = c.clone();
        base.warp.clear();
        if let Some((src, off, len)) = orig {
            base.content = soundcraft_model::ClipContent::Audio { source: src, offset: off };
            base.length = len.max(1);
            base.stretch = 1.0;
        }
        if elastic.is_none() || !base.is_audio() {
            // Varispeed: the clip plays `ratio` times as long, so its length follows (it used
            // to keep its length: the tail was cut off, or ran on into silence).
            let material = base.length as f64 / if base.stretch.is_finite() && base.stretch > 0.0 { base.stretch } else { 1.0 };
            if !base.is_audio() {
                if let Some(cm) = s.find_clip_mut(*id) {
                    cm.stretch = ratio;
                }
                continue;
            }
            let mut nc = base;
            nc.stretch = ratio;
            nc.length = soundcraft_time::to_samples(material * ratio).max(1);
            nc.fade_in.len = nc.fade_in.len.min(nc.length);
            nc.fade_out.len = nc.fade_out.len.min(nc.length);
            // Back on the original file (a render from an Elastic track before): its notes go.
            for k in ["src", "off", "len", "ratio", "start"] {
                s.edit.values.remove(&key(k));
            }
            super::more_util::replace_clip(s, t, *id, nc);
            continue;
        }
        let out_len = soundcraft_time::to_samples(base.length as f64 * ratio).max(1);
        let Some(src) = super::more_util::render_stretched(s, "clip.elastic_properties", &base, out_len)? else { continue };
        if orig.is_none()
            && let soundcraft_model::ClipContent::Audio { source, offset } = base.content
        {
            s.edit.values.insert(key("src"), source.0 as f64);
            s.edit.values.insert(key("off"), offset as f64);
            s.edit.values.insert(key("len"), base.length as f64);
        }
        s.edit.values.insert(key("ratio"), ratio);
        let mut nc = base;
        nc.content = soundcraft_model::ClipContent::Audio { source: src, offset: 0 };
        nc.length = out_len;
        nc.stretch = 1.0;
        super::more_util::replace_clip(s, t, *id, nc);
        rendered += 1;
    }
    Ok(json!({"ratio": ratio, "rendered": rendered}))
}

fn remove_warp(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let mut n = 0;
    for id in &ids {
        let key = |k: &str| format!("elastic.{}.{k}", id.0);
        let orig = warp_original(s, *id);
        let start = s.edit.values.get(&key("start")).map(|v| *v as Samples);
        if let Some(c) = s.find_clip_mut(*id) {
            // A varispeed clip (no render) goes back to its material's own length.
            if orig.is_none() && c.is_audio() && c.stretch.is_finite() && c.stretch > 0.0 && (c.stretch - 1.0).abs() > 1e-9 {
                c.length = soundcraft_time::to_samples(c.length as f64 / c.stretch).max(1);
                c.fade_in.len = c.fade_in.len.min(c.length);
                c.fade_out.len = c.fade_out.len.min(c.length);
                n += 1;
            }
            c.stretch = 1.0;
            c.warp.clear();
            if let Some((src, off, len)) = orig {
                // Straighten also moved the clip: it goes back where it was.
                if let Some(st) = start {
                    c.start = st;
                }
                c.content = soundcraft_model::ClipContent::Audio { source: src, offset: off };
                c.length = len.max(1);
                c.fade_out.len = c.fade_out.len.min(c.length);
                n += 1;
            }
        }
        for k in ["src", "off", "len", "ratio", "start"] {
            s.edit.values.remove(&key(k));
        }
    }
    Ok(json!({"restored": n}))
}

/// The audio a warped clip was rendered from, while that file is still in the session.
fn warp_original(s: &soundcraft_model::Session, id: ClipId) -> Option<(soundcraft_model::SourceId, Samples, Samples)> {
    let v = |k: &str| s.edit.values.get(&format!("elastic.{}.{k}", id.0)).copied();
    let src = soundcraft_model::SourceId(v("src")? as u64);
    s.source(src)?;
    Some((src, v("off")? as i64, v("len")? as i64))
}

fn flag(e: &mut Engine, p: &Value, get: fn(&soundcraft_model::Clip) -> bool, set: fn(&mut soundcraft_model::Clip, bool)) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let v = p.get("value").and_then(Value::as_bool).unwrap_or_else(|| !ids.iter().all(|id| s.find_clip(*id).is_some_and(|(_, c)| get(c))));
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            set(c, v);
        }
    }
    Ok(json!({"value": v, "clips": ids.len()}))
}

/// Send to Back / Bring to Front. Overlapping clips (crossfades) are mixed together, and a
/// playlist is kept in time order (code relies on it), so there is no front or back to keep:
/// the commands change nothing and say so. They used to reorder the playlist out of time order
/// until the next edit re-sorted it.
fn layer(e: &mut Engine, p: &Value, _front: bool) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    Ok(json!({"clips": ids.len(), "changed": false, "note": "overlapping clips are mixed together; there is no front or back order"}))
}

fn loop_clips(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let count = i64_or(p, "count", 4).clamp(1, 512);
    let total = position_param(e, "clip.loop", p, "length")?;
    let s = e.session_mut();
    let mut made = 0;
    for id in ids {
        let Some((tid, mut c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { continue };
        let reps = total.map_or(count - 1, |l| (l / c.length.max(1)).max(1) - 1).clamp(0, 1000);
        if reps > 0 && c.loop_id.is_none() {
            let lid = s.alloc();
            if let Some(src) = s.find_clip_mut(id) {
                src.loop_id = Some(lid);
            }
            c.loop_id = Some(lid);
        }
        // A MIDI clip on a tick-timebase track loops in bars and beats: each copy starts a whole
        // clip length in ticks after the one before, so under a changing tempo the copies stay
        // on their bars (and each is as long as the clip is in ticks there).
        let musical = matches!(c.content, soundcraft_model::ClipContent::Midi { .. }) && s.track(tid).is_some_and(|t| t.ticks_timebase);
        let sr = s.sample_rate;
        let map = s.tempo.clone();
        let (t0, t1) = (map.samples_to_ticks_f(c.start, sr), map.samples_to_ticks_f(c.end(), sr));
        let at_tick = |t: f64| {
            let x = map.ticks_f_to_samples_f(t, sr).round();
            if x.is_finite() { x.clamp(0.0, i64::MAX as f64 / 2.0) as i64 } else { 0 }
        };
        let mut copies = Vec::new();
        for k in 1..=reps {
            let mut nc = c.clone();
            nc.id = s.new_clip_id();
            if musical && t1 > t0 {
                let span = t1 - t0;
                let (a, b) = (at_tick(t0 + span * k as f64), at_tick(t0 + span * (k + 1) as f64));
                nc.start = a;
                nc.length = b.saturating_sub(a).max(1);
            } else {
                nc.start = c.start.saturating_add(c.length.saturating_mul(k));
            }
            copies.push((tid, nc));
        }
        made += copies.len();
        crate::edit::put_clips(s, copies);
    }
    Ok(json!({"loops": made}))
}

fn rename(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("clip.rename", "`name` required"))?.to_string();
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.name = name.clone();
        }
    }
    Ok(json!({"clips": ids.len()}))
}

/// Most breakpoints one clip-gain line keeps.
const MAX_GAIN_POINTS: usize = 10_000;

fn gain_line(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.gain_line";
    let id = p.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(C, "`clip` required"))?;
    let raw = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(C, "`points` required: [[offset, db], …]"))?;
    if raw.len() > MAX_GAIN_POINTS {
        return Err(bad(C, format!("at most {MAX_GAIN_POINTS} points")));
    }
    let len = e.session().find_clip(id).map(|(_, c)| c.length).ok_or_else(|| bad(C, "no such clip"))?;
    let mut points = Vec::with_capacity(raw.len());
    for v in raw {
        let pair = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(C, "each point is [offset, db]"))?;
        let at = pair.first().and_then(Value::as_i64).ok_or_else(|| bad(C, "an offset is a whole number of samples"))?;
        let db = pair.get(1).and_then(Value::as_f64).filter(|d| d.is_finite()).ok_or_else(|| bad(C, "a level is a finite number of dB"))?;
        points.push((at.clamp(0, len.max(0)), db.clamp(-144.0, 36.0) as f32));
    }
    points.sort_by_key(|x| x.0);
    // One level per offset: the last given wins.
    let mut line: Vec<(Samples, f32)> = Vec::with_capacity(points.len());
    for (at, db) in points {
        match line.last_mut() {
            Some(last) if last.0 == at => last.1 = db,
            _ => line.push((at, db)),
        }
    }
    let n = line.len();
    if let Some(c) = e.session_mut().find_clip_mut(id) {
        c.gain_env = line;
    }
    Ok(json!({"points": n}))
}

fn gain(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let abs = p.get("db").and_then(Value::as_f64);
    let delta = p.get("delta_db").and_then(Value::as_f64);
    if abs.is_none() && delta.is_none() {
        return Err(bad("clip.gain", "`db` or `delta_db` required"));
    }
    let s = e.session_mut();
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            let v = abs.unwrap_or(f64::from(c.gain_db) + delta.unwrap_or(0.0));
            c.gain_db = if v.is_finite() { (v as f32).clamp(-144.0, 36.0) } else { c.gain_db };
        }
    }
    Ok(json!({"clips": ids.len()}))
}

fn nudge_gain(e: &mut Engine, p: &Value, db: f64) -> Result<Value> {
    let clips = clip_ids_param(e, p);
    gain(e, &json!({"clips": clips, "delta_db": db}))
}

/// Sync point 0 is "none" (edit_window only draws one when > 0).
fn has_sync_point(e: &Engine) -> std::result::Result<(), String> {
    let s = e.session();
    if clip_ids_param(e, &json!({})).iter().any(|id| s.find_clip(*id).is_some_and(|(_, c)| c.sync_point != 0)) {
        Ok(())
    } else {
        Err("no selected clip has a sync point".into())
    }
}

fn remove_sync_point(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session();
    // Only clips that have a point are touched, so a call that changes nothing leaves no undo step.
    let ids: Vec<ClipId> = clip_ids_param(e, p).into_iter().filter(|id| s.find_clip(*id).is_some_and(|(_, c)| c.sync_point != 0)).collect();
    if !ids.is_empty() {
        let s = e.session_mut();
        for id in &ids {
            if let Some(c) = s.find_clip_mut(*id) {
                c.sync_point = 0
            }
        }
    }
    Ok(json!({"clips": ids.len()}))
}

fn quantize_to_grid(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session();
    let strength = f64_or(p, "strength", 100.0) / 100.0;
    let swing = f64_or(p, "swing", 0.0) / 100.0;
    let moves: Vec<(soundcraft_model::ClipId, i64)> =
        ids.iter().filter_map(|id| s.find_clip(*id).map(|(_, c)| (*id, crate::edit::quantize_delta(s, c, strength, swing)))).collect();
    let s = e.session_mut();
    for (id, d) in &moves {
        crate::edit::move_clips(s, &[*id], *d, &[]);
    }
    Ok(json!({"clips": moves.len()}))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use soundcraft_model::{ClipId, TrackId};

    /// Bug 15: a one-bar MIDI clip looped over a tempo change puts each copy on the next bar.
    #[test]
    fn midi_loops_on_a_tick_track_stay_on_the_bars() {
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut e = crate::Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        // 120 bpm, then 60 bpm from bar 2: bar 2 starts at 96 000, bar 3 at 288 000.
        e.execute("event.tempo", &json!({"bpm": 60.0, "at": 96_000})).unwrap();
        let clip = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        e.execute("clip.loop", &json!({"clips": [clip], "count": 3})).unwrap();
        let spans: Vec<(i64, i64)> = e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.length)).collect();
        assert_eq!(spans, vec![(0, 96_000), (96_000, 192_000), (288_000, 192_000)]);
    }

    #[test]
    fn elastic_warp_renders_and_remove_warp_restores() {
        let mut e = crate::demo::demo_engine();
        let pad = e.session().track_by_name("Pad").map(|t| t.id.0).unwrap();
        e.execute("track.elastic", &json!({"track": pad, "algorithm": "polyphonic"})).unwrap();
        let cid = e.session().track(TrackId(pad)).unwrap().clips()[0].id.0;
        let len0 = e.session().find_clip(ClipId(cid)).unwrap().1.length;
        let r = e.execute("clip.elastic_properties", &json!({"clips": [cid], "ratio": 1.5})).unwrap();
        assert_eq!(r["rendered"], 1);
        let c = e.session().find_clip(ClipId(cid)).unwrap().1.clone();
        assert!((c.length as f64 - len0 as f64 * 1.5).abs() < 2.0, "{} vs {}", c.length, len0);
        assert_eq!(c.stretch, 1.0);
        // Re-warping starts from the original, not the rendered audio.
        e.execute("clip.elastic_properties", &json!({"clips": [cid], "ratio": 0.5})).unwrap();
        let c = e.session().find_clip(ClipId(cid)).unwrap().1.clone();
        assert!((c.length as f64 - len0 as f64 * 0.5).abs() < 2.0);
        e.execute("clip.remove_warp", &json!({"clips": [cid]})).unwrap();
        assert_eq!(e.session().find_clip(ClipId(cid)).unwrap().1.length, len0);
    }

    /// Parity bug 14: Send to Back / Bring to Front leave the playlist in time order.
    #[test]
    fn layer_commands_keep_the_playlist_in_time_order() {
        let mut e = crate::demo::demo_engine();
        let t = e.session().tracks.iter().find(|t| t.clips().len() > 1).map(|t| t.id).unwrap();
        let before = e.session().track(t).unwrap().clips().to_vec();
        let last = before.last().unwrap().id.0;
        e.execute("clip.send_to_back", &json!({"clips": [last]})).unwrap();
        e.execute("clip.bring_to_front", &json!({"clips": [before[0].id.0]})).unwrap();
        assert_eq!(e.session().track(t).unwrap().clips(), before.as_slice());
    }

    /// Parity bug 12: Conform to Tempo on an Elastic track keeps the pitch (a rendered,
    /// pitch-preserving stretch), not a varispeed ratio.
    #[test]
    fn conform_on_an_elastic_track_keeps_the_pitch() {
        let mut e = crate::demo::demo_engine();
        let pad = e.session().track_by_name("Pad").map(|t| t.id.0).unwrap();
        e.execute("track.elastic", &json!({"track": pad, "algorithm": "polyphonic"})).unwrap();
        let cid = e.session().track(TrackId(pad)).unwrap().clips()[0].id.0;
        let len0 = e.session().find_clip(ClipId(cid)).unwrap().1.length;
        let src0 = e.session().find_clip(ClipId(cid)).unwrap().1.source();
        let bpm = e.session().tempo.tempo_at_tick(0);
        e.execute("clip.conform_to_tempo", &json!({"clip": cid, "source_bpm": bpm * 1.25})).unwrap();
        let c = e.session().find_clip(ClipId(cid)).unwrap().1.clone();
        assert_eq!(c.stretch, 1.0, "no varispeed");
        assert_ne!(c.source(), src0, "rendered to a new file");
        assert!((c.length as f64 - len0 as f64 * 1.25).abs() < 2.0, "{} vs {}", c.length, len0);
    }

    /// A clip on an Elastic track playing `clip_len` samples of a source that is silent before `tone_at`.
    fn elastic_clip(source_len: usize, tone_at: usize, clip_len: i64) -> (crate::Engine, TrackId, ClipId) {
        use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Elastic"));
        let samples = (0..source_len).map(|i| if i >= tone_at { (i as f32 * 0.2).sin() * 0.4 } else { 0.0 }).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![samples] };
        let src = crate::io::add_source(&mut s, "Tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(cid, "Clip", src, 0, 0, clip_len));
        let mut e = crate::Engine::new(s);
        e.execute("track.elastic", &json!({"track": t.0, "algorithm": "polyphonic"})).unwrap();
        (e, t, cid)
    }

    fn rendered_peak(e: &crate::Engine, t: TrackId, cid: ClipId) -> f32 {
        let range = e.session().find_clip(cid).unwrap().1.range();
        soundcraft_mix::render_clips(e.session(), t, range).iter().flatten().fold(0.0, |m, x| m.max(x.abs()))
    }

    /// Elastic Properties stretches the material the clip plays, by the requested ratio (#227).
    #[test]
    fn elastic_properties_stretches_the_clip_material() {
        let (mut e, t, cid) = elastic_clip(20_000, 10_000, 20_000);
        e.execute("clip.elastic_properties", &json!({"clips": [cid.0], "ratio": 2.0})).unwrap();
        assert_eq!(e.session().find_clip(cid).unwrap().1.length, 40_000);
        assert!(rendered_peak(&e, t, cid) > 0.3, "the tone in the second half was lost");

        let (mut e, t, cid) = elastic_clip(60_000, 20_000, 20_000);
        e.execute("clip.elastic_properties", &json!({"clips": [cid.0], "ratio": 0.5})).unwrap();
        assert_eq!(rendered_peak(&e, t, cid), 0.0, "audio from past the clip end leaked in");

        for (ratio, len) in [(4.0, 80_000), (0.2, 4_000)] {
            let (mut e, _, cid) = elastic_clip(20_000, 10_000, 20_000);
            e.execute("clip.elastic_properties", &json!({"clips": [cid.0], "ratio": ratio})).unwrap();
            assert_eq!(e.session().find_clip(cid).unwrap().1.length, len, "ratio {ratio}");
        }
    }

    #[test]
    fn elastic_properties_off_an_elastic_track_changes_length_with_speed() {
        // Bug: only the speed changed, so the tail was cut (slower) or ran into silence (faster).
        use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Plain"));
        let samples = (0..20_000).map(|i| if i >= 19_000 { 0.5 } else { 0.0 }).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![samples] };
        let src = crate::io::add_source(&mut s, "Tail", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(cid, "Clip", src, 0, 0, 20_000));
        let mut e = crate::Engine::new(s);
        e.execute("clip.elastic_properties", &json!({"clips": [cid.0], "ratio": 2.0})).unwrap();
        let c = e.session().find_clip(cid).unwrap().1.clone();
        assert_eq!((c.stretch, c.length), (2.0, 40_000));
        assert!(rendered_peak(&e, t, cid) > 0.4, "the tail still plays");
        // From the material's length, not the stretched one.
        e.execute("clip.elastic_properties", &json!({"clips": [cid.0], "ratio": 0.5})).unwrap();
        assert_eq!(e.session().find_clip(cid).unwrap().1.length, 10_000);
        e.execute("clip.remove_warp", &json!({"clips": [cid.0]})).unwrap();
        let c = e.session().find_clip(cid).unwrap().1.clone();
        assert_eq!((c.stretch, c.length), (1.0, 20_000));
    }

    #[test]
    fn remove_warp_keeps_the_audio_when_the_original_file_is_gone() {
        let mut e = crate::demo::demo_engine();
        let pad = e.session().track_by_name("Pad").map(|t| t.id).unwrap();
        e.execute("track.elastic", &json!({"track": pad, "algorithm": "polyphonic"})).unwrap();
        let clip = e.session().track(pad).unwrap().clips()[0].clone();
        e.execute("clip.elastic_properties", &json!({"clips": [clip.id], "ratio": 1.5})).unwrap();
        e.execute("clip.clear", &json!({"sources": [clip.source()]})).unwrap();
        let r = e.execute("clip.remove_warp", &json!({"clips": [clip.id]})).unwrap();
        assert_eq!(r["restored"], 0);
        let src = e.session().find_clip(clip.id).and_then(|(_, c)| c.source()).unwrap();
        assert!(e.session().source(src).is_some());
    }

    #[test]
    fn clip_gain_bypass_keeps_the_gain_and_toggles() {
        // Bug: Bypass erased the gain and its envelope.
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().clips()[0].id;
        e.execute("clip.gain", &json!({"clips": [kick.0], "db": -9.0})).unwrap();
        e.session_mut().find_clip_mut(kick).unwrap().gain_env = vec![(0, 0.0), (100, -3.0)];
        let r = e.execute("clip.gain_bypass", &json!({"clips": [kick.0]})).unwrap();
        assert_eq!(r["bypass"], true);
        let c = e.session().find_clip(kick).unwrap().1.clone();
        assert_eq!((c.gain_db, c.gain_env.len(), c.gain_bypass), (-9.0, 2, true));
        assert_eq!(c.gain_at(c.length / 2), 1.0, "unheard while bypassed");
        e.execute("clip.gain_bypass", &json!({"clips": [kick.0]})).unwrap();
        let c = e.session().find_clip(kick).unwrap().1.clone();
        assert!(!c.gain_bypass);
        assert!(c.gain_at(c.length / 2) < 0.4, "heard again");
        assert!(e.undo());
        assert!(e.session().find_clip(kick).unwrap().1.gain_bypass);
    }

    #[test]
    fn gain_nudges_one_db_at_a_time_up_to_the_ceiling() {
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().clips()[0].id;
        let gain = |e: &crate::Engine| e.session().find_clip(kick).unwrap().1.gain_db;
        e.execute("edit.select", &json!({"clips": [kick.0]})).unwrap();
        e.execute("clip.gain_nudge_up", &json!({})).unwrap();
        e.execute("clip.gain_nudge_up", &json!({})).unwrap();
        e.execute("clip.gain_nudge_down", &json!({})).unwrap();
        assert_eq!(gain(&e), 1.0);
        e.execute("clip.gain", &json!({"db": 35.5})).unwrap();
        e.execute("clip.gain_nudge_up", &json!({})).unwrap();
        assert_eq!(gain(&e), 36.0);
    }

    /// Three 10 000-sample clips side by side on one track; sync points are set by each test.
    fn three_clips() -> (crate::Engine, [ClipId; 3]) {
        use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Dialog"));
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 10_000]] };
        let src = crate::io::add_source(&mut s, "Line", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let ids = [0, 1, 2].map(|k| {
            let cid = s.new_clip_id();
            crate::edit::place_clip(&mut s, t, Clip::audio(cid, "Line", src, 0, k * 12_000, 10_000));
            cid
        });
        (crate::Engine::new(s), ids)
    }

    fn sync_of(e: &crate::Engine, id: ClipId) -> i64 {
        e.session().find_clip(id).unwrap().1.sync_point
    }

    #[test]
    fn remove_sync_point_clears_the_point_identify_set() {
        let (mut e, [a, ..]) = three_clips();
        e.execute("edit.select", &json!({"clips": [a.0]})).unwrap();
        e.execute("clip.identify_sync_point", &json!({"at": 4_000})).unwrap();
        assert_eq!(sync_of(&e, a), 4_000);
        e.execute("clip.remove_sync_point", &json!({})).unwrap();
        assert_eq!(sync_of(&e, a), 0);
    }

    #[test]
    fn remove_sync_point_clears_every_selected_clip_and_only_those() {
        let (mut e, [a, b, c]) = three_clips();
        for (id, at) in [(a, 1_000), (b, 2_000), (c, 3_000)] {
            e.session_mut().find_clip_mut(id).unwrap().sync_point = at;
        }
        e.execute("edit.select", &json!({"clips": [a.0, b.0]})).unwrap();
        let r = e.execute("clip.remove_sync_point", &json!({})).unwrap();
        assert_eq!(r["clips"], 2);
        assert_eq!([sync_of(&e, a), sync_of(&e, b), sync_of(&e, c)], [0, 0, 3_000]);
    }

    #[test]
    fn remove_sync_point_is_disabled_unless_a_selected_clip_has_one() {
        let (mut e, [a, b, c]) = three_clips();
        e.session_mut().find_clip_mut(c).unwrap().sync_point = 3_000;
        let enabled = |e: &crate::Engine| crate::find_command("clip.remove_sync_point").unwrap().info(e).enabled;
        e.execute("edit.select", &json!({"clips": [a.0, b.0]})).unwrap();
        assert!(!enabled(&e), "neither selected clip has a sync point (the unselected one does)");
        e.execute("edit.select", &json!({"clips": [b.0, c.0]})).unwrap();
        assert!(enabled(&e));
    }

    #[test]
    fn remove_sync_point_is_one_undo_step_that_puts_the_points_back() {
        let (mut e, [a, b, _]) = three_clips();
        for (id, at) in [(a, 1_000), (b, 2_000)] {
            e.session_mut().find_clip_mut(id).unwrap().sync_point = at;
        }
        e.execute("edit.select", &json!({"clips": [a.0, b.0]})).unwrap();
        let steps = e.undo_history().len();
        e.execute("clip.remove_sync_point", &json!({})).unwrap();
        assert_eq!(e.undo_history().len(), steps + 1);
        assert_eq!(e.undo_label(), Some("Remove Sync Point"));
        assert_eq!([sync_of(&e, a), sync_of(&e, b)], [0, 0]);
        assert!(e.undo());
        assert_eq!([sync_of(&e, a), sync_of(&e, b)], [1_000, 2_000]);
    }
}
