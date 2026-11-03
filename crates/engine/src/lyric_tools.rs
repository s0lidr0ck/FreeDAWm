//! Lyric writing tools (Window › Lyrics, round 2): rhyme types across all words of the sheet, and
//! line-to-line fit (syllables and stress against the same line of the section's first take).
//! Built on [`crate::lyric_text`] and the bundled [`crate::lexicon`].

use crate::lexicon::{self, RhymeKind, WordLookup};
use crate::lyric_text::{end_word, heuristic_rhyme_key, is_function_word, line_stresses};
use soundcraft_model::{LineKind, heading_name, line_kinds};

/// A section's kind: its heading without a trailing number, lower case ("Verse 2" is "verse").
pub fn section_kind(heading: &str) -> String {
    heading.trim().trim_end_matches(|c: char| c.is_ascii_digit() || c.is_whitespace()).trim().to_lowercase()
}

/// How a lyric line fits the line it is sung against.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LineFit {
    /// The sheet line (index from 0).
    pub line: usize,
    pub syllables: u32,
    /// Per syllable, whether it is stressed.
    pub stress: Vec<bool>,
    /// The partner: the same line of the first section of this kind (sheet line index), when this
    /// line is in a later one.
    pub partner: Option<usize>,
    /// The partner's lyric line number and section heading.
    pub partner_number: Option<u32>,
    pub partner_section: Option<String>,
    /// Why the line is flagged (none when it fits).
    pub flag: Option<String>,
}

/// Stress as text: "da-DUM-da".
pub fn stress_text(s: &[bool]) -> String {
    lexicon::stress_text(s)
}

/// Each lyric line's syllables, stress and fit (one entry per lyric line, in order). Lines are
/// paired by position within same-kind sections (Verse 1 line n with Verse 2 line n; a repeated
/// Chorus with the first). A line is flagged when its syllable count differs from its partner's
/// by two or more, when it has a different number of stressed syllables, or when, with as many
/// syllables, its stresses fall elsewhere.
pub fn line_fit<'a>(lines: impl IntoIterator<Item = &'a str> + Clone) -> Vec<LineFit> {
    let kinds = line_kinds(lines.clone());
    // (section kind, its heading, the sheet lines of its lyric lines)
    let mut sections: Vec<(String, String, Vec<usize>)> = Vec::new();
    let mut out: Vec<LineFit> = Vec::new();
    let mut numbers: Vec<Option<u32>> = Vec::new();
    for (i, (text, kind)) in lines.into_iter().zip(kinds).enumerate() {
        numbers.push(if let LineKind::Lyric(n) = kind { Some(n) } else { None });
        match kind {
            LineKind::Heading => {
                let name = heading_name(text).unwrap_or("").to_string();
                sections.push((section_kind(&name), name, Vec::new()));
            }
            LineKind::Lyric(_) => {
                if let Some(s) = sections.last_mut() {
                    s.2.push(i);
                }
                let stress = line_stresses(text);
                out.push(LineFit {
                    line: i,
                    syllables: stress.len() as u32,
                    stress,
                    partner: None,
                    partner_number: None,
                    partner_section: None,
                    flag: None,
                });
            }
            LineKind::Blank => {}
        }
    }
    for (si, (kind, _, members)) in sections.iter().enumerate() {
        let Some((_, first_name, first)) = sections.iter().take(si).find(|(k, _, _)| k == kind && !kind.is_empty()) else { continue };
        for (pos, &line) in members.iter().enumerate() {
            let Some(&p) = first.get(pos) else { continue };
            let partner = out.iter().find(|f| f.line == p).cloned();
            let Some(me) = out.iter_mut().find(|f| f.line == line) else { continue };
            let Some(partner) = partner else { continue };
            me.partner = Some(p);
            me.partner_number = numbers.get(p).copied().flatten();
            me.partner_section = Some(first_name.clone());
            let who = format!("line {} ({first_name})", me.partner_number.unwrap_or(0));
            let beats = |s: &[bool]| s.iter().filter(|b| **b).count();
            let diff = i64::from(me.syllables) - i64::from(partner.syllables);
            me.flag = if diff.abs() >= 2 {
                Some(format!(
                    "{} syllables {} than {who}: {} vs {}",
                    diff.abs(),
                    if diff > 0 { "more" } else { "fewer" },
                    me.syllables,
                    partner.syllables
                ))
            } else if beats(&me.stress) != beats(&partner.stress) {
                Some(format!("{} stressed syllables, {who} has {}", beats(&me.stress), beats(&partner.stress)))
            } else if diff == 0 && me.stress != partner.stress {
                Some(format!("stress falls differently from {who}: {} vs {}", stress_text(&me.stress), stress_text(&partner.stress)))
            } else {
                None
            };
        }
    }
    out
}

