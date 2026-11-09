//! The menu bar, laid out by `menubar.txt` (headings, submenus, "coming soon" items). Its leaves
//! run through the incumbent's catalog paths (`crates/engine/catalog/menus.txt`, the parity list)
//! or name a command id directly; catalog entries the bar doesn't show still count for parity.

use crate::{MainWindow, SoundApp};
use serde_json::{Value, json};
use soundcraft_engine::catalog;

/// UI-layer commands: (id, label, catalog path or "", shortcut).
pub const UI_COMMANDS: &[(&str, &str, &str, Option<&str>)] = &[
    ("ui.dock", "Arrange Panel", "", None),
    ("ui.dock.reset", "Reset Panel Layout", "Window > Reset Panel Layout", None),
    ("window.mix", "Mix", "Window > Mix", Some("Cmd+=")),
    ("window.edit", "Edit", "Window > Edit", Some("Cmd+=")),
    ("window.toggle_mix_edit", "Toggle Mix/Edit", "", Some("Cmd+=")),
    ("window.mix_separate", "Mix in Separate Window", "", None),
    ("window.vocal_separate", "Vocal Editor in Separate Window", "", None),
    ("window.midi_editor_separate", "MIDI Editor in Separate Window", "", None),
    // Window › Sync: what the Edit and Mix windows share (`window_sync`).
    ("window.sync_selection", "Sync Selection", "", None),
    ("window.sync_scroll", "Scroll to Selected Track", "", None),
    ("window.sync_folders", "Follow Folder Collapse", "", None),
    ("window.sync_visibility", "Sync Track Visibility", "", None),
    ("window.transport", "Transport", "Window > Transport", Some("Cmd+1")),
    ("window.big_counter", "Big Counter", "Window > Big Counter", Some("Cmd+3")),
    ("window.memory_locations", "Memory Locations", "Window > Memory Locations", Some("Cmd+5")),
    ("window.undo_history", "Undo History", "Window > Undo History", None),
    ("window.notes", "Notes", "", None),
    ("window.lyrics", "Lyrics", "", None),
    ("window.teleprompter", "Teleprompter", "", None),
    ("window.teleprompter_separate", "Teleprompter in Separate Window", "", None),
    // B (Commands Focus Separate), Cmd+B, Cmd+Shift+B and Cmd+Alt+B are all taken.
    ("window.plugin_browser", "Plugin Browser", "Window > Plugin Browser", Some("Alt+B")),
    ("window.clip_list", "Clip List", "Window > Clip List", None),
    ("window.track_list", "Track List", "View > Other Displays > Track List", None),
    ("window.clip_list_view", "Clip List", "View > Other Displays > Clip List", None),
    ("window.narrow_mix", "Narrow Mix", "View > Narrow Mix", Some("Cmd+Alt+M")),
    ("window.session_info", "Session Info", "", None),
    ("window.audio_health", "Session Audio Health", "", None),
    ("window.about", "About FreeDAWm", "", None),
    ("window.close", "Close Window", "Window > Close Window", Some("Cmd+W")),
    ("window.hide_floating", "Hide All Floating Windows", "Window > Hide All Floating Windows", Some("Cmd+Ctrl+W")),
    ("window.midi_editor", "MIDI Editor", "Window > MIDI Editor", None),
    ("window.vocal_editor", "Vocal Editor", "Window > Vocal Editor", None),
    ("window.vocal_studio", "Vocal Studio", "", None),
    ("window.universe", "Universe", "View > Other Displays > Universe", Some("Alt+7")),
    ("window.video", "Video", "Window > Video", Some("Cmd+9")),
    ("window.video_universe", "Video Universe", "Window > Video Universe", None),
    ("window.automation", "Automation", "Window > Automation", Some("Cmd+4")),
    ("window.color_palette", "Color Palette", "Window > Color Palette", None),
    ("window.disk_usage", "Disk Usage", "Window > Disk Usage", None),
    ("window.system_usage", "System Usage", "Window > System Usage", None),
    ("window.task_manager", "Task Manager", "Window > Task Manager", Some("Alt+'")),
    ("window.metadata", "Metadata Inspector", "Window > Metadata Inspector", None),
    ("window.event_list", "MIDI Event List", "Window > MIDI Event List", Some("Alt+=")),
    ("window.midi_keyboard", "Musical Typing", "Window > Musical Typing", Some("Shift+K")),
    // Ctrl+Shift+K is Export Clips as Files (Cmd+Shift+K) off macOS, so computer keys take Cmd+Alt+K.
    ("ui.musical_typing", "Computer Keys Play Notes", "", Some("Cmd+Alt+K")),
    ("window.workspace", "Workspace", "Window > New Workspace > Default", Some("Alt+I")),
    ("window.workspace_presets", "Workspace", "Window > New Workspace > Track Presets", None),
    ("window.workspace_sounds", "Workspace", "Window > New Workspace > Soundbase", None),
    ("window.workspace_front", "Workspaces to Front", "Window > Workspaces > Bring to Front", Some("Alt+J")),
    ("window.workspace_close", "Close All Workspaces", "Window > Workspaces > Close All Workspaces", None),
    ("window.configurations", "Window Configuration List", "Window > Configurations > Window Configuration List", Some("Cmd+Alt+J")),
    ("window.config_new", "New Configuration", "Window > Configurations > New Configuration...", None),
    ("window.config_update", "Update Active Configuration", "Window > Configurations > Update Active Configuration", None),
    ("window.config_auto_update", "Auto-Update Active Configuration", "Window > Configurations > Auto-Update Active Configuration", None),
    ("window.renderer", "Renderer", "Window > Renderer", None),
    ("window.ui_customization", "UI Customization", "Window > UI Customization", None),
    ("window.arrange_tile", "Tile", "Window > Arrange > Tile", None),
    ("window.arrange_tile_h", "Tile Horizontal", "Window > Arrange > Tile Horizontal", None),
    ("window.arrange_tile_v", "Tile Vertical", "Window > Arrange > Tile Vertical", None),
    ("window.arrange_cascade", "Cascade", "Window > Arrange > Cascade", None),
    ("window.midi_front", "MIDI Editors to Front", "Window > MIDI Editors > Bring to Front", None),
    ("window.midi_back", "MIDI Editors to Back", "Window > MIDI Editors > Send to Back", None),
    ("window.clip_effects", "Clip Effects", "Window > Clip Effects", None),
    ("window.clip_effects_dock", "Clip Effects", "View > Other Displays > Lower Dock > Clip Effects", Some("Alt+6")),
    ("window.search", "Search", "Window > Pro Tools Search", Some("Cmd+Ctrl+S")),
    ("window.score", "Score Editor", "Window > Score Editor", Some("Cmd+Ctrl+=")),
    ("window.scout", "Scout", "Window > Scout", None),
    ("window.studio", "Studio", "Window > Studio", None),
    ("ui.scout.copy", "Copy Scout Prompt + Package", "", None),
    ("ui.scout.package", "Scout Package", "", None),
    ("ui.scout.send", "Ask Claude (Scout)", "", None),
    ("ui.scout.starting_prompt_copy", "Copy Starting Prompt", "", None),
    ("ui.scout.starting_prompt_save", "Save Starting Prompt…", "", None),
    ("ui.scout.import_reference", "Import Reference…", "File > Import > Reference...", None),
    ("ui.scout.profile_import", "Import Sound Profile…", "", None),
    ("ui.scout.profile_export", "Export Sound Profile…", "", None),
    ("ui.scout.capture_stems", "Capture Profiles per Stem", "", None),
    ("window.beat_detective", "Beat Detective", "", None),
    ("window.tempo_ops", "Tempo Operations", "", None),
    ("window.time_ops", "Time Operations", "", None),
    ("window.midi_ops", "MIDI Operations", "", None),
    ("window.rtp", "MIDI Real-Time Properties", "", None),
    ("window.playback_engine", "Playback Engine", "", None),
    ("ui.audio_device", "Audio Device", "", None),
    ("setup.record_offset", "Record Offset", "", None),
    ("setup.measure_latency", "Measure Round Trip", "", None),
    ("setup.measure_cancel", "Cancel Measure", "", None),
    ("ui.reset_underruns", "Reset Underruns", "", None),
    ("window.io_setup", "I/O Setup", "", None),
    ("window.midi_inputs", "MIDI Input Devices", "Setup > MIDI > MIDI Input Devices...", None),
    ("window.network", "Network", "Setup > Network...", None),
    ("net.remote_mix", "Mix Another PC Remotely", "", None),
    ("window.x32", "X32 Control", "Setup > X32 Control...", None),
    ("setup.x32", "X32 Control Settings", "", None),
    ("window.shortcuts", "Keyboard Shortcuts", "", None),
    ("window.preferences", "Preferences", "", None),
    ("ui.meter_ballistics", "Meter Ballistics", "", None),
    ("ui.copy_imported_audio", "Copy Imported Audio into the Project", "", None),
    ("view.mix_section", "Mix Window View", "", None),
    ("view.mix_slim", "Slim Strips", "View > Mix Window Views > Slim Strips", None),
    ("ui.new_tracks_dialog", "New Tracks…", "", None),
    ("ui.bounce_dialog", "Bounce Mix…", "", None),
    ("ui.new_from_template", "New from Template…", "", None),
    ("ui.set", "Set UI State", "", None),
    ("ui.shortcuts.get", "Get Keyboard Shortcuts", "", None),
    ("ui.shortcuts.set", "Set Keyboard Shortcut", "", None),
    ("ui.shortcuts.reset", "Reset Keyboard Shortcuts", "", None),
    ("ui.theme", "Appearance", "", None),
    ("ui.mix.drawer", "Open Module Drawer", "", None),
    ("ui.mix.full_editor", "Open Full Plugin Editor", "", None),
    ("ui.mix.instrument_window", "Open Instrument Window", "", None),
    ("ui.mix.ceiling_learn", "Learn CleanQ Ceiling", "", None),
    ("ui.mix.wide_editors", "Plugin Editors as in the VST", "View > Mix Window Views > Plugin Editors as in the VST", None),
    ("ui.mix.slim_editors", "Slim Plugin Editors", "View > Mix Window Views > Slim Plugin Editors", None),
    // Channel width for the Mix window's selected channels, or all (`channel_width`).
    ("ui.mix.width.slim", "Slim Selected Channels", "", None),
    ("ui.mix.width.wide", "Wide Selected Channels", "", None),
    ("ui.mix.width.auto", "Auto Width Selected Channels", "", None),
    ("ui.mix.width.slim_all", "Slim All Channels", "", None),
    ("ui.mix.width.wide_all", "Wide All Channels", "", None),
    ("ui.mix.width.auto_all", "Auto Width All Channels", "", None),
    ("presets.import_firestation", "Import FireStation User Presets", "File > Import > FireStation User Presets", None),
    ("ui.ml.separate", "Separate Stems…", "AI > Separate Stems…", None),
    ("ui.ml.karaoke", "Make Karaoke…", "AI > Make Karaoke…", None),
    ("ui.ml.drums", "Split Drums…", "AI > Split Drums…", None),
    ("ui.ml.clean", "Clean Vocal…", "AI > Clean Vocal…", None),
    ("ui.vocal_editor", "Tune Vocal…", "AI > Tune Vocal…", None),
    ("ui.vocal_panel", "Tuning Panel", "", None),
    ("ui.vocal_studio", "Vocal Studio View", "", None),
    ("ui.vocal_harmony", "Make Harmony…", "", None),
    ("ui.vocal_preset_save", "Save Tuning Macro…", "", None),
    ("ui.ml.midi", "Convert to MIDI…", "AI > Convert to MIDI…", None),
    ("ui.ml.chords", "Detect Chords…", "AI > Detect Chords…", None),
    ("ui.ml.lyrics", "Transcribe Lyrics…", "AI > Transcribe Lyrics…", None),
    ("ui.midi_cleanup", "MIDI Cleanup…", "AI > MIDI Cleanup…", None),
    ("ui.midi_editor", "MIDI Editor State", "", None),
    ("window.ml_models", "ML Models", "Setup > ML Models…", None),
];

