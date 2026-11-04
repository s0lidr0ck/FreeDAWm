//! Setup › Keyboard Shortcuts: every command with a menu place or a shortcut, searchable; click a
//! shortcut and press new keys. Changes are overrides in `ui.json` (`UiState::shortcuts`), applied
//! at once; the registry's shortcuts stay the defaults. Context keys (`shortcuts::FIXED`) are
//! listed but fixed.

use crate::shortcuts::{self, Holder, Overrides};
use crate::theme::{Tokens, mono};
use crate::{SoundApp, UiState};
use egui::{Key, Modifiers, vec2};

/// The window's state between frames (not saved).
#[derive(Debug, Clone, Default)]
pub struct EditorState {
    pub search: String,
    /// The command whose new keys are being typed.
    pub capturing: Option<String>,
    /// Keys typed for a command that another command, or a fixed key, already has.
    pub conflict: Option<Conflict>,
    /// Why the last keys typed were not taken.
    pub hint: Option<String>,
    /// "Reset All" asks once before clearing every override.
    pub confirm_reset_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub command: String,
    pub shortcut: String,
    pub holder: Holder,
}

/// One editable (or fixed) command row.
#[derive(Debug, Clone)]
pub struct Row {
    pub id: &'static str,
    pub label: &'static str,
    /// Where the menu bar shows it; empty for none.
    pub menu: String,
    pub editable: bool,
}

/// The commands the window lists: each engine or UI command that has a shortcut or a menu place.
pub fn rows() -> &'static [Row] {
    static ROWS: std::sync::OnceLock<Vec<Row>> = std::sync::OnceLock::new();
    ROWS.get_or_init(|| {
        let mut seen = std::collections::HashSet::new();
        shortcuts::all_commands()
            .filter(|(id, _)| seen.insert(*id))
            .filter_map(|(id, label)| {
                let menu = crate::menus::menu_location(id);
                (menu.is_some() || shortcuts::default_shortcut(id).is_some()).then(|| Row {
                    id,
                    label,
                    menu: menu.unwrap_or_default(),
                    editable: shortcuts::editable(id),
                })
            })
            .collect()
    })
}

/// Whether `row` matches the search `query` (label, menu, id or shortcut; case-insensitive).
pub fn matches(row: &Row, query: &str, overrides: &Overrides, mac: bool) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let shortcut = shortcuts::effective_shortcut(overrides, row.id).map(|s| shortcuts::shortcut_label(&s, mac)).unwrap_or_default();
    [row.label, row.menu.as_str(), row.id, shortcut.as_str()].iter().any(|t| t.to_lowercase().contains(&q))
}

/// Starts typing new keys for `id`.
pub fn start_capture(ui: &mut UiState, id: &str) {
    let ed = &mut ui.shortcut_editor;
    ed.capturing = Some(id.to_string());
    ed.conflict = None;
    ed.hint = None;
    ed.confirm_reset_all = false;
}

/// Whether `id` may have `key` without Cmd, Ctrl or Alt: an F-key, or a command whose own
/// shortcut is a bare key.
fn bare_ok(id: &str, key: Key) -> bool {
    let f_key = key.name().strip_prefix('F').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    f_key || shortcuts::default_shortcut(id).and_then(shortcuts::parse).is_some_and(|(m, _)| !m.command && !m.ctrl && !m.alt)
}

