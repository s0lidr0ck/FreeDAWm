//! The browser's Library tab: everything the user saved to use again, in one list. Plug-in
//! chains, track presets, plugin presets, Juke presets and session templates all live in the
//! Library folder (`user_files`); double-click one to use it on the selected track (a template
//! opens the New dialog on it), and Save puts the selected track's chain or strip, or the
//! session as a template, there.

use super::catalog::Catalog;
use super::presets::PresetRef;
use crate::SoundApp;
use crate::user_files::{self, UserDirs, UserFile};
use egui::vec2;
use serde_json::json;
use std::path::{Path, PathBuf};

/// What a Library item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum LibraryKind {
    Chain,
    Track,
    Effect,
    Juke,
    Template,
}

impl LibraryKind {
    pub const ALL: [LibraryKind; 5] = [LibraryKind::Chain, LibraryKind::Track, LibraryKind::Effect, LibraryKind::Juke, LibraryKind::Template];

    /// The group's heading in the list.
    pub fn heading(self) -> &'static str {
        match self {
            LibraryKind::Chain => "Plug-in Chains",
            LibraryKind::Track => "Track Presets",
            LibraryKind::Effect => "Plugin Presets",
            LibraryKind::Juke => "Juke Presets",
            LibraryKind::Template => "Session Templates",
        }
    }

    /// The filter chip's label.
    pub fn chip(self) -> &'static str {
        match self {
            LibraryKind::Chain => "Chains",
            LibraryKind::Track => "Tracks",
            LibraryKind::Effect => "Plugins",
            LibraryKind::Juke => "Juke",
            LibraryKind::Template => "Templates",
        }
    }
}

/// One thing in the Library.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryItem {
    pub kind: LibraryKind,
    pub name: String,
    pub path: PathBuf,
    /// A plugin preset's plugin: its folder under `Presets/` (`preset_folder_name` of its id).
    pub plugin_folder: Option<String>,
    /// The line's dim text: the date saved, a plugin preset's plugin.
    pub detail: String,
}

/// Most plugin folders looked into, and most presets listed from one (a folder is user input).
const MAX_PLUGIN_FOLDERS: usize = 2000;
const MAX_PER_PLUGIN: usize = 200;

fn item(kind: LibraryKind, f: UserFile) -> LibraryItem {
    let detail = f.modified.map(user_files::short_date).unwrap_or_default();
    LibraryItem { kind, name: f.name, path: f.path, plugin_folder: None, detail }
}

/// The plugin presets under `root`: each plugin's folder, by folder then newest first. Folders
/// that start with a dot (the favourites) are not presets.
fn plugin_presets(root: Option<&Path>) -> Vec<LibraryItem> {
    let Some(root) = root else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut folders: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).take(MAX_PLUGIN_FOLDERS).filter(|p| p.is_dir()).collect();
    folders.sort();
    let mut out = Vec::new();
    for dir in folders {
        let Some(folder) = dir.file_name().and_then(|x| x.to_str()).filter(|n| !n.starts_with('.')).map(str::to_string) else { continue };
        for f in user_files::list(Some(&dir), user_files::TRACK_PRESET_EXT).into_iter().take(MAX_PER_PLUGIN) {
            out.push(LibraryItem {
                kind: LibraryKind::Effect,
                name: f.name,
                path: f.path,
                plugin_folder: Some(folder.clone()),
                detail: folder.clone(),
            });
        }
    }
    out
}

/// Everything in the Library, by kind in [`LibraryKind::ALL`]'s order. `juke` is the Juke
/// presets already read (their names are inside the files).
pub fn scan(dirs: &UserDirs, juke: Vec<(String, PathBuf)>) -> Vec<LibraryItem> {
    let mut out: Vec<LibraryItem> = user_files::chains(dirs).into_iter().map(|f| item(LibraryKind::Chain, f)).collect();
    out.extend(user_files::track_presets(dirs).into_iter().map(|f| item(LibraryKind::Track, f)));
    out.extend(plugin_presets(dirs.presets.as_deref()));
    out.extend(juke.into_iter().map(|(name, path)| {
        let detail = std::fs::metadata(&path).and_then(|m| m.modified()).ok().map(user_files::short_date).unwrap_or_default();
        LibraryItem { kind: LibraryKind::Juke, name, path, plugin_folder: None, detail }
    }));
    out.extend(user_files::templates(dirs).into_iter().map(|f| item(LibraryKind::Template, f)));
    out
}

