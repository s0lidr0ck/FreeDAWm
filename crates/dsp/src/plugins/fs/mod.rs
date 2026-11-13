//! FireStation engine ports: shared preset import and the factory-preset registry.

use crate::{Plugin, PluginInfo, Unit};
use std::collections::BTreeMap;

// The engine modules (`pub mod <engine>;`, one per folder) and `BY_FOLDER`, from `build.rs`.
include!(concat!(env!("OUT_DIR"), "/fs_engines.rs"));

/// One FireStation engine's registration: what the shared lists (the plugin registry, factory
/// presets, fidelity, the xtask's C++ tools) need from it. Each engine module declares
/// `pub const ENGINE: EngineReg`, and `build.rs` collects every `fs/<engine>/` folder, so adding
/// an engine touches only its own folder.
#[derive(Clone, Copy)]
pub struct EngineReg {
    pub info: &'static PluginInfo,
    /// Constructs an (unprepared) instance.
    pub make: fn() -> Box<dyn Plugin>,
    /// The bundled factory presets as `(name, json)` pairs.
    pub presets: &'static [(&'static str, &'static str)],
    /// The engine's place in the plugin menu among the FireStation engines (lower first). Unique:
    /// two engines with the same `order` fail the build.
    pub order: u16,
    /// Where the C++ original lives and how it is rendered for the fidelity test.
    pub fidelity: Option<Fidelity>,
    /// Works out a stored state for `Plugin::set_prepared_state` (see `crate::prepare_state`).
    pub prepare_state: Option<fn(&[u8]) -> Vec<f64>>,
    /// The actions the engine takes through `Plugin::action` (see `crate::plugin_actions`).
    pub actions: &'static [&'static str],
}

/// The C++ original of an engine, for `cargo xtask fidelity-refs` / `fs-tables` and the fidelity
/// test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fidelity {
    /// The engine's name in `tests/fidelity/<name>/` and on the xtask command line.
    pub name: &'static str,
    /// The engine's folder under the FireStation root.
    pub source_dir: &'static str,
    /// The render tool, without `_Render.exe`.
    pub render_tool: &'static str,
    /// Tables `cargo xtask fs-tables` converts: (C++ header under `source_dir`, Rust file under
    /// the repo root).
    pub tables: &'static [(&'static str, &'static str)],
}

/// How many engines there are.
pub const ENGINE_COUNT: usize = BY_FOLDER.len();

/// The engines in menu order (by [`EngineReg::order`]).
pub(crate) const BY_ORDER: [&EngineReg; ENGINE_COUNT] = by_order(BY_FOLDER);

static ENGINES: [&EngineReg; ENGINE_COUNT] = BY_ORDER;
static MODULES: [(&str, &EngineReg); ENGINE_COUNT] = BY_FOLDER;

/// Sorts the registrations by `order` at compile time (a tie fails the build).
const fn by_order<const N: usize>(by_folder: [(&'static str, &'static EngineReg); N]) -> [&'static EngineReg; N] {
    // N >= 1: there are engines (indexing [0] of an empty list would fail the build too).
    let mut out = [by_folder[0].1; N];
    let mut i = 0;
    while i < N {
        out[i] = by_folder[i].1;
        i += 1;
    }
    let mut i = 1;
    while i < N {
        let mut j = i;
        while j > 0 && out[j - 1].order > out[j].order {
            let t = out[j - 1];
            out[j - 1] = out[j];
            out[j] = t;
            j -= 1;
        }
        i += 1;
    }
    let mut i = 1;
    while i < N {
        assert!(out[i - 1].order != out[i].order, "two FireStation engines have the same `order`");
        i += 1;
    }
    out
}

/// Every engine's registration, in menu order.
pub fn engines() -> &'static [&'static EngineReg] {
    &ENGINES
}

/// Every engine's registration with its module name (`mod_engine` for Mod), by module name.
pub fn engine_modules() -> &'static [(&'static str, &'static EngineReg)] {
    &MODULES
}

/// The registration of the engine with this plugin id.
pub fn engine(plugin_id: &str) -> Option<&'static EngineReg> {
    ENGINES.iter().copied().find(|e| e.info.id == plugin_id)
}

/// The id of every engine's own bypass parameter (kept in the parameter order, hidden in the UI).
const ENGINE_BYPASS: &str = "bypass";

/// A FireStation preset, resolved against a plugin's parameter table.
#[derive(Debug, Clone, PartialEq)]
pub struct FsPreset {
    pub name: String,
    pub group: String,
    /// Parameter id to value (choice labels already mapped to indices, booleans to 0/1).
    pub values: BTreeMap<String, f32>,
}