/// A key typed while capturing: Escape cancels, Backspace (or Delete) clears the shortcut, other
/// keys with their modifiers become the shortcut, unless another command or a fixed key has them
/// (then the conflict waits for "Take it"). Returns false when nothing was capturing.
pub fn capture_key(ui: &mut UiState, key: Key, m: Modifiers, mac: bool) -> bool {
    let Some(id) = ui.shortcut_editor.capturing.clone() else { return false };
    let plain = !m.command && !m.mac_cmd && !m.ctrl && !m.alt && !m.shift;
    let ed = &mut ui.shortcut_editor;
    if plain && key == Key::Escape {
        ed.capturing = None;
        ed.hint = None;
        return true;
    }
    if plain && matches!(key, Key::Backspace | Key::Delete) {
        ed.capturing = None;
        ed.hint = None;
        shortcuts::set_override(&mut ui.shortcuts, &id, Some(""));
        return true;
    }
    let Some(shortcut) = shortcuts::shortcut_string(m, key, mac) else {
        ed.hint = Some(format!("{} can't be a shortcut", key.name()));
        return true;
    };
    let modified = m.command || m.mac_cmd || (mac && m.ctrl) || m.alt;
    if !modified && !bare_ok(&id, key) {
        ed.hint = Some(format!("Add {} or Alt", if mac { "Cmd, Ctrl" } else { "Ctrl" }));
        return true;
    }
    ed.capturing = None;
    ed.hint = None;
    match shortcuts::holder(&ui.shortcuts, &shortcut, &id) {
        Some(holder) => ui.shortcut_editor.conflict = Some(Conflict { command: id, shortcut, holder }),
        None => shortcuts::set_override(&mut ui.shortcuts, &id, Some(&shortcut)),
    }
    true
}

/// "Take it": the conflicting command loses the shortcut and the edited one gets it. A fixed key
/// can't be taken; the conflict stays.
pub fn take_it(ui: &mut UiState) {
    let Some(c) = ui.shortcut_editor.conflict.clone() else { return };
    let Holder::Command(other, _) = c.holder else { return };
    shortcuts::set_override(&mut ui.shortcuts, other, Some(""));
    shortcuts::set_override(&mut ui.shortcuts, &c.command, Some(&c.shortcut));
    ui.shortcut_editor.conflict = None;
}

/// Back to the registry's shortcut for `id`.
pub fn reset(ui: &mut UiState, id: &str) {
    shortcuts::set_override(&mut ui.shortcuts, id, None);
    if ui.shortcut_editor.conflict.as_ref().is_some_and(|c| c.command == id) {
        ui.shortcut_editor.conflict = None;
    }
}

/// Every command back to the registry's shortcut.
pub fn reset_all(ui: &mut UiState) {
    ui.shortcuts.clear();
    ui.shortcut_editor.conflict = None;
    ui.shortcut_editor.confirm_reset_all = false;
}

/// The control commands: `ui.shortcuts.get {}` → `{shortcuts}` (the override map);
/// `ui.shortcuts.set {command, shortcut: string | null, take?: false}` (null or "" for none; a
/// shortcut another command has is refused unless `take`, which clears it there; a fixed key is
/// always refused); `ui.shortcuts.reset {command?}` (one command, or all).
pub fn command(app: &mut SoundApp, id: &str, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::{Value, json};
    let command = p.get("command").and_then(Value::as_str);
    let known = |c: &str| shortcuts::all_commands().any(|(id, _)| id == c);
    match id {
        "ui.shortcuts.get" => {}
        "ui.shortcuts.reset" => match command {
            None => reset_all(&mut app.ui),
            Some(c) if known(c) => reset(&mut app.ui, c),
            Some(c) => return Err(format!("unknown command `{c}`")),
        },
        _ => {
            let Some(c) = command else { return Err("ui.shortcuts.set needs `command`".into()) };
            let Some(cid) = shortcuts::all_commands().map(|(id, _)| id).find(|id| *id == c) else {
                return Err(format!("unknown command `{c}`"));
            };
            if !shortcuts::editable(cid) {
                return Err(format!("`{cid}` has a fixed shortcut"));
            }
            let shortcut = match p.get("shortcut") {
                None | Some(Value::Null) => "",
                Some(v) => v.as_str().ok_or("`shortcut` must be a string or null")?,
            };
            if !shortcut.is_empty() {
                if shortcuts::parse(shortcut).is_none() {
                    return Err(format!("`{shortcut}` is not a shortcut (use the form Cmd+Shift+K)"));
                }
                match shortcuts::holder(&app.ui.shortcuts, shortcut, cid) {
                    Some(Holder::Command(other, _)) if p.get("take").and_then(Value::as_bool) == Some(true) => {
                        shortcuts::set_override(&mut app.ui.shortcuts, other, Some(""));
                    }
                    Some(h) => return Err(format!("`{shortcut}` is used by {}", h.label())),
                    None => {}
                }
            }
            shortcuts::set_override(&mut app.ui.shortcuts, cid, Some(shortcut));
        }
    }
    Ok(json!({ "shortcuts": app.ui.shortcuts }))
}