/// How a word rhymes with another nearby, as the Lyrics tab colours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkKind {
    /// End words with the same rhyme part ("long", "song").
    Perfect,
    /// End words that nearly rhyme ("home", "alone").
    Slant,
    /// End words whose last two or more syllables match ("falling", "calling").
    Multi,
    /// A rhyme inside a line, or mid-line across adjacent lines.
    Internal,
}

impl MarkKind {
    pub const ALL: [MarkKind; 4] = [MarkKind::Perfect, MarkKind::Slant, MarkKind::Multi, MarkKind::Internal];

    pub fn bit(self) -> u8 {
        match self {
            MarkKind::Perfect => 1,
            MarkKind::Slant => 2,
            MarkKind::Multi => 4,
            MarkKind::Internal => 8,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MarkKind::Perfect => "perfect",
            MarkKind::Slant => "slant",
            MarkKind::Multi => "multi",
            MarkKind::Internal => "internal",
        }
    }

    /// Which kind shows when a word has several (and they are all switched on).
    fn rank(self) -> u8 {
        match self {
            MarkKind::Multi => 4,
            MarkKind::Perfect => 3,
            MarkKind::Internal => 2,
            MarkKind::Slant => 1,
        }
    }
}

/// A rhyming word of the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct WordMark {
    /// The sheet line (index from 0, headings and blank lines counted).
    pub line: usize,
    /// The word's byte range in the line.
    pub start: usize,
    pub end: usize,
    /// The kinds of rhyme it takes part in ([`MarkKind::bit`]s).
    pub kinds: u8,
}

impl WordMark {
    /// The kind to colour it by, of those switched on in `enabled` (bits).
    pub fn shown(&self, enabled: u8) -> Option<MarkKind> {
        MarkKind::ALL.into_iter().filter(|k| self.kinds & enabled & k.bit() != 0).max_by_key(|k| k.rank())
    }
}

/// How far apart (in lyric lines) end words are compared.
const END_WINDOW: usize = 4;

struct Word<'a> {
    line: usize,
    /// Position among lyric lines.
    lyric: usize,
    /// The section block (lines between headings or blank lines).
    block: usize,
    start: usize,
    end: usize,
    text: &'a str,
    key: String,
    is_end: bool,
}

/// The words of `line` with their byte ranges (letters, digits and inner apostrophes).
pub fn word_spans(line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let inner_apostrophe = (c == '\'' || c == '\u{2019}') && start.is_some() && chars.peek().is_some_and(|(_, n)| n.is_alphanumeric());
        if c.is_alphanumeric() || inner_apostrophe {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, i));
        }
    }
    if let Some(s) = start {
        out.push((s, line.len()));
    }
    out
}

/// How two words rhyme: the dictionary's verdict when it knows both, else the spelling key
/// (perfect only).
fn kind_of(lex: Option<&dyn WordLookup>, a: &Word, b: &Word) -> Option<RhymeKind> {
    if a.key == b.key {
        return None;
    }
    if let Some(l) = lex
        && l.syllables(a.text).is_some()
        && l.syllables(b.text).is_some()
    {
        return l.rhyme_kind(a.text, b.text);
    }
    (heuristic_rhyme_key(a.text).is_some() && heuristic_rhyme_key(a.text) == heuristic_rhyme_key(b.text)).then_some(RhymeKind::Perfect)
}

/// The rhyming words of a sheet: end words that rhyme with an end word up to four lyric lines
/// away in the same block (perfect, slant or multi), and words that rhyme (perfectly or over
/// several syllables) inside a line or mid-line across adjacent lines (internal). Repeats of the
/// same word are not rhymes. Uses the lexicon if loaded (see [`rhyme_marks_with`]).
pub fn rhyme_marks<'a>(lines: impl IntoIterator<Item = &'a str> + Clone) -> Vec<WordMark> {
    rhyme_marks_with(lines, lexicon::get().map(|l| l as &dyn WordLookup))
}

