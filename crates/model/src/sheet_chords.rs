//! Chords written inline in the lyric sheet, ChordPro style: `[Am]Hello dark[G]ness my old
//! [C]friend`. A chord sits in square brackets just before the syllable it falls on. Section
//! headings are a line that is only curly brackets (`{Verse 1}`), see [`crate::heading_name`].
//!
//! What counts as a chord name is the Band's vocabulary (`soundcraft_band::chord_match::
//! parse_chord_name`: a root A–G with sharps or flats, a quality the Chords ruler accepts, an
//! optional slash bass), mirrored here because the model does not depend on the band crate; the
//! engine's tests check the two agree. One difference: the root must be a capital letter, so a
//! bracketed lower-case word (`[a]`, `[be]`) stays words. A `[text]` that is not a chord name is
//! left in the line as literal text (and flagged gently by the Lyrics tab).

/// Longest text read as a chord name inside brackets, characters.
pub const MAX_CHORD_CHARS: usize = 16;

/// A bracketed piece of a sheet line: `[Am]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineChord {
    /// Byte range of the whole `[..]`, brackets included, in the line.
    pub start: usize,
    pub end: usize,
    /// The text inside, trimmed ("Am").
    pub name: String,
    /// Whether it is a chord name; when not, the bracket text is literal words.
    pub known: bool,
}

/// ♯ ♭ Δ ∆ ° ø as the ASCII the Band reads: # b ^ ^ o h.
fn ascii_tokens(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{266F}' => '#',
            '\u{266D}' => 'b',
            '\u{0394}' | '\u{2206}' => '^',
            '\u{00B0}' => 'o',
            '\u{00F8}' => 'h',
            c => c,
        })
        .collect()
}

/// The chord qualities the Band accepts (`quality_tones` in the band crate).
fn known_quality(q: &str) -> bool {
    if matches!(q, "M" | "" | "M7" | "^" | "^7" | "M9" | "^9" | "M6" | "Madd9" | "Madd2" | "mM7" | "-^7" | "m^7") {
        return true;
    }
    matches!(
        q.to_ascii_lowercase().as_str(),
        "maj"
            | "major"
            | "m"
            | "min"
            | "-"
            | "mi"
            | "dim"
            | "o"
            | "dim7"
            | "o7"
            | "m7b5"
            | "min7b5"
            | "-7b5"
            | "mi7b5"
            | "h"
            | "h7"
            | "aug"
            | "+"
            | "7"
            | "dom7"
            | "maj7"
            | "ma7"
            | "j7"
            | "m7"
            | "min7"
            | "-7"
            | "mi7"
            | "sus"
            | "sus4"
            | "sus2"
            | "7sus4"
            | "7sus"
            | "6"
            | "maj6"
            | "m6"
            | "min6"
            | "-6"
            | "add9"
            | "add2"
            | "2"
            | "madd9"
            | "m(add9)"
            | "minadd9"
            | "9"
            | "m9"
            | "min9"
            | "-9"
            | "maj9"
            | "mmaj7"
            | "m(maj7)"
            | "minmaj7"
    )
}

/// A note name at the start of `s` ("C", "F#", "Bb"): the bytes it takes, none when it is not one.
/// `upper`: the letter must be a capital.
fn note_len(s: &[u8], upper: bool) -> Option<usize> {
    let first = *s.first()?;
    let letter = if upper { first } else { first.to_ascii_uppercase() };
    if !(b'A'..=b'G').contains(&letter) {
        return None;
    }
    let (mut n, mut sharp, mut flat) = (1, false, false);
    while let Some(&c) = s.get(n) {
        match c {
            b'#' => sharp = true,
            b'b' => flat = true,
            _ => break,
        }
        if sharp && flat {
            return None;
        }
        n += 1;
    }
    Some(n)
}

/// Whether `text` is a chord name the Band reads ("Am", "G/B", "C#m7b5", "Bbmaj7", "F♯m"), with a
/// capital root.
pub fn is_chord_name(text: &str) -> bool {
    let t = ascii_tokens(text.trim());
    if t.is_empty() || t.chars().count() > MAX_CHORD_CHARS || t.contains(char::is_whitespace) {
        return false;
    }
    let b = t.as_bytes();
    let Some(root) = note_len(b, true) else { return false };
    let rest = t.get(root..).unwrap_or("");
    let (quality, bass) = match rest.find('/') {
        Some(i) => (rest.get(..i).unwrap_or(""), Some(rest.get(i + 1..).unwrap_or(""))),
        None => (rest, None),
    };
    if !known_quality(quality) {
        return false;
    }
    match bass {
        None => true,
        Some(bass) => note_len(bass.as_bytes(), false) == Some(bass.len()) && !bass.is_empty(),
    }
}

