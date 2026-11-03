//! Automation point editing: delete, select, move, copy and paste breakpoints, curves between
//! them, and the Pencil's overwriting draw (spec `2026-10-28-automation-editing`).

use super::mix::{current_value, lane_params};
use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{AutoParam, AutomationPoint};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "automation.delete",
            "Delete Automation Points",
            [],
            None,
            "{track?, param, points: [at]} — removes the breakpoints at those times",
            has_selection,
            delete
        ),
        cmd!(query "automation.select", "Select Automation Points", [], None, "{track?, param, start?, end?} — the times of the breakpoints inside the range (default: the edit selection)", has_selection, select),
        cmd!(
            "automation.move",
            "Move Automation Points",
            [],
            None,
            "{track?, param, points: [at], dt?: samples, dv?: value} — moves the breakpoints together (time and value); result: their new times",
            has_selection,
            move_points
        ),
        cmd!(
            "automation.set_curve",
            "Curve Automation Segment",
            [],
            None,
            "{track?, param, at, curve: -1..1} — bends the segment that starts at the breakpoint at `at` (0 = straight)",
            has_selection,
            set_curve
        ),
        cmd!(noundo "automation.copy", "Copy Automation Points", [], None, "{track?, param, points?: [at] | start?/end?} — copies the breakpoints to the clipboard (times from the first)", has_selection, copy),
        cmd!(
            "automation.paste",
            "Paste Automation Points",
            [],
            None,
            "{track?, param?: the copied lane's, at?: the edit insertion} — pastes the copied breakpoints onto a lane of the same kind",
            has_auto_clipboard,
            paste
        ),
        cmd!(
            "automation.draw",
            "Draw Automation",
            [],
            None,
            "{track?, param, points: [[at, value]…], tolerance?: 0.1} — a Pencil pass: replaces the lane inside the series' span (anchors keep the curve outside it), thinned",
            has_selection,
            draw
        ),
    ]
}

/// The lanes `track`/`tracks` + `param` name, as (track, param) pairs.
fn lanes(e: &Engine, p: &Value, cmd: &str) -> Result<Vec<(soundcraft_model::TrackId, AutoParam)>> {
    let tracks = tracks_required(e, cmd, p)?;
    lane_params(e, &tracks, p, cmd)
}

/// `points: [at, …]` (samples), required unless `optional`.
fn points_param(p: &Value, cmd: &str, optional: bool) -> Result<Vec<Samples>> {
    match p.get("points").and_then(Value::as_array) {
        Some(a) => Ok(a.iter().filter_map(|v| v.as_i64().or_else(|| v.get("at").and_then(Value::as_i64))).collect()),
        None if optional => Ok(Vec::new()),
        None => Err(bad(cmd, "`points` (an array of times) required")),
    }
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let lanes = lanes(e, p, "automation.delete")?;
    let points = points_param(p, "automation.delete", false)?;
    let s = e.session_mut();
    let mut n = 0;
    for (t, param) in &lanes {
        if let Some(l) = s.track_mut(*t).and_then(|tr| tr.automation.iter_mut().find(|l| &l.param == param)) {
            n += l.remove_points(&points);
        }
    }
    Ok(json!({"removed": n}))
}

/// `points: [[at, value], …]` or `[{at, value}, …]`.
fn series_param(p: &Value, cmd: &str) -> Result<Vec<(Samples, f32)>> {
    let arr = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`points` (an array of [at, value]) required"))?;
    let mut out = Vec::with_capacity(arr.len().min(100_000));
    for v in arr {
        let time = |x: Option<&Value>| x.and_then(|x| x.as_i64().or_else(|| x.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64)));
        let (at, value) = match v.as_array() {
            Some(pair) => (time(pair.first()), pair.get(1).and_then(Value::as_f64)),
            None => (time(v.get("at")), v.get("value").and_then(Value::as_f64)),
        };
        match (at, value) {
            (Some(at), Some(value)) if value.is_finite() => out.push((at.max(0), value as f32)),
            _ => return Err(bad(cmd, format!("bad point {v}: want [at, value]"))),
        }
    }
    if out.is_empty() {
        return Err(bad(cmd, "`points` is empty"));
    }
    Ok(out)
}

