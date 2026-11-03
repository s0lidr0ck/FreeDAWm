//! CompEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_drawer::{Switch, model_index, show_switch};
use crate::module_graphs::GraphKind;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["amount", "ratio", "makeup", "dry"],
    hero: Some("amount"),
    model_switch: Some(model_switch),
    model_param: Some("style"),
    user_presets: Some("CompEngine"),
    wide_editor: Some(crate::editors::comp::draw),
    ..EngineUi::new("fs_comp", 0xFF79_D2BE, GraphKind::Comp)
};

/// CompEngine's `switchDef` (`CompConfig.h`): the two style-specific switches per style.
fn comp_switch_def(kind: &str, style: i32) -> Option<(&'static str, &'static [&'static str])> {
    match (kind, style) {
        ("variant", 1) => Some(("Revision", &["Blue stripe", "Blackface"])),
        ("variant", 6) => Some(("Circuit", &["Ross", "Dyna Comp"])),
        ("variant", 7) => Some(("Audio", &["Clean", "Dist 2", "Dist 3"])),
        ("variant", 8) => Some(("Thrust", &["Norm", "Med", "Loud"])),
        ("detMode", 2) => Some(("Emphasis", &["Flat", "HF"])),
        ("detMode", 7) => Some(("Detector", &["Norm", "HP", "Band", "HP + Band"])),
        ("detMode", 8) => Some(("Type", &["New", "Old"])),
        _ => None,
    }
}

/// Comp's variant and detector per lane style.
fn model_switch(ins: &Insert, id: &str) -> Option<Switch> {
    let (kind, style) = match id {
        "variant" => ("variant", "style"),
        "bVariant" => ("variant", "bStyle"),
        "cVariant" => ("variant", "cStyle"),
        "detMode" => ("detMode", "style"),
        "bDetMode" => ("detMode", "bStyle"),
        "cDetMode" => ("detMode", "cStyle"),
        _ => return None,
    };
    Some(show_switch(comp_switch_def(kind, model_index(ins, style))))
}
