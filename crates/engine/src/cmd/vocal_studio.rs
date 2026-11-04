//! Vocal Studio's commands (spec 2026-11-03-vocal-studio-design.md): the studio's tracks and their
//! status (`vocal.studio`), the clip formant (`vocal.formant`), clash markers (`vocal.clashes`)
//! Fit to lead (`vocal.fit_to_lead`) and Make harmony (`vocal.harmony`, `vocal.harmony_preview`). The reading is in [`crate::vocal_studio`].

use super::*;
use crate::cmd;
use crate::vocal::NoteRef;
use crate::vocal_studio::{
    HarmonyNote, MatchTo, TuneStep, clashes, compared_track, fit_plan, harmony_name, harmony_plan, lead_track, match_plan, studio_tracks,
    track_status, tune_all_plan, unison_stacks,
};
use serde_json::json;
use soundcraft_model::{StudioTuning, TrackKind, VocalTune};
use soundcraft_vocal::harmony::{HarmonyRule, Interval};

const STUDIO: &str = "vocal.studio";
const FORMANT: &str = "vocal.formant";
const CLASHES: &str = "vocal.clashes";
const FIT: &str = "vocal.fit_to_lead";
const HARMONY: &str = "vocal.harmony";
const PREVIEW: &str = "vocal.harmony_preview";
const TUNE_ALL: &str = "vocal.tune_all";
const TOLERANCE: &str = "vocal.tolerance";
const UNISON: &str = "vocal.unison";
const MATCH: &str = "vocal.match";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "vocal.formant",
            "Vocal Formant",
            [],
            None,
            "{clip? | clips? | track?, semitones?: -12..12 | by?: semitones} — the clip formant shift of the targeted audio clips (default: the selected clips, else the selected tracks' audio clips): the voice's formants move by that many semitones (positive brighter, negative darker) while the pitch stays; each note's own shift (vocal.edit `formant`) adds to it. `by` adds to the current value (clamped to ±12). A clip gets a tuning when it has none. Saved with the tuning, part of its render; undoable. Returns {clips: [{clip, formant}…]}",
            has_selection,
            formant
        ),
        cmd!(
            query "vocal.studio",
            "Vocal Studio",
            [],
            None,
            "{editing?: track, against?: track} — the Vocal Studio's Tune all `tolerance` (cents) and `fix_to` (centre | edge; vocal.tolerance), and its tracks (audio tracks marked vocal or holding a tuning, top to bottom), each {track, name, color, notes, analysing, avg_offset (cents, mean distance of its notes from their targets, null without notes), status: in tune (≤10 c) | drifting (11–17 c) | off (>17 c) | analysing | can't analyse | no notes, clashes: how many of its notes clash (vocal.clashes against the compared track)}, `lead`: the track the others are compared with when `editing` is edited (a track named Lead…, else the first other one), and `against`: the compared track (`against` when listed, else the session's lead)",
            always,
            studio
        ),
        cmd!(
            query "vocal.clashes",
            "Vocal Clashes",
            [],
            None,
            "{track, against?: track} — the clash markers of `track`'s notes: a note clashes with a note of `against` (default: the session's lead, a track named Lead…, else the first listed vocal track) sounding with it when they are a minor 2nd, major 7th or tritone apart (octaves folded); else with the band chord at its middle when it is not a chord tone and makes one of those intervals with a chord tone; else when it is out of the key (with a key set). Each {clip, note, kind: minor_second | major_seventh | tritone | out_of_key, with: the track or null (chord, key), text}. Returns {track, against, clashes}",
            always,
            clashes_query
        ),
        cmd!(
            "vocal.tolerance",
            "Tune All Tolerance",
            [],
            None,
            "{cents?: 5..25, fix_to?: centre | edge} — the Vocal Studio's Tune all settings, saved with the session: notes already within ±`cents` of their target (default 10) are left alone, and unison notes further apart than it are marked ≠; `fix_to` centre (default) pulls corrected notes to 0 c, edge only to ±`cents`. Undoable. Returns {tolerance, fix_to}",
            always,
            tolerance
        ),
        cmd!(
            "vocal.tune_all",
            "Tune All",
            [],
            None,
            "{tracks?: [track], tolerance_cents?: 5..25, fix_to?: centre | edge, drift?: 0..100, vibrato?: -100..100} — tunes every note of the tracks (default: every Vocal Studio track) that is further than the tolerance (default: the session's, vocal.tolerance) from its target: to the target (fix_to centre, the session's default) or to ±tolerance on its own side (edge), by its Centre amount (its own movement kept; the manual offset's part under a semitone when Centre alone can't reach); `drift`/`vibrato` (the Tuning panel's amounts) are set on the notes it corrects. Notes already inside the tolerance are left untouched. Refused while a track is being analysed. One undo step across all tracks. Returns {tracks, tolerance, fix_to, tuned: [{track, clip, note, before, after (cents from the target)}], left_alone: count}",
            always,
            tune_all
        ),
        cmd!(
            query "vocal.unison",
            "Vocal Unison",
            [],
            None,
            "{tracks?: [track], tolerance_cents?} — the unison stacks among the tracks' notes (default: every Vocal Studio track): notes of two or more tracks on the same row (semitone) sounding together. Each {note: \"E4\", midi, start, end (session samples, the earliest start and latest end), apart (cents between the highest and lowest held pitch), mismatch (apart > tolerance: the ≠ marker), notes: [{track, clip, note, pitch (cents), offset (from its target)}]}. Returns {tolerance, stacks, mismatches: count}",
            always,
            unison
        ),
        cmd!(
            "vocal.match",
            "Match Unison",
            [],
            None,
            "{notes: [{clip, note}], to: {clip, note} | \"centre\"} — moves each note's held pitch onto `to`'s (its manual offset changed by the difference; `to` itself is left alone), or with \"centre\" each onto its own target (0 c). One undo step across tracks. Returns {notes: [{track, clip, note, before, after (cents from the target)}]}",
            always,
            match_cmd
        ),
        cmd!(
            "vocal.fit_to_lead",
            "Fit to Lead",
            [],
            None,
            "{track?, against?: track, notes?: [{clip, note}], align_timing?: false} — moves each clashing note of `track` (default: the first selected track; `notes`: only those, else all) by whole semitones to the nearest tone within a fourth that clashes with no note of `against` sounding with it (default: the session's lead) nor with the chord, and is in the key: chord tones first, then the smaller move. Notes that fit already, or have nowhere to go, stay. Sets the notes' offset (as dragging them does). `align_timing` (moving note starts and ends to the lead's) is not available yet and is refused. Undoable, one step. Returns {track, against, moved: [{clip, note, semitones}]}",
            has_selection,
            fit_to_lead
        ),
        cmd!(
            query "vocal.harmony_preview",
            "Harmony Preview",
            [],
            None,
            "{source?, interval?, direction?, stay_in_key?, follow_chords?, range?} (as vocal.harmony) — the harmony vocal.harmony would make, changing nothing: {source, name: the new track's name, notes: [{clip, note, start, end (session samples), from, to (MIDI), pitch (the harmony's pitch in cents)}]}",
            always,
            harmony_preview
        ),
        cmd!(
            "vocal.harmony",
            "Make Harmony",
            [],
            None,
            "{source?: track (default: the session's lead), interval?: 3rd | 5th | 6th | octave (default 3rd), direction?: up | down (default up), stay_in_key?: true, follow_chords?: true, range?: [low, high] (MIDI numbers or note names), into?: track (an existing audio track; default a new one), name?: string, formant?: semitones (-12..12, default 0)} — makes a harmony part from the source's own voice: each source audio clip is copied (same audio, same place) onto the destination, a new track named \"Harmony 3rd ↑\" (or `name`) right under the source with its input and output, marked vocal, or the existing `into` track (whatever the copies cover there is replaced), with its own tuning: the source's note edits with each note moved by the harmony interval (diatonic in the key when stay_in_key, the chord's tone when follow_chords and the lead sings one, folded by octaves into range) and the clip's formant shift `formant`, rendered offline like any tuning (formants kept). Edit it like any other part. One undo step. Refused while the source is being analysed. Returns {track, name, clips, notes}",
            always,
            make_harmony
        ),
    ]
}