/// Extra catalog mappings handled by the UI layer.
pub fn ui_aliases() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<(&str, &str)> = UI_COMMANDS.iter().filter(|c| !c.2.is_empty()).map(|c| (c.2, c.0)).collect();
    v.extend(mix_view_aliases().iter().copied());
    v.extend(AUDIOSUITE.iter().map(|(p, _)| (*p, "process.run")));
    v.extend(MENU_WINDOWS.iter().copied());
    v.extend(BAND_RULERS.iter().chain(&PLAIN_RULERS).map(|p| (*p, "view.ruler")));
    // Menu-bar leaves that run a command id directly (`= @id` in menubar.txt).
    v.extend(layout().iter().filter_map(|l| match l.target {
        Target::Id(id) => Some((l.display, id)),
        _ => None,
    }));
    v
}

/// The Band's lanes in View › Rulers, right after Key Signature (not in the incumbent's catalog).
/// The chord lane's ruler is labelled "Chords" (its id stays `band_chords`).
const BAND_RULERS: [&str; 3] = ["View > Rulers > Sections", "View > Rulers > Chords", "View > Rulers > Lyrics"];

/// Tempo and Key Signature toggle their rulers like the others; their editor flags sit right after them.
const PLAIN_RULERS: [&str; 2] = ["View > Rulers > Tempo", "View > Rulers > Key Signature"];

/// Mix-view menu paths, allocated once. Rebuilding them on every menu draw leaked a copy per frame.
fn mix_view_aliases() -> &'static [(&'static str, &'static str)] {
    static ALIASES: std::sync::OnceLock<Vec<(&'static str, &'static str)>> = std::sync::OnceLock::new();
    ALIASES.get_or_init(|| {
        [
            "Inserts A-E",
            "Inserts F-J",
            "Sends A-E",
            "Sends F-J",
            "I/O",
            "Track Color",
            "Comments",
            "EQ Curve",
            "Meters and Faders",
            "All",
            "Minimal",
            "Mic Preamps",
            "Instruments",
            "Object",
            "Pitch",
        ]
        .into_iter()
        .map(|sec| {
            let path: &'static str = Box::leak(format!("View > Mix Window Views > {sec}").into_boxed_str());
            (path, "view.mix_section")
        })
        .collect()
    })
}

/// The incumbent's offline-processing catalog entries (its "AudioSuite" menu) → our processes
/// (functional equivalents, our own plugins), shown in our Process menu.
pub const AUDIOSUITE: &[(&str, &str)] = &[
    ("AudioSuite > EQ > Channel Strip", "channel_strip"),
    ("AudioSuite > EQ > EQ3 1-Band", "eq_1band"),
    ("AudioSuite > EQ > EQ3 7-Band", "eq_7band"),
    ("AudioSuite > Dynamics > BF-76", "compressor"),
    ("AudioSuite > Dynamics > Channel Strip", "channel_strip"),
    ("AudioSuite > Dynamics > Dyn3 Compressor/Limiter", "compressor"),
    ("AudioSuite > Dynamics > Dyn3 De-Esser", "de_esser"),
    ("AudioSuite > Dynamics > Dyn3 Expander/Gate", "expander_gate"),
    ("AudioSuite > Dynamics > Maxim", "maximizer"),
    ("AudioSuite > Pitch Shift > Pitch II", "pitch_shift"),
    ("AudioSuite > Pitch Shift > Pitch Shift Legacy", "pitch_shift"),
    ("AudioSuite > Pitch Shift > Time Shift", "time_stretch"),
    ("AudioSuite > Pitch Shift > Vari-Fi", "varispeed"),
    ("AudioSuite > Reverb > D-Verb", "room_reverb"),
    ("AudioSuite > Delay > Mod Delay III", "mod_delay"),
    ("AudioSuite > Modulation > Sci-Fi", "flanger"),
    ("AudioSuite > Harmonic > Eleven Lite", "saturator"),
    ("AudioSuite > Harmonic > Lo-Fi", "lofi"),
    ("AudioSuite > Harmonic > Recti-Fi", "rectifier"),
    ("AudioSuite > Harmonic > SansAmp PSA-1", "saturator"),
    ("AudioSuite > Other > DC Offset Removal", "dc_offset_removal"),
    ("AudioSuite > Other > Duplicate", "duplicate"),
    ("AudioSuite > Other > Gain", "gain"),
    ("AudioSuite > Other > Invert", "invert"),
    ("AudioSuite > Other > Normalize", "normalize"),
    ("AudioSuite > Other > Reverse", "reverse"),
    ("AudioSuite > Other > Signal Generator", "signal_generator"),
    ("AudioSuite > Other > Time Compression Expansion", "time_stretch"),
];

/// Commands that open a dialog when invoked from a menu (programmatic calls never do).
const DIALOG_COMMANDS: &[&str] = &[
    "clip.clear",
    "track.new",
    "file.bounce_mix",
    "session.new",
    "ui.new_from_template",
    "file.import_audio",
    "file.import_midi",
    "session.open",
    "session.save",
    "session.save_as",
    "session.save_copy",
    "track.group",
    "edit.repeat",
    "edit.shift",
    "edit.fades_create",
    "setup.session",
    "event.quantize",
    "event.transpose",
    "event.change_velocity",
    "edit.strip_silence",
    "clip.rename",
    "track.rename",
    "clip.clean_mix",
];

/// The menu bar as shown: see the format at the top of `menubar.txt`.
const MENUBAR: &str = include_str!("menubar.txt");

/// The hover text of a greyed-out "coming soon" item.
pub const SOON: &str = "Coming soon: needs its own window";

/// What a menu-bar line runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A catalog or UI path (resolved like the catalog's own entries).
    Path(&'static str),
    /// A command id.
    Id(&'static str),
    /// Greyed out, with this hover text.
    Soon(&'static str),
    /// Built by the UI: `instrument` (Track › Instrument) or `recent` (File › Open › Open Recent).
    Special(&'static str),
}

/// One line of `menubar.txt`.
#[derive(Debug, Clone, Copy)]
pub struct Line {
    /// Where it is shown, `Menu > Submenu > Item`.
    pub display: &'static str,
    pub target: Target,
    /// Left out of the web build.
    pub desktop: bool,
}

fn parse_line(raw: &'static str) -> Option<Line> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with("//") {
        return None;
    }
    let (line, desktop) = match line.strip_suffix("[desktop]") {
        Some(l) => (l.trim_end(), true),
        None => (line, false),
    };
    let (display, target) = match line.split_once(" = ") {
        Some((d, t)) => (d.trim(), t.trim()),
        None => (line, ""),
    };
    let target = if target.is_empty() {
        Target::Path(display)
    } else if let Some(id) = target.strip_prefix('@') {
        Target::Id(id)
    } else if let Some(name) = target.strip_prefix('!') {
        Target::Special(name)
    } else if target == "soon" {
        Target::Soon(SOON)
    } else if let Some(text) = target.strip_prefix("soon:") {
        Target::Soon(text.trim())
    } else {
        Target::Path(target)
    };
    Some(Line { display, target, desktop })
}

/// Every line of `menubar.txt`, parsed once.
pub fn layout() -> &'static [Line] {
    static LINES: std::sync::OnceLock<Vec<Line>> = std::sync::OnceLock::new();
    LINES.get_or_init(|| MENUBAR.lines().filter_map(parse_line).collect())
}

/// The catalog or UI path a shown menu path runs through (`Window > More > Search` →
/// `Window > Pro Tools Search`); any other path is returned as it is.
pub fn source_path(path: &str) -> &str {
    layout()
        .iter()
        .find(|l| l.display == path)
        .and_then(|l| match l.target {
            Target::Path(p) => Some(p),
            _ => None,
        })
        .unwrap_or(path)
}

/// Why the menu item at `path` is greyed out as "coming soon", if it is: `path` is its shown path,
/// or a catalog path in the same menu with the same label (Event › MIDI Operations › Change
/// Velocity… is shown as Event › More MIDI › Change Velocity…).
pub fn soon_reason(path: &str) -> Option<&'static str> {
    fn ends(p: &str) -> (Option<&str>, Option<&str>) {
        (p.split(" > ").next(), p.rsplit(" > ").next())
    }
    layout().iter().find_map(|l| match l.target {
        Target::Soon(text) if l.display == path || ends(l.display) == ends(path) => Some(text),
        _ => None,
    })
}

