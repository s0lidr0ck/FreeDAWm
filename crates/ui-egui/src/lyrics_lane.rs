//! A track's timed words, drawn as a strip along the bottom of its lane in the Edit window: each
//! word a box at its time, which can be clicked, dragged, retyped and so on as in the Vocal
//! Editor's word lane ([`crate::word_lane`], which draws and runs it: `word_lane::track_strip`).

/// The strip's height.
pub const STRIP_H: f32 = 14.0;
