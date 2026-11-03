//! Alternate lines and drafts (Window › Lyrics): a line can hold alternate versions, one active
//! (what the sheet, the Lyrics ruler marker and the Teleprompter show); a draft is a named
//! snapshot of the whole sheet that can be restored (undoable) or compared.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Draft, MAX_ALTERNATES, MAX_DRAFTS, sheet_texts};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "lyrics.alt_add",
            "Add Alternate",
            [],
            None,
            "{line (sheet line id) or number (lyric line number), text? (default: a copy of the line)} — adds an alternate version of a lyric line and makes it the active one (at most 16 versions)",
            always,
            alt_add
        ),
        cmd!(
            "lyrics.alt_switch",
            "Switch Alternate",
            [],
            None,
            "{line or number, index? (from 0) or step? (+1 next, -1 previous; default +1, wrapping)} — makes another version of the line active; the sheet, the Lyrics ruler and the Teleprompter show it",
            always,
            alt_switch
        ),
        cmd!(
            "lyrics.alt_delete",
            "Delete Alternate",
            [],
            None,
            "{line or number, index? (default the active one)} — removes one version of the line (the next one becomes active; one left is no alternates)",
            always,
            alt_delete
        ),
        cmd!(
            "lyrics.draft_save",
            "Save Draft",
            [],
            None,
            "{name? (default \"Draft N\")} — saves the whole lyric sheet as a named draft with the time (at most 50; the oldest goes)",
            always,
            draft_save
        ),
        cmd!(
            "lyrics.draft_restore",
            "Restore Draft",
            [],
            None,
            "{draft (id) or name} — puts a draft's lines back as the sheet (undoable; lines gone take their ruler markers with them)",
            always,
            draft_restore
        ),
        cmd!("lyrics.draft_delete", "Delete Draft", [], None, "{draft (id) or name} — deletes a saved draft", always, draft_delete),
        cmd!(
            query "lyrics.drafts",
            "Drafts",
            [],
            None,
            "{} — the saved drafts, oldest first: id, name, created, lines (count), text",
            always,
            drafts
        ),
    ]
}

/// The index in the sheet of the line `p` names (`line` id or lyric `number`).
fn line_index(e: &Engine, cmd: &str, p: &Value) -> Result<usize> {
    let sheet = &e.session().lyric_sheet;
    let id = if let Some(id) = p.get("line").and_then(Value::as_u64) {
        id
    } else if let Some(n) = p.get("number").and_then(Value::as_u64) {
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        sheet.line_numbered(n).map(|l| l.id).ok_or_else(|| bad(cmd, format!("no lyric line {n}")))?
    } else {
        return Err(bad(cmd, "give `line` or `number`"));
    };
    let i = sheet.lines.iter().position(|l| l.id == id).ok_or_else(|| bad(cmd, format!("no sheet line with id {id}")))?;
    if sheet.number_of(id).is_none() {
        return Err(bad(cmd, "that line is a heading or blank"));
    }
    Ok(i)
}

fn line_json(e: &Engine, i: usize) -> Value {
    let l = e.session().lyric_sheet.lines.get(i);
    json!({"line": l.map(|l| l.id), "text": l.map(|l| l.text.clone()), "alts": l.map(|l| l.alts.clone()), "active": l.map(|l| l.active)})
}

fn alt_add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.alt_add";
    let i = line_index(e, C, p)?;
    let text = str_param(p, "text").map(|t| sheet_texts(t).into_iter().next().unwrap_or_default());
    let Some(l) = e.session_mut().lyric_sheet.lines.get_mut(i) else { return Err(bad(C, "line gone")) };
    if l.alts.is_empty() {
        l.alts.push(l.text.clone());
    }
    if l.alts.len() >= MAX_ALTERNATES {
        return Err(bad(C, format!("a line holds at most {MAX_ALTERNATES} versions")));
    }
    let new = text.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| l.text.clone());
    l.alts.push(new.clone());
    l.active = u32::try_from(l.alts.len() - 1).unwrap_or(0);
    l.text = new;
    l.sync_alts();
    e.session_mut().lyric_sheet.tidy_places();
    Ok(line_json(e, i))
}

