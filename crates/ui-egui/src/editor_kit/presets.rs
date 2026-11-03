//! `Common/PresetBrowser` and `Common/PresetManager`, translated onto the DAW's presets.
//!
//! - Factory presets are the engine's bundled ones (`fs_presets::factory_groups`), in the
//!   catalogue's group order; user presets are the files in the DAW's preset folder for the
//!   plugin (`SoundApp::preset_dir`, the folder the narrow drawer's preset menu uses). There is no
//!   preset folder on the web, so the browser shows factory presets only there.
//! - Categories: Favorites, All, the factory groups, User. Favourites persist as a JSON list next
//!   to the preset folders (`.favourites/<plugin folder>.json`), as `favourites.json` does.
//! - Recall goes through `mix.module_params {params, preset}` as one undo step. A preset may
//!   carry something that is not a parameter under the editor's `extra_key` (CleanQ: its guides
//!   under "cleanq"); the browser hands that back to the editor, which applies it in the same
//!   undo step ([`BrowserOut::recalled`]).
//! - Save writes `<name>.json` in the DAW's preset format (`params`, plus `tags` and the extra
//!   block, which the narrow drawer's menu ignores) and names the module's preset; Delete removes
//!   the current user preset and loads the first of the category, as `deletePreset` does.

use super::controls::{PillStyle, pill, pill_height, pill_width};
use super::look::{corner, font, glow_rect};
use super::palette::VstPalette;
use super::{Host, end_gesture, recall_preset_merged};
use crate::SoundApp;
use crate::theme::regular;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Map, Value, json};

/// One preset as the browser lists it (`fs::Preset`).
#[derive(Clone, Debug, Default)]
pub struct PresetEntry {
    pub name: String,
    pub group: String,
    pub tags: String,
    pub desc: String,
    pub is_user: bool,
    /// The preset's JSON text.
    json: String,
}

/// The browser's state, kept by the editor.
#[derive(Clone, Default, Debug)]
pub struct PresetState {
    /// The full-panel browser is showing.
    pub open: bool,
    pub category: String,
    pub search: String,
    pub name_box: String,
    /// A message and when it was shown (`showMessage`: cleared after 1.8 s).
    pub message: Option<(String, f64)>,
    /// The top-level key under which this engine's presets carry what is not a parameter
    /// (`PresetCatalogue::extraKey`), empty for none.
    pub extra_key: &'static str,
    /// What Save writes under `extra_key` (the editor sets it each frame; `None`: nothing).
    pub extra_out: Option<Value>,
    /// The tags Save writes (CleanQ: "3 bands").
    pub tags_out: String,
    /// The parameters `getPresetParams` leaves out besides the engine's own `bypass` (they are
    /// neither copied nor watched for the dirty star).
    pub not_in_presets: &'static [&'static str],
    /// ChamberMade's and Shale's preset handling: Copy, Paste, Save and Delete show no message, a
    /// paste is not dirty, and Delete goes to Init instead of loading the category's first.
    pub quiet: bool,
    entries: Vec<PresetEntry>,
    favourites: Vec<String>,
    scanned: bool,
    /// Paste was pressed; the clipboard's text comes as a paste event.
    awaiting_paste: bool,
}

/// One module's preset `dirty` flag (`presetState`'s "dirty"): the preset it is for, the
/// parameters as that preset left them, and whether anything moved since.
#[derive(Clone, Debug, Default)]
pub struct DirtyRef {
    name: String,
    params: std::collections::BTreeMap<String, f32>,
    dirty: bool,
}

impl DirtyRef {
    /// The " *" shows.
    pub fn dirty(&self) -> bool {
        self.dirty
    }
}

/// What the browser asks of its editor this frame.
#[derive(Clone, Debug, Default)]
pub struct BrowserOut {
    /// A preset was recalled; its extra block (if it has one) is to be applied now, inside the
    /// still-open `"preset"` gesture, then [`super::end_gesture`].
    pub recalled: Option<Option<Value>>,
    /// The Copy and Paste buttons (`onCopy`, `onPaste`).
    pub copy: bool,
    pub paste: bool,
    /// The recalled preset's parameters (with [`BrowserOut::recalled`]), for an editor whose
    /// `loadPreset` fills in what a preset does not mention.
    pub params: Option<Map<String, Value>>,
}

