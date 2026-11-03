//! Twine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["drive", "clip", "ceiling", "release"],
    hero: Some("ceiling"),
    user_presets: Some("Twine"),
    wide_editor: Some(crate::editors::twine::draw),
    ..EngineUi::new("fs_twine", 0xFFD8_B25C, GraphKind::Twine)
};
