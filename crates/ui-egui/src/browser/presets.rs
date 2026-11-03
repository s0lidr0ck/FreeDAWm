//! The browser's Presets tab: the presets of the selected plugin (the selected card, else the
//! plugin of the module open in a drawer or plugin window), and loading one.
//!
//! - Built-in and FireStation plugins: their factory presets by group (`fs_presets`) and the user
//!   presets in the DAW's preset folder, the lists the editors' preset browsers show.
//! - VST3: the `.vstpreset` files in the standard VST3 preset folders, and the plugin's built-in
//!   programs (its `IUnitInfo` program list) once the plugin has been created in this process.
//! - CLAP: what the binary's preset-discovery provider reports, once the binary is loaded.
//!
//! Listing never loads a plugin (only already-created plugins are asked). Loading a preset is one
//! undo step: built-in presets and VST3 programs set parameters (`mix.module_params`, or
//! `mix.module_preset {params}` for an instrument); hosted preset files become the module's state
//! (`mix.module_preset {state}`), from which the running instance is made again. Placing a plugin
//! with a preset joins the preset to the placement's step ([`soundcraft_engine::Engine::execute_joined`]).

use super::catalog::{BrowserItem, Format};
use crate::SoundApp;
use egui::vec2;
use serde_json::{Map, Value, json};
use soundcraft_mix::SlotKey;
use soundcraft_model::TrackId;
use std::path::{Path, PathBuf};

/// One preset of the selected plugin.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PresetRef {
    /// A built-in plugin's factory preset: its group (`list`) and name.
    Builtin { list: String, name: String },
    /// A user preset file (the DAW's preset folder).
    User { path: PathBuf },
    /// A VST3 `.vstpreset` file.
    Vst3File { path: PathBuf },
    /// A VST3 plugin's built-in program.
    Vst3Program { index: u32 },
    /// A CLAP discovered preset: its file (empty for one inside the plugin) and key.
    Clap { location: String, load_key: Option<String> },
}

/// What a preset sets, worked out before anything is placed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Load {
    Params {
        name: String,
        params: Map<String, Value>,
    },
    State {
        name: String,
        state: Vec<u8>,
    },
    /// A CLAP preset, loaded into the running plugin (whose state the module then stores).
    Clap {
        name: String,
        preset: soundcraft_clap_host::ClapPreset,
    },
}

/// A plugin's presets as (group, name, preset), and the note to show with them.
type Listing = (Vec<(String, String, PresetRef)>, Option<String>);

/// The presets listed for one plugin, kept until the plugin or what can be known about it
/// changes (the panel draws every frame). A hosted plugin's are listed on a worker thread (folder
/// walks, CLAP preset discovery); `pending` holds the worker's answer until it comes.
#[derive(Default)]
pub(crate) struct PresetCache {
    plugin: String,
    /// What [`knowable`] said when the list was made.
    known: bool,
    /// A CLAP plugin's binary (looked up once per plugin, not every frame).
    clap_path: Option<PathBuf>,
    pub list: Vec<(String, String, PresetRef)>,
    pub note: Option<String>,
    pending: Option<std::sync::Arc<std::sync::Mutex<Option<Listing>>>>,
}

/// The note while a worker lists a hosted plugin's presets.
const LISTING: &str = "Listing presets\u{2026}";

/// The note shown when a hosted plugin's own presets cannot be listed without creating it.
pub const INSERT_ONCE: &str = "Insert the plugin once to list its built-in programs";

