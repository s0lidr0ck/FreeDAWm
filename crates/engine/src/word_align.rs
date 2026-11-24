//! Spreading a typed lyric line's words over the notes sung in its time (Time Words to Vocal):
//! no speech model, only the vocal analysis (notes, where the voice stops, re-sung syllables) and
//! each word's syllable count.
//!
//! The notes inside a line's range become [`Seg`]s, each knowing what comes before it: a gap (the
//! voice stopped: a consonant or a breath), a level dip (a syllable sung again on the same pitch)
//! or only a change of pitch (legato: likely the same syllable moving, a melisma). [`fit_words`]
//! then places the words in order by dynamic programming over half-notes:
//!
//! - every word takes at least half a note and the line's notes are used up in order;
//! - a word starting after a gap costs nothing, after a dip a little, on a pitch change more, and
//!   two words sharing one note (a split in its middle) the most;
//! - a word sung over fewer notes than it has syllables costs per missing syllable, over more
//!   (a melisma) a little per extra note, so a word's last syllable absorbs the extra notes until
//!   the next word's onset;
//! - more gaps inside a word than its syllables explain cost, and a breath inside a word costs a
//!   lot.
//!
//! A word starts at its first note, or at the short unvoiced sound just before it (a consonant,
//! at most [`LEAD_MAX`]); it ends where its last note's voice stops, never at the next word's
//! start. A longer unvoiced stretch is a breath and is never part of a word.

use soundcraft_time::Samples;

/// The most a consonant before a note can be (seconds): longer unvoiced sound is a breath.
pub const LEAD_MAX: f64 = 0.15;
/// A silence or unvoiced stretch at least this long (seconds) inside a word is a breath.
pub const BREATH_MIN: f64 = 0.25;

/// What comes just before a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Onset {
    /// The voice stopped (silence, a consonant or a breath): a word very likely starts here.
    Gap,
    /// The level dips and comes back (a syllable sung again on the same pitch).
    Dip,
    /// Only the pitch moves (legato): usually the same syllable going on.
    Glide,
}

/// One sung note of a line, in session samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seg {
    pub start: Samples,
    pub end: Samples,
    pub onset: Onset,
    /// Where a word starting on this note starts: the note's start, or earlier by the consonant
    /// just before it.
    pub lead: Samples,
    /// The silence or unvoiced stretch before the note (samples; 0 when it follows straight on).
    pub gap_before: Samples,
}

/// Where a word landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub start: Samples,
    pub end: Samples,
    /// How many notes (or parts of notes) it covers.
    pub notes: usize,
    /// It shares a note with a neighbour (two words on one note).
    pub shared: bool,
}

/// Cost of a word starting after a gap, a dip, a pitch change, and inside a note: a long one
/// (two words held on one note), else a short one (less likely than a pitch change).
const ONSET_COST: [f64; 3] = [0.0, 1.0, 3.0];
const SPLIT_COST: f64 = 2.0;
const SPLIT_SHORT_COST: f64 = 4.0;
/// A note at least this long (seconds) can carry two words.
const LONG_NOTE: f64 = 0.4;
/// Per syllable the word has more than its notes, per note more than its syllables.
const MISSING_COST: f64 = 2.5;
const EXTRA_COST: f64 = 0.5;
/// Per gap inside a word beyond what its syllables explain; per breath inside a word.
const INNER_GAP_COST: f64 = 2.0;
const INNER_BREATH_COST: f64 = 8.0;

fn onset_cost(o: Onset) -> f64 {
    match o {
        Onset::Gap => ONSET_COST[0],
        Onset::Dip => ONSET_COST[1],
        Onset::Glide => ONSET_COST[2],
    }
}

/// Splits the longest notes in two until there are at least half as many notes as words (each
/// word needs at least half a note). Returns the notes and which of them are halves of one.
fn enough_segs(segs: &[Seg], words: usize) -> Vec<(Seg, bool)> {
    let mut v: Vec<(Seg, bool)> = segs.iter().map(|s| (*s, false)).collect();
    while !v.is_empty() && v.len() * 2 < words {
        let Some((i, _)) = v.iter().enumerate().max_by_key(|(_, (s, _))| s.end - s.start) else { break };
        let Some((s, _)) = v.get(i).copied() else { break };
        if s.end - s.start < 2 {
            break;
        }
        let mid = s.start + (s.end - s.start) / 2;
        let a = Seg { end: mid, ..s };
        let b = Seg { start: mid, lead: mid, onset: Onset::Glide, gap_before: 0, ..s };
        v.splice(i..=i, [(a, true), (b, true)]);
    }
    v
}

