//! Heft's UI hooks (a built-in bass enhancer, not a FireStation engine: it has an editor of its own).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["freq", "harmonics", "original"],
    hero: Some("harmonics"),
    wide_editor: Some(crate::editors::heft::draw),
    ..EngineUi::new("heft", crate::editors::heft::ACCENT, GraphKind::Level)
};
