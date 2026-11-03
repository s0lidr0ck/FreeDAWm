//! Session notes (Window › Notes): add, edit (text, done, pin, order), delete, list. Each edit is
//! one undo step; typing into a note joins one step per note (`Engine::execute_merged`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{MAX_NOTE_CHARS, MAX_NOTES, Note, Session, split_tags};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "notes.add",
            "Add Note",
            [],
            None,
            "{text? (words; `#word` adds a tag), at? (pins it to that time), at_playhead? (true: pins it to the playhead), done?, index? (list position, default the end)} — a new note; returns its id",
            always,
            add
        ),
        cmd!(
            "notes.edit",
            "Edit Note",
            [],
            None,
            "{id, text? (`#word` sets the tags), done?, at? (a time pins it; null unpins), at_playhead? (true: pins it to the playhead), index? (moves it to that list position)}",
            has_notes,
            edit
        ),
        cmd!("notes.delete", "Delete Note", [], None, "{id}", has_notes, delete),
        cmd!(
            query "notes.list",
            "Notes",
            [],
            None,
            "{show? (open|done|all, default all), tag?} — the notes in list order: id, text, done, tags, at (samples, seconds, and the main counter), created",
            always,
            list
        ),
    ]
}

fn has_notes(e: &Engine) -> std::result::Result<(), String> {
    if e.session().notes.is_empty() { Err("there are no notes".into()) } else { Ok(()) }
}

fn playhead(e: &Engine) -> Samples {
    if e.transport.playing { e.transport.position } else { e.session().edit.selection.start }
}

/// `at` / `at_playhead` from the params: `None` = not given, `Some(None)` = unpin.
fn pin_param(e: &Engine, cmd: &str, p: &Value) -> Result<Option<Option<Samples>>> {
    if bool_or(p, "at_playhead", false) {
        return Ok(Some(Some(playhead(e).max(0))));
    }
    match p.get("at") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => Ok(Some(position_param(e, cmd, p, "at")?.map(|a| a.max(0)))),
    }
}

fn index_of(s: &Session, cmd: &str, p: &Value) -> Result<usize> {
    let id = p.get("id").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`id` required"))?;
    s.notes.iter().position(|n| n.id == id).ok_or_else(|| bad(cmd, format!("no note {id}")))
}

fn set_text(n: &mut Note, typed: &str) {
    let mut typed: String = typed.chars().take(MAX_NOTE_CHARS).collect();
    typed = typed.replace(['\r', '\n'], " ");
    let (text, tags) = split_tags(&typed);
    n.text = text;
    n.tags = tags;
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "notes.add";
    if e.session().notes.len() >= MAX_NOTES {
        return Err(bad(C, format!("a session keeps at most {MAX_NOTES} notes")));
    }
    let at = pin_param(e, C, p)?.flatten();
    let s = e.session_mut();
    let id = s.alloc();
    let mut n = Note { id, at, done: bool_or(p, "done", false), created: super::scout::now_rfc3339(), ..Note::default() };
    set_text(&mut n, str_param(p, "text").unwrap_or(""));
    let index = p.get("index").and_then(Value::as_u64).map_or(s.notes.len(), |i| usize::try_from(i).unwrap_or(usize::MAX).min(s.notes.len()));
    s.notes.insert(index, n);
    Ok(json!({"id": id, "index": index}))
}

fn edit(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "notes.edit";
    let i = index_of(e.session(), C, p)?;
    let pin = pin_param(e, C, p)?;
    let s = e.session_mut();
    let Some(n) = s.notes.get_mut(i) else { return Err(bad(C, "no such note")) };
    if let Some(t) = str_param(p, "text") {
        set_text(n, t);
    }
    if let Some(d) = p.get("done").and_then(Value::as_bool) {
        n.done = d;
    }
    if let Some(at) = pin {
        n.at = at;
    }
    let mut index = i;
    if let Some(to) = p.get("index").and_then(Value::as_u64) {
        let note = s.notes.remove(i);
        index = usize::try_from(to).unwrap_or(usize::MAX).min(s.notes.len());
        s.notes.insert(index, note);
    }
    Ok(json!({"id": s.notes.get(index).map_or(0, |n| n.id), "index": index}))
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = index_of(e.session(), "notes.delete", p)?;
    let n = e.session_mut().notes.remove(i);
    Ok(json!({"id": n.id, "deleted": true}))
}

