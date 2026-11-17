//! Keel's UI hooks (a built-in bass amp, not a FireStation engine: it has an editor of its own).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["drive", "bass", "mid", "treble"],
    hero: Some("drive"),
    model_param: Some("voicing"),
    wide_editor: Some(crate::editors::keel::draw),
    ..EngineUi::new("keel", crate::editors::keel::ACCENT, GraphKind::Level)
};
