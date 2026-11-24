//! Words with times on a vocal, typed or heard, and the tools to time them: hand edits (edges,
//! moves, split, merge, retype, delete, insert), Time Words to Vocal (the typed sheet's placed
//! lines spread over the sung notes, see [`crate::word_align`]), tapping words or line starts
//! while the song plays, and the selection helpers the word lanes use.
//!
//! Words live on clips ([`soundcraft_model::Clip::lyrics`]) in the clip's source time, the same
//! store Transcribe Lyrics writes, so they follow every clip edit; here they are addressed by
//! [`WordRef`] (clip, line, word) and handled in timeline samples. Every word knows where its
//! time came from ([`WordTiming`]): heard by a transcriber, a guess from the typed sheet, or
//! confirmed by hand (tapped or edited). Typed and hand-timed words are never replaced by a
//! transcription without asking (`ml.lyrics` `replace_typed`).
//!
//! **Which clips:** a track's playing playlist (on a comped track, the comp: each piece keeps the
//! words sung in it, in its take's source time). A duplicated track or clip carries copies of
//! the words at the same source times; a harmony or double track with its own audio is timed on
//! its own (its notes differ).

use crate::vocal::FrameMap;
use crate::word_align::{Fit, LEAD_MAX, Onset, Seg};
use serde::{Deserialize, Serialize};
use soundcraft_model::{AnalysisKey, Clip, ClipId, LyricLine, LyricWord, Session, TrackId, WordTiming, sanitize_lyrics, to_clip_time};
use soundcraft_time::{Range, Samples};
use soundcraft_vocal::{NoteMode, VocalAnalysis};

/// One word: its clip, line and place in the line (indices into `Clip::lyrics`; they change when
/// words are added, removed or reordered, so a command returns the edited word's new ref).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WordRef {
    pub clip: ClipId,
    pub line: usize,
    pub word: usize,
}

/// A word on a track's timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackWord {
    pub r: WordRef,
    pub start: Samples,
    pub end: Samples,
    pub text: String,
    pub timing: WordTiming,
    /// The typed sheet line the word came from, when it did.
    pub sheet_line: Option<u64>,
}

/// The shortest a word can be made (seconds).
pub const MIN_WORD_SECS: f64 = 0.02;
/// Edges this close (seconds) touch: dragging one moves the other.
pub const TOUCH_SECS: f64 = 0.005;
/// Default snap window (seconds) for hand edits and taps.
pub const SNAP_SECS: f64 = 0.06;

fn secs(s: &Session, x: f64) -> Samples {
    s.sample_rate.samples(x).max(1)
}

/// Source time of the clip's start (its offset into its audio; 0 for MIDI).
fn base(c: &Clip) -> Samples {
    to_clip_time(c, c.start)
}

/// Timeline time of the clip's source time `src`.
fn on_timeline(c: &Clip, src: Samples) -> Samples {
    c.start.saturating_add(src.saturating_sub(base(c)))
}

/// The visible words of `track` (its playing clips, inside each clip's window), in time order,
/// ends cut at their clip's end.
pub fn track_words(s: &Session, track: TrackId) -> Vec<TrackWord> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for c in t.clips().iter().filter(|c| !c.muted && !c.is_video()) {
        let (lo, hi) = (base(c), base(c).saturating_add(c.length));
        for (li, l) in c.lyrics.iter().enumerate() {
            for (wi, w) in l.words.iter().enumerate().filter(|(_, w)| w.start >= lo && w.start < hi) {
                out.push(TrackWord {
                    r: WordRef { clip: c.id, line: li, word: wi },
                    start: on_timeline(c, w.start),
                    end: on_timeline(c, w.end.min(hi)).max(on_timeline(c, w.start)),
                    text: w.text.clone(),
                    timing: w.timing,
                    sheet_line: l.sheet_line,
                });
            }
        }
    }
    out.sort_by_key(|w| (w.start, w.r));
    out
}

/// Typed or hand-timed words of `track` whose start is in `range` (timeline).
pub fn hand_words_in(s: &Session, track: TrackId, range: Range) -> usize {
    track_words(s, track).iter().filter(|w| w.timing != WordTiming::Heard && w.start >= range.start && w.start < range.end).count()
}

/// How many of the track's words are still guesses, and how many are confirmed.
pub fn counts(s: &Session, track: TrackId) -> (usize, usize) {
    let w = track_words(s, track);
    (w.iter().filter(|w| w.timing == WordTiming::Guess).count(), w.iter().filter(|w| w.timing == WordTiming::Confirmed).count())
}

/// The word `r` names.
pub fn word(s: &Session, r: WordRef) -> Option<&LyricWord> {
    s.find_clip(r.clip).and_then(|(_, c)| c.lyrics.get(r.line)).and_then(|l| l.words.get(r.word))
}

