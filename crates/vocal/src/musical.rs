//! Musical note detection ([`crate::NoteMode::Musical`]) and its correction curves.
//!
//! Classic segmentation ([`crate::segment()`]) makes a note of every pitch held for 70 ms. On a
//! vocal that lands near its notes that is exact; on a loose one, the scoops into notes, the
//! slides between them and wide vibrato become notes of their own, and centring each of those on
//! a semitone turns a slide into a staircase. Musical mode cuts notes the way a singer phrases
//! them:
//!
//! 1. **Phrases**: voiced runs, bridging dropouts of up to 3 frames (15 ms).
//! 2. **Pieces**: Classic's semitone runs inside each phrase, widened to tile it, and cut again at
//!    strong level dips (a re-sung syllable on the same pitch, 9 dB or more).
//! 3. **Vibrato**: runs of pieces swinging up and down around one pitch (each middle one at most
//!    150 ms, every other one within 60 c) are one piece: wide vibrato across a semitone line.
//! 4. **Folding**: a piece shorter than 70 ms, a glide of up to 200 ms (it moves 50 c or more and
//!    never holds still for 50 ms), and a piece under 100 ms that sits between its neighbours'
//!    pitches or starts or ends a phrase next to a note twice its length, is folded into a
//!    neighbour as its pitch curve: into the next note (a scoop) unless it ends a syllable or
//!    stays much nearer the note before (a fall off it).
//!    Neighbours then within 45 c of each other with no syllable between them are one note
//!    (drift across a semitone line).
//! 5. **Centre and core**: each note is centred on the stability-weighted median of its pitch
//!    (frames where the pitch moves fast count for little), and its core is the stretch whose
//!    smoothed pitch stays within 60 c of that centre: the sung note, with the glides either side.
//!
//! The correction ([`build_curves_musical`]) moves each edited note by
//! `centre·(target − centre) + offset` as a whole, glides included, removes Drift and scales
//! Vibrato over its core only, and in the glide between two touching notes blends from one move
//! to the other by how far the sung pitch has travelled between them: the slide keeps its shape
//! and lands on both tuned notes, with no step inside it. A note's Transition shapes the glide
//! into it (Smooth: a straight line between the tuned notes; Shorten: the sung glide at double
//! speed).

use std::collections::BTreeMap;

use crate::edit::{
    EditCurves, MAX_SHIFT_CENTS, MAX_TOTAL_SHIFT_CENTS, MIN_VOICED_FRAMES, NoteEdit, ScaleAt, TAPER_FRAMES, Transition, decompose64, gain_curve,
    hann_kernel, note_pitch,
};
use crate::note::{Note, NoteId, UnvoicedRegion};
use crate::segment::{NoteMode, VocalAnalysis, frame_u32, medfilt, segment_spans, unvoiced_regions};
use crate::track::PitchTrack;

/// Unvoiced dropouts this short (frames, 15 ms) inside a phrase are bridged.
const BRIDGE_FRAMES: usize = 3;
/// Phrases shorter than this (frames, 35 ms) are no note (Classic's `KEEP_FRAMES`).
const MIN_PHRASE_FRAMES: usize = 7;
/// Pieces shorter than this (frames, 70 ms) are never a note of their own.
const SHORT_FRAMES: usize = 14;
/// A glide is at most this long (frames, 200 ms)...
const GLIDE_MAX_FRAMES: usize = 40;
/// ...moves at least this far from its first to its last quarter (cents)...
const GLIDE_MIN_CENTS: f64 = 50.0;
/// ...and never holds still (a run of steady frames) for this many frames (50 ms).
const PLATEAU_FRAMES: usize = 10;
/// A frame is steady when the pitch moves less than this (cents) over ±3 frames.
const STEADY_CENTS: f64 = 18.0;
const STEADY_HALF: usize = 3;
/// A piece shorter than this (frames, 100 ms) is no note of its own either when it sits between
/// its neighbours' pitches (a passing step of a slide), or starts or ends a phrase next to a note
/// at least twice as long (an approach or a fall that holds still for a moment).
const PASSING_FRAMES: usize = 20;
/// "Between" means at least this far inside the neighbours' centres (cents).
const BETWEEN_CENTS: f64 = 25.0;
/// Vibrato: pieces alternating up and down, each middle one at most this long (frames, 150 ms)...
const VIBRATO_PIECE_FRAMES: usize = 30;
/// ...every other one within this of the one two before (cents)...
const VIBRATO_SAME_CENTS: f64 = 60.0;
/// ...and neighbours this far apart (cents): one wide vibrato, merged into one note.
const VIBRATO_SWING_CENTS: (f64, f64) = (30.0, 300.0);
/// How much nearer (cents) a transitional piece's start must be to the note before than its end
/// is to the note after for it to fold back into the note before.
const FOLD_NEXT_BIAS_CENTS: f64 = 100.0;
/// Neighbours whose centres are this close (cents) with no syllable between them are one note.
const SAME_PITCH_CENTS: f64 = 45.0;
/// A level dip this deep (dB) under the peaks either side marks a syllable boundary...
const ONSET_DIP_DB: f32 = 6.0;
/// ...and one this deep cuts a piece in two (a re-sung syllable on the same pitch).
const ONSET_SPLIT_DB: f32 = 9.0;
/// The peaks either side of a dip are looked for this far away (frames, 80 ms).
const ONSET_REACH: usize = 16;
/// A boundary this close (frames) to a syllable dip counts as a syllable boundary.
const ONSET_GUARD: usize = 6;
/// The core is where the smoothed pitch is within this of the centre (cents).
const CORE_TOL_CENTS: f64 = 60.0;
/// Moving-average half-width (frames) of the pitch the core is found on (softens vibrato).
const CORE_SMOOTH_HALF: usize = 4;
/// Stability weight: a frame moving this fast (cents per frame) counts half.
const WEIGHT_SLOPE: f64 = 2.0;
/// Glides between touching notes whose ends are closer than this (cents) blend by time, not pitch.
const PITCH_BLEND_MIN_CENTS: f64 = 40.0;
/// Hann smoothing length (frames) of the Musical shift curve, inside each voiced run.
const SMOOTH_TAPS: usize = 5;
/// Frames read at each core edge for the pitch a glide leaves from or arrives at.
const EDGE_FRAMES: usize = 3;
/// Shorten's blend into the tuned pitch (frames before the glide's midpoint).
const SHORTEN_BLEND_FRAMES: usize = 4;

