//! `scout.*`: Scout captures (start, stop, store), their names, the session goal, Claude's
//! answers and which answer's guides are shown. The capture itself runs in the mix engine: start
//! and stop only ask the host (`TransportRequest::ScoutStart`/`ScoutStop`), which arms a rig on the
//! audio thread and, when it comes back, stores its result with `scout.store`.

use super::*;
use crate::cmd;
use crate::{EngineError, TransportRequest};
use serde_json::json;
use soundcraft_dsp::scout::MAX_CAPTURE_SECONDS;
use soundcraft_model::{MAX_SCOUT_ITEMS, ScoutCapture, ScoutState, ScoutStrip, ScoutTurn, Session};

/// The refusal when loop capture is asked for without a loop.
pub const NO_LOOP: &str = "Set a loop first";
/// The refusal for a loop Scout cannot capture whole.
pub const LONG_LOOP: &str = "Loop is longer than 30 seconds; shorten it to capture it exactly";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "scout.capture",
            "Scout Capture",
            [],
            None,
            "{action: \"start\"|\"stop\", strips?: [track id or name] (default: the selected tracks; \"Master\" names the master), loop?: bool}: start asks the host to capture the strips' PRE/POST (from the loop start to its end with `loop`, which needs a loop (the last range selected) of at most 30 s); refused while a capture runs or with no strips. Stop ends it (nothing running: a no-op). → {capture: id | null}",
            always,
            capture
        ),
        cmd!(
            "scout.store",
            "Scout Capture",
            [],
            None,
            "{capture: id, name?, strips: [ScoutStrip], played: [{start, end}], loop_range?: {start, end}, played_truncated?: bool, created?: RFC 3339}: appends a finished capture (the host calls it when the rig comes back). Only ids started in this session and not yet stored are taken. Default name: the marker or clip starting at the first played sample, else \"Capture N\" → {capture, name}",
            always,
            store
        ),
        cmd!("scout.rename", "Rename Scout Capture", [], None, "{capture: id, name}: renames a capture → {capture, name}", always, rename),
        cmd!("scout.goal", "Scout Goal", [], None, "{text}: the session goal sent with every question → {goal}", always, goal),
        cmd!("scout.delete", "Delete Scout Capture", [], None, "{capture: id}: removes a capture → {capture}", always, delete),
        cmd!(
            "scout.answer",
            "Scout Answer",
            [],
            None,
            "{turn: {question, captures: [id], answer, guides?: [guide], notes?: [text], error?, model?}}: appends a question and Claude's answer (given a fresh id) → {turn: id}",
            always,
            answer
        ),
        cmd!(
            "scout.guides",
            "Scout Guides",
            [],
            None,
            "{turn: id | null}: shows that answer's guides on Clean Q (null: hide them) → {turn}",
            always,
            guides
        ),
        cmd!(
            query "scout.list",
            "Scout List",
            [],
            None,
            "{}: → the session's Scout state {goal, captures: [... , same_as: id | null], turns, guides_shown, next_id, capturing: id | null}; `same_as` is the first earlier capture of the same loop range",
            always,
            list
        ),
        cmd!(
            query "scout.starting_prompt",
            "Starting Prompt",
            [],
            None,
            "{}: → {text, chars}: the FireStation starting prompt (Markdown): what FreeDAWm and Scout are, the reply format Scout reads guides from, and every listed module with each parameter's id, label, range, unit and default",
            always,
            starting_prompt
        ),
    ]
}

fn starting_prompt(_e: &mut Engine, _p: &Value) -> Result<Value> {
    let text = crate::scout_prompt::starting_prompt();
    Ok(json!({"chars": text.chars().count(), "text": text}))
}

fn refuse(cmd: &str, why: &str) -> EngineError {
    EngineError::Disabled(cmd.to_string(), why.to_string())
}

fn id_param(cmd: &str, p: &Value, key: &str) -> Result<u32> {
    p.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(cmd, format!("`{key}` must be a capture id")))
}

/// The refusal for storing a capture this session never started (or already stored).
pub const NOT_PENDING: &str = "That capture belongs to another session or was never started";

/// Takes a fresh id from the shared capture/turn counter, above every id in use, every capture
/// still pending (an undo can lower `next_id` below one) and every id the engine ever handed out
/// (`Engine::scout_floor`, which survives undo and a replaced session).
fn reserve(e: &mut Engine) -> u32 {
    let floor = e.scout_floor;
    let pending = e.scout_pending.iter().copied().max();
    let st = &mut e.session_mut().scout;
    let used = st.captures.iter().map(|c| c.id).chain(st.turns.iter().map(|t| t.id)).chain(pending).max().map_or(0, |m| m.saturating_add(1));
    let id = st.next_id.max(used).max(floor);
    st.next_id = id.saturating_add(1);
    e.scout_floor = id.saturating_add(1);
    id
}