fn formant(e: &mut Engine, p: &Value) -> Result<Value> {
    let to = super::vocal::formant_param(FORMANT, p, "semitones")?;
    let by = match p.get("by") {
        None => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|b| b.is_finite() && b.abs() <= 24.0)
                .ok_or_else(|| bad(FORMANT, "`by` must be a number of semitones from -24 to 24"))? as f32,
        ),
    };
    if to.is_some() == by.is_some() {
        return Err(bad(FORMANT, "pass `semitones` or `by`"));
    }
    let ids = super::vocal::audio_targets(e, FORMANT, p)?;
    let max = soundcraft_vocal::MAX_FORMANT_ST;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let key = super::vocal::key_of(e.session(), id);
        let Some(t) = super::vocal::tune_mut(e.session_mut(), id, key) else { continue };
        let v = to.unwrap_or(t.formant_st + by.unwrap_or(0.0));
        t.formant_st = if v.is_finite() { (v * 10.0).round() / 10.0 } else { 0.0 }.clamp(-max, max);
        out.push(json!({"clip": id, "formant": t.formant_st}));
    }
    Ok(json!({"clips": out}))
}

fn studio(e: &mut Engine, p: &Value) -> Result<Value> {
    let editing = track_param(e, STUDIO, p, "editing")?;
    let against = track_param(e, STUDIO, p, "against")?;
    let s = e.session();
    let against = compared_track(s, against);
    let tracks: Vec<Value> = studio_tracks(s)
        .into_iter()
        .filter_map(|t| {
            let tr = s.track(t)?;
            let st = track_status(s, t);
            Some(json!({
                "track": t,
                "name": tr.name,
                "color": tr.color,
                "notes": st.notes,
                "analysing": st.analysing,
                "avg_offset": st.avg_offset,
                "status": st.level(),
                "clashes": clashes(s, t, against).len(),
            }))
        })
        .collect();
    let tu = s.studio_tuning;
    Ok(json!({"tracks": tracks, "lead": lead_track(s, editing), "against": against, "tolerance": tu.tolerance(), "fix_to": fix_name(tu.fix_to_edge)}))
}

fn clashes_query(e: &mut Engine, p: &Value) -> Result<Value> {
    let track = track_param(e, CLASHES, p, "track")?.ok_or_else(|| bad(CLASHES, "`track` required"))?;
    let against = track_param(e, CLASHES, p, "against")?;
    let s = e.session();
    let against = compared_track(s, against);
    let list: Vec<Value> = clashes(s, track, against)
        .into_iter()
        .map(|c| json!({"clip": c.r.clip, "note": c.r.note, "kind": c.kind, "with": c.with, "text": c.text}))
        .collect();
    Ok(json!({"track": track, "against": against.filter(|a| *a != track), "clashes": list}))
}

fn fit_to_lead(e: &mut Engine, p: &Value) -> Result<Value> {
    if p.get("align_timing").and_then(Value::as_bool).unwrap_or(false) {
        return Err(EngineError::Unsupported("Fit to lead's timing alignment is not available yet: leave align_timing off".into()));
    }
    let track = match track_param(e, FIT, p, "track")? {
        Some(t) => t,
        None => e.session().edit.selected_tracks.first().copied().ok_or_else(|| bad(FIT, "pass `track` or select a track"))?,
    };
    let against = track_param(e, FIT, p, "against")?;
    let only: Option<Vec<NoteRef>> = match p.get("notes") {
        None => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|_| bad(FIT, "`notes` must be a list of {clip, note}"))?),
    };
    let s = e.session();
    if s.track(track).is_none_or(|t| t.kind != soundcraft_model::TrackKind::Audio) {
        return Err(bad(FIT, "choose an audio track"));
    }
    let against = compared_track(s, against).filter(|a| *a != track);
    let plan = fit_plan(s, track, against, only.as_deref());
    if plan.is_empty() {
        return Ok(json!({"track": track, "against": against, "moved": []}));
    }
    let refs: Vec<NoteRef> = plan.iter().map(|(r, _)| *r).collect();
    let (refs, analyses) = super::vocal::note_refs(e, FIT, &json!({"notes": refs}))?;
    let by: std::collections::BTreeMap<NoteRef, i32> = plan.iter().copied().collect();
    super::vocal::change_edits(e, &refs, &analyses, |r, ed| {
        let d = by.get(&r).copied().unwrap_or(0);
        ed.offset_cents += d as f32 * 100.0;
    });
    let moved: Vec<Value> = plan.iter().map(|(r, d)| json!({"clip": r.clip, "note": r.note, "semitones": d})).collect();
    Ok(json!({"track": track, "against": against, "moved": moved}))
}

/// "centre" or "edge".
fn fix_name(edge: bool) -> &'static str {
    if edge { "edge" } else { "centre" }
}

/// `fix_to` from `p`: `Some(true)` edge, `Some(false)` centre.
fn fix_param(id: &str, p: &Value) -> Result<Option<bool>> {
    match p.get("fix_to") {
        None => Ok(None),
        Some(v) => match v.as_str().map(|x| x.trim().to_ascii_lowercase()).as_deref() {
            Some("centre" | "center") => Ok(Some(false)),
            Some("edge") => Ok(Some(true)),
            _ => Err(bad(id, "`fix_to` must be centre or edge")),
        },
    }
}

