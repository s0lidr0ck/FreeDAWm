//! CopyQ's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &soundcraft_dsp::plugins::fs::copyq::KEY_KNOBS,
    tile_labels: &[("limit", "Max"), ("autoGain", "Level")],
    hero: Some("amount"),
    tips: &[
        ("capture", "Measure everything that plays from now on and pass the audio untouched; save it as a profile in the editor"),
        ("follow", "Keep measuring this track as it plays; off holds the match where it is"),
        ("link", "Send: publish this sound to the link group. Match: take the group's sound as the target. Group: both"),
    ],
    wide_editor: Some(crate::editors::copyq::draw),
    ..EngineUi::new("fs_copyq", 0xFFC9_9BFF, GraphKind::CopyQ)
};
