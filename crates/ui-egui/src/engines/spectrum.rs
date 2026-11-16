//! The spectrum meter plug-in's UI hooks (a built-in with an editor of its own).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    wide_editor: Some(crate::editors::meters::draw_spectrum),
    ..EngineUi::new("spectrum", crate::editors::meters::ACCENT, GraphKind::Level)
};