/// Where a command sits in the menu bar, as `Menu › Submenu` (the first place it is shown).
pub fn menu_location(id: &str) -> Option<String> {
    static PLACES: std::sync::OnceLock<std::collections::HashMap<String, String>> = std::sync::OnceLock::new();
    let places = PLACES.get_or_init(|| {
        let extra = ui_aliases();
        let mut v: Vec<(String, String)> = Vec::new();
        for n in tree() {
            collect_places(&n, &extra, &mut v);
        }
        // The first place a command is shown wins.
        let mut map = std::collections::HashMap::new();
        for (c, p) in v {
            map.entry(c).or_insert(p);
        }
        map
    });
    places.get(id).cloned()
}

fn collect_places(n: &MenuNode, extra: &[(&str, &str)], out: &mut Vec<(String, String)>) {
    for c in &n.children {
        if !c.children.is_empty() {
            collect_places(c, extra, out);
        } else if c.kind == MenuKind::Item
            && let Some(id) = command_for_path(&c.path, extra)
        {
            out.push((id, n.path.replace(" > ", " › ")));
        }
    }
}

/// What a menu node is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuKind {
    /// A command (a leaf) or a submenu (with children).
    Item,
    /// A small group heading.
    Heading,
    Separator,
    /// Shown greyed out with this hover text.
    Soon(&'static str),
    /// Built by the UI (see [`Target::Special`]).
    Special(&'static str),
}

/// Shared menu hierarchy for egui and native desktop hosts. A leaf's `path` is the catalog or UI
/// path it runs through (its place in the tree is where it is shown); a submenu's is its shown path.
pub struct MenuNode {
    pub label: String,
    pub path: String,
    pub kind: MenuKind,
    pub children: Vec<MenuNode>,
}

fn insert_line(roots: &mut Vec<MenuNode>, line: &Line) {
    let parts: Vec<&str> = line.display.split(" > ").collect();
    let Some((leaf, dirs)) = parts.split_last() else { return };
    let mut level = roots;
    let mut shown = String::new();
    for d in dirs {
        if !shown.is_empty() {
            shown.push_str(" > ");
        }
        shown.push_str(d);
        let idx = match level.iter().position(|n| n.label == *d && n.kind == MenuKind::Item && !n.children.is_empty()) {
            Some(i) => i,
            None => {
                level.push(MenuNode { label: d.to_string(), path: shown.clone(), kind: MenuKind::Item, children: Vec::new() });
                level.len() - 1
            }
        };
        let Some(node) = level.get_mut(idx) else { return };
        level = &mut node.children;
    }
    let (label, kind) = if *leaf == "---" {
        (String::new(), MenuKind::Separator)
    } else if let Some(h) = leaf.strip_prefix("## ") {
        (h.to_string(), MenuKind::Heading)
    } else {
        let kind = match line.target {
            Target::Soon(text) => MenuKind::Soon(text),
            Target::Special(name) => MenuKind::Special(name),
            Target::Path(_) | Target::Id(_) => MenuKind::Item,
        };
        (leaf.to_string(), kind)
    };
    let path = match line.target {
        Target::Path(p) if kind == MenuKind::Item => p.to_string(),
        _ => line.display.to_string(),
    };
    level.push(MenuNode { label, path, kind, children: Vec::new() });
}

/// The menu bar after the FreeDAWm menu, from `menubar.txt`.
pub fn tree() -> Vec<MenuNode> {
    let mut roots: Vec<MenuNode> = Vec::new();
    for line in layout() {
        if line.desktop && cfg!(target_arch = "wasm32") {
            continue;
        }
        insert_line(&mut roots, line);
    }
    roots
}

/// Labels ending in "..." or "…" open a dialog or a window: they never carry a check mark.
pub fn opens_window(label: &str) -> bool {
    label.ends_with("...") || label.ends_with('…')
}

/// The menu bar wordmark's height as a share of the menu text's height.
pub const WORDMARK_SCALE: f32 = 0.8;

/// The menu bar wordmark's size for menu text `text_height` points tall and the art's aspect
/// (width / height).
pub fn wordmark_size(text_height: f32, aspect: f32) -> egui::Vec2 {
    let h = text_height * WORDMARK_SCALE;
    egui::vec2(h * aspect, h)
}

pub fn menu_bar(app: &mut SoundApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::current();
    egui::Panel::top("menu_bar").exact_size(24.0).frame(egui::Frame::NONE.fill(t.menu_bar).inner_margin(egui::Margin::symmetric(8, 2))).show(
        ui,
        |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // The first item is the wordmark at 80 % of the menu text's height, centred on the
                // text line (plain text if the art can't be loaded); its accessible name stays
                // "FreeDAWm".
                let brand = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| match crate::brand_art::BrandArt::get(ui.ctx()) {
                    Some(art) => {
                        let size = wordmark_size(ui.text_style_height(&egui::TextStyle::Button), art.wordmark_aspect);
                        let img =
                            egui::Image::from_texture(egui::load::SizedTexture::new(art.wordmark().id(), size)).alt_text(crate::brand_art::APP_NAME);
                        let r = egui::containers::menu::MenuButton::from_button(egui::Button::image(img)).ui(ui, |ui| add(ui));
                        r.0.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::brand_art::APP_NAME));
                    }
                    None => {
                        ui.menu_button(crate::brand_art::APP_NAME, |ui| add(ui));
                    }
                };
                brand(ui, &mut |ui| {
                    if ui.button("About FreeDAWm").clicked() {
                        app.ui.show_about = true;
                    }
                    if ui.button("Session Info").clicked() {
                        app.ui.show_session_info = true;
                    }
                    if ui.button("Session Audio Health").clicked() {
                        let _ = app.run("window.audio_health", json!({"value": true}));
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        let _ = crate::dialogs::run_or_ask(app, "app.quit", json!({}));
                    }
                });
                let extra = ui_aliases();
                for root in tree() {
                    ui.menu_button(&root.label, |ui| {
                        menu_items(app, ui, &root.children, &extra);
                        if root.label == "Process" {
                            ui.separator();
                            ui.label(egui::RichText::new("Processes selected clips offline").small().color(t.text_dim));
                        }
                    });
                }
                let title = format!(
                    "{}{}  —  {}",
                    app.engine.session().name,
                    if app.engine.is_dirty() { " *" } else { "" },
                    if app.ui.window == MainWindow::Edit || app.ui.mix_separate { "Edit" } else { "Mix" }
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(title).color(t.text_dim));
                });
            });
        },
    );
}

/// Track › Instrument: the instrument picker for the selected instrument track (#374).
fn instrument_menu(app: &mut SoundApp, ui: &mut egui::Ui) {
    match instrument_track(app.engine.session()) {
        Some(id) => {
            ui.menu_button("Instrument", |ui| crate::mix_window::instrument_menu(app, ui, id));
        }
        None => {
            ui.add_enabled(false, egui::Button::new("Instrument")).on_disabled_hover_text("Select an instrument track");
        }
    }
}

/// The first selected instrument track, in track order.
fn instrument_track(s: &soundcraft_model::Session) -> Option<soundcraft_model::TrackId> {
    s.tracks.iter().find(|t| t.kind == soundcraft_model::TrackKind::Instrument && s.edit.selected_tracks.contains(&t.id)).map(|t| t.id)
}

/// One menu's items. When any of them can carry a check mark, every item keeps the check-mark
/// column, so checked and unchecked labels line up.
fn menu_items(app: &mut SoundApp, ui: &mut egui::Ui, items: &[MenuNode], extra: &[(&str, &str)]) {
    let reserve = items.iter().any(|c| {
        c.children.is_empty()
            && c.kind == MenuKind::Item
            && !opens_window(&c.label)
            && is_toggle(app, &c.path, command_for_path(&c.path, extra).as_deref())
    });
    for c in items {
        menu_node(app, ui, c, extra, reserve);
    }
}

/// A menu label, with the check-mark column when `reserve` (the mark drawn only when `checked`).
pub(crate) fn tick_label(ui: &egui::Ui, label: &str, checked: bool, reserve: bool) -> egui::WidgetText {
    if !reserve && !checked {
        return label.into();
    }
    if checked {
        return format!("✔ {label}").into();
    }
    // An unchecked item is indented by the mark's width (the text itself stays just the label).
    let font = egui::TextStyle::Button.resolve(ui.style());
    let indent = ui.painter().layout_no_wrap("✔ ".into(), font.clone(), egui::Color32::PLACEHOLDER).size().x;
    let mut job = egui::text::LayoutJob::default();
    // PLACEHOLDER takes the button's own text colour, greyed when it is disabled.
    job.append(label, indent, egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER));
    job.into()
}

