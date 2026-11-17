//! Edit › Selection, the rest: extend the edit to the session's start or end, select every clip
//! on the selected tracks or all those that follow the insertion, select clips that match the
//! selected one (name, colour) or every muted clip, and nudge only the selection's start or end.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ClipId, Session, TrackId};
use soundcraft_time::Range;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "edit.extend_to_start", "Extend Edit to Session Start", [], None, "{} — the selection runs from the session start to its end", always, |e, _| {
            let ed = &mut e.session_mut().edit;
            ed.selection = Range::new(0, ed.selection.end.max(0));
            Ok(json!({"selection": ed.selection}))
        }),
        cmd!(noundo "edit.extend_to_end", "Extend Edit to Session End", [], None, "{} — the selection runs from its start to the end of the last clip", always, |e, _| {
            let s = e.session_mut();
            let end = s.content_end().max(s.edit.selection.end);
            s.edit.selection = Range::new(s.edit.selection.start.max(0), end);
            Ok(json!({"selection": s.edit.selection}))
        }),
        cmd!(
            noundo "edit.select_track_clips",
            "Select All Clips on Track",
            [],
            None,
            "{tracks?} — every clip on the selected tracks (or those named), as objects",
            has_tracks,
            |e, p| {
                let tracks = target_tracks(e, p)?;
                select_clips(e, |t, _| tracks.contains(&t))
            }
        ),
        cmd!(
            noundo "edit.select_following",
            "Select All Following",
            [],
            None,
            "{tracks?, at?} — the clips on the selected tracks (or every track when none is) that start at or after the insertion point",
            has_tracks,
            |e, p| {
                let tracks = if p.get("tracks").is_some() || p.get("track").is_some() || !e.session().edit.selected_tracks.is_empty() {
                    target_tracks(e, p)?
                } else {
                    e.session().tracks.iter().filter(|t| !t.hidden).map(|t| t.id).collect()
                };
                let at = position_param(e, "edit.select_following", p, "at")?.unwrap_or(e.session().edit.selection.start);
                select_clips(e, |t, c| tracks.contains(&t) && c.start >= at)
            }
        ),
        cmd!(noundo "edit.select_muted_clips", "Select Muted Clips", [], None, "{} — every muted clip on a shown track", has_tracks, |e, _| select_clips(
            e,
            |_, c| c.muted
        )),
        cmd!(
            noundo "edit.select_same_name",
            "Select Clips with Same Name",
            [],
            None,
            "{clip?} — every clip named as the selected one (case ignored)",
            has_selection,
            |e, p| {
                let name = first_clip(e, p, "edit.select_same_name")?.name.to_lowercase();
                select_clips(e, |_, c| c.name.to_lowercase() == name)
            }
        ),
        cmd!(
            noundo "edit.select_same_color",
            "Select Clips with Same Color",
            [],
            None,
            "{clip?} — every clip coloured as the selected one (clips with no colour of their own match each other)",
            has_selection,
            |e, p| {
                let color = first_clip(e, p, "edit.select_same_color")?.color;
                select_clips(e, |_, c| c.color == color)
            }
        ),
        cmd!(noundo "edit.nudge_start_earlier", "Nudge Edit Start Earlier", [], None, "{count?} — the selection's start moves earlier by the nudge value", always, |e, p| {
            nudge_edge(e, p, true, -1)
        }),
        cmd!(noundo "edit.nudge_start_later", "Nudge Edit Start Later", [], None, "{count?} — the selection's start moves later by the nudge value", always, |e, p| {
            nudge_edge(e, p, true, 1)
        }),
        cmd!(noundo "edit.nudge_end_earlier", "Nudge Edit End Earlier", [], None, "{count?} — the selection's end moves earlier by the nudge value", always, |e, p| {
            nudge_edge(e, p, false, -1)
        }),
        cmd!(noundo "edit.nudge_end_later", "Nudge Edit End Later", [], None, "{count?} — the selection's end moves later by the nudge value", always, |e, p| {
            nudge_edge(e, p, false, 1)
        }),
    ]
}

/// The tracks named by `tracks`/`track`, else the selected ones.
fn target_tracks(e: &Engine, p: &Value) -> Result<Vec<TrackId>> {
    if p.get("tracks").is_some() || p.get("track").is_some() {
        return tracks_param(e, "edit.select", p);
    }
    Ok(e.session().edit.selected_tracks.clone())
}

/// The clip named by `clip`, else the first selected one.
fn first_clip(e: &Engine, p: &Value, cmd: &str) -> Result<soundcraft_model::Clip> {
    let s = e.session();
    let id = p.get("clip").and_then(Value::as_u64).map(ClipId).or_else(|| s.edit.selected_clips.first().copied());
    id.and_then(|id| s.find_clip(id)).map(|(_, c)| c.clone()).ok_or_else(|| bad(cmd, "select a clip first"))
}

