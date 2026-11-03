//! The Lyrics tab's checks: the hook (title) phrase and the repetition count; the Copy for AI
//! prompt.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "lyrics.hook_set",
            "Mark as Hook",
            [],
            None,
            "{phrase} — the hook (title) phrase the repetition check counts; empty clears it (the song's name is used then)",
            always,
            hook_set
        ),
        cmd!(
            query "lyrics.ai_prompt",
            "Copy for AI",
            [],
            None,
            "{lines? ([lyric line numbers] the request is about), request? (default: alternatives for the line that keep its syllables and rhyme)} — a prompt to paste into any chat: the lyrics with sections, each line's syllables, stress and rhyme letter, the rhyme scheme, the selected lines and the request. Nothing is sent anywhere",
            always,
            ai_prompt
        ),
        cmd!(
            query "lyrics.repetition",
            "Repetition Check",
            [],
            None,
            "{} — the hook phrase (marked, else the session name), how often it is sung and where [{line (sheet line index), number, start, end}], and the most repeated non-function words [{word, count}]",
            always,
            repetition
        ),
    ]
}

/// The hook the repetition check counts: the marked phrase, else the session's name.
pub fn hook_of(e: &Engine) -> String {
    let s = e.session();
    s.lyric_sheet.hook.clone().unwrap_or_else(|| s.name.clone())
}

fn hook_set(e: &mut Engine, p: &Value) -> Result<Value> {
    let phrase = str_param(p, "phrase").ok_or_else(|| bad("lyrics.hook_set", "`phrase` required"))?;
    let phrase: String = phrase.chars().map(|c| if c.is_control() { ' ' } else { c }).take(80).collect();
    let phrase = phrase.trim().to_string();
    e.session_mut().lyric_sheet.hook = (!phrase.is_empty()).then_some(phrase);
    repetition(e, &json!({}))
}

fn ai_prompt(e: &mut Engine, p: &Value) -> Result<Value> {
    crate::lexicon::wait();
    let selected: Vec<u32> = p
        .get("lines")
        .and_then(Value::as_array)
        .map(|a| a.iter().take(200).filter_map(Value::as_u64).filter_map(|n| u32::try_from(n).ok()).collect())
        .unwrap_or_default();
    let s = e.session();
    let lines = || s.lyric_sheet.lines.iter().map(|l| l.text.as_str());
    let default = crate::lyric_tools::default_ai_request(lines(), &selected);
    let request: String =
        str_param(p, "request").map(|r| r.chars().take(2000).collect()).filter(|r: &String| !r.trim().is_empty()).unwrap_or_else(|| default.clone());
    let prompt = crate::lyric_tools::ai_prompt(lines(), &s.name, &selected, &request);
    Ok(json!({"prompt": prompt, "request": request, "default_request": default}))
}

fn repetition(e: &mut Engine, _p: &Value) -> Result<Value> {
    let hook = hook_of(e);
    let sheet = &e.session().lyric_sheet;
    let r = crate::lyric_tools::repetition(sheet.lines.iter().map(|l| l.text.as_str()), &hook);
    let kinds = sheet.kinds();
    let hits: Vec<Value> = r
        .hits
        .iter()
        .map(|(l, s, en)| {
            let number = match kinds.get(*l) {
                Some(soundcraft_model::LineKind::Lyric(n)) => Some(*n),
                _ => None,
            };
            json!({"line": l, "number": number, "start": s, "end": en})
        })
        .collect();
    let words: Vec<Value> = r.words.iter().map(|(w, n)| json!({"word": w, "count": n})).collect();
    Ok(json!({"hook": r.hook, "marked": sheet.hook.is_some(), "count": r.hits.len(), "hits": hits, "words": words}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn ai_prompt_command() {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"text": "[Verse 1]\nWhen the night was long\nYou sang a song"})).unwrap();
        let v = e.execute("lyrics.ai_prompt", &json!({"lines": [2]})).unwrap();
        assert!(v["request"].as_str().unwrap().contains("rhyme with \u{2018}long\u{2019}"));
        assert!(v["prompt"].as_str().unwrap().contains("## Rhyme scheme"));
        let v = e.execute("lyrics.ai_prompt", &json!({"request": "Make it sadder"})).unwrap();
        assert!(v["prompt"].as_str().unwrap().contains("## Request\nMake it sadder"));
    }

    #[test]
    fn hook_and_repetition_counts() {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"text": "[Chorus]\nJesus, the only one\nYou are the only one\nJesus"})).unwrap();
        let v = e.execute("lyrics.hook_set", &json!({"phrase": "The only one"})).unwrap();
        assert_eq!(v["count"], 2);
        assert_eq!(v["hits"][1]["number"], 2);
        assert_eq!(v["words"][0]["word"], "jesus");
        assert_eq!(v["words"][0]["count"], 2);
        // Undoable; cleared, the session's name is the hook.
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().lyric_sheet.hook.is_none());
        e.session_mut().name = "Jesus".into();
        let v = e.execute("lyrics.repetition", &json!({})).unwrap();
        assert_eq!((v["hook"].as_str(), v["count"].as_u64(), v["marked"].as_bool()), (Some("Jesus"), Some(2), Some(false)));
    }
}
