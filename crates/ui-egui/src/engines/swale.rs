//! Swale's UI hooks (its graph and switches live in `swale_ui`).

use crate::engine_ui::EngineUi;
use crate::module_drawer::Switch;
use crate::module_graphs::GraphKind;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["attack", "hold", "release", "mix"],
    hero_applies: Some(hero_applies),
    model_switch: Some(crate::swale_ui::switch),
    // Its detector knobs in Rhythmic, which has no trigger (the editor dims them).
    locked: Some(crate::swale_ui::locked),
    model_param: Some("mode"),
    user_presets: Some("Swale"),
    wide_editor: Some(crate::editors::swale::draw),
    ..EngineUi::new("fs_swale", 0xFFF2_B84B, GraphKind::Swale)
};

/// Swale's Attack knob gives way to its note while Attack Sync is on.
fn hero_applies(ins: &Insert) -> bool {
    crate::module_drawer::model_switch(ins, "attack") != Some(Switch::Hide)
}
