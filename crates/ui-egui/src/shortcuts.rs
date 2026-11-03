//! Keyboard shortcuts. Most come from the command registry's `shortcut` strings, overridden by
//! the user's map in `ui.json` (`UiState::shortcuts`, edited in Setup › Keyboard Shortcuts); a few
//! context-sensitive keys (Space, Enter, F-keys, nudge, Tab) are handled here and are fixed.

use crate::{MainWindow, SoundApp};
use egui::{Key, Modifiers};
use serde_json::json;
use std::collections::BTreeMap;

/// The user's shortcut overrides: command id → shortcut string, `""` for "no shortcut".
pub type Overrides = BTreeMap<String, String>;

/// A command's shortcut as the registry ships it (engine or UI command).
pub fn default_shortcut(id: &str) -> Option<&'static str> {
    crate::menus::shortcut_for(id)
}

/// The shortcut that runs `id`: the user's override when there is one (`""`: none), else the
/// registry's. Everything that reads or shows a shortcut goes through here.
pub fn effective_shortcut(overrides: &Overrides, id: &str) -> Option<String> {
    match overrides.get(id) {
        Some(s) if s.is_empty() => None,
        Some(s) => Some(s.clone()),
        None => default_shortcut(id).map(str::to_string),
    }
}

/// Whether two shortcut strings name the same keys ("Shift+Cmd+X" is "Cmd+Shift+X").
pub fn same_shortcut(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Context keys handled in [`fixed`] before any command's shortcut: (keys as shown, what they
/// do, the parseable shortcuts they take). Listed as fixed rows; no command can be given them.
pub const FIXED: &[(&str, &str, &[&str])] = &[
    ("Space", "Play / Stop", &["Space"]),
    ("Shift+Space", "Half-Speed Play", &["Shift+Space"]),
    ("Cmd+Space", "Record", &["Cmd+Space", "Cmd+Shift+Space"]),
    ("F12", "Record", &["F12"]),
    ("Enter", "Add Marker", &["Enter"]),
    ("F1–F4", "Edit modes: Shuffle, Slip, Spot, Grid", &["F1", "F2", "F3", "F4"]),
    ("Alt+1–4", "Edit modes: Shuffle, Slip, Spot, Grid", &["Alt+1", "Alt+2", "Alt+3", "Alt+4"]),
    ("`", "Next Edit mode", &["`"]),
    ("F5–F10", "Tools: Zoom, Trim, Selector, Grabber, Scrubber, Pencil", &["F5", "F6", "F7", "F8", "F9", "F10"]),
    ("Tab / Alt+Tab", "Tab to next / previous (Shift extends)", &["Tab", "Shift+Tab", "Alt+Tab", "Alt+Shift+Tab"]),
    ("+ / −", "Nudge later / earlier (Edit window)", &["Plus", "Minus"]),
    ("Shift+= / Shift+−", "Grid value up / down (Edit window)", &["Shift+=", "Shift+Minus", "Shift+Plus"]),
    ("Alt+Shift+= / Alt+Shift+−", "Nudge value up / down (Edit window)", &["Alt+Shift+=", "Alt+Shift+Minus", "Alt+Shift+Plus"]),
    ("Cmd+=", "Mix / Edit window", &["Cmd+="]),
    ("Delete / Backspace", "Clear, or delete the picked marker (Edit window)", &["Delete", "Backspace"]),
    ("R / T", "Zoom out / in", &["R", "T"]),
    ("A–Z, 0–9, ; ' , . /", "Single-key editing (Commands Keyboard Focus)", &[]),
];

/// Who already has a shortcut: a command (id, label) or a fixed context key (what it does).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    Command(&'static str, &'static str),
    Fixed(&'static str),
}

impl Holder {
    /// The name after "Used by".
    pub fn label(&self) -> String {
        match self {
            Holder::Command(_, label) => (*label).to_string(),
            Holder::Fixed(what) => format!("{what} (fixed)"),
        }
    }
}

/// Registry commands that [`fixed`] handles itself (so another key would not move them).
const FIXED_IDS: [&str; 5] = ["track.height_up", "track.height_down", "clip.gain_nudge_up", "clip.gain_nudge_down", "window.search"];

/// Whether the user may change `id`'s shortcut: not a key family the strings can't name
/// ("F1-F4"), and not one of the context keys [`fixed`] handles.
pub fn editable(id: &str) -> bool {
    if FIXED_IDS.contains(&id) {
        return false;
    }
    match default_shortcut(id) {
        None => true,
        Some(d) => parse(d).is_some() && !FIXED.iter().any(|f| f.2.iter().any(|k| same_shortcut(k, d))),
    }
}

/// Every engine and UI command: (id, label).
pub fn all_commands() -> impl Iterator<Item = (&'static str, &'static str)> {
    soundcraft_engine::command_specs().iter().map(|c| (c.id, c.label)).chain(crate::menus::UI_COMMANDS.iter().map(|c| (c.0, c.1)))
}

/// The command (other than `except`) or fixed key that `shortcut` already runs.
pub fn holder(overrides: &Overrides, shortcut: &str, except: &str) -> Option<Holder> {
    if let Some(what) = FIXED.iter().find(|f| f.2.iter().any(|k| same_shortcut(k, shortcut))).map(|f| f.1) {
        return Some(Holder::Fixed(what));
    }
    all_commands()
        .filter(|(id, _)| *id != except)
        .find(|(id, _)| effective_shortcut(overrides, id).is_some_and(|s| same_shortcut(&s, shortcut)))
        .map(|(id, label)| if editable(id) { Holder::Command(id, label) } else { Holder::Fixed(label) })
}

/// Gives `id` the shortcut `shortcut` (`Some("")`: none; `None`: back to the registry's). An
/// override equal to the default is dropped, so the map only holds real changes.
pub fn set_override(overrides: &mut Overrides, id: &str, shortcut: Option<&str>) {
    let default = default_shortcut(id);
    match shortcut {
        None => {
            overrides.remove(id);
        }
        Some("") if default.is_none() => {
            overrides.remove(id);
        }
        Some(s) if !s.is_empty() && default.is_some_and(|d| same_shortcut(d, s)) => {
            overrides.remove(id);
        }
        Some(s) => {
            overrides.insert(id.to_string(), s.to_string());
        }
    }
}

/// The registry-style string ("Cmd+Shift+X") for `key` pressed with `m`, or None for a key the
/// shortcut strings cannot name. Off macOS the Ctrl key is the strings' Cmd.
pub fn shortcut_string(m: Modifiers, key: Key, mac: bool) -> Option<String> {
    let cmd = m.command || m.mac_cmd;
    let ctrl = mac && m.ctrl;
    let mut parts: Vec<&str> = Vec::new();
    if cmd {
        parts.push("Cmd");
    }
    if ctrl {
        parts.push("Ctrl");
    }
    if m.alt {
        parts.push("Alt");
    }
    if m.shift {
        parts.push("Shift");
    }
    parts.push(match key {
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Slash => "/",
        Key::Quote => "'",
        Key::Equals => "=",
        Key::Backtick => "`",
        k => k.name(),
    });
    let s = parts.join("+");
    let want = Modifiers { command: cmd, ctrl, alt: m.alt, shift: m.shift, mac_cmd: false };
    (parse(&s) == Some((want, key))).then_some(s)
}

/// Parse "Cmd+Shift+X" style strings. Returns None for descriptive ones ("F1-F4", "Plus/Minus").
pub fn parse(s: &str) -> Option<(Modifiers, Key)> {
    let mut m = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+') {
        match part.trim() {
            "Cmd" => m.command = true,
            "Shift" => m.shift = true,
            "Alt" => m.alt = true,
            "Ctrl" => m.ctrl = true,
            "" => {}
            k => {
                if key.is_some() {
                    return None;
                }
                key = Some(match k {
                    "Space" => Key::Space,
                    "Enter" => Key::Enter,
                    "Home" => Key::Home,
                    "End" => Key::End,
                    "Tab" => Key::Tab,
                    "[" => Key::OpenBracket,
                    "]" => Key::CloseBracket,
                    "," => Key::Comma,
                    "." => Key::Period,
                    "/" => Key::Slash,
                    "'" => Key::Quote,
                    "=" => Key::Equals,
                    "`" => Key::Backtick,
                    other => Key::from_name(other)?,
                });
            }
        }
    }
    key.map(|k| (m, k))
}

