//! Manners' UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi { key_knobs: &["tame", "recover", "bias", "brighten"], hero: Some("tame"), wide_editor: Some(crate::editors::manners::draw), ..EngineUi::new("fs_manners", 0xFF6F_C7B8, GraphKind::Manners) };
