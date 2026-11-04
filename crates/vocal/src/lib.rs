//! SoundCraft Vocal: the vocal-tuning engine behind the Vocal Editor.
//!
//! Pure algorithms, called from worker threads only (never the audio thread). Every long-running
//! function takes a `&AtomicBool` cancel flag and returns [`VocalError::Cancelled`] promptly once
//! it is set.
//!
//! - [`track`]: a pYIN-style monophonic pitch tracker at 5 ms hops ([`track_pitch`], [`PitchTrack`]).
//! - [`note`] + [`segment`]: notes (with scoops) and unvoiced regions ([`analyse`], [`segment()`]),
//!   split/join ([`split_note`], [`join_notes`], [`apply_seg_ops`]) and [`carry_edits`].
//! - [`edit`]: per-note edits ([`NoteEdit`]) and their maths: targets ([`target_cents`]),
//!   [`decompose`], levelling ([`level_gain_db`]), the per-frame [`build_curves`] and the
//!   [`selection_summary`].
//! - [`filter`]: second-order Butterworth and zero-phase `filtfilt`.
//! - [`presets`]: [`factory_presets`] macros.
//! - [`metrics`]: objective measures: steady frames, harmonic flux (wobble), pitch error.
//! - [`resynth`]: the formant-preserving phase vocoder with partial render ([`resynth::render_spans`]).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::fmt;
use std::sync::atomic::AtomicBool;

pub mod edit;
pub mod filter;
pub mod metrics;
pub mod musical;
pub mod note;
pub mod presets;
pub mod resynth;
pub mod segment;
pub mod track;

pub use edit::*;
pub use note::*;
pub use presets::*;
pub use segment::*;
pub use track::*;

/// Tracks `samples`' pitch and segments it into notes and unvoiced regions (worker threads only).
///
/// Errors as [`track_pitch`] does.
pub fn analyse(samples: &[f32], sr: u32, cancel: &AtomicBool) -> Result<VocalAnalysis, VocalError> {
    let track = track_pitch(samples, sr, cancel)?;
    let (notes, unvoiced) = segment::segment(&track);
    Ok(VocalAnalysis { track, notes, unvoiced })
}

/// Why an analysis or render did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VocalError {
    /// The cancel flag was set.
    Cancelled,
    /// The input cannot be handled (the reason says why).
    Unsupported(String),
}

impl fmt::Display for VocalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VocalError::Cancelled => f.write_str("cancelled"),
            VocalError::Unsupported(why) => write!(f, "unsupported: {why}"),
        }
    }
}

impl std::error::Error for VocalError {}
