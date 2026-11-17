//! Automation inside clips ([`soundcraft_model::clip_automation`]): points on a clip's own lanes,
//! and moving a track's automation into its clips and back, as Logic does.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::clip_automation::{EDGE_RAMP, adds};
use soundcraft_model::{AutoParam, AutomationLane, ClipId, TrackId};
use soundcraft_time::Samples;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.automation_point",
            "Add Clip Automation Point",
            [],
            None,
            "{clip, param (an automation lane id: volume, pan, plugin:m3:gain, ...), at: samples from the clip's start, value} — a point on the clip's own lane, heard while the clip plays (a dB lane adds to the track's, any other replaces it there)",
            has_selection,
            point
        ),
        cmd!(
            "clip.automation_clear",
            "Clear Clip Automation",
            ["Edit", "Automation"],
            None,
            "{clips?, param?} — removes the selected clips' own automation (one lane, or every lane)",
            has_selection,
            clear
        ),
        cmd!(
            "clip.automation_from_track",
            "Move Track Automation into Clips",
            ["Edit", "Automation"],
            None,
            "{clips?, param?} — the track's points under each selected clip move onto the clip (as offsets for a dB lane), so they move with it; what plays does not change",
            has_selection,
            from_track
        ),
        cmd!(
            "clip.automation_to_track",
            "Move Clip Automation to Track",
            ["Edit", "Automation"],
            None,
            "{clips?, param?} — each selected clip's own automation is written onto the track's lanes as it plays now and taken off the clip",
            has_selection,
            to_track
        ),
    ]
}

fn param_of(p: &Value, c: &str) -> Result<Option<AutoParam>> {
    match p.get("param").and_then(Value::as_str) {
        None => Ok(None),
        Some(s) => AutoParam::parse(s).map(Some).ok_or_else(|| bad(c, format!("unknown automation lane `{s}`"))),
    }
}

fn point(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.automation_point";
    let id = p.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(C, "`clip` required"))?;
    let param = param_of(p, C)?.ok_or_else(|| bad(C, "`param` required"))?;
    let at = p.get("at").and_then(Value::as_i64).ok_or_else(|| bad(C, "`at` required (samples from the clip's start)"))?;
    let value = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(C, "`value` required"))? as f32;
    let Some(c) = e.session_mut().find_clip_mut(id) else { return Err(bad(C, format!("no clip {}", id.0))) };
    if !(0..c.length).contains(&at) {
        return Err(bad(C, format!("`at` is inside the clip: 0 to {}", c.length - 1)));
    }
    let lane = match c.automation.iter().position(|l| l.param == param) {
        Some(i) => &mut c.automation[i],
        None => {
            c.automation.push(AutomationLane::new(param.clone()));
            let n = c.automation.len() - 1;
            &mut c.automation[n]
        }
    };
    lane.set_point(at, value);
    Ok(json!({"points": lane.points.len()}))
}

/// The selected (or named) clips, with their tracks.
fn targets(e: &Engine, p: &Value) -> Vec<(TrackId, ClipId)> {
    clip_ids_param(e, p).into_iter().filter_map(|id| e.session().find_clip(id).map(|(t, _)| (t, id))).collect()
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let param = param_of(p, "clip.automation_clear")?;
    let mut n = 0;
    for (_, id) in targets(e, p) {
        if let Some(c) = e.session_mut().find_clip_mut(id) {
            let before = c.automation.len();
            c.automation.retain(|l| param.as_ref().is_some_and(|q| &l.param != q));
            n += before - c.automation.len();
        }
    }
    Ok(json!({"lanes": n}))
}

fn from_track(e: &mut Engine, p: &Value) -> Result<Value> {
    let param = param_of(p, "clip.automation_from_track")?;
    let mut moved = 0;
    for (t, id) in targets(e, p) {
        let s = e.session_mut();
        let Some(range) = s.find_clip(id).map(|(_, c)| (c.start, c.end())) else { continue };
        let Some(tr) = s.track_mut(t) else { continue };
        let lanes: Vec<AutomationLane> = tr.automation.iter().filter(|l| param.as_ref().is_none_or(|q| &l.param == q)).cloned().collect();
        let mut onto: Vec<AutomationLane> = Vec::new();
        for old in lanes {
            let (start, end) = range;
            let inside: Vec<Samples> = old.points.iter().map(|q| q.at).filter(|at| *at >= start && *at < end).collect();
            if inside.is_empty() {
                continue;
            }
            let def = old.param.range().2;
            // The track keeps its value at the clip's edges; the points under the clip go.
            let mut kept = old.clone();
            kept.points.retain(|q| q.at < start || q.at >= end);
            kept.set_point(start, old.value_at(start, def));
            kept.set_point(end.saturating_sub(1).max(start), old.value_at(end - 1, def));
            let mut lane = AutomationLane::new(old.param.clone());
            let mut times = inside;
            times.extend([start, end.saturating_sub(1).max(start)]);
            times.sort_unstable();
            times.dedup();
            for at in times {
                let v = if adds(&old.param) { old.value_at(at, def) - kept.value_at(at, def) } else { old.value_at(at, def) };
                lane.set_point(at - start, v);
            }
            moved += 1;
            *tr.lane_mut(&old.param) = kept;
            onto.push(lane);
        }
        if let Some(c) = s.find_clip_mut(id) {
            for l in onto {
                c.automation.retain(|x| x.param != l.param);
                c.automation.push(l);
            }
        }
    }
    Ok(json!({"lanes": moved}))
}

