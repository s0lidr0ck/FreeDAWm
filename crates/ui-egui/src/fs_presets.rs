//! FireStation presets in SoundCraft: the engines' bundled factory presets (recalled from the
//! preset menu) and the import of the owner's FireStation user presets
//! (`presets.import_firestation`, File › Import › FireStation User Presets).

use crate::SoundApp;
use serde_json::{Map, Value, json};
use soundcraft_dsp::PluginInfo;
use soundcraft_dsp::plugins::fs::{FsPreset, factory_presets};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// FireStation's per-engine folder (under `%APPDATA%\FireStation`) and the SoundCraft plugin it
/// is, for every engine with one (`EngineUi::user_presets`).
pub fn engine_folders() -> Vec<(&'static str, &'static str)> {
    crate::engine_ui::engine_uis().iter().filter_map(|u| u.user_presets.map(|f| (f, u.id))).collect()
}

/// A preset's values as `mix.module_params` takes them. As FireStation loads a preset, only the
/// values it lists are set (`loadPreset` sets each of `preset.values`).
fn params_of(p: &FsPreset) -> Map<String, Value> {
    p.values.iter().map(|(k, v)| (k.clone(), json!(v))).collect()
}

/// A factory preset of `info`'s plugin: its name and the parameters to set. `None` if the JSON
/// is not a preset.
pub fn factory_recall(info: &PluginInfo, list_name: &str, json: &str) -> Option<(String, Map<String, Value>)> {
    let p = FsPreset::parse(json, info)?;
    let name = if p.name.trim().is_empty() { list_name.to_string() } else { p.name.clone() };
    Some((name, params_of(&p)))
}

/// A plugin's factory presets in one group: `(group, [(name, json)])`.
pub type FactoryGroup = (String, Vec<(&'static str, &'static str)>);

/// The factory presets of a plugin by group, in the order the groups first appear. A preset
/// without a group goes under "Other". Parsed once per process for every built-in (the preset
/// menu asks every frame while it is open); empty for a plugin without factory presets.
pub fn factory_groups(info: &PluginInfo) -> &'static [FactoryGroup] {
    static GROUPS: OnceLock<HashMap<&'static str, Vec<FactoryGroup>>> = OnceLock::new();
    let all = GROUPS
        .get_or_init(|| soundcraft_dsp::plugins().iter().filter(|p| !factory_presets(p.id).is_empty()).map(|p| (p.id, group_presets(p))).collect());
    all.get(info.id).map_or(&[], Vec::as_slice)
}

fn group_presets(info: &PluginInfo) -> Vec<FactoryGroup> {
    let mut out: Vec<FactoryGroup> = Vec::new();
    for (name, json) in factory_presets(info.id) {
        let group = FsPreset::parse(json, info).map(|p| p.group).filter(|g| !g.trim().is_empty()).unwrap_or_else(|| "Other".to_string());
        match out.iter_mut().find(|(g, _)| *g == group) {
            Some((_, list)) => list.push((name, json)),
            None => out.push((group, vec![(name, json)])),
        }
    }
    out
}

/// The factory section of a module's preset menu: a "Factory" header and a submenu per group.
pub(crate) fn factory_menu(
    app: &mut SoundApp,
    ui: &mut egui::Ui,
    track: soundcraft_model::TrackId,
    module: soundcraft_model::ModuleId,
    info: &PluginInfo,
) {
    let groups = factory_groups(info);
    if groups.is_empty() {
        return;
    }
    ui.separator();
    ui.label(egui::RichText::new("Factory").small().strong());
    for (group, list) in groups {
        ui.menu_button(group, |ui| {
            egui::ScrollArea::vertical().max_height((ui.ctx().content_rect().height() - 80.0).max(160.0)).show(ui, |ui| {
                for (name, json) in list {
                    if ui.button(*name).clicked() {
                        if let Some((preset, params)) = factory_recall(info, name, json) {
                            let r = app.run("mix.module_params", json!({"track": track.0, "module": module.0, "params": params, "preset": preset}));
                            if let Err(e) = r {
                                app.ui.status = e;
                            }
                        }
                        ui.close();
                    }
                }
            });
        });
    }
    ui.separator();
    ui.label(egui::RichText::new("User").small().strong());
}

