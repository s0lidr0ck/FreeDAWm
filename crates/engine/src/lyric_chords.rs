//! Chords in the lyric sheet (Window › Transport & Session › Lyrics): `[Am]` inline before the syllable it falls on,
//! headings `{Verse 1}` ([`soundcraft_model::sheet_chords`]). Here: the text and ChordPro exports,
//! reading a text or ChordPro file into a sheet, and the two moves between a line placed on the
//! Lyrics ruler and the Chords ruler (sending the line's chords there, taking the ruler's chords
//! into the line). There is no automatic syncing: the Chords ruler stays what the Band plays.

use crate::lyric_tools::{section_kind, word_spans};
use soundcraft_model::{
    LineKind, LyricPlace, LyricSheet, MAX_SHEET_LINE_CHARS, Session, heading_name, is_chord_name, line_kinds, modern_line, sheet_texts, strip_chords,
};
use soundcraft_time::Samples;

// ---- text exports -----------------------------------------------------------------------------

/// A line with chords as two monospaced lines: the chord names, each over the character it
/// stood before, and the words. `None` for a line without chords.
pub fn chords_over_words(line: &str) -> Option<(String, String)> {
    let st = strip_chords(line);
    if st.chords.is_empty() {
        return None;
    }
    let mut row = String::new();
    let mut used = 0usize;
    for (name, at) in &st.chords {
        let col = st.text.get(..*at).map_or(0, |p| p.chars().count());
        // Two chords over one word: the second moves right, one space after the first.
        let col = if used == 0 { col } else { col.max(used + 1) };
        row.extend(std::iter::repeat_n(' ', col.saturating_sub(used)));
        row.push_str(name);
        used = col.max(used) + name.chars().count();
    }
    Some((row, st.text.trim_end().to_string()))
}

/// The sheet as plain text, one line per line, headings as written (`{Verse 1}`). Without
/// `with_chords` the chords are left out (and lines of only chords with them); with it each line's
/// chords stand on a line of their own above its words, lined up for a monospaced font.
pub fn sheet_text(sheet: &LyricSheet, with_chords: bool) -> String {
    let texts: Vec<&str> = sheet.lines.iter().map(|l| l.text.as_str()).collect();
    let mut out = String::new();
    for (text, kind) in texts.iter().zip(line_kinds(texts.iter().copied())) {
        match kind {
            LineKind::Chords if !with_chords => continue,
            LineKind::Chords => {
                let names: Vec<String> = strip_chords(text).chords.into_iter().map(|c| c.0).collect();
                out.push_str(&names.join("  "));
            }
            LineKind::Lyric(_) => match chords_over_words(text) {
                Some((row, words)) if with_chords => {
                    out.push_str(&row);
                    out.push('\n');
                    out.push_str(&words);
                }
                _ => out.push_str(strip_chords(text).text.trim_end()),
            },
            LineKind::Heading | LineKind::Blank => out.push_str(text.trim_end()),
        }
        out.push('\n');
    }
    out
}

/// The ChordPro section a heading opens: verse, chorus, bridge, or the heading's kind as a
/// ChordPro 6 custom section (`start_of_intro`).
fn section_env(name: &str) -> String {
    let kind = section_kind(name);
    match kind.as_str() {
        "verse" => "verse".into(),
        "chorus" | "refrain" => "chorus".into(),
        "bridge" => "bridge".into(),
        _ => {
            let slug: String = kind.chars().filter(char::is_ascii_alphanumeric).collect();
            if slug.is_empty() { "verse".into() } else { slug }
        }
    }
}

