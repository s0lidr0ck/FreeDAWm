//! Options › Overlap: what a clip moved, pasted, duplicated or recorded onto other clips does to
//! them: cuts them away (Overwrite), keeps audio whole underneath on a lower layer (Layer), or
//! cuts them away with a crossfade at each join (Crossfade). See [`crate::edit::Overlap`].

use super::*;
use crate::cmd;
use crate::edit::{OVERLAP_KEY, OVERLAP_XFADE_KEY, Overlap};
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        noundo "edit.overlap_mode",
        "Overlap",
        ["Options", "Overlap"],
        None,
        "{mode: overwrite|layer|crossfade, crossfade_ms?: 0-1000 (the Crossfade mode's crossfade, default 10)} — what a clip moved, pasted or recorded onto others does: overwrite cuts them away; layer keeps audio whole underneath (the newest clip on top is heard, the one below plays again where uncovered; MIDI and video still overwrite); crossfade overwrites with a crossfade at each join. Kept by undo, like the Edit mode → {mode, crossfade_ms}",
        always,
        set
    )]
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    let mode = p
        .get("mode")
        .and_then(Value::as_str)
        .and_then(Overlap::from_id)
        .ok_or_else(|| bad("edit.overlap_mode", "`mode` is overwrite, layer or crossfade"))?;
    let ms = match p.get("crossfade_ms") {
        None => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|ms| ms.is_finite() && (0.0..=1000.0).contains(ms))
                .ok_or_else(|| bad("edit.overlap_mode", "`crossfade_ms` is 0 to 1000"))?,
        ),
    };
    let ed = &mut e.session_mut().edit;
    ed.values.insert(OVERLAP_KEY.into(), mode.value());
    if let Some(ms) = ms {
        ed.values.insert(OVERLAP_XFADE_KEY.into(), ms);
    }
    Ok(json!({"mode": mode.id(), "crossfade_ms": ed.value(OVERLAP_XFADE_KEY, crate::edit::OVERLAP_XFADE_MS)}))
}

/// The session's overlap mode (for the menu's check marks).
pub fn current(s: &soundcraft_model::Session) -> Overlap {
    Overlap::of(s)
}