fn menu_node(app: &mut SoundApp, ui: &mut egui::Ui, n: &MenuNode, extra: &[(&str, &str)], reserve: bool) {
    if !n.children.is_empty() {
        ui.menu_button(tick_label(ui, &n.label, false, reserve), |ui| {
            menu_items(app, ui, &n.children, extra);
        });
        return;
    }
    match n.kind {
        MenuKind::Item => {}
        MenuKind::Heading => {
            let t = crate::theme::Tokens::current();
            ui.label(egui::RichText::new(n.label.to_uppercase()).small().color(t.text_dim));
            return;
        }
        MenuKind::Separator => {
            ui.separator();
            return;
        }
        MenuKind::Soon(text) => {
            ui.add_enabled(false, egui::Button::new(tick_label(ui, &n.label, false, reserve))).on_disabled_hover_text(text);
            return;
        }
        MenuKind::Special("recent") => {
            crate::recent::menu(app, ui);
            return;
        }
        MenuKind::Special("instrument") => {
            instrument_menu(app, ui);
            return;
        }
        MenuKind::Special(_) => return,
    }
    let id = command_for_path(&n.path, extra);
    let (enabled, checked) = item_state(app, &n.path, id.as_deref());
    let label = live_label(app, id.as_deref()).unwrap_or_else(|| n.label.clone());
    let label = tick_label(ui, &label, checked && !opens_window(&n.label), reserve);
    let mut shortcut = id.as_deref().map(|id| shortcut_text(app, id, ui.ctx().os().is_mac())).unwrap_or_default();
    // Clear is also plain Delete in the Edit window (`shortcuts::delete_clears`).
    if id.as_deref() == Some("edit.clear") {
        shortcut = format!("{shortcut}  Del");
    }
    let btn = egui::Button::new(label).shortcut_text(shortcut);
    let mut resp = ui.add_enabled(enabled, btn);
    if let Some(why) = disabled_reason(app, id.as_deref()) {
        resp = resp.on_disabled_hover_text(why);
    }
    if let Some(tip) = id.as_deref().and_then(hover_text) {
        resp = resp.on_hover_text(tip);
    }
    if resp.clicked()
        && let Some(id) = id
    {
        invoke_menu(app, &id, &n.path);
        ui.close();
    }
}

/// The View › Rulers entries (checkmarks included), as a flat list of menu items: the one
/// source for both the menu bar and the Edit window's ruler context menus.
pub fn rulers_menu_items(app: &mut SoundApp, ui: &mut egui::Ui) {
    let extra = ui_aliases();
    let rulers = tree().into_iter().find(|n| n.label == "View").and_then(|v| v.children.into_iter().find(|n| n.label == "Rulers"));
    if let Some(r) = rulers {
        menu_items(app, ui, &r.children, &extra);
    }
}

/// Top of a ruler's context menu: "Hide This Ruler", when `view.ruler` can toggle that ruler.
pub fn ruler_menu_head(app: &mut SoundApp, ui: &mut egui::Ui, ruler: &str) {
    let shown = app.engine.session().edit.rulers.iter().any(|r| r == ruler);
    if shown && ruler_id_of(ruler).is_some() {
        if ui.button("Hide This Ruler").clicked() {
            let _ = app.run("view.ruler", json!({"ruler": ruler, "visible": false}));
            ui.close();
        }
        ui.separator();
    }
}

fn ruler_id_of(ruler: &str) -> Option<&str> {
    const ALL: [&str; 12] =
        ["bars_beats", "min_secs", "timecode", "feet_frames", "samples", "tempo", "meter", "markers", "key", "sections", "band_chords", "lyrics"];
    ALL.contains(&ruler).then_some(ruler)
}

/// A plain ruler's context menu: Hide This Ruler, then every ruler to toggle.
pub fn ruler_context_menu(app: &mut SoundApp, ui: &mut egui::Ui, ruler: &str) {
    ruler_menu_head(app, ui, ruler);
    rulers_menu_items(app, ui);
}

/// The end of a ruler's own context menu: the lanes list as a "Rulers" submenu.
pub fn rulers_submenu(app: &mut SoundApp, ui: &mut egui::Ui) {
    ui.separator();
    ui.menu_button("Rulers", |ui| rulers_menu_items(app, ui));
}

/// Why a menu item is greyed out: the item's own reason, or the command's.
fn disabled_reason(app: &SoundApp, id: Option<&str>) -> Option<String> {
    let id = id?;
    match id {
        "ui.mix.slim_editors" => Some("Needs Plugin Editors as in the VST".into()),
        i if i.starts_with("ui.mix.width.") => Some("Select channels in the Mix window".into()),
        "track.rename" => Some("Select a track".into()),
        i if i.starts_with("ui.ml.") => Some("Select an audio clip or track".into()),
        i if i.starts_with("clip.clean_") => Some("Select a clip cleaned by Clean Vocal".into()),
        _ => soundcraft_engine::find_command(id).and_then(|c| (c.enabled)(&app.engine).err()),
    }
}

/// Hover text of an enabled item, where it needs one.
fn hover_text(id: &str) -> Option<&'static str> {
    match id {
        "window.mix_separate" => Some("Show Edit and Mix at once, e.g. one per monitor"),
        "window.vocal_separate" => Some("The Vocal Editor in its own window; closing it docks it back"),
        "window.notes" => Some("Session notes: a to-do list with check boxes, tags and times"),
        "window.teleprompter" => Some("Big lyrics that follow the playhead through the lines on the Lyrics ruler"),
        "window.teleprompter_separate" => Some("The Teleprompter in its own window (e.g. on the singer's screen); closing it docks it back"),
        "window.lyrics" => Some("Write the lyrics: line numbers, syllable counts, rhymes; place lines on the Lyrics ruler"),
        "window.midi_editor_separate" => Some("The MIDI Editor in its own window; closing it docks it back"),
        "window.sync_selection" => Some("Selecting a track in either window selects it in both"),
        "window.sync_scroll" => Some("A track selected in one window scrolls into view in the other"),
        "window.sync_folders" => Some("A closed folder hides its tracks' strips in the Mix window"),
        "window.sync_visibility" => Some("A track hidden in the Edit window has no strip in the Mix window"),
        "options.pre_post_roll" => Some("Sets the pre- and post-roll; the shortcut only turns it on or off"),
        _ => None,
    }
}

/// Resolve the command behind a menu leaf, including UI dialog overrides. Takes the path a leaf
/// runs through ([`MenuNode::path`]) or a shown menu path ([`source_path`]).
pub fn command_for_path(path: &str, extra: &[(&str, &str)]) -> Option<String> {
    let path = source_path(path);
    catalog::implemented_by(path, extra).or_else(|| MENU_WINDOWS.iter().find(|(p, _)| *p == path).map(|(_, w)| w.to_string()))
}

/// Whether a clip the clip commands would act on carries Clean Vocal's cleaned alternate.
fn cleaned_clip_selected(app: &SoundApp) -> bool {
    let s = app.engine.session();
    soundcraft_engine::cmd::clip_ids_param(&app.engine, &json!({})).iter().any(|id| s.find_clip(*id).is_some_and(|(_, c)| c.clean_alt().is_some()))
}

/// Enabled and checked state shared by all menu presentations.
pub fn item_state(app: &SoundApp, path: &str, id: Option<&str>) -> (bool, bool) {
    let enabled = match id {
        Some(i)
            if i.starts_with("window.")
                || i.starts_with("view.mix_section")
                || i == "view.mix_slim"
                || matches!(i, "process.run" | "ui.dock" | "ui.dock.reset" | "presets.import_firestation" | "ui.mix.wide_editors") =>
        {
            true
        }
        // The ML jobs need an audio track or clip to run on.
        Some(i) if i.starts_with("ui.ml.") => !app.engine.session().edit.selected_clips.is_empty() || !app.engine.target_tracks().is_empty(),
        // The slim layout is a layout of the VST-faithful editors: nothing to do without them.
        Some("ui.mix.slim_editors") => app.ui.wide_editors,
        Some(i) if i.starts_with("ui.mix.width.") => crate::channel_width::enabled(app, i),
        // Rename… opens its dialog on the first selected track.
        Some("track.rename") => !app.engine.target_tracks().is_empty(),
        // Clean Vocal's follow-ups act on a cleaned clip only.
        Some(i) if i.starts_with("clip.clean_") => cleaned_clip_selected(app),
        Some(i) => soundcraft_engine::find_command(i).is_some_and(|c| (c.enabled)(&app.engine).is_ok() && !app.engine.blocked_while_recording(i)),
        None => false,
    };
    (enabled, checked_state(app, path, id))
}

/// View and option toggles that keep their state as a session flag: command id → flag.
const FLAG_TOGGLES: &[(&str, &str)] = &[
    ("options.low_latency_monitoring", "options.low_latency_monitoring"),
    ("options.keyboard_lock", "options.edit_tool_mode_keyboard_lock"),
    ("view.track_number", "edit_view.track_number"),
    ("view.ruler_tempo_editor", "view.ruler.tempo_editor"),
    ("view.marker_ruler_lines", "view.marker.ruler_lines"),
    ("view.clip_sync_point", "view.clip.sync_point"),
    ("view.clip_name", "view.clip.name"),
    ("view.clip_rating", "view.clip.rating"),
    ("view.clip_overlap_shadows", "view.clip.overlap_shadows"),
    ("view.clip_transparency", "view.clip.transparency"),
    ("view.clip_gain_line", "view.clip.gain_line"),
    ("view.clip_gain_info", "view.clip.gain_info"),
    ("view.clip_effects_status", "view.clip.effects_status"),
    ("view.clip_time_current", "view.clip.time.current"),
    ("view.clip_time_original", "view.clip.time.original"),
    ("view.clip_time_user", "view.clip.time.user"),
    ("view.waveform_power", "waveform.power"),
    ("view.waveform_rectified", "waveform.rectified"),
    ("view.waveform_outlines", "waveform.outlines"),
    ("view.waveform_overlapped_crossfades", "waveform.overlapped_crossfades"),
    ("view.transport_counters", "view.transport.counters"),
    ("view.transport_midi_controls", "view.transport.midi_controls"),
    ("view.transport_output_meters", "view.transport.output_meters"),
    ("view.transport_expanded", "view.transport.expanded"),
];

fn checked_state(app: &SoundApp, path: &str, id: Option<&str>) -> bool {
    toggle_state(app, path, id).unwrap_or(false)
}

/// Whether the item for `id` (run through `path`) is a toggle that can show a check mark, and so
/// takes the menu's check-mark column.
pub fn is_toggle(app: &SoundApp, path: &str, id: Option<&str>) -> bool {
    toggle_state(app, path, id).is_some()
}