/// Text safe inside a ChordPro directive's value (no braces or line breaks).
fn directive_value(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '{' | '}' | '\n' | '\r') { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// The sheet as ChordPro (`.cho`): `{title: }`, each heading a section (`{start_of_verse: Verse 1}`
/// … `{end_of_verse}`), the lines with their chords inline as they are written in the sheet.
pub fn chordpro(sheet: &LyricSheet, title: &str) -> String {
    let texts: Vec<&str> = sheet.lines.iter().map(|l| l.text.as_str()).collect();
    let mut out = String::new();
    let title = directive_value(title);
    if !title.is_empty() {
        out.push_str(&format!("{{title: {title}}}\n"));
    }
    let mut open: Option<String> = None;
    let mut blanks = 0usize;
    // Whether a section or a line has been written (a heading after one gets a blank line before it).
    let mut body = false;
    for (text, kind) in texts.iter().zip(line_kinds(texts.iter().copied())) {
        match kind {
            LineKind::Blank => blanks += 1,
            LineKind::Heading => {
                if let Some(env) = open.take() {
                    out.push_str(&format!("{{end_of_{env}}}\n"));
                }
                out.extend(std::iter::repeat_n('\n', blanks.max(usize::from(body))));
                body = true;
                blanks = 0;
                let name = directive_value(heading_name(text).unwrap_or(""));
                let env = section_env(&name);
                if name.is_empty() {
                    out.push_str(&format!("{{start_of_{env}}}\n"));
                } else {
                    out.push_str(&format!("{{start_of_{env}: {name}}}\n"));
                }
                open = Some(env);
            }
            LineKind::Lyric(_) | LineKind::Chords => {
                body = true;
                out.extend(std::iter::repeat_n('\n', blanks));
                blanks = 0;
                out.push_str(text.trim_end());
                out.push('\n');
            }
        }
    }
    if let Some(env) = open {
        out.push_str(&format!("{{end_of_{env}}}\n"));
    }
    out
}

// ---- import ---------------------------------------------------------------------------------

/// ChordPro directives without a value that carry nothing the sheet keeps (page layout). With a
/// value (`{key: G}`, `{title: …}`, `{textfont: …}`) any directive other than a section or a
/// label is left out.
const DROPPED_DIRECTIVES: &[&str] = &["new_page", "np", "new_physical_page", "npp", "column_break", "colb", "new_song", "ns", "no_grid", "ng"];

/// A ChordPro directive line as the sheet keeps it: `Some(Some(heading))`, `Some(None)` to drop
/// the line, `None` when the line is no directive (a `{Verse 1}` heading stays as it is).
fn directive(line: &str) -> Option<Option<String>> {
    let inner = heading_name(line)?;
    let (name, value) = match inner.split_once(':') {
        Some((n, v)) => (n.trim().to_ascii_lowercase(), v.trim()),
        None => (inner.trim().to_ascii_lowercase(), ""),
    };
    let heading = |label: &str| Some(Some(format!("{{{label}}}")));
    let titled = |s: &str| -> String {
        let s = s.replace('_', " ");
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    };
    let short = match name.as_str() {
        "soc" => Some("chorus"),
        "sov" => Some("verse"),
        "sob" => Some("bridge"),
        "sot" => Some("tab"),
        "sog" => Some("grid"),
        _ => None,
    };
    if let Some(env) = name.strip_prefix("start_of_").or(short) {
        return heading(if value.is_empty() { titled(env) } else { value.to_string() }.as_str());
    }
    if name.starts_with("end_of_") || matches!(name.as_str(), "eoc" | "eov" | "eob" | "eot" | "eog") {
        return Some(None);
    }
    if name == "chorus" {
        return heading(if value.is_empty() { "Chorus" } else { value });
    }
    if matches!(name.as_str(), "comment" | "c" | "comment_italic" | "ci" | "comment_box" | "cb" | "highlight") {
        // A bare `{Comment}` is a heading of that name.
        return if value.is_empty() { None } else { heading(value) };
    }
    let word = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if DROPPED_DIRECTIVES.contains(&name.as_str()) || (word && inner.contains(':')) {
        return Some(None);
    }
    None
}

/// A line of bare chord names (`Am      G/B   C`), as a chords-above-words text writes them:
/// the names and their columns. A lone one-letter `A` is read as a word.
fn chord_row(line: &str) -> Option<Vec<(usize, String)>> {
    if line.contains('[') || line.contains('{') {
        return None;
    }
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut word = String::new();
    for (col, c) in line.chars().chain(std::iter::once(' ')).enumerate() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                if !is_chord_name(&word) {
                    return None;
                }
                out.push((s, std::mem::take(&mut word)));
            }
        } else {
            start.get_or_insert(col);
            word.push(c);
        }
    }
    match out.as_slice() {
        [] => None,
        [(_, w)] if w == "A" => None,
        _ => Some(out),
    }
}

