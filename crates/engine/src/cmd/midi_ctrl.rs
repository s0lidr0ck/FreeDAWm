//! MIDI controller lanes (the MIDI Editor's CC, pitch bend and aftertouch lanes): list, add,
//! move, edit, delete and draw a clip's controller events.
//!
//! A lane is named `cc1`…`cc127` (also `cc0`, `cc:7`), `pitchbend` (`pb`) or `aftertouch`
//! (`pressure`). Values: 0-127, pitch bend -8192..8191 (0 = centre). Events are addressed by index
//! in the clip's controller list (all lanes, sorted by tick); a command that can re-sort them
//! returns where the events it touched ended up.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::{CtrlEvent, CtrlKind, Sequence};
use soundcraft_model::{ClipContent, ClipId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "midi.ctrls",
            "List MIDI Controllers",
            [],
            None,
            "{clip, lane?: cc1|pitchbend|aftertouch} -> {events: [{index, tick, channel, lane, value}], lanes: [lane with events]}",
            always,
            list
        ),
        cmd!(
            "midi.ctrl_add",
            "Add Controller Event",
            [],
            None,
            "{clip, lane, tick, value, channel?: 0} -> {index} (replaces an event of the lane at that tick)",
            always,
            add
        ),
        cmd!("midi.ctrl_edit", "Edit Controller Event", [], None, "{clip, index, tick?, value?} -> {index}", always, edit),
        cmd!(
            "midi.ctrl_move",
            "Move Controller Events",
            [],
            None,
            "{clip, indices: [n], ticks?: 0, value?: 0 (an offset)} -> {indices}",
            always,
            move_events
        ),
        cmd!("midi.ctrl_delete", "Delete Controller Events", [], None, "{clip, indices: [n]} -> {deleted}", always, delete),
        cmd!(
            "midi.ctrl_draw",
            "Draw Controller",
            [],
            None,
            "{clip, lane, points: [[tick, value]…], channel?: 0} -> {events} — a Pencil pass: replaces the lane's events inside the points' span, repeats dropped",
            always,
            draw
        ),
    ]
}

/// The lane `name` names (see the module docs).
pub fn lane_kind(name: &str) -> Option<CtrlKind> {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "pitchbend" | "pitch bend" | "pb" => return Some(CtrlKind::PitchBend),
        "aftertouch" | "pressure" | "channel pressure" => return Some(CtrlKind::ChannelPressure),
        _ => {}
    }
    let num = n.strip_prefix("cc")?.trim_start_matches([':', ' ']);
    num.parse::<u8>().ok().filter(|c| *c <= 127).map(CtrlKind::Cc)
}

/// The id of a lane, as [`lane_kind`] reads it (`None` for kinds that have no lane).
pub fn lane_id(kind: CtrlKind) -> Option<String> {
    match kind {
        CtrlKind::Cc(n) => Some(format!("cc{n}")),
        CtrlKind::PitchBend => Some("pitchbend".into()),
        CtrlKind::ChannelPressure => Some("aftertouch".into()),
        CtrlKind::PolyPressure(_) | CtrlKind::Program => None,
    }
}

/// A lane's value range.
pub fn lane_range(kind: CtrlKind) -> (i32, i32) {
    match kind {
        CtrlKind::PitchBend => (-8192, 8191),
        _ => (0, 127),
    }
}

/// Whether two kinds are the same lane.
fn same(a: CtrlKind, b: CtrlKind) -> bool {
    a == b
}

fn lane_param(p: &Value, cmd: &str) -> Result<CtrlKind> {
    let name = str_param(p, "lane").ok_or_else(|| bad(cmd, "`lane` required (cc1…cc127, pitchbend, aftertouch)"))?;
    lane_kind(name).ok_or_else(|| bad(cmd, format!("unknown lane '{name}': use cc0…cc127, pitchbend or aftertouch")))
}

fn seq_mut<'a>(e: &'a mut Engine, id: ClipId, cmd: &str) -> Result<&'a mut Sequence> {
    match e.session_mut().find_clip_mut(id).map(|c| &mut c.content) {
        Some(ClipContent::Midi { sequence }) => Ok(sequence),
        Some(_) => Err(bad(cmd, "not a MIDI clip")),
        None => Err(bad(cmd, "no such clip")),
    }
}

fn indices(p: &Value) -> Vec<usize> {
    p.get("indices").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as usize).collect()).unwrap_or_default()
}

/// Sorts the controller events by tick (stable); returns where the events at `marked` (old
/// indices) ended up.
fn sort_tracked(seq: &mut Sequence, marked: &[usize]) -> Vec<usize> {
    let mut work: Vec<(CtrlEvent, bool)> = seq.ctrls.iter().enumerate().map(|(i, c)| (*c, marked.contains(&i))).collect();
    work.sort_by_key(|(c, _)| c.tick);
    let kept = work.iter().enumerate().filter(|(_, (_, f))| *f).map(|(i, _)| i).collect();
    seq.ctrls = work.into_iter().map(|(c, _)| c).collect();
    kept
}