/// Takes this frame's key presses (and typed text) away from everything else while capturing.
fn take_keys(ctx: &egui::Context) -> Vec<(Key, Modifiers)> {
    ctx.input_mut(|i| {
        let mut keys = Vec::new();
        i.events.retain(|e| match e {
            egui::Event::Key { key, pressed, repeat, modifiers, .. } => {
                if *pressed && !*repeat {
                    keys.push((*key, *modifiers));
                }
                false
            }
            egui::Event::Text(_) => false,
            _ => true,
        });
        keys
    })
}

enum Action {
    Capture(&'static str),
    Reset(&'static str),
    TakeIt,
    Cancel,
    ResetAll,
    AskResetAll(bool),
}

/// Setup › Keyboard Shortcuts.
pub fn window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_shortcuts;
    if !open {
        app.ui.shortcut_editor.capturing = None;
        return;
    }
    let mac = ctx.os().is_mac();
    if app.ui.shortcut_editor.capturing.is_some() {
        for (key, m) in take_keys(ctx) {
            capture_key(&mut app.ui, key, m, mac);
        }
    }
    let mut actions = Vec::new();
    egui::Window::new("Keyboard Shortcuts").open(&mut open).default_size(vec2(660.0, 520.0)).show(ctx, |ui| {
        body(&mut app.ui, ui, mac, &mut actions);
    });
    for a in actions {
        match a {
            Action::Capture(id) => start_capture(&mut app.ui, id),
            Action::Reset(id) => reset(&mut app.ui, id),
            Action::TakeIt => take_it(&mut app.ui),
            Action::Cancel => app.ui.shortcut_editor.conflict = None,
            Action::ResetAll => reset_all(&mut app.ui),
            Action::AskResetAll(on) => app.ui.shortcut_editor.confirm_reset_all = on,
        }
    }
    if !open {
        app.ui.shortcut_editor.capturing = None;
        app.ui.shortcut_editor.conflict = None;
    }
    app.ui.show_shortcuts = open;
}

fn body(state: &mut UiState, ui: &mut egui::Ui, mac: bool, actions: &mut Vec<Action>) {
    let t = Tokens::current();
    ui.horizontal(|ui| {
        ui.label("Search");
        ui.add(egui::TextEdit::singleline(&mut state.shortcut_editor.search).hint_text("command, menu or keys").desired_width(240.0));
    });
    let cmd = if mac { "Cmd" } else { "Ctrl" };
    ui.label(
        egui::RichText::new(format!("Click a shortcut, then press the new keys ({cmd}, Alt or Shift plus one key). Changes apply at once."))
            .small()
            .color(t.text_dim),
    );
    ui.separator();
    let query = state.shortcut_editor.search.clone();
    // A fixed list height: the window sizes to its content, so filling the space left would grow it every frame.
    egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(440.0).show(ui, |ui| {
        egui::Grid::new("shortcut_editor").num_columns(3).striped(true).min_col_width(80.0).show(ui, |ui| {
            for h in ["Command", "Menu", "Shortcut"] {
                ui.label(egui::RichText::new(h).strong());
            }
            ui.end_row();
            for row in rows().iter().filter(|r| matches(r, &query, &state.shortcuts, mac)) {
                ui.label(row.label);
                ui.label(egui::RichText::new(&row.menu).small().color(t.text_dim));
                shortcut_cell(state, ui, row, mac, actions);
                ui.end_row();
            }
            let q = query.trim().to_lowercase();
            for (keys, what, _) in
                shortcuts::FIXED.iter().filter(|f| q.is_empty() || f.0.to_lowercase().contains(&q) || f.1.to_lowercase().contains(&q))
            {
                ui.label(*what);
                ui.label(egui::RichText::new("context key").small().color(t.text_dim));
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(shortcuts::shortcut_label(keys, mac)).font(mono(11.0)).color(t.text_dim));
                    ui.label(egui::RichText::new("fixed").small().color(t.text_dim));
                });
                ui.end_row();
            }
        });
    });
    ui.separator();
    ui.horizontal(|ui| {
        let n = state.shortcuts.len();
        if state.shortcut_editor.confirm_reset_all {
            ui.label(format!("Reset all {n} changed shortcuts?"));
            if ui.button("Reset All").clicked() {
                actions.push(Action::ResetAll);
            }
            if ui.button("Cancel").clicked() {
                actions.push(Action::AskResetAll(false));
            }
        } else {
            if ui.add_enabled(n > 0, egui::Button::new("Reset All…")).clicked() {
                actions.push(Action::AskResetAll(true));
            }
            ui.label(egui::RichText::new(format!("{n} changed")).small().color(t.text_dim));
        }
    });
}