/// A toggle's state; None for items that are not toggles.
fn toggle_state(app: &SoundApp, path: &str, id: Option<&str>) -> Option<bool> {
    let e = &app.engine.session().edit;
    let leaf = path.rsplit(" > ").next().unwrap_or("");
    if let Some((_, flag)) = id.and_then(|i| FLAG_TOGGLES.iter().find(|(c, _)| *c == i)) {
        return Some(e.flag(flag));
    }
    Some(match id {
        Some("options.loop_playback") => e.loop_playback,
        Some("options.loop_record") => e.loop_record,
        Some("options.pre_post_roll") => e.pre_post_roll,
        Some("options.link_timeline_edit") => e.link_timeline_edit,
        Some("options.insertion_follows_playback") => e.insertion_follows_playback,
        Some("options.tab_to_transient") => e.tab_to_transient,
        Some("options.automation_follows_edit") => e.automation_follows_edit,
        Some("options.click") => e.click,
        Some("options.delay_compensation") => e.delay_compensation,
        // `video.online` keeps the opposite flag (the engine's `OFFLINE_FLAG`).
        Some("video.online") => !e.flag("video.offline"),
        Some("options.scrolling") => e.scrolling == scrolling_mode(leaf),
        // Only Solo in Place and X-OR are offered: X-OR is on or it isn't.
        Some("options.solo_mode") => e.solo_mode.contains("xor") == leaf.starts_with("X-OR"),
        Some(i) if i.starts_with("view.edit_") && !matches!(i, "view.edit_all" | "view.edit_minimal") => {
            e.flag("edit_view.all") || i.strip_prefix("view.edit_").is_some_and(|v| e.flag(&format!("edit_view.{v}")))
        }
        // View › Expanded Sends › Send A..J (All and None are actions).
        Some(i) if i.strip_prefix("view.expanded_send_").is_some_and(|c| c.len() == 1) => {
            i.strip_prefix("view.expanded_send_").is_some_and(|c| e.flag(&format!("expanded_send.{c}")))
        }
        Some("view.clip_time_none") => !["current", "original", "user"].iter().any(|t| e.flag(&format!("view.clip.time.{t}"))),
        Some("view.waveform_peak") => e.flag("waveform.peak") || !e.flag("waveform.power"),
        Some("view.ruler") => !ruler_id(leaf).is_empty() && e.rulers.iter().any(|x| x == ruler_id(leaf)),
        Some("view.ruler_all_markers") => ["markers", "markers2", "markers3", "markers4", "markers5"].iter().all(|m| e.rulers.iter().any(|x| x == m)),
        Some(i @ ("view.ruler_timecode2" | "view.ruler_markers2" | "view.ruler_markers3" | "view.ruler_markers4" | "view.ruler_markers5")) => {
            e.rulers.iter().any(|x| Some(x.as_str()) == i.strip_prefix("view.ruler_"))
        }
        Some("view.main_counter") => e.main_counter.id() == ruler_id(leaf),
        Some("clip.edit_lock") => selected_clips_all(app, |c| c.edit_locked),
        Some("clip.time_lock") => selected_clips_all(app, |c| c.time_locked),
        Some("window.mix") => app.ui.window == MainWindow::Mix,
        Some("window.edit") => app.ui.window == MainWindow::Edit,
        Some("window.mix_separate") => app.ui.mix_separate,
        Some("window.vocal_separate") => app.ui.vocal_separate,
        Some("window.midi_editor_separate") => app.ui.midi_editor_separate,
        Some("ui.musical_typing") => app.typing.capture,
        Some("window.sync_selection") => app.ui.sync.selection,
        Some("window.sync_scroll") => app.ui.sync.scroll,
        Some("window.sync_folders") => app.ui.sync.folders,
        Some("window.sync_visibility") => app.ui.sync.visibility,
        Some("window.track_list") => app.ui.show_tracks_list,
        Some("window.clip_list_view") | Some("window.clip_list") => app.ui.show_clip_list,
        Some("window.narrow_mix") | Some("view.mix_slim") => app.ui.narrow_mix,
        Some("window.transport") => app.ui.show_transport,
        Some("window.big_counter") => app.ui.show_big_counter,
        Some("window.memory_locations") => app.ui.show_memory_locations,
        Some("window.automation") => app.ui.show_automation,
        Some("window.undo_history") => app.ui.show_undo_history,
        Some("window.notes") => app.ui.show_notes,
        Some("window.lyrics") => app.ui.show_lyrics,
        Some("window.teleprompter") => app.ui.show_teleprompter,
        Some("window.teleprompter_separate") => app.ui.teleprompter_separate,
        Some("window.clip_effects") => app.ui.show_clip_effects,
        Some("window.midi_editor") => app.ui.show_midi_editor,
        Some("window.vocal_editor") => app.ui.show_vocal_editor,
        Some("window.vocal_studio") => app.ui.show_vocal_editor && app.vocal.studio.on,
        Some("window.score") => app.ui.show_score,
        Some("window.event_list") => app.ui.show_event_list,
        Some("window.midi_keyboard") => app.ui.show_midi_keyboard,
        Some("window.workspace") => app.ui.show_workspace,
        Some("window.preferences") => app.ui.show_preferences,
        Some("window.configurations") => app.ui.show_configurations,
        Some("window.universe") => app.ui.show_universe,
        Some("window.video") => app.ui.show_video,
        Some("window.video_universe") => app.ui.show_video_universe,
        Some("window.search") => app.ui.show_search,
        Some("window.metadata") => app.ui.show_metadata,
        Some("window.color_palette") => app.ui.show_color_palette,
        Some("window.task_manager") => app.ui.show_task_manager,
        Some("window.disk_usage") => app.ui.show_disk_usage,
        Some("window.system_usage") => app.ui.show_system_usage,
        Some("ui.mix.wide_editors") => app.ui.wide_editors,
        Some("ui.mix.slim_editors") => app.ui.slim_editors,
        Some("window.renderer") => app.extra.show_renderer,
        Some("window.scout") => app.scout.show,
        Some("window.studio") => app.studio.show,
        Some("window.plugin_browser") => app.ui.show_plugin_browser,
        Some("window.ui_customization") => app.extra.show_ui_customization,
        Some("window.config_auto_update") => app.extra.auto_update_config,
        // All and Minimal are actions, not states.
        Some("view.mix_section") if matches!(leaf, "All" | "Minimal") => return None,
        Some("view.mix_section") => app.ui.mix_views.iter().any(|v| v == mix_section_id(leaf)),
        _ => return None,
    })
}

/// True when the clip commands would act on clips (the selected ones, or those in the edit
/// selection) and every one of them passes `f`.
fn selected_clips_all(app: &SoundApp, f: impl Fn(&soundcraft_model::Clip) -> bool) -> bool {
    let s = app.engine.session();
    let ids = soundcraft_engine::cmd::clip_ids_param(&app.engine, &json!({}));
    let clips: Vec<_> = ids.iter().filter_map(|id| s.find_clip(*id).map(|(_, c)| c)).collect();
    !clips.is_empty() && clips.into_iter().all(f)
}

/// `options.scrolling`'s mode for an Edit Window Scrolling item.
fn scrolling_mode(leaf: &str) -> &'static str {
    match leaf {
        "No Scrolling" => "none",
        "After Playback" => "after_playback",
        "Continuous" => "continuous",
        "Center Playhead" => "center",
        _ => "page",
    }
}

fn ruler_id(label: &str) -> &'static str {
    match label {
        "Bars|Beats" => "bars_beats",
        "Min:Secs" => "min_secs",
        "Timecode" => "timecode",
        "Feet+Frames" => "feet_frames",
        "Samples" => "samples",
        "Tempo" => "tempo",
        "Meter" => "meter",
        "Markers" => "markers",
        "Key Signature" => "key",
        "Sections" => "sections",
        "Chords" => "band_chords",
        "Lyrics" => "lyrics",
        _ => "",
    }
}

fn mix_section_id(label: &str) -> &'static str {
    match label {
        "Inserts A-E" => "inserts_ae",
        "Inserts F-J" => "inserts_fj",
        "Sends A-E" => "sends_ae",
        "Sends F-J" => "sends_fj",
        "I/O" => "io",
        "Track Color" => "color",
        "Comments" => "comments",
        "EQ Curve" => "eq_curve",
        "Meters and Faders" => "meters",
        "All" => "all",
        "Minimal" => "minimal",
        "Mic Preamps" => "mic_preamps",
        "Instruments" => "instruments",
        "Object" => "object",
        "Pitch" => "pitch",
        _ => "",
    }
}

/// Parameters implied by the menu item a command was reached through.
fn params_for(path: &str, id: &str) -> Value {
    let leaf = path.rsplit(" > ").next().unwrap_or("");
    match id {
        "view.ruler" => json!({"ruler": ruler_id(leaf)}),
        "view.main_counter" => json!({"format": ruler_id(leaf)}),
        "options.scrolling" => json!({"mode": scrolling_mode(leaf)}),
        "options.solo_mode" => {
            json!({"mode": if leaf.starts_with("AFL") { "afl" } else if leaf.starts_with("PFL") { "pfl" } else { "sip" }, "xor": leaf.starts_with("X-OR")})
        }
        "clip.rating" => json!({"rating": leaf.parse::<u8>().unwrap_or(0)}),
        "track.change_width" => json!({"format": leaf}),
        // Track › Hide hides; `track.hide` alone would unhide tracks that are all hidden already.
        "track.hide" => json!({"hidden": true}),
        "view.mix_section" => json!({"section": mix_section_id(leaf)}),
        "process.run" => json!({"process": AUDIOSUITE.iter().find(|(p, _)| *p == path).map_or("", |(_, x)| *x)}),
        _ => json!({}),
    }
}