/// The Library as last read.
#[derive(Debug, Default)]
pub(crate) struct LibraryCache {
    items: Vec<LibraryItem>,
    /// When it was read (egui's clock, seconds).
    at: f64,
}

/// How long a reading of the folder is shown before it is read again (a save from a dialog, or a
/// sync service, changes it under the panel).
const STALE_SECONDS: f64 = 2.0;

impl SoundApp {
    /// The Library folder in use: the one the preferences name, else the settings folder. `None`
    /// without either (the web build, tests).
    pub fn library_root(&self) -> Option<PathBuf> {
        let chosen = self.ui.library_dir.trim();
        if chosen.is_empty() { self.library_default.clone() } else { Some(PathBuf::from(chosen)) }
    }

    /// Points the preset menus, the save dialogs and Juke at the Library folder in use. Without
    /// one, what a host or test set by hand stays.
    pub fn apply_library_dir(&mut self) {
        let Some(root) = self.library_root() else { return };
        self.dialogs.dirs = UserDirs::under(&root);
        self.preset_dir = self.dialogs.dirs.presets.clone();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.juke_kits.presets = self.dialogs.dirs.juke_presets.clone();
        }
        self.browser_library = None;
        self.browser_presets = None;
    }

    /// Moves the Library to `dir` (`None`: back to the settings folder). What was saved in the
    /// folder before stays there: nothing is copied or deleted.
    pub fn set_library_dir(&mut self, dir: Option<&str>) {
        self.ui.library_dir = dir.unwrap_or_default().trim().to_string();
        self.apply_library_dir();
    }
}

/// The Juke presets in the Library (none on the web).
fn juke_presets(app: &SoundApp) -> Vec<(String, PathBuf)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        app.juke_kits.user_presets()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        Vec::new()
    }
}

/// One line of the list: a group's heading, or an item (its index in the cache).
enum Line {
    Group(LibraryKind),
    Item(usize),
}

const LINE: f32 = 20.0;

