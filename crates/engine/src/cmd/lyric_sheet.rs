//! The lyric sheet (Window › Lyrics): set and read the song's words, line by line with stable ids,
//! syllable counts and rhyme letters; rhyme suggestions. See `soundcraft_model::LyricSheet` and
//! `crate::lyric_text`.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{MAX_SHEET_LINES, SheetLine, carry_ids, sheet_texts};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "lyrics.sheet_set",
            "Edit Lyrics",
            [],
            None,
            "{text (the whole sheet, one line per line; \"[Verse 1]\" lines are headings)} or {lines: [{id?, text}]} — sets the lyric sheet. Unchanged and edited lines keep their ids (and so their markers on the Lyrics ruler); a line given with an id keeps it",
            always,
            sheet_set
        ),
        cmd!(
            query "lyrics.sheet_get",
            "Lyric Sheet",
            [],
            None,
            "{} — the lyric sheet: text, and per line id, text, number (lyric lines only), heading, syllables, rhyme letter, rhyme word, rhyming words [{word, start, end (bytes), kinds: perfect|slant|multi|internal}], stress (\"da-DUM-da\"), alternates {count, active, versions} when the line has some, fit against the same line of the section's first take {partner (line number), section, flag (why it does not fit, or null)}",
            always,
            sheet_get
        ),
    ]
}

fn sheet_set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.sheet_set";
    let old = &e.session().lyric_sheet.lines;
    // Fresh ids counted here; the session takes them only if the sheet changes (no undo step
    // for setting the same text).
    let mut next = e.session().next_id.max(1);
    let mut alloc = || {
        let id = next;
        next = next.saturating_add(1);
        id
    };
    let lines: Vec<SheetLine> = if let Some(text) = str_param(p, "text") {
        carry_ids(old, &sheet_texts(text), &mut alloc)
    } else if let Some(Value::Array(items)) = p.get("lines") {
        if items.len() > MAX_SHEET_LINES {
            return Err(bad(C, format!("a sheet holds at most {MAX_SHEET_LINES} lines")));
        }
        let texts: Vec<String> =
            items.iter().map(|l| sheet_texts(l.get("text").and_then(Value::as_str).unwrap_or("")).into_iter().next().unwrap_or_default()).collect();
        let mut lines = carry_ids(old, &texts, &mut alloc);
        // Ids given explicitly win (when unique); a clash gets a fresh one past them all.
        let mut used: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for (l, item) in lines.iter_mut().zip(items) {
            if let Some(id) = item.get("id").and_then(Value::as_u64).filter(|id| *id != 0 && !used.contains(id)) {
                l.id = id;
            }
            used.insert(l.id);
        }
        let mut fresh = lines.iter().map(|l| l.id).max().unwrap_or(0).saturating_add(1).max(alloc());
        let mut seen = std::collections::HashSet::new();
        for l in &mut lines {
            if !seen.insert(l.id) {
                l.id = fresh;
                fresh = fresh.saturating_add(1);
                seen.insert(l.id);
            }
        }
        lines
    } else {
        return Err(bad(C, "give `text` or `lines`"));
    };
    if e.session().lyric_sheet.lines == lines {
        return Ok(sheet_json(e));
    }
    let s = e.session_mut();
    let top = lines.iter().map(|l| l.id).max().unwrap_or(0);
    s.next_id = s.next_id.max(top.saturating_add(1));
    s.lyric_sheet.lines = lines;
    // A deleted line's marker on the Lyrics ruler goes with it (undo brings both back).
    s.lyric_sheet.tidy_places();
    Ok(sheet_json(e))
}