fn to_track(e: &mut Engine, p: &Value) -> Result<Value> {
    let param = param_of(p, "clip.automation_to_track")?;
    let mut moved = 0;
    for (t, id) in targets(e, p) {
        let s = e.session_mut();
        let Some(tr) = s.track(t) else { continue };
        let Some((_, c)) = s.find_clip(id) else { continue };
        let (start, end) = (c.start, c.end());
        let lanes: Vec<AutoParam> = c.automation.iter().map(|l| l.param.clone()).filter(|q| param.as_ref().is_none_or(|x| x == q)).collect();
        // What plays now, with only this clip's automation laid over the track's.
        let mut written: Vec<AutomationLane> = Vec::new();
        for q in &lanes {
            let Some(flat) = soundcraft_model::clip_automation::flatten(q, tr.lane(q), std::slice::from_ref(c)) else { continue };
            let mut lane = tr.lane(q).cloned().unwrap_or_else(|| AutomationLane::new(q.clone()));
            lane.points.retain(|x| x.at < start - EDGE_RAMP || x.at > end);
            lane.points.extend(flat.points.iter().filter(|x| x.at >= start - EDGE_RAMP && x.at <= end).copied());
            lane.points.sort_by_key(|x| x.at);
            lane.points.dedup_by_key(|x| x.at);
            written.push(lane);
        }
        let Some(tr) = s.track_mut(t) else { continue };
        for l in written {
            moved += 1;
            let param = l.param.clone();
            *tr.lane_mut(&param) = l;
        }
        if let Some(c) = s.find_clip_mut(id) {
            c.automation.retain(|l| !lanes.contains(&l.param));
        }
    }
    Ok(json!({"lanes": moved}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{AutoParam, AutomationLane, Clip, SourceId};

    /// A stereo track with a clip from 48 000 to 96 000 and a pan lane hard left.
    fn engine() -> (Engine, soundcraft_model::TrackId, soundcraft_model::ClipId) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id;
        let id = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), t, Clip::audio(id, "c", SourceId(1), 0, 48_000, 48_000));
        let mut l = AutomationLane::new(AutoParam::Pan(0));
        l.set_point(0, -1.0);
        l.set_point(200_000, -1.0);
        e.session_mut().track_mut(t).unwrap().automation.push(l);
        (e, t, id)
    }

    fn played(e: &Engine, t: soundcraft_model::TrackId, p: &AutoParam, at: i64) -> f32 {
        e.session().track(t).unwrap().played_lane(p).unwrap().value_at(at, 0.0)
    }

    #[test]
    fn a_clip_point_is_heard_over_the_clip_only_and_moves_with_it() {
        let (mut e, t, id) = engine();
        let pan = AutoParam::Pan(0);
        for at in [0, 47_999] {
            e.execute("clip.automation_point", &json!({"clip": id.0, "param": "pan", "at": at, "value": 0.5})).unwrap();
        }
        assert_eq!(played(&e, t, &pan, 60_000), 0.5);
        assert_eq!(played(&e, t, &pan, 30_000), -1.0);
        assert_eq!(played(&e, t, &pan, 120_000), -1.0);
        // Moved, the clip takes its automation with it.
        e.execute("edit.move_clips", &json!({"clips": [id.0], "to": 100_000})).unwrap();
        assert_eq!(played(&e, t, &pan, 60_000), -1.0);
        assert_eq!(played(&e, t, &pan, 120_000), 0.5);
        // Undo brings back what played before.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(played(&e, t, &pan, 60_000), 0.5);
        // Saved and loaded, it plays the same (the played lanes are not stored).
        let text = serde_json::to_string(e.session()).unwrap();
        assert!(!text.contains("\"played\""));
        e.session_mut().edit.selected_clips = vec![id];
        e.execute("clip.automation_clear", &json!({})).unwrap();
        assert_eq!(played(&e, t, &pan, 60_000), -1.0);
        assert!(e.session().track(t).unwrap().played.is_empty());
    }

    #[test]
    fn moving_automation_into_clips_and_back_keeps_what_plays() {
        let (mut e, t, id) = engine();
        let vol = AutoParam::Volume;
        let mut l = AutomationLane::new(vol.clone());
        for (at, v) in [(0, -6.0), (60_000, -12.0), (80_000, 0.0), (200_000, -3.0)] {
            l.set_point(at, v);
        }
        e.session_mut().track_mut(t).unwrap().automation.push(l.clone());
        let before: Vec<f32> = (0..200).map(|k| played(&e, t, &vol, k * 1_000)).collect();
        e.session_mut().edit.selected_clips = vec![id];
        let v = e.execute("clip.automation_from_track", &json!({"param": "volume"})).unwrap();
        assert_eq!(v["lanes"], 1);
        assert!(e.session().find_clip(id).unwrap().1.automation.iter().any(|l| l.param == vol));
        assert!(e.session().track(t).unwrap().lane(&vol).unwrap().points.iter().all(|q| q.at != 60_000 && q.at != 80_000));
        let after: Vec<f32> = (0..200).map(|k| played(&e, t, &vol, k * 1_000)).collect();
        for (k, (a, b)) in before.iter().zip(&after).enumerate() {
            assert!((a - b).abs() < 0.05, "{k} ms: {a} vs {b}");
        }
        e.execute("clip.automation_to_track", &json!({})).unwrap();
        assert!(e.session().find_clip(id).unwrap().1.automation.is_empty());
        let back: Vec<f32> = (0..200).map(|k| played(&e, t, &vol, k * 1_000)).collect();
        for (k, (a, b)) in before.iter().zip(&back).enumerate() {
            assert!((a - b).abs() < 0.05, "{k} ms: {a} vs {b}");
        }
    }
}