/// [`rhyme_marks`] with an explicit word source (`None`: spelling only).
pub fn rhyme_marks_with<'a>(lines: impl IntoIterator<Item = &'a str> + Clone, lex: Option<&dyn WordLookup>) -> Vec<WordMark> {
    let kinds = line_kinds(lines.clone());
    let mut words: Vec<Word> = Vec::new();
    let (mut block, mut lyric) = (0usize, 0usize);
    let mut in_block = false;
    for (i, (text, kind)) in lines.into_iter().zip(kinds).enumerate() {
        let LineKind::Lyric(_) = kind else {
            if in_block {
                block += 1;
                in_block = false;
            }
            continue;
        };
        in_block = true;
        let spans = word_spans(text);
        let last = end_word(text).and_then(|w| text.rfind(w));
        for (s, e) in spans {
            let Some(w) = text.get(s..e) else { continue };
            let is_end = Some(s) == last;
            if !w.chars().any(char::is_alphabetic) || (!is_end && (is_function_word(w) || w.chars().count() < 3)) {
                continue;
            }
            words.push(Word { line: i, lyric, block, start: s, end: e, text: w, key: lexicon::pron::normalise(w), is_end });
        }
        lyric += 1;
    }
    let mut marks: Vec<u8> = vec![0; words.len()];
    for a in 0..words.len() {
        for b in a + 1..words.len() {
            let (Some(wa), Some(wb)) = (words.get(a), words.get(b)) else { continue };
            let gap = wb.lyric.saturating_sub(wa.lyric);
            if gap > END_WINDOW {
                break;
            }
            let both_ends = wa.is_end && wb.is_end && wa.line != wb.line && wa.block == wb.block;
            let near = gap <= 1 && !(wa.is_end && wb.is_end);
            if !both_ends && !near {
                continue;
            }
            let Some(k) = kind_of(lex, wa, wb) else { continue };
            let bit = if both_ends {
                match k {
                    RhymeKind::Perfect => MarkKind::Perfect.bit(),
                    RhymeKind::Slant => MarkKind::Slant.bit(),
                    RhymeKind::Multi => MarkKind::Multi.bit(),
                }
            } else if k != RhymeKind::Slant {
                MarkKind::Internal.bit()
            } else {
                0
            };
            if let Some(m) = marks.get_mut(a) {
                *m |= bit;
            }
            if let Some(m) = marks.get_mut(b) {
                *m |= bit;
            }
        }
    }
    words.iter().zip(marks).filter(|(_, m)| *m != 0).map(|(w, kinds)| WordMark { line: w.line, start: w.start, end: w.end, kinds }).collect()
}

/// The repetition check: how often the hook phrase is sung, and the most repeated words.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Repetition {
    pub hook: String,
    /// Each time the hook is sung: (sheet line, byte start, byte end).
    pub hits: Vec<(usize, usize, usize)>,
    /// The most repeated words that are not function words (two or more times), most first.
    pub words: Vec<(String, usize)>,
}

/// The lyric lines' words, normalised, with their byte ranges: (sheet line, start, end, word).
fn lyric_words<'a>(lines: impl IntoIterator<Item = &'a str> + Clone) -> Vec<(usize, usize, usize, String)> {
    let kinds = line_kinds(lines.clone());
    let mut out = Vec::new();
    for (i, (text, kind)) in lines.into_iter().zip(kinds).enumerate() {
        if !matches!(kind, LineKind::Lyric(_)) {
            continue;
        }
        for (s, e) in word_spans(text) {
            if let Some(w) = text.get(s..e) {
                out.push((i, s, e, lexicon::pron::normalise(w)));
            }
        }
    }
    out
}