/// The ref of the word of `clip` starting at source time `src` with `text` (after an edit
/// re-sorted the clip's lines).
fn locate(s: &Session, clip: ClipId, src: Samples, text: &str) -> Option<WordRef> {
    let (_, c) = s.find_clip(clip)?;
    c.lyrics
        .iter()
        .enumerate()
        .find_map(|(li, l)| l.words.iter().position(|w| w.start == src && w.text == text).map(|wi| WordRef { clip, line: li, word: wi }))
}

/// The track a word of `clip` is on.
pub fn track_of(s: &Session, r: WordRef) -> Option<TrackId> {
    s.find_clip(r.clip).map(|(t, _)| t)
}

/// What a hand edit did: the edited word's new ref and times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edited {
    pub r: WordRef,
    pub start: Samples,
    pub end: Samples,
}

/// Sets `w`'s timeline times on its clip (source time), clipped to the clip's window start.
fn write_times(s: &mut Session, r: WordRef, start: Samples, end: Samples, confirm: bool) -> Option<()> {
    let c = s.find_clip_mut(r.clip)?;
    let (a, b) = (to_clip_time(c, start), to_clip_time(c, end));
    let w = c.lyrics.get_mut(r.line)?.words.get_mut(r.word)?;
    w.start = a;
    w.end = b.max(a);
    if confirm {
        w.timing = WordTiming::Confirmed;
    }
    Some(())
}

fn tidy(s: &mut Session, clips: &[ClipId]) {
    for id in clips {
        if let Some(c) = s.find_clip_mut(*id) {
            sanitize_lyrics(&mut c.lyrics);
        }
    }
}

/// The new ref and times of the word that was `r` with source start `src` and `text`.
fn edited(s: &Session, track: TrackId, clip: ClipId, src: Samples, text: &str) -> Option<Edited> {
    let r = locate(s, clip, src, text)?;
    let w = track_words(s, track).into_iter().find(|w| w.r == r)?;
    Some(Edited { r, start: w.start, end: w.end })
}

/// Sets word `r`'s start and/or end (timeline). A neighbour whose edge touches the moved one
/// follows it, unless `alone`; otherwise edges stop at the neighbours (words never overlap) and
/// at the clip's window, and a word stays at least 20 ms long. With `snap`, each given edge first
/// moves to the nearest note start or end, or breath edge, within `snap` samples. The word (not
/// the neighbour) becomes confirmed.
pub fn set_edges(
    s: &mut Session,
    r: WordRef,
    start: Option<Samples>,
    end: Option<Samples>,
    alone: bool,
    snap: Option<Samples>,
) -> Result<Edited, String> {
    let track = track_of(s, r).ok_or("no such word")?;
    let words = track_words(s, track);
    let i = words.iter().position(|w| w.r == r).ok_or("that word is outside its clip's window")?;
    let cur = words.get(i).cloned().ok_or("no such word")?;
    let (min, touch) = (secs(s, MIN_WORD_SECS), secs(s, TOUCH_SECS));
    let (to_start, to_end) = snap.map(|_| snap_points(s, track)).unwrap_or_default();
    let (start, end) = (start.map(|t| snap.map_or(t, |w| snap_to(t, &to_start, w))), end.map(|t| snap.map_or(t, |w| snap_to(t, &to_end, w))));
    let (lo, hi) = s.find_clip(r.clip).map(|(_, c)| (c.start, c.end())).ok_or("no such clip")?;
    let prev = i.checked_sub(1).and_then(|k| words.get(k)).filter(|p| p.r.clip == r.clip || p.end > lo).cloned();
    let next = words.get(i + 1).cloned();
    let prev_touch = prev.as_ref().is_some_and(|p| p.end + touch >= cur.start) && !alone;
    let next_touch = next.as_ref().is_some_and(|n| n.start <= cur.end + touch) && !alone;
    let low = match &prev {
        Some(p) if prev_touch => p.start + min,
        Some(p) => p.end.max(lo),
        None => lo,
    }
    .max(lo);
    let high = match &next {
        Some(n) if next_touch => n.end - min,
        Some(n) => n.start.min(hi),
        None => hi,
    }
    .min(hi);
    let mut ns = start.unwrap_or(cur.start);
    let mut ne = end.unwrap_or(cur.end);
    if start.is_some() {
        ns = ns.clamp(low, (ne - min).max(low));
    }
    if end.is_some() {
        ne = ne.clamp((ns + min).min(high), high.max(ns + min));
    }
    if ne <= ns {
        return Err("a word needs some length".into());
    }
    let src = s.find_clip(r.clip).map(|(_, c)| to_clip_time(c, ns)).ok_or("no such clip")?;
    write_times(s, r, ns, ne, true).ok_or("no such word")?;
    let mut clips = vec![r.clip];
    if let Some(p) = prev.filter(|_| prev_touch && start.is_some()) {
        write_times(s, p.r, p.start, ns, false);
        clips.push(p.r.clip);
    }
    if let Some(n) = next.filter(|_| next_touch && end.is_some()) {
        write_times(s, n.r, ne, n.end, false);
        clips.push(n.r.clip);
    }
    tidy(s, &clips);
    edited(s, track, r.clip, src, &cur.text).ok_or_else(|| "the word was lost".into())
}