/// Whether the held modifiers `got` fire a shortcut that asks for `want`.
fn mods_match(want: Modifiers, got: Modifiers, mac: bool) -> bool {
    // Off macOS egui reports Ctrl as both `ctrl` and `command`: Ctrl is the shortcut's Cmd, and a shortcut's
    // Ctrl (the Mac Control key) has no key of its own there, so Ctrl+X is Cut, not Cmd+Ctrl+X.
    let ctrl = if mac { want.ctrl == got.ctrl } else { !want.ctrl };
    want.command == got.command && want.shift == got.shift && want.alt == got.alt && ctrl
}

/// Cmd+Ctrl+S opens Search. Off macOS Ctrl+S is Save (Cmd+S), so Search is reached from the Window menu.
fn search_chord(m: Modifiers, mac: bool) -> bool {
    mac && m.command && m.ctrl
}

/// Runs the shortcuts typed into `ctx`'s viewport: the main window's, or the separate Mix
/// window's (it calls this too, so every global shortcut works from whichever window has the
/// keyboard). A focused text field or an open dialog keeps its keys.
pub fn handle(app: &mut SoundApp, ctx: &egui::Context) {
    // Cmd+= and Cmd+0 are Mix/Edit and Quantize to Grid, not egui's built-in interface zoom.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    let window = crate::window_sync::window_of(app, ctx);
    // An open dialog takes the keys of the window it is in; the other window keeps its shortcuts.
    let dialog_here = app.dialogs.open.is_some() && crate::window_sync::draws_here(app.window_sync.dialog_home, ctx);
    // Keys typed into Setup › Keyboard Shortcuts become a shortcut; they run nothing.
    let capturing = app.ui.show_shortcuts && app.ui.shortcut_editor.capturing.is_some();
    if ctx.egui_wants_keyboard_input() || dialog_here || capturing {
        return;
    }
    let events: Vec<(Key, Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect()
    });
    // Cmd and Ctrl are separate keys only on macOS (asked at runtime so the web build in a Mac browser agrees).
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    // Typing over the plugin browser goes to its search field, not to the single-key commands.
    let over_browser = app
        .browser_rect
        .filter(|(viewport, _)| *viewport == ctx.viewport_id())
        .zip(ctx.pointer_latest_pos())
        .is_some_and(|((_, r), p)| r.contains(p));
    for (key, mods) in events {
        if over_browser && types_text(key, mods) {
            continue;
        }
        // A wide plugin editor under the pointer handles its own keys (`keyPressed`).
        if app.ui.wide_key_owner.get(&ctx.viewport_id()).is_some_and(|owns| owns(key, mods)) {
            continue;
        }
        // The Vocal Editor under the pointer takes S, J, Escape, Delete and Ctrl+A.
        if crate::vocal_editor::owns_key(app, ctx, key, mods) {
            continue;
        }
        // The MIDI Editor under the pointer takes F (Fit).
        if crate::midi_editor::owns_key(app, ctx, key, mods) {
            continue;
        }
        // Musical Typing, while it is on, plays its letter keys (`musical_typing::handle_keys`).
        if crate::musical_typing::owns_key(app, key, mods) {
            continue;
        }
        if fixed(app, window, key, mods, mac) {
            continue;
        }
        if delete_clears(app, window, key, mods) {
            continue;
        }
        if let Some(id) = command_for(&app.ui.shortcuts, key, mods, mac) {
            if shortcut_dialog(app, id) {
                continue;
            }
            let _ = app.run(id, selection_params(app, window, id));
        } else if let Some(id) = ui_command_for(&app.ui.shortcuts, key, mods, mac) {
            let _ = app.run(id, json!({}));
        }
    }
    // Tab moves the insertion point here, so egui must not also hand focus to the first widget: that would block every shortcut.
    ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    // A track selected from the keyboard scrolls into view in the other window.
    crate::window_sync::watch_selection(app, Some(window));
}