/// Notes in Musical mode and the unvoiced regions (the same as Classic's), both in frame order.
/// No note has a scoop: the approach is part of the note ([`core_range`] tells the sung middle
/// from the glides).
pub fn segment_musical(track: &PitchTrack) -> (Vec<Note>, Vec<UnvoicedRegion>) {
    let notes = musical_spans(track).into_iter().map(|(a, b)| musical_note(track, a, b)).collect();
    (notes, unvoiced_regions(track))
}

/// A Musical note over `[start, end)`: Classic's level, with the stability-weighted centre, and
/// drift and vibrato measured over the core ([`core_range`]).
pub fn musical_note(track: &PitchTrack, start: u32, end: u32) -> Note {
    let mut n = Note::new(track, start, end, None);
    remeasure_musical(&mut n, track);
    n
}

/// Re-takes a Musical note's centre, drift and vibrato from `track` (its level as Classic's).
pub(crate) fn remeasure_musical(n: &mut Note, track: &PitchTrack) {
    if let Some(m) = measure(track, n.start, n.end) {
        n.centre_cents = m.centre as f32;
        n.drift_cents = m.drift as f32;
        n.vibrato_cents = m.vibrato as f32;
    }
}

/// A Musical note's core: frames `[c0, c1)` inside `[note.start, note.end)` where the sung pitch
/// settles (its smoothed pitch within 60 c of the centre). The frames either side are the note's
/// glides. The whole note when it has fewer than 3 voiced frames.
pub fn core_range(track: &PitchTrack, note: &Note) -> (u32, u32) {
    measure(track, note.start, note.end).map_or((note.start, note.end), |m| m.core)
}

/// The voiced cents of frames `[start, end)` from the first to the last voiced frame, interior
/// gaps filled linearly: `(offset of the first voiced frame from start, cents)`; `None` with
/// fewer than 3 voiced frames.
fn filled(track: &PitchTrack, start: usize, end: usize) -> Option<(usize, Vec<f64>)> {
    let frames = track.frames.get(start.min(end)..end.min(track.frames.len()))?;
    let raw: Vec<Option<f64>> = frames.iter().map(|f| f.cents.filter(|c| c.is_finite()).map(f64::from)).collect();
    if raw.iter().flatten().count() < MIN_VOICED_FRAMES {
        return None;
    }
    let first = raw.iter().position(Option::is_some)?;
    let last = raw.iter().rposition(Option::is_some)?;
    Some((first, fill_gaps(raw.get(first..=last)?)))
}

