//! Segmentation of a [`PitchTrack`] into notes (port of the spike's `segment_notes2`), scoop and
//! unvoiced classification (the mockup's `build.py` rule), split/join, and carrying edits across a
//! re-analysis.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::edit::NoteEdit;
use crate::note::{Note, NoteId, UnvoicedRegion, median};
use crate::track::PitchTrack;

/// A clip's analysis: the pitch track and the notes and unvoiced regions found in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocalAnalysis {
    pub track: PitchTrack,
    pub notes: Vec<Note>,
    pub unvoiced: Vec<UnvoicedRegion>,
}

/// A user's segmentation change, replayed after every re-analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegOp {
    /// Split the note containing frame `at` into `[start, at)` and `[at, end)`.
    Split { at: u32 },
    /// Join the note starting at frame `at` onto the note before it.
    Join { at: u32 },
}

/// How a clip's pitch is cut into notes. Saved per clip. The `Default` is what a tuning saved
/// without a mode means ([`NoteMode::Classic`]: sessions tuned before modes existed sound as
/// they did); new tunings get [`NoteMode::NEW`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum NoteMode {
    /// Semitone steps with hysteresis ([`segment`]): every pitch the voice holds for 70 ms is a
    /// note, a short approach into a long note is its scoop. Tight on a vocal that is near the
    /// notes; on a loose one, slides and scoops become notes of their own. What tunings saved
    /// before modes existed use.
    #[default]
    Classic,
    /// Notes by syllable and settled pitch ([`crate::musical::segment_musical`]): glides, scoops
    /// and vibrato are folded into the note they belong to as its pitch curve, each note is
    /// centred on its stable middle, and the correction moves notes without snapping inside the
    /// glides between them. What new tunings use ([`NoteMode::NEW`]).
    Musical,
}

impl NoteMode {
    /// The mode a new tuning gets (and an untuned clip is shown in): Musical.
    pub const NEW: NoteMode = NoteMode::Musical;

    /// The mode's command/protocol name: `classic` or `musical`.
    pub fn name(self) -> &'static str {
        match self {
            NoteMode::Classic => "classic",
            NoteMode::Musical => "musical",
        }
    }

    /// The mode named `name` (case-insensitive).
    pub fn parse(name: &str) -> Option<NoteMode> {
        match name.trim().to_ascii_lowercase().as_str() {
            "classic" => Some(NoteMode::Classic),
            "musical" => Some(NoteMode::Musical),
            _ => None,
        }
    }
}

/// [`segment`] in `mode`: Classic is [`segment`] itself; Musical is
/// [`crate::musical::segment_musical`]. The unvoiced regions are the same in both.
pub fn segment_with(track: &PitchTrack, mode: NoteMode) -> (Vec<Note>, Vec<UnvoicedRegion>) {
    match mode {
        NoteMode::Classic => segment(track),
        NoteMode::Musical => crate::musical::segment_musical(track),
    }
}

/// Median filter kernel (frames) applied inside each voiced run (scipy `medfilt`, zero-padded).
const MEDFILT: usize = 9;
/// Frames used for the initial semitone of a voiced run.
const INITIAL_FRAMES: usize = 10;
/// Re-quantise only when the smoothed pitch is further than this from the current semitone (cents).
const HYSTERESIS_CENTS: f64 = 70.0;
/// Runs shorter than this are absorbed into their longer neighbour (`min_len` 70 ms).
const MIN_RUN_FRAMES: usize = 14;
/// Runs kept as notes: at least `max(4, MIN_RUN_FRAMES / 2)` frames.
const KEEP_FRAMES: usize = if MIN_RUN_FRAMES / 2 > 4 { MIN_RUN_FRAMES / 2 } else { 4 };
/// A scoop is a note of at most this many frames (90 ms)...
const SCOOP_MAX_FRAMES: u32 = 18;
/// ...ending less than this many frames (20 ms) before the next note's start...
const SCOOP_GAP_FRAMES: u32 = 4;
/// ...when the next note is longer than this (300 ms).
const SCOOP_NEXT_MIN_FRAMES: u32 = 60;
/// Unvoiced frames louder than this belong to an [`UnvoicedRegion`] (dB).
const UNVOICED_MIN_DB: f32 = -60.0;

/// Splits `track` into notes (with scoops attached) and unvoiced regions, both in frame order.
pub fn segment(track: &PitchTrack) -> (Vec<Note>, Vec<UnvoicedRegion>) {
    let spans = segment_spans(track);

    // Scoops: a short note that leads straight into a long one becomes that note's approach.
    let mut notes: Vec<Note> = Vec::with_capacity(spans.len());
    let mut k = 0;
    while k < spans.len() {
        let (a, b) = spans[k];
        if let Some(&(na, nb)) = spans.get(k + 1)
            && b - a <= SCOOP_MAX_FRAMES
            && na.saturating_sub(b) < SCOOP_GAP_FRAMES
            && nb - na > SCOOP_NEXT_MIN_FRAMES
        {
            notes.push(Note::new(track, na, nb, Some((a, na))));
            k += 2;
            continue;
        }
        notes.push(Note::new(track, a, b, None));
        k += 1;
    }

    (notes, unvoiced_regions(track))
}