/// A shortcut whose command opens a dialog first: opens it, unless a dialog is already open in the
/// other window (it would replace that one and lose what was typed there): then nothing happens
/// and the status line says why. True when the key is used up.
fn shortcut_dialog(app: &mut SoundApp, id: &str) -> bool {
    let Some(held) = app.dialogs.open.take() else { return crate::menus::invoke_shortcut_dialog(app, id) };
    let opens = crate::menus::invoke_shortcut_dialog(app, id);
    app.dialogs.open = Some(held);
    if opens {
        app.ui.status = "Finish the open dialog first".into();
    }
    opens
}

/// Whether `spec` changes tracks' own state (mute, solo, record, height, commit, delete, colour…):
/// a track or mix command that takes `tracks?` and no time range. Edit-range and automation
/// commands (`start`, `end`, `at`) keep the Edit window's selection, so one action never mixes two.
fn track_state_command(spec: &soundcraft_engine::cmd::CommandSpec) -> bool {
    let p = spec.params;
    (spec.id.starts_with("track.") || spec.id.starts_with("mix."))
        && p.contains("tracks?")
        && !["start", "end", "at?", "at:", "range"].iter().any(|w| p.contains(w))
}

/// The params of a shortcut's command typed into `window`: a track-state command typed into the
/// Mix window with its own selection (Sync Selection off) acts on that selection; others take none.
fn selection_params(app: &SoundApp, window: MainWindow, id: &str) -> serde_json::Value {
    let takes_tracks = soundcraft_engine::command_specs().iter().any(|c| c.id == id && track_state_command(c));
    if crate::window_sync::own_selection(app, window) && takes_tracks {
        let tracks: Vec<u64> = app.window_sync.mix_selection.iter().map(|t| t.0).collect();
        json!({ "tracks": tracks })
    } else {
        json!({})
    }
}

/// Whether `key` with `m` types a character (a letter, digit or punctuation without Cmd, Ctrl or
/// Alt) rather than being a command key (Space, Enter, arrows, F-keys, …).
fn types_text(key: Key, m: Modifiers) -> bool {
    let name = key.name();
    let f_key = name.strip_prefix('F').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    !m.command
        && !m.ctrl
        && !m.alt
        && !f_key
        && !matches!(
            key,
            Key::Space
                | Key::Enter
                | Key::Escape
                | Key::Tab
                | Key::Backspace
                | Key::Delete
                | Key::Insert
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::ArrowUp
                | Key::ArrowDown
                | Key::ArrowLeft
                | Key::ArrowRight
        )
}

/// Plain Delete or Backspace in the Edit window is Clear (Edit › Clear, Cmd+B) for the clip
/// selection: the selected clips, or the edit selection on the selected tracks. Runs after
/// `fixed`, so a live ruler pick (marker, chord) still wins. With nothing to clear the key is left
/// alone, as it is while the MIDI Editor shows with notes selected (it deletes those itself) and in
/// the Mix window (no clips there). Returns true when handled.
fn delete_clears(app: &mut SoundApp, window: MainWindow, key: Key, m: Modifiers) -> bool {
    let plain = !m.command && !m.alt && !m.ctrl && !m.shift;
    // Typed into the Edit window (the main one while Mix is separate), never the Mix window.
    if !(plain && matches!(key, Key::Delete | Key::Backspace)) || window != MainWindow::Edit {
        return false;
    }
    if app.ui.show_midi_editor && !app.midi.selected.is_empty() {
        return false;
    }
    // Selected automation breakpoints go first (automation editing spec, §1).
    if let Some(sel) = app.auto_sel.take().filter(|a| !a.points.is_empty()) {
        let _ = app.run("automation.delete", json!({"track": sel.track.0, "param": sel.param.id(), "points": sel.points}));
        return true;
    }
    let clearable = soundcraft_engine::find_command("edit.clear").is_some_and(|c| (c.enabled)(&app.engine).is_ok());
    if !clearable {
        return false;
    }
    if let Err(e) = app.run("edit.clear", json!({})) {
        app.ui.status = e;
    }
    true
}

