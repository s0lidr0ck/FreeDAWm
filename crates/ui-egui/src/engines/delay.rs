//! DelayEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_drawer::{Switch, model_index};
use crate::module_graphs::GraphKind;
use soundcraft_dsp::ParamInfo;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &soundcraft_dsp::plugins::fs::delay::KEY_KNOBS,
    hero_applies: Some(hero_applies),
    model_switch: Some(crate::module_drawer::delay_switch),
    value_text: Some(value_text),
    // The C++ holds Tap for 60 ms.
    momentary: &[("tap", crate::module_drawer::TAP_TIP)],
    model_param: Some("style"),
    user_presets: Some("DelayEngine"),
    wide_editor: Some(crate::editors::delay::draw),
    ..EngineUi::new("fs_delay", 0xFFFF_B454, GraphKind::Delay)
};

/// Delay's hero is Time, which Sync swaps for the Note (`whatApplies`).
fn hero_applies(ins: &Insert) -> bool {
    crate::module_drawer::model_switch(ins, "time") != Some(Switch::Hide)
}

/// Mod Rate is a note value while Mod Sync is on (`formatValue`).
fn value_text(_: &egui::Context, ins: &Insert, info: &ParamInfo, v: f32) -> Option<String> {
    (info.id == "modRate" && model_index(ins, "modSync") == 1).then(|| soundcraft_dsp::plugins::fs::delay::delay_config::mod_rate_sync_text(v))
}