/// The runs of unvoiced frames that still carry level (louder than −60 dB), in frame order.
pub(crate) fn unvoiced_regions(track: &PitchTrack) -> Vec<UnvoicedRegion> {
    let mut unvoiced = Vec::new();
    let mut run: Option<u32> = None;
    for (i, f) in track.frames.iter().enumerate() {
        let i = frame_u32(i);
        let quiet_unvoiced = f.cents.is_none() && f.level_db > UNVOICED_MIN_DB;
        match (quiet_unvoiced, run) {
            (true, None) => run = Some(i),
            (false, Some(s)) => {
                unvoiced.push(UnvoicedRegion { start: s, end: i });
                run = None;
            }
            _ => {}
        }
    }
    if let Some(s) = run {
        unvoiced.push(UnvoicedRegion { start: s, end: frame_u32(track.frames.len()) });
    }
    unvoiced
}

/// `segment_notes2`: note spans `[a, b)` in frames.
pub(crate) fn segment_spans(track: &PitchTrack) -> Vec<(u32, u32)> {
    let c: Vec<Option<f64>> = track.frames.iter().map(|f| f.cents.filter(|x| x.is_finite()).map(f64::from)).collect();
    let n = c.len();
    let mut notes = Vec::new();
    let mut i = 0;
    while i < n {
        if c[i].is_none() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && c[j].is_some() {
            j += 1;
        }
        let seg: Vec<f64> = c[i..j].iter().flatten().copied().collect();
        let sm = if seg.len() >= MEDFILT { medfilt(&seg, MEDFILT) } else { seg };

        // Quantise to semitones with hysteresis.
        let first = sm.get(..sm.len().min(INITIAL_FRAMES)).unwrap_or(&[]);
        let mut cur = (median(first).unwrap_or(0.0) / 100.0).round_ties_even();
        let q: Vec<i64> = sm
            .iter()
            .map(|&x| {
                if (x - cur * 100.0).abs() > HYSTERESIS_CENTS {
                    cur = (x / 100.0).round_ties_even();
                }
                cur as i64
            })
            .collect();

        // Runs of equal semitone: (start, end, semitone).
        let mut runs: Vec<(usize, usize, i64)> = Vec::new();
        let mut a = 0;
        for k in 1..=q.len() {
            if k == q.len() || q[k] != q[a] {
                runs.push((a, k, q[a]));
                a = k;
            }
        }

        // Absorb the shortest run into its longer neighbour while it is shorter than the minimum.
        // Departure from the spike: when that leaves two neighbours on the same semitone (a blip
        // inside a held note), they are merged, so the note is not cut at the blip.
        while runs.len() > 1 {
            let lens: Vec<usize> = runs.iter().map(|r| r.1 - r.0).collect();
            let Some((k, &len)) = lens.iter().enumerate().min_by_key(|&(_, l)| *l) else { break };
            if len >= MIN_RUN_FRAMES {
                break;
            }
            let last = runs.len() - 1;
            if k == 0 {
                runs[1].0 = runs[0].0;
            } else if k == last {
                runs[last - 1].1 = runs[last].1;
            } else if lens[k - 1] >= lens[k + 1] {
                runs[k - 1].1 = runs[k].1;
            } else {
                runs[k + 1].0 = runs[k].0;
            }
            runs.remove(k);
            if k > 0 && k < runs.len() && runs[k - 1].2 == runs[k].2 {
                runs[k - 1].1 = runs[k].1;
                runs.remove(k);
            }
        }

        for (a, b, _) in runs {
            if b - a >= KEEP_FRAMES {
                notes.push((frame_u32(i + a), frame_u32(i + b)));
            }
        }
        i = j;
    }
    notes
}

/// scipy `medfilt` (odd kernel, zero padding at both ends).
pub(crate) fn medfilt(x: &[f64], kernel: usize) -> Vec<f64> {
    let h = kernel / 2;
    let mut win = Vec::with_capacity(kernel);
    (0..x.len())
        .map(|k| {
            win.clear();
            for j in 0..kernel {
                let idx = (k + j).checked_sub(h);
                win.push(idx.and_then(|i| x.get(i)).copied().unwrap_or(0.0));
            }
            win.sort_by(f64::total_cmp);
            win.get(h).copied().unwrap_or(0.0)
        })
        .collect()
}