/// Whether `id`'s effective shortcut is `key` with `mods`.
fn fires(overrides: &Overrides, id: &str, key: Key, mods: Modifiers, mac: bool) -> bool {
    effective_shortcut(overrides, id).and_then(|s| parse(&s)).is_some_and(|(m, k)| k == key && mods_match(m, mods, mac))
}

/// The engine command whose (effective) shortcut is `key` with `mods`.
fn command_for(overrides: &Overrides, key: Key, mods: Modifiers, mac: bool) -> Option<&'static str> {
    soundcraft_engine::command_specs().iter().map(|c| c.id).find(|id| fires(overrides, id, key, mods, mac))
}

/// The UI command (windows, dialogs) whose (effective) shortcut is `key` with `mods`.
fn ui_command_for(overrides: &Overrides, key: Key, mods: Modifiers, mac: bool) -> Option<&'static str> {
    crate::menus::UI_COMMANDS.iter().map(|c| c.0).find(|id| fires(overrides, id, key, mods, mac))
}

/// Selected automation breakpoints take Cmd+C, Cmd+V (onto the lane the selected track shows, at
/// the edit insertion) and Up/Down (nudge the value by 0.5 dB, or 1 % of the range).
fn automation_keys(app: &mut SoundApp, window: MainWindow, key: Key, m: Modifiers, mac: bool) -> bool {
    if window != MainWindow::Edit {
        return false;
    }
    let plain = !m.command && !m.alt && !m.ctrl && !m.shift;
    let cmd = mods_match(Modifiers::COMMAND, m, mac);
    match key {
        Key::C if cmd => {
            let Some(sel) = app.auto_sel.clone().filter(|a| !a.points.is_empty()) else { return false };
            let _ = app.run("automation.copy", json!({"track": sel.track.0, "param": sel.param.id(), "points": sel.points}));
            true
        }
        Key::V if cmd && !app.engine.clipboard.auto_points.is_empty() => {
            let s = app.engine.session();
            let Some(track) = s.edit.selected_tracks.first().copied() else { return false };
            let Some(param) = s.track(track).and_then(|t| soundcraft_model::AutoParam::parse(&t.view)) else { return false };
            let at = s.edit.selection.start;
            match app.run("automation.paste", json!({"track": track.0, "param": param.id(), "at": at})) {
                Ok(r) => {
                    let points =
                        r.get("points").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_i64()).collect()).unwrap_or_default();
                    app.auto_sel = Some(crate::AutoSelection { track, param, points });
                }
                Err(e) => app.ui.status = e,
            }
            true
        }
        Key::ArrowUp | Key::ArrowDown if plain => {
            let Some(sel) = app.auto_sel.clone().filter(|a| !a.points.is_empty()) else {
                return crate::edit_window::comp_layers::cycle_key(app, key == Key::ArrowDown);
            };
            let (lo, hi, _) = sel.param.range();
            let step = if matches!(
                sel.param,
                soundcraft_model::AutoParam::Volume | soundcraft_model::AutoParam::SendLevel(_) | soundcraft_model::AutoParam::Trim
            ) {
                0.5
            } else if lo.is_finite() && hi.is_finite() {
                (hi - lo) * 0.01
            } else {
                0.01
            };
            let dv = if key == Key::ArrowUp { step } else { -step };
            if let Ok(r) = app.run("automation.move", json!({"track": sel.track.0, "param": sel.param.id(), "points": sel.points, "dv": dv}))
                && let Some(a) = r.get("points").and_then(|v| v.as_array())
                && let Some(cur) = &mut app.auto_sel
            {
                cur.points = a.iter().filter_map(|v| v.as_i64()).collect();
            }
            true
        }
        _ => false,
    }
}

