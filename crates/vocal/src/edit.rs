//! Per-note edits (Centre, Drift, Vibrato, levelling, transitions) and the maths that turns them
//! into per-frame pitch-shift and gain curves (port of the spike's `decompose` and `macro_edits`).
//!
//! Curve algorithm, per edited note (frames `[start, end)`, pitch `x`):
//! - `cen = note_core(x)`, `tgt = target_cents(cen, scale if Snap to key)`;
//! - `(_, slow, fast) = decompose(x − cen)`; taper `w` 0→1 over the first `r = min(8, len/4)`
//!   frames and 1→0 over the last `r`;
//! - `s[k] = centre·(tgt − cen) − drift·slow[k]·w[k] + vibrato·fast[k] + offset`;
//! - the scoop `[s0, start)` follows the note's [`Transition`] (Smooth and Shorten aim at the
//!   moved note; Keep leaves the approach as sung);
//! - the whole curve is smoothed with a normalised 9-tap Hann, clamped to ±250 c beyond the
//!   note's manual offset (±2400 c at most) and zeroed on
//!   unvoiced frames (and inside every [`crate::UnvoicedRegion`]); unedited notes stay exactly 0.0
//!   (except within 4 frames of an edited note). A `Smooth`/`Shorten` scoop and its note's first
//!   4 frames keep their unsmoothed shift: the transition already shapes that pitch, and smoothing
//!   the shift across the sung pitch's jump at the note start would bend it by several cents.
//!
//! Gain is a constant per edited note, the levelling [`level_gain_db`] (0 when the edit has no
//! `typical_db`) plus the note's manual `gain_db`, with 3-frame raised-cosine ramps inside its edges, zero inside every [`crate::UnvoicedRegion`] (with
//! the same ramps at the region's edges), and exactly 0.0 elsewhere.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::filter::{butter2_lowpass, filtfilt};
use crate::note::{Note, NoteId, median, note_core};
use crate::segment::VocalAnalysis;

/// A set of pitch classes: bit `pc` set means pitch class `pc` (C = 0) is in the scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PitchClassSet(pub u16);

impl PitchClassSet {
    /// The set holding `pcs` (values of 12 and above are ignored).
    pub fn from_pcs(pcs: &[u8]) -> PitchClassSet {
        PitchClassSet(pcs.iter().filter(|&&pc| pc < 12).fold(0u16, |m, &pc| m | (1 << pc)))
    }

    /// Whether pitch class `pc` (0..12) is in the set.
    pub fn contains(&self, pc: u8) -> bool {
        pc < 12 && self.0 & (1 << pc) != 0
    }

    fn is_empty(&self) -> bool {
        self.0 & 0x0fff == 0
    }
}

/// How a note's scoop (its approach) is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Transition {
    /// Leave the approach as sung.
    #[default]
    Keep,
    /// A straight glide from the scoop's first pitch to the note's tuned start pitch.
    Smooth,
    /// The sung approach at double speed, blended (raised cosine, 4 frames) into the tuned pitch,
    /// which it reaches at the scoop's midpoint and holds.
    Shorten,
}

/// The user's edit of one note. Missing fields deserialise to their defaults.
#[derive(Default, Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteEdit {
    /// Fraction of the way to the target pitch (0..=1).
    pub centre: f32,
    /// Fraction of the slow drift removed (0..=1).
    pub drift: f32,
    /// Fast-part (vibrato) scale change: −1 removes it, +1 doubles it (−1..=1).
    pub vibrato: f32,
    /// How much of a shortfall below `typical_db` is made up (0..=1).
    pub raise: f32,
    /// How much of an excess above `typical_db` is taken off (0..=1).
    pub lower: f32,
    /// The level the levelling aims at (dB), usually [`SelectionSummary::typical_db`]. `None`
    /// (the default) means no levelling: the gain is 0 whatever `raise` and `lower` are.
    pub typical_db: Option<f32>,
    /// Target the nearest pitch class of the scale passed in, not the nearest semitone.
    pub snap_to_key: bool,
    pub transition: Transition,
    /// A manual pitch move (cents, ±2400) on top of Centre, Drift and Vibrato: what dragging the
    /// note up or down in the Vocal Editor sets. 0 in sessions saved before it existed.
    #[serde(default)]
    pub offset_cents: f32,
    /// A manual level change (dB, ±12) over the note's body, on top of the levelling: the note
    /// bar's `level` field. 0 in sessions saved before it existed.
    #[serde(default)]
    pub gain_db: f32,
}

