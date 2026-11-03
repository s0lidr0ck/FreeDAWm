//! ModEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &soundcraft_dsp::plugins::fs::mod_engine::KEY_KNOBS,
    // Depth is ModEngine's drive-role knob (`kDriveName`).
    hero: Some("depth"),
    model_switch: Some(crate::module_drawer::mod_switch),
    locked: Some(level_linked),
    model_param: Some("circuit"),
    user_presets: Some("ModEngine"),
    wide_editor: Some(crate::editors::mod_engine::draw),
    ..EngineUi::new("fs_mod", 0xFF8F_D3C6, GraphKind::Mod)
};

/// Level while Link is on, as the editor locks the dial.
fn level_linked(ins: &Insert, id: &str) -> bool {
    id == "level" && ins.params.get("link").copied().unwrap_or(0.0) >= 0.5
}