/// Keys with context-dependent meaning, typed into `window`. Returns true when handled.
fn fixed(app: &mut SoundApp, window: MainWindow, key: Key, m: Modifiers, mac: bool) -> bool {
    if automation_keys(app, window, key, m, mac) {
        return true;
    }
    let plain = !m.command && !m.alt && !m.ctrl && !m.shift;
    match key {
        Key::Space if !m.command && !m.alt => {
            let id = if m.shift { "transport.half_speed" } else { "transport.toggle" };
            let _ = app.run(id, json!({}));
            true
        }
        Key::Space if m.command => {
            let _ = app.run("transport.record", json!({}));
            true
        }
        Key::S if search_chord(m, mac) => {
            let _ = app.run("window.search", json!({}));
            true
        }
        Key::Equals if m.command && !(mac && m.ctrl) => {
            let _ = app.run("window.toggle_mix_edit", json!({}));
            true
        }
        // A marker or chord picked on a ruler: Delete (or Backspace) removes it.
        Key::Delete | Key::Backspace if plain && crate::ruler_items::pick_is_live(app) => crate::ruler_items::delete_picked(app),
        Key::Enter if plain => {
            let _ = app.run("markers.add", json!({}));
            true
        }
        // Shift+=/- step the Grid value and Alt+Shift+=/- the Nudge value; macOS sends Alt+Shift+= as =, not +.
        Key::Plus | Key::Equals | Key::Minus if m.shift && !m.command && !m.ctrl && window == MainWindow::Edit => {
            let step = if key == Key::Minus { -1 } else { 1 };
            let _ = app.run(if m.alt { "view.nudge" } else { "view.grid" }, json!({"step": step}));
            true
        }
        Key::F12 if plain => {
            let _ = app.run("transport.record", json!({}));
            true
        }
        Key::F1 | Key::F2 | Key::F3 | Key::F4 if plain => {
            edit_mode_key(app, key);
            true
        }
        Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 if m.alt && !m.command && !m.ctrl && !m.shift => {
            let f = match key {
                Key::Num1 => Key::F1,
                Key::Num2 => Key::F2,
                Key::Num3 => Key::F3,
                _ => Key::F4,
            };
            edit_mode_key(app, f);
            true
        }
        Key::Backtick if plain => {
            cycle_edit_mode(app);
            true
        }
        Key::F5 | Key::F6 | Key::F7 | Key::F8 | Key::F9 | Key::F10 if plain => {
            let tool = match key {
                Key::F5 => "zoom",
                Key::F6 => "trim",
                Key::F7 => "selector",
                Key::F8 => "grabber",
                Key::F9 => "scrubber",
                _ => "pencil",
            };
            let _ = app.run("edit.tool", json!({"tool": tool}));
            true
        }
        Key::Plus | Key::Minus if !m.command && window == MainWindow::Edit => {
            let dir = if key == Key::Plus { 1 } else { -1 };
            let _ = app.run("edit.nudge", json!({"direction": dir}));
            true
        }
        // Alt+Tab goes back (off macOS the desktop takes Alt+Tab, so Ctrl+Tab does) and Shift extends the selection.
        // Cmd+Alt+Tab is Tab to Transient, and the Mac's Control+Tab is Pro Tools' Go to Next Clip.
        Key::Tab if !(m.command && (mac || m.alt)) && !(mac && m.ctrl) => {
            let _ = app.run("edit.tab", json!({"back": m.alt || (!mac && m.ctrl), "extend": m.shift}));
            true
        }
        // The registry's key families "Cmd+Alt+[ / ]", "Cmd+Alt+0-5" and "Cmd+Ctrl+1-5", which `parse` can't read.
        Key::OpenBracket | Key::CloseBracket if mods_match(Modifiers::COMMAND | Modifiers::ALT, m, mac) => {
            let factor = if key == Key::CloseBracket { 2.0 } else { 0.5 };
            let _ = app.run("view.waveform_zoom", json!({"factor": factor}));
            true
        }
        Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 if mods_match(Modifiers::COMMAND | Modifiers::ALT, m, mac) => {
            let _ = app.run("clip.rating", json!({"rating": digit(key)}));
            true
        }
        Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 if mods_match(Modifiers::COMMAND | Modifiers::CTRL, m, mac) => {
            let _ = app.run("view.zoom_preset", json!({"preset": digit(key)}));
            true
        }
        // Pro Tools' Control+Up/Down track height, on Ctrl off macOS too (its Start key isn't available there).
        Key::ArrowUp | Key::ArrowDown if m.ctrl && (m.alt || !m.shift) => {
            let own = crate::window_sync::selection_of(app, window);
            let tracks = crate::edit_window::height_targets(app.engine.session(), m, &own);
            let id = if key == Key::ArrowUp { "track.height_up" } else { "track.height_down" };
            // The Mix window's own selection, even empty, never falls back to the shared one.
            if crate::window_sync::own_selection(app, window) {
                if !tracks.is_empty() {
                    let _ = app.run(id, json!({"tracks": tracks}));
                }
            } else {
                let _ = app.run(id, if tracks.is_empty() { json!({}) } else { json!({"tracks": tracks}) });
            }
            true
        }
        // Extend Edit Down is Control+Shift+;, which macOS sends as Control+:.
        Key::Semicolon | Key::Colon if mac && m.ctrl && m.shift && !m.alt && !m.command => {
            let _ = app.run("edit.extend_selection_down", json!({}));
            true
        }
        // Ctrl+Shift+Up/Down nudge clip gain off macOS too (the registry's Ctrl is the Mac Control key).
        Key::ArrowUp | Key::ArrowDown if !mac && m.ctrl && m.shift && !m.alt => {
            let id = if key == Key::ArrowUp { "clip.gain_nudge_up" } else { "clip.gain_nudge_down" };
            let _ = app.run(id, json!({}));
            true
        }
        // L with a range selected and the Lyrics ruler shown: place the next unplaced lyric line there.
        Key::L if plain && window == MainWindow::Edit && lyric_place_ready(app) => {
            let _ = app.run("lyrics.place", json!({}));
            true
        }
        // Commands Keyboard Focus: single-key editing (Edit window only).
        k if !m.command
            && !m.alt
            && !m.ctrl
            && window == MainWindow::Edit
            && app.engine.session().edit.keyboard_focus == "commands"
            && commands_focus(app, k, m.shift) =>
        {
            true
        }
        Key::R | Key::T if plain => {
            // Zoom out / in on the timeline (Commands Focus style).
            let id = if key == Key::T { "view.zoom_in" } else { "view.zoom_out" };
            let _ = app.run(id, json!({}));
            true
        }
        _ => false,
    }
}