/// The merge key a recall is sent under (the editor's extra joins it).
pub const RECALL_GESTURE: &str = "preset";

/// The file system the browser touches: the user preset folder and the favourites file. None on
/// the web (no file system): factory presets only, no Save / Delete / Folder.
#[cfg(not(target_arch = "wasm32"))]
mod disk {
    use std::path::Path;
    pub fn json_files(dir: &Path) -> Vec<(String, String)> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        rd.flatten()
            .take(2000)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
            .filter_map(|p| Some((p.file_stem()?.to_str()?.to_string(), std::fs::read_to_string(&p).ok()?)))
            .collect()
    }
    pub fn read(f: &Path) -> Option<String> {
        std::fs::read_to_string(f).ok()
    }
    pub fn write(f: &Path, text: &str) -> bool {
        f.parent().is_none_or(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(f, text).is_ok()
    }
    pub fn remove(f: &Path) {
        let _ = std::fs::remove_file(f);
    }
    pub fn make_dir(d: &Path) {
        let _ = std::fs::create_dir_all(d);
    }
}

#[cfg(target_arch = "wasm32")]
mod disk {
    use std::path::Path;
    pub fn json_files(_dir: &Path) -> Vec<(String, String)> {
        Vec::new()
    }
    pub fn read(_f: &Path) -> Option<String> {
        None
    }
    pub fn write(_f: &Path, _text: &str) -> bool {
        false
    }
    pub fn remove(_f: &Path) {}
    pub fn make_dir(_d: &Path) {}
}

fn user_dir(app: &SoundApp, plugin: &str) -> Option<std::path::PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join(crate::preset_folder_name(plugin)))
}

fn favourites_file(app: &SoundApp, plugin: &str) -> Option<std::path::PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join(".favourites").join(format!("{}.json", crate::preset_folder_name(plugin))))
}

fn text_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// `rescan`: factory presets in group order, then user presets by name (case-insensitive).
pub(crate) fn rescan(app: &SoundApp, host: &Host, st: &mut PresetState) {
    let mut out = Vec::new();
    for (group, list) in crate::fs_presets::factory_groups(host.info) {
        for (name, json) in list {
            let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
            out.push(PresetEntry {
                name: (*name).to_string(),
                group: group.clone(),
                tags: text_of(&v, "tags"),
                desc: crate::hardware_credit::with_credit(&text_of(&v, "desc"), &crate::hardware_credit::preset_credit(host.info.id, name)),
                is_user: false,
                json: (*json).to_string(),
            });
        }
    }
    let mut users = Vec::new();
    if let Some(dir) = user_dir(app, host.info.id) {
        for (stem, text) in disk::json_files(&dir) {
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            users.push(PresetEntry {
                name: stem,
                group: "User".into(),
                tags: text_of(&v, "tags"),
                desc: text_of(&v, "desc"),
                is_user: true,
                json: text,
            });
        }
    }
    users.sort_by_key(|p| p.name.to_lowercase());
    out.extend(users);
    st.entries = out;
    st.favourites = favourites_file(app, host.info.id)
        .and_then(|f| disk::read(&f))
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default();
    st.scanned = true;
    if st.category.is_empty() {
        st.category = "All".into();
    }
}

/// `categories`: Favorites, All, the factory groups, User.
pub(crate) fn categories(host: &Host) -> Vec<String> {
    let mut c = vec!["Favorites".to_string(), "All".to_string()];
    c.extend(crate::fs_presets::factory_groups(host.info).iter().map(|(g, _)| g.clone()));
    c.push("User".into());
    c
}

fn list_for<'a>(st: &'a PresetState, category: &str) -> Vec<&'a PresetEntry> {
    st.entries.iter().filter(|p| category == "All" || (category == "Favorites" && st.favourites.contains(&p.name)) || p.group == category).collect()
}

fn search<'a>(st: &'a PresetState, q: &str) -> Vec<&'a PresetEntry> {
    let q = q.trim().to_lowercase();
    st.entries.iter().filter(|p| format!("{} {} {} {}", p.name, p.desc, p.group, p.tags).to_lowercase().contains(&q)).collect()
}