/// Moves word `r` by `by` samples (both edges), stopping at the clip's window and at the far
/// edges of its neighbours; touching neighbours' edges follow unless `alone`.
pub fn move_word(s: &mut Session, r: WordRef, by: Samples, alone: bool) -> Result<Edited, String> {
    let track = track_of(s, r).ok_or("no such word")?;
    let words = track_words(s, track);
    let i = words.iter().position(|w| w.r == r).ok_or("that word is outside its clip's window")?;
    let cur = words.get(i).cloned().ok_or("no such word")?;
    let (min, touch) = (secs(s, MIN_WORD_SECS), secs(s, TOUCH_SECS));
    let (lo, hi) = s.find_clip(r.clip).map(|(_, c)| (c.start, c.end())).ok_or("no such clip")?;
    let prev = i.checked_sub(1).and_then(|k| words.get(k)).cloned();
    let next = words.get(i + 1).cloned();
    let prev_touch = prev.as_ref().is_some_and(|p| p.end + touch >= cur.start) && !alone;
    let next_touch = next.as_ref().is_some_and(|n| n.start <= cur.end + touch) && !alone;
    let low = match &prev {
        Some(p) if prev_touch => p.start + min,
        Some(p) => p.end,
        None => lo,
    }
    .max(lo);
    let high = match &next {
        Some(n) if next_touch => n.end - min,
        Some(n) => n.start,
        None => hi,
    }
    .min(hi);
    let by = by.clamp(low - cur.start, (high - cur.end).max(low - cur.start));
    let (ns, ne) = (cur.start + by, cur.end + by);
    let src = s.find_clip(r.clip).map(|(_, c)| to_clip_time(c, ns)).ok_or("no such clip")?;
    write_times(s, r, ns, ne, true).ok_or("no such word")?;
    let mut clips = vec![r.clip];
    if let Some(p) = prev.filter(|_| prev_touch) {
        write_times(s, p.r, p.start, ns, false);
        clips.push(p.r.clip);
    }
    if let Some(n) = next.filter(|_| next_touch) {
        write_times(s, n.r, ne, n.end, false);
        clips.push(n.r.clip);
    }
    tidy(s, &clips);
    edited(s, track, r.clip, src, &cur.text).ok_or_else(|| "the word was lost".into())
}

/// Retypes word `r` (confirmed).
pub fn set_text(s: &mut Session, r: WordRef, text: &str) -> Result<Edited, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("type the word (Delete removes it)".into());
    }
    if text.split_whitespace().count() > 1 {
        return Err("one word only (Split makes two)".into());
    }
    let track = track_of(s, r).ok_or("no such word")?;
    let c = s.find_clip_mut(r.clip).ok_or("no such clip")?;
    let w = c.lyrics.get_mut(r.line).and_then(|l| l.words.get_mut(r.word)).ok_or("no such word")?;
    w.text = text.to_string();
    w.timing = WordTiming::Confirmed;
    let src = w.start;
    tidy(s, &[r.clip]);
    let text = word(s, r).map(|w| w.text.clone()).unwrap_or_default();
    edited(s, track, r.clip, src, &text).ok_or_else(|| "the word was lost".into())
}

/// Marks words confirmed without moving them.
pub fn confirm(s: &mut Session, refs: &[WordRef]) -> usize {
    let mut n = 0;
    for r in refs {
        if let Some(w) = s.find_clip_mut(r.clip).and_then(|c| c.lyrics.get_mut(r.line)).and_then(|l| l.words.get_mut(r.word))
            && w.timing != WordTiming::Confirmed
        {
            w.timing = WordTiming::Confirmed;
            n += 1;
        }
    }
    n
}

/// Where a split cuts a word's text: `parts` when given, else after the vowel group nearest the
/// middle ("Amen" → "A-", "men"); a word too short to cut keeps its text and the second part is
/// "-" (the syllable held on).
pub fn split_text(text: &str) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    if let Some(i) = chars.iter().position(|c| *c == '-').filter(|i| *i > 0 && *i + 1 < chars.len()) {
        return (chars.iter().take(i + 1).collect(), chars.iter().skip(i + 1).collect());
    }
    let vowel = |c: &char| "aeiouyAEIOUY".contains(*c);
    // Cut points: after a vowel, before a consonant followed by a vowel (V-CV), or between two
    // consonants (VC-CV).
    let cuts: Vec<usize> = (1..chars.len().saturating_sub(1))
        .filter(|&i| {
            let (a, b, c) = (chars.get(i - 1), chars.get(i), chars.get(i + 1));
            match (a, b, c) {
                (Some(a), Some(b), Some(c)) => (vowel(a) && !vowel(b) && vowel(c)) || (!vowel(a) && !vowel(b) && vowel(c) && i >= 2),
                _ => false,
            }
        })
        .collect();
    let mid = chars.len() / 2;
    match cuts.iter().min_by_key(|i| i.abs_diff(mid)) {
        Some(&i) if chars.iter().take(i).any(|c| c.is_alphanumeric()) => {
            (chars.iter().take(i).collect::<String>() + "-", chars.iter().skip(i).collect())
        }
        _ => (text.to_string(), "-".to_string()),
    }
}