impl NoteEdit {
    /// Whether the edit changes nothing (no pitch, level or transition change).
    pub fn is_identity(&self) -> bool {
        self.centre == 0.0
            && self.drift == 0.0
            && self.vibrato == 0.0
            && self.raise == 0.0
            && self.lower == 0.0
            && self.offset_cents == 0.0
            && self.gain_db == 0.0
            && self.transition == Transition::Keep
    }

    /// The edit with every range clamped and non-finite values mapped to 0 (a non-finite
    /// `typical_db` becomes `None`).
    pub fn clamped(self) -> Self {
        let c = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
        NoteEdit {
            centre: c(self.centre, 0.0, 1.0),
            drift: c(self.drift, 0.0, 1.0),
            vibrato: c(self.vibrato, -1.0, 1.0),
            raise: c(self.raise, 0.0, 1.0),
            lower: c(self.lower, 0.0, 1.0),
            offset_cents: c(self.offset_cents, -MAX_OFFSET_CENTS, MAX_OFFSET_CENTS),
            gain_db: c(self.gain_db, -MAX_GAIN_DB, MAX_GAIN_DB),
            typical_db: self.typical_db.filter(|t| t.is_finite()),
            ..self
        }
    }
}

/// Largest manual pitch move either way (cents): two octaves.
pub const MAX_OFFSET_CENTS: f32 = 2400.0;

/// The target a note moved by `offset_cents` is judged by: its own `target` moved by the offset's
/// whole semitones (nearest), so a note dragged up two rows is measured against the semitone two
/// up, and a few cents' nudge leaves the target where it was. A non-finite offset moves nothing.
pub fn moved_target(target: f32, offset_cents: f32) -> f32 {
    if !offset_cents.is_finite() {
        return target;
    }
    target + (offset_cents.clamp(-MAX_OFFSET_CENTS, MAX_OFFSET_CENTS) / 100.0).round() * 100.0
}

/// The target pitch for a note centred at `centre_cents`: the nearest multiple of 100, or with a
/// non-empty `snap` scale the nearest cents value whose pitch class is in the scale (ties go to
/// the lower). A non-finite centre is returned as is.
pub fn target_cents(centre_cents: f32, snap: Option<PitchClassSet>) -> f32 {
    if !centre_cents.is_finite() {
        return centre_cents;
    }
    let c = f64::from(centre_cents);
    let nearest = ((c / 100.0).round_ties_even() * 100.0) as f32;
    let Some(scale) = snap.filter(|s| !s.is_empty()) else { return nearest };
    let base = (c / 100.0).floor() as i64;
    let mut best: Option<(f64, i64)> = None;
    for m in base - 6..=base + 7 {
        let pc = m.rem_euclid(12) as u8;
        if !scale.contains(pc) {
            continue;
        }
        let d = (c - m as f64 * 100.0).abs();
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, m));
        }
    }
    best.map_or(nearest, |(_, m)| (m as f64 * 100.0) as f32)
}

/// Analysis frame rate (Hz): 5 ms hops.
const FRAME_RATE: f64 = 200.0;
/// Slow/fast split frequency (Hz).
const SLOW_CUTOFF_HZ: f64 = 2.5;
/// Notes this long or shorter get no slow part.
const MIN_FILTER_LEN: usize = 15;
/// filtfilt pad length (clamped to `len − 1`).
const PADLEN: usize = 12;