/// `raw` with every gap filled linearly between its voiced neighbours, edges held.
fn fill_gaps(raw: &[Option<f64>]) -> Vec<f64> {
    let mut out = vec![0.0; raw.len()];
    let mut prev: Option<(usize, f64)> = None;
    for (k, v) in raw.iter().enumerate() {
        let Some(v) = *v else { continue };
        match prev {
            Some((lk, lv)) => {
                for (j, o) in out.iter_mut().enumerate().take(k).skip(lk + 1) {
                    *o = lv + (v - lv) * (j - lk) as f64 / (k - lk) as f64;
                }
            }
            None => {
                for o in out.iter_mut().take(k) {
                    *o = v;
                }
            }
        }
        if let Some(o) = out.get_mut(k) {
            *o = v;
        }
        prev = Some((k, v));
    }
    if let Some((lk, lv)) = prev {
        for o in out.iter_mut().skip(lk + 1) {
            *o = lv;
        }
    }
    out
}

/// `x` median-filtered over 5 frames when long enough (spikes out, glides kept).
fn despike(x: &[f64]) -> Vec<f64> {
    if x.len() >= 5 { medfilt(x, 5) } else { x.to_vec() }
}

/// Per frame, how much it counts towards a note's centre: `1 / (1 + (slope / 2 c)²)`, the slope
/// (cents per frame) of the despiked pitch over ±2 frames.
fn stability_weights(x: &[f64]) -> Vec<f64> {
    let m = despike(x);
    let n = m.len();
    (0..n)
        .map(|k| {
            let a = k.saturating_sub(2);
            let b = (k + 2).min(n.saturating_sub(1));
            let span = (b - a).max(1) as f64;
            let slope = (m.get(b).copied().unwrap_or(0.0) - m.get(a).copied().unwrap_or(0.0)).abs() / span;
            1.0 / (1.0 + (slope / WEIGHT_SLOPE).powi(2))
        })
        .collect()
}

/// The weighted median of `x` under `w` (the smallest value whose cumulative weight reaches half).
fn weighted_median(x: &[f64], w: &[f64]) -> Option<f64> {
    let mut v: Vec<(f64, f64)> = x.iter().copied().zip(w.iter().copied()).filter(|(a, b)| a.is_finite() && b.is_finite() && *b > 0.0).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = v.iter().map(|p| p.1).sum();
    let mut acc = 0.0;
    for (val, wt) in &v {
        acc += wt;
        if acc >= total / 2.0 {
            return Some(*val);
        }
    }
    v.last().map(|p| p.0)
}

/// A Musical note's centre from its (gap-filled) pitch: the median weighted by
/// [`stability_weights`], so the held part counts and the glides hardly do. `None` when empty.
pub fn weighted_centre(x: &[f64]) -> Option<f64> {
    weighted_median(x, &stability_weights(x))
}

/// A Musical note's measures.
struct Measured {
    centre: f64,
    core: (u32, u32),
    drift: f64,
    vibrato: f64,
}

fn measure(track: &PitchTrack, start: u32, end: u32) -> Option<Measured> {
    let (off, x) = filled(track, start as usize, end as usize)?;
    let centre = weighted_centre(&x)?;
    let (k0, k1) = core_of(&x, centre);
    let base = start as usize + off;
    let core_x = x.get(k0..k1).unwrap_or(&x);
    let d: Vec<f64> = core_x.iter().map(|v| v - centre).collect();
    let (_, slow, fast) = decompose64(&d);
    Some(Measured { centre, core: (frame_u32(base + k0), frame_u32(base + k1)), drift: pop_std(&slow), vibrato: pop_std(&fast) })
}

/// The core `[k0, k1)` of a note's pitch `x` around `centre`: from the first to the last frame
/// whose moving average (±4 frames) is within 60 c of the centre; all of `x` when fewer than 3
/// frames are.
fn core_of(x: &[f64], centre: f64) -> (usize, usize) {
    let n = x.len();
    let sm: Vec<f64> = (0..n)
        .map(|k| {
            let a = k.saturating_sub(CORE_SMOOTH_HALF);
            let b = (k + CORE_SMOOTH_HALF + 1).min(n);
            let w = x.get(a..b).unwrap_or(&[]);
            if w.is_empty() { centre } else { w.iter().sum::<f64>() / w.len() as f64 }
        })
        .collect();
    let inside = |v: &f64| (v - centre).abs() <= CORE_TOL_CENTS;
    match (sm.iter().position(inside), sm.iter().rposition(inside)) {
        (Some(a), Some(b)) if b + 1 >= a + 3 => (a, b + 1),
        _ => (0, n),
    }
}

fn pop_std(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let m = xs.iter().sum::<f64>() / xs.len() as f64;
    (xs.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / xs.len() as f64).sqrt()
}

