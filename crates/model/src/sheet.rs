//! The lyric sheet (Window › Lyrics): the song's words as written, one entry per line, each with a
//! stable id so what refers to a line (its marker on the Lyrics ruler) survives edits around it.
//! "[Verse 1]" lines are section headings; blank lines separate; every other line is a lyric line,
//! numbered from 1 down the sheet.

use serde::{Deserialize, Deserializer, Serialize};
use soundcraft_time::Samples;

/// Most lines a sheet keeps.
pub const MAX_SHEET_LINES: usize = 2000;
/// Longest line, characters.
pub const MAX_SHEET_LINE_CHARS: usize = 400;
/// Most versions (alternates) a line keeps.
pub const MAX_ALTERNATES: usize = 16;
/// Most drafts a sheet keeps.
pub const MAX_DRAFTS: usize = 50;

/// One line of the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SheetLine {
    /// Unique in the session (from `Session::alloc`), kept while the line is edited.
    pub id: u64,
    /// The line as sung: the active version when it has alternates.
    pub text: String,
    /// Alternate versions of the line, the active one included (empty: none). `alts[active]` is
    /// always `text`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alts: Vec<String>,
    /// Which of `alts` is active.
    #[serde(skip_serializing_if = "is_zero")]
    pub active: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

impl SheetLine {
    pub fn new(id: u64, text: impl Into<String>) -> SheetLine {
        SheetLine { id, text: text.into(), ..SheetLine::default() }
    }

    /// Keeps the alternates in step with the text: the active index in range, the active version
    /// the text; a single version is no alternates.
    pub fn sync_alts(&mut self) {
        self.alts.truncate(MAX_ALTERNATES);
        if self.alts.len() <= 1 {
            self.alts.clear();
            self.active = 0;
            return;
        }
        let last = u32::try_from(self.alts.len() - 1).unwrap_or(0);
        self.active = self.active.min(last);
        if let Some(a) = self.alts.get_mut(self.active as usize) {
            a.clone_from(&self.text);
        }
    }
}

/// A named snapshot of the whole sheet (Lyrics › Drafts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Draft {
    pub id: u64,
    pub name: String,
    /// When it was saved (RFC 3339; empty when the clock was unavailable).
    pub created: String,
    pub lines: Vec<SheetLine>,
}

impl Draft {
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// What a sheet line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    /// "[Verse 1]": a section heading.
    Heading,
    /// A lyric line, with its number (1-based).
    Lyric(u32),
}

/// A lyric line placed on the Lyrics ruler: a range marker titled by the line's current text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LyricPlace {
    /// The sheet line's id.
    pub line: u64,
    pub start: Samples,
    pub end: Samples,
}

/// The session's lyric sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LyricSheet {
    pub lines: Vec<SheetLine>,
    /// Lines placed on the Lyrics ruler, in time order (one per line).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub placed: Vec<LyricPlace>,
    /// Saved drafts, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<Draft>,
    /// The hook (title) phrase the repetition check counts; none: the song's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<String>,
}

/// The section name of a heading line ("[Verse 1]" is "Verse 1").
pub fn heading_name(text: &str) -> Option<&str> {
    let t = text.trim();
    t.strip_prefix('[').and_then(|r| r.strip_suffix(']')).map(str::trim)
}

impl LyricSheet {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.placed.is_empty() && self.drafts.is_empty() && self.hook.is_none()
    }

    /// Where the line with id `id` is placed.
    pub fn place_of(&self, id: u64) -> Option<LyricPlace> {
        self.placed.iter().find(|p| p.line == id).copied()
    }

    /// The placement at `at` (the last one starting at or before it that has not ended).
    pub fn placed_at(&self, at: Samples) -> Option<LyricPlace> {
        self.placed.iter().rev().find(|p| p.start <= at && at < p.end).copied()
    }

    /// Keeps the placements in step with the lines: those of lines gone (or no longer lyric
    /// lines) go, the rest sorted by time, one per line, ranges ordered and non-negative.
    pub fn tidy_places(&mut self) {
        let kinds = self.kinds();
        let lyric: std::collections::HashSet<u64> =
            self.lines.iter().zip(kinds).filter(|(_, k)| matches!(k, LineKind::Lyric(_))).map(|(l, _)| l.id).collect();
        let mut seen = std::collections::HashSet::new();
        self.placed.retain_mut(|p| {
            p.start = p.start.max(0);
            p.end = p.end.max(p.start.saturating_add(1));
            lyric.contains(&p.line) && seen.insert(p.line)
        });
        self.placed.sort_by_key(|p| (p.start, p.end));
    }

    /// The sheet as text, one line per line.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Each line's kind, in order.
    pub fn kinds(&self) -> Vec<LineKind> {
        line_kinds(self.lines.iter().map(|l| l.text.as_str()))
    }

    /// The lyric line numbered `n`.
    pub fn line_numbered(&self, n: u32) -> Option<&SheetLine> {
        self.kinds().iter().position(|k| *k == LineKind::Lyric(n)).and_then(|i| self.lines.get(i))
    }

    /// The number of the lyric line with id `id`.
    pub fn number_of(&self, id: u64) -> Option<u32> {
        let i = self.lines.iter().position(|l| l.id == id)?;
        match self.kinds().get(i) {
            Some(LineKind::Lyric(n)) => Some(*n),
            _ => None,
        }
    }

    /// The section heading above the line with id `id` ("Verse 1").
    pub fn section_of(&self, id: u64) -> Option<String> {
        let i = self.lines.iter().position(|l| l.id == id)?;
        self.lines.get(..i)?.iter().rev().find_map(|l| heading_name(&l.text)).map(str::to_string)
    }
}

