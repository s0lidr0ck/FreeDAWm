//! A stretch heard in real time while its offline render is made (see the engine's
//! `stretch_jobs`): the render's source is reserved, and until its audio is in the pool the
//! mixer reads the original through the real-time stretcher by this entry's map.

use crate::{Clip, SourceId};
use soundcraft_time::Samples;

/// How to hear a reserved stretch render before it exists, and how to make it.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveStretch {
    /// The original audio: what the render is made from and what is heard meanwhile.
    pub source: SourceId,
    /// `(file position in source, position in the render)` points, in render order; between
    /// and past them the stretch is linear ([`soundcraft_time`] samples, fractional).
    pub map: Vec<(f64, f64)>,
    /// The clip whose window the render is made from (its Clean Vocal mix, its channel).
    pub base: Clip,
    /// The window's first file position and its length.
    pub start: Samples,
    pub len: usize,
    /// What the render is.
    pub kind: StretchKind,
}

/// The offline render a live stretch stands in for.
#[derive(Debug, Clone, PartialEq)]
pub enum StretchKind {
    /// The whole window stretched by `ratio` to `frames` samples, by the Elastic algorithm
    /// `mode` (an id; `None` the default), as Elastic Properties, Conform and TCE render.
    Ratio { ratio: f64, mode: Option<String>, frames: Samples },
    /// Warp markers `(input from the window start, output)`, by the Elastic algorithm `mode`.
    Warp { marks: Vec<(Samples, Samples)>, mode: Option<String> },
}

impl StretchKind {
    /// The render's length in samples.
    pub fn frames(&self) -> Samples {
        match self {
            StretchKind::Ratio { frames, .. } => *frames,
            StretchKind::Warp { marks, .. } => marks.last().map_or(0, |m| m.1),
        }
    }

    /// The map from render positions to file positions, for a window starting at file
    /// position `start` (see [`LiveStretch::map`]).
    pub fn map(&self, start: Samples, len: usize) -> Vec<(f64, f64)> {
        match self {
            StretchKind::Ratio { frames, .. } => vec![(start as f64, 0.0), (start as f64 + len as f64, *frames as f64)],
            StretchKind::Warp { marks, .. } => marks.iter().map(|m| (start as f64 + m.0 as f64, m.1 as f64)).collect(),
        }
    }
}
