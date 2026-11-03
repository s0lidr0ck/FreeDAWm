//! CleanQ's UI hooks (its ceiling card lives in `cleanq_ceiling`).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    // The curve is the hero; the output trim and the dynamic ceiling's Thresh and Amount.
    key_knobs: &soundcraft_dsp::plugins::fs::cleanq::KEY_KNOBS,
    // Its ceiling knobs wear its Ceiling card's names (`CleanQCeilingCard.h`).
    tile_labels: &[("outGain", "Output"), ("ceilThr", "Thresh"), ("ceilAmount", "Amount")],
    user_presets: Some("CleanQ"),
    wide_editor: Some(crate::editors::cleanq::draw),
    ..EngineUi::new("fs_cleanq", 0xFF5E_D1FF, GraphKind::CleanQ)
};