fn file_name(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn vst3_desc(id: &str) -> Option<soundcraft_vst3_host::Vst3Descriptor> {
    soundcraft_vst3_host::cached().unwrap_or_default().into_iter().find(|d| d.id == id)
}

fn clap_desc(id: &str) -> Option<soundcraft_clap_host::ClapDescriptor> {
    soundcraft_clap_host::cached().unwrap_or_default().into_iter().find(|d| d.id == id)
}

/// The user presets of a built-in plugin, by name (case-insensitive).
fn user_presets(preset_dir: Option<&Path>, plugin: &str) -> Vec<(String, String, PresetRef)> {
    let Some(dir) = preset_dir.map(|d| d.join(crate::preset_folder_name(plugin))) else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<(String, String, PresetRef)> = rd
        .flatten()
        .take(2000)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .map(|p| ("User".to_string(), file_name(&p), PresetRef::User { path: p }))
        .collect();
    out.sort_by_cached_key(|(_, n, _)| n.to_lowercase());
    out
}

/// Whether more could be listed for `item` once its plugin is created (VST3 programs, CLAP
/// discovery), and so the cache must be checked again. Cheap: asked every frame.
fn knowable(item: &BrowserItem, clap_path: Option<&Path>) -> bool {
    match item.format {
        Format::Vst3 => soundcraft_vst3_host::programs_known(&item.id),
        Format::Clap => clap_path.is_some_and(soundcraft_clap_host::binary_loaded),
        _ => true,
    }
}

/// The presets of `item` as (group, name, preset), in list order.
pub fn presets_for(app: &SoundApp, item: &BrowserItem) -> Vec<(String, String, PresetRef)> {
    listing(app.preset_dir.as_deref(), item).0
}

/// The presets and the note to show with them. Reads folders (and asks a loaded CLAP binary):
/// a hosted plugin's list is made off the UI thread.
fn listing(preset_dir: Option<&Path>, item: &BrowserItem) -> Listing {
    let mut out = Vec::new();
    match item.format {
        Format::BuiltIn | Format::FireStation => {
            if let Some(info) = soundcraft_dsp::plugin_info(&item.id) {
                for (group, list) in crate::fs_presets::factory_groups(info) {
                    for (name, _) in list {
                        out.push((group.clone(), (*name).to_string(), PresetRef::Builtin { list: group.clone(), name: (*name).to_string() }));
                    }
                }
            }
            out.extend(user_presets(preset_dir, &item.id));
            (out, None)
        }
        Format::Vst3 => {
            if let Some(d) = vst3_desc(&item.id) {
                let base: Vec<PathBuf> = soundcraft_vst3_host::vstpreset::preset_roots().iter().map(|r| r.join(&d.vendor).join(&d.name)).collect();
                for path in soundcraft_vst3_host::vstpreset::preset_files(&d) {
                    let sub = base.iter().find_map(|b| path.parent()?.strip_prefix(b).ok()).map(|s| s.to_string_lossy().replace('\\', " / "));
                    let group = sub.filter(|s| !s.is_empty()).unwrap_or_else(|| "Presets".into());
                    out.push((group, file_name(&path), PresetRef::Vst3File { path }));
                }
            }
            let note = match soundcraft_vst3_host::programs(&item.id) {
                Some(p) => {
                    for (i, name) in p.names.iter().enumerate() {
                        out.push(("Programs".into(), name.clone(), PresetRef::Vst3Program { index: u32::try_from(i).unwrap_or(u32::MAX) }));
                    }
                    None
                }
                None => Some(INSERT_ONCE.to_string()),
            };
            (out, note)
        }
        Format::Clap => {
            let Some(d) = clap_desc(&item.id) else { return (out, None) };
            if !soundcraft_clap_host::bundle_loaded(&d) {
                return (out, Some("Insert the plugin once to list its presets".into()));
            }
            for p in soundcraft_clap_host::presets(&d) {
                let group = p.location.as_deref().and_then(|l| Path::new(l).parent()?.file_name()).map(|f| f.to_string_lossy().into_owned());
                let preset = PresetRef::Clap { location: p.location.unwrap_or_default(), load_key: p.load_key };
                out.push((group.unwrap_or_else(|| "Presets".into()), p.name, preset));
            }
            let note = out.is_empty().then(|| "This plugin offers no presets to the host".to_string());
            (out, note)
        }
        Format::Au => (out, None),
    }
}

/// The selected plugin's presets, from the cache (rebuilt when the plugin changes, when more
/// becomes known about it, or when `refresh`), taking a worker's list once it is ready.
pub(crate) fn cached(app: &mut SoundApp, item: &BrowserItem, refresh: bool) {
    let same = app.browser_presets.as_ref().filter(|c| c.plugin == item.id);
    let clap_path = match same {
        Some(c) => c.clap_path.clone(),
        None if item.format == Format::Clap => clap_desc(&item.id).map(|d| d.path),
        None => None,
    };
    let known = knowable(item, clap_path.as_deref());
    if refresh || same.is_none_or(|c| c.known != known) {
        let mut cache = PresetCache { plugin: item.id.clone(), known, clap_path, ..Default::default() };
        let dir = app.preset_dir.clone();
        match item.format {
            Format::Vst3 | Format::Clap => match spawn_listing(dir, item.clone()) {
                Some(slot) => {
                    cache.note = Some(LISTING.into());
                    cache.pending = Some(slot);
                }
                None => (cache.list, cache.note) = listing(app.preset_dir.as_deref(), item),
            },
            _ => (cache.list, cache.note) = listing(dir.as_deref(), item),
        }
        app.browser_presets = Some(cache);
    }
    if let Some(c) = app.browser_presets.as_mut()
        && let Some(slot) = &c.pending
        && let Some((list, note)) = slot.lock().ok().and_then(|mut s| s.take())
    {
        (c.list, c.note) = (list, note);
        c.pending = None;
    }
}

/// Lists `item`'s presets on a worker thread; `None` when no thread can be started (the web).
fn spawn_listing(dir: Option<PathBuf>, item: BrowserItem) -> Option<std::sync::Arc<std::sync::Mutex<Option<Listing>>>> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let out = std::sync::Arc::clone(&slot);
    let started = std::thread::Builder::new().name("preset-listing".into()).spawn(move || {
        let l = listing(dir.as_deref(), &item);
        if let Ok(mut s) = out.lock() {
            *s = Some(l);
        }
    });
    started.ok().map(|_| slot)
}