/// The kinds of `lines` (texts) in order: blank, heading or numbered lyric.
pub fn line_kinds<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<LineKind> {
    let mut n = 0u32;
    lines
        .into_iter()
        .map(|t| {
            if t.trim().is_empty() {
                LineKind::Blank
            } else if heading_name(t).is_some() {
                LineKind::Heading
            } else {
                n = n.saturating_add(1);
                LineKind::Lyric(n)
            }
        })
        .collect()
}

fn clean_line(s: &str) -> String {
    let mut t: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if let Some((i, _)) = t.char_indices().nth(MAX_SHEET_LINE_CHARS) {
        t.truncate(i);
    }
    t.trim_end().to_string()
}

/// Text split into sheet lines (control characters to spaces, lines capped in length and number).
pub fn sheet_texts(text: &str) -> Vec<String> {
    text.split('\n').take(MAX_SHEET_LINES).map(|l| clean_line(l.strip_suffix('\r').unwrap_or(l))).collect()
}

/// Sheet `new` lines with ids carried over from `old`: unchanged lines keep theirs (matched in
/// order, longest common run), and an edited line keeps the id of the old line it replaced (the
/// unmatched lines between two kept ones, paired in order: positionally when as many changed as
/// were there, else by shared words). New lines get ids from `alloc`.
pub fn carry_ids(old: &[SheetLine], new: &[String], mut alloc: impl FnMut() -> u64) -> Vec<SheetLine> {
    let (n, m) = (old.len(), new.len());
    // Longest common subsequence of the texts (the sheet is capped, so n*m stays bounded).
    let w = m + 1;
    let mut dp = vec![0u16; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let v = if old.get(i).map(|l| &l.text) == new.get(j) {
                dp.get((i + 1) * w + j + 1).copied().unwrap_or(0).saturating_add(1)
            } else {
                dp.get((i + 1) * w + j).copied().unwrap_or(0).max(dp.get(i * w + j + 1).copied().unwrap_or(0))
            };
            if let Some(c) = dp.get_mut(i * w + j) {
                *c = v;
            }
        }
    }
    let at = |i: usize, j: usize| dp.get(i * w + j).copied().unwrap_or(0);
    let mut ids: Vec<Option<u64>> = vec![None; m];
    let (mut i, mut j) = (0, 0);
    let mut gap_old: Vec<usize> = Vec::new();
    let mut gap_new: Vec<usize> = Vec::new();
    let flush = |gap_old: &mut Vec<usize>, gap_new: &mut Vec<usize>, ids: &mut Vec<Option<u64>>| {
        pair_gap(old, new, gap_old, gap_new, ids);
        gap_old.clear();
        gap_new.clear();
    };
    while i < n || j < m {
        if i < n && j < m && old.get(i).map(|l| &l.text) == new.get(j) && at(i, j) == at(i + 1, j + 1) + 1 {
            flush(&mut gap_old, &mut gap_new, &mut ids);
            if let (Some(slot), Some(l)) = (ids.get_mut(j), old.get(i)) {
                *slot = Some(l.id);
            }
            i += 1;
            j += 1;
        } else if j < m && (i >= n || at(i, j + 1) >= at(i + 1, j)) {
            gap_new.push(j);
            j += 1;
        } else {
            gap_old.push(i);
            i += 1;
        }
    }
    flush(&mut gap_old, &mut gap_new, &mut ids);
    let mut seen = std::collections::HashSet::new();
    let by_id: std::collections::HashMap<u64, &SheetLine> = old.iter().map(|l| (l.id, l)).collect();
    new.iter()
        .zip(ids)
        .map(|(t, id)| {
            let id = id.filter(|id| *id != 0 && seen.insert(*id)).unwrap_or_else(&mut alloc);
            seen.insert(id);
            // A line keeps its alternates through edits; the edit is to the active version.
            let mut line = SheetLine::new(id, t.clone());
            if let Some(o) = by_id.get(&id) {
                line.alts.clone_from(&o.alts);
                line.active = o.active;
                line.sync_alts();
            }
            line
        })
        .collect()
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// How alike two lines are, 0..1: shared words over the longer line's words, or a shared start.
fn likeness(a: &str, b: &str) -> f32 {
    let (wa, wb) = (words(a), words(b));
    let longest = wa.len().max(wb.len());
    let shared = if longest == 0 { 0.0 } else { wa.iter().filter(|w| wb.contains(w)).count() as f32 / longest as f32 };
    let prefix = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count() as f32 / a.chars().count().max(b.chars().count()).max(1) as f32;
    shared.max(prefix)
}