/// What an import did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImportCounts {
    pub imported: usize,
    /// A preset of that name already exists (it is left as it is).
    pub skipped: usize,
    /// Unreadable, not a preset, or nothing in it this engine knows.
    pub failed: usize,
}

/// Imports the FireStation user presets under `appdata` (`<appdata>\FireStation\<Engine>\Presets\
/// User\*.json`) of one engine (its folder name or plugin id, any case) or of every ported one,
/// writing each as a SoundCraft preset of its plugin under `preset_root`. Existing names are
/// skipped, never overwritten.
pub fn import_firestation(appdata: &Path, preset_root: &Path, engine: Option<&str>) -> Result<ImportCounts, String> {
    let engines: Vec<(&str, &str)> = match engine.map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) => {
            let one = engine_folders().into_iter().find(|(folder, id)| folder.eq_ignore_ascii_case(e) || id.eq_ignore_ascii_case(e));
            vec![one.ok_or_else(|| format!("unknown FireStation engine `{e}`"))?]
        }
        None => engine_folders(),
    };
    let mut n = ImportCounts::default();
    for (folder, plugin) in engines {
        // An engine not ported yet has nothing to import into.
        let Some(info) = soundcraft_dsp::plugin_info(plugin) else { continue };
        let dir = appdata.join("FireStation").join(folder).join("Presets").join("User");
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<std::path::PathBuf> =
            rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"))).collect();
        files.sort();
        let out_dir = preset_root.join(crate::preset_folder_name(plugin));
        for f in files {
            let Some(p) = std::fs::read_to_string(&f).ok().and_then(|text| FsPreset::parse(&text, info)).filter(|p| !p.values.is_empty()) else {
                n.failed += 1;
                continue;
            };
            let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("preset");
            let name = soundcraft_engine::io::sanitize_name(if p.name.trim().is_empty() { stem } else { &p.name });
            let target = out_dir.join(format!("{name}.json"));
            if target.exists() {
                n.skipped += 1;
                continue;
            }
            let body = json!({"format": "soundcraft-plugin-preset", "plugin": plugin, "params": params_of(&p)});
            match std::fs::create_dir_all(&out_dir).and_then(|()| std::fs::write(&target, body.to_string())) {
                Ok(()) => n.imported += 1,
                Err(_) => n.failed += 1,
            }
        }
    }
    Ok(n)
}