/// Splits a note's pitch (cents, no gaps) into `(centre, slow, fast)`: the mean, the < 2.5 Hz
/// wander around it (zero-phase Butterworth; zeros for 15 frames or fewer) and the rest.
pub fn decompose(cents: &[f32]) -> (f32, Vec<f32>, Vec<f32>) {
    let x: Vec<f64> = cents.iter().map(|&c| if c.is_finite() { f64::from(c) } else { 0.0 }).collect();
    let (centre, slow, fast) = decompose64(&x);
    (centre as f32, slow.iter().map(|&v| v as f32).collect(), fast.iter().map(|&v| v as f32).collect())
}

pub(crate) fn decompose64(x: &[f64]) -> (f64, Vec<f64>, Vec<f64>) {
    if x.is_empty() {
        return (0.0, Vec::new(), Vec::new());
    }
    let centre = x.iter().sum::<f64>() / x.len() as f64;
    let d: Vec<f64> = x.iter().map(|v| v - centre).collect();
    let slow = if d.len() > MIN_FILTER_LEN { filtfilt(&butter2_lowpass(SLOW_CUTOFF_HZ, FRAME_RATE), &d, PADLEN) } else { vec![0.0; d.len()] };
    let fast = d.iter().zip(&slow).map(|(a, b)| a - b).collect();
    (centre, slow, fast)
}

/// Largest level change (dB) either way: the levelling's, and the manual `gain_db`'s.
pub const MAX_GAIN_DB: f32 = 12.0;

/// The levelling gain for a note at `level_db`: `−raise·(L − typ)` below `typical_db`,
/// `−lower·(L − typ)` above it, clamped to ±12 dB (0 for non-finite input).
pub fn level_gain_db(level_db: f32, typical_db: f32, raise: f32, lower: f32) -> f32 {
    let d = level_db - typical_db;
    let g = if d < 0.0 {
        -raise * d
    } else if d > 0.0 {
        -lower * d
    } else {
        0.0
    };
    if g.is_finite() { g.clamp(-MAX_GAIN_DB, MAX_GAIN_DB) } else { 0.0 }
}

/// Level reported when there is nothing to measure (dB).
const EMPTY_LEVEL_DB: f32 = -120.0;

/// The typical level of `levels`: their median (−120 dB when there are none).
pub fn typical_level_db(levels: &[f32]) -> f32 {
    let v: Vec<f64> = levels.iter().filter(|l| l.is_finite()).map(|&l| f64::from(l)).collect();
    median(&v).map_or(EMPTY_LEVEL_DB, |m| m as f32)
}

/// Per-frame curves, one value per track frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EditCurves {
    pub shift_cents: Vec<f32>,
    pub gain_db: Vec<f32>,
}

/// The scale in force at a frame (for Snap to key). The crate never stores a key.
pub trait ScaleAt {
    fn scale_at(&self, frame: u32) -> Option<PitchClassSet>;
}

/// A constant scale (or none).
impl ScaleAt for Option<PitchClassSet> {
    fn scale_at(&self, _frame: u32) -> Option<PitchClassSet> {
        *self
    }
}

/// A note needs at least this many voiced frames to get a pitch change.
const MIN_VOICED_FRAMES: usize = 3;
/// Taper length (frames) at each end of the drift removal (40 ms).
const TAPER_FRAMES: usize = 8;
/// Hann smoothing length (frames) of the whole shift curve.
const SMOOTH_TAPS: usize = 9;
/// Largest pitch shift either way beyond a note's manual offset (cents).
const MAX_SHIFT_CENTS: f64 = 250.0;
/// Largest pitch shift either way, offset included (cents): what the resynthesis takes.
const MAX_TOTAL_SHIFT_CENTS: f64 = 2400.0;
/// Unedited-note frames this close to an edited note keep the smoothing spill.
const SPILL_FRAMES: u32 = 4;
/// Shorten's blend from the double-speed glide into the tuned pitch (frames before the midpoint).
const SHORTEN_BLEND_FRAMES: usize = 4;
/// Raised-cosine gain ramp length (frames, 15 ms).
const GAIN_RAMP_FRAMES: usize = 3;