/// Splits word `r` at `at` (timeline; default its middle) into two confirmed words in the same
/// line; `texts` names the parts (default [`split_text`]). Returns both.
pub fn split(s: &mut Session, r: WordRef, at: Option<Samples>, texts: Option<(String, String)>) -> Result<(Edited, Edited), String> {
    let track = track_of(s, r).ok_or("no such word")?;
    let cur = track_words(s, track).into_iter().find(|w| w.r == r).ok_or("that word is outside its clip's window")?;
    let min = secs(s, MIN_WORD_SECS);
    let at = at.unwrap_or(cur.start + (cur.end - cur.start) / 2);
    if at < cur.start + min || at > cur.end - min {
        return Err("split inside the word, at least 20 ms from its edges".into());
    }
    let (a, b) = texts.filter(|(a, b)| !a.trim().is_empty() && !b.trim().is_empty()).unwrap_or_else(|| split_text(&cur.text));
    let c = s.find_clip_mut(r.clip).ok_or("no such clip")?;
    let cut = to_clip_time(c, at);
    let line = c.lyrics.get_mut(r.line).ok_or("no such line")?;
    let w = line.words.get_mut(r.word).ok_or("no such word")?;
    let (src_a, end_b) = (w.start, w.end);
    w.end = cut;
    w.text = a.trim().to_string();
    w.timing = WordTiming::Confirmed;
    let second = LyricWord { start: cut, end: end_b, text: b.trim().to_string(), timing: WordTiming::Confirmed };
    line.words.insert(r.word + 1, second);
    tidy(s, &[r.clip]);
    let first = edited(s, track, r.clip, src_a, a.trim()).ok_or("the word was lost")?;
    let second = edited(s, track, r.clip, cut, b.trim()).ok_or("the word was lost")?;
    Ok((first, second))
}

/// Joins word `r` with the next word of its line: one confirmed word from the first's start to
/// the second's end ("A-" + "men" → "Amen", else the two joined by a space).
pub fn merge(s: &mut Session, r: WordRef) -> Result<Edited, String> {
    let track = track_of(s, r).ok_or("no such word")?;
    let c = s.find_clip_mut(r.clip).ok_or("no such clip")?;
    let line = c.lyrics.get_mut(r.line).ok_or("no such line")?;
    let next = line.words.get(r.word + 1).cloned().ok_or("that is the last word of its line")?;
    let w = line.words.get_mut(r.word).ok_or("no such word")?;
    w.text = match w.text.strip_suffix('-') {
        Some(head) => format!("{head}{}", next.text.trim_start_matches('-')),
        None if next.text == "-" => w.text.clone(),
        None => format!("{} {}", w.text, next.text),
    };
    w.end = w.end.max(next.end);
    w.timing = WordTiming::Confirmed;
    let (src, text) = (w.start, w.text.clone());
    line.words.remove(r.word + 1);
    tidy(s, &[r.clip]);
    edited(s, track, r.clip, src, &text).ok_or_else(|| "the word was lost".into())
}

/// Removes words (a line left empty goes too).
pub fn delete(s: &mut Session, refs: &[WordRef]) -> usize {
    let mut sorted = refs.to_vec();
    sorted.sort();
    sorted.dedup();
    let mut n = 0;
    let mut clips = Vec::new();
    for r in sorted.iter().rev() {
        if let Some(l) = s.find_clip_mut(r.clip).and_then(|c| c.lyrics.get_mut(r.line))
            && r.word < l.words.len()
        {
            l.words.remove(r.word);
            n += 1;
            clips.push(r.clip);
        }
    }
    clips.dedup();
    tidy(s, &clips);
    n
}