/// `words` with each chord written in before the character at its column.
fn merge_row(row: &[(usize, String)], words: &str) -> String {
    let mut chars: Vec<String> = words.chars().map(String::from).collect();
    for (col, name) in row.iter().rev() {
        while chars.len() < *col {
            chars.push(" ".into());
        }
        chars.insert(*col, format!("[{name}]"));
    }
    chars.concat()
}

/// Text read as sheet lines: ChordPro (directives to headings, `# ` comments left out), old
/// square-bracket headings (`[Verse 1]`) written curly, and a chords-above-words layout written
/// back inline (`[Am]Hello`). Control characters, line length and count capped as typed text is.
pub fn import_lines(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let lines = sheet_texts(text);
    // A ChordPro file: some directive is not one of the sheet's own headings.
    let is_chordpro = lines.iter().any(|l| directive(l).is_some_and(|d| d.as_deref() != Some(l.trim())));
    let mut kept: Vec<String> = Vec::new();
    for l in lines {
        if is_chordpro && l.trim_start().starts_with('#') {
            continue;
        }
        match directive(&l) {
            Some(Some(h)) => kept.push(h),
            Some(None) => {}
            None => kept.push(modern_line(l)),
        }
    }
    let mut out: Vec<String> = Vec::with_capacity(kept.len());
    let mut i = 0;
    while let Some(l) = kept.get(i) {
        if let Some(row) = chord_row(l) {
            let next = kept.get(i + 1).filter(|n| !n.trim().is_empty() && heading_name(n).is_none() && chord_row(n).is_none() && !n.contains('['));
            match next {
                Some(words) => {
                    out.push(merge_row(&row, words));
                    i += 2;
                    continue;
                }
                None => out.push(row.iter().map(|(_, n)| format!("[{n}]")).collect::<Vec<_>>().join(" ")),
            }
        } else {
            out.push(l.clone());
        }
        i += 1;
    }
    // The file's last line break is no blank line of the sheet.
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    // Capped again: a merged line is longer than its words.
    out.into_iter().map(|l| sheet_texts(&l).into_iter().next().unwrap_or_default()).collect()
}

// ---- the Chords ruler ------------------------------------------------------------------------

/// A word as matched against transcribed words: lower-case letters and digits.
fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Where in time a placed line's text falls: (character position in the sung text, time), from
/// the line's start and end and the times Transcribe Lyrics found for its words there, if any.
fn anchors(s: &Session, sung: &str, place: LyricPlace) -> Vec<(f64, Samples)> {
    let len = sung.trim_end().chars().count().max(1) as f64;
    let mut out = vec![(0.0, place.start)];
    let heard: Vec<(String, Samples)> = crate::lyrics::lyrics_track(s, None)
        .and_then(|t| s.track(t))
        .map(|t| {
            soundcraft_model::track_lines(t)
                .into_iter()
                .flat_map(|l| l.words)
                .filter(|w| w.start >= place.start && w.start < place.end)
                .map(|w| (norm(&w.text), w.start))
                .collect()
        })
        .unwrap_or_default();
    if !heard.is_empty() {
        let mut j = 0;
        for (a, b) in word_spans(sung) {
            let w = norm(sung.get(a..b).unwrap_or(""));
            let Some(k) = heard.iter().skip(j).take(3).position(|h| h.0 == w) else { continue };
            let Some(&(_, at)) = heard.get(j + k) else { continue };
            let col = sung.get(..a).map_or(0, |p| p.chars().count()) as f64;
            if out.last().is_some_and(|l| col > l.0 && at > l.1) && at < place.end {
                out.push((col, at));
            }
            j += k + 1;
        }
    }
    if out.last().is_some_and(|l| len > l.0) {
        out.push((len, place.end));
    }
    out
}

/// The time of character position `col` (piecewise linear between anchors).
fn time_at(anchors: &[(f64, Samples)], col: f64) -> Samples {
    for w in anchors.windows(2) {
        let (a, b) = (w[0], w[1]);
        if col <= b.0 {
            let k = ((col - a.0) / (b.0 - a.0).max(1e-9)).clamp(0.0, 1.0);
            return a.1 + ((b.1 - a.1) as f64 * k).round() as Samples;
        }
    }
    anchors.last().map_or(0, |a| a.1)
}

