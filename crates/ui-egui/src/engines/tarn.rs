//! Tarn's UI hooks. It has no wide editor yet: the drawer's knobs and its gain-curve graph.

use crate::engine_ui::EngineUi;
use crate::module_graphs::GraphKind;

pub const UI: EngineUi = EngineUi { key_knobs: &["tame", "recover", "bias", "brighten"], hero: Some("tame"), ..EngineUi::new("fs_tarn", 0xFF6F_C7B8, GraphKind::Tarn) };