/// A preset's parameters (only ones the plugin has, finite) and its extra block.
fn decode(host: &Host, p: &PresetEntry, extra_key: &str) -> Option<(String, Map<String, Value>, Option<Value>)> {
    let v: Value = serde_json::from_str(&p.json).ok()?;
    let extra = (!extra_key.is_empty()).then(|| v.get(extra_key).cloned()).flatten().filter(|e| !e.is_null());
    if p.is_user && v.get("params").is_some() {
        let params: Map<String, Value> = v
            .get("params")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(k, val)| host.info.params.iter().any(|pi| pi.id == k.as_str()) && val.as_f64().is_some_and(f64::is_finite))
            .map(|(k, val)| (k.clone(), val.clone()))
            .collect();
        return Some((p.name.clone(), params, extra));
    }
    // FireStation's own format (factory presets, and FireStation user files dropped in).
    let (_, params) = crate::fs_presets::factory_recall(host.info, &p.name, &p.json)?;
    Some((p.name.clone(), params, extra))
}

/// `loadPreset`: the parameters as one undo step (left open for the extra; see
/// [`BrowserOut::recalled`]).
fn load(app: &mut SoundApp, host: &Host, st: &mut PresetState, p: &PresetEntry, out: &mut BrowserOut) {
    let Some((name, params, extra)) = decode(host, p, st.extra_key) else {
        show_message(st, "Not a preset", 0.0);
        return;
    };
    end_gesture(app);
    recall_preset_merged(app, host, &name, &params, RECALL_GESTURE);
    st.name_box = name;
    out.recalled = Some(extra);
    out.params = Some(params);
}

/// `copyPreset` / `pastePreset`, the browser's Copy and Paste: Copy puts FireStation's clipboard
/// text (`PresetManager::toClipboardText`: `{"plugin", "name", "values"}`, choices by their
/// labels) on the clipboard; Paste reads one back (`fromClipboardText`) and loads it as one undo
/// step, "Pasted, Save to keep" and dirty, or says "Not a preset" (silently, and clean, where
/// [`PresetState::quiet`]). Returns the pasted parameters when it loaded some.
pub(crate) fn clipboard(app: &mut SoundApp, ui: &Ui, host: &Host, st: &mut PresetState, out: &BrowserOut) -> Option<Map<String, Value>> {
    let now = ui.input(|i| i.time);
    if out.copy {
        ui.ctx().copy_text(clipboard_text(host, st));
        if !st.quiet {
            show_message(st, "Copied", now);
        }
    }
    if out.paste {
        st.awaiting_paste = true;
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }
    if !st.awaiting_paste {
        return None;
    }
    let text = ui.input(|i| i.events.iter().find_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }))?;
    st.awaiting_paste = false;
    let parsed = serde_json::from_str::<Value>(&text).ok().filter(|v| v.get("values").is_some_and(Value::is_object));
    match parsed.and_then(|_| crate::fs_presets::factory_recall(host.info, "Pasted", &text)) {
        Some((name, params)) => {
            end_gesture(app);
            recall_preset_merged(app, host, &name, &params, RECALL_GESTURE);
            end_gesture(app);
            st.name_box = name;
            if !st.quiet {
                // `markPresetDirty` after the load.
                set_dirty(app, host, true);
                show_message(st, "Pasted \u{2014} Save to keep", now);
            }
            Some(params)
        }
        None => {
            if !st.quiet {
                show_message(st, "Not a preset", now);
            }
            None
        }
    }
}

/// `toClipboardText`: the preset parameters (choices by their labels, `encodeValue`) under the
/// engine's storage key and the current preset's name.
pub(crate) fn clipboard_text(host: &Host, st: &PresetState) -> String {
    let mut vals = Map::new();
    for p in host.info.params.iter().filter(|p| in_presets(st, p.id)) {
        let v = super::param(host, p.id);
        let enc = if p.unit == soundcraft_dsp::Unit::Choice && !p.choices.is_empty() {
            json!(p.choices[(v.round().max(0.0) as usize).min(p.choices.len() - 1)])
        } else {
            json!(v)
        };
        vals.insert(p.id.to_string(), enc);
    }
    let key = crate::engine_ui::engine_ui(host.info.id).and_then(|u| u.user_presets).unwrap_or(host.info.id);
    json!({"plugin": key, "name": shown_name(host), "values": vals}).to_string()
}