/// Menu items that open a FreeDAWm window when clicked (programmatic calls still run the command).
const MENU_WINDOWS: &[(&str, &str)] = &[
    ("Setup > Appearance", "window.ui_customization"),
    ("Setup > Hardware...", "window.playback_engine"),
    ("Setup > Playback Engine...", "window.playback_engine"),
    ("Setup > I/O...", "window.io_setup"),
    ("Setup > MIDI Input Devices...", "window.midi_inputs"),
    ("Setup > Network...", "window.network"),
    ("Setup > Keyboard Shortcuts...", "window.shortcuts"),
    ("Setup > Preferences...", "window.preferences"),
    ("Setup > Session", "setup.session"),
    ("Event > Beat Detective", "window.beat_detective"),
    ("Event > Tempo Operations > Tempo Operations Window", "window.tempo_ops"),
    ("Event > Time Operations > Time Operations Window", "window.time_ops"),
    ("Event > MIDI Operations > MIDI Operations Window", "window.midi_ops"),
    ("Event > MIDI Real-Time Properties", "window.rtp"),
];

/// The shortcut a menu item shows for `id`: the user's, else the registry's (blank for none).
pub fn shortcut_text(app: &SoundApp, id: &str, mac: bool) -> String {
    crate::shortcuts::effective_shortcut(&app.ui.shortcuts, id).map(|s| crate::shortcuts::shortcut_label(&s, mac)).unwrap_or_default()
}

/// The registry's shortcut of an engine or window command (the default; see `shortcut_text`).
pub fn shortcut_for(id: &str) -> Option<&'static str> {
    soundcraft_engine::find_command(id).map_or_else(|| UI_COMMANDS.iter().find(|c| c.0 == id).and_then(|c| c.3), |c| c.shortcut)
}

/// Undo and Redo name the step they would undo or redo, and Freeze reads Unfreeze when it would
/// thaw the selected tracks; other items keep their menu-bar label.
pub fn live_label(app: &SoundApp, id: Option<&str>) -> Option<String> {
    let e = &app.engine;
    match id? {
        "edit.undo" => Some(e.undo_label().map_or_else(|| "Can't Undo".into(), |l| format!("Undo {l}"))),
        "edit.redo" => Some(e.redo_label().map_or_else(|| "Can't Redo".into(), |l| format!("Redo {l}"))),
        "track.freeze" => Some(if selection_frozen(app) { "Unfreeze" } else { "Freeze" }.into()),
        _ => None,
    }
}

/// Track › Freeze thaws instead when every selected track is frozen (`track.freeze`'s own rule).
fn selection_frozen(app: &SoundApp) -> bool {
    soundcraft_engine::cmd::all_frozen(&app.engine, &app.engine.target_tracks())
}

/// Menu-style invocation: may open a dialog.
pub fn invoke_menu(app: &mut SoundApp, id: &str, path: &str) {
    // The ML jobs run for minutes: from a menu or the palette they open their dialog and run on a
    // worker, never as the blocking engine command on the UI thread.
    #[cfg(not(target_arch = "wasm32"))]
    if id.starts_with("ml.") && id != "ml.models" && !crate::ml_downloads::is_download_command(id) {
        if let Err(e) = crate::ml_ui::open(app, id, None) {
            app.ui.status = e;
        }
        return;
    }
    if let Some((_, w)) = MENU_WINDOWS.iter().find(|(p, _)| *p == path)
        && w.starts_with("window.")
    {
        let _ = app.run(w, json!({"value": true}));
        return;
    }
    // A window opened from an item that names it with "…" (Track › Color…) opens; it never closes.
    if id.starts_with("window.") && opens_window(path) {
        let _ = app.run(id, json!({"value": true}));
        return;
    }
    // Detect Tempo… analyses on a worker in its dialog, never on the UI thread.
    if id == "event.tempo_detect" {
        if let Err(e) = crate::tempo_ui::open(app, &json!({})) {
            app.ui.status = e;
        }
        return;
    }
    // Options › Pre/Post-Roll… sets the amounts too (its shortcut, Cmd+K, only toggles).
    if id == "options.pre_post_roll" {
        app.dialogs.open_pre_post_roll(&app.engine);
        return;
    }
    if soundcraft_engine::canonical_id(id) == "process.run" {
        let p = params_for(path, "process.run");
        app.ui.audiosuite = p.get("process").and_then(Value::as_str).map(str::to_string);
        return;
    }
    if wants_dialog(id) && app.dialogs.open_for_command(&app.engine, id) {
        return;
    }
    // These ids only exist to open a dialog from the menu. Programmatic `app.run` refuses them.
    if id == "ui.new_tracks_dialog" {
        let _ = app.dialogs.open_for_command(&app.engine, "track.new");
        return;
    }
    if id == "ui.bounce_dialog" {
        let _ = app.dialogs.open_for_command(&app.engine, "file.bounce_mix");
        return;
    }
    let params = params_for(path, id);
    let _ = crate::dialogs::run_or_ask(app, id, params);
}

/// Keyboard shortcut for a dialog command, or one that would close a modified session: open its
/// dialog. Returns true when handled.
pub fn invoke_shortcut_dialog(app: &mut SoundApp, id: &str) -> bool {
    (wants_dialog(id) && app.dialogs.open_for_command(&app.engine, id)) || crate::dialogs::ask_to_save(app, id, &json!({}))
}

/// Commands that open a dialog first: the listed ones, Score Setup, and anything that needs a path.
fn wants_dialog(id: &str) -> bool {
    DIALOG_COMMANDS.contains(&id) || id == "file.score_setup" || crate::dialogs::takes_path(id)
}

/// Whether clicking the menu item for `id` (run through `path`) opens a dialog or a window rather
/// than acting at once: what a label ending in "…" promises.
pub fn opens_dialog(id: &str, path: &str) -> bool {
    wants_dialog(id)
        || id.starts_with("window.")
        || id.starts_with("ui.ml.")
        || MENU_WINDOWS.iter().any(|(p, _)| *p == path)
        // Pre/Post-Roll… asks for the amounts; Revert… asks before throwing changes away.
        || matches!(id, "event.tempo_detect" | "options.pre_post_roll" | "session.revert" | "ui.midi_cleanup" | "ui.vocal_editor" | "ui.vocal_harmony" | "ui.vocal_preset_save" | "ui.scout.import_reference")
}

