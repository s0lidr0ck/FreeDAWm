//! VintEngine's UI hooks.

use crate::engine_ui::EngineUi;
use crate::module_drawer::{Switch, model_index, show_switch};
use crate::module_graphs::GraphKind;
use soundcraft_model::Insert;

pub const UI: EngineUi = EngineUi {
    key_knobs: &["drive", "mix", "output"],
    hero: Some("drive"),
    hero_applies: Some(hero_applies),
    default_group: Some(default_group),
    model_switch: Some(model_switch),
    model_param: Some("circuit"),
    user_presets: Some("VintEngine"),
    wide_editor: Some(crate::editors::vint::draw),
    ..EngineUi::new("fs_vint", 0xFFD9_B26A, GraphKind::Vint)
};

/// VintEngine's output stage per circuit (`stageFor`), as the drawer names its switch; `None`:
/// the circuit has none (the Passives run ideal, the Sampler's converter is its colour), and the
/// editor greys the switch out.
fn vint_stage_label(circuit: i32) -> Option<&'static str> {
    use soundcraft_dsp::plugins::fs::vint::{vint_engine_dsp::stage_for, vint_stages as st};
    match stage_for(circuit) {
        st::ST_VALVE => Some("Valve Stage"),
        st::ST_CLASS_A => Some("Class A Stage"),
        st::ST_DISCRETE => Some("Discrete Stage"),
        st::ST_OP_AMP => Some("Op-Amp Stage"),
        _ => None,
    }
}

/// Vint's output stage per circuit.
fn model_switch(ins: &Insert, id: &str) -> Option<Switch> {
    (id == "stage").then(|| show_switch(vint_stage_label(model_index(ins, "circuit")).map(|l| (l, &["Off", "On"][..]))))
}

/// Vint's drive feeds its output stage, so a circuit without one ignores it, and the Console's
/// drive is its gain switch (`conPre`); the editor greys the dial out for both (`applyCircuit`).
fn hero_applies(ins: &Insert) -> bool {
    let circuit = model_index(ins, "circuit");
    vint_stage_label(circuit).is_some() && circuit != soundcraft_dsp::plugins::fs::vint::vint_engine_dsp::C_CONSOLE
}

/// Vint's current circuit (its editor shows that circuit's panel; the MEQ-5, "Passive Mid", lives
/// in the Passive tab).
fn default_group(ins: &Insert, groups: &[&'static str]) -> Option<&'static str> {
    let circuit = model_index(ins, "circuit");
    let want = if circuit == soundcraft_dsp::plugins::fs::vint::vint_engine_dsp::C_PASSIVE_MID {
        Some("Passive".to_string())
    } else {
        soundcraft_dsp::plugin_info(&ins.plugin)
            .and_then(|i| i.param("circuit"))
            .and_then(|p| p.choices.get(circuit.max(0) as usize))
            .map(|c| c.to_string())
    };
    want.and_then(|w| groups.iter().copied().find(|g| *g == w))
}
