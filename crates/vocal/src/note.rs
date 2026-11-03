//! Notes and unvoiced regions over a [`PitchTrack`], and the per-note measures (port of the
//! spike's `note_core` and the power-mean level).

use serde::{Deserialize, Serialize};

use crate::track::PitchTrack;

/// A note's identity: its start frame when it was created (stable across measure updates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NoteId(pub u32);

/// One detected note: frames `[start, end)` of the track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    pub start: u32,
    /// Exclusive frame.
    pub end: u32,
    /// The scoop into the note: frames `[s0, start)`, adjacent to `start`.
    pub scoop: Option<(u32, u32)>,
    /// Robust centre pitch (MIDI × 100): median of the middle 60 % of the voiced frames.
    pub centre_cents: f32,
    /// Power-mean level of the note's frames (dB).
    pub level_db: f32,
    /// Slow pitch drift (cents std of [`crate::decompose`]'s slow part).
    pub drift_cents: f32,
    /// Fast pitch movement, vibrato/jitter (cents std of [`crate::decompose`]'s fast part).
    pub vibrato_cents: f32,
}

/// A run of unvoiced frames `[start, end)` that still carries level (breaths, consonants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnvoicedRegion {
    pub start: u32,
    /// Exclusive frame.
    pub end: u32,
}

/// `note_core`'s trim fraction at each end.
const CORE_TRIM: f64 = 0.2;
/// Level reported for a note with no frames (dB).
const EMPTY_LEVEL_DB: f32 = -120.0;

impl Note {
    /// A note over `[start, end)` with its measures taken from `track`.
    pub fn new(track: &PitchTrack, start: u32, end: u32, scoop: Option<(u32, u32)>) -> Note {
        let (centre_cents, level_db, drift_cents, vibrato_cents) = Note::measures(track, start, end);
        Note { id: NoteId(start), start, end, scoop, centre_cents, level_db, drift_cents, vibrato_cents }
    }

    /// Re-takes this note's measures from `track` (after a split or join changed its frames).
    pub fn remeasure(&mut self, track: &PitchTrack) {
        (self.centre_cents, self.level_db, self.drift_cents, self.vibrato_cents) = Note::measures(track, self.start, self.end);
    }

    /// `(centre, level, drift, vibrato)` of frames `[start, end)` (clamped to the track).
    ///
    /// - centre: the spike's `note_core` (trim 0.2): the median of the voiced frames after dropping
    ///   `floor(n·0.2)` at each end, or of all of them when fewer than 4 would remain (0.0 when none
    ///   is voiced);
    /// - level: `10·log10(mean(10^(L/10)))` over all the frames;
    /// - drift, vibrato: the (population) std of [`crate::decompose`]'s slow and fast parts over
    ///   the voiced frames (0.0 when none is voiced).
    pub fn measures(track: &PitchTrack, start: u32, end: u32) -> (f32, f32, f32, f32) {
        let frames = frames_in(track, start, end);
        let cents: Vec<f64> = frames.iter().filter_map(|f| f.cents).filter(|c| c.is_finite()).map(f64::from).collect();
        let centre = note_core(&cents).unwrap_or(0.0) as f32;
        // Drift and vibrato: the std of decompose()'s slow and fast parts over the voiced frames.
        let (_, slow, fast) = crate::edit::decompose64(&cents);
        let (drift, vibrato) = (pop_std(&slow) as f32, pop_std(&fast) as f32);

        let powers: Vec<f64> = frames.iter().map(|f| if f.level_db.is_finite() { 10f64.powf(f64::from(f.level_db) / 10.0) } else { 0.0 }).collect();
        let level = if powers.is_empty() {
            EMPTY_LEVEL_DB
        } else {
            let mean = powers.iter().sum::<f64>() / powers.len() as f64;
            if mean > 0.0 { (10.0 * mean.log10()) as f32 } else { EMPTY_LEVEL_DB }
        };
        (centre, level, drift, vibrato)
    }
}

/// The track's frames `[start, end)`, clamped.
pub(crate) fn frames_in(track: &PitchTrack, start: u32, end: u32) -> &[crate::track::PitchFrame] {
    let len = track.frames.len();
    let s = (start as usize).min(len);
    let e = (end as usize).clamp(s, len);
    track.frames.get(s..e).unwrap_or(&[])
}

/// The spike's `note_core` (trim 0.2): the median of `cents` after dropping `floor(n·0.2)` at
/// each end, or of all of them when fewer than 4 would remain; `None` when empty.
pub(crate) fn note_core(cents: &[f64]) -> Option<f64> {
    let n = cents.len();
    let k = (n as f64 * CORE_TRIM) as usize;
    let core = if n >= 2 * k + 4 { cents.get(k..n - k).unwrap_or(cents) } else { cents };
    median(core)
}

/// Population standard deviation (numpy's default); 0.0 when empty.
fn pop_std(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let m = xs.iter().sum::<f64>() / xs.len() as f64;
    (xs.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / xs.len() as f64).sqrt()
}

/// numpy's median: the mean of the two middle values for an even count; `None` when empty.
pub(crate) fn median(xs: &[f64]) -> Option<f64> {
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        return None;
    }
    if n % 2 == 1 { v.get(n / 2).copied() } else { Some((v.get(n / 2 - 1)? + v.get(n / 2)?) / 2.0) }
}