/// The character position at time `t` (piecewise linear between anchors).
fn col_at(anchors: &[(f64, Samples)], t: Samples) -> f64 {
    for w in anchors.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.1 {
            let k = ((t - a.1) as f64 / (b.1 - a.1).max(1) as f64).clamp(0.0, 1.0);
            return a.0 + (b.0 - a.0) * k;
        }
    }
    anchors.last().map_or(0.0, |a| a.0)
}

/// The nearest half bar to `at` (the Chords ruler's grid), as a sample position.
fn on_grid(s: &Session, at: Samples) -> Samples {
    let half = soundcraft_mix::half_ticks(s).max(1);
    let t = s.tempo.samples_to_ticks(at.max(0), s.sample_rate).max(0);
    let snapped = t.saturating_add(half / 2).div_euclid(half).saturating_mul(half);
    s.tempo.tick_to_samples(snapped, s.sample_rate)
}

/// Why a line's chords cannot go to (or come from) the Chords ruler, if they cannot.
pub fn ruler_refusal(s: &Session, line: u64) -> Option<&'static str> {
    let sheet = &s.lyric_sheet;
    if sheet.number_of(line).is_none() {
        return Some("only a lyric line has chords to move");
    }
    if sheet.place_of(line).is_none() {
        return Some("place the line on the Lyrics ruler first (drag its number there)");
    }
    None
}

/// The chords a placed line sends to the Chords ruler: (name, start, end) in samples on the
/// ruler's half-bar grid, each at the time of the syllable it stands before (the transcribed
/// words' times when the line has them, else spread by position in the text), ending at the next
/// chord or the line's end. Two chords on one grid step keep the first.
pub fn chords_to_send(s: &Session, line: u64) -> Result<Vec<(String, Samples, Samples)>, String> {
    if let Some(why) = ruler_refusal(s, line) {
        return Err(why.into());
    }
    let sheet = &s.lyric_sheet;
    let place = sheet.place_of(line).ok_or("not placed")?;
    let text = sheet.lines.iter().find(|l| l.id == line).map_or("", |l| l.text.as_str());
    let st = strip_chords(text);
    if st.chords.is_empty() {
        return Err("the line has no chords: write them in it like [Am]".into());
    }
    let marks = anchors(s, &st.text, place);
    let mut at: Vec<(String, Samples)> = Vec::new();
    for (name, pos) in &st.chords {
        let col = st.text.get(..*pos).map_or(0, |p| p.chars().count()) as f64;
        let t = on_grid(s, time_at(&marks, col));
        if at.last().is_none_or(|l| t > l.1) {
            at.push((name.clone(), t));
        }
    }
    let end = on_grid(s, place.end);
    let mut out = Vec::new();
    for (i, (name, a)) in at.iter().enumerate() {
        let b = at.get(i + 1).map_or(end, |n| n.1);
        // The last chord holds to the line's end; one past it still gets its half bar.
        out.push((name.clone(), *a, if b > *a { b } else { *a + 1 }));
    }
    Ok(out)
}