/// F1–F4 (and Alt+1–4) choose Shuffle, Slip, Spot and Grid; F4 in Grid switches between Absolute and
/// Relative unless Edit/Tool Mode Keyboard Lock is on. Holding F4 with F1–F3, in either order, adds Snap to Grid.
fn edit_mode_key(app: &mut SoundApp, pressed: Key) {
    let ctx = app.docking_context.clone();
    let held = |k: Key| ctx.as_ref().is_some_and(|c| c.input(|i| i.key_down(k)));
    let (key, snap) = match [Key::F1, Key::F2, Key::F3].into_iter().find(|k| *k != pressed && held(*k)) {
        Some(k) if pressed == Key::F4 => (k, true),
        _ => (pressed, pressed != Key::F4 && held(Key::F4)),
    };
    let ed = &app.engine.session().edit;
    let keep_grid = ed.edit_mode.is_grid() && ed.flag("options.edit_tool_mode_keyboard_lock");
    // F4 pressed first in an F4+F1–F3 chord switched the Grid mode on its own; put that back.
    let flipped = egui::Id::new("edit_mode_key_grid_before_f4");
    let before =
        (ed.edit_mode.is_grid() && !keep_grid).then(|| if ed.flag(soundcraft_engine::edit::RELATIVE_GRID) { "relative_grid" } else { "grid" });
    let restore = match &ctx {
        Some(c) if pressed == Key::F4 => {
            c.data_mut(|d| d.insert_temp(flipped, before));
            None
        }
        Some(c) if snap => c.data_mut(|d| d.remove_temp::<Option<&'static str>>(flipped)).flatten(),
        _ => None,
    };
    let mode = match key {
        Key::F1 => "shuffle",
        Key::F2 => "slip",
        Key::F3 => "spot",
        _ if keep_grid => return,
        _ => grid_mode(app),
    };
    if let Some(grid) = restore {
        let _ = app.run("edit.mode", json!({"mode": grid}));
    }
    let _ = app.run("edit.mode", json!({"mode": mode, "snap_to_grid": snap}));
}

/// What choosing Grid picks: in Grid the other Grid mode, from another mode the one used last.
pub(crate) fn grid_mode(app: &SoundApp) -> &'static str {
    use soundcraft_model::EditMode;
    let ed = &app.engine.session().edit;
    match ed.edit_mode {
        EditMode::Grid => "relative_grid",
        EditMode::GridRelative => "grid",
        _ if ed.flag(soundcraft_engine::edit::RELATIVE_GRID) => "relative_grid",
        _ => "grid",
    }
}

/// ` steps through the Edit modes in F-key order, past a locked Shuffle.
fn cycle_edit_mode(app: &mut SoundApp) {
    use soundcraft_model::EditMode;
    let ed = &app.engine.session().edit;
    let mode = match ed.edit_mode {
        EditMode::Shuffle => "slip",
        EditMode::Slip => "spot",
        EditMode::Spot => grid_mode(app),
        _ if ed.flag(soundcraft_engine::edit::SHUFFLE_LOCK) => "slip",
        _ => "shuffle",
    };
    let _ = app.run("edit.mode", json!({"mode": mode}));
}

/// Whether L places a lyric line: the Lyrics ruler is shown, a range is selected and a lyric line
/// is not placed yet.
fn lyric_place_ready(app: &SoundApp) -> bool {
    let s = app.engine.session();
    s.edit.rulers.iter().any(|r| r == "lyrics")
        && s.edit.selection.end > s.edit.selection.start
        && !soundcraft_engine::cmd::lyric_place::unplaced(s).is_empty()
}

/// Single-key commands. Returns true when `key` was handled.
fn commands_focus(app: &mut SoundApp, key: Key, shift: bool) -> bool {
    let id = match key {
        Key::Z if shift => "edit.redo",
        Key::P if shift => "edit.extend_selection_up",
        Key::Semicolon | Key::Colon if shift => "edit.extend_selection_down",
        _ if shift => return false,
        Key::A => "edit.trim_start_to_insertion",
        Key::S => "edit.trim_end_to_insertion",
        Key::D => "edit.fade_to_start",
        Key::G => "edit.fade_to_end",
        Key::F => "edit.fades_create",
        Key::Comma | Key::Period | Key::M | Key::Slash => {
            let direction = if matches!(key, Key::Period | Key::Slash) { 1 } else { -1 };
            let _ = app.run("edit.nudge", json!({"direction": direction, "next": matches!(key, Key::M | Key::Slash)}));
            return true;
        }
        Key::Num0 => "edit.timeline_to_selection",
        Key::O => "edit.selection_to_timeline",
        Key::OpenBracket => "transport.play_selection",
        Key::CloseBracket => "edit.play_timeline",
        Key::B => "edit.separate",
        Key::X => "edit.cut",
        Key::C => "edit.copy",
        Key::V => "edit.paste",
        Key::Z => "edit.undo",
        Key::N => "options.insertion_follows_playback",
        Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 => {
            let _ = app.run("view.zoom_preset", json!({"preset": digit(key)}));
            return true;
        }
        Key::P => {
            move_selection_track(app, -1);
            return true;
        }
        Key::Semicolon => {
            move_selection_track(app, 1);
            return true;
        }
        Key::L | Key::Quote => {
            tab_clip(app, key == Key::L);
            return true;
        }
        Key::E => {
            crate::edit_window::zoom_toggle(app);
            return true;
        }
        _ => return false,
    };
    let _ = app.run(id, json!({}));
    true
}