/// What a click on the tab asks for.
enum Ask {
    Use(usize),
    /// A chain after the track's inserts, not in their place.
    Append(usize),
    Save(&'static str),
    Folder(Option<String>),
}

/// The Library tab below the search field.
pub(crate) fn tab(app: &mut SoundApp, ui: &mut egui::Ui, catalog: &Catalog) {
    use crate::theme::{Tokens, bold, regular};
    let t = Tokens::current();
    if app.dialogs.dirs.root.is_none() {
        ui.label(egui::RichText::new("The Library needs the desktop app: it is a folder on this computer.").color(t.text_dim));
        return;
    }
    let now = ui.input(|i| i.time);
    if app.browser_library.as_ref().is_none_or(|c| now - c.at > STALE_SECONDS || now < c.at) {
        app.browser_library = Some(LibraryCache { items: scan(&app.dialogs.dirs, juke_presets(app)), at: now });
    }
    let Some(cache) = app.browser_library.take() else { return };
    let mut ask = None;

    // Save, and the kinds.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        ui.menu_button("Save…", |ui| {
            for (label, cmd, tip) in [
                ("Plug-in Chain…", "track.save_chain", "The selected track's inserts, to put on other tracks"),
                ("Track Preset…", "track.save_preset", "The selected track's whole strip: level, inserts, sends, instrument"),
                ("Session Template…", "session.save_template", "This session's tracks and routing, without its clips"),
            ] {
                if ui.button(label).on_hover_text(tip).clicked() {
                    ask = Some(Ask::Save(cmd));
                    ui.close();
                }
            }
            ui.separator();
            ui.label(egui::RichText::new("Plugin presets: the preset menu of the plugin").color(t.text_dim).font(regular(11.0)));
            ui.label(egui::RichText::new("Juke presets: Save preset in Juke").color(t.text_dim).font(regular(11.0)));
        })
        .response
        .on_hover_text("Save to the Library, to use in any session");
        let active = app.ui.browser.library_kind;
        for kind in LibraryKind::ALL {
            // A plugin preset counts when the app has its plugin (the list leaves the others out).
            let has = |i: &LibraryItem| {
                i.kind == kind && i.plugin_folder.as_deref().is_none_or(|f| catalog.items.iter().any(|c| crate::preset_folder_name(&c.id) == f))
            };
            if !cache.items.iter().any(has) {
                continue;
            }
            if super::panel::chip(ui, kind.chip(), active == Some(kind)) {
                app.ui.browser.library_kind = if active == Some(kind) { None } else { Some(kind) };
            }
        }
    });

    // A plugin preset's plugin, by its folder: its id and name. One the app does not have (a
    // plugin since removed, another folder kept beside the presets) is not listed.
    let plugin_of = |folder: &str| catalog.items.iter().find(|i| crate::preset_folder_name(&i.id) == folder);
    let needle = app.ui.browser.library_text.trim().to_lowercase();
    let kind_filter = app.ui.browser.library_kind;
    let mut lines = Vec::new();
    let mut group = None;
    for (i, it) in cache.items.iter().enumerate() {
        if kind_filter.is_some_and(|k| k != it.kind) {
            continue;
        }
        let plugin = it.plugin_folder.as_deref().map(plugin_of);
        if plugin == Some(None) {
            continue;
        }
        let plugin_name = plugin.flatten().map(|p| p.name.to_lowercase()).unwrap_or_default();
        if !needle.is_empty() && !it.name.to_lowercase().contains(&needle) && !plugin_name.contains(&needle) {
            continue;
        }
        if group != Some(it.kind) {
            lines.push(Line::Group(it.kind));
            group = Some(it.kind);
        }
        lines.push(Line::Item(i));
    }
    let folder_h = 20.0;
    if cache.items.is_empty() {
        ui.label(egui::RichText::new("Nothing saved yet. Save… puts the selected track's plug-in chain or strip here.").color(t.text_dim));
    } else if lines.is_empty() {
        ui.label(egui::RichText::new("Nothing in the Library matches").color(t.text_dim));
    }
    let list_h = (ui.available_height() - folder_h - 6.0).max(0.0);
    ui.scope(|ui| {
        // No space between lines: `show_rows` counts the spacing in each row's height.
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().id_salt("browser-library").max_height(list_h).auto_shrink([false, false]).show_rows(
            ui,
            LINE,
            lines.len(),
            |ui, range| {
                for line in lines.iter().skip(range.start).take(range.len()) {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), LINE), egui::Sense::hover());
                    match line {
                        Line::Group(kind) => {
                            ui.painter().text(r.left_center() + vec2(2.0, 1.0), egui::Align2::LEFT_CENTER, kind.heading(), bold(11.0), t.text_dim);
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, kind.heading()));
                        }
                        Line::Item(i) => {
                            let Some(it) = cache.items.get(*i) else { continue };
                            let resp = ui.interact(r, egui::Id::new(("browser-library", *i)), egui::Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(r, egui::CornerRadius::same(3), t.panel_bg2);
                            }
                            let detail = match it.plugin_folder.as_deref().and_then(plugin_of) {
                                Some(p) => p.name.clone(),
                                None => it.detail.clone(),
                            };
                            let right =
                                ui.painter().text(r.right_center() - vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, &detail, regular(10.5), t.text_dim);
                            let name_clip = egui::Rect::from_min_max(r.min, egui::pos2((right.left() - 6.0).max(r.left()), r.bottom()));
                            ui.painter().with_clip_rect(name_clip.intersect(ui.clip_rect())).text(
                                r.left_center() + vec2(12.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                &it.name,
                                regular(11.5),
                                t.text,
                            );
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &it.name));
                            if resp.double_clicked() {
                                ask = Some(Ask::Use(*i));
                            }
                            let resp = resp.on_hover_text(use_tip(it.kind));
                            resp.context_menu(|ui| {
                                if ui.button(use_label(it.kind)).clicked() {
                                    ask = Some(Ask::Use(*i));
                                    ui.close();
                                }
                                if it.kind == LibraryKind::Chain
                                    && ui.button("Add After the Track's Inserts").on_hover_text("Keeps the inserts the track has").clicked()
                                {
                                    ask = Some(Ask::Append(*i));
                                    ui.close();
                                }
                            });
                        }
                    }
                }
            },
        );
    });

    // The folder, and moving it.
    ui.horizontal(|ui| {
        let root = app.dialogs.dirs.root.as_ref().map(|r| r.display().to_string()).unwrap_or_default();
        ui.menu_button("Folder…", |ui| {
            if let Some(pick) = &app.services.pick_folder
                && ui
                    .button("Choose Library Folder…")
                    .on_hover_text("A folder a sync service keeps makes the Library the same on every computer")
                    .clicked()
            {
                if let Some(dir) = pick("library.folder") {
                    ask = Some(Ask::Folder(Some(dir)));
                }
                ui.close();
            }
            if ui.add_enabled(!app.ui.library_dir.trim().is_empty(), egui::Button::new("Use the Settings Folder")).clicked() {
                ask = Some(Ask::Folder(None));
                ui.close();
            }
        })
        .response
        .on_hover_text("Where the Library is kept");
        ui.add(egui::Label::new(egui::RichText::new(&root).color(t.text_dim).font(regular(10.5))).truncate()).on_hover_text(&root);
    });

    let chosen = match &ask {
        Some(Ask::Use(i) | Ask::Append(i)) => cache.items.get(*i).cloned(),
        _ => None,
    };
    app.browser_library = Some(cache);
    match ask {
        Some(Ask::Use(_)) => {
            if let Some(it) = chosen {
                use_item(app, catalog, &it, false);
            }
        }
        Some(Ask::Append(_)) => {
            if let Some(it) = chosen {
                use_item(app, catalog, &it, true);
            }
        }
        Some(Ask::Save(cmd)) => {
            if app.dialogs.open_for_command(&app.engine, cmd) {
                // Read again once the dialog has saved.
                app.browser_library = None;
            } else {
                app.ui.status = "Select a track first".into();
            }
        }
        Some(Ask::Folder(dir)) => {
            let before = app.dialogs.dirs.root.clone();
            app.set_library_dir(dir.as_deref());
            let now = app.dialogs.dirs.root.clone();
            if now != before {
                let was = before.map(|b| b.display().to_string()).unwrap_or_default();
                app.ui.status =
                    format!("The Library is now {}. What was saved before stays in {was}", now.map(|n| n.display().to_string()).unwrap_or_default());
            }
        }
        None => {}
    }
}