pub(crate) fn frame_u32(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// A note over `[start, end)` measured as `mode` measures notes (Musical: the stability-weighted
/// centre, drift and vibrato over the core).
pub fn note_in(track: &PitchTrack, start: u32, end: u32, mode: NoteMode) -> Note {
    match mode {
        NoteMode::Classic => Note::new(track, start, end, None),
        NoteMode::Musical => crate::musical::musical_note(track, start, end),
    }
}

/// [`Note::remeasure`] as `mode` measures notes.
fn remeasure_in(n: &mut Note, track: &PitchTrack, mode: NoteMode) {
    n.remeasure(track);
    if mode == NoteMode::Musical {
        crate::musical::remeasure_musical(n, track);
    }
}

/// Splits note `id` at frame `at` into `[start, at)` (keeps `id` and the scoop) and `[at, end)`
/// (id `NoteId(at)`). Both halves get the parent's edit. `None` (and no change) when the note is
/// missing or `at` is not strictly inside it.
pub fn split_note(a: &mut VocalAnalysis, edits: &mut BTreeMap<NoteId, NoteEdit>, id: NoteId, at: u32) -> Option<(NoteId, NoteId)> {
    split_note_with(a, edits, id, at, NoteMode::Classic)
}

/// [`split_note`] on notes made in `mode` (the halves are measured as `mode` measures notes).
pub fn split_note_with(
    a: &mut VocalAnalysis,
    edits: &mut BTreeMap<NoteId, NoteEdit>,
    id: NoteId,
    at: u32,
    mode: NoteMode,
) -> Option<(NoteId, NoteId)> {
    let k = a.notes.iter().position(|n| n.id == id)?;
    let parent = a.notes.get(k)?;
    if at <= parent.start || at >= parent.end {
        return None;
    }
    let second_id = NoteId(at);
    if a.notes.iter().any(|n| n.id == second_id) {
        return None;
    }
    let end = parent.end;
    let mut second = note_in(&a.track, at, end, mode);
    second.id = second_id;
    let first = a.notes.get_mut(k)?;
    first.end = at;
    remeasure_in(first, &a.track, mode);
    a.notes.insert(k + 1, second);
    if let Some(e) = edits.get(&id).cloned() {
        edits.insert(second_id, e);
    }
    Some((id, second_id))
}

/// Joins `second` onto `first` (they must be neighbours in that order): the joined note spans
/// `[first.start, second.end)`, keeps `first`'s id, scoop and edit, and `second`'s edit is dropped.
/// `None` (and no change) when either is missing or they are not neighbours.
///
/// `second`'s scoop is not kept as a scoop: its frames (and any unvoiced gap between the notes)
/// become part of the joined note's body, whose pitch is filled across the gap for editing.
pub fn join_notes(a: &mut VocalAnalysis, edits: &mut BTreeMap<NoteId, NoteEdit>, first: NoteId, second: NoteId) -> Option<NoteId> {
    join_notes_with(a, edits, first, second, NoteMode::Classic)
}

/// [`join_notes`] on notes made in `mode` (the joined note is measured as `mode` measures notes).
pub fn join_notes_with(
    a: &mut VocalAnalysis,
    edits: &mut BTreeMap<NoteId, NoteEdit>,
    first: NoteId,
    second: NoteId,
    mode: NoteMode,
) -> Option<NoteId> {
    let k = a.notes.iter().position(|n| n.id == first)?;
    if a.notes.get(k + 1)?.id != second {
        return None;
    }
    let removed = a.notes.remove(k + 1);
    let joined = a.notes.get_mut(k)?;
    joined.end = removed.end;
    remeasure_in(joined, &a.track, mode);
    edits.remove(&second);
    Some(first)
}

/// Replays `ops` in order on a fresh analysis; ops that no longer apply are skipped.
pub fn apply_seg_ops(a: &mut VocalAnalysis, ops: &[SegOp]) {
    apply_seg_ops_with(a, ops, NoteMode::Classic);
}

/// [`apply_seg_ops`] on notes made in `mode`.
pub fn apply_seg_ops_with(a: &mut VocalAnalysis, ops: &[SegOp], mode: NoteMode) {
    let mut scratch = BTreeMap::new();
    for op in ops {
        match *op {
            SegOp::Split { at } => {
                if let Some(id) = a.notes.iter().find(|n| n.start < at && at < n.end).map(|n| n.id) {
                    split_note_with(a, &mut scratch, id, at, mode);
                }
            }
            SegOp::Join { at } => {
                if let Some(k) = a.notes.iter().position(|n| n.id == NoteId(at))
                    && let Some(prev) = k.checked_sub(1).and_then(|p| a.notes.get(p)).map(|n| n.id)
                {
                    join_notes_with(a, &mut scratch, prev, NoteId(at), mode);
                }
            }
        }
    }
}

/// Moves edits from `old` notes onto `new` ones: each new note takes the edit of the old note it
/// overlaps most (the earlier one on a tie), when that overlap is at least half the new note.
pub fn carry_edits(old: &[Note], edits: &BTreeMap<NoteId, NoteEdit>, new: &[Note]) -> BTreeMap<NoteId, NoteEdit> {
    let mut out = BTreeMap::new();
    for n in new {
        let len = n.end.saturating_sub(n.start);
        if len == 0 {
            continue;
        }
        let mut best: Option<(&Note, u32)> = None;
        for o in old {
            let ov = n.end.min(o.end).saturating_sub(n.start.max(o.start));
            if ov > 0 && best.is_none_or(|(_, b)| ov > b) {
                best = Some((o, ov));
            }
        }
        if let Some((o, ov)) = best
            && u64::from(ov) * 2 >= u64::from(len)
            && let Some(e) = edits.get(&o.id)
        {
            out.insert(n.id, e.clone());
        }
    }
    out
}
