//! FilterEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["cutoff", "reso", "drive", "mix"],
    model_switch: Some(crate::module_drawer::filter_switch),
    model_param: Some("circuit"),
    user_presets: Some("FilterEngine"),
    wide_editor: Some(crate::editors::filter::draw),
    ..EngineUi::new("fs_filter", 0xFF35_E0C2, GraphKind::Filter)
};
