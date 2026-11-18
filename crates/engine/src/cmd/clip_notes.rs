//! Clip Notes: free text kept on a clip (`Clip::notes`), shown when the pointer rests on it.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::MAX_NOTE_CHARS;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "clip.notes",
        "Clip Notes...",
        [],
        None,
        "{clips?, text} — the selected clips' notes (empty clears them; at most 2000 characters)",
        has_selection,
        set_notes
    )]
}

fn set_notes(e: &mut Engine, p: &Value) -> Result<Value> {
    let text = p.get("text").and_then(Value::as_str).ok_or_else(|| bad("clip.notes", "`text` required"))?;
    // Control characters other than line breaks and tabs become spaces; the length is capped.
    let text: String = text.chars().map(|c| if c.is_control() && c != '\n' && c != '\t' { ' ' } else { c }).take(MAX_NOTE_CHARS).collect();
    let text = text.trim_end().to_string();
    let ids = clip_ids_param(e, p);
    if ids.is_empty() {
        return Err(bad("clip.notes", "select a clip first"));
    }
    let s = e.session_mut();
    let mut n = 0;
    for id in ids {
        if let Some(c) = s.find_clip_mut(id) {
            c.notes.clone_from(&text);
            n += 1;
        }
    }
    Ok(json!({"clips": n}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{Clip, SourceId, TrackId};

    #[test]
    fn notes_are_kept_on_the_clip_and_by_its_pieces() {
        let mut e = Engine::default();
        let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
        let s = e.session_mut();
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips = vec![Clip::audio(id, "c", SourceId(1), 0, 0, 1000)];
        e.execute("edit.select", &json!({"clips": [id.0]})).unwrap();
        e.execute("clip.notes", &json!({"text": "retake the second line\u{7}\n"})).unwrap();
        assert_eq!(e.session().find_clip(id).unwrap().1.notes, "retake the second line");
        e.execute("edit.separate", &json!({"tracks": [t.0], "at": 500})).unwrap();
        let notes: Vec<String> = e.session().track(t).unwrap().clips().iter().map(|c| c.notes.clone()).collect();
        assert_eq!(notes, vec!["retake the second line"; 2]);
        let long = "x".repeat(MAX_NOTE_CHARS + 10);
        e.execute("clip.notes", &json!({"clips": [id.0], "text": long})).unwrap();
        assert_eq!(e.session().find_clip(id).unwrap().1.notes.chars().count(), MAX_NOTE_CHARS);
        assert!(e.execute("clip.notes", &json!({"clips": [id.0]})).is_err());
    }
}
