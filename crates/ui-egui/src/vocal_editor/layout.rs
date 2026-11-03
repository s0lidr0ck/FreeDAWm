//! The Vocal Editor roll's pure geometry: the view mapping (session time across, cents up),
//! note bars at their tuned centres (the engine's, so a note moved two semitones reads 0 c; with
//! the original's ghost and the scoop), and the pitch curves as polylines. Everything is culled to the visible range and the clip's window, so a
//! whole-song view of thousands of notes stays cheap.

use crate::vocal_style::{note_thickness, off_colour};
use egui::{Color32, Pos2, Rect, pos2};
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::Clip;
use soundcraft_time::Samples;
use soundcraft_vocal::{EditCurves, Note, PitchTrack, VocalAnalysis};

/// A pitch curve breaks where it jumps by more than this (cents) between frames.
const JUMP_CENTS: f32 = 250.0;

/// What the roll shows: session time `[t0, t1)` across `rect`, `top_cents` at its top edge and
/// `row_h` px per semitone (100 c), pitch increasing upward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollView {
    pub rect: Rect,
    pub t0: Samples,
    pub t1: Samples,
    pub top_cents: f32,
    pub row_h: f32,
}

impl RollView {
    fn span(&self) -> f64 {
        (self.t1.saturating_sub(self.t0)).max(1) as f64
    }

    /// Screen x of session time `at`.
    pub fn x(&self, at: Samples) -> f32 {
        self.rect.left() + (at.saturating_sub(self.t0) as f64 / self.span() * f64::from(self.rect.width())) as f32
    }

    /// Screen y of a pitch (MIDI × 100).
    pub fn y(&self, cents: f32) -> f32 {
        self.rect.top() + (self.top_cents - cents) / 100.0 * self.row_h
    }

    /// Session time at screen x.
    pub fn at(&self, x: f32) -> Samples {
        let w = f64::from(self.rect.width().max(1.0));
        let v = f64::from(x - self.rect.left()) / w * self.span();
        self.t0.saturating_add(if v.is_finite() { v.round() as Samples } else { 0 })
    }

    /// Pitch (MIDI × 100) at screen y.
    pub fn cents(&self, y: f32) -> f32 {
        self.top_cents - (y - self.rect.top()) / self.row_h.max(1.0) * 100.0
    }
}

/// One note bar to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteShape {
    pub r: NoteRef,
    /// The note's index in the analysis's notes.
    pub index: usize,
    /// The bar, centred on the tuned centre, as thick as the note is loud.
    pub rect: Rect,
    pub colour: Color32,
    /// The sung centre's offset from the target (cents).
    pub offset: f32,
    /// The tuned centre's offset from the target (cents).
    pub tuned_offset: f32,
    /// The note's target pitch (MIDI × 100).
    pub target: f32,
    /// The original-centre bar, when tuning moved the note by 1 c or more.
    pub ghost: Option<Rect>,
    /// The scoop into the note: from its first point to the note's start on the bar.
    pub scoop: Option<[Pos2; 2]>,
}

/// Where a clip's analysis frames sit in session time, and the visible part of its window.
#[derive(Debug, Clone, Copy)]
pub struct Frames {
    /// Session time of source sample 0.
    zero: Samples,
    hop: Samples,
    /// The clip's window in session time.
    lo: Samples,
    hi: Samples,
}

impl Frames {
    pub fn of(clip: &Clip, hop: u32) -> Frames {
        Frames { zero: clip.start.saturating_sub(clip.source_offset()), hop: Samples::from(hop.max(1)), lo: clip.start, hi: clip.end() }
    }

    /// Session time of frame `f`.
    pub fn time(&self, f: u32) -> Samples {
        self.zero.saturating_add(Samples::from(f).saturating_mul(self.hop))
    }

    /// The frame sounding at session time `at` (0 before the source).
    pub fn frame(&self, at: Samples) -> u32 {
        u32::try_from(at.saturating_sub(self.zero).max(0) / self.hop).unwrap_or(u32::MAX)
    }

    /// `t` clamped into the clip's window.
    pub fn clamp(&self, t: Samples) -> Samples {
        t.clamp(self.lo, self.hi.max(self.lo))
    }

    /// The frames `[lo, hi)` that are both in the clip's window and in `view` (one extra each
    /// side so lines reach the edges); `None` when they don't meet.
    pub fn visible(&self, view: &RollView) -> Option<(u32, u32)> {
        let (a, b) = (view.t0.max(self.lo), view.t1.min(self.hi));
        if b <= a {
            return None;
        }
        Some((self.frame(a).saturating_sub(1), self.frame(b).saturating_add(2)))
    }
}

/// The mean shift over a note's voiced frames (all its frames when none is voiced): where an
/// unedited note's bar sits when a neighbour's smoothing spills onto it.
fn mean_shift(track: &PitchTrack, shift: &[f32], n: &Note) -> f32 {
    let (mut sum, mut k, mut all, mut ka) = (0.0f64, 0u32, 0.0f64, 0u32);
    for f in n.start..n.end {
        let Some(s) = shift.get(f as usize).copied().filter(|s| s.is_finite()) else { continue };
        all += f64::from(s);
        ka += 1;
        if track.frames.get(f as usize).is_some_and(|fr| fr.cents.is_some()) {
            sum += f64::from(s);
            k += 1;
        }
    }
    if k > 0 {
        (sum / f64::from(k)) as f32
    } else if ka > 0 {
        (all / f64::from(ka)) as f32
    } else {
        0.0
    }
}