/// The line `line` with the Chords ruler's chords under its range written in (each at the word
/// nearest its time; the line's own chords replaced, any other bracket text kept).
pub fn line_with_ruler_chords(s: &Session, line: u64) -> Result<String, String> {
    if let Some(why) = ruler_refusal(s, line) {
        return Err(why.into());
    }
    let sheet = &s.lyric_sheet;
    let place = sheet.place_of(line).ok_or("not placed")?;
    let text = sheet.lines.iter().find(|l| l.id == line).map_or("", |l| l.text.as_str());
    let st = strip_chords(text);
    let (key, scale) = (i32::from(s.band.key), i32::from(s.band.scale));
    let tick = |x: Samples| s.tempo.samples_to_ticks(x.max(0), s.sample_rate);
    let (a, b) = (tick(place.start), tick(place.end));
    let mut chords: Vec<_> = s.band.chords.iter().filter(|c| c.length_ticks > 0 && c.tick < b && c.tick.saturating_add(c.length_ticks) > a).collect();
    chords.sort_by_key(|c| c.tick);
    if chords.is_empty() {
        return Err("no chord on the Chords ruler under this line".into());
    }
    let marks = anchors(s, &st.text, place);
    // Word starts as (character, byte) positions; a line without words takes its chords at 0.
    let starts: Vec<(usize, usize)> = word_spans(&st.text).into_iter().map(|(x, _)| (st.text.get(..x).map_or(0, |p| p.chars().count()), x)).collect();
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for c in chords {
        let at = s.tempo.tick_to_samples(c.tick.max(a), s.sample_rate);
        let col = col_at(&marks, at);
        let byte = starts.iter().min_by(|p, q| (p.0 as f64 - col).abs().total_cmp(&(q.0 as f64 - col).abs())).map_or(0, |p| p.1);
        let name = soundcraft_band::chord_theory::name_for(i32::from(c.slot), key, scale, i32::from(c.bass));
        inserts.push((byte, name));
    }
    let mut out = String::with_capacity(st.text.len() + inserts.len() * 4);
    let mut from = 0;
    for (byte, name) in &inserts {
        out.push_str(st.text.get(from..*byte).unwrap_or(""));
        out.push('[');
        out.push_str(name);
        out.push(']');
        from = (*byte).max(from);
    }
    out.push_str(st.text.get(from..).unwrap_or(""));
    if out.chars().count() > MAX_SHEET_LINE_CHARS {
        return Err(format!("the line would be longer than {MAX_SHEET_LINE_CHARS} characters with these chords"));
    }
    Ok(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(texts: &[&str]) -> LyricSheet {
        LyricSheet {
            lines: texts.iter().enumerate().map(|(i, t)| soundcraft_model::SheetLine::new(10 + i as u64, *t)).collect(),
            ..LyricSheet::default()
        }
    }

    #[test]
    fn the_sheet_reads_chord_names_as_the_band_does() {
        // Every root, accidental, quality the Band knows (and some it does not) and bass: the
        // sheet's reader agrees with the Band's, a capital root aside.
        let qualities = [
            "",
            "M",
            "m",
            "min",
            "-",
            "mi",
            "maj",
            "major",
            "dim",
            "o",
            "dim7",
            "o7",
            "m7b5",
            "min7b5",
            "-7b5",
            "mi7b5",
            "h",
            "h7",
            "aug",
            "+",
            "7",
            "dom7",
            "maj7",
            "ma7",
            "j7",
            "M7",
            "^",
            "^7",
            "m7",
            "min7",
            "-7",
            "mi7",
            "sus",
            "sus4",
            "sus2",
            "7sus4",
            "7sus",
            "6",
            "maj6",
            "M6",
            "m6",
            "min6",
            "-6",
            "add9",
            "add2",
            "2",
            "Madd9",
            "madd9",
            "m(add9)",
            "minadd9",
            "9",
            "M9",
            "^9",
            "m9",
            "min9",
            "-9",
            "maj9",
            "mmaj7",
            "m(maj7)",
            "minmaj7",
            "mM7",
            "m^7",
            "-^7",
            "MAJ7",
            "Sus4",
            "x",
            "m7x",
            "11",
            "13",
            "7b9",
            "add11",
            "mm",
            "(",
            "\u{0394}7",
            "\u{00B0}",
            "\u{00F8}7",
        ];
        let mut n = 0;
        for root in ["A", "B", "C", "D", "E", "F", "G", "H"] {
            for acc in ["", "#", "b", "bb", "##", "#b", "\u{266F}", "\u{266D}"] {
                for q in qualities {
                    for bass in ["", "/E", "/Bb", "/f#", "/", "/H", "/Ebb", "/E7"] {
                        let name = format!("{root}{acc}{q}{bass}");
                        let band = soundcraft_band::chord_match::parse_chord_name(&name).is_some();
                        assert_eq!(is_chord_name(&name), band, "{name}");
                        // A lower-case root: the Band reads it, the sheet keeps it as words.
                        assert!(!is_chord_name(&name.to_lowercase()) || name.to_lowercase() == name, "{name}");
                        n += 1;
                    }
                }
            }
        }
        assert!(n > 30_000);
    }

    #[test]
    fn text_exports_with_and_without_chords() {
        let s = sheet(&["{Verse 1}", "[Am]Hello dark[G]ness my old [C]friend", "[F]  [G]", "", "{Chorus}", "No chords [here]"]);
        assert_eq!(sheet_text(&s, false), "{Verse 1}\nHello darkness my old friend\n\n{Chorus}\nNo chords [here]\n");
        assert_eq!(sheet_text(&s, true), "{Verse 1}\nAm        G           C\nHello darkness my old friend\nF  G\n\n{Chorus}\nNo chords [here]\n");
        // Chords over one word step apart; a chord at the end stands past the words.
        assert_eq!(chords_over_words("[Am][G7]la la [C]"), Some(("Am G7 C".into(), "la la".into())));
        assert_eq!(chords_over_words("plain"), None);
    }

    #[test]
    fn chordpro_export_has_title_sections_and_inline_chords() {
        let s = sheet(&["{Intro}", "[Am] [G]", "", "{Verse 1}", "[Am]Hello dark[G]ness", "", "{Chorus}", "Sing", "{Pre-Chorus 2}", "la"]);
        let cho = chordpro(&s, "Sound {of} Silence");
        assert_eq!(
            cho,
            "{title: Sound  of  Silence}\n{start_of_intro: Intro}\n[Am] [G]\n{end_of_intro}\n\n{start_of_verse: Verse 1}\n[Am]Hello dark[G]ness\n{end_of_verse}\n\n{start_of_chorus: Chorus}\nSing\n{end_of_chorus}\n\n{start_of_prechorus: Pre-Chorus 2}\nla\n{end_of_prechorus}\n"
        );
        // Read back: the same sheet (sections as headings, blank lines kept).
        let back = import_lines(&cho);
        assert_eq!(back, vec!["{Intro}", "[Am] [G]", "", "{Verse 1}", "[Am]Hello dark[G]ness", "", "{Chorus}", "Sing", "", "{Pre-Chorus 2}", "la"]);
    }

    #[test]
    fn import_reads_chordpro_old_headings_and_chords_above_words() {
        let cho = "# a comment\n{title: Song}\n{key: G}\n{soc}\n[G]Glory\n{eoc}\n{c: Bridge}\n{start_of_verse}\nla\n{chorus}\n{x_app: 1}";
        assert_eq!(import_lines(cho), vec!["{Chorus}", "[G]Glory", "{Bridge}", "{Verse}", "la", "{Chorus}"]);
        // Plain text from before: square-bracket headings become curly; chord lines stay.
        assert_eq!(import_lines("[Verse 1]\n[Am] [G]\nWhen the night\n#1 hit"), vec!["{Verse 1}", "[Am] [G]", "When the night", "#1 hit"]);
        // Chords above words come back inline; a row with nothing under it becomes a chord line.
        let t = sheet_text(&sheet(&["[Am]Hello dark[G]ness my old [C]friend", "[F]  [G]"]), true);
        assert_eq!(import_lines(&t), vec!["[Am]Hello dark[G]ness my old [C]friend", "[F] [G]"]);
        // A lone "A" is a word; a lyric line is never taken for chords.
        assert_eq!(import_lines("A\nman"), vec!["A", "man"]);
        assert_eq!(import_lines("Be Good\nto me"), vec!["Be Good", "to me"]);
    }

    #[test]
    fn hostile_import_never_panics_and_stays_capped() {
        let long = format!("{}\n{}", "Am ".repeat(300), "x".repeat(500));
        for t in ["", "{", "}", "{}", "{:}", "{start_of_}", "[[[", "]]]", "\u{0}\u{1}", "{c:}", "\u{FEFF}{t: x}", long.as_str()] {
            for l in import_lines(t) {
                assert!(l.chars().count() <= MAX_SHEET_LINE_CHARS);
            }
        }
        let many = "la\n".repeat(soundcraft_model::MAX_SHEET_LINES * 2);
        assert!(import_lines(&many).len() <= soundcraft_model::MAX_SHEET_LINES);
    }
}