/// Move the edit selection to the track above/below (P / ;).
fn move_selection_track(app: &mut SoundApp, dir: i64) {
    let s = app.engine.session();
    let visible: Vec<u64> = s.tracks.iter().filter(|t| !t.hidden).map(|t| t.id.0).collect();
    let Some(cur) = s.edit.selected_tracks.first().map(|t| t.0) else { return };
    let Some(i) = visible.iter().position(|t| *t == cur) else { return };
    let j = (i as i64 + dir).clamp(0, visible.len() as i64 - 1) as usize;
    if let Some(t) = visible.get(j) {
        let sel = s.edit.selection;
        let _ = app.run("edit.select", json!({"tracks": [t], "start": sel.start, "end": sel.end, "exact": true}));
    }
}

/// Tab to the previous/next clip boundary on the selected tracks (L / ').
fn tab_clip(app: &mut SoundApp, back: bool) {
    let s = app.engine.session();
    let at = s.edit.selection.start;
    let mut edges: Vec<i64> =
        s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).flat_map(|t| t.clips().iter().flat_map(|c| [c.start, c.end()])).collect();
    edges.sort_unstable();
    let next = if back { edges.iter().rev().find(|e| **e < at).copied() } else { edges.iter().find(|e| **e > at).copied() };
    if let Some(n) = next {
        let _ = app.run("transport.locate", json!({"at": n}));
    }
}

fn digit(key: Key) -> u8 {
    key.name().parse().unwrap_or(0)
}

/// Shortcut text for display. The registry stores macOS-style strings ("Cmd+N"); off macOS the Cmd
/// modifier is the Ctrl key. Mac-only Control chords ("Cmd+Ctrl+S") have no key there, so they show nothing.
pub fn shortcut_label(s: &str, mac: bool) -> String {
    let has = |m: &str| s.split('+').any(|p| p == m);
    if mac || (!has("Cmd") && !has("Ctrl")) || PC_CTRL_CHORDS.contains(&s) {
        return s.to_string();
    }
    if has("Ctrl") {
        return String::new();
    }
    s.split('+').map(|p| if p == "Cmd" { "Ctrl" } else { p }).collect::<Vec<_>>().join("+")
}

/// Mac Control chords that `fixed` also maps to Ctrl off macOS.
const PC_CTRL_CHORDS: [&str; 4] = ["Ctrl+Up", "Ctrl+Down", "Ctrl+Shift+Up", "Ctrl+Shift+Down"];

#[cfg(test)]
mod label_tests {
    use super::shortcut_label;

    #[test]
    fn cmd_becomes_ctrl_off_mac() {
        assert_eq!(shortcut_label("Cmd+N", false), "Ctrl+N");
        assert_eq!(shortcut_label("Cmd+Shift+W", false), "Ctrl+Shift+W");
        assert_eq!(shortcut_label("Cmd+Alt+B", false), "Ctrl+Alt+B");
        assert_eq!(shortcut_label("Cmd+,", false), "Ctrl+,");
        assert_eq!(shortcut_label("Shift+R", false), "Shift+R");
        assert_eq!(shortcut_label("Cmd+Ctrl+S", false), "");
        assert_eq!(shortcut_label("", false), "");
        assert_eq!(shortcut_label("Ctrl+Up", false), "Ctrl+Up");
        assert_eq!(shortcut_label("Ctrl+Shift+Down", false), "Ctrl+Shift+Down");
        assert_eq!(shortcut_label("Ctrl+Shift+P", false), "");
        assert_eq!(shortcut_label("Ctrl+Alt+L", false), "");
    }

