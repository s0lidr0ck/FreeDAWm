//! `clip.follow_chords`: a MIDI clip that follows the chord lane (see [`crate::chord_follow`]).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::ClipContent;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "clip.follow_chords",
        "Follow Chord Lane",
        [],
        None,
        "{clips?, on?: bool (default: toggle)} — the clip's notes move with the chord lane when it changes, from the chords there now",
        has_selection,
        follow_chords
    )]
}

fn follow_chords(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.follow_chords";
    let s = e.session();
    let ids: Vec<_> =
        clip_ids_param(e, p).into_iter().filter(|id| s.find_clip(*id).is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. }))).collect();
    if ids.is_empty() {
        return Err(bad(C, "select a MIDI clip"));
    }
    let on = p
        .get("on")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| !ids.iter().all(|id| s.find_clip(*id).is_some_and(|(_, c)| c.follow_chords.is_some())));
    let snaps: Vec<_> = ids.iter().map(|id| s.find_clip(*id).map(|(_, c)| crate::chord_follow::snapshot(s, c))).collect();
    let s = e.session_mut();
    for (id, snap) in ids.iter().zip(snaps) {
        if let Some(c) = s.find_clip_mut(*id) {
            c.follow_chords = if on { snap } else { None };
        }
    }
    Ok(json!({"clips": ids.len(), "on": on}))
}