/// `presets.import_firestation {engine?}`: imports from `%APPDATA%` into the app's preset folder
/// and reports `{imported, skipped, failed}` (also in the status line).
pub(crate) fn command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let root = app.preset_dir.clone().ok_or("user presets need the desktop app")?;
    let appdata = std::env::var_os("APPDATA").map(std::path::PathBuf::from).ok_or("APPDATA is not set")?;
    let n = import_firestation(&appdata, &root, p.get("engine").and_then(Value::as_str))?;
    app.ui.status = format!("FireStation presets: {} imported, {} skipped (already there), {} failed", n.imported, n.skipped, n.failed);
    serde_json::to_value(n).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_factory_preset_recalls_its_values() {
        let info = soundcraft_dsp::plugin_info("fs_sat").unwrap();
        let (name, json) = factory_presets("fs_sat").iter().find(|(n, _)| *n == "Acoustic Body").unwrap();
        let (preset, params) = factory_recall(info, name, json).unwrap();
        assert_eq!(preset, "Acoustic Body");
        assert_eq!(params["circuit"], json!(0.0));
        assert_eq!(params["variant"], json!(2.0));
        assert!((params["drive"].as_f64().unwrap() - 0.466_667).abs() < 1e-5);

        // Through the command the menu runs: the module takes the values and the preset's name.
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.run("track.new", json!({"name": "Gtr"})).unwrap();
        let track = app.engine.session().track_by_name("Gtr").unwrap().id;
        let r = app.run("mix.module_add", json!({"track": track.0, "plugin": "fs_sat"})).unwrap();
        let module = r["module"].as_u64().unwrap();
        app.run("mix.module_params", json!({"track": track.0, "module": module, "params": params, "preset": preset})).unwrap();
        let ins = app.engine.session().track(track).unwrap().mixer.inserts[0].clone();
        assert_eq!(ins.preset, "Acoustic Body");
        assert_eq!(ins.params["circuit"], 0.0);
        assert_eq!(ins.params["variant"], 2.0);
        assert!((ins.params["drive"] - 0.466_667).abs() < 1e-5);
    }

    #[test]
    fn factory_presets_come_grouped() {
        let info = soundcraft_dsp::plugin_info("fs_sat").unwrap();
        let groups = factory_groups(info);
        assert!(groups.iter().any(|(g, list)| g == "Guitars" && list.iter().any(|(n, _)| *n == "Acoustic Body")));
        let total: usize = groups.iter().map(|(_, l)| l.len()).sum();
        assert_eq!(total, factory_presets("fs_sat").len());
        assert!(factory_groups(soundcraft_dsp::plugin_info("eq_7band").unwrap()).is_empty());
    }

    #[test]
    fn factory_groups_are_parsed_once() {
        // The menu asks every frame while open: the same parsed groups every time.
        for p in soundcraft_dsp::plugins().iter().filter(|p| !factory_presets(p.id).is_empty()) {
            let (a, b) = (factory_groups(p), factory_groups(p));
            assert!(std::ptr::eq(a, b), "{}", p.id);
            assert_eq!(a, group_presets(p).as_slice(), "{}", p.id);
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sc-fs-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn user_presets_import_skipping_names_that_exist() {
        let appdata = temp_dir("appdata");
        let presets = temp_dir("presets");
        let user = appdata.join("FireStation").join("SatEngine").join("Presets").join("User");
        std::fs::create_dir_all(&user).unwrap();
        let preset = |name: &str, drive: f32| {
            format!(r#"{{"engine":"SatEngine","name":"{name}","group":"Mine","values":{{"circuit":"Valve","drive":{drive}}}}}"#)
        };
        std::fs::write(user.join("a.json"), preset("Warm Vox", 0.3)).unwrap();
        std::fs::write(user.join("b.json"), preset("Hot Drums", 0.8)).unwrap();
        std::fs::write(user.join("c.json"), preset("Warm Vox", 0.9)).unwrap();
        std::fs::write(user.join("d.json"), "not a preset at all").unwrap();
        std::fs::write(user.join("notes.txt"), "ignored").unwrap();

        let n = import_firestation(&appdata, &presets, None).unwrap();
        assert_eq!((n.imported, n.skipped, n.failed), (2, 1, 1));
        let written = presets.join("fs_sat").join("Warm Vox.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
        assert_eq!(v["format"], "soundcraft-plugin-preset");
        assert_eq!(v["plugin"], "fs_sat");
        assert_eq!(v["params"]["circuit"], json!(1.0));
        assert!((v["params"]["drive"].as_f64().unwrap() - 0.3).abs() < 1e-6, "the first one is kept, not overwritten");

        // Again: everything is there already. One engine by name; an unknown one is an error.
        let again = import_firestation(&appdata, &presets, Some("fs_sat")).unwrap();
        assert_eq!((again.imported, again.skipped, again.failed), (0, 3, 1));
        assert_eq!(import_firestation(&appdata, &presets, Some("tape")).map(|n| n.imported), Err("unknown FireStation engine `tape`".into()));
        assert_eq!(import_firestation(&appdata, &presets, Some("TapeEngine")), Ok(ImportCounts::default()));
        let _ = std::fs::remove_dir_all(&appdata);
        let _ = std::fs::remove_dir_all(&presets);
    }

    #[test]
    fn the_import_command_needs_a_preset_folder() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        assert!(app.run("presets.import_firestation", json!({})).is_err());
    }
}
