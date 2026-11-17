//! Preset housekeeping for built-in plug-ins, shared by the wide editors, the narrow drawers and
//! the plug-in windows:
//!
//! - **One save path**: every user preset is written by [`write_user`] (the folder per plug-in,
//!   the file per name), whichever view saves it.
//! - **Rename** a user preset ([`rename_user`]): the file, its favourite star, and every module of
//!   this session showing the old name.
//! - **A default preset per effect** ([`default_for`], [`set_default`]): a preset (factory or
//!   user) chosen as the one a new insert of that plug-in starts from; [`apply_default`] loads it
//!   when the insert menu adds one. Kept in the preset folder's `.defaults.json`.
//! - **A/B** in every narrow drawer and plug-in window ([`ab_toggle`]): two sets of settings to
//!   flip between, as the wide editors have.
//! - **Copy and paste settings** between instances in every narrow drawer and plug-in window
//!   ([`copy_settings`], [`paste_settings`]): what a preset would hold, onto another instance of
//!   the same plug-in on any track. One undo step.

use crate::SoundApp;
use serde_json::{Map, Value, json};
use soundcraft_dsp::PluginInfo;
use soundcraft_model::{Insert, ModuleId, TrackId};
use std::path::PathBuf;

/// The folder of a plug-in's user presets (`None`: no preset folder, e.g. the web build).
pub fn user_dir(app: &SoundApp, plugin: &str) -> Option<PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join(crate::preset_folder_name(plugin)))
}

fn defaults_file(app: &SoundApp) -> Option<PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join(".defaults.json"))
}

/// Writes a user preset's file (`body`, as `editor_kit::presets::user_preset_body` makes it).
/// The one place a user preset is saved. Returns the name it was saved under.
pub fn write_user(app: &SoundApp, plugin: &str, name: &str, body: &Value) -> Result<String, String> {
    let dir = user_dir(app, plugin).ok_or("User presets need the desktop app")?;
    let clean = soundcraft_engine::io::sanitize_name(name.trim());
    let clean = if clean.is_empty() { "Preset".to_string() } else { clean };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{clean}.json")), body.to_string()).map_err(|e| e.to_string())?;
    Ok(clean)
}

/// The names of a plug-in's user presets, sorted.
pub fn user_names(app: &SoundApp, plugin: &str) -> Vec<String> {
    let Some(dir) = user_dir(app, plugin) else { return Vec::new() };
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
                .filter_map(|p| p.file_stem().and_then(|x| x.to_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// A user preset's parameters, as the module takes them: only the plug-in's own, never Bypass
/// or what its editor keeps out of presets, finite values only.
pub fn user_params(app: &SoundApp, info: &PluginInfo, name: &str) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(user_dir(app, info.id)?.join(format!("{name}.json"))).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let skip = crate::editors::not_in_presets(info.id);
    Some(
        v.get("params")?
            .as_object()?
            .iter()
            .filter(|(k, val)| {
                info.params.iter().any(|p| p.id == k.as_str())
                    && k.as_str() != "bypass"
                    && !skip.contains(&k.as_str())
                    && val.as_f64().is_some_and(f64::is_finite)
            })
            .map(|(k, val)| (k.clone(), val.clone()))
            .collect(),
    )
}

/// A factory preset's parameters by name.
fn factory_params(info: &PluginInfo, name: &str) -> Option<Map<String, Value>> {
    crate::fs_presets::factory_groups(info)
        .iter()
        .flat_map(|(_, list)| list.iter())
        .find(|(n, _)| *n == name)
        .and_then(|(n, json)| crate::fs_presets::factory_recall(info, n, json))
        .map(|(_, mut params)| {
            crate::fs_presets::fill_missing(info, &mut params);
            params
        })
}

/// Renames user preset `old` of `plugin` to `new`: the file, its favourite star, a default
/// that named it, and the modules of this session that show it. Refuses a factory preset's name
/// or one already used.
pub fn rename_user(app: &mut SoundApp, info: &PluginInfo, old: &str, new: &str) -> Result<String, String> {
    let dir = user_dir(app, info.id).ok_or("User presets need the desktop app")?;
    let clean = soundcraft_engine::io::sanitize_name(new.trim());
    if clean.is_empty() {
        return Err("Name it first".into());
    }
    if crate::editor_kit::presets::is_factory_name(info, &clean) {
        return Err(format!("{clean} is a factory preset: pick another name"));
    }
    let (from, to) = (dir.join(format!("{old}.json")), dir.join(format!("{clean}.json")));
    if !from.is_file() {
        return Err(format!("no user preset {old}"));
    }
    if clean != old && to.exists() {
        return Err(format!("{clean} is taken"));
    }
    // The name inside the file follows.
    let text = std::fs::read_to_string(&from).map_err(|e| e.to_string())?;
    let mut body: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(o) = body.as_object_mut() {
        o.insert("name".into(), json!(clean));
    }
    std::fs::write(&to, body.to_string()).map_err(|e| e.to_string())?;
    if clean != old {
        let _ = std::fs::remove_file(&from);
    }
    // Its favourite star.
    if let Some(f) = app.preset_dir.as_ref().map(|d| d.join(".favourites").join(format!("{}.json", crate::preset_folder_name(info.id))))
        && let Ok(t) = std::fs::read_to_string(&f)
        && let Ok(mut favs) = serde_json::from_str::<Vec<String>>(&t)
    {
        for x in favs.iter_mut().filter(|x| x.as_str() == old) {
            *x = clean.clone();
        }
        let _ = std::fs::write(&f, json!(favs).to_string());
    }
    if default_for(app, info.id).as_deref() == Some(old) {
        set_default(app, info.id, Some(&clean))?;
    }
    // The modules showing it (a name change only: their settings stay).
    let shown: Vec<(TrackId, ModuleId)> = app
        .engine
        .session()
        .tracks
        .iter()
        .flat_map(|t| t.mixer.inserts.iter().filter(|i| i.plugin == info.id && i.preset == old).map(move |i| (t.id, i.id)))
        .collect();
    for (t, m) in shown {
        let _ = app.run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {}, "preset": clean}));
    }
    Ok(clean)
}