    #[test]
    fn unchanged_on_mac() {
        for s in ["Cmd+N", "Cmd+Shift+W", "Cmd+Alt+B", "Shift+R", "Cmd+Ctrl+S"] {
            assert_eq!(shortcut_label(s, true), s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    /// Ctrl held on Windows or Linux, as egui reports it.
    const PC_CTRL: Modifiers = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    /// Cmd held on macOS.
    const MAC_CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };

    fn want(s: &str) -> Modifiers {
        parse(s).map(|(m, _)| m).unwrap()
    }

    #[test]
    fn ctrl_is_cmd_off_macos() {
        assert!(mods_match(want("Cmd+S"), PC_CTRL, false));
        assert!(mods_match(want("Cmd+Shift+N"), Modifiers { shift: true, ..PC_CTRL }, false));
        assert!(mods_match(want("Cmd+X"), PC_CTRL, false));
        assert!(!mods_match(want("Cmd+Ctrl+X"), PC_CTRL, false));
        assert!(!mods_match(want("Cmd+S"), Modifiers::NONE, false));
        assert!(!search_chord(PC_CTRL, false));
    }

    #[test]
    fn cmd_and_ctrl_stay_apart_on_macos() {
        assert!(mods_match(want("Cmd+S"), MAC_CMD, true));
        assert!(!mods_match(want("Cmd+S"), Modifiers::CTRL, true));
        assert!(mods_match(want("Cmd+Ctrl+X"), Modifiers { ctrl: true, ..MAC_CMD }, true));
        assert!(!mods_match(want("Cmd+X"), Modifiers { ctrl: true, ..MAC_CMD }, true), "Cmd+Ctrl+X is not Cut");
        assert!(!search_chord(MAC_CMD, true));
        assert!(search_chord(Modifiers { ctrl: true, ..MAC_CMD }, true));
    }

    #[test]
    fn shift_cmd_k_prompts_for_a_folder_to_export_clips() {
        let none = Overrides::new();
        assert_eq!(command_for(&none, Key::K, Modifiers { shift: true, ..MAC_CMD }, true), Some("file.export_clips"));
        assert_eq!(command_for(&none, Key::K, Modifiers { shift: true, ..PC_CTRL }, false), Some("file.export_clips"));
        assert!(crate::dialogs::takes_path("file.export_clips"));
    }

    fn overrides(pairs: &[(&str, &str)]) -> Overrides {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn effective_shortcut_prefers_the_override() {
        let o = overrides(&[("session.save", "Cmd+Alt+Shift+Y"), ("file.export_clips", "")]);
        assert_eq!(effective_shortcut(&o, "session.save").as_deref(), Some("Cmd+Alt+Shift+Y"));
        assert_eq!(effective_shortcut(&o, "file.export_clips"), None, "\"\" means no shortcut");
        assert_eq!(effective_shortcut(&o, "track.new").as_deref(), Some("Cmd+Shift+N"), "missing: the registry's");
        assert_eq!(effective_shortcut(&o, "window.search").as_deref(), Some("Cmd+Ctrl+S"), "UI commands too");
    }

    #[test]
    fn overrides_round_trip_through_ui_json() {
        let ui = crate::UiState { shortcuts: overrides(&[("session.save", "Cmd+Alt+Shift+Y"), ("file.export_clips", "")]), ..Default::default() };
        let text = serde_json::to_string(&ui).unwrap();
        let back: crate::UiState = serde_json::from_str(&text).unwrap();
        assert_eq!(back.shortcuts, ui.shortcuts);
        // A ui.json saved before shortcuts were editable loads with none.
        let mut old = serde_json::to_value(crate::UiState::default()).unwrap();
        old.as_object_mut().unwrap().remove("shortcuts");
        assert!(serde_json::from_value::<crate::UiState>(old).unwrap().shortcuts.is_empty());
    }

    #[test]
    fn the_key_handler_fires_the_override_not_the_default() {
        let o = overrides(&[("session.save", "Cmd+Alt+Shift+Y")]);
        let y = Modifiers { alt: true, shift: true, ..MAC_CMD };
        assert_eq!(command_for(&o, Key::Y, y, true), Some("session.save"));
        assert_eq!(command_for(&o, Key::Y, Modifiers { alt: true, shift: true, ..PC_CTRL }, false), Some("session.save"));
        assert_eq!(command_for(&o, Key::S, MAC_CMD, true), None, "Cmd+S no longer saves");
        // A disabled shortcut fires nothing.
        let off = overrides(&[("file.export_clips", "")]);
        assert_eq!(command_for(&off, Key::K, Modifiers { shift: true, ..MAC_CMD }, true), None);
        // UI commands too.
        let ui = overrides(&[("window.configurations", "Cmd+Alt+Shift+Y")]);
        assert_eq!(ui_command_for(&ui, Key::Y, y, true), Some("window.configurations"));
        assert_eq!(ui_command_for(&ui, Key::J, Modifiers { alt: true, ..MAC_CMD }, true), None);
    }

    #[test]
    fn typed_keys_name_themselves() {
        for key in Key::ALL {
            for m in [Modifiers::NONE, MAC_CMD, Modifiers { shift: true, alt: true, ..MAC_CMD }, Modifiers { ctrl: true, ..MAC_CMD }] {
                if let Some(s) = shortcut_string(m, *key, true) {
                    let (pm, pk) = parse(&s).unwrap();
                    assert_eq!(pk, *key, "{s}");
                    assert_eq!((pm.command, pm.ctrl, pm.alt, pm.shift), (m.command, m.ctrl, m.alt, m.shift), "{s}");
                }
            }
        }
        assert_eq!(shortcut_string(Modifiers { shift: true, ..MAC_CMD }, Key::OpenBracket, true).as_deref(), Some("Cmd+Shift+["));
        assert_eq!(shortcut_string(Modifiers { alt: true, ..PC_CTRL }, Key::K, false).as_deref(), Some("Cmd+Alt+K"));
        assert_eq!(shortcut_string(Modifiers::NONE, Key::Backtick, true).as_deref(), Some("`"));
        assert!(same_shortcut("Shift+Cmd+K", "Cmd+Shift+K"));
    }

    #[test]
    fn registry_defaults_are_editable_or_fixed() {
        // Every default either parses (and so can be edited) or is a key family shown as fixed.
        for (id, _) in all_commands() {
            if let Some(d) = default_shortcut(id)
                && parse(d).is_none()
            {
                assert!(!editable(id), "{id}: {d}");
            }
        }
        assert!(editable("session.save"));
        assert!(!editable("track.height_up"), "Ctrl+Up is handled as a context key");
    }

    #[test]
    fn set_override_keeps_only_changes() {
        let mut o = Overrides::new();
        set_override(&mut o, "session.save", Some("Cmd+S"));
        assert!(o.is_empty(), "the default is no override");
        set_override(&mut o, "session.save", Some(""));
        assert_eq!(o.get("session.save").map(String::as_str), Some(""));
        set_override(&mut o, "session.save", None);
        assert!(o.is_empty());
    }
}
