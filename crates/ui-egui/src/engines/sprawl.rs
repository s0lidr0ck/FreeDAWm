//! Sprawl's UI hooks (a built-in, not a FireStation engine: it has an editor of its own).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["width", "stereoize", "b1_width", "b4_width"],
    hero: Some("width"),
    tile_labels: &[("stereoize", "Stereo"), ("b1_width", "Low"), ("b2_width", "Lo Mid"), ("b3_width", "Hi Mid"), ("b4_width", "High")],
    wide_editor: Some(crate::editors::sprawl::draw),
    ..EngineUi::new("sprawl", crate::editors::sprawl::ACCENT, GraphKind::Level)
};
