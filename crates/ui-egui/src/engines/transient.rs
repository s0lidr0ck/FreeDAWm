//! TransientEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["attack", "sustain", "mix", "output"],
    model_param: Some("source"),
    user_presets: Some("TransientEngine"),
    wide_editor: Some(crate::editors::transient::draw),
    ..EngineUi::new("fs_transient", 0xFFC4_E86A, GraphKind::Transient)
};