/// The preset's name: as listed, else from its file or key.
pub(crate) fn label(app: &SoundApp, plugin: &str, preset: &PresetRef) -> String {
    if let Some((_, n, _)) = app.browser_presets.as_ref().filter(|c| c.plugin == plugin).and_then(|c| c.list.iter().find(|(_, _, p)| p == preset)) {
        return n.clone();
    }
    match preset {
        PresetRef::Builtin { name, .. } => name.clone(),
        PresetRef::User { path } | PresetRef::Vst3File { path } => file_name(path),
        PresetRef::Vst3Program { index } => soundcraft_vst3_host::programs(plugin)
            .and_then(|p| p.names.get(*index as usize).cloned())
            .unwrap_or_else(|| format!("Program {}", index.saturating_add(1))),
        PresetRef::Clap { location, load_key } => load_key.clone().filter(|k| !k.is_empty()).unwrap_or_else(|| file_name(Path::new(location))),
    }
}

/// A `.vstpreset` file for the plugin of `class_id`: its name and the state it sets.
pub fn vstpreset_state(path: &Path, class_id: &str) -> Result<(String, Vec<u8>), String> {
    let p = soundcraft_vst3_host::vstpreset::load_vstpreset_file(path, class_id)?;
    let state = p.state_blob().ok_or_else(|| "the preset is too large".to_string())?;
    Ok((file_name(path), state))
}

/// A built-in plugin's preset parameters from a preset file's JSON: the DAW's own format
/// (`params`), else FireStation's; only the plugin's parameters with finite values.
fn params_of_json(info: &soundcraft_dsp::PluginInfo, name: &str, text: &str) -> Option<Map<String, Value>> {
    let v: Value = serde_json::from_str(text).ok()?;
    if let Some(obj) = v.get("params").and_then(Value::as_object) {
        let params = obj
            .iter()
            .filter(|(k, val)| info.params.iter().any(|pi| pi.id == k.as_str()) && val.as_f64().is_some_and(f64::is_finite))
            .map(|(k, val)| (k.clone(), val.clone()))
            .collect();
        return Some(params);
    }
    crate::fs_presets::factory_recall(info, name, text).map(|(_, p)| p)
}

