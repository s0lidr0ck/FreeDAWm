//! MIDI controller lanes (the MIDI Editor's CC, pitch bend, aftertouch, poly pressure and program
//! lanes): list, add, move, edit, delete and draw a clip's controller events; draw a line or a
//! curve, ramp between points, scale and thin them.
//!
//! A lane is named `cc1`…`cc127` (also `cc0`, `cc:7`), `pitchbend` (`pb`), `aftertouch`
//! (`pressure`), `polyat60` (poly pressure on one key; `poly60`) or `program` (`pc`). Values:
//! 0-127, pitch bend -8192..8191 (0 = centre). Lines, curves and ramps are written as a point
//! whose segment ramps to the next point of its lane and channel ([`CtrlShape::Ramp`]), played as
//! a stream of values; `stepped` writes the steps instead. Events are addressed by index
//! in the clip's controller list (all lanes, sorted by tick); a command that can re-sort them
//! returns where the events it touched ended up.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::{CtrlEvent, CtrlKind, CtrlShape, Sequence};
use soundcraft_model::{ClipContent, ClipId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "midi.ctrls",
            "List MIDI Controllers",
            [],
            None,
            "{clip, lane?: cc1|pitchbend|aftertouch} -> {events: [{index, tick, channel, lane, value, ramp?: curve}], lanes: [lane with events]}",
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
        cmd!("midi.ctrl_edit", "Edit Controller Event", [], None, "{clip, index, tick?, value?, channel?: 0..15} -> {index}", always, edit),
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
        cmd!(
            "midi.ctrl_line",
            "Draw Controller Line",
            [],
            None,
            "{clip, lane, from: [tick, value], to: [tick, value], curve?: 0 (-1..1: below 0 starts slow, above 0 starts fast), channel?: 0, stepped?: false, step?: 30 ticks} -> {events} — replaces the lane's events on the channel between the ends with the two points and a ramp from one to the other (stepped: steps, one every `step` ticks at most)",
            always,
            line
        ),
        cmd!(
            "midi.ctrl_ramp",
            "Ramp Controller Points",
            [],
            None,
            "{clip, indices: [n], curve?: 0, shape?: ramp|step, stepped?: false, step?: 30 ticks} -> {events} — between each picked point and the next of its lane and channel, the points between go and the first ramps to the next (shape step: holds to it; stepped: steps written out)",
            always,
            ramp
        ),
        cmd!(
            "midi.ctrl_scale",
            "Scale Controller Points",
            [],
            None,
            "{clip, indices: [n], percent?: 100, add?: 0} -> {indices} — each value times percent (from 0, or from the centre for pitch bend), plus add",
            always,
            scale
        ),
        cmd!(
            "midi.ctrl_thin",
            "Thin Controller Points",
            [],
            None,
            "{clip, indices: [n], tolerance?: 2} -> {removed} — drops the picked points a straight line between their kept neighbours (same lane and channel) passes within `tolerance` of, and repeats",
            always,
            thin
        ),
    ]
}

