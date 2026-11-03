//! `loop.*`: the sticky loop (docs/superpowers/specs/2026-10-28-sticky-loop.md). The edit
//! selection is the loop: every range selection sets it (`sync_loop`, run after each command), and
//! it stays after deselecting until the next range selection, an end drag (`loop.set`) or Clear.

use super::*;
use crate::cmd;
use serde_json::{Value, json};
use soundcraft_model::Session;
use soundcraft_time::Range;

/// The refusal for `loop.set` with no range given and only an insertion point selected.
pub const NO_SELECTION: &str = "make an edit selection first";
/// The refusal for `loop.select` with no loop.
pub const NO_LOOP: &str = "no loop";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "loop.set", "Set Loop", [], None, "{start?, end?}: the loop becomes start–end (one given: that end of the current loop moves, the selection staying as it is); no params = the current edit selection (refused when it is a point) → {start, end}", always, set),
        cmd!(noundo "loop.clear", "Clear Loop", ["Edit"], None, "{}: forgets the loop (the selection stays)", has_loop, |e, _| {
            e.session_mut().edit.loop_range = None;
            Ok(json!({}))
        }),
        cmd!(noundo "loop.select", "Select Loop Range", ["Edit"], Some("Ctrl+Alt+L"), "{}: selects the loop's exact range on the selected tracks (all tracks when none is selected) → {start, end}", has_loop, select),
        cmd!(query "loop.get", "Loop", [], None, "{} → {start, end} | null", always, |e, _| Ok(e.session().edit.loop_range.map_or(Value::Null, |r| json!({"start": r.start, "end": r.end})))),
    ]
}

fn has_loop(e: &Engine) -> std::result::Result<(), String> {
    if e.session().edit.loop_range.is_some() { Ok(()) } else { Err(NO_LOOP.into()) }
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "loop.set";
    let st = position_param(e, C, p, "start")?;
    let en = position_param(e, C, p, "end")?;
    let cur = e.session().edit.loop_range;
    let r = match (st, en) {
        (Some(a), Some(b)) => Range::new(a.max(0), b.max(0)),
        (Some(a), None) => Range::new(a.max(0), cur.map_or(a.max(0), |c| c.end)),
        (None, Some(b)) => Range::new(cur.map_or(b.max(0), |c| c.start), b.max(0)),
        (None, None) => {
            let sel = e.session().edit.selection;
            if sel.is_empty() {
                return Err(bad(C, NO_SELECTION));
            }
            sel
        }
    };
    if r.is_empty() {
        return Err(bad(C, "the loop needs a start before its end"));
    }
    if cur != Some(r) {
        e.session_mut().edit.loop_range = Some(r);
    }
    Ok(json!({"start": r.start, "end": r.end}))
}

fn select(e: &mut Engine, _: &Value) -> Result<Value> {
    let r = e.session().edit.loop_range.ok_or_else(|| bad("loop.select", NO_LOOP))?;
    let s = e.session();
    let tracks: Vec<u64> = if s.edit.selected_tracks.is_empty() {
        s.tracks.iter().filter(|t| !t.hidden).map(|t| t.id.0).collect()
    } else {
        s.edit.selected_tracks.iter().map(|t| t.0).collect()
    };
    e.execute("edit.select", &json!({"tracks": tracks, "start": r.start, "end": r.end}))?;
    Ok(json!({"start": r.start, "end": r.end}))
}

