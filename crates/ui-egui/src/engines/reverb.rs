//! ReverbEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &soundcraft_dsp::plugins::fs::reverb::KEY_KNOBS,
    model_switch: Some(crate::module_drawer::reverb_switch),
    model_param: Some("machine"),
    user_presets: Some("ReverbEngine"),
    wide_editor: Some(crate::editors::reverb::draw),
    ..EngineUi::new("fs_reverb", 0xFF6F_D3E8, GraphKind::Reverb)
};