fn in_presets(st: &PresetState, id: &str) -> bool {
    id != "bypass" && !st.not_in_presets.contains(&id)
}

fn show_message(st: &mut PresetState, text: &str, now: f64) {
    st.message = Some((text.to_string(), now));
}

/// The name the preset button shows: "Init" for none (or the DAW's "<factory default>", which is
/// the same thing: every parameter at its default).
pub(crate) fn shown_name<'a>(host: &'a Host) -> &'a str {
    let n = host.ins.preset.trim();
    if n.is_empty() || n.starts_with('<') { "Init" } else { n }
}

/// The preset's parameters as recalled; Init (or no name), and a name no list knows, are every
/// parameter at its default.
fn preset_values(host: &Host, st: &PresetState, name: &str) -> Map<String, Value> {
    let defaults = || host.info.params.iter().map(|p| (p.id.to_string(), json!(p.default))).collect();
    if name.trim().is_empty() || name == "Init" || name.starts_with('<') {
        return defaults();
    }
    for (_, list) in crate::fs_presets::factory_groups(host.info) {
        if let Some((n, js)) = list.iter().find(|(n, _)| *n == name) {
            return crate::fs_presets::factory_recall(host.info, n, js).map_or_else(defaults, |(_, m)| m);
        }
    }
    st.entries.iter().find(|e| e.name == name).and_then(|e| decode(host, e, "")).map_or_else(defaults, |(_, m, _)| m)
}

/// Marks the module's preset clean (`setCurrentPreset (name, false)`: a load or save just named
/// it) or dirty (`markPresetDirty`), as the session now has it.
pub(crate) fn set_dirty(app: &mut SoundApp, host: &Host, dirty: bool) {
    let Some(ins) = app.engine.session().track(host.track).and_then(|t| t.mixer.module(host.ins.id)) else { return };
    let r = DirtyRef { name: ins.preset.clone(), params: ins.params.clone(), dirty };
    app.ui.preset_dirty.insert((host.track, host.ins.id), r);
}

/// `isPresetDirty`, the name button's accent " *": raised by any change of a preset parameter
/// since the last preset load or save (`parameterChanged` → `markPresetDirty`), even one moved
/// back. The flag lives with the UI, not the session, so a module first seen (a session just
/// opened) starts dirty when its parameters differ from what its preset sets.
pub(crate) fn is_dirty(app: &mut SoundApp, host: &Host, st: &mut PresetState) -> bool {
    let key = (host.track, host.ins.id);
    if app.ui.preset_dirty.get(&key).is_none_or(|d| d.name != host.ins.preset) {
        if !st.scanned {
            rescan(app, host, st);
        }
        let want = preset_values(host, st, &host.ins.preset);
        let dirty = want.iter().any(|(k, v)| {
            in_presets(st, k)
                && host.info.params.iter().find(|p| p.id == k.as_str()).is_some_and(|p| {
                    let want = p.clamp(v.as_f64().unwrap_or(f64::from(p.default)) as f32);
                    (super::param(host, p.id) - want).abs() > 1e-4 * (p.max - p.min).abs().max(1e-6)
                })
        });
        app.ui.preset_dirty.insert(key, DirtyRef { name: host.ins.preset.clone(), params: host.ins.params.clone(), dirty });
    }
    let Some(d) = app.ui.preset_dirty.get_mut(&key) else { return false };
    if !d.dirty {
        let moved = |k: &String| in_presets(st, k) && d.params.get(k).map(|v| v.to_bits()) != host.ins.params.get(k).map(|v| v.to_bits());
        d.dirty = host.ins.params.keys().chain(d.params.keys()).any(moved);
    }
    d.dirty
}