/// The strips to capture: `strips` (ids or names; "Master" also names the master), else the
/// selected tracks in session order.
fn strips_param(e: &Engine, cmd: &str, p: &Value) -> Result<Vec<TrackId>> {
    let s = e.session();
    let Some(v) = p.get("strips") else {
        return Ok(s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).map(|t| t.id).collect());
    };
    let arr = v.as_array().ok_or_else(|| bad(cmd, "`strips` must be an array of track ids or names"))?;
    let mut out = Vec::new();
    for v in arr.iter().take(MAX_SCOUT_ITEMS) {
        let id = match (v.as_u64(), v.as_str()) {
            (Some(n), _) => s.track(TrackId(n)).map(|t| t.id),
            (_, Some(name)) => s.track_by_name(name).or_else(|| s.master().filter(|_| name.eq_ignore_ascii_case("master"))).map(|t| t.id),
            _ => None,
        };
        let id = id.ok_or_else(|| bad(cmd, format!("unknown track {v}")))?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

fn capture(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.capture";
    match str_param(p, "action") {
        Some("stop") => {
            // Nothing running: a no-op. The stopped capture stays pending until it is stored.
            let id = e.scout_capture.take();
            if id.is_some() {
                e.transport_requests.push(TransportRequest::ScoutStop);
            }
            Ok(json!({"capture": id}))
        }
        Some("start") => {
            if e.scout_capture.is_some() {
                return Err(refuse(C, "Scout is already capturing; stop it first"));
            }
            let strips = strips_param(e, C, p)?;
            if strips.is_empty() {
                return Err(refuse(C, "Select tracks, buses or the master and press Scout"));
            }
            let (start, end) = if bool_or(p, "loop", false) {
                let s = e.session();
                let Some(sel) = s.edit.loop_range.filter(|r| !r.is_empty()) else {
                    return Err(refuse(C, NO_LOOP));
                };
                let max = (f64::from(MAX_CAPTURE_SECONDS) * s.sample_rate.as_f64()).round() as i64;
                if sel.len() > max {
                    return Err(refuse(C, LONG_LOOP));
                }
                (Some(sel.start), Some(sel.end))
            } else {
                (None, None)
            };
            let id = reserve(e);
            e.scout_capture = Some(id);
            e.scout_pending.push(id);
            e.transport_requests.push(TransportRequest::ScoutStart { capture: id, strips, start, end });
            Ok(json!({"capture": id}))
        }
        _ => Err(bad(C, "`action` must be \"start\" or \"stop\"")),
    }
}

fn parse<T: serde::de::DeserializeOwned>(cmd: &str, p: &Value, key: &str) -> Result<T> {
    serde_json::from_value(p.get(key).cloned().unwrap_or(Value::Null)).map_err(|err| bad(cmd, format!("`{key}`: {err}")))
}

/// The marker, or a captured strip's clip, starting at `at`.
fn name_at(s: &Session, at: Samples, strips: &[ScoutStrip]) -> Option<String> {
    let marker = s.markers.iter().find(|m| m.start == at && !m.name.trim().is_empty()).map(|m| m.name.clone());
    marker.or_else(|| {
        strips
            .iter()
            .filter_map(|st| s.track(st.track)?.playlist())
            .flat_map(|pl| pl.clips.iter())
            .find(|c| c.start == at && !c.name.trim().is_empty())
            .map(|c| c.name.clone())
    })
}

fn store(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.store";
    let id = id_param(C, p, "capture")?;
    let mut strips: Vec<ScoutStrip> = parse(C, p, "strips")?;
    strips.truncate(MAX_SCOUT_ITEMS);
    let mut played: Vec<soundcraft_time::Range> = parse(C, p, "played")?;
    played.truncate(MAX_SCOUT_ITEMS);
    // A hostile or corrupt range (end before start) would give a negative duration.
    played.retain(|r| r.end >= r.start);
    let loop_range: Option<soundcraft_time::Range> = match p.get("loop_range") {
        None | Some(Value::Null) => None,
        Some(_) => parse(C, p, "loop_range")?,
    };
    if !e.scout_pending.contains(&id) || e.session().scout.captures.iter().any(|c| c.id == id) {
        return Err(refuse(C, NOT_PENDING));
    }
    if e.session().scout.captures.len() >= MAX_SCOUT_ITEMS {
        return Err(bad(C, "too many captures; delete some first"));
    }
    let s = e.session();
    let name = match str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => played.first().and_then(|r| name_at(s, r.start, &strips)).unwrap_or_else(|| {
            let mut n = s.scout.captures.len() + 1;
            while s.scout.captures.iter().any(|c| c.name == format!("Capture {n}")) {
                n += 1;
            }
            format!("Capture {n}")
        }),
    };
    let at = played.first().map_or(0, |r| r.start);
    let tempo = s.tempo.tempo_at_tick(s.tempo.samples_to_ticks(at, s.sample_rate));
    let capture = ScoutCapture {
        id,
        name: name.clone(),
        created: str_param(p, "created").map_or_else(now_rfc3339, str::to_string),
        sample_rate: s.sample_rate.as_f64(),
        tempo: if tempo.is_finite() { tempo } else { 0.0 },
        played,
        loop_range,
        played_truncated: bool_or(p, "played_truncated", false),
        strips,
        bands_version: soundcraft_dsp::scout::BANDS_VERSION,
    };
    if e.scout_capture == Some(id) {
        e.scout_capture = None;
    }
    e.scout_pending.retain(|p| *p != id);
    let st = &mut e.session_mut().scout;
    st.captures.push(capture);
    st.next_id = st.next_id.max(id.saturating_add(1));
    Ok(json!({"capture": id, "name": name}))
}