/// Rule 1: a command that changed the edit selection to a range made that range the loop. Run
/// after every command. The loop set this way does not mark the session modified (selecting
/// never does), so the saved snapshot takes it too.
pub(crate) fn sync_loop(e: &mut Engine, before: &Session) {
    let new = e.session().edit.selection;
    if new.is_empty() || new == before.edit.selection || e.session().edit.loop_range == Some(new) {
        return;
    }
    e.set_loop_quietly(new);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> (Engine, u64) {
        let mut e = Engine::default();
        let t = e.execute("track.new", &json!({})).unwrap()["tracks"][0].as_u64().unwrap();
        (e, t)
    }

    #[test]
    fn a_range_selection_sets_the_loop_and_a_point_or_deselect_leaves_it() {
        // No track: `track.new` would mark the session modified before the check below.
        let mut e = Engine::default();
        assert_eq!(e.execute("loop.get", &json!({})).unwrap(), Value::Null);
        let out = e.execute("edit.select", &json!({"start": 1_000, "end": 5_000})).unwrap();
        assert_eq!(out["loop"], json!({"start": 1_000, "end": 5_000}));
        assert_eq!(e.session().edit.loop_range, Some(Range::new(1_000, 5_000)));
        assert!(!e.is_dirty(), "selecting never marks the session modified");
        e.execute("edit.select", &json!({"start": 7_000})).unwrap();
        assert_eq!(e.session().edit.loop_range, Some(Range::new(1_000, 5_000)));
        assert!(!e.is_dirty());
        e.execute("edit.select_none", &json!({})).unwrap();
        assert_eq!(e.session().edit.loop_range, Some(Range::new(1_000, 5_000)));
        assert!(e.session().edit.selection.is_empty());
        assert_eq!(e.execute("loop.get", &json!({})).unwrap(), json!({"start": 1_000, "end": 5_000}));
        // Every way of selecting a range counts (Double Edit here).
        e.execute("edit.select", &json!({"start": 2_000, "end": 4_000})).unwrap();
        e.execute("edit.double_selection", &json!({})).unwrap();
        assert_eq!(e.session().edit.selection, Range::new(2_000, 6_000));
        assert_eq!(e.session().edit.loop_range, Some(Range::new(2_000, 6_000)));
    }

    #[test]
    fn loop_set_clear_and_select() {
        let (mut e, t) = engine();
        let err = e.execute("loop.set", &json!({})).unwrap_err().to_string();
        assert!(err.contains(NO_SELECTION), "{err}");
        assert!(e.execute("loop.select", &json!({})).is_err());
        e.execute("edit.select", &json!({"tracks": [t], "start": 1_000, "end": 5_000})).unwrap();
        e.execute("edit.select_none", &json!({})).unwrap();
        // One end moves; the selection stays.
        assert_eq!(e.execute("loop.set", &json!({"end": 8_000})).unwrap(), json!({"start": 1_000, "end": 8_000}));
        assert_eq!(e.execute("loop.set", &json!({"start": 2_000})).unwrap(), json!({"start": 2_000, "end": 8_000}));
        assert!(e.session().edit.selection.is_empty());
        assert!(e.is_dirty(), "an end drag marks the session modified");
        assert!(e.execute("loop.set", &json!({"start": 9_000, "end": 9_000})).is_err());
        // Select Loop Range: the exact range, all tracks when none is selected.
        assert_eq!(e.execute("loop.select", &json!({})).unwrap(), json!({"start": 2_000, "end": 8_000}));
        assert_eq!(e.session().edit.selection, Range::new(2_000, 8_000));
        assert_eq!(e.session().edit.selected_tracks.len(), 1);
        e.execute("loop.clear", &json!({})).unwrap();
        assert_eq!(e.session().edit.loop_range, None);
        assert_eq!(e.session().edit.selection, Range::new(2_000, 8_000), "Clear Loop keeps the selection");
        let err = e.execute("loop.select", &json!({})).unwrap_err().to_string();
        assert!(err.contains(NO_LOOP), "{err}");
        // Set from the selection.
        assert_eq!(e.execute("loop.set", &json!({})).unwrap(), json!({"start": 2_000, "end": 8_000}));
    }

    #[test]
    fn the_loop_is_not_an_undo_step() {
        let (mut e, t) = engine();
        let steps = e.undo_history().len();
        e.execute("edit.select", &json!({"tracks": [t], "start": 1_000, "end": 5_000})).unwrap();
        e.execute("loop.set", &json!({"end": 6_000})).unwrap();
        e.execute("loop.clear", &json!({})).unwrap();
        assert_eq!(e.undo_history().len(), steps);
    }
}