/// Voiced phrases `[a, b)`: voiced runs with dropouts of up to [`BRIDGE_FRAMES`] bridged, at
/// least [`MIN_PHRASE_FRAMES`] long.
fn phrases(track: &PitchTrack) -> Vec<(usize, usize)> {
    let voiced: Vec<bool> = track.frames.iter().map(|f| f.cents.is_some_and(f32::is_finite)).collect();
    let n = voiced.len();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        if !voiced.get(i).copied().unwrap_or(false) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && voiced.get(j).copied().unwrap_or(false) {
            j += 1;
        }
        match out.last_mut() {
            Some(last) if i - last.1 <= BRIDGE_FRAMES => last.1 = j,
            _ => out.push((i, j)),
        }
        i = j;
    }
    out.retain(|(a, b)| b - a >= MIN_PHRASE_FRAMES);
    out
}

/// Syllable dips inside `[a, b)`: `(frame, depth dB)` of level minima at least [`ONSET_DIP_DB`]
/// under the highest level on both sides within [`ONSET_REACH`] frames, at least that far apart
/// (the deeper kept).
fn onsets(track: &PitchTrack, a: usize, b: usize) -> Vec<(usize, f32)> {
    let lv: Vec<f32> = (a..b).map(|i| track.frames.get(i).map_or(-120.0, |f| if f.level_db.is_finite() { f.level_db } else { -120.0 })).collect();
    let n = lv.len();
    // Three-frame mean.
    let sm: Vec<f32> = (0..n)
        .map(|k| {
            let w = lv.get(k.saturating_sub(1)..(k + 2).min(n)).unwrap_or(&[]);
            if w.is_empty() { -120.0 } else { w.iter().sum::<f32>() / w.len() as f32 }
        })
        .collect();
    let mut out: Vec<(usize, f32)> = Vec::new();
    for k in 1..n.saturating_sub(1) {
        let v = sm.get(k).copied().unwrap_or(0.0);
        let local = sm.get(k.saturating_sub(2)..(k + 3).min(n)).unwrap_or(&[]).iter().all(|&o| o >= v);
        if !local {
            continue;
        }
        let left = sm.get(k.saturating_sub(ONSET_REACH)..k).unwrap_or(&[]).iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let right = sm.get(k + 1..(k + 1 + ONSET_REACH).min(n)).unwrap_or(&[]).iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let depth = left.min(right) - v;
        if depth >= ONSET_DIP_DB {
            match out.last_mut() {
                Some(last) if k - last.0 < ONSET_REACH => {
                    if depth > last.1 {
                        *last = (k, depth);
                    }
                }
                _ => out.push((k, depth)),
            }
        }
    }
    out.into_iter().map(|(k, d)| (a + k, d)).collect()
}

/// One piece of a phrase while folding: `[start, end)`, and whether the boundary at its start is
/// a syllable boundary.
#[derive(Debug, Clone, Copy)]
struct Piece {
    start: usize,
    end: usize,
    onset_before: bool,
}

/// The phrase pitch helpers: gap-filled cents over the phrase.
struct PhrasePitch {
    a: usize,
    x: Vec<f64>,
}

impl PhrasePitch {
    fn slice(&self, s: usize, e: usize) -> &[f64] {
        self.x.get(s.saturating_sub(self.a)..e.saturating_sub(self.a).min(self.x.len())).unwrap_or(&[])
    }

    fn centre(&self, p: &Piece) -> f64 {
        weighted_centre(self.slice(p.start, p.end)).unwrap_or(0.0)
    }

    /// Median of the first (or last) quarter of the piece (at least 2 frames).
    fn end_pitch(&self, p: &Piece, last: bool) -> f64 {
        let x = self.slice(p.start, p.end);
        let q = (x.len() / 4).max(2).min(x.len());
        let part = if last { x.get(x.len() - q..) } else { x.get(..q) };
        crate::note::median(part.unwrap_or(&[])).unwrap_or(0.0)
    }

    /// Whether the piece is a glide: no longer than 200 ms, moving 50 c or more from its first to
    /// its last quarter, with no steady run of 50 ms.
    fn is_glide(&self, p: &Piece) -> bool {
        let len = p.end - p.start;
        if len > GLIDE_MAX_FRAMES {
            return false;
        }
        if (self.end_pitch(p, true) - self.end_pitch(p, false)).abs() < GLIDE_MIN_CENTS {
            return false;
        }
        // The steady runs are judged on the phrase pitch, so the frames at the piece's edges see
        // their neighbours.
        let m = despike(&self.x);
        let at = |i: usize| m.get(i.saturating_sub(self.a).min(m.len().saturating_sub(1))).copied().unwrap_or(0.0);
        let (lo, hi) = (self.a, self.a + m.len());
        let mut run = 0usize;
        let mut best = 0usize;
        for i in p.start..p.end {
            let i0 = i.saturating_sub(STEADY_HALF).max(lo);
            let i1 = (i + STEADY_HALF).min(hi.saturating_sub(1));
            if (at(i1) - at(i0)).abs() < STEADY_CENTS {
                run += 1;
                best = best.max(run);
            } else {
                run = 0;
            }
        }
        best < PLATEAU_FRAMES
    }