/// The track's cents over `[start, end)`, trimmed to its first and last voiced frames, with every
/// interior gap (of any length, e.g. across a joined note's break) filled linearly. Returns
/// `(offset of the first voiced frame from start, cents)`; `None` when fewer than 3 frames are
/// voiced (or the span is off the track).
fn filled_pitch(a: &VocalAnalysis, start: u32, end: u32) -> Option<(usize, Vec<f64>)> {
    let frames = a.track.frames.get(start as usize..end as usize)?;
    let raw: Vec<Option<f64>> = frames.iter().map(|f| f.cents.filter(|c| c.is_finite()).map(f64::from)).collect();
    if raw.iter().flatten().count() < MIN_VOICED_FRAMES {
        return None;
    }
    let first = raw.iter().position(Option::is_some)?;
    let last = raw.iter().rposition(Option::is_some)?;
    let raw = raw.get(first..=last)?;
    let mut out = Vec::with_capacity(raw.len());
    let mut prev: Option<(usize, f64)> = None;
    for (k, v) in raw.iter().enumerate() {
        let Some(v) = *v else { continue };
        if let Some((lk, lv)) = prev {
            for j in lk + 1..k {
                out.push(lv + (v - lv) * (j - lk) as f64 / (k - lk) as f64);
            }
        }
        out.push(v);
        prev = Some((k, v));
    }
    Some((first, out))
}

/// A note's pitch, centre and target under an edit. `x` starts `off` frames into the note.
struct NotePitch {
    off: usize,
    x: Vec<f64>,
    cen: f64,
    tgt: f64,
}

fn note_pitch(a: &VocalAnalysis, note: &Note, edit: &NoteEdit, scale: &dyn ScaleAt) -> Option<NotePitch> {
    let (off, x) = filled_pitch(a, note.start, note.end)?;
    let cen = note_core(&x)?;
    let snap = if edit.snap_to_key { scale.scale_at(note.start) } else { None };
    let tgt = f64::from(target_cents(cen as f32, snap));
    Some(NotePitch { off, x, cen, tgt })
}

/// The note's shift `s[k]` before smoothing.
fn note_shift(p: &NotePitch, e: &NoteEdit) -> Vec<f64> {
    let n = p.x.len();
    let d: Vec<f64> = p.x.iter().map(|v| v - p.cen).collect();
    let (_, slow, fast) = decompose64(&d);
    let r = TAPER_FRAMES.min(n / 4);
    let ramp = |i: usize| if r > 1 { i as f64 / (r - 1) as f64 } else { 0.0 };
    (0..n)
        .map(|k| {
            let w = if k < r {
                ramp(k)
            } else if k >= n - r {
                ramp(n - 1 - k)
            } else {
                1.0
            };
            let sl = slow.get(k).copied().unwrap_or(0.0);
            let fa = fast.get(k).copied().unwrap_or(0.0);
            f64::from(e.centre) * (p.tgt - p.cen) - f64::from(e.drift) * sl * w + f64::from(e.vibrato) * fa + f64::from(e.offset_cents)
        })
        .collect()
}

/// Scoop shifts for frames `[s0, start)` given the note's tuned start pitch `end_pitch`.
fn scoop_shift(a: &VocalAnalysis, s0: u32, start: u32, end_pitch: f64, tr: Transition) -> Vec<f64> {
    let p: Vec<Option<f64>> =
        (s0..start).map(|i| a.track.frames.get(i as usize).and_then(|f| f.cents).filter(|c| c.is_finite()).map(f64::from)).collect();
    let n = p.len();
    let target_at = |k: usize| -> Option<f64> {
        match tr {
            Transition::Keep => None,
            Transition::Smooth => {
                // From the first voiced scoop frame's pitch (the scoop's first frame when voiced).
                let (k0, p0) = p.iter().enumerate().find_map(|(j, v)| v.map(|v| (j, v)))?;
                if k < k0 {
                    return None;
                }
                Some(p0 + (end_pitch - p0) * (k - k0) as f64 / (n - k0) as f64)
            }
            Transition::Shorten => {
                let mid = n / 2;
                if k < mid {
                    let glide = p.get((2 * k).min(n - 1)).copied().flatten().unwrap_or(end_pitch);
                    // Raised-cosine blend into the tuned pitch over the last frames before the
                    // midpoint, so the output has no step where the glide hands over.
                    let j = k + SHORTEN_BLEND_FRAMES;
                    let w = if j >= mid {
                        let i = (j - mid + 1) as f64;
                        0.5 - 0.5 * (std::f64::consts::PI * i / (SHORTEN_BLEND_FRAMES + 1) as f64).cos()
                    } else {
                        0.0
                    };
                    Some(glide + (end_pitch - glide) * w)
                } else {
                    Some(end_pitch)
                }
            }
        }
    };
    (0..n)
        .map(|k| match (p.get(k).copied().flatten(), target_at(k)) {
            (Some(pk), Some(q)) => q - pk,
            _ => 0.0,
        })
        .collect()
}

