//! The Lyrics tab's word finder and dictionary: rhymes (perfect, multi-syllable, slant) with
//! syllable counts and stress, synonyms and related words, definitions, and replacing a word of
//! the sheet. Offline, from the bundled cmudict and WordNet ([`crate::lexicon`]); the first call
//! loads them (about a second).

use super::*;
use crate::cmd;
use crate::lexicon::{self, Found, RhymeKind, WordLookup};
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "lyrics.rhymes",
            "Suggest Rhymes",
            [],
            None,
            "{word, max? (per kind, default 24)} — words that rhyme with `word`: `rhymes` (perfect and multi, plain), and `perfect`, `multi`, `slant` lists of {word, syllables, stress (\"da-DUM\")}, most common first (cmudict; the built-in list for words it lacks)",
            always,
            rhymes
        ),
        cmd!(
            query "lyrics.thesaurus",
            "Synonyms",
            [],
            None,
            "{word} — WordNet synonyms and related words of `word` (or its base form): synonyms, similar, broader, narrower, opposites, each a list of {word, syllables}",
            always,
            thesaurus
        ),
        cmd!(
            query "lyrics.define",
            "Define Word",
            [],
            None,
            "{word} — WordNet senses of `word` (or its base form), most used first: [{pos (noun|verb|adjective|adverb), definition, examples, synonyms}]; plus its syllables and stress",
            always,
            define
        ),
        cmd!(
            "lyrics.replace_word",
            "Replace Word",
            [],
            None,
            "{line (sheet line id) or number (lyric line number), start, end (byte range of the word in the line), with (the new word), word? (the word expected there; refused if it differs)} — replaces one word of the lyric sheet (one undo step)",
            always,
            replace_word
        ),
    ]
}

fn word_param<'a>(cmd: &str, p: &'a Value) -> Result<&'a str> {
    let w = str_param(p, "word").map(str::trim).filter(|w| !w.is_empty()).ok_or_else(|| bad(cmd, "`word` required"))?;
    if w.chars().count() > 64 {
        return Err(bad(cmd, "`word` is too long"));
    }
    Ok(w)
}

fn found_json(f: &Found) -> Value {
    json!({"word": f.word, "syllables": f.syllables, "stress": f.stress})
}

fn rhymes(_e: &mut Engine, p: &Value) -> Result<Value> {
    let word = word_param("lyrics.rhymes", p)?;
    let max = usize::try_from(i64_or(p, "max", 24).clamp(1, 200)).unwrap_or(24);
    let lex = lexicon::wait();
    let found = lex.rhymes(word, max);
    let of = |k: RhymeKind| found.iter().filter(|f| f.kind == k).map(found_json).collect::<Vec<_>>();
    Ok(json!({
        "word": word,
        "key": crate::lyric_text::rhyme_key(word),
        "known": lex.syllables(word).is_some(),
        "rhymes": crate::lyric_text::suggest_rhymes(word, max),
        "perfect": of(RhymeKind::Perfect),
        "multi": of(RhymeKind::Multi),
        "slant": of(RhymeKind::Slant),
    }))
}

fn with_syllables(words: &[String]) -> Vec<Value> {
    words.iter().map(|w| json!({"word": w, "syllables": crate::lyric_text::line_syllables(w)})).collect()
}

fn thesaurus(_e: &mut Engine, p: &Value) -> Result<Value> {
    let word = word_param("lyrics.thesaurus", p)?;
    let r = lexicon::wait().related(word);
    Ok(json!({
        "word": word,
        "synonyms": with_syllables(&r.synonyms),
        "similar": with_syllables(&r.similar),
        "broader": with_syllables(&r.broader),
        "narrower": with_syllables(&r.narrower),
        "opposites": with_syllables(&r.opposites),
    }))
}

fn define(_e: &mut Engine, p: &Value) -> Result<Value> {
    let word = word_param("lyrics.define", p)?;
    let lex = lexicon::wait();
    let senses = lex.senses(word);
    Ok(json!({
        "word": word,
        "syllables": crate::lyric_text::syllables(word),
        "stress": lexicon::stress_text(&crate::lyric_text::word_stresses(word)),
        "senses": senses,
    }))
}