    /// Whether piece `k` is no note of its own (see the module docs).
    fn transitional(&self, pieces: &[Piece], k: usize) -> bool {
        let Some(p) = pieces.get(k) else { return false };
        let len = p.end - p.start;
        if len < SHORT_FRAMES || self.is_glide(p) {
            return true;
        }
        if len >= PASSING_FRAMES {
            return false;
        }
        match (k.checked_sub(1).and_then(|j| pieces.get(j)), pieces.get(k + 1)) {
            (Some(a), Some(b)) => {
                let (ca, cb, c) = (self.centre(a), self.centre(b), self.centre(p));
                c > ca.min(cb) + BETWEEN_CENTS && c < ca.max(cb) - BETWEEN_CENTS
            }
            (Some(n), None) | (None, Some(n)) => n.end - n.start >= 2 * len,
            (None, None) => false,
        }
    }

    /// Merges runs of pieces that swing up and down around one pitch (wide vibrato that crosses a
    /// semitone line) into one piece.
    fn merge_vibrato(&self, pieces: &mut Vec<Piece>) {
        let centres: Vec<f64> = pieces.iter().map(|p| self.centre(p)).collect();
        // Whether pieces k, k+1, k+2 swing: k+1 short, away from k by a vibrato's swing, k+2 back.
        let swings = |k: usize| {
            let (Some(a), Some(b), Some(c), Some(mid), Some(last)) =
                (centres.get(k), centres.get(k + 1), centres.get(k + 2), pieces.get(k + 1), pieces.get(k + 2))
            else {
                return false;
            };
            let d = (b - a).abs();
            mid.end - mid.start <= VIBRATO_PIECE_FRAMES
                && (VIBRATO_SWING_CENTS.0..=VIBRATO_SWING_CENTS.1).contains(&d)
                && (c - a).abs() < VIBRATO_SAME_CENTS
                && !mid.onset_before
                && !last.onset_before
        };
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut k = 0;
        while k + 2 < pieces.len() {
            if !swings(k) {
                k += 1;
                continue;
            }
            let mut j = k + 2;
            while swings(j - 1) {
                j += 1;
            }
            runs.push((k, j));
            k = j + 1;
        }
        for (a, b) in runs.into_iter().rev() {
            for _ in a..b {
                merge(pieces, a);
            }
        }
    }
}