/// numpy `hanning(n)`, normalised to sum 1.
fn hann_kernel(n: usize) -> Vec<f64> {
    let w: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos()).collect();
    let s: f64 = w.iter().sum();
    w.into_iter().map(|v| v / s).collect()
}

/// numpy `convolve(x, k, mode="same")` for an odd symmetric kernel (zero padding).
fn convolve_same(x: &[f64], k: &[f64]) -> Vec<f64> {
    let h = k.len() / 2;
    (0..x.len()).map(|i| k.iter().enumerate().filter_map(|(j, w)| (i + h).checked_sub(j).and_then(|idx| x.get(idx)).map(|v| v * w)).sum()).collect()
}

/// The pitch-shift and gain curves for `edits` over the whole track (worker threads only).
///
/// A note's pitch is taken from its first to its last voiced frame, with interior unvoiced gaps
/// of any length filled linearly (the shift itself stays 0 on the unvoiced frames). A note with
/// fewer than 3 voiced frames gets gain but no pitch change.
pub fn build_curves(a: &VocalAnalysis, edits: &BTreeMap<NoteId, NoteEdit>, scale: &dyn ScaleAt) -> EditCurves {
    let n = a.track.frames.len();
    let mut shift = vec![0.0f64; n];
    let mut gain = vec![0.0f32; n];
    let mut edited_spans: Vec<(u32, u32)> = Vec::new();
    // Frames whose shift skips the smoothing: (first frame, raw shifts).
    let mut unsmoothed: Vec<(usize, Vec<f64>)> = Vec::new();
    // Per frame, the largest manual offset (cents) the shift may carry on top of the ±250 c.
    let mut offset_room = vec![0.0f64; n];
    let ramp = |d: usize| {
        if d >= GAIN_RAMP_FRAMES { 1.0 } else { 0.5 - 0.5 * (std::f32::consts::PI * (d + 1) as f32 / (GAIN_RAMP_FRAMES + 1) as f32).cos() }
    };
    // Gain factor from the unvoiced regions: 0 inside one, ramping back up over 3 frames on
    // either side of it.
    let mut unvoiced_factor = vec![1.0f32; n];
    for r in &a.unvoiced {
        let (s, e) = ((r.start as usize).min(n), (r.end as usize).min(n));
        for v in unvoiced_factor.iter_mut().take(e).skip(s) {
            *v = 0.0;
        }
        for d in 0..GAIN_RAMP_FRAMES {
            for i in [s.checked_sub(d + 1), Some(e + d)].into_iter().flatten() {
                if let Some(v) = unvoiced_factor.get_mut(i) {
                    *v = v.min(ramp(d));
                }
            }
        }
    }

    for note in &a.notes {
        let Some(e) = edits.get(&note.id).cloned().map(NoteEdit::clamped) else { continue };
        if e.is_identity() {
            continue;
        }
        let span_start = note.scoop.map_or(note.start, |(s0, _)| s0.min(note.start));
        edited_spans.push((span_start, note.end));
        if e.offset_cents != 0.0 {
            // Over the moved note only: the smoothing's spill onto an unedited neighbour keeps the
            // plain ±250 c clamp, so an octave move never drags the next note's first frames along.
            for v in offset_room.iter_mut().take(note.end as usize).skip(span_start as usize) {
                *v = v.max(f64::from(e.offset_cents.abs()));
            }
        }

        // Gain: the levelling plus the manual gain, constant with raised-cosine ramps inside the
        // note's edges, zero inside every unvoiced region (ramped at its edges too).
        let g = e.typical_db.map_or(0.0, |typ| level_gain_db(note.level_db, typ, e.raise, e.lower)) + e.gain_db;
        if g != 0.0 {
            let len = (note.end.saturating_sub(note.start)) as usize;
            for k in 0..len {
                let i = note.start as usize + k;
                let uv = unvoiced_factor.get(i).copied().unwrap_or(1.0);
                if let Some(v) = gain.get_mut(i) {
                    *v = g * ramp(k).min(ramp(len - 1 - k)).min(uv);
                }
            }
        }

        // Pitch (over the note's first to last voiced frame).
        let Some(p) = note_pitch(a, note, &e, scale) else { continue };
        let s = note_shift(&p, &e);
        let body = note.start as usize + p.off;
        for (k, v) in s.iter().enumerate() {
            if let Some(o) = shift.get_mut(body + k) {
                *o = *v;
            }
        }
        if let (Some((s0, _)), Some(x0), Some(sh0)) = (note.scoop, p.x.first(), s.first())
            && s0 < note.start
        {
            let sc = scoop_shift(a, s0, note.start, x0 + sh0, e.transition);
            for (k, v) in sc.iter().enumerate() {
                if let Some(o) = shift.get_mut(s0 as usize + k) {
                    *o = *v;
                }
            }
            if e.transition != Transition::Keep {
                // The transition already shapes the pitch from the scoop into the note: smoothing
                // the shift across the sung pitch's jump at the note start would bend it.
                let mut raw = sc;
                raw.extend(std::iter::repeat_n(0.0, p.off).chain(s.iter().copied()).take(SPILL_FRAMES as usize));
                unsmoothed.push((s0 as usize, raw));
            }
        }
    }

    let mut smoothed = convolve_same(&shift, &hann_kernel(SMOOTH_TAPS));
    for (first, raw) in unsmoothed {
        for (k, v) in raw.into_iter().enumerate() {
            if let Some(o) = smoothed.get_mut(first + k) {
                *o = v;
            }
        }
    }
    let mut out: Vec<f32> = smoothed
        .iter()
        .zip(&a.track.frames)
        .zip(&offset_room)
        .map(|((v, f), room)| {
            let lim = (MAX_SHIFT_CENTS + room).min(MAX_TOTAL_SHIFT_CENTS);
            if f.cents.is_some_and(f32::is_finite) && v.is_finite() { v.clamp(-lim, lim) as f32 } else { 0.0 }
        })
        .collect();

    // Unedited notes stay untouched, except for the spill right next to an edited note.
    let near_edited = |i: u32| edited_spans.iter().any(|&(s, e)| i.saturating_add(SPILL_FRAMES) >= s && i < e.saturating_add(SPILL_FRAMES));
    for note in &a.notes {
        let edited = edits.get(&note.id).is_some_and(|e| !e.clone().clamped().is_identity());
        if edited {
            continue;
        }
        for i in note.start..note.end {
            if !near_edited(i)
                && let Some(v) = out.get_mut(i as usize)
            {
                *v = 0.0;
            }
        }
    }
    // Unvoiced always wins.
    for r in &a.unvoiced {
        for i in r.start..r.end {
            if let Some(v) = out.get_mut(i as usize) {
                *v = 0.0;
            }
        }
    }
    EditCurves { shift_cents: out, gain_db: gain }
}

