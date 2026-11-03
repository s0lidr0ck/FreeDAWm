//! SatEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_drawer::{Switch, model_index, show_switch};
use crate::module_graphs::GraphKind;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["drive", "bias", "mix", "output"],
    hero: Some("drive"),
    model_switch: Some(model_switch),
    model_param: Some("circuit"),
    user_presets: Some("SatEngine"),
    wide_editor: Some(crate::editors::sat::draw),
    ..EngineUi::new("fs_sat", 0xFFF0_8CA8, GraphKind::Console)
};

/// SatEngine's variant names per circuit (`SatConfig.h`).
const SAT_VARIANTS: [&[&str]; 5] = [
    &["Nickel", "Steel", "Ferrite"],
    &["Triode", "Pentode", "Starved"],
    &["Clean", "Vintage", "Driven"],
    &["Germanium", "Silicon", "Mixed"],
    &["Cold", "Stock", "Hot"],
];

/// Sat's variant per circuit.
fn model_switch(ins: &Insert, id: &str) -> Option<Switch> {
    (id == "variant").then(|| show_switch(SAT_VARIANTS.get(model_index(ins, "circuit").clamp(0, 4) as usize).map(|o| ("Variant", *o))))
}
