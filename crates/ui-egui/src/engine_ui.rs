//! The FireStation engines' UI hooks: one [`EngineUi`] per engine, in its own file
//! `src/engines/<engine>.rs` (`pub const UI: EngineUi`). `build.rs` collects every file there, so
//! adding an engine's UI touches no shared list: the accent, tile knobs and labels, graph, hero,
//! model-dependent switches, locks, value texts, momentary switches and preset folder are all
//! read from here by `theme`, `key_knobs`, `module_graphs`, `module_drawer`, `chain_view` and
//! `fs_presets`.

use crate::module_drawer::Switch;
use crate::module_graphs::GraphKind;
use soundcraft_dsp::ParamInfo;
use soundcraft_model::Insert;

// The engine UI modules (`pub mod <engine>;`, one per file in `src/engines`) and `BY_FILE`.
include!(concat!(env!("OUT_DIR"), "/engine_uis.rs"));

/// One FireStation engine's UI hooks. Start from [`EngineUi::new`] and set what the engine has.
#[derive(Clone, Copy)]
pub struct EngineUi {
    /// The plugin id (`fs_<engine>`).
    pub id: &'static str,
    /// The engine's accent colour (ARGB), as Station gives it its tile
    /// (`Station/Source/Rig/ModuleRegistry.cpp`), else its editor's.
    pub accent: u32,
    /// The accent as the engine's current model wears it (Amp's follows the amp in use); `None`
    /// from the hook: [`EngineUi::accent`].
    pub model_accent: Option<fn(&Insert) -> Option<u32>>,
    /// The graph its tile and drawer draw.
    pub graph: GraphKind,
    /// The key knobs a tile shows until the user picks others (Station's own); empty: the first
    /// three non-toggle parameters.
    pub key_knobs: &'static [&'static str],
    /// Key knobs that follow the engine's model (Amp: each amp has its four).
    pub model_key_knobs: Option<fn(&Insert) -> Vec<&'static str>>,
    /// Tile labels the general shortening would get wrong: (param, label).
    pub tile_labels: &'static [(&'static str, &'static str)],
    /// A tile label for every other parameter, instead of the general shortening.
    pub tile_label: Option<fn(&ParamInfo) -> String>,
    /// The parameter shown as the drawer's large knob; `None`: the first key knob.
    pub hero: Option<&'static str>,
    /// Whether the hero does anything in the engine's current model; `None`: it always does.
    pub hero_applies: Option<fn(&Insert) -> bool>,
    /// The tab a drawer opens on (from the drawer's groups); `None` from the hook: the first.
    pub default_group: Option<fn(&Insert, &[&'static str]) -> Option<&'static str>>,
    /// How the drawer shows a parameter whose meaning depends on the engine's model
    /// (`module_drawer::model_switch`).
    pub model_switch: Option<fn(&Insert, &str) -> Option<Switch>>,
    /// A parameter the engine derives from others right now (shown, not movable).
    pub locked: Option<fn(&Insert, &str) -> bool>,
    /// A knob's value text where the C++ prints more than the value; `None` from the hook: the
    /// parameter's own text.
    pub value_text: Option<fn(&egui::Context, &Insert, &ParamInfo, f32) -> Option<String>>,
    /// Switches that are pressed, not toggled, with their tooltips: (param, tip).
    pub momentary: &'static [(&'static str, &'static str)],
    /// What a switch or knob does, for its tooltip, by parameter id (the drawer's generic cells
    /// say only the name otherwise): Shale's MIDI Out.
    pub tips: &'static [(&'static str, &'static str)],
    /// The parameter a tile names the engine's current model after (style, circuit, machine...).
    pub model_param: Option<&'static str>,
    /// The tile's model name, where it is not a parameter's text (Amp's short amp name).
    pub model_name: Option<fn(&Insert) -> Option<String>>,
    /// FireStation's user preset folder for the engine (under `%APPDATA%\FireStation`).
    pub user_presets: Option<&'static str>,
    /// The engine's VST-faithful wide editor (`crate::editors::<engine>::draw`), once translated;
    /// `None`: the narrow drawer.
    pub wide_editor: Option<crate::editors::WideEditor>,
}

impl EngineUi {
    /// An engine with an accent and a graph, and no other hook.
    pub const fn new(id: &'static str, accent: u32, graph: GraphKind) -> EngineUi {
        EngineUi {
            id,
            accent,
            model_accent: None,
            graph,
            key_knobs: &[],
            model_key_knobs: None,
            tile_labels: &[],
            tile_label: None,
            hero: None,
            hero_applies: None,
            default_group: None,
            model_switch: None,
            locked: None,
            value_text: None,
            momentary: &[],
            tips: &[],
            model_param: None,
            model_name: None,
            user_presets: None,
            wide_editor: None,
        }
    }
}

static ENGINE_UIS: [&EngineUi; BY_FILE.len()] = BY_FILE;

/// Every engine's UI hooks, by file name.
pub fn engine_uis() -> &'static [&'static EngineUi] {
    &ENGINE_UIS
}

