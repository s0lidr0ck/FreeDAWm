//! Factory macro presets: one-click settings for Centre, Drift, Vibrato, Raise and Lower.

use serde::{Deserialize, Serialize};

/// A named macro setting applied to a selection of notes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroPreset {
    pub name: String,
    pub centre: f32,
    pub drift: f32,
    pub vibrato: f32,
    pub raise: f32,
    pub lower: f32,
}

/// The built-in presets, in menu order.
pub fn factory_presets() -> Vec<MacroPreset> {
    let p = |name: &str, centre, drift, vibrato, raise, lower| MacroPreset { name: name.to_owned(), centre, drift, vibrato, raise, lower };
    vec![
        // The mockup's first pass.
        p("First pass (gentle)", 0.40, 0.30, -0.10, 0.25, 0.25),
        // The spike's calibration against Melodyne at 70 %.
        p("Like Melodyne 70 %", 0.40, 0.30, 0.0, 0.25, 0.25),
        p("Firm", 0.70, 0.60, 0.0, 0.40, 0.40),
    ]
}