fn shortcut_cell(state: &UiState, ui: &mut egui::Ui, row: &Row, mac: bool, actions: &mut Vec<Action>) {
    let t = Tokens::current();
    let shown = shortcuts::effective_shortcut(&state.shortcuts, row.id).map(|s| shortcuts::shortcut_label(&s, mac)).unwrap_or_default();
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            if !row.editable {
                ui.label(egui::RichText::new(&shown).font(mono(11.0)).color(t.text_dim));
                ui.label(egui::RichText::new("fixed").small().color(t.text_dim));
                return;
            }
            let ed = &state.shortcut_editor;
            let capturing = ed.capturing.as_deref() == Some(row.id);
            let text = if capturing {
                "Press keys… (Esc cancels, Backspace clears)".to_string()
            } else if shown.is_empty() {
                "—".to_string()
            } else {
                shown.clone()
            };
            let resp = ui.add(egui::Button::new(egui::RichText::new(text).font(mono(11.0))).selected(capturing).min_size(vec2(96.0, 0.0)));
            if resp.clicked() {
                resp.surrender_focus();
                actions.push(Action::Capture(row.id));
            }
            if state.shortcuts.contains_key(row.id) && ui.link(egui::RichText::new("Reset").small()).clicked() {
                actions.push(Action::Reset(row.id));
            }
        });
        let ed = &state.shortcut_editor;
        if ed.capturing.as_deref() == Some(row.id)
            && let Some(hint) = &ed.hint
        {
            ui.label(egui::RichText::new(hint).small().color(ui.visuals().warn_fg_color));
        }
        if let Some(c) = ed.conflict.as_ref().filter(|c| c.command == row.id) {
            ui.horizontal(|ui| {
                let keys = shortcuts::shortcut_label(&c.shortcut, mac);
                ui.label(egui::RichText::new(format!("{keys}: used by {}", c.holder.label())).color(ui.visuals().warn_fg_color));
                if matches!(c.holder, Holder::Command(..)) && ui.small_button("Take it").clicked() {
                    actions.push(Action::TakeIt);
                }
                if ui.small_button("Cancel").clicked() {
                    actions.push(Action::Cancel);
                }
            });
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const CMD: Modifiers = Modifiers { command: true, mac_cmd: true, ..Modifiers::NONE };
    const PC_CTRL: Modifiers = Modifiers { command: true, ctrl: true, ..Modifiers::NONE };

    fn ui() -> UiState {
        UiState::default()
    }

    #[test]
    fn typed_keys_become_the_shortcut() {
        let mut u = ui();
        start_capture(&mut u, "session.save");
        assert!(capture_key(&mut u, Key::Y, Modifiers { alt: true, shift: true, ..CMD }, true));
        assert_eq!(u.shortcuts.get("session.save").map(String::as_str), Some("Cmd+Alt+Shift+Y"));
        assert!(u.shortcut_editor.capturing.is_none());
        // Ctrl off macOS is the strings' Cmd.
        start_capture(&mut u, "session.save");
        capture_key(&mut u, Key::J, Modifiers { shift: true, ..PC_CTRL }, false);
        assert_eq!(shortcuts::effective_shortcut(&u.shortcuts, "session.save").as_deref(), Some("Cmd+Shift+J"));
        // Typing the default back drops the override.
        start_capture(&mut u, "session.save");
        capture_key(&mut u, Key::S, CMD, true);
        assert!(u.shortcuts.is_empty());
    }

    #[test]
    fn escape_cancels_and_backspace_clears() {
        let mut u = ui();
        start_capture(&mut u, "session.save");
        capture_key(&mut u, Key::Escape, Modifiers::NONE, true);
        assert!(u.shortcut_editor.capturing.is_none());
        assert!(u.shortcuts.is_empty());
        start_capture(&mut u, "session.save");
        capture_key(&mut u, Key::Backspace, Modifiers::NONE, true);
        assert_eq!(u.shortcuts.get("session.save").map(String::as_str), Some(""));
        assert_eq!(shortcuts::effective_shortcut(&u.shortcuts, "session.save"), None);
    }

    #[test]
    fn a_bare_letter_needs_a_modifier() {
        let mut u = ui();
        start_capture(&mut u, "session.save");
        capture_key(&mut u, Key::J, Modifiers::NONE, true);
        assert!(u.shortcuts.is_empty());
        assert!(u.shortcut_editor.hint.is_some());
        assert!(u.shortcut_editor.capturing.is_some(), "still waiting for keys");
        // An F-key may be bare.
        capture_key(&mut u, Key::F11, Modifiers::NONE, true);
        assert_eq!(u.shortcuts.get("session.save").map(String::as_str), Some("F11"));
    }

    #[test]
    fn a_taken_shortcut_reports_the_conflict_and_take_it_moves_it() {
        let mut u = ui();
        start_capture(&mut u, "file.export_clips");
        capture_key(&mut u, Key::S, CMD, true);
        let c = u.shortcut_editor.conflict.clone().unwrap();
        assert_eq!(c.holder, Holder::Command("session.save", "Save Session"));
        assert_eq!(c.shortcut, "Cmd+S");
        assert!(!u.shortcuts.contains_key("file.export_clips"), "nothing changes before Take it");
        take_it(&mut u);
        assert!(u.shortcut_editor.conflict.is_none());
        assert_eq!(shortcuts::effective_shortcut(&u.shortcuts, "file.export_clips").as_deref(), Some("Cmd+S"));
        assert_eq!(shortcuts::effective_shortcut(&u.shortcuts, "session.save"), None);
        // Reset gives Save its Cmd+S back; Reset All undoes the rest.
        reset(&mut u, "session.save");
        assert_eq!(shortcuts::effective_shortcut(&u.shortcuts, "session.save").as_deref(), Some("Cmd+S"));
        reset_all(&mut u);
        assert!(u.shortcuts.is_empty());
        assert_eq!(shortcuts::effective_shortcut(&u.shortcuts, "file.export_clips").as_deref(), Some("Cmd+Shift+K"));
    }

    #[test]
    fn fixed_keys_cannot_be_taken() {
        let mut u = ui();
        start_capture(&mut u, "session.save");
        capture_key(&mut u, Key::Space, CMD, true);
        let c = u.shortcut_editor.conflict.clone().unwrap();
        assert!(matches!(c.holder, Holder::Fixed(_)));
        take_it(&mut u);
        assert!(u.shortcut_editor.conflict.is_some());
        assert!(u.shortcuts.is_empty());
        assert!(!rows().iter().find(|r| r.id == "track.height_up").unwrap().editable);
    }

    #[test]
    fn search_filters_by_label_menu_id_and_keys() {
        let u = ui();
        let save = rows().iter().find(|r| r.id == "session.save").unwrap();
        assert!(matches(save, "save sess", &u.shortcuts, true));
        assert!(matches(save, "file", &u.shortcuts, true));
        assert!(matches(save, "cmd+s", &u.shortcuts, true));
        assert!(matches(save, "ctrl+s", &u.shortcuts, false));
        assert!(!matches(save, "bounce", &u.shortcuts, true));
        let n = rows().iter().filter(|r| matches(r, "save", &u.shortcuts, true)).count();
        assert!(n > 0 && n < rows().len());
    }

    /// The window: a click starts the capture, the next key press (taken from every other
    /// widget) sets the shortcut.
    #[test]
    fn window_captures_keys() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.ui.show_shortcuts = true;
        app.ui.shortcut_editor.search = "Save Session".into();
        let ctx = egui::Context::default();
        let frame = |app: &mut SoundApp, events: Vec<egui::Event>| {
            let input = egui::RawInput { events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| window(app, ui.ctx()));
            out.textures_delta.clear();
        };
        frame(&mut app, vec![]);
        start_capture(&mut app.ui, "session.save");
        // The window reads keys the way this platform reports them: Cmd on a Mac (where Ctrl is a
        // key of its own), Ctrl (reported as command + ctrl) elsewhere.
        let cmd = if cfg!(target_os = "macos") { Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE } } else { PC_CTRL };
        let press = egui::Event::Key {
            key: Key::Y,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers { alt: true, shift: true, ..cmd },
        };
        frame(&mut app, vec![press]);
        assert_eq!(app.ui.shortcuts.get("session.save").map(String::as_str), Some("Cmd+Alt+Shift+Y"));
        assert!(app.ui.shortcut_editor.capturing.is_none());
    }

    #[test]
    fn control_commands_get_set_and_reset() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let r = app.run("ui.shortcuts.set", serde_json::json!({"command": "file.export_clips", "shortcut": "Cmd+S"}));
        assert!(r.unwrap_err().contains("Save Session"), "a taken shortcut is refused");
        let r = app.run("ui.shortcuts.set", serde_json::json!({"command": "file.export_clips", "shortcut": "Cmd+S", "take": true})).unwrap();
        assert_eq!(r["shortcuts"]["file.export_clips"], "Cmd+S");
        assert_eq!(r["shortcuts"]["session.save"], "");
        assert!(app.run("ui.shortcuts.set", serde_json::json!({"command": "session.save", "shortcut": "Space"})).is_err(), "fixed");
        assert!(app.run("ui.shortcuts.set", serde_json::json!({"command": "session.save", "shortcut": "F1-F4"})).is_err());
        assert!(app.run("ui.shortcuts.set", serde_json::json!({"command": "nope", "shortcut": null})).is_err());
        app.run("ui.shortcuts.set", serde_json::json!({"command": "track.new", "shortcut": null})).unwrap();
        assert_eq!(shortcuts::effective_shortcut(&app.ui.shortcuts, "track.new"), None);
        app.run("ui.shortcuts.reset", serde_json::json!({"command": "track.new"})).unwrap();
        let got = app.run("ui.shortcuts.get", serde_json::json!({})).unwrap();
        assert_eq!(got["shortcuts"].as_object().unwrap().len(), 2);
        app.run("ui.shortcuts.reset", serde_json::json!({})).unwrap();
        assert!(app.ui.shortcuts.is_empty());
    }

    #[test]
    fn menu_labels_show_the_override() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        assert_eq!(crate::menus::shortcut_text(&app, "session.save", false), "Ctrl+S");
        shortcuts::set_override(&mut app.ui.shortcuts, "session.save", Some("Cmd+Alt+Shift+Y"));
        assert_eq!(crate::menus::shortcut_text(&app, "session.save", false), "Ctrl+Alt+Shift+Y");
        assert_eq!(crate::menus::shortcut_text(&app, "session.save", true), "Cmd+Alt+Shift+Y");
        shortcuts::set_override(&mut app.ui.shortcuts, "session.save", Some(""));
        assert_eq!(crate::menus::shortcut_text(&app, "session.save", true), "");
    }
}