/// Where `phrase` is sung in the sheet's lyric lines (word by word, case and punctuation aside;
/// within a line).
pub fn find_phrase<'a>(lines: impl IntoIterator<Item = &'a str> + Clone, phrase: &str) -> Vec<(usize, usize, usize)> {
    let want: Vec<String> = word_spans(phrase).into_iter().filter_map(|(s, e)| phrase.get(s..e)).map(lexicon::pron::normalise).collect();
    if want.is_empty() {
        return Vec::new();
    }
    let words = lyric_words(lines);
    let mut hits = Vec::new();
    let mut k = 0;
    while k + want.len() <= words.len() {
        let run = words.get(k..k + want.len()).unwrap_or(&[]);
        let same_line = run.first().map(|w| w.0) == run.last().map(|w| w.0);
        if same_line && run.iter().zip(&want).all(|(w, x)| w.3 == *x) {
            if let (Some(a), Some(b)) = (run.first(), run.last()) {
                hits.push((a.0, a.1, b.2));
            }
            k += want.len();
        } else {
            k += 1;
        }
    }
    hits
}

/// The repetition check for a sheet with hook phrase `hook` (empty: no hook).
pub fn repetition<'a>(lines: impl IntoIterator<Item = &'a str> + Clone, hook: &str) -> Repetition {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (_, _, _, w) in lyric_words(lines.clone()) {
        if w.chars().filter(|c| c.is_alphabetic()).count() >= 2 && !is_function_word(&w) {
            *counts.entry(w).or_insert(0) += 1;
        }
    }
    let mut words: Vec<(String, usize)> = counts.into_iter().filter(|(_, n)| *n >= 2).collect();
    words.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    words.truncate(30);
    Repetition { hook: hook.trim().to_string(), hits: find_phrase(lines, hook), words }
}

/// The request "Copy for AI" starts from for lyric lines `selected` (numbers): alternatives that
/// keep the line's syllables and rhyme with its partner ("Suggest 5 alternatives for line 7 that
/// keep 7 syllables and rhyme with 'hill'").
pub fn default_ai_request<'a>(lines: impl IntoIterator<Item = &'a str> + Clone, selected: &[u32]) -> String {
    let texts: Vec<&str> = lines.clone().into_iter().collect();
    let infos = crate::lyric_text::analyse(lines);
    let Some(&n) = selected.first() else {
        return "Suggest stronger versions of the weakest lines, keeping each line's syllable count and the rhyme scheme.".into();
    };
    if selected.len() > 1 {
        let last = selected.last().copied().unwrap_or(n);
        return format!("Suggest 3 rewrites of lines {n}\u{2013}{last} that keep each line's syllable count and the rhyme scheme.");
    }
    let Some(i) = infos.iter().position(|x| x.number == Some(n)) else { return format!("Suggest 5 alternatives for line {n}.") };
    let syl = infos.get(i).map_or(0, |x| x.syllables);
    // The line it rhymes with: the nearest other line with its rhyme letter, earlier first.
    let letter = infos.get(i).and_then(|x| x.rhyme);
    let partner = letter.and_then(|c| {
        let near = |k: &usize| infos.get(*k).is_some_and(|x| x.rhyme == Some(c) && x.number.is_some());
        (0..i).rev().find(near).or_else(|| (i + 1..infos.len()).find(near))
    });
    let word = partner.and_then(|k| {
        let (a, b) = infos.get(k)?.word?;
        texts.get(k)?.get(a..b)
    });
    match word {
        Some(w) => format!("Suggest 5 alternatives for line {n} that keep {syl} syllables and rhyme with \u{2018}{w}\u{2019}."),
        None => format!("Suggest 5 alternatives for line {n} that keep {syl} syllables."),
    }
}