/// Musical note spans `[a, b)` in frames.
fn musical_spans(track: &PitchTrack) -> Vec<(u32, u32)> {
    let candidates = segment_spans(track);
    let mut out = Vec::new();
    for (pa, pb) in phrases(track) {
        let raw: Vec<Option<f64>> = (pa..pb).map(|i| track.frames.get(i).and_then(|f| f.cents).filter(|c| c.is_finite()).map(f64::from)).collect();
        let pitch = PhrasePitch { a: pa, x: fill_gaps(&raw) };
        let dips = onsets(track, pa, pb);

        // Classic's runs in this phrase, widened to tile it.
        let runs: Vec<(usize, usize)> =
            candidates.iter().map(|&(a, b)| (a as usize, b as usize)).filter(|&(a, b)| a >= pa && b <= pb && b > a).collect();
        let mut cuts: Vec<usize> = Vec::new();
        for w in runs.windows(2) {
            if let [(_, e0), (s1, _)] = *w {
                cuts.push(if s1 >= e0 { (e0 + s1) / 2 } else { s1 });
            }
        }
        // Strong dips cut too (a re-sung syllable), unless a cut is already that close.
        for &(k, depth) in &dips {
            if depth >= ONSET_SPLIT_DB && !cuts.iter().any(|&c| c.abs_diff(k) < SHORT_FRAMES) {
                cuts.push(k);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.retain(|&c| c > pa && c < pb);
        let mut pieces: Vec<Piece> = Vec::new();
        let mut s = pa;
        for c in cuts.into_iter().chain(std::iter::once(pb)) {
            if c > s {
                let onset_before = !pieces.is_empty() && dips.iter().any(|&(k, _)| k.abs_diff(s) <= ONSET_GUARD);
                pieces.push(Piece { start: s, end: c, onset_before });
                s = c;
            }
        }

        pitch.merge_vibrato(&mut pieces);
        // Fold glides and short pieces into a neighbour, shortest first.
        while pieces.len() > 1 {
            let Some(k) =
                (0..pieces.len()).filter(|&k| pitch.transitional(&pieces, k)).min_by_key(|&k| pieces.get(k).map_or(usize::MAX, |p| p.end - p.start))
            else {
                break;
            };
            let into_next = fold_into_next(&pitch, &pieces, k);
            merge(&mut pieces, if into_next { k } else { k - 1 });
        }
        // Same pitch either side of a non-syllable boundary: one note.
        let mut k = 1;
        while k < pieces.len() {
            let (Some(p), Some(q)) = (pieces.get(k - 1), pieces.get(k)) else { break };
            if !q.onset_before && (pitch.centre(p) - pitch.centre(q)).abs() < SAME_PITCH_CENTS {
                merge(&mut pieces, k - 1);
            } else {
                k += 1;
            }
        }
        out.extend(pieces.iter().map(|p| (frame_u32(p.start), frame_u32(p.end))));
    }
    out
}

/// Whether transitional piece `k` folds into the piece after it (else the one before).
fn fold_into_next(pitch: &PhrasePitch, pieces: &[Piece], k: usize) -> bool {
    let (Some(p), prev, next) = (pieces.get(k), k.checked_sub(1).and_then(|j| pieces.get(j)), pieces.get(k + 1)) else { return false };
    let (Some(prev), Some(next)) = (prev, next) else { return next.is_some() };
    // A syllable starts with it: it is the next note's approach; one starts after it: the end of
    // the note before.
    match (p.onset_before, next.onset_before) {
        (true, false) => return true,
        (false, true) => return false,
        _ => {}
    }
    // Otherwise by pitch: the approach ends near the next note, a fall starts near the last one.
    // The approach is favoured: a glide between two notes is the second one's scoop, as a
    // singer phrases it, unless it stays much nearer the note before (a fall off it).
    let to_next = (pitch.end_pitch(p, true) - pitch.centre(next)).abs();
    let from_prev = (pitch.end_pitch(p, false) - pitch.centre(prev)).abs();
    to_next <= from_prev + FOLD_NEXT_BIAS_CENTS
}

/// Merges piece `k + 1` into piece `k`.
fn merge(pieces: &mut Vec<Piece>, k: usize) {
    if k + 1 >= pieces.len() {
        return;
    }
    let next = pieces.remove(k + 1);
    if let Some(p) = pieces.get_mut(k) {
        p.end = next.end;
    }
}

/// Per note, what the Musical curves need.
struct NoteMove {
    start: usize,
    end: usize,
    /// Core `[c0, c1)`, inside `[start, end)`.
    c0: usize,
    c1: usize,
    /// The whole-note move (cents): `centre·(target − centre) + offset`; 0 when unedited.
    delta: f64,
    edit: Option<NoteEdit>,
    pitched: bool,
}

/// The Musical pitch-shift and gain curves (see the module docs). Gain is Classic's.
pub(crate) fn build_curves_musical(a: &VocalAnalysis, edits: &BTreeMap<NoteId, NoteEdit>, scale: &dyn ScaleAt) -> EditCurves {
    let n = a.track.frames.len();
    let gain = gain_curve(a, edits);
    let voiced: Vec<bool> = a.track.frames.iter().map(|f| f.cents.is_some_and(f32::is_finite)).collect();
    let sung = fill_gaps(&a.track.frames.iter().map(|f| f.cents.filter(|c| c.is_finite()).map(f64::from)).collect::<Vec<_>>());
    let mut raw = vec![0.0f64; n];
    // Frames whose shift skips the smoothing (Smooth and Shorten glides: already shaped).
    let mut exempt = vec![false; n];
    let mut offset_room = vec![0.0f64; n];

    let mut moves: Vec<NoteMove> = Vec::with_capacity(a.notes.len());
    for note in &a.notes {
        let start = (note.start as usize).min(n);
        let end = (note.end as usize).clamp(start, n);
        let (c0, c1) = core_range(&a.track, note);
        let (c0, c1) = ((c0 as usize).clamp(start, end), (c1 as usize).clamp(start, end));
        let (c0, c1) = if c1 > c0 { (c0, c1) } else { (start, end) };
        let edit = edits.get(&note.id).cloned().map(NoteEdit::clamped).filter(|e| !e.is_identity());
        let mut mv = NoteMove { start, end, c0, c1, delta: 0.0, edit: edit.clone(), pitched: false };
        if let Some(e) = edit {
            if e.offset_cents != 0.0 {
                for v in offset_room.iter_mut().take(end).skip(start) {
                    *v = v.max(f64::from(e.offset_cents.abs()));
                }
            }
            if let Some(p) = note_pitch(a, note, &e, scale, NoteMode::Musical) {
                let delta = f64::from(e.centre) * (p.tgt - p.cen) + f64::from(e.offset_cents);
                mv.delta = delta;
                mv.pitched = true;
                // The whole note moves; over the core, Drift and Vibrato act too (tapered in and
                // out over 40 ms, so the core joins its glides without a step).
                for v in raw.iter_mut().take(end).skip(start) {
                    *v = delta;
                }
                let x0 = start + p.off;
                let (k0, k1) = (c0.max(x0), c1.min(x0 + p.x.len()));
                if k1 > k0 {
                    let d: Vec<f64> = p.x.get(k0 - x0..k1 - x0).unwrap_or(&[]).iter().map(|v| v - p.cen).collect();
                    let (_, slow, fast) = decompose64(&d);
                    let len = d.len();
                    let r = TAPER_FRAMES.min(len / 4);
                    let ramp = |i: usize| if r > 1 { i as f64 / (r - 1) as f64 } else { 0.0 };
                    for k in 0..len {
                        let w = if k < r {
                            ramp(k)
                        } else if k >= len - r {
                            ramp(len - 1 - k)
                        } else {
                            1.0
                        };
                        let sl = slow.get(k).copied().unwrap_or(0.0);
                        let fa = fast.get(k).copied().unwrap_or(0.0);
                        if let Some(o) = raw.get_mut(k0 + k) {
                            *o = delta + (-f64::from(e.drift) * sl + f64::from(e.vibrato) * fa) * w;
                        }
                    }
                }
            }
        }
        moves.push(mv);
    }

    // Glides between touching notes, and approaches after silence.
    for j in 0..moves.len() {
        let Some(b) = moves.get(j) else { continue };
        let prev = j.checked_sub(1).and_then(|i| moves.get(i)).filter(|p| b.start <= p.end + BRIDGE_FRAMES);
        let tr = b.edit.as_ref().filter(|_| b.pitched).map_or(Transition::Keep, |e| e.transition);
        match prev {
            Some(p) if p.pitched || b.pitched => {
                let (r0, r1) = (p.c1, b.c0);
                if r1 <= r0 {
                    continue;
                }
                let at = |i: usize| sung.get(i).copied().unwrap_or(0.0);
                let mean = |s: usize, e: usize| {
                    let w = sung.get(s..e.min(n)).unwrap_or(&[]);
                    if w.is_empty() { at(s) } else { w.iter().sum::<f64>() / w.len() as f64 }
                };
                let pa = mean(r0.saturating_sub(EDGE_FRAMES).max(p.c0), r0);
                let pb = mean(r1, (r1 + EDGE_FRAMES).min(b.c1));
                let keep = glide_weights(&sung, r0, r1, pa, pb);
                let (da, db) = (p.delta, b.delta);
                let keep_shift: Vec<f64> = keep.iter().map(|w| da + (db - da) * w).collect();
                let len = r1 - r0;
                // Tuned pitch where the glide leaves and where it lands.
                let ta = at(r0.saturating_sub(1)) + raw.get(r0.saturating_sub(1)).copied().unwrap_or(da);
                let tb = at(r1) + raw.get(r1).copied().unwrap_or(db);
                match tr {
                    Transition::Keep => {
                        for (k, v) in keep_shift.iter().enumerate() {
                            if let Some(o) = raw.get_mut(r0 + k) {
                                *o = *v;
                            }
                        }
                    }
                    Transition::Smooth => {
                        for k in 0..len {
                            let target = ta + (tb - ta) * (k + 1) as f64 / (len + 1) as f64;
                            if let (Some(o), Some(e)) = (raw.get_mut(r0 + k), exempt.get_mut(r0 + k)) {
                                *o = target - at(r0 + k);
                                *e = true;
                            }
                        }
                    }
                    Transition::Shorten => {
                        let tuned: Vec<f64> = keep_shift.iter().enumerate().map(|(k, s)| at(r0 + k) + s).collect();
                        let sh = shorten(&tuned, tb);
                        for (k, t) in sh.iter().enumerate() {
                            if let (Some(o), Some(e)) = (raw.get_mut(r0 + k), exempt.get_mut(r0 + k)) {
                                *o = t - at(r0 + k);
                                *e = true;
                            }
                        }
                    }
                }
            }
            Some(_) => {}
            None => {
                // After silence: the approach moves with the note (Keep), or glides from where it
                // was sung into the tuned note (Smooth, Shorten), as Classic's scoop.
                if tr == Transition::Keep {
                    continue;
                }
                let Some(k0) = (b.start..b.c0).find(|&i| voiced.get(i).copied().unwrap_or(false)) else { continue };
                let len = b.c0 - k0;
                if len == 0 {
                    continue;
                }
                let p0 = sung.get(k0).copied().unwrap_or(0.0);
                let tb = sung.get(b.c0).copied().unwrap_or(p0) + raw.get(b.c0).copied().unwrap_or(b.delta);
                let target: Vec<f64> = match tr {
                    Transition::Smooth => (0..len).map(|k| p0 + (tb - p0) * k as f64 / len as f64).collect(),
                    _ => {
                        let sung_part: Vec<f64> = (k0..b.c0).map(|i| sung.get(i).copied().unwrap_or(p0)).collect();
                        shorten(&sung_part, tb)
                    }
                };
                for (k, t) in target.iter().enumerate() {
                    if let (Some(o), Some(e)) = (raw.get_mut(k0 + k), exempt.get_mut(k0 + k)) {
                        *o = t - sung.get(k0 + k).copied().unwrap_or(0.0);
                        *e = true;
                    }
                }
            }
        }
    }

    // Smooth inside each voiced run (edges held, so a note's first frames keep their move).
    let kern = hann_kernel(SMOOTH_TAPS);
    let h = SMOOTH_TAPS / 2;
    let mut smoothed = raw.clone();
    let mut i = 0;
    while i < n {
        if !voiced.get(i).copied().unwrap_or(false) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && voiced.get(j).copied().unwrap_or(false) {
            j += 1;
        }
        for k in i..j {
            if exempt.get(k).copied().unwrap_or(false) {
                continue;
            }
            let v: f64 = kern
                .iter()
                .enumerate()
                .map(|(t, w)| {
                    let idx = (k + t).saturating_sub(h).clamp(i, j - 1);
                    w * raw.get(idx).copied().unwrap_or(0.0)
                })
                .sum();
            if let Some(o) = smoothed.get_mut(k) {
                *o = v;
            }
        }
        i = j;
    }

    let mut out: Vec<f32> = smoothed
        .iter()
        .zip(&voiced)
        .zip(&offset_room)
        .map(|((v, &vo), room)| {
            let lim = (MAX_SHIFT_CENTS + room).min(MAX_TOTAL_SHIFT_CENTS);
            if vo && v.is_finite() { v.clamp(-lim, lim) as f32 } else { 0.0 }
        })
        .collect();
    // An unedited note's core stays exactly as sung (only the glides next to an edited note move).
    for m in moves.iter().filter(|m| !m.pitched) {
        for v in out.iter_mut().take(m.c1).skip(m.c0) {
            *v = 0.0;
        }
    }
    // Unvoiced always wins.
    for r in &a.unvoiced {
        for v in out.iter_mut().take((r.end as usize).min(n)).skip(r.start as usize) {
            *v = 0.0;
        }
    }
    EditCurves { shift_cents: out, gain_db: gain }
}

/// The blend weight 0 → 1 across a glide `[r0, r1)` from a note sung at `pa` to one at `pb`: how
/// far the sung pitch has travelled from `pa` to `pb` (when they are 40 c or more apart; by time
/// otherwise), never going back, eased (smoothstep), and held within time ramps over the glide's
/// first and last quarters so it starts at 0 and arrives at 1 whatever the pitch does.
fn glide_weights(sung: &[f64], r0: usize, r1: usize, pa: f64, pb: f64) -> Vec<f64> {
    let len = r1.saturating_sub(r0);
    let by_pitch = (pb - pa).abs() >= PITCH_BLEND_MIN_CENTS;
    let ease = |t: f64| {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let mut prog = 0.0f64;
    (0..len)
        .map(|k| {
            let t = (k as f64 + 1.0) / (len as f64 + 1.0);
            let p = if by_pitch { (sung.get(r0 + k).copied().unwrap_or(pa) - pa) / (pb - pa) } else { t };
            prog = prog.max(p.clamp(0.0, 1.0));
            let w = ease(prog);
            w.max(ease((t - 0.75) / 0.25)).min(ease(t / 0.25))
        })
        .collect()
}

/// Shorten: the glide `g` (tuned pitch per frame) at double speed into `end_pitch`, which it
/// reaches at the midpoint and holds, blended in with a raised cosine over the 4 frames before.
fn shorten(g: &[f64], end_pitch: f64) -> Vec<f64> {
    let n = g.len();
    let mid = n / 2;
    (0..n)
        .map(|k| {
            if k >= mid {
                return end_pitch;
            }
            let glide = g.get((2 * k).min(n.saturating_sub(1))).copied().unwrap_or(end_pitch);
            let j = k + SHORTEN_BLEND_FRAMES;
            let w = if j >= mid {
                let i = (j - mid + 1) as f64;
                0.5 - 0.5 * (std::f64::consts::PI * i / (SHORTEN_BLEND_FRAMES + 1) as f64).cos()
            } else {
                0.0
            };
            glide + (end_pitch - glide) * w
        })
        .collect()
}