/// Inserts a confirmed word `text` on `track` at `at` (timeline), into the clip playing there:
/// into the line around it (or the line before it in that clip), else a line of its own. It ends
/// at `end`, else 300 ms later, stopping at the next word and the clip's end.
pub fn insert(s: &mut Session, track: TrackId, at: Samples, end: Option<Samples>, text: &str) -> Result<Edited, String> {
    let text = text.trim();
    if text.is_empty() || text.split_whitespace().count() > 1 {
        return Err("type one word".into());
    }
    let words = track_words(s, track);
    if words.iter().any(|w| w.start <= at && at < w.end) {
        return Err("a word is already sung there: split it, or move it first".into());
    }
    let min = secs(s, MIN_WORD_SECS);
    let t = s.track(track).ok_or("no such track")?;
    let c = t.clips().iter().find(|c| !c.muted && !c.is_video() && c.range().contains(at)).ok_or("no clip plays there")?;
    let clip = c.id;
    let next = words.iter().filter(|w| w.start > at).map(|w| w.start).min().unwrap_or(Samples::MAX);
    let end = end.unwrap_or(at + secs(s, 0.3)).min(next).min(c.end());
    if end < at + min {
        return Err("no room for a word there".into());
    }
    let (src, src_end) = (to_clip_time(c, at), to_clip_time(c, end));
    let c = s.find_clip_mut(clip).ok_or("no such clip")?;
    let w = LyricWord { start: src, end: src_end, text: text.to_string(), timing: WordTiming::Confirmed };
    let line = c.lyrics.iter().rposition(|l| l.start <= src && src <= l.end).or_else(|| c.lyrics.iter().rposition(|l| l.start <= src));
    match line.and_then(|i| c.lyrics.get_mut(i)) {
        Some(l) => l.words.push(w),
        None => c.lyrics.push(LyricLine { start: src, end: src_end, words: vec![w], sheet_line: None }),
    }
    tidy(s, &[clip]);
    edited(s, track, clip, src, text).ok_or_else(|| "the word was lost".into())
}

// ---- The vocal's notes -------------------------------------------------------------------------

/// A level dip this deep (dB) between a note and the one before it is a re-sung syllable.
const DIP_DB: f32 = 6.0;
/// Notes further apart than this (seconds) have a gap between them.
const GAP_SECS: f64 = 0.012;

/// Frame level (dB), very low when missing.
fn level(a: &VocalAnalysis, f: usize) -> f32 {
    a.track.frames.get(f).map(|fr| fr.level_db).filter(|l| l.is_finite()).unwrap_or(-120.0)
}

/// How deep the level dips around frame `f` under the peaks either side (dB).
fn dip_db(a: &VocalAnalysis, f: usize) -> f32 {
    let max_in = |lo: usize, hi: usize| (lo..hi).map(|i| level(a, i)).fold(-120.0f32, f32::max);
    let low = (f.saturating_sub(3)..f + 3).map(|i| level(a, i)).fold(0.0f32, f32::min);
    let (before, after) = (max_in(f.saturating_sub(16), f.saturating_sub(3)), max_in(f + 3, f + 16));
    before.min(after) - low
}

/// A level dip this deep (dB) inside a note marks a syllable sung through a voiced consonant...
const SYLLABLE_DIP_DB: f32 = 5.0;
/// A shallower dip (dB) is only a hint ([`Onset::Soft`]).
const SOFT_DIP_DB: f32 = 3.5;
/// Dips are at least this far (frames, 60 ms) from the note's edges and from each other.
const SYLLABLE_MIN_FRAMES: u32 = 12;

/// The frames inside note `[start, end)` where the level dips like a syllable boundary: local
/// minima (over ±4 frames) at least 3.5 dB under the peaks within 80 ms either side, with the
/// kind of onset each one is (5 dB or more: a dip, else a soft one).
fn syllable_dips(a: &VocalAnalysis, start: u32, end: u32) -> Vec<(u32, Onset)> {
    let mut out: Vec<(u32, Onset)> = Vec::new();
    let mut f = start + SYLLABLE_MIN_FRAMES;
    while f + SYLLABLE_MIN_FRAMES < end {
        let lv = level(a, f as usize);
        let local_min = (f.saturating_sub(4)..f + 5).all(|g| level(a, g as usize) >= lv);
        let far = out.last().is_none_or(|(p, _)| f - p >= SYLLABLE_MIN_FRAMES);
        let depth = dip_db(a, f as usize);
        if local_min && far && depth >= SOFT_DIP_DB {
            out.push((f, if depth >= SYLLABLE_DIP_DB { Onset::Dip } else { Onset::Soft }));
            f += SYLLABLE_MIN_FRAMES;
            continue;
        }
        f += 1;
    }
    out
}

/// `Seg::gap_before` of a clip's first note until the notes of all clips are merged.
const FIRST_IN_CLIP: Samples = Samples::MAX / 4;

/// A pitch-tracker dropout is at most this long (frames, 40 ms)...
const DROPOUT_FRAMES: u32 = 8;
/// ...and stays within this of the notes either side (dB): the voice went on, only its pitch
/// was lost for a moment (common across a fast interval in a melisma). A consonant or a breath
/// is quieter.
const DROPOUT_DB: f32 = 8.0;

/// Whether the unvoiced frames `[from, to)` between two notes are a tracker dropout inside one
/// sung sound rather than a consonant or a breath.
fn dropout(a: &VocalAnalysis, from: u32, to: u32) -> bool {
    let (from, to) = (from as usize, to as usize);
    let max_in = |lo: usize, hi: usize| (lo..hi).map(|i| level(a, i)).fold(-120.0f32, f32::max);
    let around = max_in(from.saturating_sub(6), from).min(max_in(to, to + 6));
    let inside = (from..to).map(|i| level(a, i)).fold(0.0f32, f32::min);
    to > from && inside >= around - DROPOUT_DB
}