/// Works out what `preset` of `plugin` sets. Hosted presets are read here (a CLAP one by loading
/// it into a fresh instance of the plugin), so nothing is placed for a preset that cannot load.
pub(crate) fn resolve(app: &SoundApp, plugin: &str, preset: &PresetRef) -> Result<Load, String> {
    let builtin = || soundcraft_dsp::plugin_info(plugin).ok_or_else(|| format!("unknown plugin `{plugin}`"));
    match preset {
        PresetRef::Builtin { list, name } => {
            let info = builtin()?;
            let json = crate::fs_presets::factory_groups(info)
                .iter()
                .filter(|(g, _)| g == list)
                .flat_map(|(_, l)| l.iter())
                .find(|(n, _)| n == name)
                .map(|(_, j)| *j)
                .ok_or_else(|| format!("{}: no preset {name}", info.name))?;
            let (name, params) = crate::fs_presets::factory_recall(info, name, json).ok_or_else(|| format!("{name} is not a preset"))?;
            Ok(Load::Params { name, params })
        }
        PresetRef::User { path } => {
            let info = builtin()?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let name = file_name(path);
            let params = params_of_json(info, &name, &text).ok_or_else(|| format!("{name} is not a preset"))?;
            Ok(Load::Params { name, params })
        }
        PresetRef::Vst3File { path } => {
            let d = vst3_desc(plugin).ok_or_else(|| format!("unknown plugin `{plugin}`"))?;
            let (name, state) = vstpreset_state(path, &d.class_id)?;
            Ok(Load::State { name, state })
        }
        PresetRef::Vst3Program { index } => {
            let p = soundcraft_vst3_host::programs(plugin).ok_or_else(|| INSERT_ONCE.to_string())?;
            let i = *index as usize;
            let (Some(name), Some(value)) = (p.names.get(i), p.values.get(i)) else { return Err(format!("no program {}", i + 1)) };
            let mut params = Map::new();
            params.insert(p.param.clone(), json!(value));
            Ok(Load::Params { name: name.clone(), params })
        }
        PresetRef::Clap { location, load_key } => {
            let name = label(app, plugin, preset);
            let location = (!location.is_empty()).then(|| location.clone());
            let preset = soundcraft_clap_host::ClapPreset { name: name.clone(), location, load_key: load_key.clone() };
            Ok(Load::Clap { name, preset })
        }
    }
}

/// The state of the plugin in `slot` once a CLAP preset is loaded into it: into the running
/// instance (made now for a module just placed), else (no audio engine) into a fresh instance.
fn clap_state(app: &mut SoundApp, t: TrackId, slot: SlotKey, plugin: &str, preset: &soundcraft_clap_host::ClapPreset) -> Result<Vec<u8>, String> {
    app.sync_player();
    let Some(p) = &app.player else { return soundcraft_clap_host::preset_state(plugin, preset) };
    if !p.load_preset(t, slot, preset.kind(), preset.location.as_deref(), preset.load_key.as_deref()) {
        return Err(format!("the plugin did not load the preset {}", preset.name));
    }
    p.capture_state(t, slot).ok_or_else(|| "the plugin did not give its state after the preset".to_string())
}

/// Loads `load` into the module in `slot` of track `t`: a step of its own, or (`joined`) part of
/// the step before (the plugin just placed there).
pub(crate) fn apply(app: &mut SoundApp, t: TrackId, slot: SlotKey, load: &Load, joined: bool) -> Result<(), String> {
    let plugin = slot_plugin(app, t, slot);
    let hosted = soundcraft_mix::is_third_party(&plugin);
    if hosted && !joined {
        // Undo puts back what is set in the plugin now, not what was last saved. Read once,
        // before anything reaches the plugin (a CLAP preset is loaded into it below).
        app.capture_slot_state(t, slot);
    }
    if let Load::Clap { name, preset } = load {
        let state = clap_state(app, t, slot, &plugin, preset)?;
        run_load(app, t, slot, &Load::State { name: name.clone(), state }, joined, hosted)?;
        // The player learns at once that the plugin holds the preset (it was loaded into it
        // directly), so an undo straight after loads the state from before back.
        app.sync_player();
        return Ok(());
    }
    run_load(app, t, slot, load, joined, hosted)
}

