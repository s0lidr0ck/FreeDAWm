//! `track.print_input`: record a track's module MIDI input over a range as a MIDI clip, offline
//! (what Record does for such a track when the transport stops; see `module_record`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::Route;
use soundcraft_time::Range;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "track.print_input",
        "Print MIDI Input",
        [],
        None,
        "{tracks?, range?: {start, end} (default: the selection)} → {clips: [{track, clip, notes}], start, end}. Each track's input must be modules' MIDI (`track.input module:…`); the session is rendered offline over the range and what the track's instrument was given becomes one MIDI clip per track (channels kept). A track that received nothing gets no clip.",
        has_selection,
        print_input
    )]
}

fn print_input(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.print_input";
    let tracks = tracks_required(e, C, p)?;
    for t in tracks.iter().filter_map(|t| e.session().track(*t)) {
        if !matches!(t.mixer.input, Route::Modules(_)) {
            return Err(bad(C, format!("`{}`'s input is not a module's MIDI (set it with track.input module:<track>/<module>)", t.name)));
        }
        if t.frozen_audio().is_some() {
            return Err(bad(C, format!("`{}` is frozen", t.name)));
        }
    }
    let r = match p.get("range") {
        Some(rv) => {
            let a = position_param(e, C, rv, "start")?.ok_or_else(|| bad(C, "`range.start` required"))?;
            let b = position_param(e, C, rv, "end")?.ok_or_else(|| bad(C, "`range.end` required"))?;
            Range::new(a.max(0), b)
        }
        None => e.session().edit.selection,
    };
    if r.is_empty() {
        return Err(bad(C, "the range to print is empty: give `range` or select one"));
    }
    let ids = crate::module_record::record_module_inputs(e, r, Some(&tracks));
    if ids.is_empty() {
        return Err(bad(C, "Nothing to print in that range: the modules sent no MIDI (is MIDI Out on?)"));
    }
    let s = e.session();
    let clips: Vec<Value> = s
        .tracks
        .iter()
        .flat_map(|t| {
            t.clips()
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(move |c| json!({"track": t.id.0, "clip": c.id, "notes": c.midi_playing().map_or(0, |q| q.notes.len())}))
        })
        .collect();
    Ok(json!({"clips": clips, "start": r.start, "end": r.end}))
}
