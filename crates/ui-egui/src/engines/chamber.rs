//! ChamberMade's UI hooks (its plan and value texts live in `chamber_plan`).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;
use soundcraft_dsp::ParamInfo;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["mix", "decay", "predelay", "level"],
    // Decay is the big knob beside Mix under ChamberMade's room panel.
    hero: Some("decay"),
    model_switch: Some(crate::module_drawer::chamber_switch),
    value_text: Some(value_text),
    user_presets: Some("ChamberMade"),
    wide_editor: Some(crate::editors::chamber::draw),
    ..EngineUi::new("fs_chamber", 0xFFFF_9A3D, GraphKind::Chamber)
};

/// ChamberMade's `formatParam` reads the built room.
fn value_text(ctx: &egui::Context, ins: &Insert, info: &ParamInfo, v: f32) -> Option<String> {
    Some(crate::chamber_plan::value_text(ctx, egui::Id::new(("chamber-text", ins.id.0)), ins, info, v))
}