/// Whether a note shows under `show` (open|done|all) and `tag`.
pub fn note_shown(n: &Note, show: &str, tag: Option<&str>) -> bool {
    let by_state = match show {
        "open" => !n.done,
        "done" => n.done,
        _ => true,
    };
    by_state && tag.is_none_or(|t| n.tags.iter().any(|x| x.eq_ignore_ascii_case(t.trim_start_matches('#'))))
}

fn list(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session();
    let show = str_param(p, "show").unwrap_or("all");
    if !["open", "done", "all"].contains(&show) {
        return Err(bad("notes.list", "`show` is open, done or all"));
    }
    let tag = str_param(p, "tag");
    let rate = s.sample_rate.as_f64().max(1.0);
    let notes: Vec<Value> = s
        .notes
        .iter()
        .filter(|n| note_shown(n, show, tag))
        .map(|n| {
            let at = n.at.map(|a| json!({"samples": a, "seconds": a as f64 / rate, "counter": s.format_position(a, s.edit.main_counter)}));
            json!({"id": n.id, "text": n.text, "done": n.done, "tags": n.tags, "at": at, "created": n.created})
        })
        .collect();
    Ok(json!({"notes": notes, "count": s.notes.len()}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    fn ids(e: &mut Engine, show: &str) -> Vec<u64> {
        let v = e.execute("notes.list", &json!({"show": show})).unwrap();
        v["notes"].as_array().unwrap().iter().map(|n| n["id"].as_u64().unwrap()).collect()
    }

    #[test]
    fn add_edit_toggle_pin_filter_reorder_delete() {
        let mut e = Engine::default();
        let a = e.execute("notes.add", &json!({"text": "Lower the kick 2 dB #mix"})).unwrap()["id"].as_u64().unwrap();
        e.execute("transport.locate", &json!({"at": 96_000})).unwrap();
        let b = e.execute("notes.add", &json!({"text": "Retake line 7", "at_playhead": true})).unwrap()["id"].as_u64().unwrap();
        let c = e.execute("notes.add", &json!({"text": "Ask about the bridge", "at": {"seconds": 1.0}})).unwrap()["id"].as_u64().unwrap();
        let s = e.session();
        assert_eq!(s.notes[0].tags, vec!["mix".to_string()]);
        assert_eq!(s.notes[0].text, "Lower the kick 2 dB");
        assert_eq!(s.notes[2].at, Some(48_000));
        assert!(s.notes[1].at.is_some());
        e.execute("notes.edit", &json!({"id": a, "done": true})).unwrap();
        assert_eq!(ids(&mut e, "open"), vec![b, c]);
        assert_eq!(ids(&mut e, "done"), vec![a]);
        assert_eq!(ids(&mut e, "all"), vec![a, b, c]);
        let v = e.execute("notes.list", &json!({"tag": "#mix"})).unwrap();
        assert_eq!(v["notes"].as_array().unwrap().len(), 1);
        e.execute("notes.edit", &json!({"id": c, "index": 0})).unwrap();
        assert_eq!(ids(&mut e, "all"), vec![c, a, b]);
        e.execute("notes.edit", &json!({"id": c, "at": null, "text": "Ask Davide #vocal"})).unwrap();
        let n = e.session().notes[0].clone();
        assert_eq!((n.at, n.text.as_str(), n.tags), (None, "Ask Davide", vec!["vocal".to_string()]));
        e.execute("notes.delete", &json!({"id": b})).unwrap();
        assert_eq!(ids(&mut e, "all"), vec![c, a]);
        assert!(e.execute("notes.edit", &json!({"id": 9999, "done": true})).is_err());
        assert!(e.execute("notes.list", &json!({"show": "later"})).is_err());
    }

    #[test]
    fn each_edit_is_one_undo_step_and_notes_are_saved() {
        let mut e = Engine::default();
        let a = e.execute("notes.add", &json!({"text": "one"})).unwrap()["id"].as_u64().unwrap();
        e.execute("notes.edit", &json!({"id": a, "done": true})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(!e.session().notes[0].done);
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().notes.is_empty());
        e.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(e.session().notes.len(), 1);
        // Typing joins one step per note.
        for t in ["o", "on", "one!"] {
            e.execute_merged("notes.edit", &json!({"id": a, "text": t}), "note-text").unwrap();
        }
        e.end_merge();
        assert_eq!(e.session().notes[0].text, "one!");
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().notes[0].text, "one");
        // Saved with the session.
        let json = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.notes, e.session().notes);
        // An empty list is left out of the file.
        let blank = serde_json::to_value(soundcraft_model::Session::default()).unwrap();
        assert!(blank.get("notes").is_none());
    }
}