/// The note's centre after `edit` (for "now 12c" readouts): `cen + centre·(tgt − cen) + offset`,
/// or the measured centre when the note has fewer than 3 voiced frames (it gets no pitch change).
pub fn tuned_centre(a: &VocalAnalysis, note: &Note, edit: &NoteEdit, scale: &dyn ScaleAt) -> f32 {
    let e = edit.clone().clamped();
    match note_pitch(a, note, &e, scale) {
        Some(p) => (p.cen + f64::from(e.centre) * (p.tgt - p.cen) + f64::from(e.offset_cents)) as f32,
        None => note.centre_cents,
    }
}

/// The target the note is judged by after `edit`: its target ([`target_cents`] of the measured
/// centre, Snap to key followed) moved by the offset's whole semitones ([`moved_target`]); not
/// moved when the note has fewer than 3 voiced frames (it gets no pitch change).
pub fn tuned_target(a: &VocalAnalysis, note: &Note, edit: &NoteEdit, scale: &dyn ScaleAt) -> f32 {
    let e = edit.clone().clamped();
    match note_pitch(a, note, &e, scale) {
        Some(p) => moved_target(p.tgt as f32, e.offset_cents),
        None => {
            let snap = if e.snap_to_key { scale.scale_at(note.start) } else { None };
            target_cents(note.centre_cents, snap)
        }
    }
}