fn clamp_value(kind: CtrlKind, v: i64) -> i32 {
    let (lo, hi) = lane_range(kind);
    v.clamp(i64::from(lo), i64::from(hi)) as i32
}

fn list(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrls";
    let id = clip_id(p, C)?;
    let only = match str_param(p, "lane") {
        Some(_) => Some(lane_param(p, C)?),
        None => None,
    };
    let Some((_, clip)) = e.session().find_clip(id) else { return Err(bad(C, "no such clip")) };
    let ClipContent::Midi { sequence } = &clip.content else { return Err(bad(C, "not a MIDI clip")) };
    let mut lanes: Vec<String> = Vec::new();
    let mut events = Vec::new();
    for (i, c) in sequence.ctrls.iter().enumerate() {
        let Some(name) = lane_id(c.kind) else { continue };
        if !lanes.contains(&name) {
            lanes.push(name.clone());
        }
        if only.is_none_or(|k| same(k, c.kind)) {
            events.push(json!({"index": i, "tick": c.tick, "channel": c.channel, "lane": name, "value": c.value}));
        }
    }
    Ok(json!({"events": events, "lanes": lanes}))
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_add";
    let id = clip_id(p, C)?;
    let kind = lane_param(p, C)?;
    let tick = p.get("tick").and_then(Value::as_i64).ok_or_else(|| bad(C, "`tick` required"))?.max(0);
    let value = p.get("value").and_then(Value::as_i64).ok_or_else(|| bad(C, "`value` required"))?;
    let channel = i64_or(p, "channel", 0).clamp(0, 15) as u8;
    let seq = seq_mut(e, id, C)?;
    seq.ctrls.retain(|c| !(c.tick == tick && c.channel == channel && same(c.kind, kind)));
    seq.ctrls.push(CtrlEvent { tick, channel, kind, value: clamp_value(kind, value) });
    let last = seq.ctrls.len() - 1;
    let kept = sort_tracked(seq, &[last]);
    Ok(json!({"index": kept.first()}))
}

fn edit(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_edit";
    let id = clip_id(p, C)?;
    let idx = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad(C, "`index` required"))? as usize;
    let seq = seq_mut(e, id, C)?;
    let c = seq.ctrls.get_mut(idx).ok_or_else(|| bad(C, "index out of range"))?;
    if let Some(t) = p.get("tick").and_then(Value::as_i64) {
        c.tick = t.max(0);
    }
    if let Some(v) = p.get("value").and_then(Value::as_i64) {
        c.value = clamp_value(c.kind, v);
    }
    let kept = sort_tracked(seq, &[idx]);
    Ok(json!({"index": kept.first()}))
}

fn move_events(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_move";
    let id = clip_id(p, C)?;
    let idx = indices(p);
    let dt = i64_or(p, "ticks", 0);
    let dv = i64_or(p, "value", 0);
    let seq = seq_mut(e, id, C)?;
    for (i, c) in seq.ctrls.iter_mut().enumerate() {
        if idx.contains(&i) {
            c.tick = c.tick.saturating_add(dt).max(0);
            c.value = clamp_value(c.kind, i64::from(c.value).saturating_add(dv));
        }
    }
    let kept = sort_tracked(seq, &idx);
    Ok(json!({"indices": kept}))
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_delete";
    let id = clip_id(p, C)?;
    let idx = indices(p);
    let seq = seq_mut(e, id, C)?;
    let before = seq.ctrls.len();
    let mut i = 0;
    seq.ctrls.retain(|_| {
        let keep = !idx.contains(&i);
        i += 1;
        keep
    });
    Ok(json!({"deleted": before - seq.ctrls.len()}))
}

/// The most points one Pencil pass takes.
const MAX_POINTS: usize = 100_000;

