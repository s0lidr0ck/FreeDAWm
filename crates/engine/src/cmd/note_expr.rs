//! Per-note expression (MPE) on a MIDI clip's notes (`soundcraft_midi::note_expr`): a note's
//! tuning, pressure and brightness, read and written as points from its start, and the notes
//! given channels of their own so each can carry its own. The MIDI Editor's lanes follow the
//! selected note (its channel, over its length) with the same data.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::note_expr::{self, ExprKind};
use soundcraft_model::{ClipContent, ClipId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "midi.note_expr",
            "Note Expression",
            [],
            None,
            "{clip, index} -> {channel, shared: bool, tuning: [[tick, value]], pressure: [...], brightness: [...]}: the note's expression (its channel's bend, pressure and CC 74 while it sounds), ticks from its start",
            always,
            get
        ),
        cmd!(
            "midi.note_expr_set",
            "Set Note Expression",
            [],
            None,
            "{clip, index, kind: tuning|pressure|brightness|all (all: clears), points: [[tick from the note's start, value]] (tuning -8192..8191, others 0..127)} -> {points}: replaces that expression over the note; refused when the note shares its channel",
            always,
            set
        ),
        cmd!(
            "midi.own_channels",
            "Give Notes Their Own Channels",
            [],
            None,
            "{clip, indices?: [n] (default: every note)} -> {changed}: each note on a member channel of its own (2-16), so it can carry its own expression (MPE)",
            always,
            own
        ),
    ]
}

fn seq_of(e: &Engine, id: ClipId, cmd: &str) -> Result<soundcraft_midi::Sequence> {
    match e.session().find_clip(id).map(|(_, c)| &c.content) {
        Some(ClipContent::Midi { sequence }) => Ok(sequence.clone()),
        Some(_) => Err(bad(cmd, "not a MIDI clip")),
        None => Err(bad(cmd, "no such clip")),
    }
}

fn put(e: &mut Engine, id: ClipId, seq: soundcraft_midi::Sequence) {
    if let Some(ClipContent::Midi { sequence }) = e.session_mut().find_clip_mut(id).map(|c| &mut c.content) {
        *sequence = seq;
    }
}

fn index(p: &Value, cmd: &str, seq: &soundcraft_midi::Sequence) -> Result<usize> {
    let i =
        p.get("index").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(cmd, "`index` (a note, from 0) required"))?;
    if i >= seq.notes.len() {
        return Err(bad(cmd, format!("note {i} is out of range (the clip has {})", seq.notes.len())));
    }
    Ok(i)
}

fn get(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.note_expr";
    let seq = seq_of(e, clip_id(p, id)?, id)?;
    let i = index(p, id, &seq)?;
    let pts = |k| note_expr::expression(&seq, i, k).into_iter().map(|(t, v)| json!([t, v])).collect::<Vec<_>>();
    Ok(json!({
        "channel": seq.notes.get(i).map_or(0, |n| n.channel),
        "shared": note_expr::shares_channel(&seq, i),
        "tuning": pts(ExprKind::Tuning),
        "pressure": pts(ExprKind::Pressure),
        "brightness": pts(ExprKind::Brightness),
    }))
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.note_expr_set";
    let clip = clip_id(p, id)?;
    let mut seq = seq_of(e, clip, id)?;
    let i = index(p, id, &seq)?;
    // `all` with no points clears the note's whole expression in one step.
    let kinds: Vec<ExprKind> = match p.get("kind").and_then(Value::as_str) {
        Some("all") => ExprKind::ALL.to_vec(),
        _ => vec![
            p.get("kind")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or_else(|| bad(id, "`kind` is tuning, pressure, brightness or all"))?,
        ],
    };
    let kind = kinds.first().copied().unwrap_or(ExprKind::Tuning);
    let list = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(id, "`points` ([[tick, value]]) required"))?;
    if list.len() > 4096 {
        return Err(bad(id, "at most 4096 points"));
    }
    let points: Vec<(i64, i32)> = list
        .iter()
        .map(|pt| {
            let t = pt.get(0).and_then(Value::as_i64);
            let v = pt.get(1).and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok());
            t.zip(v).filter(|(t, _)| *t >= 0).ok_or_else(|| bad(id, "each point is [tick from 0, value]"))
        })
        .collect::<Result<_>>()?;
    if kinds.len() > 1 && !points.is_empty() {
        return Err(bad(id, "`all` only clears: give a kind to set points"));
    }
    for k in &kinds {
        note_expr::set(&mut seq, i, *k, &points).map_err(|m| bad(id, m))?;
    }
    let back: Vec<Value> = note_expr::expression(&seq, i, kind).into_iter().map(|(t, v)| json!([t, v])).collect();
    put(e, clip, seq);
    Ok(json!({"points": back}))
}

fn own(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.own_channels";
    let clip = clip_id(p, id)?;
    let mut seq = seq_of(e, clip, id)?;
    let indices: Vec<usize> = match p.get("indices") {
        None => Vec::new(),
        Some(v) => v
            .as_array()
            .ok_or_else(|| bad(id, "`indices` is a list of notes"))?
            .iter()
            .filter_map(|x| x.as_u64().and_then(|n| usize::try_from(n).ok()))
            .collect(),
    };
    let changed = note_expr::own_channels(&mut seq, &indices);
    if changed > 0 {
        put(e, clip, seq);
        if let Some(c) = e.session_mut().find_clip_mut(clip) {
            // The clip plays each note on its channel.
            c.midi_channels = true;
        }
    }
    Ok(json!({"changed": changed}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn a_notes_expression_round_trips_and_needs_a_channel_of_its_own() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let c = e.execute("midi.new_clip", &json!({"track": t, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        for pitch in [60, 64] {
            e.execute("midi.note_add", &json!({"clip": c, "pitch": pitch, "start_ticks": 0, "length_ticks": 960})).unwrap();
        }
        let set = json!({"clip": c, "index": 0, "kind": "tuning", "points": [[0, 0], [480, 2048]]});
        assert!(e.execute("midi.note_expr_set", &set).is_err(), "two notes on one channel");
        assert_eq!(e.execute("midi.own_channels", &json!({"clip": c})).unwrap()["changed"], 2);
        let before = e.history_sessions().len();
        e.execute("midi.note_expr_set", &set).unwrap();
        assert_eq!(e.history_sessions().len(), before + 1);
        let got = e.execute("midi.note_expr", &json!({"clip": c, "index": 0})).unwrap();
        assert_eq!(got["tuning"], json!([[0, 0], [480, 2048]]));
        assert_eq!(got["shared"], false);
        assert!(e.execute("midi.note_expr_set", &json!({"clip": c, "index": 0, "kind": "wobble", "points": []})).is_err());
        assert!(e.execute("midi.note_expr_set", &json!({"clip": c, "index": 9, "kind": "tuning", "points": []})).is_err());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.execute("midi.note_expr", &json!({"clip": c, "index": 0})).unwrap()["tuning"], json!([]));
    }
}
