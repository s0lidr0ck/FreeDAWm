//! Mend's UI hooks (a built-in, not a FireStation engine): its accent, the knobs its tile shows
//! and the drawer's large knob. Its status line is `crate::mend_ui`.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

/// Mend's accent (ARGB): a clear sea green.
pub const ACCENT: u32 = 0xFF5F_C9A6;

pub const UI: EngineUi = EngineUi { key_knobs: &["amount", "out"], hero: Some("amount"), ..EngineUi::new("mend", ACCENT, GraphKind::Level) };
