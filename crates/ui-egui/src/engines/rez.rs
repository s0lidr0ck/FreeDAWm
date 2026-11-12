//! Rez's UI hooks (a built-in, not a FireStation engine: it has an editor of its own).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["depth", "sharpness", "selectivity", "mix"],
    hero: Some("depth"),
    tile_labels: &[("sharpness", "Sharp"), ("selectivity", "Select")],
    wide_editor: Some(crate::editors::rez::draw),
    ..EngineUi::new("rez", crate::editors::rez::ACCENT, GraphKind::Level)
};