/// The sheet as `lyrics.sheet_get` returns it.
pub(crate) fn sheet_json(e: &Engine) -> Value {
    let sheet = &e.session().lyric_sheet;
    let info = crate::lyric_text::analyse(sheet.lines.iter().map(|l| l.text.as_str()));
    let marks = crate::lyric_tools::rhyme_marks(sheet.lines.iter().map(|l| l.text.as_str()));
    let fit = crate::lyric_tools::line_fit(sheet.lines.iter().map(|l| l.text.as_str()));
    let lines: Vec<Value> = sheet
        .lines
        .iter()
        .zip(info)
        .enumerate()
        .map(|(n, (l, i))| {
            let word = i.word.and_then(|(a, b)| l.text.get(a..b));
            let rhyming: Vec<Value> = marks
                .iter()
                .filter(|m| m.line == n)
                .map(|m| {
                    let kinds: Vec<&str> = crate::lyric_tools::MarkKind::ALL
                        .into_iter()
                        .filter(|k| m.kinds & k.bit() != 0)
                        .map(crate::lyric_tools::MarkKind::name)
                        .collect();
                    json!({"word": l.text.get(m.start..m.end), "start": m.start, "end": m.end, "kinds": kinds})
                })
                .collect();
            json!({
                "id": l.id,
                "text": l.text,
                "number": i.number,
                "heading": i.heading,
                "syllables": i.syllables,
                "rhyme": i.rhyme.map(String::from),
                "rhymes": i.rhymes,
                "rhyme_word": word,
                "rhyming": rhyming,
                "alternates": (!l.alts.is_empty()).then(|| json!({"count": l.alts.len(), "active": l.active, "versions": l.alts})),
                "stress": fit.iter().find(|f| f.line == n).map(|f| crate::lyric_tools::stress_text(&f.stress)),
                "fit": fit.iter().find(|f| f.line == n && f.partner.is_some()).map(|f| json!({
                    "partner": f.partner_number,
                    "section": f.partner_section,
                    "flag": f.flag,
                })),
                "placed": sheet.place_of(l.id).map(|q| json!({"start": q.start, "end": q.end})),
            })
        })
        .collect();
    json!({"text": sheet.text(), "lines": lines})
}

fn sheet_get(e: &mut Engine, _p: &Value) -> Result<Value> {
    // Counts and rhymes from the dictionary, not the heuristic stand-in (loads it once).
    crate::lexicon::wait();
    Ok(sheet_json(e))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    const SONG: &str =
        "[Verse 1]\nWhen the night was long\nAnd my heart could not be still\nYou were there to sing the song\nStanding strong upon the hill";

    fn ids(e: &Engine) -> Vec<u64> {
        e.session().lyric_sheet.lines.iter().map(|l| l.id).collect()
    }

    #[test]
    fn sheet_set_and_get_with_stable_ids() {
        let mut e = Engine::default();
        let v = e.execute("lyrics.sheet_set", &json!({"text": SONG})).unwrap();
        assert_eq!(v["lines"].as_array().unwrap().len(), 5);
        assert_eq!(v["lines"][1]["number"], 1);
        assert_eq!(v["lines"][1]["syllables"], 5);
        assert_eq!(v["lines"][1]["rhyme"], "A");
        assert_eq!(v["lines"][2]["rhyme"], "B");
        assert_eq!(v["lines"][3]["rhyme"], "A");
        assert_eq!(v["lines"][4]["rhyme_word"], "hill");
        assert_eq!(v["lines"][0]["heading"], true);
        let v = e.execute("lyrics.sheet_get", &json!({})).unwrap();
        assert_eq!(v["lines"][1]["rhyming"][0]["word"], "long");
        assert_eq!(v["lines"][1]["rhyming"][0]["kinds"][0], "perfect");
        assert_eq!(v["lines"][1]["stress"], "da-da-DUM-da-DUM");
        assert!(v["lines"][1]["fit"].is_null());
        let before = ids(&e);
        // Edit line 2, insert a line above everything: ids follow their lines.
        let edited = SONG.replace("could not be still", "would not keep still");
        e.execute("lyrics.sheet_set", &json!({"text": format!("Intro words\n{edited}")})).unwrap();
        let after = ids(&e);
        assert_eq!(&after[1..], &before[..]);
        assert!(!before.contains(&after[0]));
        // Undo brings the old sheet back.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(ids(&e), before);
        assert_eq!(e.session().lyric_sheet.text(), SONG);
        // Saved with the session.
        let back: soundcraft_model::Session = serde_json::from_str(&serde_json::to_string(e.session()).unwrap()).unwrap();
        assert_eq!(back.lyric_sheet, e.session().lyric_sheet);
        // The same text again is no undo step.
        let steps = e.undo_history().len();
        e.execute("lyrics.sheet_set", &json!({"text": SONG})).unwrap();
        assert_eq!(e.undo_history().len(), steps);
    }

    #[test]
    fn sheet_set_by_lines_keeps_given_ids() {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"lines": [{"id": 900_000, "text": "one"}, {"text": "two"}, {"id": 900_000, "text": "dup"}]})).unwrap();
        let ids = ids(&e);
        assert_eq!(ids[0], 900_000);
        assert!(ids[1] != ids[0] && ids[2] != ids[0] && ids[1] != ids[2]);
        assert!(e.session().next_id > 900_000);
        assert!(e.execute("lyrics.sheet_set", &json!({})).is_err());
        let r = e.execute("lyrics.rhymes", &json!({"word": "still"})).unwrap();
        assert!(r["rhymes"].as_array().unwrap().iter().any(|w| w == "hill"));
    }
}