/// Handle UI-layer commands. Returns None when `id` is not a UI command.
pub fn run_ui_command(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if matches!(id, "ui.dock" | "ui.dock.reset") {
        return Some(crate::docking::command(app, id == "ui.dock.reset", p));
    }
    // The MIDI Editor's state and tools: `linked` sets its 🔗 Link; Select all, Split, Join,
    // Grid, ghosts, zoom and the note bar (`midi_edit_tools::command`).
    if id == "ui.midi_editor" {
        return Some(crate::midi_editor_link::command(app, p).and_then(|link| {
            let mut v = crate::midi_edit_tools::command(app, p)?;
            if let (Some(o), Some(l)) = (v.as_object_mut(), link.as_object()) {
                for k in ["open", "linked"] {
                    if let Some(x) = l.get(k) {
                        o.insert(k.into(), x.clone());
                    }
                }
            }
            Ok(v)
        }));
    }
    // MIDI Cleanup… opens the MIDI Editor with its Cleanup panel showing.
    if id == "ui.midi_cleanup" {
        app.ui.show_midi_editor = true;
        app.midi.show_cleanup = true;
        return Some(Ok(json!({})));
    }
    // Tune Vocal… opens the Vocal Editor, on a clip or a track when given.
    if id == "ui.vocal_editor" {
        return Some(crate::vocal_editor::open_command(app, p));
    }
    // The Vocal Studio: the editor over every vocal track (Window › Vocal Studio toggles it).
    if id == "ui.vocal_studio" {
        return Some(crate::vocal_editor::studio::command(app, p));
    }
    if id == "ui.vocal_harmony" {
        return Some(crate::vocal_editor::harmony::command(app, p));
    }
    if id == "window.vocal_studio" {
        let cur = app.ui.show_vocal_editor && app.vocal.studio.on;
        let on = p.get("value").and_then(Value::as_bool).unwrap_or(!cur);
        if on {
            return Some(crate::vocal_editor::studio::command(app, &json!({"value": true})).map(|_| json!({"value": true})));
        }
        app.vocal.studio.on = false;
        return Some(Ok(json!({"value": false})));
    }
    // The Vocal Editor's Tuning panel, and saving its values as a user macro preset.
    if id == "ui.vocal_panel" {
        app.vocal.show_panel = p.get("value").and_then(Value::as_bool).unwrap_or(!app.vocal.show_panel);
        return Some(Ok(json!({"value": app.vocal.show_panel})));
    }
    if id == "ui.vocal_preset_save" {
        return Some(crate::vocal_editor::panel::save_command(app, p));
    }
    if let Some(r) = crate::preferences::run(app, id, p) {
        return Some(r);
    }
    if let Some(v) = crate::extra_windows::run(app, id, p) {
        return Some(Ok(v));
    }
    if let Some(r) = crate::scout::run(app, id, p) {
        return Some(r);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(r) = crate::ml_ui::run(app, id, p) {
        return Some(r);
    }
    if id == "window.studio" {
        return Some(crate::band::studio::command(app, p));
    }
    if id == "ui.audio_device" {
        return Some(crate::windows::audio_device_command(app, p));
    }
    if id == "setup.record_offset" {
        return Some(crate::record_offset::record_offset_command(app, p));
    }
    if id == "setup.measure_latency" {
        return Some(crate::record_offset::measure_command(app, p));
    }
    if id == "setup.measure_cancel" {
        return Some(Ok(crate::record_offset::measure_cancel_command(app)));
    }
    if id == "ui.reset_underruns" {
        if let Some(pl) = &app.player {
            pl.reset_underruns();
        }
        return Some(Ok(json!({"underruns": 0})));
    }
    if id == "presets.import_firestation" {
        return Some(crate::fs_presets::command(app, p));
    }
    if let Some(r) = crate::channel_width::run(app, id, p) {
        return Some(r);
    }
    let toggle = |v: &mut bool| {
        *v = p.get("value").and_then(Value::as_bool).unwrap_or(!*v);
        json!({"value": *v})
    };
    let r = match id {
        "window.mix" => {
            app.ui.window = MainWindow::Mix;
            app.focus_mix = app.ui.mix_separate;
            json!({"window": "mix"})
        }
        "window.mix_separate" => {
            let v = toggle(&mut app.ui.mix_separate);
            app.focus_mix = app.ui.mix_separate;
            v
        }
        "ui.musical_typing" => {
            let on = p.get("value").and_then(Value::as_bool).unwrap_or(!app.typing.capture);
            crate::musical_typing::set_capture(app, on);
            json!({"value": on})
        }
        "window.midi_editor_separate" => {
            let v = toggle(&mut app.ui.midi_editor_separate);
            if app.ui.midi_editor_separate {
                app.ui.show_midi_editor = true;
                app.focus_midi_editor = true;
            }
            v
        }
        "window.vocal_separate" => {
            let v = toggle(&mut app.ui.vocal_separate);
            if app.ui.vocal_separate {
                app.ui.show_vocal_editor = true;
                app.focus_vocal = true;
            }
            v
        }
        "window.sync_selection" => {
            let v = toggle(&mut app.ui.sync.selection);
            crate::window_sync::selection_sync_changed(app);
            v
        }
        "window.sync_scroll" => toggle(&mut app.ui.sync.scroll),
        "window.sync_folders" => toggle(&mut app.ui.sync.folders),
        "window.sync_visibility" => toggle(&mut app.ui.sync.visibility),
        "window.edit" => {
            app.ui.window = MainWindow::Edit;
            json!({"window": "edit"})
        }
        "window.toggle_mix_edit" => {
            if app.ui.mix_separate {
                // Both are on screen: bring the Mix window forward instead.
                app.focus_mix = true;
            } else {
                app.ui.window = if app.ui.window == MainWindow::Edit { MainWindow::Mix } else { MainWindow::Edit };
            }
            json!({"window": app.ui.window})
        }
        "window.transport" => toggle(&mut app.ui.show_transport),
        "window.big_counter" => toggle(&mut app.ui.show_big_counter),
        "window.memory_locations" => toggle(&mut app.ui.show_memory_locations),
        "window.undo_history" => toggle(&mut app.ui.show_undo_history),
        "window.notes" => toggle(&mut app.ui.show_notes),
        "window.lyrics" => toggle(&mut app.ui.show_lyrics),
        "window.teleprompter" => toggle(&mut app.ui.show_teleprompter),
        "window.teleprompter_separate" => {
            let v = toggle(&mut app.ui.teleprompter_separate);
            if app.ui.teleprompter_separate {
                app.ui.show_teleprompter = true;
            }
            v
        }
        "window.plugin_browser" => toggle(&mut app.ui.show_plugin_browser),
        "window.clip_list" | "window.clip_list_view" => toggle(&mut app.ui.show_clip_list),
        "window.track_list" => toggle(&mut app.ui.show_tracks_list),
        "window.narrow_mix" | "view.mix_slim" => toggle(&mut app.ui.narrow_mix),
        "window.session_info" => toggle(&mut app.ui.show_session_info),
        "window.audio_health" => {
            app.audio_health_report = None;
            toggle(&mut app.ui.show_audio_health)
        }
        "window.about" => toggle(&mut app.ui.show_about),
        "window.midi_editor" => toggle(&mut app.ui.show_midi_editor),
        "window.vocal_editor" => toggle(&mut app.ui.show_vocal_editor),
        "window.midi_front" => {
            app.ui.show_midi_editor = true;
            json!({"value": true})
        }
        "window.midi_back" => {
            app.ui.show_midi_editor = false;
            json!({})
        }
        "window.automation" => toggle(&mut app.ui.show_automation),
        "window.clip_effects" | "window.clip_effects_dock" => toggle(&mut app.ui.show_clip_effects),
        "window.search" => toggle(&mut app.ui.show_search),
        "window.score" => toggle(&mut app.ui.show_score),
        "window.beat_detective" => toggle(&mut app.ui.show_beat_detective),
        "window.tempo_ops" => toggle(&mut app.ui.show_tempo_ops),
        "window.time_ops" => toggle(&mut app.ui.show_time_ops),
        "window.midi_ops" => toggle(&mut app.ui.show_midi_ops),
        "window.rtp" => toggle(&mut app.ui.show_rtp),
        "window.playback_engine" => toggle(&mut app.ui.show_playback_engine),
        "window.io_setup" => toggle(&mut app.ui.show_io_setup),
        "window.midi_inputs" => toggle(&mut app.ui.show_midi_inputs),
        "window.network" => toggle(&mut app.ui.show_network),
        #[cfg(not(target_arch = "wasm32"))]
        "net.remote_mix" => return Some(crate::net_ui::remote_mix_command(app, p)),
        "window.x32" => toggle(&mut app.ui.show_x32),
        #[cfg(not(target_arch = "wasm32"))]
        "setup.x32" => return Some(crate::x32_ui::command(app, p)),
        "window.shortcuts" => toggle(&mut app.ui.show_shortcuts),
        "window.color_palette" => toggle(&mut app.ui.show_color_palette),
        "window.disk_usage" => toggle(&mut app.ui.show_disk_usage),
        "window.system_usage" => toggle(&mut app.ui.show_system_usage),
        "window.task_manager" => toggle(&mut app.ui.show_task_manager),
        "window.metadata" => toggle(&mut app.ui.show_metadata),
        "window.event_list" => toggle(&mut app.ui.show_event_list),
        // Musical Typing opens ready to play: the computer keys on with the window, off with it.
        "window.midi_keyboard" => {
            let open = !app.ui.show_midi_keyboard;
            app.ui.show_midi_keyboard = open;
            crate::musical_typing::set_capture(app, open);
            if !open {
                crate::musical_typing::release_all(app);
            }
            json!({"value": open})
        }
        "window.workspace" | "window.workspace_presets" | "window.workspace_sounds" | "window.workspace_front" => {
            app.ui.show_workspace = true;
            json!({"value": true})
        }
        "window.workspace_close" => {
            app.ui.show_workspace = false;
            json!({"value": false})
        }
        "window.configurations" => toggle(&mut app.ui.show_configurations),
        "window.config_new" | "window.config_update" => {
            if let Err(error) = app.ui.docking.validate().and_then(|()| app.ui.mix_docking.validate_mix()) {
                return Some(Err(error));
            }
            let mut snap = app.ui.clone();
            snap.configurations.clear();
            let v = serde_json::to_value(&snap).unwrap_or_default();
            if id == "window.config_update" && !app.ui.configurations.is_empty() {
                if let Some(last) = app.ui.configurations.last_mut() {
                    last.1 = v;
                }
            } else {
                let n = app.ui.configurations.len() + 1;
                app.ui.configurations.push((format!("Configuration {n}"), v));
            }
            json!({"configurations": app.ui.configurations.len()})
        }
        "window.arrange_tile" | "window.arrange_tile_h" | "window.arrange_tile_v" | "window.arrange_cascade" => {
            if let Err(error) = crate::docking::arrange(app) {
                app.ui.status = format!("Panel windows could not be arranged: {error}");
            }
            app.arrange_request = true;
            json!({})
        }
        "window.universe" => toggle(&mut app.ui.show_universe),
        "window.video" => toggle(&mut app.ui.show_video),
        "window.video_universe" => toggle(&mut app.ui.show_video_universe),
        "window.close" | "window.hide_floating" => {
            app.ui.show_transport = false;
            app.ui.show_big_counter = false;
            app.ui.show_memory_locations = false;
            app.ui.show_undo_history = false;
            app.ui.show_session_info = false;
            app.ui.show_audio_health = false;
            app.ui.show_automation = false;
            app.ui.plugin_windows.clear();
            app.ui.full_editors.clear();
            app.ui.audiosuite = None;
            json!({})
        }
        "view.mix_section" => {
            let sec = p.get("section").and_then(Value::as_str).unwrap_or("");
            match sec {
                "all" => {
                    app.ui.mix_views = ["inserts_ae", "inserts_fj", "sends_ae", "sends_fj", "io", "comments", "color", "pitch"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                }
                "minimal" => app.ui.mix_views.clear(),
                "" => return Some(Err("`section` required".into())),
                s => {
                    if app.ui.mix_views.iter().any(|v| v == s) {
                        app.ui.mix_views.retain(|v| v != s);
                    } else {
                        app.ui.mix_views.push(s.to_string());
                    }
                }
            }
            json!({"mix_views": app.ui.mix_views})
        }
        "ui.new_tracks_dialog" => {
            return Some(Err("ui.new_tracks_dialog opens a dialog from the menu only; call track.new with parameters".into()));
        }
        "ui.bounce_dialog" => {
            return Some(Err("ui.bounce_dialog opens a dialog from the menu only; call file.bounce_mix with a path".into()));
        }
        "ui.new_from_template" => {
            return Some(Err("ui.new_from_template opens a dialog from the menu only; call session.new with a template path".into()));
        }
        "ui.theme" => {
            let Some(mode) = p.get("mode").and_then(Value::as_str) else {
                return Some(Ok(json!({"mode": app.ui.theme.id()})));
            };
            let Some(mode) = crate::theme::ThemeMode::parse(mode) else {
                return Some(Err(format!("unknown theme `{mode}` (system, light or dark)")));
            };
            app.ui.theme = mode;
            json!({"mode": mode.id()})
        }
        "ui.mix.drawer" => return Some(mix_drawer(app, p)),
        "ui.mix.full_editor" => return Some(full_editor(app, p)),
        "ui.mix.instrument_window" => return Some(instrument_window(app, p)),
        "ui.mix.wide_editors" => {
            // `{on?: bool}`: sets the flag, or flips it without `on` (the menu item).
            app.ui.wide_editors = match p.get("on") {
                None | Some(Value::Null) => !app.ui.wide_editors,
                Some(v) => match v.as_bool() {
                    Some(b) => b,
                    None => return Some(Err("`on` must be a boolean".into())),
                },
            };
            json!({"on": app.ui.wide_editors})
        }
        "ui.mix.slim_editors" => {
            // `{on?: bool}`: every FireStation editor draws its slim layout in the
            // classic drawer's width; flips without `on` (the menu item).
            app.ui.slim_editors = match p.get("on") {
                None | Some(Value::Null) => !app.ui.slim_editors,
                Some(v) => match v.as_bool() {
                    Some(b) => b,
                    None => return Some(Err("`on` must be a boolean".into())),
                },
            };
            json!({"on": app.ui.slim_editors})
        }
        "ui.mix.ceiling_learn" => return Some(crate::cleanq_ceiling::learn_command(app, p)),
        "ui.shortcuts.get" | "ui.shortcuts.set" | "ui.shortcuts.reset" => return Some(crate::shortcut_editor::command(app, id, p)),
        "ui.set" => match serde_json::to_value(&app.ui) {
            Ok(mut cur) => {
                if let (Some(obj), Some(patch)) = (cur.as_object_mut(), p.as_object()) {
                    for (k, v) in patch {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                match serde_json::from_value::<crate::UiState>(cur) {
                    Ok(mut ns) => {
                        // Not part of the saved state, so the round trip would drop it.
                        if ns.browser.selected.is_none() {
                            ns.browser.selected = app.ui.browser.selected.clone();
                        }
                        if let Err(error) = ns.docking.validate().and_then(|()| ns.mix_docking.validate_mix()) {
                            return Some(Err(error));
                        }
                        crate::docking::cancel_replaced_state(app, &ns);
                        app.ui = ns;
                        json!({"ui": app.ui})
                    }
                    Err(e) => return Some(Err(format!("bad ui state: {e}"))),
                }
            }
            Err(e) => return Some(Err(e.to_string())),
        },
        _ => return None,
    };
    Some(Ok(r))
}

/// `ui.mix.drawer {track, module, tab?}`: opens a built-in module's drawer in the Mix window
/// (replacing the strip's open one), on its `tab` (a parameter group, such as CleanQ's "Ceiling")
/// when given; without `module` (or with `null`) it folds the strip's drawer. `track` is an id or a
/// name; `module` an id, or a plugin id or short name (its first such module).
fn mix_drawer(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let id = named_track(app, p)?;
    if p.get("module").is_none_or(Value::is_null) {
        app.ui.drawers.remove(&id);
        return Ok(json!({"track": id.0, "module": null}));
    }
    let (_, module) = named_module(app, p)?;
    let plugin = app.engine.session().track(id).and_then(|t| t.mixer.module(module)).map(|i| i.plugin.clone()).unwrap_or_default();
    if !crate::module_drawer::has_drawer(&plugin) {
        return Err("only built-in modules open in a drawer; a plugin opens in its own window".into());
    }
    app.ui.drawers.insert(id, module);
    if let Some(tab) = p.get("tab").and_then(Value::as_str) {
        app.ui.drawer_tab = Some((id, module, tab.to_string()));
    }
    Ok(json!({"track": id.0, "module": module.0}))
}

/// `ui.mix.full_editor {track, module, open?}`: a module's full editor in a window of its own (the
/// tile's ⤢): a built-in's VST-faithful editor at full size (its generic parameter window when it
/// has none), a hosted plugin's own editor. `open: false` closes a built-in's.
fn full_editor(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let id = named_track(app, p)?;
    let (_, module) = named_module(app, p)?;
    let plugin = app.engine.session().track(id).and_then(|t| t.mixer.module(module)).map(|i| i.plugin.clone()).unwrap_or_default();
    let open = p.get("open").and_then(Value::as_bool).unwrap_or(true);
    let key = (id, module);
    let kind = if soundcraft_mix::is_third_party(&plugin) {
        if open {
            crate::chain_view::open_hosted(app, id, soundcraft_mix::SlotKey::Module(module));
        } else if let Some(pl) = &app.player {
            pl.close_editor(id, soundcraft_mix::SlotKey::Module(module));
        }
        "hosted"
    } else if crate::editors::has_wide(&plugin) {
        app.ui.full_editors.retain(|k| *k != key);
        if open {
            app.ui.full_editors.push(key);
            app.module_focus = Some((id, soundcraft_mix::SlotKey::Module(module)));
        }
        "editor"
    } else {
        let slot = (id, soundcraft_mix::SlotKey::Module(module));
        app.ui.plugin_windows.retain(|k| *k != slot);
        if open {
            crate::chain_view::show_parameters(app, id, slot.1);
        }
        "parameters"
    };
    Ok(json!({"track": id.0, "module": module.0, "open": open, "window": kind}))
}

/// The track a `{track}` command names: an id or a name.
/// `{track, open?: bool}`: opens (or with `open: false` closes) an instrument track's instrument
/// window, as double-clicking its instrument slot does: Lift's editor for the built-in sampler,
/// the parameter window of another built-in, a hosted plugin's window.
fn instrument_window(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let id = named_track(app, p)?;
    let s = app.engine.session();
    let plugin = s.track(id).and_then(|t| t.instrument.as_ref()).map(|i| i.plugin.clone()).ok_or("the track has no instrument")?;
    let open = p.get("open").and_then(Value::as_bool).unwrap_or(true);
    let key = (id, soundcraft_mix::SlotKey::Instrument);
    app.ui.plugin_windows.retain(|k| *k != key);
    if open {
        app.ui.plugin_windows.push(key);
    }
    Ok(json!({"track": id.0, "plugin": plugin, "open": open}))
}

fn named_track(app: &SoundApp, p: &Value) -> Result<soundcraft_model::TrackId, String> {
    let s = app.engine.session();
    match p.get("track") {
        Some(Value::Number(n)) => n.as_u64().and_then(|id| s.track(soundcraft_model::TrackId(id))),
        Some(Value::String(name)) => s.track_by_name(name),
        _ => return Err("`track` required (an id or a name)".into()),
    }
    .map(|t| t.id)
    .ok_or_else(|| "no such track".into())
}

/// The track and module a `{track, module}` command names: `track` an id or a name, `module` an id
/// or a plugin id / short name ("compressor", "Comp": the chain's first such module).
pub(crate) fn named_module(app: &SoundApp, p: &Value) -> Result<(soundcraft_model::TrackId, soundcraft_model::ModuleId), String> {
    let id = named_track(app, p)?;
    let s = app.engine.session();
    let track = s.track(id).ok_or("no such track")?;
    let m = p.get("module").filter(|v| !v.is_null()).ok_or("`module` required")?;
    let ins = match m {
        Value::String(name) => track.mixer.inserts.iter().find(|i| {
            i.plugin.eq_ignore_ascii_case(name) || soundcraft_dsp::plugin_info(&i.plugin).is_some_and(|p| p.short_name.eq_ignore_ascii_case(name))
        }),
        _ => {
            let module = m.as_u64().and_then(|m| u32::try_from(m).ok()).ok_or("`module` must be a module id or a plugin name")?;
            track.mixer.module(soundcraft_model::ModuleId(module))
        }
    }
    .ok_or("no such module on that track")?;
    Ok((id, ins.id))
}

/// Parity including UI-layer commands.
pub fn parity() -> Value {
    let extra = ui_aliases();
    catalog::parity_with(&extra)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, SoundApp};

    #[test]
    fn mix_view_aliases_are_allocated_once() {
        let first = ui_aliases();
        let second = ui_aliases();
        let a = first.iter().find(|(path, _)| path.contains("Mix Window Views > All")).map(|(path, _)| *path).unwrap();
        let b = second.iter().find(|(path, _)| path.contains("Mix Window Views > All")).map(|(path, _)| *path).unwrap();
        assert!(std::ptr::eq(a, b));
        assert_eq!(first.iter().filter(|(path, id)| path.contains("Mix Window Views") && *id == "view.mix_section").count(), 15);
    }

    #[test]
    fn programmatic_dialog_commands_stay_closed_and_the_menu_opens_them() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let err = app.run("ui.new_tracks_dialog", json!({})).unwrap_err();
        assert!(err.contains("track.new"), "{err}");
        let err = app.run("ui.bounce_dialog", json!({})).unwrap_err();
        assert!(err.contains("file.bounce_mix"), "{err}");
        assert!(app.dialogs.open_name().is_none());

        invoke_menu(&mut app, "ui.new_tracks_dialog", "Track > New...");
        assert_eq!(app.dialogs.open_name(), Some("new_tracks"));
        app.dialogs.open = None;
        invoke_menu(&mut app, "ui.bounce_dialog", "File > Bounce Mix...");
        assert_eq!(app.dialogs.open_name(), Some("bounce"));
    }

    #[test]
    fn save_asks_for_a_path_until_the_session_has_one() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        invoke_menu(&mut app, "session.save", "File > Save Session");
        assert!(matches!(&app.dialogs.open, Some(crate::dialogs::Dialog::PathPrompt { cmd, .. }) if cmd == "session.save_as"));
        app.dialogs.open = None;
        assert!(invoke_shortcut_dialog(&mut app, "session.save"));
        app.dialogs.open = None;

        let dir = std::env::temp_dir().join(format!("soundcraft-save-{}", std::process::id()));
        let path = dir.join("Song.scraft");
        app.run("session.save_as", json!({"path": path.to_string_lossy()})).unwrap();
        app.run("track.new", json!({})).unwrap();
        assert!(!invoke_shortcut_dialog(&mut app, "session.save"));
        invoke_menu(&mut app, "session.save", "File > Save Session");
        assert!(app.dialogs.open.is_none());
        assert!(!app.engine.is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn track_instrument_menu_targets_the_selected_instrument_track() {
        let mut e = soundcraft_engine::Engine::default();
        let audio = e.execute("track.new", &json!({"kind": "audio"})).unwrap()["tracks"][0].as_u64().unwrap();
        assert_eq!(instrument_track(e.session()), None);
        let inst = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("edit.select", &json!({"tracks": [audio]})).unwrap();
        assert_eq!(instrument_track(e.session()), None, "an audio track has no instrument");
        e.execute("edit.select", &json!({"tracks": [audio, inst]})).unwrap();
        assert_eq!(instrument_track(e.session()).map(|t| t.0), Some(inst));
    }
}
