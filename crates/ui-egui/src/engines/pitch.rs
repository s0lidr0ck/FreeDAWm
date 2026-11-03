//! PitchEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["dry", "octDown", "octUp", "level"],
    hero: Some("dry"),
    model_switch: Some(crate::module_drawer::pitch_switch),
    model_param: Some("style"),
    user_presets: Some("PitchEngine"),
    wide_editor: Some(crate::editors::pitch::draw),
    ..EngineUi::new("fs_pitch", 0xFF7B_D4FF, GraphKind::Pitch)
};