fn read_defaults(app: &SoundApp) -> Map<String, Value> {
    defaults_file(app)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// The preset a new insert of `plugin` starts from, if one is chosen.
pub fn default_for(app: &SoundApp, plugin: &str) -> Option<String> {
    read_defaults(app).get(plugin).and_then(Value::as_str).map(str::to_string)
}

/// Chooses (or with `None` clears) the preset new inserts of `plugin` start from.
pub fn set_default(app: &SoundApp, plugin: &str, name: Option<&str>) -> Result<(), String> {
    let f = defaults_file(app).ok_or("User presets need the desktop app")?;
    let mut all = read_defaults(app);
    match name {
        Some(n) => {
            all.insert(plugin.to_string(), json!(n));
        }
        None => {
            all.remove(plugin);
        }
    }
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&f, Value::Object(all).to_string()).map_err(|e| e.to_string())
}

/// Loads the default preset of a module's plug-in into it, if one is chosen and still there
/// (a user preset first, then a factory one of that name). Returns its name.
pub fn apply_default(app: &mut SoundApp, track: TrackId, module: ModuleId) -> Option<String> {
    let plugin = app.engine.session().track(track)?.module(module)?.plugin.clone();
    let info = soundcraft_dsp::plugin_info(&plugin)?;
    let name = default_for(app, &plugin)?;
    let params = user_params(app, info, &name).or_else(|| factory_params(info, &name))?;
    app.run("mix.module_params", json!({"track": track.0, "module": module.0, "params": params, "preset": name})).ok()?;
    Some(name)
}

/// The other side of a module's A/B compare.
#[derive(Clone, Default)]
struct Ab {
    on_b: bool,
    other: Option<Map<String, Value>>,
}

fn ab_key(track: TrackId, module: ModuleId) -> egui::Id {
    egui::Id::new(("preset-ab", track.0, module.0))
}

/// Whether the module shows B of its A/B compare.
pub fn on_b(ctx: &egui::Context, track: TrackId, module: ModuleId) -> bool {
    ctx.data(|d| d.get_temp::<Ab>(ab_key(track, module))).is_some_and(|a| a.on_b)
}

/// Flips a module between A and B: what it has now is kept as the side it leaves, the other
/// side's settings come back (the first time B is a copy of A). One undo step.
pub fn ab_toggle(app: &mut SoundApp, ctx: &egui::Context, track: TrackId, ins: &Insert, info: &PluginInfo) {
    let key = ab_key(track, ins.id);
    let mut ab = ctx.data(|d| d.get_temp::<Ab>(key)).unwrap_or_default();
    let now: Map<String, Value> = info
        .params
        .iter()
        .filter(|p| p.id != "bypass")
        .map(|p| (p.id.to_string(), json!(ins.params.get(p.id).copied().unwrap_or(p.default))))
        .collect();
    if let Some(other) = ab.other.take() {
        let _ = app.run("mix.module_params", json!({"track": track.0, "module": ins.id.0, "params": other}));
    }
    ab.other = Some(now);
    ab.on_b = !ab.on_b;
    ctx.data_mut(|d| d.insert_temp(key, ab));
}