/// Reads one line leniently (text and versions cleaned and capped).
fn tame_line(line: &mut SheetLine) {
    line.text = clean_line(&line.text);
    line.alts.truncate(MAX_ALTERNATES);
    for a in &mut line.alts {
        *a = clean_line(a);
    }
    line.sync_alts();
}

fn pair_gap(old: &[SheetLine], new: &[String], gap_old: &[usize], gap_new: &[usize], ids: &mut [Option<u64>]) {
    if gap_old.is_empty() || gap_new.is_empty() {
        return;
    }
    // The non-blank lines on each side (a blank line's id carries nothing).
    let real = |t: &str| !t.trim().is_empty();
    let go: Vec<usize> = gap_old.iter().copied().filter(|&i| old.get(i).is_some_and(|l| real(&l.text))).collect();
    let gn: Vec<usize> = gap_new.iter().copied().filter(|&j| new.get(j).is_some_and(|t| real(t))).collect();
    if go.len() == gn.len() {
        for (&i, &j) in go.iter().zip(&gn) {
            if let (Some(slot), Some(l)) = (ids.get_mut(j), old.get(i)) {
                *slot = Some(l.id);
            }
        }
        return;
    }
    // Different counts (lines added or removed beside an edit): pair alike lines in order.
    let mut from = 0;
    for &j in &gn {
        let Some(t) = new.get(j) else { continue };
        let best = go
            .iter()
            .enumerate()
            .skip(from)
            .filter_map(|(k, &i)| old.get(i).map(|l| (k, l, likeness(&l.text, t))))
            .filter(|(_, _, s)| *s >= 0.5)
            .max_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((k, l, _)) = best {
            if let Some(slot) = ids.get_mut(j) {
                *slot = Some(l.id);
            }
            from = k + 1;
        }
    }
}