/// The indices of `a`'s notes in frames `[lo, hi)` (notes are sorted and don't overlap).
pub fn notes_in(a: &VocalAnalysis, lo: u32, hi: u32) -> std::ops::Range<usize> {
    let first = a.notes.partition_point(|n| n.end <= lo);
    let last = a.notes.partition_point(|n| n.start < hi).max(first);
    first..last
}

/// The note bars of `clip` (analysis `a`, edit curves `curves` when tuned) inside `view`:
/// `target` gives each note's target pitch, `tuned` an edited note's tuned centre as the engine
/// computes it (`None` for a note with no edit: its bar follows the mean of its curve, which is
/// only the spill from its neighbours), `typical_db` the level of a middling thickness.
pub fn note_shapes(
    view: &RollView,
    clip: &Clip,
    a: &VocalAnalysis,
    curves: Option<&EditCurves>,
    typical_db: f32,
    target: impl Fn(&Note) -> f32,
    tuned: impl Fn(&Note) -> Option<f32>,
) -> Vec<NoteShape> {
    let fm = Frames::of(clip, a.track.hop_samples);
    let Some((lo, hi)) = fm.visible(view) else { return Vec::new() };
    let mut out = Vec::new();
    let range = notes_in(a, lo, hi);
    for (n, index) in a.notes.get(range.clone()).unwrap_or(&[]).iter().zip(range) {
        let (t0, t1) = (fm.clamp(fm.time(n.start)), fm.clamp(fm.time(n.end)));
        if t1 <= t0 || t1 <= view.t0 || t0 >= view.t1 {
            continue;
        }
        let tgt = target(n);
        let offset = n.centre_cents - tgt;
        let tuned = match curves {
            None => n.centre_cents,
            Some(c) => tuned(n).filter(|t| t.is_finite()).unwrap_or_else(|| n.centre_cents + mean_shift(&a.track, &c.shift_cents, n)),
        };
        let tuned_offset = tuned - tgt;
        let h = note_thickness(n.level_db, typical_db, view.row_h);
        let (x0, x1) = (view.x(t0), view.x(t1).max(view.x(t0) + 2.0));
        let bar = |c: f32| Rect::from_min_max(pos2(x0, view.y(c) - h / 2.0), pos2(x1, view.y(c) + h / 2.0));
        let rect = bar(tuned);
        let ghost = ((tuned_offset - offset).abs() >= 1.0).then(|| bar(n.centre_cents));
        let scoop = n.scoop.filter(|(s0, s1)| s1 > s0).map(|(s0, _)| {
            let first = a.track.frames.get(s0 as usize).and_then(|f| f.cents).filter(|c| c.is_finite()).unwrap_or(n.centre_cents);
            let s = curves.and_then(|c| c.shift_cents.get(s0 as usize)).copied().filter(|s| s.is_finite()).unwrap_or(0.0);
            [pos2(view.x(fm.clamp(fm.time(s0))), view.y(first + s)), pos2(x0, view.y(tuned))]
        });
        out.push(NoteShape {
            r: NoteRef { clip: clip.id, note: n.id },
            index,
            rect,
            colour: off_colour(tuned_offset),
            offset,
            tuned_offset,
            target: tgt,
            ghost,
            scoop,
        });
    }
    out
}

/// `track`'s pitch over `clip`'s window inside `view` as polylines (plus `shift` per frame when
/// given), broken at unvoiced frames and at jumps of more than 250 c.
pub fn curve_points(view: &RollView, clip: &Clip, track: &PitchTrack, shift: Option<&[f32]>) -> Vec<Vec<Pos2>> {
    curve_points_where(view, clip, track, shift, |_| true)
}

/// [`curve_points`] over the frames `keep` accepts only (the line breaks elsewhere).
pub fn curve_points_where(view: &RollView, clip: &Clip, track: &PitchTrack, shift: Option<&[f32]>, keep: impl Fn(u32) -> bool) -> Vec<Vec<Pos2>> {
    let fm = Frames::of(clip, track.hop_samples);
    let Some((lo, hi)) = fm.visible(view) else { return Vec::new() };
    let hi = hi.min(u32::try_from(track.frames.len()).unwrap_or(u32::MAX));
    let mut out: Vec<Vec<Pos2>> = Vec::new();
    let mut line: Vec<Pos2> = Vec::new();
    let mut prev: Option<f32> = None;
    for f in lo..hi {
        let t = fm.time(f);
        let c = track.frames.get(f as usize).and_then(|fr| fr.cents).filter(|c| c.is_finite() && t >= fm.lo && t <= fm.hi && keep(f));
        let v = c.map(|c| c + shift.and_then(|s| s.get(f as usize)).copied().filter(|s| s.is_finite()).unwrap_or(0.0));
        let jump = matches!((prev, v), (Some(p), Some(v)) if (v - p).abs() > JUMP_CENTS);
        if (v.is_none() || jump) && !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        if let Some(v) = v {
            line.push(pos2(view.x(t), view.y(v)));
        }
        prev = v;
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}