/// The lane `name` names (see the module docs).
pub fn lane_kind(name: &str) -> Option<CtrlKind> {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "pitchbend" | "pitch bend" | "pb" => return Some(CtrlKind::PitchBend),
        "aftertouch" | "pressure" | "channel pressure" => return Some(CtrlKind::ChannelPressure),
        "program" | "pc" | "program change" => return Some(CtrlKind::Program),
        _ => {}
    }
    for prefix in ["polyat", "poly", "polypressure"] {
        if let Some(num) = n.strip_prefix(prefix).map(|x| x.trim_start_matches([':', ' ']))
            && let Ok(p) = num.parse::<u8>()
        {
            return (p <= 127).then_some(CtrlKind::PolyPressure(p));
        }
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
        CtrlKind::PolyPressure(p) => Some(format!("polyat{p}")),
        CtrlKind::Program => Some("program".into()),
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
    lane_kind(name).ok_or_else(|| bad(cmd, format!("unknown lane '{name}': use cc0…cc127, pitchbend, aftertouch, polyat0…polyat127 or program")))
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
            let mut ev = json!({"index": i, "tick": c.tick, "channel": c.channel, "lane": name, "value": c.value});
            if let CtrlShape::Ramp(curve) = c.shape {
                ev["ramp"] = json!(curve);
            }
            events.push(ev);
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
    seq.ctrls.push(CtrlEvent { tick, channel, kind, value: clamp_value(kind, value), shape: Default::default() });
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
    if let Some(ch) = p.get("channel").and_then(Value::as_i64) {
        c.channel = ch.clamp(0, 15) as u8;
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
    seq.ctrls.extend(thin.into_iter().map(|(tick, value)| CtrlEvent { tick, channel, kind, value, shape: Default::default() }));
    seq.ctrls.sort_by_key(|c| c.tick);
    Ok(json!({"events": n}))
}

/// The points of a ramp from `a` to `b` (tick, value): the value where it changes, at most one
/// every `step` ticks, `b` exactly at its end. `curve` -1..1 bends it (0: straight).
pub fn ramp_points(kind: CtrlKind, a: (i64, i32), b: (i64, i32), curve: f64, step: i64) -> Vec<(i64, i32)> {
    let (a, b) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let span = b.0.saturating_sub(a.0);
    if span == 0 {
        return vec![(b.0, clamp_value(kind, i64::from(b.1)))];
    }
    let k = if curve.is_finite() { curve.clamp(-1.0, 1.0) * 5.0 } else { 0.0 };
    let shape = |x: f64| if k.abs() < 1e-6 { x } else { (1.0 - (-k * x).exp()) / (1.0 - (-k).exp()) };
    let step = step.clamp(1, 3840);
    let mut out: Vec<(i64, i32)> = Vec::new();
    let mut t = a.0;
    while t < b.0 && out.len() < MAX_POINTS {
        let x = (t - a.0) as f64 / span as f64;
        let v = f64::from(a.1) + f64::from(b.1 - a.1) * shape(x);
        let v = clamp_value(kind, v.round() as i64);
        if out.last().is_none_or(|l| l.1 != v) {
            out.push((t, v));
        }
        t = t.saturating_add(step);
    }
    let end = clamp_value(kind, i64::from(b.1));
    match out.last() {
        Some(l) if l.1 == end => {}
        _ => out.push((b.0, end)),
    }
    out
}

/// Replaces `kind`'s events on `channel` in `lo..=hi` with `pts`.
fn write_span(seq: &mut Sequence, kind: CtrlKind, channel: u8, lo: i64, hi: i64, pts: &[(i64, i32)]) {
    seq.ctrls.retain(|c| !(same(c.kind, kind) && c.channel == channel && c.tick >= lo && c.tick <= hi));
    seq.ctrls.extend(pts.iter().map(|(tick, value)| CtrlEvent { tick: *tick, channel, kind, value: *value, shape: Default::default() }));
    seq.ctrls.sort_by_key(|c| c.tick);
}

fn pair(p: &Value, key: &str) -> Option<(i64, i64)> {
    let a = p.get(key)?.as_array()?;
    Some((a.first()?.as_i64()?, a.get(1)?.as_i64()?))
}

fn line(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_line";
    let id = clip_id(p, C)?;
    let kind = lane_param(p, C)?;
    let channel = i64_or(p, "channel", 0).clamp(0, 15) as u8;
    let (Some(a), Some(b)) = (pair(p, "from"), pair(p, "to")) else { return Err(bad(C, "`from` and `to` ([tick, value]) required")) };
    let (a, b) = ((a.0.max(0), clamp_value(kind, a.1)), (b.0.max(0), clamp_value(kind, b.1)));
    let curve = p.get("curve").and_then(Value::as_f64).unwrap_or(0.0);
    let seq = seq_mut(e, id, C)?;
    // Two points and the segment between them; with `stepped`, the steps as before ramps existed.
    let pts = if p.get("stepped").and_then(Value::as_bool).unwrap_or(false) {
        ramp_points(kind, a, b, curve, i64_or(p, "step", 30))
    } else if a.0 == b.0 {
        vec![b]
    } else {
        vec![(a.0.min(b.0), if a.0 <= b.0 { a.1 } else { b.1 }), (a.0.max(b.0), if a.0 <= b.0 { b.1 } else { a.1 })]
    };
    write_span(seq, kind, channel, a.0.min(b.0), a.0.max(b.0), &pts);
    if pts.len() == 2 && !p.get("stepped").and_then(Value::as_bool).unwrap_or(false) {
        let first = pts.first().map_or(0, |x| x.0);
        if let Some(c) = seq.ctrls.iter_mut().find(|c| same(c.kind, kind) && c.channel == channel && c.tick == first) {
            c.shape = segment(curve);
        }
    }
    Ok(json!({"events": pts.len()}))
}

/// The ramp shape a `curve` parameter asks for (-1..1).
fn segment(curve: f64) -> CtrlShape {
    CtrlShape::Ramp(if curve.is_finite() { curve.clamp(-1.0, 1.0) as f32 } else { 0.0 })
}

fn ramp(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_ramp";
    let id = clip_id(p, C)?;
    let idx = indices(p);
    let curve = p.get("curve").and_then(Value::as_f64).unwrap_or(0.0);
    let to_steps = matches!(str_param(p, "shape"), Some("step" | "steps"));
    let stepped = p.get("stepped").and_then(Value::as_bool).unwrap_or(false);
    let step = i64_or(p, "step", 30);
    let seq = seq_mut(e, id, C)?;
    let mut picked: Vec<CtrlEvent> = idx.iter().filter_map(|i| seq.ctrls.get(*i).copied()).collect();
    picked.sort_by_key(|c| c.tick);
    let mut n = 0;
    for (i, a) in picked.iter().enumerate() {
        // The next picked point of the same lane and channel.
        let Some(b) = picked.iter().skip(i + 1).find(|b| same(b.kind, a.kind) && b.channel == a.channel && b.tick > a.tick) else { continue };
        if stepped {
            let pts = ramp_points(a.kind, (a.tick, a.value), (b.tick, b.value), curve, step);
            write_span(seq, a.kind, a.channel, a.tick, b.tick, &pts);
            n += pts.len();
            continue;
        }
        // The points between go; the first point ramps (or steps) to the second.
        seq.ctrls.retain(|c| !(same(c.kind, a.kind) && c.channel == a.channel && c.tick > a.tick && c.tick < b.tick));
        if let Some(c) = seq.ctrls.iter_mut().find(|c| same(c.kind, a.kind) && c.channel == a.channel && c.tick == a.tick) {
            c.shape = if to_steps { CtrlShape::Step } else { segment(curve) };
        }
        n += 2;
    }
    if n == 0 {
        return Err(bad(C, "pick at least two points of one lane"));
    }
    Ok(json!({"events": n}))
}

fn scale(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_scale";
    let id = clip_id(p, C)?;
    let idx = indices(p);
    let pct = p.get("percent").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(100.0).clamp(0.0, 1000.0);
    let add = i64_or(p, "add", 0).clamp(-16_384, 16_384);
    let seq = seq_mut(e, id, C)?;
    for i in &idx {
        if let Some(c) = seq.ctrls.get_mut(*i) {
            c.value = clamp_value(c.kind, (f64::from(c.value) * pct / 100.0).round() as i64 + add);
        }
    }
    Ok(json!({"indices": idx}))
}

fn thin(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl_thin";
    let id = clip_id(p, C)?;
    let idx = indices(p);
    let tol = i64_or(p, "tolerance", 2).clamp(0, 8192) as f64;
    let seq = seq_mut(e, id, C)?;
    let before = seq.ctrls.len();
    thin_points(seq, &idx, |_| tol);
    Ok(json!({"removed": before - seq.ctrls.len()}))
}

/// Drops the points `idx` names (indices into `seq.ctrls`) that a straight line between their
/// kept neighbours (same lane and channel) passes within `tol(kind)` of, or that repeat the kept
/// value before them. Per lane and channel, in time order.
pub(crate) fn thin_points(seq: &mut soundcraft_midi::Sequence, idx: &[usize], tol: impl Fn(CtrlKind) -> f64) {
    let mut drop: Vec<usize> = Vec::new();
    let mut lanes: Vec<(CtrlKind, u8)> = Vec::new();
    for i in idx {
        if let Some(c) = seq.ctrls.get(*i)
            && !lanes.iter().any(|(k, ch)| same(*k, c.kind) && *ch == c.channel)
        {
            lanes.push((c.kind, c.channel));
        }
    }
    for (kind, ch) in lanes {
        let tol = tol(kind);
        let lane: Vec<usize> = (0..seq.ctrls.len()).filter(|i| seq.ctrls.get(*i).is_some_and(|c| same(c.kind, kind) && c.channel == ch)).collect();
        let mut kept: Option<CtrlEvent> = None;
        for (k, i) in lane.iter().enumerate() {
            let Some(c) = seq.ctrls.get(*i).copied() else { continue };
            let next = lane.get(k + 1).and_then(|j| seq.ctrls.get(*j)).copied();
            let goes = idx.contains(i)
                && match (kept, next) {
                    (Some(a), _) if a.value == c.value => true,
                    (Some(a), Some(b)) if b.tick > a.tick => {
                        let x = (c.tick - a.tick) as f64 / (b.tick - a.tick) as f64;
                        (f64::from(a.value) + f64::from(b.value - a.value) * x - f64::from(c.value)).abs() <= tol
                    }
                    _ => false,
                };
            if goes {
                drop.push(*i);
            } else {
                kept = Some(c);
            }
        }
    }
    drop.sort_unstable();
    let mut i = 0;
    seq.ctrls.retain(|_| {
        let keep = drop.binary_search(&i).is_err();
        i += 1;
        keep
    });
}

/// Controller data thinned on the way in (Setup › MIDI Input Devices › Thin controllers): per
/// lane and channel, a controller, pitch bend or pressure point that the straight line from the
/// last kept point to the next passes within one step of (a 64th of the range for pitch bend) is
/// dropped, and the kept point before it ramps to the next (`CtrlShape::Ramp`), so a sweep plays
/// as smoothly as it was recorded with a fraction of its points. Pedals and programs are kept.
/// Returns how many went.
pub fn thin_take(seq: &mut soundcraft_midi::Sequence) -> usize {
    use soundcraft_midi::CtrlShape;
    let mut lanes: Vec<(CtrlKind, u8)> = Vec::new();
    for c in &seq.ctrls {
        if !matches!(c.kind, CtrlKind::Program | CtrlKind::Cc(64..=69)) && !lanes.contains(&(c.kind, c.channel)) {
            lanes.push((c.kind, c.channel));
        }
    }
    let mut drop: Vec<usize> = Vec::new();
    let mut ramps: Vec<usize> = Vec::new();
    for (kind, ch) in lanes {
        let tol = if kind == CtrlKind::PitchBend { 128.0 } else { 1.0 };
        let lane: Vec<usize> = (0..seq.ctrls.len()).filter(|i| seq.ctrls.get(*i).is_some_and(|c| c.kind == kind && c.channel == ch)).collect();
        let mut kept: Option<usize> = None;
        for (k, i) in lane.iter().enumerate() {
            let (Some(c), Some(next)) = (seq.ctrls.get(*i).copied(), lane.get(k + 1).and_then(|j| seq.ctrls.get(*j)).copied()) else { continue };
            let on_line = kept.and_then(|a| seq.ctrls.get(a)).is_some_and(|a| {
                next.tick > a.tick && a.shape.is_step() && {
                    let x = (c.tick - a.tick) as f64 / (next.tick - a.tick) as f64;
                    (f64::from(a.value) + f64::from(next.value - a.value) * x - f64::from(c.value)).abs() <= tol
                }
            });
            // A held value (the same before and after) is not a ramp: it stays held.
            let held = kept.and_then(|a| seq.ctrls.get(a)).is_some_and(|a| a.value == c.value && c.value == next.value);
            match kept {
                Some(a) if on_line || held => {
                    drop.push(*i);
                    if !held && !ramps.contains(&a) {
                        ramps.push(a);
                    }
                }
                _ => kept = Some(*i),
            }
        }
    }
    for a in &ramps {
        if let Some(c) = seq.ctrls.get_mut(*a) {
            c.shape = CtrlShape::Ramp(0.0);
        }
    }
    drop.sort_unstable();
    let before = seq.ctrls.len();
    let mut i = 0;
    seq.ctrls.retain(|_| {
        let keep = drop.binary_search(&i).is_err();
        i += 1;
        keep
    });
    before - seq.ctrls.len()
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

    /// Thinned on the way in: a mod-wheel sweep keeps its ends and ramps between them, a held
    /// value stays held, a turn stays, the pedal is untouched.
    #[test]
    fn a_take_keeps_the_points_that_shape_a_move() {
        use soundcraft_midi::{CtrlEvent, CtrlShape, Sequence};
        let ev = |tick: i64, kind: CtrlKind, value: i32| CtrlEvent { tick, channel: 0, kind, value, shape: CtrlShape::Step };
        let mut seq = Sequence::default();
        // 0 → 100 in steps of 1, back down to 50, then held there.
        for v in 0..=100 {
            seq.ctrls.push(ev(i64::from(v) * 10, CtrlKind::Cc(1), v));
        }
        for v in (50..100).rev() {
            seq.ctrls.push(ev(1_000 + i64::from(100 - v) * 10, CtrlKind::Cc(1), v));
        }
        for k in 0..5 {
            seq.ctrls.push(ev(1_600 + k * 10, CtrlKind::Cc(1), 50));
        }
        seq.ctrls.push(ev(0, CtrlKind::Cc(64), 127));
        seq.ctrls.push(ev(5, CtrlKind::Cc(64), 127));
        seq.sort();
        let removed = super::thin_take(&mut seq);
        let cc1: Vec<(i64, i32, bool)> =
            seq.ctrls.iter().filter(|c| c.kind == CtrlKind::Cc(1)).map(|c| (c.tick, c.value, c.shape.is_step())).collect();
        assert_eq!(cc1, vec![(0, 0, false), (1_000, 100, false), (1_500, 50, true), (1_640, 50, true)], "{cc1:?}");
        assert_eq!(seq.ctrls.iter().filter(|c| c.kind == CtrlKind::Cc(64)).count(), 2, "the pedal kept as played");
        assert_eq!(removed, 101 + 50 + 5 - 4);
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
    fn program_and_poly_pressure_have_lanes() {
        assert_eq!(lane_kind("program"), Some(CtrlKind::Program));
        assert_eq!(lane_kind("polyat60"), Some(CtrlKind::PolyPressure(60)));
        assert_eq!(lane_kind("poly:61"), Some(CtrlKind::PolyPressure(61)));
        assert_eq!(lane_kind("polyat128"), None);
        assert_eq!(lane_id(CtrlKind::PolyPressure(60)).as_deref(), Some("polyat60"));
        let mut e = Engine::default();
        let c = clip(&mut e);
        e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "program", "tick": 0, "value": 5})).unwrap();
        e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "polyat60", "tick": 10, "value": 90})).unwrap();
        let all = e.execute("midi.ctrls", &json!({"clip": c})).unwrap();
        assert_eq!(all["lanes"], json!(["program", "polyat60"]));
        assert_eq!(lane(&mut e, c, "polyat61"), vec![]);
    }

    #[test]
    fn lines_ramps_scale_and_thin() {
        // A straight line: values where they change, every 30 ticks at most, the end exact.
        let pts = ramp_points(CtrlKind::Cc(1), (0, 0), (300, 100), 0.0, 30);
        assert_eq!(pts.first(), Some(&(0, 0)));
        assert_eq!(pts.last(), Some(&(300, 100)));
        assert_eq!(pts.len(), 11);
        assert!(pts.windows(2).all(|w| w[1].0 - w[0].0 >= 30 && w[1].1 > w[0].1));
        // A curve starting fast is above the line in the middle; one starting slow below.
        let mid = |curve| ramp_points(CtrlKind::Cc(1), (0, 0), (300, 100), curve, 30).iter().find(|p| p.0 == 150).map(|p| p.1);
        assert!(mid(1.0).unwrap() > 60 && mid(-1.0).unwrap() < 40);
        // A flat line is one point; a reversed one is put in order.
        assert_eq!(ramp_points(CtrlKind::Cc(1), (0, 64), (480, 64), 0.0, 30), vec![(0, 64)]);
        assert_eq!(ramp_points(CtrlKind::Cc(1), (300, 0), (0, 10), 0.0, 100).first(), Some(&(0, 10)));

        let mut e = Engine::default();
        let c = clip(&mut e);
        e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc1", "tick": 960, "value": 7})).unwrap();
        e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc1", "tick": 100, "value": 99, "channel": 1})).unwrap();
        e.execute("midi.ctrl_line", &json!({"clip": c, "lane": "cc1", "from": [0, 0], "to": [1920, 127], "step": 60})).unwrap();
        let l = lane(&mut e, c, "cc1");
        assert!(!l.contains(&(960, 7)), "the line replaced what was under it");
        assert!(l.contains(&(100, 99)), "another channel's point stays");
        assert_eq!(l.last(), Some(&(1920, 127)));
        // The line is its two points, the first ramping to the second.
        assert_eq!(l, vec![(0, 0), (100, 99), (1920, 127)]);
        assert_eq!(e.execute("midi.ctrls", &json!({"clip": c, "lane": "cc1"})).unwrap()["events"][0]["ramp"], json!(0.0));
        // `stepped` writes the steps instead.
        e.execute("midi.ctrl_line", &json!({"clip": c, "lane": "cc1", "from": [0, 0], "to": [1920, 127], "step": 60, "stepped": true})).unwrap();
        assert!(lane(&mut e, c, "cc1").len() > 20);
        // Ramp between two points (indices of the steps 0 and 127 at 3000 and 3960).
        let mut e = Engine::default();
        let c = clip(&mut e);
        for (t, v) in [(3000, 0), (3480, 30), (3960, 120)] {
            e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc11", "tick": t, "value": v})).unwrap();
        }
        e.execute("midi.ctrl_ramp", &json!({"clip": c, "indices": [0, 2]})).unwrap();
        assert_eq!(lane(&mut e, c, "cc11"), vec![(3000, 0), (3960, 120)], "the step between goes; the first point ramps");
        let ramp = |e: &mut Engine| e.execute("midi.ctrls", &json!({"clip": c, "lane": "cc11"})).unwrap()["events"][0]["ramp"].clone();
        assert_eq!(ramp(&mut e), json!(0.0));
        e.execute("midi.ctrl_ramp", &json!({"clip": c, "indices": [0, 1], "shape": "step"})).unwrap();
        assert_eq!(ramp(&mut e), Value::Null, "back to a step");
        // The old way: the steps written out.
        e.execute("midi.ctrl_ramp", &json!({"clip": c, "indices": [0, 1], "stepped": true, "step": 120})).unwrap();
        let l = lane(&mut e, c, "cc11");
        assert_eq!((l.first(), l.last(), l.len()), (Some(&(3000, 0)), Some(&(3960, 120)), 9));
        assert!(l.windows(2).all(|w| w[1].1 >= w[0].1), "{l:?}");
        // Thin the ramp's inner points: a straight line keeps only its ends.
        let n = l.len();
        let all: Vec<usize> = (0..n).collect();
        let r = e.execute("midi.ctrl_thin", &json!({"clip": c, "indices": all})).unwrap();
        assert_eq!(r["removed"].as_u64().unwrap() as usize, n - 2);
        assert_eq!(lane(&mut e, c, "cc11"), vec![(3000, 0), (3960, 120)]);
        // Scale: half, plus 10.
        e.execute("midi.ctrl_scale", &json!({"clip": c, "indices": [0, 1], "percent": 50, "add": 10})).unwrap();
        assert_eq!(lane(&mut e, c, "cc11"), vec![(3000, 10), (3960, 70)]);
        assert!(e.execute("midi.ctrl_ramp", &json!({"clip": c, "indices": [0]})).is_err());
    }

    #[test]
    fn controllers_can_follow_their_notes() {
        let mut e = Engine::default();
        let c = clip(&mut e);
        e.execute("midi.note_add", &json!({"clip": c, "pitch": 60, "start_ticks": 0, "length_ticks": 960})).unwrap();
        for (t, v) in [(0, 10), (480, 20), (1920, 30), (2400, 40)] {
            e.execute("midi.ctrl_add", &json!({"clip": c, "lane": "cc1", "tick": t, "value": v})).unwrap();
        }
        // Without the option, controllers stay.
        e.execute("midi.notes_move", &json!({"clip": c, "indices": [0], "ticks": 1920})).unwrap();
        assert_eq!(lane(&mut e, c, "cc1"), vec![(0, 10), (480, 20), (1920, 30), (2400, 40)]);
        assert!(e.undo());
        // With it, the two under the note move and replace those where they land.
        e.execute("midi.notes_move", &json!({"clip": c, "indices": [0], "ticks": 1920, "with_ctrls": true})).unwrap();
        assert_eq!(lane(&mut e, c, "cc1"), vec![(1920, 10), (2400, 20)]);
        assert!(e.undo());
        // A copy copies them.
        e.execute("midi.notes_copy", &json!({"clip": c, "indices": [0], "ticks": 3840, "with_ctrls": true})).unwrap();
        assert_eq!(lane(&mut e, c, "cc1"), vec![(0, 10), (480, 20), (1920, 30), (2400, 40), (3840, 10), (4320, 20)]);
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
