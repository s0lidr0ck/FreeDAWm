//! Lull's UI hooks (a built-in, not a FireStation engine: it has an editor of its own).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["threshold", "range", "release", "mix"],
    hero: Some("threshold"),
    tile_labels: &[("threshold", "Thresh")],
    model_param: Some("mode"),
    wide_editor: Some(crate::editors::lull::draw),
    ..EngineUi::new("lull", crate::editors::lull::ACCENT, GraphKind::Level)
};