fn alt_switch(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.alt_switch";
    let i = line_index(e, C, p)?;
    let Some(l) = e.session_mut().lyric_sheet.lines.get_mut(i) else { return Err(bad(C, "line gone")) };
    let n = l.alts.len();
    if n < 2 {
        return Err(bad(C, "the line has no alternates"));
    }
    let to = if let Some(ix) = p.get("index").and_then(Value::as_u64) {
        usize::try_from(ix).ok().filter(|ix| *ix < n).ok_or_else(|| bad(C, format!("index must be below {n}")))?
    } else {
        let step = i64_or(p, "step", 1).signum();
        let cur = i64::from(l.active);
        usize::try_from((cur + step).rem_euclid(n as i64)).unwrap_or(0)
    };
    l.active = u32::try_from(to).unwrap_or(0);
    l.text = l.alts.get(to).cloned().unwrap_or_default();
    l.sync_alts();
    e.session_mut().lyric_sheet.tidy_places();
    Ok(line_json(e, i))
}

fn alt_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.alt_delete";
    let i = line_index(e, C, p)?;
    let Some(l) = e.session_mut().lyric_sheet.lines.get_mut(i) else { return Err(bad(C, "line gone")) };
    let n = l.alts.len();
    if n < 2 {
        return Err(bad(C, "the line has no alternates"));
    }
    let ix = match p.get("index").and_then(Value::as_u64) {
        Some(ix) => usize::try_from(ix).ok().filter(|ix| *ix < n).ok_or_else(|| bad(C, format!("index must be below {n}")))?,
        None => l.active as usize,
    };
    l.alts.remove(ix);
    let active = (l.active as usize).min(l.alts.len().saturating_sub(1));
    let active = if ix < l.active as usize { active.saturating_sub(1) } else { active };
    l.active = u32::try_from(active).unwrap_or(0);
    l.text = l.alts.get(active).cloned().unwrap_or_default();
    l.sync_alts();
    e.session_mut().lyric_sheet.tidy_places();
    Ok(line_json(e, i))
}

fn draft_index(e: &Engine, cmd: &str, p: &Value) -> Result<usize> {
    let drafts = &e.session().lyric_sheet.drafts;
    if let Some(id) = p.get("draft").and_then(Value::as_u64) {
        return drafts.iter().position(|d| d.id == id).ok_or_else(|| bad(cmd, format!("no draft {id}")));
    }
    let name = str_param(p, "name").or_else(|| str_param(p, "draft")).ok_or_else(|| bad(cmd, "give `draft` (id) or `name`"))?;
    drafts.iter().rposition(|d| d.name == name).ok_or_else(|| bad(cmd, format!("no draft named `{name}`")))
}

fn draft_json(d: &Draft) -> Value {
    json!({"id": d.id, "name": d.name, "created": d.created, "lines": d.lines.len(), "text": d.text()})
}

fn draft_save(e: &mut Engine, p: &Value) -> Result<Value> {
    let n = e.session().lyric_sheet.drafts.len() + 1;
    let name = str_param(p, "name").map(|s| s.trim().chars().filter(|c| !c.is_control()).take(80).collect::<String>()).filter(|s| !s.is_empty());
    let name = name.unwrap_or_else(|| format!("Draft {n}"));
    let id = e.session_mut().alloc();
    let s = e.session_mut();
    let draft = Draft { id, name, created: super::scout::now_rfc3339(), lines: s.lyric_sheet.lines.clone() };
    s.lyric_sheet.drafts.push(draft.clone());
    if s.lyric_sheet.drafts.len() > MAX_DRAFTS {
        s.lyric_sheet.drafts.remove(0);
    }
    Ok(draft_json(&draft))
}

fn draft_restore(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = draft_index(e, "lyrics.draft_restore", p)?;
    let Some(d) = e.session().lyric_sheet.drafts.get(i).cloned() else { return Err(bad("lyrics.draft_restore", "draft gone")) };
    let s = e.session_mut();
    let top = d.lines.iter().map(|l| l.id).max().unwrap_or(0);
    s.next_id = s.next_id.max(top.saturating_add(1));
    s.lyric_sheet.lines = d.lines.clone();
    s.lyric_sheet.tidy_places();
    Ok(draft_json(&d))
}