/// The level (dB) unvoiced sound must reach to count as a consonant or a breath rather than the
/// room: 10 dB over the quietest tenth of the clip's frames, and at least -60 dB.
fn sound_floor(a: &VocalAnalysis) -> f32 {
    let mut lv: Vec<f32> = a.track.frames.iter().map(|f| f.level_db).filter(|l| l.is_finite()).collect();
    if lv.is_empty() {
        return -60.0;
    }
    lv.sort_by(f32::total_cmp);
    let low = lv.get(lv.len() / 10).copied().unwrap_or(-120.0);
    (low + 10.0).max(-60.0)
}

/// The cached analysis of `clip` with Musical notes (syllables split where re-sung).
fn musical(s: &Session, clip: &Clip) -> Option<VocalAnalysis> {
    let key = AnalysisKey::of(s, clip)?;
    let a = s.vocal.analyses.get(&key.hash())?.analysis.clone();
    Some(crate::vocal::in_mode(a, NoteMode::Musical))
}

/// The sung notes of `track`'s playing audio clips as [`Seg`]s in timeline samples, in time
/// order, from the cached analyses (clips not analysed yet give none), and the unvoiced stretches
/// longer than a consonant (breaths), for checking.
pub fn track_segs(s: &Session, track: TrackId) -> (Vec<Seg>, Vec<(Samples, Samples)>) {
    let Some(t) = s.track(track) else { return (Vec::new(), Vec::new()) };
    let lead_max = s.sample_rate.samples(LEAD_MAX);
    let mut segs: Vec<Seg> = Vec::new();
    let mut breaths = Vec::new();
    for c in t.clips().iter().filter(|c| c.is_audio() && !c.muted) {
        let Some(a) = musical(s, c) else { continue };
        let fm = FrameMap::of(c, &a);
        let at = |f: u32| fm.time(f).clamp(c.start, c.end());
        let hop = a.track.hop_samples.max(1);
        let lead_frames = u32::try_from(lead_max / Samples::from(hop)).unwrap_or(30).max(1);
        // Sound above the room: unvoiced frames this loud are a consonant or a breath.
        let thr = sound_floor(&a);
        let heard = |f: usize| a.track.frames.get(f).is_some_and(|fr| fr.cents.is_none() && fr.level_db.is_finite() && fr.level_db >= thr);
        let mut run: Option<usize> = None;
        for f in 0..=a.track.frames.len() {
            match (heard(f), run) {
                (true, None) => run = Some(f),
                (false, Some(r)) => {
                    let (r, f) = (u32::try_from(r).unwrap_or(u32::MAX), u32::try_from(f).unwrap_or(u32::MAX));
                    if f - r > lead_frames && fm.in_window(r, f) {
                        breaths.push((at(r), at(f)));
                    }
                    run = None;
                }
                _ => {}
            }
        }
        let mut prev_end: Option<u32> = None;
        for n in a.notes.iter().filter(|n| fm.in_window(n.start, n.end)) {
            let (start, end) = (at(n.start), at(n.end));
            if end <= start {
                continue;
            }
            let gap_frames = prev_end.map_or(u32::MAX, |p| n.start.saturating_sub(p));
            let onset = if gap_frames > 2 && (gap_frames > DROPOUT_FRAMES || prev_end.is_some_and(|p| !dropout(&a, p, n.start))) {
                Onset::Gap
            } else if dip_db(&a, n.start as usize) >= DIP_DB {
                Onset::Dip
            } else {
                Onset::Glide
            };
            // The consonant just before a note that follows a gap: the unvoiced sound above the
            // room right before it, when it is short (a longer one is a breath).
            let lead = if onset == Onset::Gap {
                let stop = prev_end.unwrap_or(0);
                let mut f = n.start;
                while f > stop && heard(f as usize - 1) && n.start - f <= lead_frames {
                    f -= 1;
                }
                if n.start - f >= 2 && n.start - f <= lead_frames { at(f) } else { start }
            } else {
                start
            };
            let gap_before = if onset == Onset::Gap { prev_end.map_or(FIRST_IN_CLIP, |p| start - at(p).min(start)) } else { 0 };
            // Syllables inside the note: a voiced consonant (l, m, n, r, w, y) dips the level
            // without stopping the voice.
            let cuts = syllable_dips(&a, n.start, n.end);
            let mut from = (n.start, onset, lead, gap_before);
            for (f, kind) in cuts {
                segs.push(Seg { start: at(from.0), end: at(f), onset: from.1, lead: from.2, gap_before: from.3 });
                from = (f, kind, at(f), 0);
            }
            segs.push(Seg { start: at(from.0), end, onset: from.1, lead: from.2, gap_before: from.3 });
            prev_end = Some(n.end);
        }
    }
    segs.sort_by_key(|g| g.start);
    // A clip's first note: the gap before it is from the note before it on the timeline (a comp
    // piece right after another sings on).
    let gap = s.sample_rate.samples(GAP_SECS);
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());
    for g in segs {
        if let Some(p) = out.last() {
            if g.start < p.end {
                continue;
            }
            if g.gap_before == FIRST_IN_CLIP {
                let before = g.start - p.end;
                let onset = if before > gap { Onset::Gap } else { Onset::Glide };
                out.push(Seg { onset, gap_before: before, lead: if onset == Onset::Gap { g.lead } else { g.start }, ..g });
                continue;
            }
        }
        out.push(g);
    }
    (out, breaths)
}

