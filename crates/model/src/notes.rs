//! Session notes (Window › Notes): a to-do list saved with the session ("lower the kick 2 dB when
//! we come back"), each with a done box, free `#tags` and an optional time it is pinned to.

use serde::{Deserialize, Deserializer, Serialize};
use soundcraft_time::Samples;

/// Most notes one session keeps.
pub const MAX_NOTES: usize = 2000;
/// Longest note text, characters.
pub const MAX_NOTE_CHARS: usize = 2000;
/// Most tags on one note.
pub const MAX_NOTE_TAGS: usize = 16;
/// Longest tag, characters.
pub const MAX_TAG_CHARS: usize = 32;

/// One note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Note {
    /// Unique in the session (from `Session::alloc`).
    pub id: u64,
    /// The text without its tags.
    pub text: String,
    pub done: bool,
    /// Free words, without the `#`, lowercase.
    pub tags: Vec<String>,
    /// The time it is pinned to (a link that locates there; a flag on the Markers ruler).
    pub at: Option<Samples>,
    /// When it was written (RFC 3339 UTC; empty when unknown).
    pub created: String,
}

/// Splits typed note text into its words and its `#tags`: "Lower the kick #mix" is
/// ("Lower the kick", ["mix"]). Tags are lowercased, deduplicated and capped.
pub fn split_tags(typed: &str) -> (String, Vec<String>) {
    let mut words = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    for w in typed.split(' ') {
        let tag = w.strip_prefix('#').map(|t| t.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_'));
        match tag {
            Some(t) if !t.is_empty() => {
                let mut t = t.to_lowercase();
                cut_chars(&mut t, MAX_TAG_CHARS);
                if !tags.contains(&t) && tags.len() < MAX_NOTE_TAGS {
                    tags.push(t);
                }
            }
            _ => words.push(w),
        }
    }
    (words.join(" ").trim().to_string(), tags)
}

/// The text to edit for a note: its text, then its tags as `#tag` words.
pub fn note_typed(n: &Note) -> String {
    let mut s = n.text.clone();
    for t in &n.tags {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push('#');
        s.push_str(t);
    }
    s
}

fn cut_chars(s: &mut String, max: usize) {
    if let Some((i, _)) = s.char_indices().nth(max) {
        s.truncate(i);
    }
}

/// Notes a session file can hold: control characters (but not line breaks) become spaces, text
/// and tags capped, times non-negative, ids unique (a duplicate is dropped), the list capped.
pub fn sanitize_notes(notes: &mut Vec<Note>) {
    let mut seen = std::collections::HashSet::new();
    notes.retain_mut(|n| {
        n.text = n.text.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect();
        cut_chars(&mut n.text, MAX_NOTE_CHARS);
        n.tags.retain(|t| !t.trim().is_empty());
        for t in &mut n.tags {
            *t = t.chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect::<String>().to_lowercase();
            cut_chars(t, MAX_TAG_CHARS);
        }
        n.tags.truncate(MAX_NOTE_TAGS);
        n.at = n.at.map(|a| a.max(0));
        cut_chars(&mut n.created, 40);
        n.id != 0 && seen.insert(n.id)
    });
    notes.truncate(MAX_NOTES);
}

/// Reads notes leniently: an unreadable note is skipped, an unreadable list loads as none.
pub(crate) fn de_notes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Note>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let serde_json::Value::Array(items) = v else { return Ok(Vec::new()) };
    let mut notes: Vec<Note> = items.into_iter().take(MAX_NOTES * 2).filter_map(|n| serde_json::from_value(n).ok()).collect();
    sanitize_notes(&mut notes);
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_split_from_text_and_back() {
        let (text, tags) = split_tags("Lower the kick 2 dB #Mix when we come back #mix #drums,");
        assert_eq!(text, "Lower the kick 2 dB when we come back");
        assert_eq!(tags, vec!["mix".to_string(), "drums".to_string()]);
        let n = Note { id: 1, text, tags, ..Note::default() };
        assert_eq!(note_typed(&n), "Lower the kick 2 dB when we come back #mix #drums");
        assert_eq!(split_tags("# alone").0, "# alone");
    }

    #[test]
    fn hostile_notes_are_tamed() {
        let v = serde_json::json!([
            {"id": 3, "text": "a\u{0}b", "at": -40, "tags": ["Mix", " "]},
            {"id": 3, "text": "duplicate"},
            {"id": "x"},
            {"id": 0, "text": "no id"},
            {"id": 4, "text": "y".repeat(5000), "done": true}
        ]);
        let n = de_notes(v).unwrap();
        assert_eq!(n.len(), 2);
        assert_eq!((n[0].text.as_str(), n[0].at, n[0].tags.clone()), ("a b", Some(0), vec!["mix".to_string()]));
        assert_eq!(n[1].text.chars().count(), MAX_NOTE_CHARS);
        assert!(de_notes(serde_json::json!({"no": 1})).unwrap().is_empty());
    }
}