fn draft_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = draft_index(e, "lyrics.draft_delete", p)?;
    let d = e.session_mut().lyric_sheet.drafts.remove(i);
    Ok(json!({"deleted": d.id, "name": d.name}))
}

fn drafts(e: &mut Engine, _p: &Value) -> Result<Value> {
    Ok(json!({"drafts": e.session().lyric_sheet.drafts.iter().map(draft_json).collect::<Vec<_>>()}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    const SONG: &str = "[Verse 1]\nWhen the night was long\nAnd my heart could not be still";

    #[test]
    fn alternates_switch_and_the_ruler_marker_follows() {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"text": SONG})).unwrap();
        let id = e.session().lyric_sheet.lines[1].id;
        e.execute("lyrics.place", &json!({"line": id, "start": 0, "end": 48_000})).unwrap();
        let v = e.execute("lyrics.alt_add", &json!({"number": 1, "text": "When the day was gone"})).unwrap();
        assert_eq!(v["active"], 1);
        let line = |e: &Engine| e.session().lyric_sheet.lines[1].clone();
        assert_eq!(line(&e).text, "When the day was gone");
        assert_eq!(line(&e).alts, vec!["When the night was long", "When the day was gone"]);
        // The marker is the same line, titled by the active version.
        let sheet = e.execute("lyrics.sheet_get", &json!({})).unwrap();
        assert_eq!(sheet["lines"][1]["text"], "When the day was gone");
        assert!(!sheet["lines"][1]["placed"].is_null());
        assert_eq!(sheet["lines"][1]["alternates"]["count"], 2);
        // Switching wraps; editing the text edits the active version.
        e.execute("lyrics.alt_switch", &json!({"line": id})).unwrap();
        assert_eq!(line(&e).text, "When the night was long");
        let edited = SONG.replace("When the night was long", "When the night was so long");
        e.execute("lyrics.sheet_set", &json!({"text": edited})).unwrap();
        assert_eq!(line(&e).alts, vec!["When the night was so long", "When the day was gone"]);
        assert_eq!(line(&e).id, id);
        // Undo steps back through the switch.
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(line(&e).text, "When the day was gone");
        // Deleting down to one version leaves no alternates.
        e.execute("lyrics.alt_delete", &json!({"line": id})).unwrap();
        assert!(line(&e).alts.is_empty());
        assert_eq!(line(&e).text, "When the night was long");
        assert!(e.execute("lyrics.alt_switch", &json!({"line": id})).is_err());
        assert!(e.execute("lyrics.alt_add", &json!({"number": 99})).is_err());
    }

    #[test]
    fn drafts_save_and_restore_with_undo() {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"text": SONG})).unwrap();
        let d = e.execute("lyrics.draft_save", &json!({"name": "First take"})).unwrap();
        assert_eq!(d["name"], "First take");
        assert_eq!(d["lines"], 3);
        e.execute("lyrics.sheet_set", &json!({"text": "[Verse 1]\nSomething new entirely"})).unwrap();
        e.execute("lyrics.draft_save", &json!({})).unwrap();
        let list = e.execute("lyrics.drafts", &json!({})).unwrap();
        assert_eq!(list["drafts"][1]["name"], "Draft 2");
        e.execute("lyrics.draft_restore", &json!({"name": "First take"})).unwrap();
        assert_eq!(e.session().lyric_sheet.text(), SONG);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().lyric_sheet.text(), "[Verse 1]\nSomething new entirely");
        // Saved with the session.
        let back: soundcraft_model::Session = serde_json::from_str(&serde_json::to_string(e.session()).unwrap()).unwrap();
        assert_eq!(back.lyric_sheet.drafts.len(), 2);
        let id = list["drafts"][0]["id"].as_u64().unwrap();
        e.execute("lyrics.draft_delete", &json!({"draft": id})).unwrap();
        assert_eq!(e.session().lyric_sheet.drafts.len(), 1);
        assert!(e.execute("lyrics.draft_restore", &json!({"name": "nope"})).is_err());
    }
}