/// Where word edges of `track` snap: starts to where a word can begin (a note's consonant, else
/// its start, and a breath's end), ends to where the voice stops (a note's end, a breath's start).
pub fn snap_points(s: &Session, track: TrackId) -> (Vec<Samples>, Vec<Samples>) {
    let (segs, breaths) = track_segs(s, track);
    let mut starts: Vec<Samples> = segs.iter().map(|g| g.lead).chain(breaths.iter().map(|b| b.1)).collect();
    let mut ends: Vec<Samples> = segs.iter().map(|g| g.end).chain(breaths.iter().map(|b| b.0)).collect();
    for v in [&mut starts, &mut ends] {
        v.sort_unstable();
        v.dedup();
    }
    (starts, ends)
}

/// `t` moved to the nearest of `points` within `window`, else unchanged.
pub fn snap_to(t: Samples, points: &[Samples], window: Samples) -> Samples {
    points.iter().copied().filter(|p| (p - t).abs() <= window).min_by_key(|p| (p - t).abs()).unwrap_or(t)
}

/// Makes sure every audio clip of `track` is analysed (blocking).
pub fn analyse_track(e: &mut crate::Engine, track: TrackId) -> usize {
    let ids: Vec<ClipId> = e.session().track(track).map(|t| t.clips().iter().filter(|c| c.is_audio()).map(|c| c.id).collect()).unwrap_or_default();
    crate::vocal::analyse_now(e, &ids)
}

// ---- Time Words to Vocal -----------------------------------------------------------------------