/// [`apply`] once the plugin's state is captured.
fn run_load(app: &mut SoundApp, t: TrackId, slot: SlotKey, load: &Load, joined: bool, hosted: bool) -> Result<(), String> {
    let module = match slot {
        SlotKey::Module(m) => json!(m.0),
        SlotKey::Instrument => json!("instrument"),
    };
    let (id, params) = match (load, slot) {
        (Load::Clap { .. }, _) => return Err("not a state".into()),
        // A built-in module's preset (derived parameters recalled with it).
        (Load::Params { name, params }, SlotKey::Module(_)) if !hosted => {
            ("mix.module_params", json!({"track": t.0, "module": module, "params": params, "preset": name}))
        }
        // An instrument's, or a hosted plugin's program: a step that also reloads the state on
        // undo (the program change lives in the plugin's state).
        (Load::Params { name, params }, _) => ("mix.module_preset", json!({"track": t.0, "module": module, "params": params, "preset": name})),
        (Load::State { name, state }, _) => {
            ("mix.module_preset", json!({"track": t.0, "module": module, "state": soundcraft_model::b64::encode(state), "preset": name}))
        }
    };
    if joined {
        app.engine.execute_joined(id, &params).map_err(|e| e.to_string())?;
    } else {
        app.run(id, params)?;
    }
    if let (true, Load::Params { params, .. }) = (hosted, load) {
        // A VST3 program: the controller (and so the plugin's own editor) follows at once, not
        // only at the next state read. The instance exists once the session is handed over.
        app.sync_player();
        if let Some(p) = &app.player {
            for (k, v) in params {
                if let Some(v) = v.as_f64() {
                    p.editor_set_param(t, slot, k, v as f32);
                }
            }
        }
    }
    Ok(())
}

/// The plugin in `slot` of track `t` (empty when there is none).
fn slot_plugin(app: &SoundApp, t: TrackId, slot: SlotKey) -> String {
    let s = app.engine.session();
    let ins = s.track(t).and_then(|tr| match slot {
        SlotKey::Instrument => tr.instrument.as_ref(),
        SlotKey::Module(m) => tr.mixer.module(m),
    });
    ins.map(|i| i.plugin.clone()).unwrap_or_default()
}

/// Loads `preset` into the module in `slot` of track `t` (which holds `plugin`): one undo step.
pub(crate) fn load_into(app: &mut SoundApp, t: TrackId, slot: SlotKey, plugin: &str, preset: &PresetRef) -> Result<(), String> {
    let load = resolve(app, plugin, preset)?;
    apply(app, t, slot, &load, false)
}

/// The plugin the tab shows: the selected card's, else that of the module open in a plugin window
/// (the latest) or a drawer (the first selected track's, else the topmost), with that slot.
fn shown_plugin(app: &SoundApp, catalog: &super::catalog::Catalog) -> Option<(BrowserItem, Option<(TrackId, SlotKey)>)> {
    let find = |id: &str| catalog.items.iter().find(|i| i.id == id).cloned();
    // A selected card the catalog no longer has falls back to the open module.
    if let Some(item) = app.ui.browser.selected.as_deref().and_then(find) {
        return Some((item, None));
    }
    let (t, slot) = focused_slot(app)?;
    find(&slot_plugin(app, t, slot)).map(|i| (i, Some((t, slot))))
}