fn draw(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_draw";
    let id = clip_id(p, C)?;
    let kind = lane_param(p, C)?;
    let channel = i64_or(p, "channel", 0).clamp(0, 15) as u8;
    let arr = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(C, "`points` (an array of [tick, value]) required"))?;
    if arr.is_empty() || arr.len() > MAX_POINTS {
        return Err(bad(C, format!("`points` must hold 1 to {MAX_POINTS} points")));
    }
    let mut pts: Vec<(i64, i32)> = Vec::with_capacity(arr.len());
    for v in arr {
        let pair = v.as_array().map(|a| (a.first().and_then(Value::as_i64), a.get(1).and_then(Value::as_i64)));
        match pair {
            Some((Some(t), Some(val))) => pts.push((t.max(0), clamp_value(kind, val))),
            _ => return Err(bad(C, format!("bad point {v}: want [tick, value]"))),
        }
    }
    // One event a tick (the last drawn wins), in time order, without repeats of the same value.
    pts.sort_by_key(|x| x.0);
    let mut thin: Vec<(i64, i32)> = Vec::with_capacity(pts.len());
    for (t, v) in pts {
        match thin.last_mut() {
            Some(last) if last.0 == t => last.1 = v,
            _ => thin.push((t, v)),
        }
    }
    thin.dedup_by(|later, kept| later.1 == kept.1);
    let lo = thin.first().map_or(0, |x| x.0);
    let hi = thin.last().map_or(0, |x| x.0);
    let seq = seq_mut(e, id, C)?;
    seq.ctrls.retain(|c| !(same(c.kind, kind) && c.channel == channel && c.tick >= lo && c.tick <= hi));
    let n = thin.len();
    seq.ctrls.extend(thin.into_iter().map(|(tick, value)| CtrlEvent { tick, channel, kind, value }));
    seq.ctrls.sort_by_key(|c| c.tick);
    Ok(json!({"events": n}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    fn clip(e: &mut Engine) -> u64 {
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        e.execute("midi.new_clip", &json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap()
    }

    /// (tick, value) of the lane's events.
    fn lane(e: &mut Engine, clip: u64, lane: &str) -> Vec<(i64, i64)> {
        let r = e.execute("midi.ctrls", &json!({"clip": clip, "lane": lane})).unwrap();
        r["events"].as_array().unwrap().iter().map(|x| (x["tick"].as_i64().unwrap(), x["value"].as_i64().unwrap())).collect()
    }

    #[test]
    fn lane_names() {
        assert_eq!(lane_kind("cc1"), Some(CtrlKind::Cc(1)));
        assert_eq!(lane_kind("CC:64"), Some(CtrlKind::Cc(64)));
        assert_eq!(lane_kind("pb"), Some(CtrlKind::PitchBend));
        assert_eq!(lane_kind("aftertouch"), Some(CtrlKind::ChannelPressure));
        assert_eq!(lane_kind("cc128"), None);
        assert_eq!(lane_kind("volume"), None);
        assert_eq!(lane_id(CtrlKind::Cc(11)).as_deref(), Some("cc11"));
    }

    #[test]
    fn add_edit_move_delete_keep_lanes_apart() {
        let mut e = Engine::default();
        let c = clip(&mut e);
        e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc1", "tick": 480, "value": 64})).unwrap();
        e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc1", "tick": 0, "value": 200})).unwrap();
        e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "pitchbend", "tick": 240, "value": -9000})).unwrap();
        assert_eq!(lane(&mut e, c, "cc1"), vec![(0, 127), (480, 64)]);
        assert_eq!(lane(&mut e, c, "pb"), vec![(240, -8192)]);
        // Adding at a taken tick replaces that event.
        let r = e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc1", "tick": 480, "value": 10})).unwrap();
        assert_eq!(r["index"], 2);
        assert_eq!(lane(&mut e, c, "cc1"), vec![(0, 127), (480, 10)]);
        let all = e.execute("midi.ctrls", &json!({"clip": c})).unwrap();
        assert_eq!(all["lanes"], json!(["cc1", "pitchbend"]));
        // Move the first CC past the pitch bend: it is found again where it sorted.
        let r = e.execute("midi.ctrl_move", &json!({"clip": c, "indices": [0], "ticks": 300, "value": -27})).unwrap();
        assert_eq!(r["indices"], json!([1]));
        assert_eq!(lane(&mut e, c, "cc1"), vec![(300, 100), (480, 10)]);
        let r = e.execute("midi.ctrl_edit", &json!({"clip": c, "index": 1, "tick": 960, "value": 5})).unwrap();
        assert_eq!(r["index"], 2);
        assert_eq!(lane(&mut e, c, "cc1"), vec![(480, 10), (960, 5)]);
        e.execute("midi.ctrl_delete", &json!({"clip": c, "indices": [0, 99]})).unwrap();
        assert_eq!(lane(&mut e, c, "pb"), vec![]);
        assert_eq!(e.undo_label(), Some("Delete Controller Events"));
        assert!(e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc200", "tick": 0, "value": 1})).is_err());
        assert!(e.execute("midi.ctrl_edit", &json!({"clip": c, "index": 50})).is_err());
    }

    #[test]
    fn a_pencil_pass_replaces_the_span_and_drops_repeats() {
        let mut e = Engine::default();
        let c = clip(&mut e);
        for (t, v) in [(0, 10), (500, 20), (2000, 30)] {
            e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc11", "tick": t, "value": v})).unwrap();
        }
        e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc1", "tick": 600, "value": 1})).unwrap();
        let r = e
            .execute("midi.ctrl_draw", &json!({"clip": c, "lane": "cc11", "points": [[400, 50], [450, 50], [600, 60], [600, 61], [800, 70]]}))
            .unwrap();
        assert_eq!(r["events"], 3);
        assert_eq!(lane(&mut e, c, "cc11"), vec![(0, 10), (400, 50), (600, 61), (800, 70), (2000, 30)]);
        // Other lanes are untouched; one undo step.
        assert_eq!(lane(&mut e, c, "cc1"), vec![(600, 1)]);
        assert!(e.undo());
        assert_eq!(lane(&mut e, c, "cc11"), vec![(0, 10), (500, 20), (2000, 30)]);
        assert!(e.execute("midi.ctrl_draw", &json!({"clip": c, "lane": "cc11", "points": []})).is_err());
        assert!(e.execute("midi.ctrl_draw", &json!({"clip": c, "lane": "cc11", "points": [[1, "x"]]})).is_err());
    }
}