/// A sheet line's words as typed (chords left out).
pub fn sheet_words(s: &Session, line: u64) -> Vec<String> {
    s.lyric_sheet
        .lines
        .iter()
        .find(|l| l.id == line)
        .map(|l| soundcraft_model::sung_text(&l.text).split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The notes each placed line owns: notes starting in its range (from a little before its marker,
/// up to where the next line's marker starts). Lines in time order: `(line id, its range)`.
pub fn owned_segs(segs: &[Seg], lines: &[(u64, Range)], rate: f64) -> Vec<(u64, Vec<Seg>)> {
    let early = (0.12 * rate).round() as Samples;
    let mut out = Vec::new();
    for (i, (id, r)) in lines.iter().enumerate() {
        let next = lines.get(i + 1).map_or(Samples::MAX, |(_, n)| n.start - early);
        let (lo, hi) = (r.start - early, (r.end + early).min(next));
        out.push((*id, segs.iter().filter(|g| g.start >= lo && g.start < hi).copied().collect()));
    }
    out
}

/// One line's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LineTimed {
    pub line: u64,
    pub words: usize,
    pub notes: usize,
    /// Words that had to share a note with a neighbour.
    pub shared: usize,
    /// Why the line was left alone, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// Removes the words of `track`'s playing clips that belong to sheet line `line`, or start in
/// `range` and are not confirmed (all of them with `force`).
fn clear_for_line(s: &mut Session, track: TrackId, line: u64, range: Range, force: bool) {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return };
    for c in &mut pl.clips {
        let b = base(c);
        let start = c.start;
        for l in &mut c.lyrics {
            let ours = l.sheet_line == Some(line);
            l.words.retain(|w| {
                let at = start.saturating_add(w.start.saturating_sub(b));
                let in_range = at >= range.start && at < range.end;
                let replaceable = force || w.timing != WordTiming::Confirmed;
                !((ours || in_range) && replaceable)
            });
        }
        sanitize_lyrics(&mut c.lyrics);
    }
}

/// Writes words (timeline times) as sheet line `line` onto the playing clips of `track` under
/// each word's start (a line crossing clips becomes one piece per clip). Words over no clip are
/// dropped. Returns how many were placed.
pub fn put_words(s: &mut Session, track: TrackId, line: Option<u64>, words: &[(Samples, Samples, String)], timing: WordTiming) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut pieces: Vec<(usize, Vec<LyricWord>)> = Vec::new();
    for (a, b, text) in words {
        let Some(ci) = pl.clips.iter().position(|c| !c.is_video() && !c.muted && c.range().contains(*a)) else { continue };
        let Some(c) = pl.clips.get(ci) else { continue };
        let w = LyricWord { start: to_clip_time(c, *a), end: to_clip_time(c, (*b).min(c.end()).max(*a)), text: text.clone(), timing };
        match pieces.last_mut() {
            Some((i, v)) if *i == ci => v.push(w),
            _ => pieces.push((ci, vec![w])),
        }
    }
    let mut n = 0;
    for (ci, ws) in pieces {
        let Some(c) = pl.clips.get_mut(ci) else { continue };
        let (a, b) = (ws.first().map_or(0, |w| w.start), ws.iter().map(|w| w.end).max().unwrap_or(0));
        n += ws.len();
        c.lyrics.push(LyricLine { start: a, end: b, words: ws, sheet_line: line });
        sanitize_lyrics(&mut c.lyrics);
    }
    n
}

/// Times the placed sheet lines `lines` (all placed lines when empty) on `track`: each line's
/// words spread over the notes it owns ([`crate::word_align::fit_words`]), as guesses. A line
/// with a confirmed word in its range is left alone unless `force`. The track's clips must be
/// analysed ([`analyse_track`]).
pub fn time_lines(s: &mut Session, track: TrackId, lines: &[u64], force: bool) -> Vec<LineTimed> {
    crate::lexicon::wait();
    let rate = s.sample_rate.as_f64();
    let (segs, _) = track_segs(s, track);
    let mut placed: Vec<(u64, Range)> = s.lyric_sheet.placed.iter().map(|p| (p.line, Range::new(p.start, p.end))).collect();
    placed.sort_by_key(|(_, r)| r.start);
    let owned = owned_segs(&segs, &placed, rate);
    let mut out = Vec::new();
    for (id, own) in owned {
        if !lines.is_empty() && !lines.contains(&id) {
            continue;
        }
        let words = sheet_words(s, id);
        let mut res = LineTimed { line: id, words: 0, notes: own.len(), shared: 0, skipped: None };
        if words.is_empty() {
            res.skipped = Some("the line has no words".into());
            out.push(res);
            continue;
        }
        if own.is_empty() {
            res.skipped = Some("no singing found in its range on this track".into());
            out.push(res);
            continue;
        }
        let span = Range::new(own.first().map_or(0, |g| g.lead.min(g.start)), own.last().map_or(0, |g| g.end));
        let mine = track_words(s, track);
        let hand =
            mine.iter().any(|w| w.timing == WordTiming::Confirmed && (w.sheet_line == Some(id) || w.start >= span.start && w.start < span.end));
        if hand && !force {
            res.skipped = Some("it has hand-timed words (time it again with force to replace them)".into());
            out.push(res);
            continue;
        }
        let syl: Vec<u32> = words.iter().map(|w| crate::word_align::sung_syllables(w)).collect();
        let fits: Vec<Fit> = crate::word_align::fit_words(&syl, &own, rate);
        clear_for_line(s, track, id, span, force);
        let ws: Vec<(Samples, Samples, String)> = fits.iter().zip(&words).map(|(f, w)| (f.start, f.end, w.clone())).collect();
        res.words = put_words(s, track, Some(id), &ws, WordTiming::Guess);
        res.shared = fits.iter().filter(|f| f.shared).count();
        out.push(res);
    }
    out
}

// ---- Tapping -----------------------------------------------------------------------------------

/// One tap: key down, and key up when the key was held for the word's length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tap {
    pub down: Samples,
    pub up: Option<Samples>,
}

/// Turns taps (already latency-compensated, timeline) into word times: each start snapped to the
/// nearest note start (or consonant before it) within `snap` samples (none: raw); each end at
/// the key-up (snapped to a note end), else where the voice stops before the next word's start
/// (the last note that starts before it), else 300 ms on.
pub fn tap_times(taps: &[Tap], segs: &[Seg], snap: Option<Samples>, rate: f64) -> Vec<(Samples, Samples)> {
    let starts: Vec<Samples> = segs.iter().map(|g| g.lead).collect();
    let ends: Vec<Samples> = segs.iter().map(|g| g.end).collect();
    let min = (MIN_WORD_SECS * rate).round() as Samples;
    let fallback = (0.3 * rate).round() as Samples;
    let mut v: Vec<Samples> = taps.iter().map(|t| snap.map_or(t.down, |w| snap_to(t.down, &starts, w))).collect();
    // Never out of order.
    for i in 1..v.len() {
        let a = v.get(i - 1).copied().unwrap_or(0);
        if let Some(b) = v.get_mut(i)
            && *b < a + min
        {
            *b = a + min;
        }
    }
    let mut out = Vec::with_capacity(v.len());
    for (i, t) in taps.iter().enumerate() {
        let Some(&start) = v.get(i) else { continue };
        let next = v.get(i + 1).copied().unwrap_or(Samples::MAX);
        let end = match t.up {
            Some(u) => snap.map_or(u, |w| snap_to(u, &ends, w)),
            None => segs.iter().filter(|g| g.start >= start - min && g.start < next).map(|g| g.end).max().unwrap_or(start + fallback),
        };
        out.push((start, end.min(next).max(start + min)));
    }
    out
}