/// Reads a sheet leniently: unreadable lines are skipped, ids made unique, sizes capped.
pub(crate) fn de_sheet<'de, D: Deserializer<'de>>(d: D) -> Result<LyricSheet, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let mut sheet = LyricSheet::default();
    if let Some(serde_json::Value::Array(places)) = v.get("placed") {
        sheet.placed = places.iter().take(MAX_SHEET_LINES).filter_map(|p| serde_json::from_value::<LyricPlace>(p.clone()).ok()).collect();
    }
    if let Some(serde_json::Value::Array(lines)) = v.get("lines") {
        let mut seen = std::collections::HashSet::new();
        for l in lines.iter().take(MAX_SHEET_LINES) {
            let Ok(mut line) = serde_json::from_value::<SheetLine>(l.clone()) else { continue };
            if line.id == 0 || !seen.insert(line.id) {
                continue;
            }
            tame_line(&mut line);
            sheet.lines.push(line);
        }
    }
    if let Some(h) = v.get("hook").and_then(serde_json::Value::as_str) {
        let h: String = clean_line(h).chars().take(80).collect();
        sheet.hook = Some(h).filter(|h| !h.trim().is_empty());
    }
    if let Some(serde_json::Value::Array(drafts)) = v.get("drafts") {
        for d in drafts.iter().take(MAX_DRAFTS) {
            let Ok(mut draft) = serde_json::from_value::<Draft>(d.clone()) else { continue };
            draft.lines.truncate(MAX_SHEET_LINES);
            let mut seen = std::collections::HashSet::new();
            draft.lines.retain(|l| l.id != 0 && seen.insert(l.id));
            draft.lines.iter_mut().for_each(tame_line);
            draft.name = clean_line(&draft.name).chars().take(80).collect();
            draft.created = draft.created.chars().take(40).collect();
            sheet.drafts.push(draft);
        }
    }
    sheet.tidy_places();
    Ok(sheet)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(texts: &[&str]) -> Vec<SheetLine> {
        texts.iter().enumerate().map(|(i, t)| SheetLine::new(100 + i as u64, *t)).collect()
    }

    #[test]
    fn alternates_follow_edits_and_are_tamed() {
        let mut old = sheet(&["[Verse 1]", "When the night was long"]);
        old[1].alts = vec!["When the night was long".into(), "When the day was gone".into()];
        let new: Vec<String> = vec!["[Verse 1]".into(), "When the night was so long".into()];
        let mut n = 0;
        let lines = carry_ids(&old, &new, || {
            n += 1;
            n
        });
        assert_eq!(lines[1].alts, vec!["When the night was so long", "When the day was gone"]);
        let v = serde_json::json!({"lines": [{"id": 1, "text": "a", "alts": ["x", "y"], "active": 9}, {"id": 2, "text": "b", "alts": ["b"]}],
            "drafts": [{"id": 5, "name": "First", "lines": [{"id": 1, "text": "old"}, {"id": 1, "text": "dup"}]}, "junk"]});
        let s = de_sheet(v).unwrap();
        assert_eq!((s.lines[0].active, s.lines[0].alts.clone()), (1, vec!["x".to_string(), "a".to_string()]));
        assert!(s.lines[1].alts.is_empty());
        assert_eq!(s.drafts.len(), 1);
        assert_eq!(s.drafts[0].text(), "old");
    }

    fn carried(old: &[SheetLine], new: &[&str]) -> Vec<u64> {
        let mut next = 1000;
        let new: Vec<String> = new.iter().map(|s| s.to_string()).collect();
        carry_ids(old, &new, || {
            next += 1;
            next
        })
        .iter()
        .map(|l| l.id)
        .collect()
    }

    #[test]
    fn kinds_number_lyric_lines_only() {
        let k = line_kinds(["[Verse 1]", "When the night was long", "", "And my heart", " [Chorus] ", "Jesus"]);
        assert_eq!(k, vec![LineKind::Heading, LineKind::Lyric(1), LineKind::Blank, LineKind::Lyric(2), LineKind::Heading, LineKind::Lyric(3)]);
        let s = LyricSheet { lines: sheet(&["[Verse 1]", "a", "b", "[Chorus]", "c"]), ..LyricSheet::default() };
        assert_eq!(s.number_of(104), Some(3));
        assert_eq!(s.section_of(104).as_deref(), Some("Chorus"));
        assert_eq!(s.line_numbered(2).map(|l| l.id), Some(102));
    }

    #[test]
    fn ids_survive_edits_inserts_and_deletes() {
        let old = sheet(&["[Verse 1]", "When the night was long", "And my heart could not be still", "You were there"]);
        // An edited line keeps its id.
        assert_eq!(
            carried(&old, &["[Verse 1]", "When the night was so long", "And my heart could not be still", "You were there"]),
            vec![100, 101, 102, 103]
        );
        // A line inserted above keeps the others'.
        assert_eq!(
            carried(&old, &["New first line", "[Verse 1]", "When the night was long", "And my heart could not be still", "You were there"]),
            vec![1001, 100, 101, 102, 103]
        );
        // A deleted line takes only its own id with it.
        assert_eq!(carried(&old, &["[Verse 1]", "When the night was long", "You were there"]), vec![100, 101, 103]);
        // An edit next to an insert pairs by words.
        assert_eq!(
            carried(&old, &["[Verse 1]", "When the night was long", "A brand new line", "And my heart could never be still", "You were there"]),
            vec![100, 101, 1001, 102, 103]
        );
        // Duplicate texts keep distinct ids.
        let dup = sheet(&["Jesus, the only one", "Jesus, the only one"]);
        assert_eq!(carried(&dup, &["Jesus, the only one", "Jesus, the only one", "Jesus, the only one"]), vec![100, 101, 1001]);
    }

    #[test]
    fn hostile_sheets_are_tamed() {
        let v = serde_json::json!({"lines": [{"id": 1, "text": "a\u{0}b"}, {"id": 1, "text": "dup"}, {"id": 0}, {"id": "x"}, {"id": 2, "text": "z".repeat(900)}]});
        let s = de_sheet(v).unwrap();
        assert_eq!(s.lines.len(), 2);
        assert_eq!(s.lines[0].text, "a b");
        assert_eq!(s.lines[1].text.chars().count(), MAX_SHEET_LINE_CHARS);
        assert!(de_sheet(serde_json::json!("nope")).unwrap().is_empty());
        let v = serde_json::json!({"lines": [{"id": 1, "text": "a"}, {"id": 2, "text": "[Chorus]"}],
            "placed": [{"line": 1, "start": 50, "end": 10}, {"line": 1, "start": 0, "end": 5}, {"line": 2, "start": 0, "end": 9}, {"line": 7, "start": 0, "end": 9}, {"line": "x"}]});
        let s = de_sheet(v).unwrap();
        assert_eq!(s.placed, vec![LyricPlace { line: 1, start: 50, end: 51 }]);
        assert_eq!(s.placed_at(50).map(|p| p.line), Some(1));
        assert_eq!(s.placed_at(51), None);
        assert_eq!(sheet_texts("a\r\nb\n").len(), 3);
    }
}
