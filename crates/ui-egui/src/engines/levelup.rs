//! LevelUp's UI hooks (its graph lives in `levelup_ui`).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    // Station has no LevelUp: the editor's Level row (Target first, its biggest control).
    key_knobs: &["target", "attack", "release", "gate"],
    // Normalize and Learn are actions the editor presses (`momentary`), firing on the rising edge.
    momentary: &[("normalize", "Jump straight to the target, once (for Static mode)"), ("learn", "Take what is playing now as the Target")],
    user_presets: Some("LevelUp"),
    wide_editor: Some(crate::editors::levelup::draw),
    // LevelUp is not in Station: its editor's one cyan accent (`kAccent`, the Amorph look).
    ..EngineUi::new("fs_levelup", 0xFF3D_D6FF, GraphKind::LevelUp)
};