fn draw(e: &mut Engine, p: &Value) -> Result<Value> {
    let lanes = lanes(e, p, "automation.draw")?;
    let series = series_param(p, "automation.draw")?;
    let tol = f32_or(p, "tolerance", 0.1).clamp(0.0, 100.0);
    let s = e.session_mut();
    let mut n = 0;
    for (t, param) in &lanes {
        if let Some(tr) = s.track_mut(*t) {
            let def = current_value(tr, param);
            let lane = tr.lane_mut(param);
            lane.draw(&series, def, tol);
            n = lane.points.len();
        }
    }
    Ok(json!({"points": n}))
}

fn has_auto_clipboard(e: &Engine) -> std::result::Result<(), String> {
    if e.clipboard.auto_points.is_empty() { Err("no automation points were copied".into()) } else { Ok(()) }
}

fn select(e: &mut Engine, p: &Value) -> Result<Value> {
    let lanes = lanes(e, p, "automation.select")?;
    let r = range_param(e, "automation.select", p)?;
    let s = e.session();
    let points: Vec<Samples> =
        lanes.iter().filter_map(|(t, param)| s.track(*t).and_then(|tr| tr.lane(param))).flat_map(|l| l.points_in(r.start, r.end)).collect();
    Ok(json!({"points": points, "start": r.start, "end": r.end}))
}

fn move_points(e: &mut Engine, p: &Value) -> Result<Value> {
    let lanes = lanes(e, p, "automation.move")?;
    let points = points_param(p, "automation.move", false)?;
    let dt = position_param(e, "automation.move", p, "dt")?.unwrap_or(0);
    let dv = f32_or(p, "dv", 0.0);
    let s = e.session_mut();
    let mut out = Vec::new();
    for (t, param) in &lanes {
        if let Some(l) = s.track_mut(*t).and_then(|tr| tr.automation.iter_mut().find(|l| &l.param == param)) {
            out = l.move_points(&points, dt, dv);
        }
    }
    Ok(json!({"points": out}))
}

fn set_curve(e: &mut Engine, p: &Value) -> Result<Value> {
    let lanes = lanes(e, p, "automation.set_curve")?;
    let at = position_param(e, "automation.set_curve", p, "at")?.ok_or_else(|| bad("automation.set_curve", "`at` required"))?;
    let curve =
        p.get("curve").and_then(Value::as_f64).filter(|c| c.is_finite()).ok_or_else(|| bad("automation.set_curve", "`curve` (-1..1) required"))?;
    let s = e.session_mut();
    let mut n = 0;
    for (t, param) in &lanes {
        if let Some(l) = s.track_mut(*t).and_then(|tr| tr.automation.iter_mut().find(|l| &l.param == param))
            && l.set_curve(at, curve as f32)
        {
            n += 1;
        }
    }
    if n == 0 {
        return Err(bad("automation.set_curve", format!("no breakpoint at {at}")));
    }
    Ok(json!({"curve": (curve as f32).clamp(-1.0, 1.0)}))
}

fn copy(e: &mut Engine, p: &Value) -> Result<Value> {
    let lanes = lanes(e, p, "automation.copy")?;
    let named = points_param(p, "automation.copy", true)?;
    let r = range_param(e, "automation.copy", p)?;
    let s = e.session();
    let Some((t, param)) = lanes.first() else { return Err(bad("automation.copy", "no lane")) };
    let lane = s.track(*t).and_then(|tr| tr.lane(param));
    let mut pts: Vec<AutomationPoint> = lane
        .map(|l| {
            l.points.iter().filter(|x| if named.is_empty() { x.at >= r.start && x.at <= r.end } else { named.contains(&x.at) }).copied().collect()
        })
        .unwrap_or_default();
    if pts.is_empty() {
        return Err(bad("automation.copy", "no breakpoints there"));
    }
    let first = pts.first().map_or(0, |x| x.at);
    for x in &mut pts {
        x.at -= first;
    }
    let n = pts.len();
    e.clipboard.auto_points = pts;
    e.clipboard.auto_param = Some(param.clone());
    Ok(json!({"points": n}))
}

/// Lanes of the same kind: the same value range, both stepped or both ramped.
fn same_kind(a: &AutoParam, b: &AutoParam) -> bool {
    let stepped = |p: &AutoParam| matches!(p, AutoParam::Mute | AutoParam::SendMute(_));
    a.range() == b.range() && stepped(a) == stepped(b)
}