/// Before/after readouts for a selection of notes.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionSummary {
    pub count: usize,
    pub start_frame: u32,
    pub end_frame: u32,
    /// Mean |centre − target| before the edits (cents).
    pub avg_off_before: f32,
    /// Mean |tuned centre − target| after the edits (cents); a moved note's target moves with it
    /// ([`moved_target`]).
    pub avg_off_after: f32,
    /// Mean slow drift before (cents std).
    pub drift_before: f32,
    /// Mean slow drift after, `drift_cents·(1 − drift)` (cents std).
    pub drift_after: f32,
    /// Loudest minus quietest note level (dB).
    pub level_spread_db: f32,
    /// Median note level (dB): what levelling aims at.
    pub typical_db: f32,
}

/// The summary of notes `ids` under `edits` (missing edits count as none); `None` when no id
/// names a note.
pub fn selection_summary(a: &VocalAnalysis, edits: &BTreeMap<NoteId, NoteEdit>, ids: &[NoteId], scale: &dyn ScaleAt) -> Option<SelectionSummary> {
    let notes: Vec<&Note> = a.notes.iter().filter(|n| ids.contains(&n.id)).collect();
    if notes.is_empty() {
        return None;
    }
    let none = NoteEdit::default();
    let (mut off_b, mut off_a, mut dr_b, mut dr_a) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for n in &notes {
        let e = edits.get(&n.id).unwrap_or(&none).clone().clamped();
        // A note too short to pitch gets no move, so its target stays put.
        let (cen, tgt, moved) = match note_pitch(a, n, &e, scale) {
            Some(p) => (p.cen, p.tgt, f64::from(moved_target(p.tgt as f32, e.offset_cents))),
            None => {
                let snap = if e.snap_to_key { scale.scale_at(n.start) } else { None };
                let tgt = f64::from(target_cents(n.centre_cents, snap));
                (f64::from(n.centre_cents), tgt, tgt)
            }
        };
        off_b += (cen - tgt).abs();
        off_a += (f64::from(tuned_centre(a, n, &e, scale)) - moved).abs();
        dr_b += f64::from(n.drift_cents);
        dr_a += f64::from(n.drift_cents) * (1.0 - f64::from(e.drift));
    }
    let count = notes.len();
    let c = count as f64;
    let levels: Vec<f32> = notes.iter().map(|n| n.level_db).collect();
    let max = levels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min = levels.iter().copied().fold(f32::INFINITY, f32::min);
    Some(SelectionSummary {
        count,
        start_frame: notes.iter().map(|n| n.start).min().unwrap_or(0),
        end_frame: notes.iter().map(|n| n.end).max().unwrap_or(0),
        avg_off_before: (off_b / c) as f32,
        avg_off_after: (off_a / c) as f32,
        drift_before: (dr_b / c) as f32,
        drift_after: (dr_a / c) as f32,
        level_spread_db: if max.is_finite() && min.is_finite() { max - min } else { 0.0 },
        typical_db: typical_level_db(&levels),
    })
}
