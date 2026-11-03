//! Shale's UI hooks (its graph, switches and value texts live in `shale_ui`).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;
use soundcraft_dsp::ParamInfo;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    // Station has no Shale: the editor's main dial row (its DeBleed fifth stays in the drawer).
    key_knobs: &["threshold", "sens", "length", "reduction"],
    model_switch: Some(crate::shale_ui::switch),
    // Its detector band and crossover while Auto Band gives them to the Mic.
    locked: Some(crate::shale_ui::locked),
    value_text: Some(value_text),
    model_param: Some("instrument"),
    tips: &[("midiOut", crate::shale_ui::MIDI_OUT_TIP), ("midiNote", "The note every hit sends (channel 10)")],
    user_presets: Some("Shale"),
    wide_editor: Some(crate::editors::shale::draw),
    // Shale is not in Station: its editor's `kAccent`.
    ..EngineUi::new("fs_shale", 0xFFC8_A06A, GraphKind::Shale)
};

/// Shale's `formatParam` reads the Mic and Auto Band for Length, Hold and the bands.
fn value_text(_: &egui::Context, ins: &Insert, info: &ParamInfo, v: f32) -> Option<String> {
    Some(crate::shale_ui::value_text(ins, info, v))
}
