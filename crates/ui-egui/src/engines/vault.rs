//! Vault's UI hooks (a built-in, not a FireStation engine): its accent, the knobs its tile
//! shows and the drawer's large knob. Its response picker is `crate::vault_ui`.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

/// Vault's accent (ARGB): a cool stone blue.
pub const ACCENT: u32 = 0xFF7F_A8C9;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["mix", "decay", "predelay", "high_cut"],
    hero: Some("mix"),
    tile_labels: &[("predelay", "Pre"), ("low_cut", "Lo Cut"), ("high_cut", "Hi Cut")],
    ..EngineUi::new("vault", ACCENT, GraphKind::Level)
};