/// The open module the tab can follow: the one last opened (a plugin's own editor included)
/// while it is still open, else the latest plugin window, else a drawer (the first selected
/// track's, else the topmost).
pub(crate) fn focused_slot(app: &SoundApp) -> Option<(TrackId, SlotKey)> {
    let s = app.engine.session();
    let exists = |t: TrackId, slot: SlotKey| !slot_plugin(app, t, slot).is_empty();
    let open = |t: TrackId, slot: SlotKey| {
        app.ui.plugin_windows.contains(&(t, slot))
            || matches!(slot, SlotKey::Module(m) if app.ui.drawers.get(&t) == Some(&m))
            || app.player.as_ref().is_some_and(|p| p.editor_open(t, slot))
    };
    if let Some((t, slot)) = app.module_focus.filter(|&(t, slot)| exists(t, slot) && open(t, slot)) {
        return Some((t, slot));
    }
    let window = app.ui.plugin_windows.iter().rev().copied().find(|&(t, slot)| exists(t, slot));
    window.or_else(|| {
        let first = s.edit.selected_tracks.first().and_then(|t| app.ui.drawers.get(t).map(|m| (*t, *m)));
        let top = || s.tracks.iter().find_map(|tr| app.ui.drawers.get(&tr.id).map(|m| (tr.id, *m)));
        let (t, m) = first.or_else(top)?;
        exists(t, SlotKey::Module(m)).then_some((t, SlotKey::Module(m)))
    })
}

/// Whatever happened last wins: an open module that changed since the last frame takes over from
/// the selected card (a card clicked afterwards takes over again). Called every frame the panel
/// is drawn, before the cards act.
pub(crate) fn follow_focus(app: &mut SoundApp) {
    let now = focused_slot(app);
    if now != app.browser_focus_seen {
        app.browser_focus_seen = now;
        if now.is_some() {
            app.ui.browser.selected = None;
        }
    }
}

/// One line of the list: a group's heading, or a preset (its index in the cache).
enum Line {
    Group(String),
    Preset(usize),
}

/// Height of a list line.
const LINE: f32 = 20.0;