/// The bracketed pieces of `line`, in order: each `[` up to the next `]` with no `[` between (an
/// unclosed `[` and a stray `]` are plain text; in `[[Am]]` the inner `[Am]` is the chord).
pub fn inline_chords(line: &str) -> Vec<InlineChord> {
    let mut out = Vec::new();
    let b = line.as_bytes();
    let mut i = 0;
    while let Some(off) = b.get(i..).and_then(|r| r.iter().position(|&c| c == b'[')) {
        let open = i + off;
        let after = open + 1;
        let close = b.get(after..).and_then(|r| r.iter().position(|&c| c == b']' || c == b'[')).map(|k| after + k);
        match close {
            Some(c) if b.get(c) == Some(&b']') => {
                let inner = line.get(after..c).unwrap_or("");
                let name = inner.trim().to_string();
                out.push(InlineChord { start: open, end: c + 1, known: is_chord_name(&name), name });
                i = c + 1;
            }
            // Another `[` first: this one is literal; read on from that one.
            Some(c) => i = c,
            None => break,
        }
    }
    out
}

/// The known chords of `line` (bracketed chord names), in order.
pub fn line_chords(line: &str) -> Vec<InlineChord> {
    inline_chords(line).into_iter().filter(|c| c.known).collect()
}

/// A line without its chords, with the way back to the line's own byte offsets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stripped {
    /// The line as sung: the known `[chords]` taken out (unknown brackets stay).
    pub text: String,
    /// Kept runs: (offset in `text`, offset in the line, length).
    runs: Vec<(usize, usize, usize)>,
    /// Each chord taken out: its name and the offset in `text` it stood at (the syllable it
    /// precedes starts there).
    pub chords: Vec<(String, usize)>,
}

impl Stripped {
    /// The line's offset of `text` offset `b` as the start of something: after any chord that
    /// stands there.
    pub fn start_in_line(&self, b: usize) -> usize {
        match self.runs.iter().rev().find(|r| r.0 <= b) {
            Some(&(s, l, len)) => l + (b - s).min(len),
            None => b,
        }
    }

    /// The line's offset of `text` offset `b` as the end of something: before any chord that
    /// stands there.
    pub fn end_in_line(&self, b: usize) -> usize {
        match self.runs.iter().find(|r| r.0 < b && b <= r.0 + r.2).or_else(|| self.runs.iter().rev().find(|r| r.0 <= b)) {
            Some(&(s, l, len)) => l + (b - s).min(len),
            None => b,
        }
    }

    /// Whether the line is only chords (an instrumental bar): at least one, and no words.
    pub fn chords_only(&self) -> bool {
        !self.chords.is_empty() && self.text.trim().is_empty()
    }
}

/// `line` without its known chords.
pub fn strip_chords(line: &str) -> Stripped {
    let mut s = Stripped::default();
    let mut at = 0;
    for c in line_chords(line) {
        if c.start > at {
            s.runs.push((s.text.len(), at, c.start - at));
            s.text.push_str(line.get(at..c.start).unwrap_or(""));
        }
        s.chords.push((c.name, s.text.len()));
        at = c.end;
    }
    if at < line.len() || s.runs.is_empty() {
        s.runs.push((s.text.len(), at, line.len().saturating_sub(at)));
        s.text.push_str(line.get(at..).unwrap_or(""));
    }
    s
}

/// The text of `line` as sung (its chords taken out).
pub fn sung_text(line: &str) -> String {
    if line.contains('[') { strip_chords(line).text } else { line.to_string() }
}

/// Whether `line` is only chords: `[Am]`, `[G/B]  [C]`.
pub fn is_chord_line(line: &str) -> bool {
    line.contains('[') && strip_chords(line).chords_only()
}