impl FsPreset {
    /// Parses FireStation preset JSON (`{"name":..,"group":..,"values":{..}}`) against `info`.
    /// Choice labels map to indices (case-insensitive), booleans to 0/1. Unknown keys and
    /// non-finite or otherwise unusable values are dropped, and so is the engine's own `bypass`:
    /// the UI hides it (the module's bypass is used instead), so a preset that set it would
    /// silence the module with nothing to undo it. `None` if the JSON is not an object.
    pub fn parse(json: &str, info: &PluginInfo) -> Option<FsPreset> {
        let root: serde_json::Value = serde_json::from_str(json).ok()?;
        let obj = root.as_object()?;
        let text = |k: &str| obj.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut values = BTreeMap::new();
        if let Some(vals) = obj.get("values").and_then(|v| v.as_object()) {
            for (key, raw) in vals {
                if key == ENGINE_BYPASS {
                    continue;
                }
                let Some(param) = info.param(key) else { continue };
                let v = match raw {
                    serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                    serde_json::Value::Number(n) => n.as_f64().map(|x| x as f32),
                    serde_json::Value::String(s) if param.unit == Unit::Choice => {
                        param.choices.iter().position(|c| c.eq_ignore_ascii_case(s)).map(|i| i as f32)
                    }
                    _ => None,
                };
                if let Some(v) = v.filter(|v| v.is_finite()) {
                    values.insert(key.clone(), v);
                }
            }
        }
        Some(FsPreset { name: text("name"), group: text("group"), values })
    }
}

/// The bundled factory presets of a plugin as `(name, json)` pairs: a FireStation engine's, or
/// Rez's own (a built-in with presets in the same form). Empty for any other id.
pub fn factory_presets(plugin_id: &str) -> &'static [(&'static str, &'static str)] {
    match engine(plugin_id) {
        Some(e) => e.presets,
        None if plugin_id == super::rez::REZ_INFO.id => super::rez::presets::factory_presets(),
        None => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{fs_choice, fs_toggle, knob01};
    use crate::{Category, ParamInfo};

    fn pct(v: f32) -> String {
        format!("{:.0}%", v * 100.0)
    }

    static PARAMS: [ParamInfo; 4] = [
        fs_choice("circuit", "Circuit", 0.0, "Main", &["Iron", "Valve", "Desk"]),
        knob01("drive", "Drive", 0.5, "Main", pct),
        fs_toggle("autoGain", "Auto Gain", 0.0, "Main"),
        fs_toggle("bypass", "Bypass", 0.0, ""),
    ];
    static INFO: PluginInfo = PluginInfo {
        id: "test_sat",
        name: "Test",
        short_name: "Test",
        category: Category::FireStation,
        params: &PARAMS,
        is_instrument: false,
        audiosuite: false,
        sidechain: false,
        link: None,
        references: 0,
        midi_out: false,
    };

    #[test]
    fn fs_preset_maps_labels_and_drops_junk() {
        let json = r#"{"engine":"SatEngine","name":"Body","group":"Guitars",
            "values":{"circuit":"valve","drive":0.4667,"autoGain":true,"nope":1,"bad":null}}"#;
        let p = FsPreset::parse(json, &INFO).unwrap();
        assert_eq!(p.name, "Body");
        assert_eq!(p.group, "Guitars");
        assert_eq!(p.values["circuit"], 1.0);
        assert!((p.values["drive"] - 0.4667).abs() < 1e-6);
        assert_eq!(p.values["autoGain"], 1.0);
        assert!(!p.values.contains_key("nope") && !p.values.contains_key("bad"));
    }

    #[test]
    fn fs_preset_numeric_booleans_unknown_labels_and_garbage() {
        let json = r#"{"name":"x","values":{"autoGain":0,"circuit":"Nonsense"}}"#;
        let p = FsPreset::parse(json, &INFO).unwrap();
        assert_eq!(p.values["autoGain"], 0.0);
        assert!(!p.values.contains_key("circuit"));
        assert!(FsPreset::parse("not json", &INFO).is_none());
        assert!(FsPreset::parse("[1]", &INFO).is_none());
    }

    #[test]
    fn fs_preset_never_carries_the_engine_bypass() {
        // The engine's own bypass has no control in the UI: a preset (factory or imported) that
        // set it would silence the module with no way to undo it.
        for raw in ["true", "1", "1.0", "false"] {
            let json = format!(r#"{{"name":"x","values":{{"bypass":{raw},"drive":0.25}}}}"#);
            let p = FsPreset::parse(&json, &INFO).unwrap();
            assert!(!p.values.contains_key("bypass"), "{raw}");
            assert_eq!(p.values["drive"], 0.25);
        }
        // And no bundled factory preset of any port carries it.
        let ported: Vec<&crate::PluginInfo> = crate::plugins().iter().copied().filter(|p| p.category == crate::Category::FireStation).collect();
        assert!(ported.len() >= 14, "{}", ported.len());
        for info in ported {
            let id = info.id;
            for (name, json) in factory_presets(id) {
                let p = FsPreset::parse(json, info).unwrap();
                assert!(!p.values.contains_key("bypass"), "{id} {name}");
            }
        }
    }

    #[test]
    fn unknown_plugin_has_no_factory_presets() {
        assert!(factory_presets("no_such_plugin").is_empty());
    }

    #[test]
    fn display_overrides_format() {
        fn db(v: f32) -> String {
            format!("{:+.1} dB", v * 24.0 - 12.0)
        }
        let p = knob01("out", "Output", 0.5, "Output", db);
        assert_eq!(p.format(0.75), "+6.0 dB");
        assert_eq!(p.format(f32::NAN), "+0.0 dB");
        assert_eq!(p.group, "Output");
    }
}
