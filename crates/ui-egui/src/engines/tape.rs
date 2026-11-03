//! TapeEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_drawer::{Switch, model_index, show_switch};
use crate::module_graphs::GraphKind;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["drive", "bias", "flutter", "mix"],
    tile_labels: &[("flutter", "W&F")],
    hero: Some("drive"),
    model_switch: Some(model_switch),
    model_param: Some("machine"),
    user_presets: Some("TapeEngine"),
    wide_editor: Some(crate::editors::tape::draw),
    ..EngineUi::new("fs_tape", 0xFFB9_A2FF, GraphKind::Console)
};

/// TapeEngine's tape formulas per machine (`TapeConfig.h`): the reel formulas, the cassette types.
const TAPE_REEL: &[&str] = &["Standard", "High Output", "Vintage"];
const TAPE_CASSETTE: &[&str] = &["Type I", "Type II", "Type IV"];

/// Tape's formula per machine.
fn model_switch(ins: &Insert, id: &str) -> Option<Switch> {
    (id == "formula").then(|| show_switch(Some(("Tape", if model_index(ins, "machine") == 3 { TAPE_CASSETTE } else { TAPE_REEL }))))
}