/// `stepPreset (direction)`: the next or previous preset of the current category, wrapping.
pub(crate) fn step(app: &mut SoundApp, host: &Host, st: &mut PresetState, direction: i32) -> BrowserOut {
    let mut out = BrowserOut::default();
    if !st.scanned {
        rescan(app, host, st);
    }
    let cat = if st.category.is_empty() { "All".to_string() } else { st.category.clone() };
    let list: Vec<PresetEntry> = list_for(st, &cat).into_iter().cloned().collect();
    if list.is_empty() {
        return out;
    }
    let n = list.len() as i64;
    let i = list.iter().position(|p| p.name == host.ins.preset).map_or(0, |i| (i as i64 + n + i64::from(direction)).rem_euclid(n) as usize);
    if let Some(p) = list.get(i) {
        load(app, host, st, p, &mut out);
    }
    out
}

/// ChamberMade's and Shale's `stepPreset`: through every preset (`presetManager.search ({})`,
/// whatever the browser's category), wrapping; from a name that is not a preset, `>` goes to the
/// first and `<` to the last.
pub(crate) fn step_all(app: &mut SoundApp, host: &Host, st: &mut PresetState, direction: i32) -> BrowserOut {
    let mut out = BrowserOut::default();
    if !st.scanned {
        rescan(app, host, st);
    }
    let n = st.entries.len() as i64;
    if n == 0 {
        return out;
    }
    let i = match st.entries.iter().position(|p| p.name == host.ins.preset) {
        Some(i) => (i as i64 + i64::from(direction)).rem_euclid(n),
        None if direction > 0 => 0,
        None => n - 1,
    };
    if let Some(p) = st.entries.get(i as usize).cloned() {
        load(app, host, st, &p, &mut out);
    }
    out
}

/// Opens the browser (`setCurrentPresetName`, `setNameBoxText`, `setVisible`): it always reloads
/// its lists when it opens.
pub(crate) fn open(app: &SoundApp, host: &Host, st: &mut PresetState) {
    st.open = true;
    st.name_box = shown_name(host).to_string();
    rescan(app, host, st);
}