/// The settings last copied from a module: its plug-in and the values a preset would hold.
#[derive(Clone)]
struct Copied {
    plugin: String,
    params: Map<String, Value>,
}

fn copied_key() -> egui::Id {
    egui::Id::new("preset-copied-settings")
}

/// Copies a module's settings (every parameter but Bypass and those its editor keeps out of
/// presets: a reference, a link group, a capture) for [`paste_settings`].
pub fn copy_settings(ctx: &egui::Context, ins: &Insert, info: &PluginInfo) {
    let skip = crate::editors::not_in_presets(info.id);
    let params: Map<String, Value> = info
        .params
        .iter()
        .filter(|p| p.id != "bypass" && !skip.contains(&p.id))
        .map(|p| (p.id.to_string(), json!(ins.params.get(p.id).copied().unwrap_or(p.default))))
        .collect();
    ctx.data_mut(|d| d.insert_temp(copied_key(), Copied { plugin: info.id.to_string(), params }));
}

/// The name of the plug-in whose settings are copied, when they would paste onto `plugin`.
pub fn can_paste(ctx: &egui::Context, plugin: &str) -> bool {
    ctx.data(|d| d.get_temp::<Copied>(copied_key())).is_some_and(|c| c.plugin == plugin)
}

/// Pastes the copied settings onto module `module` of `track` (the same plug-in only). One undo
/// step; the module's preset name is left as it is.
pub fn paste_settings(app: &mut SoundApp, ctx: &egui::Context, track: TrackId, module: ModuleId) -> Result<(), String> {
    let copied = ctx.data(|d| d.get_temp::<Copied>(copied_key())).ok_or("Nothing copied")?;
    let plugin = app.engine.session().track(track).and_then(|t| t.module(module)).map(|i| i.plugin.clone());
    if plugin.as_deref() != Some(copied.plugin.as_str()) {
        return Err("The copied settings are for another plug-in".into());
    }
    app.run("mix.module_params", json!({"track": track.0, "module": module.0, "params": copied.params})).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_engine::Engine;

    fn app_with_presets() -> (SoundApp, PathBuf) {
        let dir = std::env::temp_dir().join(format!("freedawm-preset-house-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut engine = Engine::new(soundcraft_model::Session::default());
        engine.execute("track.new", &json!({"name": "Bass"})).unwrap();
        let mut app = SoundApp::new(engine, None, crate::Services::default());
        app.preset_dir = Some(dir.clone());
        (app, dir)
    }

    #[test]
    fn save_rename_and_default_presets() {
        let (mut app, dir) = app_with_presets();
        let info = soundcraft_dsp::plugin_info("keel").unwrap();
        let t = app.engine.session().tracks[0].id;
        // one save path
        let body = crate::editor_kit::presets::user_preset_body(
            info,
            &[],
            |id| if id == "drive" { 8.0 } else { info.param(id).map_or(0.0, |p| p.default) },
            "Mine",
            "",
            None,
        );
        assert_eq!(write_user(&app, "keel", "Mine", &body).unwrap(), "Mine");
        assert_eq!(user_names(&app, "keel"), ["Mine"]);
        assert_eq!(user_params(&app, info, "Mine").unwrap()["drive"], json!(8.0));
        // a module showing it follows a rename; a factory name is refused
        app.run("mix.module_add", json!({"track": t.0, "plugin": "keel"})).unwrap();
        let m = app.engine.session().tracks[0].mixer.inserts[0].id;
        app.run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"drive": 8.0}, "preset": "Mine"})).unwrap();
        set_default(&app, "keel", Some("Mine")).unwrap();
        assert!(rename_user(&mut app, info, "Mine", "Warm Valve").is_err(), "a factory name");
        assert_eq!(rename_user(&mut app, info, "Mine", "Growl").unwrap(), "Growl");
        assert_eq!(user_names(&app, "keel"), ["Growl"]);
        assert_eq!(app.engine.session().tracks[0].mixer.inserts[0].preset, "Growl");
        assert_eq!(default_for(&app, "keel").as_deref(), Some("Growl"), "the default follows");
        // a new insert starts from the default
        app.run("mix.module_add", json!({"track": t.0, "plugin": "keel"})).unwrap();
        let m2 = app.engine.session().tracks[0].mixer.inserts[1].id;
        assert_eq!(apply_default(&mut app, t, m2).as_deref(), Some("Growl"));
        let ins = &app.engine.session().tracks[0].mixer.inserts[1];
        assert_eq!((ins.preset.as_str(), ins.params.get("drive").copied()), ("Growl", Some(8.0)));
        // a factory preset as the default
        set_default(&app, "keel", Some("Fuzz Wall")).unwrap();
        app.run("mix.module_add", json!({"track": t.0, "plugin": "keel"})).unwrap();
        let m3 = app.engine.session().tracks[0].mixer.inserts[2].id;
        assert_eq!(apply_default(&mut app, t, m3).as_deref(), Some("Fuzz Wall"));
        assert_eq!(app.engine.session().tracks[0].mixer.inserts[2].params.get("drive").copied(), Some(9.0));
        set_default(&app, "keel", None).unwrap();
        assert_eq!(default_for(&app, "keel"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ab_flips_between_two_settings() {
        let (mut app, dir) = app_with_presets();
        let ctx = egui::Context::default();
        let info = soundcraft_dsp::plugin_info("heft").unwrap();
        let t = app.engine.session().tracks[0].id;
        app.run("mix.module_add", json!({"track": t.0, "plugin": "heft"})).unwrap();
        fn app_ins(app: &SoundApp) -> Insert {
            app.engine.session().tracks[0].mixer.inserts[0].clone()
        }
        let m = app_ins(&app).id;
        app.run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"harmonics": 10.0}})).unwrap();
        // to B (a copy of A), changed there
        let i = app_ins(&app);
        ab_toggle(&mut app, &ctx, t, &i, info);
        assert!(on_b(&ctx, t, m));
        app.run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"harmonics": 90.0}})).unwrap();
        // back to A, then B again
        let i = app_ins(&app);
        ab_toggle(&mut app, &ctx, t, &i, info);
        assert_eq!(app_ins(&app).params.get("harmonics").copied(), Some(10.0));
        let i = app_ins(&app);
        ab_toggle(&mut app, &ctx, t, &i, info);
        assert_eq!(app_ins(&app).params.get("harmonics").copied(), Some(90.0));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The parity list's "copy settings to another instance" from a narrow drawer or plug-in
    /// window: Copy Settings on one Heft, Paste Settings on another (on another track); a
    /// different plug-in refuses them.
    #[test]
    fn settings_copy_to_another_instance() {
        let (mut app, dir) = app_with_presets();
        let ctx = egui::Context::default();
        let info = soundcraft_dsp::plugin_info("heft").unwrap();
        app.run("track.new", json!({"name": "Kick"})).unwrap();
        let (a, b) = (app.engine.session().tracks[0].id, app.engine.session().tracks[1].id);
        app.run("mix.module_add", json!({"track": a.0, "plugin": "heft"})).unwrap();
        app.run("mix.module_add", json!({"track": b.0, "plugin": "heft"})).unwrap();
        app.run("mix.module_add", json!({"track": b.0, "plugin": "keel"})).unwrap();
        let ins = |app: &SoundApp, t: usize, i: usize| app.engine.session().tracks[t].mixer.inserts[i].clone();
        let (ma, mb, mk) = (ins(&app, 0, 0).id, ins(&app, 1, 0).id, ins(&app, 1, 1).id);
        app.run("mix.module_params", json!({"track": a.0, "module": ma.0, "params": {"harmonics": 70.0}})).unwrap();
        assert!(!can_paste(&ctx, "heft"));
        copy_settings(&ctx, &ins(&app, 0, 0), info);
        assert!(can_paste(&ctx, "heft") && !can_paste(&ctx, "keel"));
        paste_settings(&mut app, &ctx, b, mb).unwrap();
        assert_eq!(ins(&app, 1, 0).params.get("harmonics").copied(), Some(70.0));
        assert!(paste_settings(&mut app, &ctx, b, mk).is_err());
        app.run("edit.undo", json!({})).unwrap();
        assert_ne!(ins(&app, 1, 0).params.get("harmonics").copied(), Some(70.0), "one undo step");
        let _ = std::fs::remove_dir_all(dir);
    }
}