fn capture_mut<'a>(e: &'a mut Engine, cmd: &str, id: u32) -> Result<&'a mut ScoutCapture> {
    if !e.session().scout.captures.iter().any(|c| c.id == id) {
        return Err(EngineError::NotFound(format!("{cmd}: no Scout capture {id}")));
    }
    e.session_mut().scout.captures.iter_mut().find(|c| c.id == id).ok_or_else(|| EngineError::NotFound(format!("{cmd}: no Scout capture {id}")))
}

fn rename(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.rename";
    let id = id_param(C, p, "capture")?;
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "`name` must be a non-empty string"))?.to_string();
    capture_mut(e, C, id)?.name = name.clone();
    Ok(json!({"capture": id, "name": name}))
}

fn goal(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.goal";
    let text = str_param(p, "text").ok_or_else(|| bad(C, "`text` must be a string"))?.to_string();
    if e.session().scout.goal != text {
        e.session_mut().scout.goal = text.clone();
    }
    Ok(json!({"goal": text}))
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.delete";
    let id = id_param(C, p, "capture")?;
    capture_mut(e, C, id)?;
    e.session_mut().scout.captures.retain(|c| c.id != id);
    Ok(json!({"capture": id}))
}

fn answer(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.answer";
    let mut turn: ScoutTurn = parse(C, p, "turn")?;
    if e.session().scout.turns.len() >= MAX_SCOUT_ITEMS {
        return Err(bad(C, "too many turns; the conversation is full"));
    }
    turn.guides.truncate(MAX_SCOUT_ITEMS);
    turn.notes.truncate(MAX_SCOUT_ITEMS);
    turn.captures.truncate(MAX_SCOUT_ITEMS);
    turn.id = reserve(e);
    let id = turn.id;
    e.session_mut().scout.turns.push(turn);
    Ok(json!({"turn": id}))
}

fn guides(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.guides";
    let turn = match p.get("turn") {
        Some(Value::Null) => None,
        Some(_) => Some(id_param(C, p, "turn")?),
        None => return Err(bad(C, "`turn` required (an answer id, or null to hide the guides)")),
    };
    if let Some(t) = turn
        && !e.session().scout.turns.iter().any(|x| x.id == t)
    {
        return Err(EngineError::NotFound(format!("{C}: no Scout answer {t}")));
    }
    if e.session().scout.guides_shown != turn {
        e.session_mut().scout.guides_shown = turn;
    }
    Ok(json!({"turn": turn}))
}

/// For each capture, the first earlier capture of the same loop range.
pub fn same_as(st: &ScoutState) -> Vec<Option<u32>> {
    st.captures
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let r = c.loop_range?;
            st.captures.iter().take(i).find(|x| x.loop_range == Some(r)).map(|x| x.id)
        })
        .collect()
}

fn list(e: &mut Engine, _: &Value) -> Result<Value> {
    let st = &e.session().scout;
    let mut v = serde_json::to_value(st).map_err(|err| EngineError::Internal("scout.list".into(), err.to_string()))?;
    if let Some(caps) = v.get_mut("captures").and_then(Value::as_array_mut) {
        for (c, same) in caps.iter_mut().zip(same_as(st)) {
            if let Some(o) = c.as_object_mut() {
                o.insert("same_as".into(), json!(same));
            }
        }
    }
    if let Some(o) = v.as_object_mut() {
        o.insert("capturing".into(), json!(e.scout_capture));
    }
    Ok(v)
}

/// Now as RFC 3339 UTC (seconds). Empty on the web build, where the system clock is unavailable.
pub(crate) fn now_rfc3339() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        rfc3339(i64::try_from(secs).unwrap_or(0))
    }
    #[cfg(target_arch = "wasm32")]
    {
        String::new()
    }
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (days to civil date: Howard Hinnant's algorithm).
#[cfg(not(target_arch = "wasm32"))]
fn rfc3339(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_792_454_400 + 3661), "2026-10-20T01:01:01Z");
    }
}
