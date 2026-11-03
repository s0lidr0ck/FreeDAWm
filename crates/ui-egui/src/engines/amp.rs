//! AmpEngine's UI hooks: the tile, drawer and accent follow the amp in use (Station's
//! `kAmpModels`).

use crate::engine_ui::EngineUi;
use crate::module_drawer::amp_of;
use crate::module_graphs::GraphKind;
use soundcraft_dsp::ParamInfo;
use soundcraft_dsp::plugins::fs::amp;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    model_accent: Some(model_accent),
    key_knobs: &amp::KEY_KNOBS[0],
    model_key_knobs: Some(model_key_knobs),
    tile_label: Some(tile_label),
    default_group: Some(default_group),
    model_switch: Some(crate::module_drawer::amp_switch),
    model_name: Some(model_name),
    wide_editor: Some(crate::editors::amp::draw),
    // (the Twin's; the tile follows the amp in use: `model_accent`)
    ..EngineUi::new("fs_amp", 0xFF8F_C9D6, GraphKind::Amp)
};

/// The accent of the amp in use.
fn model_accent(ins: &Insert) -> Option<u32> {
    amp::ACCENTS.get(amp_of(ins)).copied()
}

/// Each amp has its four.
fn model_key_knobs(ins: &Insert) -> Vec<&'static str> {
    amp::KEY_KNOBS[amp_of(ins)].to_vec()
}

/// Amp's knobs carry their amp in their names ("Volume (Twin)"); the tile is one amp's.
fn tile_label(p: &ParamInfo) -> String {
    crate::chain_view::short_param(amp::short_name(p.name))
}

/// The amp's own face.
fn default_group(ins: &Insert, groups: &[&'static str]) -> Option<&'static str> {
    let name = amp::AMP_NAMES[amp_of(ins)];
    groups.iter().copied().find(|g| *g == name)
}

/// The amp's short name (the tile's width has no room for the full one).
fn model_name(ins: &Insert) -> Option<String> {
    amp::AMP_NAMES.get(amp_of(ins)).map(|n| n.to_string())
}
