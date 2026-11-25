//! Plumb's UI hooks (a built-in pitch corrector, not a FireStation engine: it has an editor
//! of its own).

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["retune", "humanize", "mix"],
    hero: Some("retune"),
    model_param: Some("style"),
    tips: &[
        ("style", "Natural: gentle, keeps the vibrato. Hard tune: snaps at once, flattens the vibrato."),
        ("key", "Follow the song (the Key ruler, key changes included) or a key of its own."),
        ("hold", "How much closer the voice must get to another note before the target changes."),
    ],
    wide_editor: Some(crate::editors::autopitch::draw),
    ..EngineUi::new("autopitch", crate::editors::autopitch::ACCENT, GraphKind::Level)
};