/// The "Copy for AI" prompt: the lyrics with their structure, each line's syllables, stress and
/// rhyme letter, the rhyme scheme per section, the selected lines and the request. Pasted into
/// any chat; nothing is sent anywhere.
pub fn ai_prompt<'a>(lines: impl IntoIterator<Item = &'a str> + Clone, title: &str, selected: &[u32], request: &str) -> String {
    use std::fmt::Write as _;
    let texts: Vec<&str> = lines.clone().into_iter().collect();
    let infos = crate::lyric_text::analyse(lines.clone());
    let fits = line_fit(lines);
    let mut p = String::new();
    let title = title.trim();
    let _ = writeln!(
        p,
        "I'm writing a song{}. Help me with the lyrics.",
        if title.is_empty() { String::new() } else { format!(" called \u{201C}{title}\u{201D}") }
    );
    let _ = writeln!(p);
    let _ = writeln!(p, "## Lyrics");
    let _ = writeln!(
        p,
        "Each lyric line: its number, the words, then [syllables | stress pattern (DUM stressed, da not) | rhyme letter]. Section headings in brackets."
    );
    let _ = writeln!(p);
    let mut scheme: Vec<(String, String)> = Vec::new();
    for (i, (text, info)) in texts.iter().zip(&infos).enumerate() {
        if info.heading {
            let _ = writeln!(p, "{}", text.trim());
            scheme.push((soundcraft_model::heading_name(text).unwrap_or("").to_string(), String::new()));
        } else if let Some(n) = info.number {
            let stress = fits.iter().find(|f| f.line == i).map(|f| stress_text(&f.stress)).unwrap_or_default();
            let letter = info.rhyme.map(String::from).unwrap_or_else(|| "-".into());
            let _ = writeln!(p, "{n}. {} [{} | {stress} | {letter}]", text.trim(), info.syllables);
            if scheme.is_empty() {
                scheme.push((String::new(), String::new()));
            }
            if let Some(s) = scheme.last_mut() {
                s.1.push(info.rhyme.unwrap_or('-'));
            }
        } else {
            let _ = writeln!(p);
        }
    }
    let _ = writeln!(p);
    let _ = writeln!(p, "## Rhyme scheme");
    for (name, letters) in scheme.iter().filter(|(_, l)| !l.is_empty()) {
        let _ = writeln!(p, "{}: {letters}", if name.is_empty() { "(no section)" } else { name });
    }
    if !selected.is_empty() {
        let _ = writeln!(p);
        let _ = writeln!(p, "## Selected line{}", if selected.len() == 1 { "" } else { "s" });
        for n in selected {
            if let Some(i) = infos.iter().position(|x| x.number == Some(*n)) {
                let stress = fits.iter().find(|f| f.line == i).map(|f| stress_text(&f.stress)).unwrap_or_default();
                let syl = infos.get(i).map_or(0, |x| x.syllables);
                let _ = writeln!(p, "{n}. \u{201C}{}\u{201D} ({syl} syllables, {stress})", texts.get(i).map_or("", |t| t.trim()));
            }
        }
    }
    let _ = writeln!(p);
    let _ = writeln!(p, "## Request");
    let _ = writeln!(p, "{}", request.trim());
    let _ = writeln!(p);
    let _ = writeln!(p, "Give each suggestion on its own line with its syllable count, and keep the song's voice.");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_for_ai_prompt_has_structure_and_request() {
        lexicon::wait();
        let sheet = [
            "[Verse 1]",
            "When the night was long",
            "And my heart could not be still",
            "You were there to sing the song",
            "Standing strong upon the hill",
        ];
        let req = default_ai_request(sheet, &[4]);
        assert_eq!(req, "Suggest 5 alternatives for line 4 that keep 7 syllables and rhyme with \u{2018}still\u{2019}.");
        let p = ai_prompt(sheet, "Hill Song", &[4], &req);
        assert!(p.contains("\u{201C}Hill Song\u{201D}"));
        assert!(p.contains("[Verse 1]"));
        assert!(p.contains("1. When the night was long [5 | da-da-DUM-da-DUM | A]"), "{p}");
        assert!(p.contains("Verse 1: ABAB"), "{p}");
        assert!(p.contains("## Selected line\n4. \u{201C}Standing strong upon the hill\u{201D} (7 syllables"), "{p}");
        assert!(p.contains(&req));
        assert!(default_ai_request(sheet, &[]).starts_with("Suggest stronger"));
        assert!(default_ai_request(sheet, &[1, 2]).contains("lines 1\u{2013}2"));
    }

    #[test]
    fn repetition_counts_the_hook_and_words() {
        let sheet = ["[Chorus]", "Jesus, the only one", "You are the only one", "", "[Verse 2]", "The only ONE I need, Jesus", "only"];
        let r = repetition(sheet, "the only one");
        assert_eq!(r.hits, vec![(1, 7, 19), (2, 8, 20), (5, 0, 12)]);
        assert_eq!(r.words.first(), Some(&("only".to_string(), 4)));
        assert!(r.words.contains(&("jesus".to_string(), 2)));
        // Function words are not counted; a hook not sung is 0.
        assert!(!r.words.iter().any(|(w, _)| w == "the"));
        assert!(repetition(sheet, "nowhere man").hits.is_empty());
        assert!(repetition(sheet, "").hits.is_empty());
    }

    fn marked<'a>(lines: &[&'a str]) -> Vec<(&'a str, Vec<&'static str>)> {
        let lex = lexicon::wait();
        rhyme_marks_with(lines.iter().copied(), Some(lex))
            .into_iter()
            .map(|m| {
                let w = lines[m.line].get(m.start..m.end).unwrap_or("");
                (w, MarkKind::ALL.into_iter().filter(|k| m.kinds & k.bit() != 0).map(MarkKind::name).collect())
            })
            .collect()
    }

    #[test]
    fn line_fit_flags_a_mismatched_verse_two_line() {
        lexicon::wait();
        let sheet = [
            "[Verse 1]",
            "When the night was long",
            "And my heart could not be still",
            "",
            "[Chorus]",
            "Jesus, the only one",
            "[Verse 2]",
            "When the day was gone",
            "And my soul was wandering far away from home",
            "[Chorus]",
            "Jesus, the only one",
        ];
        let fit = line_fit(sheet);
        assert_eq!(fit.len(), 6);
        let at = |i: usize| fit.iter().find(|f| f.line == i).unwrap();
        // Verse 2 line 1 matches Verse 1 line 1; line 2 runs long.
        assert_eq!(at(7).partner, Some(1));
        assert_eq!(at(7).partner_section.as_deref(), Some("Verse 1"));
        assert_eq!(at(7).flag, None, "{:?}", at(7));
        assert_eq!(at(8).partner, Some(2));
        assert!(at(8).flag.as_deref().is_some_and(|f| f.contains("more than line 2 (Verse 1)")), "{:?}", at(8).flag);
        // The repeated chorus compares to the first and fits; first takes have no partner.
        assert_eq!(at(10).partner, Some(5));
        assert_eq!(at(10).flag, None);
        assert_eq!(at(1).partner, None);
        assert_eq!(stress_text(&at(1).stress), "da-da-DUM-da-DUM");
        assert_eq!(section_kind("Pre-Chorus 2"), "pre-chorus");
        // Same count, stresses elsewhere.
        let f = line_fit(["[Verse 1]", "Over the river", "[Verse 2]", "Remember the night"]);
        assert!(f[1].flag.as_deref().is_some_and(|x| x.contains("stress")), "{f:?}");
    }

    #[test]
    fn classifies_rhyme_types_across_the_sheet() {
        let m = marked(&["[Verse 1]", "When the night was long", "I was all alone", "You sang a song", "and called me home"]);
        assert!(m.contains(&("long", vec!["perfect"])), "{m:?}");
        assert!(m.contains(&("song", vec!["perfect"])), "{m:?}");
        assert!(m.contains(&("alone", vec!["slant"])), "{m:?}");
        assert!(m.contains(&("home", vec!["slant"])), "{m:?}");
        let m = marked(&["I keep falling", "hear you calling"]);
        assert!(m.contains(&("falling", vec!["multi"])) && m.contains(&("calling", vec!["multi"])), "{m:?}");
        // Internal: inside a line, and mid-line across adjacent lines.
        let m = marked(&["The light of the night burns on", "a fire in the dark"]);
        assert!(m.contains(&("light", vec!["internal"])) && m.contains(&("night", vec!["internal"])), "{m:?}");
        // Non-rhymes and repeats are not marked; blocks part end words.
        assert!(marked(&["the hill", "the long road", "the hill"]).is_empty());
        assert!(marked(&["When the night was long", "", "You sang a song"]).is_empty());
        let wm = WordMark { line: 0, start: 0, end: 1, kinds: MarkKind::Slant.bit() | MarkKind::Internal.bit() };
        assert_eq!(wm.shown(0xff), Some(MarkKind::Internal));
        assert_eq!(wm.shown(MarkKind::Slant.bit()), Some(MarkKind::Slant));
        assert_eq!(wm.shown(MarkKind::Perfect.bit()), None);
        // Spelling only, without the dictionary: perfect end rhymes still found.
        let plain = rhyme_marks_with(["When the night was long", "You sang a song"], None);
        assert_eq!(plain.len(), 2);
        assert_eq!(word_spans("don't stop—now"), vec![(0, 5), (6, 10), (13, 16)]);
    }
}