fn paste(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "automation.paste", p)?;
    let copied = e.clipboard.auto_param.clone().ok_or_else(|| bad("automation.paste", "no automation points were copied"))?;
    let lanes = if p.get("param").is_some() {
        lane_params(e, &tracks, p, "automation.paste")?
    } else {
        tracks.iter().map(|t| (*t, copied.clone())).collect()
    };
    let at = position_param(e, "automation.paste", p, "at")?.unwrap_or(e.session().edit.selection.start).max(0);
    let pts = e.clipboard.auto_points.clone();
    let s = e.session_mut();
    let mut out = Vec::new();
    for (t, param) in &lanes {
        if !same_kind(param, &copied) {
            return Err(bad("automation.paste", format!("`{}` points don't fit a `{}` lane", copied.label(), param.label())));
        }
        if let Some(tr) = s.track_mut(*t) {
            let lane = tr.lane_mut(param);
            out.clear();
            for x in &pts {
                let when = at.saturating_add(x.at);
                lane.set_point_curved(when, x.value, Some(x.curve));
                out.push(when);
            }
        }
    }
    Ok(json!({"points": out, "at": at}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::TrackId;

    fn volume_lane(points: &[(i64, f32)]) -> (Engine, TrackId) {
        let mut e = crate::demo::demo_engine();
        let t = e.session().track_by_name("Kick").unwrap().id;
        for (at, v) in points {
            e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": at, "value": v})).unwrap();
        }
        (e, t)
    }

    fn points(e: &Engine, t: TrackId) -> Vec<(i64, f32)> {
        e.session()
            .track(t)
            .and_then(|tr| tr.lane(&AutoParam::Volume))
            .map(|l| l.points.iter().map(|p| (p.at, p.value)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn delete_removes_the_named_points_in_one_undo_step() {
        let (mut e, t) = volume_lane(&[(0, 0.0), (100, -6.0), (200, -12.0), (300, -3.0)]);
        let before = points(&e, t);
        let r = e.execute("automation.delete", &json!({"track": t.0, "param": "volume", "points": [100, 300, 999]})).unwrap();
        assert_eq!(r["removed"], 2);
        assert_eq!(points(&e, t), [(0, 0.0), (200, -12.0)]);
        assert_eq!(e.undo_label(), Some("Delete Automation Points"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(points(&e, t), before);
        assert!(e.execute("automation.delete", &json!({"track": t.0, "param": "volume"})).is_err(), "`points` required");
    }

    #[test]
    fn select_move_copy_and_paste_points() {
        let (mut e, t) = volume_lane(&[(0, 0.0), (100, -6.0), (200, -12.0), (300, -3.0)]);
        let r = e.execute("automation.select", &json!({"track": t.0, "param": "volume", "start": 100, "end": 250})).unwrap();
        assert_eq!(r["points"], json!([100, 200]));
        let r = e.execute("automation.move", &json!({"track": t.0, "param": "volume", "points": [100, 200], "dt": 50, "dv": 1.5})).unwrap();
        assert_eq!(r["points"], json!([150, 250]));
        assert_eq!(points(&e, t), [(0, 0.0), (150, -4.5), (250, -10.5), (300, -3.0)]);
        assert_eq!(e.undo_label(), Some("Move Automation Points"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(points(&e, t), [(0, 0.0), (100, -6.0), (200, -12.0), (300, -3.0)]);
        // Copy two, paste them at 1000 on the same lane and on another track's send level lane
        // (same kind: dB); a pan lane refuses them.
        let undo_len = e.undo_history().len();
        e.execute("automation.copy", &json!({"track": t.0, "param": "volume", "points": [100, 200]})).unwrap();
        assert_eq!(e.undo_history().len(), undo_len, "copy is not an undo step");
        let r = e.execute("automation.paste", &json!({"track": t.0, "at": 1000})).unwrap();
        assert_eq!(r["points"], json!([1000, 1100]));
        assert_eq!(points(&e, t)[4..], [(1000, -6.0), (1100, -12.0)]);
        assert_eq!(e.undo_label(), Some("Paste Automation Points"));
        let snare = e.session().track_by_name("Snare").unwrap().id;
        e.execute("automation.paste", &json!({"track": snare.0, "param": "send_a_level", "at": 0})).unwrap();
        let lane = e.session().track(snare).unwrap().lane(&AutoParam::SendLevel(0)).unwrap();
        assert_eq!(lane.points.iter().map(|p| (p.at, p.value)).collect::<Vec<_>>(), [(0, -6.0), (100, -12.0)]);
        assert!(e.execute("automation.paste", &json!({"track": snare.0, "param": "pan", "at": 0})).is_err());
        // Without `at`, the edit insertion.
        e.execute("edit.select", &json!({"tracks": [t.0], "start": 5000, "end": 5000})).unwrap();
        let r = e.execute("automation.paste", &json!({"track": t.0})).unwrap();
        assert_eq!(r["at"], 5000);
        // Copying a range instead of named points.
        e.execute("automation.copy", &json!({"track": t.0, "param": "volume", "start": 0, "end": 300})).unwrap();
        assert_eq!(e.clipboard.auto_points.len(), 4);
        assert!(e.execute("automation.copy", &json!({"track": t.0, "param": "volume", "points": [7]})).is_err(), "nothing there");
    }

    #[test]
    fn set_curve_bends_a_segment_and_set_point_takes_a_curve() {
        let (mut e, t) = volume_lane(&[(0, 0.0), (1000, -12.0)]);
        let mid = |e: &Engine| e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap().value_at(500, 0.0);
        assert!((mid(&e) + 6.0).abs() < 1e-4);
        e.execute("automation.set_curve", &json!({"track": t.0, "param": "volume", "at": 0, "curve": 1.0})).unwrap();
        assert!((mid(&e) + 12.0 / 16.0).abs() < 1e-3, "{}", mid(&e));
        assert_eq!(e.undo_label(), Some("Curve Automation Segment"));
        assert!(e.execute("automation.set_curve", &json!({"track": t.0, "param": "volume", "at": 5, "curve": 1.0})).is_err());
        e.execute("automation.set_curve", &json!({"track": t.0, "param": "volume", "at": 0, "curve": 0.0})).unwrap();
        assert!((mid(&e) + 6.0).abs() < 1e-4);
        e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": 0, "value": 0.0, "curve": -1.0})).unwrap();
        assert!((mid(&e) + 12.0 * 0.5f32.powf(0.25)).abs() < 1e-3, "{}", mid(&e));
        // Thin keeps the curved segment.
        e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": 500, "value": -6.0})).unwrap();
        e.execute("automation.thin", &json!({"track": t.0, "tolerance": 1.0})).unwrap();
        assert_eq!(e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap().points.len(), 3);
    }

    #[test]
    fn draw_replaces_the_span_keeps_anchors_and_undoes_in_one_step() {
        let (mut e, t) = volume_lane(&[(0, 0.0), (1000, 0.0), (400, -30.0), (500, -30.0), (600, -30.0)]);
        let before = points(&e, t);
        let series: Vec<[f64; 2]> = (0..=30).map(|i| [300.0 + f64::from(i) * 10.0, -10.0 - f64::from(i) * 0.1]).collect();
        let r = e.execute("automation.draw", &json!({"track": t.0, "param": "volume", "points": series, "tolerance": 0.01})).unwrap();
        let after = points(&e, t);
        assert_eq!(r["points"], after.len());
        // The old points under the drag went; the drawn line is thinned to its ends; anchors.
        assert!(!after.iter().any(|(at, v)| [400, 500].contains(at) || (*at == 600 && *v == -30.0)), "{after:?}");
        assert_eq!(after.iter().map(|p| p.0).collect::<Vec<_>>(), [0, 299, 300, 600, 601, 1000], "{after:?}");
        let lane = e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap();
        assert!((lane.value_at(450, 0.0) + 11.5).abs() < 0.05, "{}", lane.value_at(450, 0.0));
        assert!((lane.value_at(200, 0.0) + 15.0).abs() < 0.01, "the old ramp 0 → -30 at 400 holds before the drag: {}", lane.value_at(200, 0.0));
        assert_eq!(e.undo_label(), Some("Draw Automation"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(points(&e, t), before);
        // `{at, value}` objects work too; bad points are refused as a whole.
        e.execute("automation.draw", &json!({"track": t.0, "param": "volume", "points": [{"at": 10, "value": 1.0}, {"at": 20, "value": 2.0}]}))
            .unwrap();
        assert!(e.execute("automation.draw", &json!({"track": t.0, "param": "volume", "points": [[10, "x"]]})).is_err());
        assert!(e.execute("automation.draw", &json!({"track": t.0, "param": "volume", "points": []})).is_err());
    }
}