/// The Presets tab below the search field: the plugin's name, a note, the presets by group
/// (drag one onto a track or slot; double-click to place it on the selected track, or to load it
/// into the open module the tab shows).
pub(crate) fn tab(app: &mut SoundApp, ui: &mut egui::Ui, catalog: &super::catalog::Catalog) {
    use crate::theme::{Tokens, bold, regular};
    let t = Tokens::current();
    let Some((item, slot)) = shown_plugin(app, catalog) else {
        ui.label(egui::RichText::new("Select a plugin card to see its presets.").color(t.text_dim));
        return;
    };
    let mut refresh = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&item.name).font(bold(13.0)).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            refresh = ui.small_button("Refresh").on_hover_text("List the preset files again").clicked();
        });
    });
    let via = match slot {
        Some(_) => " · the open module",
        None => "",
    };
    ui.label(egui::RichText::new(format!("{} · {}{via}", item.vendor, item.format.label())).color(t.text_dim).font(regular(11.0)));
    cached(app, &item, refresh);
    let Some(cache) = app.browser_presets.take() else { return };
    if cache.pending.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    if let Some(note) = &cache.note {
        ui.label(egui::RichText::new(note).color(t.text_dim).font(regular(11.0)));
    }
    let needle = app.ui.browser.preset_text.trim().to_lowercase();
    let mut lines = Vec::new();
    let mut group: Option<&str> = None;
    for (i, (g, name, _)) in cache.list.iter().enumerate() {
        if !needle.is_empty() && !name.to_lowercase().contains(&needle) && !g.to_lowercase().contains(&needle) {
            continue;
        }
        if group != Some(g.as_str()) {
            lines.push(Line::Group(g.clone()));
            group = Some(g.as_str());
        }
        lines.push(Line::Preset(i));
    }
    if cache.list.is_empty() && cache.pending.is_none() {
        ui.label(egui::RichText::new(format!("No presets for {}", item.name)).color(t.text_dim));
    } else if lines.is_empty() {
        ui.label(egui::RichText::new("No presets match").color(t.text_dim));
    }
    let mut chosen = None;
    // No space between lines: `show_rows` counts the spacing in each row's height.
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical().id_salt("browser-presets").auto_shrink([false, false]).show_rows(ui, LINE, lines.len(), |ui, range| {
        for line in lines.iter().skip(range.start).take(range.len()) {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), LINE), egui::Sense::hover());
            match line {
                Line::Group(g) => {
                    ui.painter().text(r.left_center() + vec2(2.0, 1.0), egui::Align2::LEFT_CENTER, g, bold(11.0), t.text_dim);
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, g));
                }
                Line::Preset(i) => {
                    let Some((_, name, preset)) = cache.list.get(*i) else { continue };
                    let resp = ui.interact(r, egui::Id::new(("browser-preset", *i)), egui::Sense::click_and_drag());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, egui::CornerRadius::same(3), t.panel_bg2);
                    }
                    ui.painter().text(r.left_center() + vec2(12.0, 0.0), egui::Align2::LEFT_CENTER, name, regular(11.5), t.text);
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
                    if resp.drag_started() {
                        super::drop::start_with(ui.ctx(), &item, Some(preset.clone()));
                    }
                    if resp.double_clicked() {
                        chosen = Some(preset.clone());
                    }
                }
            }
        }
    });
    app.browser_presets = Some(cache);
    let Some(preset) = chosen else { return };
    match slot {
        Some((track, slot)) => {
            if let Err(e) = load_into(app, track, slot, &item.id, &preset) {
                app.ui.status = e;
            }
        }
        None => super::drop::place_on_selected_with(app, &item, Some(preset)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::Insert;
    use std::sync::{Arc, Mutex};

    /// A running plugin's state handle: its state is what was last loaded into it.
    struct Live(Arc<Mutex<Vec<u8>>>);

    impl soundcraft_dsp::PluginStateReader for Live {
        fn save_state(&mut self) -> Option<Vec<u8>> {
            self.0.lock().ok().map(|s| s.clone())
        }
        fn load_state(&mut self, data: &[u8]) -> bool {
            if let Ok(mut s) = self.0.lock() {
                *s = data.to_vec();
            }
            true
        }
        fn load_preset(&mut self, _kind: u32, _location: Option<&str>, load_key: Option<&str>) -> bool {
            if let Ok(mut s) = self.0.lock() {
                *s = load_key.unwrap_or_default().as_bytes().to_vec();
            }
            true
        }
    }

    /// A session with a hosted insert on "A" and a hosted instrument on "K", both holding
    /// `saved`, and a player.
    fn hosted_app() -> (SoundApp, TrackId, TrackId, soundcraft_model::ModuleId) {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"name": "A"})).unwrap();
        engine.execute("track.new", &json!({"name": "K", "kind": "instrument"})).unwrap();
        let (a, k) = (engine.session().tracks[0].id, engine.session().tracks[1].id);
        let mut fx = Insert::new("clap:test.fx");
        fx.set_state_bytes(b"saved");
        let m = engine.session_mut().track_mut(a).unwrap().mixer.add_module(fx, None).unwrap();
        let mut synth = Insert::new("clap:test.synth");
        synth.set_state_bytes(b"saved");
        engine.session_mut().track_mut(k).unwrap().instrument = Some(synth);
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.sync_player();
        (app, a, k, m)
    }

    fn attach(app: &SoundApp, t: TrackId, slot: SlotKey, state: &[u8]) -> Arc<Mutex<Vec<u8>>> {
        let live = Arc::new(Mutex::new(state.to_vec()));
        app.player.as_ref().unwrap().attach_state_reader(t, slot, Box::new(Live(Arc::clone(&live))));
        live
    }

    /// Undo after a CLAP preset or a VST3 program loaded into a running plugin gives back the
    /// sound it had before (read once, before the preset reached the plugin).
    #[test]
    fn clap_presets_and_programs_undo_to_the_live_sound() {
        let (mut app, a, _, m) = hosted_app();
        let slot = SlotKey::Module(m);
        let live = attach(&app, a, slot, b"tweaked");
        let preset = soundcraft_clap_host::ClapPreset { name: "Warm".into(), location: None, load_key: Some("warm".into()) };
        apply(&mut app, a, slot, &Load::Clap { name: "Warm".into(), preset }, false).unwrap();
        assert_eq!(stored(&app, a, slot), b"warm");
        assert_eq!(*live.lock().unwrap(), b"warm");
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(stored(&app, a, slot), b"tweaked", "the sound before the preset");
        app.sync_player();
        assert_eq!(*live.lock().unwrap(), b"tweaked");
        // A program: the plugin changes its own state; undo loads the state from before.
        *live.lock().unwrap() = b"tweaked again".to_vec();
        let mut params = Map::new();
        params.insert("2103".into(), json!(1.0));
        apply(&mut app, a, slot, &Load::Params { name: "Prog 2".into(), params }, false).unwrap();
        app.sync_player();
        *live.lock().unwrap() = b"program 2".to_vec();
        app.run("edit.undo", json!({})).unwrap();
        app.sync_player();
        assert_eq!(*live.lock().unwrap(), b"tweaked again");
    }

    /// Replacing a hosted instrument (a dropped instrument card runs `mix.instrument`): undo
    /// gives back the instrument as it sounded, not as last saved.
    #[test]
    fn replacing_a_hosted_instrument_keeps_its_live_sound_for_undo() {
        let (mut app, _, k, _) = hosted_app();
        attach(&app, k, SlotKey::Instrument, b"tweaked");
        app.run("mix.instrument", json!({"track": k.0, "plugin": "subtractive_synth"})).unwrap();
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(stored(&app, k, SlotKey::Instrument), b"tweaked");
    }

    fn stored(app: &SoundApp, t: TrackId, slot: SlotKey) -> Vec<u8> {
        let tr = app.engine.session().track(t).unwrap();
        let ins = match slot {
            SlotKey::Instrument => tr.instrument.as_ref(),
            SlotKey::Module(m) => tr.mixer.module(m),
        };
        ins.unwrap().state_bytes().unwrap_or_default()
    }

    /// A hosted preset loads into the running plugin; undo gives back the sound the user had
    /// (read from the plugin, not the last saved state), for an insert and an instrument alike;
    /// and a save in the same frame as the load keeps the preset.
    #[test]
    fn hosted_presets_load_in_place_and_undo_to_the_live_sound() {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"name": "A"})).unwrap();
        engine.execute("track.new", &json!({"name": "K", "kind": "instrument"})).unwrap();
        let (a, k) = (engine.session().tracks[0].id, engine.session().tracks[1].id);
        let mut fx = Insert::new("vst3:0123456789ABCDEF0123456789ABCDEF");
        fx.set_state_bytes(b"saved");
        let m = engine.session_mut().track_mut(a).unwrap().mixer.add_module(fx, None).unwrap();
        let mut synth = Insert::new("clap:test.synth");
        synth.set_state_bytes(b"saved");
        engine.session_mut().track_mut(k).unwrap().instrument = Some(synth);
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.sync_player();
        for (t, slot) in [(a, SlotKey::Module(m)), (k, SlotKey::Instrument)] {
            let live = Arc::new(Mutex::new(b"tweaked".to_vec()));
            app.player.as_ref().unwrap().attach_state_reader(t, slot, Box::new(Live(Arc::clone(&live))));
            let load = Load::State { name: "Warm".into(), state: b"preset".to_vec() };
            apply(&mut app, t, slot, &load, false).unwrap();
            // Saved in the same frame: the plugin has the preset already.
            app.capture_plugin_states();
            assert_eq!(stored(&app, t, slot), b"preset");
            assert_eq!(*live.lock().unwrap(), b"preset", "loaded into the running plugin");
            app.run("edit.undo", json!({})).unwrap();
            assert_eq!(stored(&app, t, slot), b"tweaked", "the sound before the preset");
            app.sync_player();
            assert_eq!(*live.lock().unwrap(), b"tweaked");
        }
    }

    #[test]
    fn user_preset_json_in_either_format() {
        let info = soundcraft_dsp::plugin_info("fs_comp").unwrap();
        let p = &info.params[0];
        let ours = json!({"format": "soundcraft-plugin-preset", "params": {p.id: 0.5, "not_a_param": 1.0, "bad": "x"}}).to_string();
        let got = params_of_json(info, "Mine", &ours).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[p.id], json!(0.5));
        assert!(params_of_json(info, "Mine", "not json").is_none());
    }
}
