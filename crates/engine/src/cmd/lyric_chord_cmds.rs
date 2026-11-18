//! Chords in the lyric sheet (Window › Lyrics): send a placed line's chords to the Chords ruler,
//! take the ruler's chords into a line (each one undo step; no automatic syncing), export the sheet
//! as text (with or without chords) or ChordPro, and import a text or ChordPro file. See
//! `crate::lyric_chords`.

use super::*;
use crate::cmd;
use crate::lyric_chords;
use serde_json::json;
use soundcraft_model::Session;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "lyrics.chords_to_ruler",
            "Send Chords to the Chords Ruler",
            [],
            None,
            "{line? (id) or number? (default: the placed line at the insertion point)} — lays a placed lyric line's chords ([Am] in its text) on the Chords ruler across the line's time: each at its syllable's time (Transcribe Lyrics' word times when the line has them, else spread by position in the text), on the half-bar grid, through `band.chord.add`; one undo step → {line, chords: [{name, start, end, exact}]}",
            always,
            to_ruler
        ),
        cmd!(
            "lyrics.chords_from_ruler",
            "Take Chords from the Chords Ruler",
            [],
            None,
            "{line? (id) or number?} — writes the Chords ruler's chords under a placed line's range into its text as [Am] at the nearest word starts (its own chords replaced); one undo step → {line, text}",
            always,
            from_ruler
        ),
        cmd!(
            noundo "lyrics.export_sheet_text",
            "Export Lyric Sheet as Text",
            [],
            None,
            "{path} — the lyric sheet as plain text: headings as written ({Verse 1}), the words without their chords",
            has_sheet,
            |e, p| export(e, p, "lyrics.export_sheet_text", Format::Words)
        ),
        cmd!(
            noundo "lyrics.export_sheet_chords",
            "Export Lyric Sheet as Text with Chords",
            [],
            None,
            "{path} — the lyric sheet as plain text with each line's chords on a line above its words, lined up for a monospaced font",
            has_sheet,
            |e, p| export(e, p, "lyrics.export_sheet_chords", Format::WithChords)
        ),
        cmd!(
            noundo "lyrics.export_chordpro",
            "Export Lyric Sheet as ChordPro",
            [],
            None,
            "{path (.cho or .chopro)} — the lyric sheet as ChordPro: {title: the session's name}, each heading a section ({start_of_verse: Verse 1} … {end_of_verse}), chords inline as [Am]",
            has_sheet,
            |e, p| export(e, p, "lyrics.export_chordpro", Format::ChordPro)
        ),
        cmd!(
            "lyrics.import_sheet",
            "Import Lyric Sheet",
            [],
            None,
            "{path} or {text} — replaces the lyric sheet with a text or ChordPro file: ChordPro sections and labels become {headings}, metadata and # comments are left out, old [Verse 1] headings become {Verse 1}, chords written above the words go inline. Lines the sheet already has keep their ids (and Lyrics ruler places); one undo step",
            always,
            import
        ),
    ]
}

fn has_sheet(e: &Engine) -> std::result::Result<(), String> {
    if e.session().lyric_sheet.lines.iter().any(|l| !l.text.trim().is_empty()) {
        Ok(())
    } else {
        Err("the lyric sheet is empty (Window › Lyrics)".into())
    }
}

/// The line `line`/`number` names, else the placed one at the insertion point.
fn line_of(s: &Session, cmd: &str, p: &Value) -> Result<u64> {
    match super::lyric_place::line_param(s, cmd, p)? {
        Some(l) => Ok(l),
        None => s.lyric_sheet.placed_at(s.edit.selection.start).map(|q| q.line).ok_or_else(|| bad(cmd, "give `line` or `number`")),
    }
}

fn to_ruler(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.chords_to_ruler";
    let line = line_of(e.session(), C, p)?;
    let chords = lyric_chords::chords_to_send(e.session(), line).map_err(|m| bad(C, m))?;
    let add = find_command("band.chord.add").ok_or_else(|| bad(C, "the Chords ruler is not available"))?;
    let mut laid = Vec::new();
    // Inside this command, so the whole line is one undo step.
    for (name, start, end) in chords {
        let v = (add.run)(e, &json!({"chord": name, "at": start, "end": end}))?;
        laid.push(json!({"name": name, "start": start, "end": end, "exact": v["exact"]}));
    }
    Ok(json!({"line": line, "chords": laid}))
}

fn from_ruler(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.chords_from_ruler";
    let line = line_of(e.session(), C, p)?;
    let text = lyric_chords::line_with_ruler_chords(e.session(), line).map_err(|m| bad(C, m))?;
    let same = e.session().lyric_sheet.lines.iter().any(|l| l.id == line && l.text == text);
    if !same && let Some(l) = e.session_mut().lyric_sheet.lines.iter_mut().find(|l| l.id == line) {
        l.text.clone_from(&text);
        l.sync_alts();
    }
    Ok(json!({"line": line, "text": text}))
}

#[derive(Clone, Copy)]
enum Format {
    Words,
    WithChords,
    ChordPro,
}