fn replace_word(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.replace_word";
    let sheet = &e.session().lyric_sheet;
    let idx = if let Some(id) = p.get("line").and_then(Value::as_u64) {
        sheet.lines.iter().position(|l| l.id == id).ok_or_else(|| bad(C, format!("no sheet line with id {id}")))?
    } else if let Some(n) = p.get("number").and_then(Value::as_u64) {
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        let id = sheet.line_numbered(n).map(|l| l.id).ok_or_else(|| bad(C, format!("no lyric line {n}")))?;
        sheet.lines.iter().position(|l| l.id == id).ok_or_else(|| bad(C, "line gone"))?
    } else {
        return Err(bad(C, "give `line` or `number`"));
    };
    let start = usize::try_from(p.get("start").and_then(Value::as_u64).ok_or_else(|| bad(C, "`start` required"))?).unwrap_or(usize::MAX);
    let end = usize::try_from(p.get("end").and_then(Value::as_u64).ok_or_else(|| bad(C, "`end` required"))?).unwrap_or(usize::MAX);
    let with = str_param(p, "with").ok_or_else(|| bad(C, "`with` required"))?;
    if with.chars().any(char::is_control) || with.chars().count() > 64 {
        return Err(bad(C, "`with` must be a short word or phrase"));
    }
    let text = sheet.lines.get(idx).map(|l| l.text.clone()).unwrap_or_default();
    let old = text.get(start..end).filter(|_| start < end).ok_or_else(|| bad(C, "no word at that range"))?;
    if let Some(want) = str_param(p, "word")
        && want != old
    {
        return Err(bad(C, format!("the line has `{old}` there, not `{want}`")));
    }
    let old = old.to_string();
    let new_text = format!("{}{with}{}", text.get(..start).unwrap_or(""), text.get(end..).unwrap_or(""));
    if new_text.chars().count() > soundcraft_model::MAX_SHEET_LINE_CHARS {
        return Err(bad(C, "the line would be too long"));
    }
    if let Some(l) = e.session_mut().lyric_sheet.lines.get_mut(idx) {
        l.text = new_text.clone();
        l.sync_alts();
    }
    Ok(json!({"line": idx, "old": old, "text": new_text, "start": start, "end": start + with.len()}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn finder_returns_rhymes_synonyms_and_definitions() {
        let mut e = Engine::default();
        let r = e.execute("lyrics.rhymes", &json!({"word": "hill", "max": 40})).unwrap();
        let perfect = r["perfect"].as_array().unwrap();
        assert!(perfect.iter().any(|f| f["word"] == "still" && f["syllables"] == 1), "{r}");
        assert!(r["slant"].as_array().is_some());
        // Grouped by syllable count in the UI: several counts present.
        let counts: std::collections::HashSet<u64> = perfect.iter().filter_map(|f| f["syllables"].as_u64()).collect();
        assert!(counts.len() >= 2, "{counts:?}");
        let t = e.execute("lyrics.thesaurus", &json!({"word": "happy"})).unwrap();
        assert!(t["synonyms"].as_array().unwrap().iter().any(|w| w["word"] == "glad" && w["syllables"] == 1), "{t}");
        let d = e.execute("lyrics.define", &json!({"word": "still"})).unwrap();
        let senses = d["senses"].as_array().unwrap();
        assert!(senses.iter().any(|s| s["pos"] == "adverb") && senses.iter().any(|s| s["pos"] == "noun"));
        assert!(senses.iter().any(|s| !s["examples"].as_array().unwrap().is_empty()));
        assert!(e.execute("lyrics.define", &json!({})).is_err());
    }

    #[test]
    fn replace_word_is_one_undo_step() {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"text": "[Verse 1]\nStanding strong upon the hill"})).unwrap();
        let id = e.session().lyric_sheet.lines[1].id;
        let v = e.execute("lyrics.replace_word", &json!({"line": id, "start": 9, "end": 15, "word": "strong", "with": "tall"})).unwrap();
        assert_eq!(v["text"], "Standing tall upon the hill");
        assert_eq!(e.session().lyric_sheet.lines[1].text, "Standing tall upon the hill");
        assert_eq!(e.session().lyric_sheet.lines[1].id, id);
        // The word given must be the one there; ranges are checked.
        assert!(e.execute("lyrics.replace_word", &json!({"number": 1, "start": 9, "end": 13, "word": "nope", "with": "x"})).is_err());
        assert!(e.execute("lyrics.replace_word", &json!({"number": 1, "start": 90, "end": 93, "with": "x"})).is_err());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().lyric_sheet.lines[1].text, "Standing strong upon the hill");
    }
}