/// A tolerance in cents from `p[key]`, within [`StudioTuning::TOLERANCE_RANGE`].
fn tolerance_param(id: &str, p: &Value, key: &str) -> Result<Option<f32>> {
    let (lo, hi) = StudioTuning::TOLERANCE_RANGE;
    match p.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|c| c.is_finite() && (f64::from(lo)..=f64::from(hi)).contains(c))
            .map(|c| Some(c as f32))
            .ok_or_else(|| bad(id, format!("`{key}` must be a number of cents from {lo} to {hi}"))),
    }
}

/// The tracks a studio command acts on: `tracks` (ids or names), else every studio track.
fn studio_targets(e: &Engine, id: &str, p: &Value) -> Result<Vec<TrackId>> {
    if p.get("tracks").is_some() {
        if !p.get("tracks").is_some_and(Value::is_array) {
            return Err(bad(id, "`tracks` must be a list of tracks"));
        }
        let ts = super::tracks_param(e, id, p)?;
        let s = e.session();
        if let Some(t) = ts.iter().find(|t| s.track(**t).is_none_or(|tr| tr.kind != TrackKind::Audio)) {
            return Err(bad(id, format!("track {} is not an audio track", t.0)));
        }
        return Ok(ts);
    }
    Ok(studio_tracks(e.session()))
}

fn tolerance(e: &mut Engine, p: &Value) -> Result<Value> {
    let cents = tolerance_param(TOLERANCE, p, "cents")?;
    let edge = fix_param(TOLERANCE, p)?;
    let s = e.session_mut();
    if let Some(c) = cents {
        s.studio_tuning.tolerance_cents = (c * 10.0).round() / 10.0;
    }
    if let Some(x) = edge {
        s.studio_tuning.fix_to_edge = x;
    }
    let tu = s.studio_tuning;
    Ok(json!({"tolerance": tu.tolerance(), "fix_to": fix_name(tu.fix_to_edge)}))
}

/// Writes `steps`' edits (one command: one undo step however many tracks they span).
fn apply_steps(e: &mut Engine, id: &str, steps: &[TuneStep], extra: impl Fn(&mut soundcraft_vocal::NoteEdit)) -> Result<()> {
    if steps.is_empty() {
        return Ok(());
    }
    let refs: Vec<NoteRef> = steps.iter().map(|x| x.r).collect();
    let (refs, analyses) = super::vocal::note_refs(e, id, &json!({"notes": refs}))?;
    let by: std::collections::BTreeMap<NoteRef, soundcraft_vocal::NoteEdit> = steps.iter().map(|x| (x.r, x.edit.clone())).collect();
    super::vocal::change_edits(e, &refs, &analyses, |r, ed| {
        if let Some(new) = by.get(&r) {
            *ed = new.clone();
            extra(ed);
        }
    });
    Ok(())
}

fn steps_json(steps: &[TuneStep]) -> Vec<Value> {
    let r1 = |c: f32| (c * 10.0).round() / 10.0;
    steps.iter().map(|x| json!({"track": x.track, "clip": x.r.clip, "note": x.r.note, "before": r1(x.before), "after": r1(x.after)})).collect()
}

fn tune_all(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = studio_targets(e, TUNE_ALL, p)?;
    let s = e.session();
    let tol = tolerance_param(TUNE_ALL, p, "tolerance_cents")?.unwrap_or_else(|| s.studio_tuning.tolerance());
    let edge = fix_param(TUNE_ALL, p)?.unwrap_or(s.studio_tuning.fix_to_edge);
    let pct = |k: &str, lo: f64| -> Result<Option<f32>> {
        match p.get(k) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite() && (lo..=100.0).contains(x))
                .map(|x| Some((x / 100.0) as f32))
                .ok_or_else(|| bad(TUNE_ALL, format!("`{k}` must be a number from {lo} to 100 (%)"))),
        }
    };
    let (drift, vibrato) = (pct("drift", 0.0)?, pct("vibrato", -100.0)?);
    let waiting: Vec<String> =
        tracks.iter().filter(|t| track_status(s, **t).analysing).filter_map(|t| s.track(*t).map(|tr| tr.name.clone())).collect();
    if !waiting.is_empty() {
        return Err(bad(TUNE_ALL, format!("analysing {} — try again in a moment", waiting.join(", "))));
    }
    let (steps, alone) = tune_all_plan(s, &tracks, tol, edge);
    apply_steps(e, TUNE_ALL, &steps, |ed| {
        if let Some(d) = drift {
            ed.drift = d;
        }
        if let Some(v) = vibrato {
            ed.vibrato = v;
        }
    })?;
    Ok(json!({"tracks": tracks, "tolerance": tol, "fix_to": fix_name(edge), "tuned": steps_json(&steps), "left_alone": alone}))
}

fn unison(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = studio_targets(e, UNISON, p)?;
    let s = e.session();
    let tol = tolerance_param(UNISON, p, "tolerance_cents")?.unwrap_or_else(|| s.studio_tuning.tolerance());
    let stacks = unison_stacks(s, &tracks);
    let r1 = |c: f32| (c * 10.0).round() / 10.0;
    let list: Vec<Value> = stacks
        .iter()
        .map(|g| {
            let start = g.notes.iter().map(|n| n.note.start).min().unwrap_or(0);
            let end = g.notes.iter().map(|n| n.note.end).max().unwrap_or(0);
            let notes: Vec<Value> = g
                .notes
                .iter()
                .map(|n| json!({"track": n.track, "clip": n.note.r.clip, "note": n.note.r.note, "pitch": r1(n.note.pitch), "offset": r1(n.note.offset())}))
                .collect();
            json!({"note": crate::vocal_studio::midi_name(g.midi), "midi": g.midi, "start": start, "end": end, "apart": r1(g.apart), "mismatch": g.mismatch(tol), "notes": notes})
        })
        .collect();
    let mismatches = stacks.iter().filter(|g| g.mismatch(tol)).count();
    Ok(json!({"tolerance": tol, "stacks": list, "mismatches": mismatches}))
}

fn match_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    let notes: Vec<NoteRef> =
        p.get("notes").cloned().and_then(|v| serde_json::from_value(v).ok()).ok_or_else(|| bad(MATCH, "`notes` must be a list of {clip, note}"))?;
    if notes.is_empty() {
        return Err(bad(MATCH, "choose a note"));
    }
    let to = match p.get("to") {
        Some(Value::String(x)) if matches!(x.trim().to_ascii_lowercase().as_str(), "centre" | "center") => MatchTo::Centre,
        Some(v) => MatchTo::Note(serde_json::from_value(v.clone()).map_err(|_| bad(MATCH, "`to` must be a note {clip, note} or \"centre\""))?),
        None => return Err(bad(MATCH, "`to` required: a note {clip, note} or \"centre\"")),
    };
    let s = e.session();
    let mut tracks: Vec<TrackId> = Vec::new();
    for r in notes.iter().chain(match &to {
        MatchTo::Note(a) => std::slice::from_ref(a),
        MatchTo::Centre => &[],
    }) {
        let (t, _) = s.find_clip(r.clip).ok_or_else(|| bad(MATCH, format!("no clip with id {}", r.clip.0)))?;
        if !tracks.contains(&t) {
            tracks.push(t);
        }
    }
    let steps = match_plan(s, &tracks, &notes, to);
    apply_steps(e, MATCH, &steps, |_| {})?;
    Ok(json!({"notes": steps_json(&steps)}))
}