/// The UI hooks of the engine with this plugin id.
pub fn engine_ui(plugin: &str) -> Option<&'static EngineUi> {
    ENGINE_UIS.iter().copied().find(|u| u.id == plugin)
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_firestation_engine_has_exactly_one_ui() {
        // (A built-in with an editor of its own, Rez, has hooks here too: not an engine.)
        let fs = |id: &&str| soundcraft_dsp::plugins::fs::engine(id).is_some();
        let mut ids: Vec<&str> = super::engine_uis().iter().map(|u| u.id).filter(fs).collect();
        ids.sort_unstable();
        for u in super::engine_uis() {
            assert!(soundcraft_dsp::plugin_info(u.id).is_some(), "{} has UI hooks but is not a plugin", u.id);
        }
        let mut engines: Vec<&str> = soundcraft_dsp::plugins::fs::engines().iter().map(|e| e.info.id).collect();
        engines.sort_unstable();
        assert_eq!(ids, engines);
    }
}

#[cfg(test)]
mod snapshot_tests {
    //! Every per-engine UI answer, digested per plugin and pinned (snapshot taken before the hooks
    //! moved into per-engine files): accents, graph, key knobs, hero, tile labels, model switches,
    //! locks, value texts, momentary switches and model names, for the default module and for
    //! every position of every choice parameter.
    use crate::module_drawer as md;
    use soundcraft_dsp::{PluginInfo, Unit};
    use soundcraft_model::Insert;
    use std::fmt::Write as _;

    fn states(info: &PluginInfo) -> Vec<Insert> {
        let mut out = vec![Insert::new(info.id)];
        for c in info.params.iter().filter(|p| p.unit == Unit::Choice) {
            for i in 0..c.choices.len() {
                let mut ins = Insert::new(info.id);
                ins.params.insert(c.id.to_string(), i as f32);
                out.push(ins);
            }
        }
        out
    }

    fn digest(ctx: &egui::Context, info: &'static PluginInfo) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "{} graph={:?} station={:?}", info.id, crate::module_graphs::graph_kind(info.id), crate::theme::station_accent(info.id));
        let _ = writeln!(s, "keys={:?}", crate::key_knobs::default_key_knobs(info));
        for p in info.params {
            let _ = writeln!(s, "tile {}={}", p.id, crate::key_knobs::tile_label(info.id, p));
        }
        for ins in states(info) {
            let groups = md::drawer_groups_for(info, &ins);
            let _ = writeln!(
                s,
                "state {:?} keys={:?} hero={:?} applies={} group={} model={:?} accent={:?}",
                ins.params,
                crate::key_knobs::key_knobs_for(&ins, info),
                md::hero_param_for(info, &ins).map(|p| p.id),
                md::hero_applies(&ins),
                md::default_group(&ins, &groups),
                crate::chain_view::model_name(&ins, info),
                crate::theme::module_accent_for(&ins),
            );
            for p in info.params {
                let _ = writeln!(
                    s,
                    "  {} switch={:?} dial={:?} knob={} locked={} text={} momentary={}",
                    p.id,
                    md::model_switch(&ins, p.id),
                    md::dial_name(&ins, p.id),
                    crate::key_knobs::tile_knob_label(&ins, p),
                    md::param_locked(&ins, p.id),
                    md::value_text(ctx, &ins, p, ins.params.get(p.id).copied().unwrap_or(p.default)),
                    if md::is_momentary(&ins, p.id) { md::momentary_tip(&ins, p.id) } else { "-" },
                );
            }
        }
        s
    }

    fn fnv(s: &str) -> u64 {
        s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
    }

    #[test]
    fn engine_ui_answers_are_unchanged() {
        let want = include_str!("engine_ui_snapshot.txt").trim().replace("\r\n", "\n");
        // Accents follow the palette, a process-wide flag another test may flip for a moment:
        // a mismatch is taken again before it counts.
        let mut got = snapshot();
        for _ in 0..3 {
            if got == want {
                break;
            }
            got = snapshot();
        }
        assert_eq!(got, want);
    }

    fn snapshot() -> String {
        let ctx = egui::Context::default();
        let mut got = String::new();
        let mut full = String::new();
        for info in soundcraft_dsp::plugins() {
            let d = digest(&ctx, info);
            let _ = writeln!(got, "{} {:016x}", info.id, fnv(&d));
            full.push_str(&d);
        }
        let folders: Vec<_> = {
            let mut v = crate::fs_presets::engine_folders();
            v.sort();
            v
        };
        let _ = writeln!(got, "folders {folders:?}");
        if let Some(dir) = std::env::var_os("ENGINE_UI_SNAPSHOT_DUMP") {
            let _ = std::fs::write(std::path::Path::new(&dir).join("engine_ui_full.txt"), &full);
            let _ = std::fs::write(std::path::Path::new(&dir).join("engine_ui_snapshot.txt"), &got);
        }
        got.trim().to_string()
    }
}