fn export(e: &mut Engine, p: &Value, cmd: &str, f: Format) -> Result<Value> {
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(cmd, "`path` required"))?.to_string();
    let s = e.session();
    let body = match f {
        Format::Words => lyric_chords::sheet_text(&s.lyric_sheet, false),
        Format::WithChords => lyric_chords::sheet_text(&s.lyric_sheet, true),
        Format::ChordPro => lyric_chords::chordpro(&s.lyric_sheet, &s.name),
    };
    crate::io::write_file(&path, body.as_bytes())?;
    Ok(json!({"path": path, "lines": body.lines().count(), "bytes": body.len()}))
}

/// Largest file Import Lyric Sheet reads.
const MAX_IMPORT_BYTES: u64 = 1 << 20;

fn import(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.import_sheet";
    let text = match (str_param(p, "text"), str_param(p, "path").filter(|s| !s.trim().is_empty())) {
        (Some(t), _) => t.to_string(),
        (None, Some(path)) => String::from_utf8_lossy(&crate::io::read_file_max(path, MAX_IMPORT_BYTES)?).into_owned(),
        (None, None) => return Err(bad(C, "give `path` or `text`")),
    };
    let lines = lyric_chords::import_lines(&text);
    let set = find_command("lyrics.sheet_set").ok_or_else(|| bad(C, "the lyric sheet is not available"))?;
    (set.run)(e, &json!({"text": lines.join("\n")}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    /// A session at 120 bpm 4/4, 48 kHz: a bar is 96 000 samples, half a bar 48 000.
    fn engine(text: &str) -> Engine {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"text": text})).unwrap();
        e
    }

    fn ruler(e: &Engine) -> Vec<(String, i64, i64)> {
        let s = e.session();
        let mut v: Vec<(String, i64, i64)> = s
            .band
            .chords
            .iter()
            .map(|c| {
                let name =
                    soundcraft_band::chord_theory::name_for(i32::from(c.slot), i32::from(s.band.key), i32::from(s.band.scale), i32::from(c.bass));
                (name, s.tempo.tick_to_samples(c.tick, s.sample_rate), s.tempo.tick_to_samples(c.tick + c.length_ticks, s.sample_rate))
            })
            .collect();
        v.sort_by_key(|c| c.1);
        v
    }

    #[test]
    fn send_chords_lays_them_by_text_position_in_one_undo_step() {
        let mut e = engine("{Verse 1}\n[C]When the night [G]was long\nNo chords here");
        let id = e.session().lyric_sheet.line_numbered(1).unwrap().id;
        // Not placed: refused with the reason.
        let err = e.execute("lyrics.chords_to_ruler", &json!({"line": id})).unwrap_err().to_string();
        assert!(err.contains("place the line"), "{err}");
        // Placed over two bars: "[G]" stands at character 15 of 22 → 1.36 bars, nearest half bar
        // 1.5 bars.
        let bar = 96_000;
        e.execute("lyrics.place", &json!({"line": id, "start": 0, "end": 2 * bar})).unwrap();
        let steps = e.undo_history().len();
        let v = e.execute("lyrics.chords_to_ruler", &json!({"line": id})).unwrap();
        assert_eq!(v["chords"].as_array().unwrap().len(), 2, "{v}");
        assert_eq!(ruler(&e), vec![("C".into(), 0, 3 * bar / 2), ("G".into(), 3 * bar / 2, 2 * bar)]);
        assert_eq!(e.undo_history().len(), steps + 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().band.chords.is_empty());
        // A line with no chords, a heading: refused.
        let plain = e.session().lyric_sheet.line_numbered(2).unwrap().id;
        e.execute("lyrics.place", &json!({"line": plain, "start": 2 * bar, "end": 4 * bar})).unwrap();
        assert!(e.execute("lyrics.chords_to_ruler", &json!({"line": plain})).is_err());
        let heading = e.session().lyric_sheet.lines[0].id;
        assert!(e.execute("lyrics.chords_to_ruler", &json!({"line": heading})).is_err());
    }

    #[test]
    fn send_chords_follows_transcribed_word_times() {
        use crate::lyrics::TimedWord;
        let mut e = engine("[C]When the night was [G]long");
        let rate = e.session().sample_rate;
        let t = {
            let s = e.session_mut();
            let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some("Vox"));
            let id = s.new_clip_id();
            if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
                pl.clips.push(soundcraft_model::Clip::midi(id, "Vox", 0, rate.samples(20.0), Default::default()));
            }
            t
        };
        // "long" sung late: at 3.0 s of a 0..4 s line (by text alone it would be 2.7 s).
        let tw = |a: f64, s: &str| TimedWord { start: a, end: a + 0.4, text: s.into() };
        let words = vec![tw(0.0, "When"), tw(0.5, "the"), tw(1.0, "night"), tw(1.5, "was"), tw(3.0, "long")];
        crate::lyrics::place_lyrics(e.session_mut(), t, 0, rate.samples(20.0), &[words]);
        let id = e.session().lyric_sheet.lines[0].id;
        e.execute("lyrics.place", &json!({"line": id, "start": 0, "end": rate.samples(4.0)})).unwrap();
        e.execute("lyrics.chords_to_ruler", &json!({"line": id})).unwrap();
        // 3.0 s is 1.5 bars at 120 bpm: on the grid exactly.
        assert_eq!(ruler(&e)[1], ("G".into(), rate.samples(3.0), rate.samples(4.0)));
    }

    #[test]
    fn take_chords_writes_them_at_word_starts_in_one_undo_step() {
        let bar = 96_000;
        let mut e = engine("[D]When the night was long\nAnd my heart");
        let id = e.session().lyric_sheet.line_numbered(1).unwrap().id;
        assert!(e.execute("lyrics.chords_from_ruler", &json!({"line": id})).unwrap_err().to_string().contains("place the line"));
        e.execute("lyrics.place", &json!({"line": id, "start": 0, "end": 2 * bar})).unwrap();
        assert!(e.execute("lyrics.chords_from_ruler", &json!({"line": id})).unwrap_err().to_string().contains("no chord"));
        // Am from the line's start, then F at one bar (half the text: the word nearest is "night").
        e.execute("band.chord.add", &json!({"chord": "Am", "at": 0, "end": bar})).unwrap();
        e.execute("band.chord.add", &json!({"chord": "F", "at": bar, "end": 3 * bar})).unwrap();
        let steps = e.undo_history().len();
        let v = e.execute("lyrics.chords_from_ruler", &json!({"line": id})).unwrap();
        assert_eq!(v["text"], "[Am]When the [F]night was long");
        assert_eq!(e.session().lyric_sheet.lines[0].text, "[Am]When the [F]night was long");
        assert_eq!(e.undo_history().len(), steps + 1);
        // The other line is untouched; undo brings the old chord back.
        assert_eq!(e.session().lyric_sheet.lines[1].text, "And my heart");
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().lyric_sheet.lines[0].text, "[D]When the night was long");
        // Round trip: send what was taken, take it back: the same line.
        e.execute("lyrics.chords_from_ruler", &json!({"line": id})).unwrap();
        e.execute("band.chords.delete", &json!({"all": true})).unwrap();
        e.execute("lyrics.chords_to_ruler", &json!({"line": id})).unwrap();
        e.execute("lyrics.chords_from_ruler", &json!({"line": id})).unwrap();
        assert_eq!(e.session().lyric_sheet.lines[0].text, "[Am]When the [F]night was long");
    }

    #[test]
    fn exports_and_import_round_trip() {
        let dir = std::env::temp_dir().join(format!("fdawm-lyric-chords-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sheet = "{Verse 1}\n[Am]Hello dark[G]ness\n[F] [C]\n\n{Chorus}\nSing";
        let mut e = engine(sheet);
        e.session_mut().name = "Song".into();
        let p = |n: &str| dir.join(n).to_string_lossy().to_string();
        e.execute("lyrics.export_sheet_text", &json!({"path": p("a.txt")})).unwrap();
        e.execute("lyrics.export_sheet_chords", &json!({"path": p("b.txt")})).unwrap();
        e.execute("lyrics.export_chordpro", &json!({"path": p("c.cho")})).unwrap();
        let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap();
        assert_eq!(read("a.txt"), "{Verse 1}\nHello darkness\n\n{Chorus}\nSing\n");
        assert_eq!(read("b.txt"), "{Verse 1}\nAm        G\nHello darkness\nF  C\n\n{Chorus}\nSing\n");
        assert!(read("c.cho").starts_with("{title: Song}\n{start_of_verse: Verse 1}\n[Am]Hello dark[G]ness\n[F] [C]\n{end_of_verse}\n"));
        // Each file read back gives the sheet (the words-only one without its chords); ids kept.
        let ids: Vec<u64> = e.session().lyric_sheet.lines.iter().map(|l| l.id).collect();
        for (f, want) in [("c.cho", sheet), ("b.txt", sheet), ("a.txt", "{Verse 1}\nHello darkness\n\n{Chorus}\nSing")] {
            let steps = e.undo_history().len();
            e.execute("lyrics.import_sheet", &json!({"path": p(f)})).unwrap();
            assert_eq!(e.session().lyric_sheet.text(), want, "{f}");
            if want == sheet {
                // The same sheet back: nothing changed, no undo step, the same ids.
                assert_eq!(e.undo_history().len(), steps, "{f}");
            } else {
                e.execute("edit.undo", &json!({})).unwrap();
            }
            assert_eq!(e.session().lyric_sheet.lines.iter().map(|l| l.id).collect::<Vec<_>>(), ids);
        }
        // An old sheet's text: square headings come in curly.
        e.execute("lyrics.import_sheet", &json!({"text": "[Verse 1]\nla"})).unwrap();
        assert_eq!(e.session().lyric_sheet.text(), "{Verse 1}\nla");
        assert!(e.execute("lyrics.import_sheet", &json!({})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
