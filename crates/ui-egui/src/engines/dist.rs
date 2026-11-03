//! DistEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_drawer::{Switch, model_index};
use crate::module_graphs::GraphKind;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["drive", "tone", "level"],
    model_switch: Some(model_switch),
    locked: Some(level_linked),
    user_presets: Some("DistEngine"),
    wide_editor: Some(crate::editors::dist::draw),
    ..EngineUi::new("fs_dist", 0xFFFF_7A2F, GraphKind::Dist)
};

/// Dist names its dials and switches per circuit.
fn model_switch(ins: &Insert, id: &str) -> Option<Switch> {
    crate::module_drawer::dist_switch(model_index(ins, "circuit"), id)
}

/// Level while Link is on, as the editor locks the dial.
fn level_linked(ins: &Insert, id: &str) -> bool {
    id == "level" && ins.params.get("link").copied().unwrap_or(0.0) >= 0.5
}