/// Places `syllables.len()` words (each with its syllable count) over `segs` (a line's notes in
/// time order, not overlapping), at `rate` Hz. Empty when there are no notes or no words.
pub fn fit_words(syllables: &[u32], segs: &[Seg], rate: f64) -> Vec<Fit> {
    let n = syllables.len();
    if n == 0 || segs.is_empty() {
        return Vec::new();
    }
    let segs = enough_segs(segs, n);
    let m = segs.len();
    let h = 2 * m;
    let breath = (BREATH_MIN * rate).round() as Samples;
    let seg = |i: usize| segs.get(i).map(|s| s.0);
    // Cost of a word with `syl` syllables over half-notes [a, b).
    let word_cost = |syl: u32, a: usize, b: usize, first: bool| -> f64 {
        let mut c = 0.0;
        if !first {
            c += match seg(a / 2) {
                Some(s) if a % 2 == 1 => {
                    if (s.end - s.start) as f64 >= LONG_NOTE * rate {
                        SPLIT_COST
                    } else {
                        SPLIT_SHORT_COST
                    }
                }
                Some(s) => onset_cost(s.onset),
                None => 0.0,
            };
        }
        let notes = b.div_ceil(2) - a / 2;
        let syl = syl.max(1) as usize;
        if notes < syl {
            c += (syl - notes) as f64 * MISSING_COST;
        } else {
            c += (notes - syl) as f64 * EXTRA_COST;
        }
        // Gaps inside: at even half-positions strictly inside (a, b).
        let (mut gaps, mut breaths) = (0usize, 0usize);
        let mut k = a + 1;
        while k < b {
            if k.is_multiple_of(2)
                && let Some(s) = seg(k / 2)
                && s.onset == Onset::Gap
            {
                gaps += 1;
                if s.gap_before >= breath {
                    breaths += 1;
                }
            }
            k += 1;
        }
        c += gaps.saturating_sub(syl - 1) as f64 * INNER_GAP_COST + breaths as f64 * INNER_BREATH_COST;
        c
    };
    // dp[i][j]: the first i words over the first j half-notes.
    let inf = f64::INFINITY;
    let mut dp = vec![vec![inf; h + 1]; n + 1];
    let mut back = vec![vec![0usize; h + 1]; n + 1];
    if let Some(row) = dp.get_mut(0)
        && let Some(c) = row.get_mut(0)
    {
        *c = 0.0;
    }
    for i in 1..=n {
        let syl = syllables.get(i - 1).copied().unwrap_or(1);
        for j in 1..=h {
            // The rest of the words need at least one half-note each.
            if h - j < n - i {
                continue;
            }
            let mut best = (inf, 0usize);
            for a in (i - 1)..j {
                let prev = dp.get(i - 1).and_then(|r| r.get(a)).copied().unwrap_or(inf);
                if !prev.is_finite() {
                    continue;
                }
                let c = prev + word_cost(syl, a, j, i == 1);
                if c < best.0 {
                    best = (c, a);
                }
            }
            if let (Some(d), Some(bk)) = (dp.get_mut(i).and_then(|r| r.get_mut(j)), back.get_mut(i).and_then(|r| r.get_mut(j))) {
                *d = best.0;
                *bk = best.1;
            }
        }
    }
    // Walk back the word boundaries.
    let mut bounds = vec![0usize; n + 1];
    if let Some(b) = bounds.get_mut(n) {
        *b = h;
    }
    let mut j = h;
    for i in (1..=n).rev() {
        let a = back.get(i).and_then(|r| r.get(j)).copied().unwrap_or(0);
        if let Some(b) = bounds.get_mut(i - 1) {
            *b = a;
        }
        j = a;
    }
    // A half-note position as a time: even = a note's start (its lead for a word start), odd =
    // the middle of a note.
    let mid = |s: &Seg| s.start + (s.end - s.start) / 2;
    let mut out = Vec::with_capacity(n);
    for w in 0..n {
        let (a, b) = (bounds.get(w).copied().unwrap_or(0), bounds.get(w + 1).copied().unwrap_or(h));
        let Some(first) = seg(a / 2) else { continue };
        let Some(last) = seg(b.saturating_sub(1) / 2) else { continue };
        let start = if a % 2 == 1 { mid(&first) } else { first.lead.min(first.start) };
        let end = if b % 2 == 1 { mid(&last) } else { last.end };
        let halves = segs.iter().skip(a / 2).take(b.div_ceil(2) - a / 2).any(|(_, half)| *half);
        let shared = a % 2 == 1 || b % 2 == 1 || halves;
        out.push(Fit { start, end: end.max(start + 1), notes: b.div_ceil(2) - a / 2, shared });
    }
    out
}