/// An old sheet's heading (`[Verse 1]`, from before headings were curly) as it is written now
/// (`{Verse 1}`); `None` for any other line, a chord line (`[Am]`, `[G/B] [C]`) included.
pub fn legacy_heading(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix('[')?.strip_suffix(']')?;
    if is_chord_line(line) {
        return None;
    }
    let lead = line.len() - line.trim_start().len();
    Some(format!("{}{{{inner}}}", line.get(..lead).unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_names_follow_the_band_vocabulary() {
        for c in
            ["Am", "G", "G/B", "C#m7b5", "Bbmaj7", "F\u{266F}m", "Dsus4", "E7", "Cadd9", "A-7", "Bo7", "Cmaj9/E", "Gb", "Ab/Eb", "C\u{0394}7", " Em "]
        {
            assert!(is_chord_name(c), "{c}");
        }
        for c in ["", "a", "am", "be", "Verse 1", "H", "Am7x", "C/", "C/X", "G/Bb7", "C#b", "Am G", "Chorus", "Intro", "Bridge", "x"] {
            assert!(!is_chord_name(c), "{c}");
        }
        assert!(!is_chord_name(&"A".repeat(40)));
    }

    #[test]
    fn inline_chords_strip_and_map_back() {
        let line = "[Am]Hello dark[G]ness my old [C]friend";
        let s = strip_chords(line);
        assert_eq!(s.text, "Hello darkness my old friend");
        assert_eq!(s.chords, vec![("Am".to_string(), 0), ("G".to_string(), 10), ("C".to_string(), 22)]);
        // "friend" in the sung text, back in the line.
        let at = s.text.find("friend").unwrap();
        assert_eq!(&line[s.start_in_line(at)..s.end_in_line(at + 6)], "friend");
        // "Hello" ends right before no chord; "dark" ends right before [G].
        assert_eq!(&line[s.start_in_line(0)..s.end_in_line(5)], "Hello");
        assert_eq!(&line[s.start_in_line(6)..s.end_in_line(10)], "dark");
        assert_eq!(&line[s.start_in_line(6)..s.end_in_line(14)], "dark[G]ness");
        // Unknown brackets stay as words.
        let s = strip_chords("Sing [loud] now [Am]");
        assert_eq!(s.text, "Sing [loud] now ");
        assert!(!s.chords_only());
        let c = inline_chords("Sing [loud] now [Am]");
        assert_eq!(c.iter().map(|c| (c.name.as_str(), c.known)).collect::<Vec<_>>(), vec![("loud", false), ("Am", true)]);
        assert_eq!(sung_text("no chords"), "no chords");
    }

    #[test]
    fn chord_lines_and_legacy_headings() {
        assert!(is_chord_line("[Am]"));
        assert!(is_chord_line("  [G/B]  [C] "));
        assert!(!is_chord_line("[Verse 1]"));
        assert!(!is_chord_line("[Am] la"));
        assert!(!is_chord_line(""));
        assert_eq!(legacy_heading("[Verse 1]").as_deref(), Some("{Verse 1}"));
        assert_eq!(legacy_heading("  [Chorus]  ").as_deref(), Some("  {Chorus}"));
        assert_eq!(legacy_heading("[Am]"), None);
        assert_eq!(legacy_heading("[G/B]  [C]"), None);
        assert_eq!(legacy_heading("When the night"), None);
        assert_eq!(legacy_heading("{Verse 1}"), None);
        // An old heading the old app read as one keeps its name.
        assert_eq!(legacy_heading("[Verse 1] [x]").as_deref(), Some("{Verse 1] [x}"));
    }

    #[test]
    fn hostile_brackets_never_panic() {
        for line in ["[", "]", "[[", "]]", "[[Am]]", "[Am", "Am]", "[]", "[ ]", "[\u{0}]", "[é]", "[Am][G][C]", "[\u{266F}]", "a[b[c]d]e"] {
            let s = strip_chords(line);
            for b in 0..=s.text.len() {
                let (x, y) = (s.start_in_line(b), s.end_in_line(b));
                assert!(x <= line.len() && y <= line.len(), "{line:?} {b}");
            }
            let _ = legacy_heading(line);
            let _ = is_chord_line(line);
        }
        assert_eq!(strip_chords("[[Am]]").text, "[]");
        assert_eq!(strip_chords("[Am][G][C]").chords.len(), 3);
        let long = "[Am]".repeat(100) + &"x[".repeat(200);
        assert_eq!(strip_chords(&long).chords.len(), 100);
    }
}