fn use_label(kind: LibraryKind) -> &'static str {
    match kind {
        LibraryKind::Chain => "Put on the Selected Tracks",
        LibraryKind::Track => "Load on the Selected Tracks",
        LibraryKind::Effect => "Insert on the Selected Track",
        LibraryKind::Juke => "Load into the Selected Juke Track",
        LibraryKind::Template => "New Session from This Template…",
    }
}

fn use_tip(kind: LibraryKind) -> &'static str {
    match kind {
        LibraryKind::Chain => "Double-click: the chain replaces the selected tracks' inserts",
        LibraryKind::Track => "Double-click: the selected tracks take this strip's level, inserts, sends and instrument",
        LibraryKind::Effect => "Double-click: the plugin goes on the selected track with this preset",
        LibraryKind::Juke => "Double-click: the selected Juke track plays this kit",
        LibraryKind::Template => "Double-click: a new session from this template",
    }
}

/// Uses a Library item on the selection (`append`: a chain after the inserts already there).
pub(crate) fn use_item(app: &mut SoundApp, catalog: &Catalog, it: &LibraryItem, append: bool) {
    let path = it.path.to_string_lossy().into_owned();
    let tracks: Vec<u64> = app.engine.session().edit.selected_tracks.iter().map(|t| t.0).collect();
    let need_track = |app: &mut SoundApp| {
        app.ui.status = "Select a track first".into();
    };
    match it.kind {
        LibraryKind::Chain | LibraryKind::Track => {
            if tracks.is_empty() {
                return need_track(app);
            }
            let (cmd, what) =
                if it.kind == LibraryKind::Chain { ("track.load_chain", "plug-in chain") } else { ("track.load_preset", "track preset") };
            app.ui.status = match app.run(cmd, json!({"path": path, "tracks": tracks, "append": append})) {
                Ok(r) => match r.get("missing_samples").and_then(serde_json::Value::as_u64).unwrap_or(0) {
                    0 => format!("Loaded {what} {}", it.name),
                    n => format!("Loaded {what} {}, without {n} of its samples (their files are not beside it)", it.name),
                },
                Err(e) => e,
            };
        }
        LibraryKind::Effect => {
            let found = it.plugin_folder.as_deref().and_then(|f| catalog.items.iter().find(|i| crate::preset_folder_name(&i.id) == f));
            match found {
                Some(plugin) => super::drop::place_on_selected_with(app, plugin, Some(PresetRef::User { path: it.path.clone() })),
                None => app.ui.status = format!("{}: its plugin is not installed", it.name),
            }
        }
        LibraryKind::Juke => {
            let s = app.engine.session();
            let juke = s
                .edit
                .selected_tracks
                .iter()
                .find(|t| s.track(**t).and_then(|tr| tr.instrument.as_ref()).is_some_and(|i| i.plugin == soundcraft_engine::cmd::juke::JUKE))
                .copied();
            let Some(t) = juke else {
                app.ui.status = "Select a Juke track first (Track › New Beat makes one)".into();
                return;
            };
            #[cfg(not(target_arch = "wasm32"))]
            let kits = app.juke_kits.dir().to_string_lossy().into_owned();
            #[cfg(target_arch = "wasm32")]
            let kits = String::new();
            app.ui.status = match app.run("mix.kit_preset", json!({"track": t.0, "path": path, "kits_dir": kits})) {
                Ok(_) => format!("Loaded Juke preset {}", it.name),
                Err(e) => e,
            };
        }
        LibraryKind::Template => {
            let mut n = crate::dialogs::NewSession::new(&app.dialogs.dirs, true);
            n.start = crate::dialogs::Start::Template(it.path.clone());
            app.dialogs.open = Some(crate::dialogs::Dialog::NewSession(n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fd-library-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_library_lists_every_kind_from_its_folders() {
        let root = scratch("scan");
        let dirs = UserDirs::under(&root);
        for (dir, file) in [
            ("Chains", "Vocal chain.json"),
            ("Track Presets", "Lead Vox.json"),
            ("Presets/compressor", "Glue.json"),
            ("Presets/.favourites", "compressor.json"),
            ("Templates", "Band.fdawm"),
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(file), "{}").unwrap();
        }
        // What is not a preset is not listed: a samples folder, a stray file.
        std::fs::create_dir_all(root.join("Chains/Vocal chain_samples")).unwrap();
        std::fs::write(root.join("Chains/notes.txt"), "x").unwrap();
        let juke = vec![("Dusty Kit".to_string(), root.join("Juke Presets/dusty_kit.json"))];
        let got: Vec<(LibraryKind, String)> = scan(&dirs, juke).into_iter().map(|i| (i.kind, i.name)).collect();
        assert_eq!(
            got,
            [
                (LibraryKind::Chain, "Vocal chain".to_string()),
                (LibraryKind::Track, "Lead Vox".to_string()),
                (LibraryKind::Effect, "Glue".to_string()),
                (LibraryKind::Juke, "Dusty Kit".to_string()),
                (LibraryKind::Template, "Band".to_string()),
            ]
        );
        assert!(scan(&UserDirs::default(), Vec::new()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_library_folder_moves_every_place_presets_are_kept() {
        let (a, b) = (scratch("root-a"), scratch("root-b"));
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        // No folder at all (the web, a test): what was set by hand stays.
        app.preset_dir = Some(a.join("mine"));
        app.apply_library_dir();
        assert_eq!(app.preset_dir, Some(a.join("mine")));
        app.library_default = Some(a.clone());
        app.apply_library_dir();
        assert_eq!(app.preset_dir, Some(a.join("Presets")));
        assert_eq!(app.dialogs.dirs.chains, Some(a.join("Chains")));
        app.set_library_dir(Some(&b.to_string_lossy()));
        assert_eq!(app.dialogs.dirs.track_presets, Some(b.join("Track Presets")));
        assert_eq!(app.dialogs.dirs.templates, Some(b.join("Templates")));
        assert_eq!(app.juke_kits.presets_dir(), b.join("Juke Presets"));
        app.set_library_dir(None);
        assert_eq!(app.dialogs.dirs.root, Some(a.clone()));
        for d in [a, b] {
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}