/// A word's syllables as sung (the dictionary, else spelling rules), at least 1.
pub fn sung_syllables(word: &str) -> u32 {
    let w: String = soundcraft_model::sung_text(word).chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect();
    crate::lyric_text::syllables(&w).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f64 = 1000.0;

    fn seg(start: Samples, end: Samples, onset: Onset) -> Seg {
        Seg { start, end, onset, lead: start, gap_before: if onset == Onset::Gap { 50 } else { 0 } }
    }

    #[test]
    fn one_note_per_syllable_lands_exactly() {
        let segs = [seg(0, 100, Onset::Gap), seg(150, 250, Onset::Gap), seg(300, 400, Onset::Gap), seg(450, 600, Onset::Gap)];
        let f = fit_words(&[1, 2, 1], &segs, R);
        assert_eq!(f.iter().map(|f| (f.start, f.end)).collect::<Vec<_>>(), vec![(0, 100), (150, 400), (450, 600)]);
        assert!(f.iter().all(|f| !f.shared));
    }

    #[test]
    fn a_melisma_stays_on_its_word_and_words_start_after_gaps() {
        // "Lord" over three legato notes, then "say" after a gap.
        let segs = [seg(0, 100, Onset::Gap), seg(100, 200, Onset::Glide), seg(200, 300, Onset::Glide), seg(400, 500, Onset::Gap)];
        let f = fit_words(&[1, 1], &segs, R);
        assert_eq!(f.iter().map(|f| (f.start, f.end, f.notes)).collect::<Vec<_>>(), vec![(0, 300, 3), (400, 500, 1)]);
    }

    #[test]
    fn two_words_share_a_note_when_notes_run_short() {
        let segs = [seg(0, 400, Onset::Gap), seg(500, 600, Onset::Gap)];
        let f = fit_words(&[1, 1, 1], &segs, R);
        assert_eq!(f.iter().map(|f| (f.start, f.end, f.shared)).collect::<Vec<_>>(), vec![(0, 200, true), (200, 400, true), (500, 600, false)]);
        // Many words on one note: the note is cut until each has a part.
        let f = fit_words(&[1, 1, 1, 1, 1], &[seg(0, 1000, Onset::Gap)], R);
        assert_eq!(f.len(), 5);
        assert!(f.windows(2).all(|w| w[0].end <= w[1].start && w[0].start < w[0].end));
    }

    #[test]
    fn no_word_starts_inside_a_breath() {
        let mut b = seg(800, 900, Onset::Gap);
        b.gap_before = 400;
        let segs = [seg(0, 100, Onset::Gap), seg(100, 200, Onset::Glide), b];
        // Two one-syllable words: the second starts after the breath, never across it.
        let f = fit_words(&[1, 1], &segs, R);
        assert_eq!(f.iter().map(|f| f.start).collect::<Vec<_>>(), vec![0, 800]);
        assert_eq!(fit_words(&[1], &[], R), vec![]);
        assert_eq!(fit_words(&[], &segs, R), vec![]);
    }

    #[test]
    fn syllables_of_sung_words() {
        crate::lexicon::wait();
        assert_eq!(sung_syllables("Amen,"), 2);
        assert_eq!(sung_syllables("[G]whatever"), 3);
        assert_eq!(sung_syllables("&"), 1);
    }
}