/// A MIDI note from a number or a name ("B3", "C#5", "Eb4").
fn midi_of(id: &str, v: &Value) -> Result<i32> {
    if let Some(n) = v.as_i64() {
        return i32::try_from(n).ok().filter(|m| (0..=127).contains(m)).ok_or_else(|| bad(id, "a note number is 0..127"));
    }
    let t = v.as_str().map(str::trim).unwrap_or_default();
    let mut ch = t.chars();
    let base = match ch.next().map(|c| c.to_ascii_uppercase()) {
        Some('C') => 0,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => return Err(bad(id, format!("`{t}` is not a note (like B3 or 59)"))),
    };
    let rest: String = ch.collect();
    let (acc, oct) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b') {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    let oct: i32 = oct.parse().map_err(|_| bad(id, format!("`{t}` is not a note (like B3 or 59)")))?;
    let m = (oct + 1) * 12 + base + acc;
    if (0..=127).contains(&m) { Ok(m) } else { Err(bad(id, format!("`{t}` is out of range"))) }
}

/// The harmony rule and source track from `p`.
fn harmony_rule(e: &Engine, id: &str, p: &Value) -> Result<(TrackId, HarmonyRule)> {
    let s = e.session();
    let source = match track_param(e, id, p, "source")? {
        Some(t) => t,
        None => compared_track(s, None).ok_or_else(|| bad(id, "no vocal track to harmonise: pass `source`"))?,
    };
    if s.track(source).is_none_or(|t| t.kind != TrackKind::Audio) {
        return Err(bad(id, "the source must be an audio track"));
    }
    let interval = match p.get("interval") {
        None => Interval::Third,
        Some(v) => v.as_str().and_then(Interval::parse).ok_or_else(|| bad(id, "`interval` must be 3rd, 5th, 6th or octave"))?,
    };
    let up = match p.get("direction").and_then(Value::as_str) {
        None if p.get("direction").is_none() => true,
        Some("up") => true,
        Some("down") => false,
        _ => return Err(bad(id, "`direction` must be up or down")),
    };
    let flag = |k: &str| match p.get(k) {
        None => Ok(true),
        Some(v) => v.as_bool().ok_or_else(|| bad(id, format!("`{k}` must be true or false"))),
    };
    let range = match p.get("range") {
        None => (0, 127),
        Some(v) => {
            let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(id, "`range` must be [low, high]"))?;
            let (lo, hi) = (midi_of(id, &a[0])?, midi_of(id, &a[1])?);
            (lo.min(hi), lo.max(hi))
        }
    };
    Ok((source, HarmonyRule { interval, up, stay_in_key: flag("stay_in_key")?, follow_chords: flag("follow_chords")?, range }))
}

fn harmony_preview(e: &mut Engine, p: &Value) -> Result<Value> {
    let (source, rule) = harmony_rule(e, PREVIEW, p)?;
    let plan = harmony_plan(e.session(), source, &rule);
    Ok(json!({"source": source, "name": harmony_name(&rule), "notes": plan_json(&plan)}))
}

fn plan_json(plan: &[HarmonyNote]) -> Vec<Value> {
    plan.iter()
        .map(|n| {
            json!({
                "clip": n.r.clip, "note": n.r.note, "start": n.start, "end": n.end, "from": n.from, "to": n.to,
                "pitch": n.pitch + (n.to - n.from) as f32 * 100.0,
            })
        })
        .collect()
}

