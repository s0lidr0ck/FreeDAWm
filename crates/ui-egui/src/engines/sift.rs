//! Sift's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["sibAmount", "sibRange", "ploAmount", "ploRange"],
    hero: Some("sibAmount"),
    tile_labels: &[("sibAmount", "Sib"), ("sibRange", "S Range"), ("ploAmount", "Plo"), ("ploRange", "P Range")],
    wide_editor: Some(crate::editors::sift::draw),
    ..EngineUi::new("fs_sift", crate::editors::sift::ACCENT, GraphKind::Sift)
};