/// The full-panel browser over `rect` (the whole editor) while `st.open`.
pub(crate) fn preset_browser(app: &mut SoundApp, ui: &mut Ui, host: &Host, rect: Rect, pal: &VstPalette, st: &mut PresetState) -> BrowserOut {
    let mut out = BrowserOut::default();
    if !st.open {
        return out;
    }
    if !st.scanned {
        rescan(app, host, st);
    }
    let now = ui.input(|i| i.time);
    if st.message.as_ref().is_some_and(|(_, at)| *at > 0.0 && now - at > 1.8) {
        st.message = None;
    }
    if let Some((_, at)) = st.message.as_mut().filter(|(_, at)| *at == 0.0) {
        *at = now.max(1e-6);
    }
    let p = ui.painter().clone();
    let _ = ui.interact(rect, ui.id().with("vst-browser-swallow"), Sense::click_and_drag());
    p.rect_filled(rect, CornerRadius::ZERO, pal.background);

    // ---- resized()
    let b = rect.shrink2(vec2((rect.width() * 0.03).round(), (rect.height() * 0.025).round()));
    let top = Rect::from_min_size(b.min, vec2(b.width(), 26.0));
    p.text(pos2(top.left(), top.center().y), Align2::LEFT_CENTER, "Presets", font(16.0, true), pal.text_bright);
    let close_w = pill_width(ui, "Close", PillStyle::Small);
    let close = Rect::from_center_size(pos2(top.right() - close_w * 0.5, top.center().y), vec2(close_w, pill_height(PillStyle::Small)));
    let search_r = Rect::from_min_max(pos2(top.left() + 96.0, top.top() + 2.0), pos2(close.left() - 10.0, top.bottom() - 2.0));
    if pill(ui, close, "close", pal, "Close", false, PillStyle::Small).clicked() {
        st.open = false;
    }
    if text_box(ui, search_r, "search", pal, &mut st.search, "Search presets\u{2026}") {
        // `search.onTextChange = refresh`: the list below follows the text every frame.
    }

    let foot = Rect::from_min_max(pos2(b.left(), b.bottom() - 26.0), b.max);
    let body = Rect::from_min_max(pos2(b.left(), top.bottom() + 10.0), pos2(b.right(), foot.top() - 10.0));
    let current = host.ins.preset.clone();
    let can_delete = st.entries.iter().any(|e| e.name == current && e.is_user);
    let desktop = app.preset_dir.is_some();
    let mut x = foot.right();
    let mut btn = |ui: &mut Ui, label: &str| {
        x -= 6.0;
        let w = pill_width(ui, label, PillStyle::Small);
        let r = Rect::from_center_size(pos2(x - w * 0.5, foot.center().y), vec2(w, pill_height(PillStyle::Small)));
        x -= w;
        pill(ui, r, ("foot", label.to_string()), pal, label, false, PillStyle::Small).clicked()
    };
    let folder = desktop && btn(ui, "Folder");
    let paste = btn(ui, "Paste");
    let copy = btn(ui, "Copy");
    let delete = can_delete && btn(ui, "Delete");
    let save = desktop && btn(ui, "Save");
    let msg_r = Rect::from_min_max(pos2(x - 140.0, foot.top()), pos2(x, foot.bottom()));
    let name_r = Rect::from_min_max(pos2(foot.left(), foot.top() + 2.0), pos2(msg_r.left(), foot.bottom() - 2.0));
    let enter = text_box(ui, name_r, "name", pal, &mut st.name_box, "Preset name");
    if let Some((m, _)) = &st.message {
        p.text(pos2(msg_r.right(), msg_r.center().y), Align2::RIGHT_CENTER, m, regular(10.0), pal.text_dim);
    }
    out.copy = copy;
    out.paste = paste;
    if folder && let Some(dir) = user_dir(app, host.info.id) {
        disk::make_dir(&dir);
        ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file:///{}", dir.display().to_string().replace('\\', "/"))));
    }
    if save || (enter && desktop) {
        save_user(app, host, st, now);
    }
    if delete {
        delete_user(app, host, st, now, &mut out);
    }

    // ---- the category list and the card grid
    let cats_r = Rect::from_min_size(body.min, vec2(150.0, body.height()));
    let grid_r = Rect::from_min_max(pos2(cats_r.right() + 12.0, body.top()), body.max);
    let cats = categories(host);
    let counts: Vec<usize> = cats.iter().map(|c| list_for(st, c).len()).collect();
    let mut cui = ui.new_child(egui::UiBuilder::new().id_salt("vst-browser-cats").max_rect(cats_r));
    cui.set_clip_rect(cats_r.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical().id_salt("vst-browser-cats-scroll").auto_shrink([false, false]).show(&mut cui, |ui| {
        let row_h = 26.0;
        for (i, c) in cats.iter().enumerate() {
            let (row, resp) = ui.allocate_exact_size(vec2(cats_r.width() - 8.0, row_h + 3.0), Sense::click());
            let row = Rect::from_min_size(row.min, vec2(row.width(), row_h));
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, *c == st.category, format!("Category {c}")));
            let on = *c == st.category;
            let pp = ui.painter();
            if on {
                pp.rect_filled(row, CornerRadius::same(3), pal.panel);
                pp.rect_stroke(row.shrink(0.5), CornerRadius::same(3), Stroke::new(1.0, pal.border), StrokeKind::Middle);
            }
            let text = row.shrink2(vec2(10.0, 0.0));
            let ink = if on {
                pal.accent
            } else if resp.hovered() {
                pal.text_bright
            } else {
                pal.text_dim
            };
            pp.text(pos2(text.left(), text.center().y), Align2::LEFT_CENTER, c, font(11.0, on), ink);
            pp.text(
                pos2(text.right(), text.center().y),
                Align2::RIGHT_CENTER,
                counts.get(i).copied().unwrap_or(0).to_string(),
                regular(10.0),
                pal.text_faint,
            );
            if resp.clicked() {
                st.category = c.clone();
            }
        }
    });

    let list: Vec<PresetEntry> =
        if st.search.trim().is_empty() { list_for(st, &st.category) } else { search(st, &st.search) }.into_iter().cloned().collect();
    let empty = if !st.search.trim().is_empty() {
        "Nothing matches.".to_string()
    } else if st.category == "User" {
        if desktop {
            "No user presets yet \u{2014} dial something in, name it, Save.".to_string()
        } else {
            "User presets need the desktop app.".to_string()
        }
    } else {
        "Nothing here yet.".to_string()
    };
    let mut gui = ui.new_child(egui::UiBuilder::new().id_salt("vst-browser-grid").max_rect(grid_r));
    gui.set_clip_rect(grid_r.intersect(ui.clip_rect()));
    let mut chosen = None;
    let mut fav_toggle = None;
    egui::ScrollArea::vertical().id_salt("vst-browser-grid-scroll").auto_shrink([false, false]).show(&mut gui, |ui| {
        const MIN_CARD_W: f32 = 190.0;
        const CARD_H: f32 = 76.0;
        const GAP: f32 = 8.0;
        let avail = (grid_r.width() - 8.0).max(200.0);
        let columns = ((avail / (MIN_CARD_W + GAP)).floor() as usize).max(1);
        let card_w = ((avail - GAP * (columns as f32 - 1.0)) / columns as f32).floor();
        let rows = list.len().div_ceil(columns);
        let (area, _) = ui.allocate_exact_size(vec2(avail, (rows as f32 * (CARD_H + GAP)).max(40.0)), Sense::hover());
        let pp = ui.painter().clone();
        if list.is_empty() {
            pp.text(area.min + vec2(10.0, 10.0), Align2::LEFT_TOP, &empty, regular(11.0), pal.text_faint);
            return;
        }
        for (i, preset) in list.iter().enumerate() {
            let (col, row) = (i % columns, i / columns);
            let r = Rect::from_min_size(area.min + vec2(col as f32 * (card_w + GAP), row as f32 * (CARD_H + GAP)), vec2(card_w, CARD_H));
            let resp = ui.interact(r, ui.id().with(("vst-preset-card", i)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Preset card {}", preset.name)));
            let on = preset.name == current;
            pp.rect_filled(r, corner(4.0), pal.panel);
            if on {
                glow_rect(&pp, r, 4.0, pal.accent, 1.2);
            }
            let edge = if on {
                pal.accent
            } else if resp.hovered() {
                pal.border_hover
            } else {
                pal.border
            };
            pp.rect_stroke(r.shrink(0.5), corner(4.0), Stroke::new(1.0, edge), StrokeKind::Middle);
            let inner = Rect::from_min_max(r.min + vec2(10.0, 8.0), r.max - vec2(26.0, 8.0));
            let pc = pp.with_clip_rect(inner.intersect(pp.clip_rect()));
            pc.text(inner.min, Align2::LEFT_TOP, &preset.name, regular(12.0), pal.text_bright);
            let tag = if !preset.tags.is_empty() {
                preset.tags.clone()
            } else if preset.is_user {
                "User".into()
            } else {
                String::new()
            };
            pc.text(pos2(inner.left(), inner.top() + 15.0 + 6.5), Align2::LEFT_CENTER, tag, regular(10.0), pal.accent);
            let desc = ui.painter().layout(preset.desc.clone(), regular(10.0), pal.text_dim, inner.width().max(10.0));
            pc.galley(pos2(inner.left(), inner.top() + 28.0), desc, pal.text_dim);
            let star = Rect::from_min_size(pos2(r.right() - 20.0, r.top()), vec2(20.0, 20.0));
            let fav = st.favourites.contains(&preset.name);
            star_glyph(&pp, star.center(), 5.5, if fav { pal.star } else { pal.border_hover });
            if resp.clicked() {
                let at = resp.interact_pointer_pos().unwrap_or(r.center());
                if star.contains(at) {
                    fav_toggle = Some(preset.name.clone());
                } else {
                    chosen = Some(preset.clone());
                }
            }
        }
    });
    if let Some(name) = fav_toggle {
        toggle_favourite(app, host, st, &name);
    }
    if let Some(p) = chosen {
        load(app, host, st, &p, &mut out);
    }
    out
}

/// U+2605, drawn: a five-pointed star of outer radius `r`.
fn star_glyph(p: &egui::Painter, c: egui::Pos2, r: f32, col: Color32) {
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 5.0;
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            c + vec2(a.sin() * rr, -a.cos() * rr)
        })
        .collect();
    // Fan from the centre: the star is not convex.
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, col);
    for q in &pts {
        mesh.colored_vertex(*q, col);
    }
    for i in 0..10u32 {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % 10);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// A `styleEditor` text box (panel ground, border, accent when focused, faint placeholder).