fn make_harmony(e: &mut Engine, p: &Value) -> Result<Value> {
    let (source, rule) = harmony_rule(e, HARMONY, p)?;
    let formant = super::vocal::formant_param(HARMONY, p, "formant")?.unwrap_or(0.0);
    let into = track_param(e, HARMONY, p, "into")?;
    let name = match p.get("name") {
        None => harmony_name(&rule),
        Some(v) => v.as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(HARMONY, "`name` must be a name"))?.to_string(),
    };
    let s = e.session();
    if let Some(t) = into {
        if t == source {
            return Err(bad(HARMONY, "the harmony can't go onto its own source track"));
        }
        if s.track(t).is_none_or(|tr| tr.kind != TrackKind::Audio) {
            return Err(bad(HARMONY, "`into` must be an audio track"));
        }
    }
    let src = s.track(source).cloned().ok_or_else(|| bad(HARMONY, "no source track"))?;
    let clips: Vec<soundcraft_model::Clip> = src.clips().iter().filter(|c| c.is_audio() && !crate::vocal::stretched(c)).cloned().collect();
    if clips.is_empty() {
        return Err(bad(HARMONY, format!("{} has no audio clip to harmonise", src.name)));
    }
    let mut analyses = std::collections::BTreeMap::new();
    for c in &clips {
        let a = crate::vocal::analysis_for(s, c).ok_or_else(|| bad(HARMONY, "analysing — try again in a moment"))?;
        analyses.insert(c.id, a);
    }
    let plan = harmony_plan(s, source, &rule);
    let max_off = soundcraft_vocal::MAX_OFFSET_CENTS;
    // Each copy: the source's tuning with every planned note moved, and the formant shift.
    let mut copies = Vec::with_capacity(clips.len());
    for c in &clips {
        let mut nc = c.clone();
        let key = super::vocal::key_of(s, c.id);
        let mut tune = c.tune_live().cloned().unwrap_or_else(|| VocalTune {
            analysed: key,
            source: c.source(),
            note_mode: soundcraft_vocal::NoteMode::NEW,
            ..VocalTune::default()
        });
        tune.original = false;
        tune.formant_st = formant;
        for n in plan.iter().filter(|n| n.r.clip == c.id) {
            let ed = tune.edits.entry(n.r.note).or_default();
            ed.offset_cents = (ed.offset_cents + (n.to - n.from) as f32 * 100.0).clamp(-max_off, max_off);
        }
        if let Some(a) = analyses.get(&c.id) {
            crate::vocal::sync_note_spans(&mut tune, a);
        }
        nc.tune = Some(tune);
        copies.push(nc);
    }
    let s = e.session_mut();
    let dest = match into {
        Some(t) => t,
        None => {
            let t = s.add_track(TrackKind::Audio, src.format, Some(&name));
            // Right under the source, with its input and output.
            if let (Some(from), Some(at)) = (s.track_index(t), s.track_index(source)) {
                let tr = s.tracks.remove(from);
                let at = if from < at { at } else { at + 1 };
                s.tracks.insert(at.min(s.tracks.len()), tr);
            }
            if let Some(tr) = s.track_mut(t) {
                tr.mixer.input = src.mixer.input.clone();
                tr.mixer.output = src.mixer.output.clone();
                tr.vocal = Some(true);
            }
            t
        }
    };
    let dest_name = s.track(dest).map_or_else(|| name.clone(), |t| t.name.clone());
    let mut ids = Vec::with_capacity(copies.len());
    for mut c in copies {
        c.id = s.new_clip_id();
        c.name = dest_name.clone();
        ids.push(c.id);
        crate::edit::place_clip(s, dest, c);
    }
    Ok(json!({"track": dest, "name": dest_name, "clips": ids, "notes": plan.len()}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, FileFormat};
    use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackId, TrackKind};
    use std::f64::consts::TAU;

    const RATE: u32 = 48_000;

    /// A sung vowel (harmonics through three formants) at `f0` for `secs`.
    pub(crate) fn vowel(f0: f64, secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(RATE)) as usize;
        let formants = [(700.0, 100.0, 1.0), (1200.0, 110.0, 0.6), (2600.0, 160.0, 0.3)];
        let ks: Vec<(f64, f64)> = (1..=40)
            .map(|k| f64::from(k) * f0)
            .filter(|f| *f < f64::from(RATE) / 2.0)
            .map(|f| (f, formants.iter().map(|(c, b, a)| a / (1.0 + ((f - c) / b).powi(2))).sum::<f64>() + 0.01))
            .collect();
        let peak = 0.3 / ks.iter().map(|(_, a)| a).sum::<f64>().max(1e-9);
        (0..n).map(|i| (ks.iter().map(|(f, a)| a * (TAU * f * i as f64 / f64::from(RATE)).sin()).sum::<f64>() * peak) as f32).collect()
    }

    /// Notes (Hz, seconds) with 0.15 s of silence after each.
    pub(crate) fn line(notes: &[(f64, f64)]) -> Vec<f32> {
        let mut x = Vec::new();
        for (f, d) in notes {
            x.extend(vowel(*f, *d));
            x.extend(std::iter::repeat_n(0.0, (0.15 * f64::from(RATE)) as usize));
        }
        x
    }

    pub(crate) fn hz(midi: f64) -> f64 {
        440.0 * 2f64.powf((midi - 69.0) / 12.0)
    }

    /// Adds a track `name` with `take` at 0.
    pub(crate) fn add(s: &mut Session, name: &str, take: Vec<f32>) -> (TrackId, ClipId) {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        let len = take.len() as i64;
        let src = crate::io::add_source(s, name, AudioBuffer { sample_rate: RATE, channels: vec![take] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(id, name, src, 0, 0, len));
        (t, id)
    }

    #[test]
    fn studio_lists_vocal_tracks_with_status() {
        let mut s = Session::default();
        let (lead, lc) = add(&mut s, "Lead Vox", line(&[(hz(64.0), 0.5), (hz(66.0), 0.5)]));
        // A BV a quarter tone sharp: off.
        let (bv, bc) = add(&mut s, "BV 1", line(&[(hz(67.25), 0.5), (hz(69.25), 0.5)]));
        let (keys, _) = add(&mut s, "Keys", line(&[(hz(60.0), 0.5)]));
        let mut e = Engine::new(s);
        crate::vocal::run_until_idle(&mut e, &[lc, bc]).unwrap();
        let r = e.execute("vocal.studio", &json!({"editing": bv.0})).unwrap();
        let tracks = r["tracks"].as_array().unwrap();
        let ids: Vec<u64> = tracks.iter().map(|t| t["track"].as_u64().unwrap()).collect();
        assert_eq!(ids, [lead.0, bv.0], "Keys is not a vocal track");
        assert!(!ids.contains(&keys.0));
        assert_eq!(tracks[0]["status"], "in tune", "{r}");
        assert_eq!(tracks[1]["status"], "off", "{r}");
        assert_eq!(tracks[0]["notes"], 2);
        assert_eq!(r["lead"], lead.0);
    }

    #[test]
    fn formant_per_clip_and_note_is_saved_rendered_and_undoable() {
        let mut s = Session::default();
        let (_, c) = add(&mut s, "Lead Vox", line(&[(hz(64.0), 0.5), (hz(66.0), 0.5)]));
        let mut e = Engine::new(s);
        crate::vocal::run_until_idle(&mut e, &[c]).unwrap();
        assert!(e.execute("vocal.formant", &json!({"clip": c.0, "semitones": 13})).is_err(), "beyond ±12");
        assert!(e.execute("vocal.formant", &json!({"clip": c.0})).is_err());
        let r = e.execute("vocal.formant", &json!({"clip": c.0, "semitones": 3.04})).unwrap();
        assert_eq!(r["clips"][0]["formant"], 3.0);
        let clip = |e: &Engine| e.session().find_clip(c).unwrap().1.clone();
        assert_eq!(clip(&e).tune_live().unwrap().formant_st, 3.0);
        let inputs = crate::vocal::render_inputs(e.session(), &clip(&e)).unwrap();
        crate::vocal::run_until_idle(&mut e, &[c]).unwrap();
        let render = e.session().vocal.renders.get(&c).cloned().unwrap();
        assert_eq!(render.inputs, inputs, "rendered with the formant shift");
        assert!(!render.spans.is_empty());
        // By: added and clamped.
        e.execute("vocal.formant", &json!({"clip": c.0, "by": 20})).unwrap();
        assert_eq!(clip(&e).tune_live().unwrap().formant_st, 12.0);
        assert!(e.undo());
        assert_eq!(clip(&e).tune_live().unwrap().formant_st, 3.0);
        // Saved with the clip.
        let back: soundcraft_model::Clip = serde_json::from_str(&serde_json::to_string(&clip(&e)).unwrap()).unwrap();
        assert_eq!(back.tune_live().unwrap().formant_st, 3.0);
        // A note's own shift, through vocal.edit.
        let notes = crate::vocal::track_notes(e.session(), e.session().find_clip(c).unwrap().0, None);
        e.execute("vocal.edit", &json!({"notes": [notes[0]], "formant": -2.5})).unwrap();
        let t = clip(&e).tune_live().cloned().unwrap();
        assert_eq!(t.edits[&notes[0].note].formant_st, -2.5);
        assert_eq!(t.edited_count(), 1, "a formant-only edit counts");
        assert_ne!(crate::vocal::render_inputs(e.session(), &clip(&e)), Some(inputs), "the note's shift is part of the render key");
        assert!(e.execute("vocal.edit", &json!({"notes": [notes[0]], "formant": "x"})).is_err());
        assert!(e.execute("vocal.macro", &json!({"notes": [notes[0]], "formant": 1})).is_err(), "the macro leaves formants alone");
        // Back to 0 everywhere: an untouched clip renders as before formants existed.
        e.execute("vocal.formant", &json!({"clip": c.0, "semitones": 0})).unwrap();
        e.execute("vocal.reset", &json!({"notes": [notes[0]]})).unwrap();
        assert!(clip(&e).tune.is_none(), "nothing left: the tuning goes");
    }

    #[test]
    fn clashes_and_fit_to_lead() {
        let mut s = Session::default();
        s.band.key = 4; // E major
        let (lead, lc) = add(&mut s, "Lead Vox", line(&[(hz(64.0), 0.5), (hz(68.0), 0.5)]));
        // C#4 under E4 is fine; G4 under G#4 is a minor 2nd (and out of E major).
        let (bv, bc) = add(&mut s, "BV 1", line(&[(hz(61.0), 0.5), (hz(67.0), 0.5)]));
        let mut e = Engine::new(s);
        crate::vocal::run_until_idle(&mut e, &[lc, bc]).unwrap();
        let r = e.execute("vocal.clashes", &json!({"track": bv.0})).unwrap();
        let list = r["clashes"].as_array().unwrap();
        assert_eq!(list.len(), 1, "{r}");
        assert_eq!(list[0]["kind"], "minor_second");
        assert_eq!(list[0]["with"], lead.0);
        assert_eq!(list[0]["text"], "minor 2nd against Lead Vox (G#4)");
        let st = e.execute("vocal.studio", &json!({})).unwrap();
        assert_eq!(st["against"], lead.0);
        assert_eq!(st["tracks"][1]["clashes"], 1);
        assert_eq!(st["tracks"][0]["clashes"], 0, "the lead is checked against the chord and key only");
        // Fit to lead: no chords, so the nearest tone in key that doesn't clash: F# (down wins a tie).
        assert!(e.execute("vocal.fit_to_lead", &json!({"track": bv.0, "align_timing": true})).is_err());
        let undo = e.undo_history().len();
        let f = e.execute("vocal.fit_to_lead", &json!({"track": bv.0})).unwrap();
        assert_eq!(f["moved"].as_array().unwrap().len(), 1, "{f}");
        assert_eq!(f["moved"][0]["semitones"], -1);
        assert_eq!(e.undo_history().len(), undo + 1, "one undo step");
        let r = e.execute("vocal.clashes", &json!({"track": bv.0})).unwrap();
        assert!(r["clashes"].as_array().unwrap().is_empty(), "fitted: {r}");
        assert!(e.undo());
        // Over an E chord the chord tone wins: up to G# (unison with the lead).
        let end = e.session().tracks.iter().flat_map(|t| t.clips()).map(|c| c.end()).max().unwrap();
        e.execute("band.chord.add", &json!({"at": 0, "end": end, "chord": "E"})).unwrap();
        let f = e.execute("vocal.fit_to_lead", &json!({"track": bv.0})).unwrap();
        assert_eq!(f["moved"][0]["semitones"], 1, "{f}");
        let notes = crate::vocal_studio::studio_notes(e.session(), bv);
        assert_eq!(notes.iter().map(|n| n.midi()).collect::<Vec<_>>(), [61, 68]);
        // Only the notes given.
        assert!(e.undo());
        let first = notes[0].r;
        let f = e.execute("vocal.fit_to_lead", &json!({"track": bv.0, "notes": [first]})).unwrap();
        assert!(f["moved"].as_array().unwrap().is_empty(), "C#4 fits already: {f}");
    }

    /// A lead over E | A | B | E (one chord a second at 120 bpm, E major): E4, A4, F#4, G#4,
    /// 0.85 s each with 0.15 s between.
    fn progression() -> (Engine, TrackId, ClipId) {
        let mut s = Session::default();
        s.band.key = 4;
        let mut take = Vec::new();
        for m in [64.0, 69.0, 66.0, 68.0] {
            take.extend(vowel(hz(m), 0.85));
            take.extend(std::iter::repeat_n(0.0, (0.15 * f64::from(RATE)) as usize));
        }
        let (lead, lc) = add(&mut s, "Lead Vox", take);
        let mut e = Engine::new(s);
        let sec = i64::from(RATE);
        for (k, c) in ["E", "A", "B", "E"].iter().enumerate() {
            e.execute("band.chord.add", &json!({"at": k as i64 * sec, "end": (k as i64 + 1) * sec, "chord": c})).unwrap();
        }
        crate::vocal::run_until_idle(&mut e, &[lc]).unwrap();
        (e, lead, lc)
    }

    fn midis(e: &Engine, t: TrackId) -> Vec<i32> {
        crate::vocal_studio::studio_notes(e.session(), t).iter().map(|n| n.midi()).collect()
    }

    fn to(v: &serde_json::Value) -> Vec<i64> {
        v["notes"].as_array().unwrap().iter().map(|n| n["to"].as_i64().unwrap()).collect()
    }

    #[test]
    fn harmony_follows_key_and_chords() {
        let (mut e, lead, _) = progression();
        assert_eq!(midis(&e, lead), [64, 69, 66, 68]);
        // Diatonic 3rds up in E major: G#, C#, A, B.
        let p = e.execute("vocal.harmony_preview", &json!({"source": lead.0, "follow_chords": false})).unwrap();
        assert_eq!(to(&p), [68, 73, 69, 71]);
        assert_eq!(p["name"], "Harmony 3rd \u{2191}");
        // Following the chords: the lead's F# is B major's 5th, so its harmony is the root above.
        let p = e.execute("vocal.harmony_preview", &json!({"source": lead.0})).unwrap();
        assert_eq!(to(&p), [68, 73, 71, 71]);
        // Fifths down, chromatic sixths up, a range.
        let p = e
            .execute("vocal.harmony_preview", &json!({"source": "Lead Vox", "interval": "5th", "direction": "down", "follow_chords": false}))
            .unwrap();
        assert_eq!(to(&p), [57, 63, 59, 61], "A down a diatonic 5th in E major is D# (a diminished 5th)");
        let p = e.execute("vocal.harmony_preview", &json!({"interval": "6th", "stay_in_key": false, "follow_chords": false})).unwrap();
        assert_eq!(to(&p), [73, 78, 75, 77]);
        let p = e.execute("vocal.harmony_preview", &json!({"range": ["B3", 70]})).unwrap();
        assert_eq!(to(&p), [68, 61, 59, 59], "folded by octaves into B3..A#4");
        assert!(e.execute("vocal.harmony_preview", &json!({"interval": "9th"})).is_err());
        assert!(e.execute("vocal.harmony_preview", &json!({"range": ["H2", 70]})).is_err());
        assert!(e.execute("vocal.harmony_preview", &json!({"direction": "sideways"})).is_err());
    }

    #[test]
    fn make_harmony_lands_on_a_new_track_in_one_undo_step() {
        let (mut e, lead, lc) = progression();
        let tracks_before = e.session().tracks.len();
        let undo = e.undo_history().len();
        let r = e.execute("vocal.harmony", &json!({"source": lead.0, "formant": 1.5})).unwrap();
        assert_eq!(e.undo_history().len(), undo + 1, "one undo step");
        assert_eq!(e.undo_label(), Some("Make Harmony"));
        let h = TrackId(r["track"].as_u64().unwrap());
        assert_eq!(r["name"], "Harmony 3rd \u{2191}");
        let s = e.session();
        assert_eq!(s.tracks.len(), tracks_before + 1);
        assert_eq!(s.track_index(h), s.track_index(lead).map(|i| i + 1), "placed under the source");
        let (ht, lt) = (s.track(h).unwrap(), s.track(lead).unwrap());
        assert_eq!(ht.mixer.output, lt.mixer.output);
        assert_eq!(ht.mixer.input, lt.mixer.input);
        assert!(ht.is_vocal());
        // Its own clip: the lead's audio, its own tuning, the formant shift.
        let hc = ht.clips()[0].clone();
        let lclip = s.find_clip(lc).unwrap().1.clone();
        assert_ne!(hc.id, lc);
        assert_eq!((hc.start, hc.length, hc.source()), (lclip.start, lclip.length, lclip.source()));
        assert_eq!(hc.tune_live().unwrap().formant_st, 1.5);
        assert!(lclip.tune.is_none(), "the lead is untouched");
        assert_eq!(midis(&e, h), [68, 73, 71, 71], "the harmony's notes, as tuned");
        // In the studio's list, and rendered offline like any tuning.
        let st = e.execute("vocal.studio", &json!({})).unwrap();
        assert!(st["tracks"].as_array().unwrap().iter().any(|t| t["track"] == h.0));
        crate::vocal::run_until_idle(&mut e, &[hc.id]).unwrap();
        let inputs = crate::vocal::render_inputs(e.session(), &hc).unwrap();
        assert_eq!(e.session().vocal.renders.get(&hc.id).map(|r| r.inputs), Some(inputs));
        // One undo takes it all back.
        assert!(e.undo());
        assert_eq!(e.session().tracks.len(), tracks_before);
        assert!(e.session().track(h).is_none());
    }

    #[test]
    fn make_harmony_into_an_existing_track() {
        let (mut e, lead, _) = progression();
        let bv = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("BV 2"));
        let r = e.execute("vocal.harmony", &json!({"source": lead.0, "into": "BV 2", "interval": "octave", "direction": "down"})).unwrap();
        assert_eq!(r["track"], bv.0);
        assert_eq!(e.session().track(bv).unwrap().clips().len(), 1);
        assert_eq!(midis(&e, bv), [52, 57, 54, 56]);
        assert!(e.execute("vocal.harmony", &json!({"source": lead.0, "into": lead.0})).is_err(), "not onto its own source");
        assert!(e.execute("vocal.harmony", &json!({"source": lead.0, "formant": 20})).is_err());
    }

    // ---- Edit All, Tune all, unison (spec 2026-11-04-vocal-studio-edit-all-design.md) ----

    /// MIDI `m` moved by `c` cents, in Hz.
    fn hzc(m: f64, c: f64) -> f64 {
        hz(m + c / 100.0)
    }

    /// Each note's offset from its target (cents), in time order.
    fn offsets(e: &Engine, t: TrackId) -> Vec<f32> {
        crate::vocal_studio::studio_notes(e.session(), t).iter().map(|n| n.offset()).collect()
    }

    /// Lead Vox (E4 +3 c, F#4 +30 c), BV 1 (G#3 −25 c, B3 −4 c) and BV 2 (E3 +40 c), analysed.
    fn three() -> (Engine, [TrackId; 3]) {
        let mut s = Session::default();
        let (lead, a) = add(&mut s, "Lead Vox", line(&[(hzc(64.0, 3.0), 0.5), (hzc(66.0, 30.0), 0.5)]));
        let (bv1, b) = add(&mut s, "BV 1", line(&[(hzc(56.0, -25.0), 0.5), (hzc(59.0, -4.0), 0.5)]));
        let (bv2, c) = add(&mut s, "BV 2", line(&[(hzc(52.0, 40.0), 0.5)]));
        let mut e = Engine::new(s);
        crate::vocal::run_until_idle(&mut e, &[a, b, c]).unwrap();
        (e, [lead, bv1, bv2])
    }

    #[test]
    fn tune_all_leaves_notes_inside_the_tolerance_and_fixes_the_rest_in_one_step() {
        let (mut e, [lead, bv1, bv2]) = three();
        let (l0, b0, c0) = (offsets(&e, lead), offsets(&e, bv1), offsets(&e, bv2));
        assert!(l0[0].abs() <= 10.0 && l0[1] > 20.0, "lead {l0:?}");
        assert!(b0[0] < -20.0 && b0[1].abs() <= 10.0, "bv1 {b0:?}");
        let undo = e.undo_history().len();
        // BV 2 is hidden: the UI leaves it out of `tracks`.
        let r = e.execute("vocal.tune_all", &json!({"tracks": [lead.0, bv1.0]})).unwrap();
        assert_eq!(e.undo_history().len(), undo + 1, "one undo step across both tracks");
        assert_eq!(e.undo_label(), Some("Tune All"));
        assert_eq!(r["tuned"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["left_alone"], 2);
        assert_eq!(r["tolerance"], 10.0);
        assert_eq!(r["fix_to"], "centre");
        let (l1, b1) = (offsets(&e, lead), offsets(&e, bv1));
        assert_eq!(l1[0], l0[0], "inside the tolerance: untouched");
        assert_eq!(b1[1], b0[1]);
        assert!(l1[1].abs() < 1.0 && b1[0].abs() < 1.0, "fixed to the centre: {l1:?} {b1:?}");
        assert_eq!(offsets(&e, bv2), c0, "the hidden track is untouched");
        let tune = |e: &Engine, t: TrackId| e.session().track(t).unwrap().clips()[0].tune.clone();
        let edits = tune(&e, lead).unwrap().edits;
        assert_eq!(edits.len(), 1, "only the fixed note gets an edit");
        // Nothing left to do: no new step.
        let r = e.execute("vocal.tune_all", &json!({"tracks": [lead.0, bv1.0]})).unwrap();
        assert!(r["tuned"].as_array().unwrap().is_empty());
        assert_eq!(e.undo_history().len(), undo + 1);
        // One undo takes both tracks back.
        assert!(e.undo());
        assert!(tune(&e, lead).is_none() && tune(&e, bv1).is_none());
        // Default tracks: every studio track, BV 2 included.
        e.execute("vocal.tune_all", &json!({})).unwrap();
        assert!(offsets(&e, bv2)[0].abs() < 1.0);
        assert!(e.execute("vocal.tune_all", &json!({"tolerance_cents": 2})).is_err(), "5..25");
        assert!(e.execute("vocal.tune_all", &json!({"fix_to": "middle"})).is_err());
    }

    #[test]
    fn tune_all_to_the_edge_leaves_notes_at_the_tolerance() {
        let (mut e, [lead, bv1, _]) = three();
        e.execute("vocal.tolerance", &json!({"cents": 12, "fix_to": "edge"})).unwrap();
        let st = e.execute("vocal.studio", &json!({})).unwrap();
        assert_eq!((st["tolerance"].as_f64(), st["fix_to"].as_str()), (Some(12.0), Some("edge")));
        e.execute("vocal.tune_all", &json!({"tracks": [lead.0, bv1.0], "drift": 50})).unwrap();
        let (l, b) = (offsets(&e, lead), offsets(&e, bv1));
        assert!((l[1] - 12.0).abs() < 1.0, "sharp note to +12 c: {l:?}");
        assert!((b[0] + 12.0).abs() < 1.0, "flat note to −12 c: {b:?}");
        let c = e.session().track(lead).unwrap().clips()[0].tune.clone().unwrap();
        assert!(c.edits.values().all(|ed| (ed.drift - 0.5).abs() < 1e-6), "the panel's drift on the fixed notes");
        // At the edge: inside the tolerance next time.
        let undo = e.undo_history().len();
        let r = e.execute("vocal.tune_all", &json!({"tracks": [lead.0, bv1.0]})).unwrap();
        assert!(r["tuned"].as_array().unwrap().is_empty(), "{r}");
        assert_eq!(e.undo_history().len(), undo);
        // The settings: saved with the session, undoable, ranged.
        let back: Session = serde_json::from_str(&serde_json::to_string(e.session()).unwrap()).unwrap();
        assert_eq!(back.studio_tuning.tolerance(), 12.0);
        assert!(back.studio_tuning.fix_to_edge);
        assert!(e.execute("vocal.tolerance", &json!({"cents": 30})).is_err());
        assert!(e.execute("vocal.tolerance", &json!({"fix_to": 3})).is_err());
        let fresh = serde_json::to_string(&Session::default()).unwrap();
        assert!(!fresh.contains("studio_tuning"), "default settings are not written");
    }

    /// Lead Vox (E4 +3 c then G#4 0 c) and Lead Double (E4 −12 c then G#4 +4 c) together.
    fn doubled() -> (Engine, TrackId, TrackId) {
        let mut s = Session::default();
        let (lead, a) = add(&mut s, "Lead Vox", line(&[(hzc(64.0, 3.0), 0.6), (hzc(68.0, 0.0), 0.6)]));
        let (dbl, b) = add(&mut s, "Lead Double", line(&[(hzc(64.0, -12.0), 0.6), (hzc(68.0, 4.0), 0.6)]));
        let mut e = Engine::new(s);
        crate::vocal::run_until_idle(&mut e, &[a, b]).unwrap();
        (e, lead, dbl)
    }

    #[test]
    fn unison_stacks_mark_mismatches_above_the_tolerance_and_match_lines_them_up() {
        let (mut e, lead, dbl) = doubled();
        let u = e.execute("vocal.unison", &json!({})).unwrap();
        let stacks = u["stacks"].as_array().unwrap();
        assert_eq!(stacks.len(), 2, "{u}");
        assert_eq!(stacks[0]["note"], "E4");
        assert_eq!(stacks[0]["notes"].as_array().unwrap().len(), 2);
        let apart = stacks[0]["apart"].as_f64().unwrap();
        assert!((10.5..20.0).contains(&apart), "≈15 c apart: {apart}");
        assert_eq!(stacks[0]["mismatch"], true);
        assert_eq!(stacks[1]["mismatch"], false, "4 c apart agrees");
        assert_eq!(u["mismatches"], 1);
        // Only above the tolerance.
        let u = e.execute("vocal.unison", &json!({"tolerance_cents": 20})).unwrap();
        assert_eq!(u["mismatches"], 0);
        // One track alone stacks nothing.
        let u = e.execute("vocal.unison", &json!({"tracks": [lead.0]})).unwrap();
        assert!(u["stacks"].as_array().unwrap().is_empty());
        // Match the double to the lead: one step, the pitches equal.
        let ln = crate::vocal_studio::studio_notes(e.session(), lead);
        let dn = crate::vocal_studio::studio_notes(e.session(), dbl);
        let undo = e.undo_history().len();
        let m = e.execute("vocal.match", &json!({"notes": [dn[0].r], "to": ln[0].r})).unwrap();
        assert_eq!(m["notes"].as_array().unwrap().len(), 1);
        assert_eq!(e.undo_history().len(), undo + 1);
        assert_eq!(e.undo_label(), Some("Match Unison"));
        let p = |e: &Engine, t: TrackId| crate::vocal_studio::studio_notes(e.session(), t)[0].pitch;
        assert!((p(&e, lead) - p(&e, dbl)).abs() < 0.5, "{} vs {}", p(&e, lead), p(&e, dbl));
        assert_eq!(p(&e, lead), ln[0].pitch, "the anchor stays");
        let u = e.execute("vocal.unison", &json!({})).unwrap();
        assert_eq!(u["mismatches"], 0);
        // Both to centre: both on 0 c, one step.
        assert!(e.undo());
        e.execute("vocal.match", &json!({"notes": [ln[0].r, dn[0].r], "to": "centre"})).unwrap();
        assert_eq!(e.undo_history().len(), undo + 1);
        assert!(offsets(&e, lead)[0].abs() < 1.0 && offsets(&e, dbl)[0].abs() < 1.0);
        assert!(e.execute("vocal.match", &json!({"notes": [], "to": "centre"})).is_err());
        assert!(e.execute("vocal.match", &json!({"notes": [dn[0].r]})).is_err());
    }

    #[test]
    fn centre_note_on_notes_of_several_tracks_is_one_step() {
        let (mut e, lead, dbl) = doubled();
        let ln = crate::vocal_studio::studio_notes(e.session(), lead);
        let dn = crate::vocal_studio::studio_notes(e.session(), dbl);
        let undo = e.undo_history().len();
        e.execute("vocal.step", &json!({"notes": [ln[0].r, dn[0].r], "kind": "centre"})).unwrap();
        assert_eq!(e.undo_history().len(), undo + 1);
        assert_eq!(e.undo_label(), Some("Centre Note"));
        for (t, r) in [(lead, ln[0].r), (dbl, dn[0].r)] {
            let tune = e.session().track(t).unwrap().clips()[0].tune.clone().unwrap();
            assert!((tune.edits[&r.note].centre - 0.4).abs() < 1e-6);
        }
        assert!(e.undo());
        assert!(e.session().track(dbl).unwrap().clips()[0].tune.is_none());
    }
}