/// Selects, as objects, the clips on shown tracks that `keep` accepts: the selection spans them
/// and their tracks are selected. Nothing matching leaves the selection as it was.
fn select_clips(e: &mut Engine, keep: impl Fn(TrackId, &soundcraft_model::Clip) -> bool) -> Result<Value> {
    let s: &Session = e.session();
    let mut ids = Vec::new();
    let mut tracks = Vec::new();
    let mut span: Option<Range> = None;
    for t in s.tracks.iter().filter(|t| !t.hidden) {
        for c in t.clips().iter().filter(|c| keep(t.id, c)) {
            ids.push(c.id);
            if !tracks.contains(&t.id) {
                tracks.push(t.id);
            }
            span = Some(span.map_or(c.range(), |r| Range::new(r.start.min(c.start), r.end.max(c.end()))));
        }
    }
    let Some(span) = span else { return Ok(json!({"clips": 0})) };
    let ed = &mut e.session_mut().edit;
    ed.selected_clips = ids;
    ed.selected_tracks = tracks;
    ed.selection = span;
    Ok(json!({"clips": ed.selected_clips.len(), "selection": span}))
}

/// Moves only the selection's start (or end) by the nudge value, `dir` times `count`; the edges
/// never cross and the start stays at or after the session start.
fn nudge_edge(e: &mut Engine, p: &Value, start: bool, dir: i64) -> Result<Value> {
    let count = i64_or(p, "count", 1).clamp(1, 1000);
    let s = e.session_mut();
    let at = if start { s.edit.selection.start } else { s.edit.selection.end };
    let step = s.edit.nudge.step_samples(at, &s.grid_ctx()).saturating_mul(count).saturating_mul(dir);
    let r = s.edit.selection;
    s.edit.selection = if start {
        Range::new(r.start.saturating_add(step).clamp(0, r.end), r.end)
    } else {
        Range::new(r.start, r.end.saturating_add(step).max(r.start))
    };
    Ok(json!({"selection": s.edit.selection, "by": step}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{Clip, SourceId};

    fn setup() -> (Engine, Vec<TrackId>, Vec<ClipId>) {
        let mut e = Engine::default();
        let v = e.execute("track.new", &json!({"count": 2})).unwrap();
        let t: Vec<TrackId> = v["tracks"].as_array().unwrap().iter().map(|x| TrackId(x.as_u64().unwrap())).collect();
        let s = e.session_mut();
        let ids: Vec<ClipId> = (0..4).map(|_| s.new_clip_id()).collect();
        let c = |i: usize, start, name: &str| {
            let mut c = Clip::audio(ids[i], name, SourceId(1), 0, start, 1000);
            c.muted = i == 3;
            c.color = i.is_multiple_of(2).then_some([1, 2, 3]);
            c
        };
        let a = vec![c(0, 0, "Kick"), c(1, 2000, "Snare")];
        let b = vec![c(2, 500, "kick"), c(3, 5000, "Hat")];
        s.track_mut(t[0]).unwrap().playlist_mut().unwrap().clips = a;
        s.track_mut(t[1]).unwrap().playlist_mut().unwrap().clips = b;
        (e, t, ids)
    }

    fn sel(e: &Engine) -> (Vec<ClipId>, Range) {
        (e.session().edit.selected_clips.clone(), e.session().edit.selection)
    }

    #[test]
    fn selects_clips_by_track_following_name_colour_and_mute() {
        let (mut e, t, c) = setup();
        e.execute("edit.select", &json!({"tracks": [t[1].0], "exact": true})).unwrap();
        e.execute("edit.select_track_clips", &json!({})).unwrap();
        assert_eq!(sel(&e), (vec![c[2], c[3]], Range::new(500, 6000)));
        // Following the insertion, on every track when none is selected.
        e.execute("edit.select_none", &json!({})).unwrap();
        e.execute("edit.select", &json!({"start": 1000})).unwrap();
        e.execute("edit.select_following", &json!({})).unwrap();
        assert_eq!(sel(&e).0, vec![c[1], c[3]]);
        e.execute("edit.select", &json!({"clips": [c[0].0], "exact": true})).unwrap();
        e.execute("edit.select_same_name", &json!({})).unwrap();
        assert_eq!(sel(&e).0, vec![c[0], c[2]], "Kick and kick");
        e.execute("edit.select", &json!({"clips": [c[1].0], "exact": true})).unwrap();
        e.execute("edit.select_same_color", &json!({})).unwrap();
        assert_eq!(sel(&e).0, vec![c[1], c[3]], "the clips with no colour of their own");
        e.execute("edit.select_muted_clips", &json!({})).unwrap();
        assert_eq!(sel(&e), (vec![c[3]], Range::new(5000, 6000)));
        assert_eq!(e.session().edit.selected_tracks, vec![t[1]]);
    }

    #[test]
    fn extends_to_the_session_ends_and_nudges_one_edge() {
        let (mut e, _, _) = setup();
        e.execute("edit.select", &json!({"start": 1000, "end": 3000})).unwrap();
        e.execute("edit.extend_to_end", &json!({})).unwrap();
        assert_eq!(sel(&e).1, Range::new(1000, 6000));
        e.execute("edit.extend_to_start", &json!({})).unwrap();
        assert_eq!(sel(&e).1, Range::new(0, 6000));
        e.session_mut().edit.nudge = soundcraft_time::GridValue::Samples(100);
        e.execute("edit.nudge_start_later", &json!({"count": 3})).unwrap();
        e.execute("edit.nudge_end_earlier", &json!({})).unwrap();
        assert_eq!(sel(&e).1, Range::new(300, 5900));
        // The start stops at zero and the edges never cross.
        e.execute("edit.nudge_start_earlier", &json!({"count": 10})).unwrap();
        assert_eq!(sel(&e).1.start, 0);
        e.execute("edit.nudge_end_earlier", &json!({"count": 1000})).unwrap();
        assert_eq!(sel(&e).1, Range::new(0, 0));
    }
}