/// Returns true when Enter was pressed in it.
fn text_box(ui: &mut Ui, r: Rect, salt: &str, pal: &VstPalette, text: &mut String, hint: &str) -> bool {
    let id = ui.id().with(("vst-text", salt));
    let focused = ui.memory(|m| m.has_focus(id));
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(3), pal.panel);
    p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, if focused { pal.accent } else { pal.border }), StrokeKind::Inside);
    let te = egui::TextEdit::singleline(text)
        .id(id)
        .font(regular(11.0))
        .text_color(pal.text_bright)
        .hint_text(egui::RichText::new(hint).color(pal.text_faint))
        .frame(egui::Frame::NONE)
        .vertical_align(egui::Align::Center);
    let resp = ui.put(r.shrink2(vec2(6.0, 2.0)), te);
    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn toggle_favourite(app: &SoundApp, host: &Host, st: &mut PresetState, name: &str) {
    if let Some(i) = st.favourites.iter().position(|f| f == name) {
        st.favourites.remove(i);
    } else {
        st.favourites.push(name.to_string());
    }
    if let Some(f) = favourites_file(app, host.info.id) {
        let _ = disk::write(&f, &json!(st.favourites).to_string());
    }
}

/// `savePreset`.
fn save_user(app: &mut SoundApp, host: &Host, st: &mut PresetState, now: f64) {
    let name = st.name_box.trim().to_string();
    // ChamberMade's and Shale's `savePreset` checks nothing: `saveUser` writes whatever is in the
    // box (an empty name becomes the file "Preset", `sanitiseFileName`), a factory name included,
    // and a failed write is silent.
    if !st.quiet {
        if name.is_empty() {
            return show_message(st, "Name it first", now);
        }
        if st.entries.iter().any(|e| e.name == name && !e.is_user) {
            return show_message(st, "Factory name \u{2014} pick another", now);
        }
    }
    let Some(dir) = user_dir(app, host.info.id) else { return };
    let clean = if name.is_empty() { "Preset".to_string() } else { soundcraft_engine::io::sanitize_name(&name) };
    let existed = st.entries.iter().any(|e| e.is_user && e.name == clean);
    let params: Map<String, Value> = host.info.params.iter().map(|p| (p.id.to_string(), json!(super::param(host, p.id)))).collect();
    let mut body = json!({"format": "soundcraft-plugin-preset", "plugin": host.info.id, "name": clean, "tags": st.tags_out, "params": params});
    if let (false, Some(extra), Some(o)) = (st.extra_key.is_empty(), st.extra_out.clone(), body.as_object_mut()) {
        o.insert(st.extra_key.to_string(), extra);
    }
    let ok = disk::write(&dir.join(format!("{clean}.json")), &body.to_string());
    if !ok {
        if !st.quiet {
            show_message(st, "Could not write file", now);
        }
        return;
    }
    let _ = app.run("mix.module_params", json!({"track": host.track.0, "module": host.ins.id.0, "params": {}, "preset": clean}));
    set_dirty(app, host, false);
    rescan(app, host, st);
    st.name_box = clean;
    if !st.quiet {
        show_message(st, if existed { "Overwritten" } else { "Saved" }, now);
    }
}

/// `deletePreset`.
fn delete_user(app: &mut SoundApp, host: &Host, st: &mut PresetState, now: f64, out: &mut BrowserOut) {
    let name = host.ins.preset.clone();
    if !st.entries.iter().any(|e| e.is_user && e.name == name) {
        return;
    }
    if let Some(dir) = user_dir(app, host.info.id) {
        disk::remove(&dir.join(format!("{name}.json")));
    }
    st.favourites.retain(|f| *f != name);
    rescan(app, host, st);
    let first = if st.quiet { None } else { list_for(st, &st.category).first().map(|p| (*p).clone()) };
    match first {
        Some(p) => load(app, host, st, &p, out),
        None => {
            let _ = app.run("mix.module_params", json!({"track": host.track.0, "module": host.ins.id.0, "params": {}, "preset": "Init"}));
            set_dirty(app, host, false);
            st.name_box = "Init".into();
        }
    }
    if !st.quiet {
        show_message(st, "Deleted", now);
    }
}
