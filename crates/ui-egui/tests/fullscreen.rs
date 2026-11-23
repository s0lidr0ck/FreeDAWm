//! Window › Full Screen (`window.fullscreen`): the command, its menu item, its shortcut, and Esc
//! leaving it alone. The window itself is eframe's; here the emitted viewport command is checked.

use egui::{Event, Key, Modifiers, RawInput, ViewportCommand, ViewportId, ViewportInfo};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, fullscreen, menus, shortcuts};

fn rig() -> (egui::Context, SoundApp) {
    let ctx = egui::Context::default();
    // The registry's default key is the platform's own (Ctrl+Cmd+F on macOS).
    ctx.set_os(if cfg!(target_os = "macos") { egui::os::OperatingSystem::Mac } else { egui::os::OperatingSystem::Nix });
    (ctx, SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default()))
}

fn keys(key: Key, modifiers: Modifiers) -> Vec<Event> {
    let presses = [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    [Event::ModifiersChanged(modifiers)].into_iter().chain(presses).collect()
}

/// One whole frame (logic and the window), at egui time `time`, with `events` and the main window
/// reporting `reported` as its full-screen state. Returns the main window's viewport commands.
fn frame(ctx: &egui::Context, app: &mut SoundApp, time: f64, events: Vec<Event>, reported: Option<bool>) -> Vec<ViewportCommand> {
    let info = ViewportInfo { fullscreen: reported, ..Default::default() };
    let raw = RawInput { time: Some(time), events, viewports: [(ViewportId::ROOT, info)].into_iter().collect(), ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    out.viewport_output.get(&ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default()
}

fn fullscreen_cmds(cmds: &[ViewportCommand]) -> Vec<bool> {
    cmds.iter()
        .filter_map(|c| match c {
            ViewportCommand::Fullscreen(on) => Some(*on),
            _ => None,
        })
        .collect()
}

#[test]
fn the_command_toggles_the_state_and_tells_the_main_window() {
    let (ctx, mut app) = rig();
    // Fonts settle over the first frames; the app opens windowed and says nothing about it.
    for t in 0..3 {
        assert!(fullscreen_cmds(&frame(&ctx, &mut app, t as f64 * 0.1, vec![], Some(false))).is_empty());
    }
    assert!(!app.ui.fullscreen);
    assert_eq!(app.run("window.fullscreen", json!({})).unwrap(), json!({"value": true}));
    assert!(app.ui.fullscreen);
    assert_eq!(fullscreen_cmds(&frame(&ctx, &mut app, 1.0, vec![], Some(false))), [true]);
    // Sent once, not every frame, while the window catches up.
    assert!(fullscreen_cmds(&frame(&ctx, &mut app, 1.1, vec![], Some(false))).is_empty());
    assert!(fullscreen_cmds(&frame(&ctx, &mut app, 1.2, vec![], Some(true))).is_empty());
    assert!(app.ui.fullscreen, "the window still reporting the old state is not a change");
    let seen = app.inspect(&ctx);
    assert_eq!(seen["fullscreen"], json!(true));
    assert_eq!(seen["fullscreen_hint"], json!(true));
    assert_eq!(app.ui.status, fullscreen::hint_text(&app, false));
    if !cfg!(target_os = "macos") {
        assert_eq!(app.ui.status, "Press Shift+F11 to leave full screen");
    }
    // The hint goes after a couple of seconds.
    frame(&ctx, &mut app, 4.0, vec![], Some(true));
    assert_eq!(app.inspect(&ctx)["fullscreen_hint"], json!(false));
    // Toggled again: back to a window. An explicit value sets it.
    assert_eq!(app.run("window.fullscreen", json!({})).unwrap(), json!({"value": false}));
    assert_eq!(fullscreen_cmds(&frame(&ctx, &mut app, 4.1, vec![], Some(true))), [false]);
    app.run("window.fullscreen", json!({"value": false})).unwrap();
    assert!(fullscreen_cmds(&frame(&ctx, &mut app, 4.2, vec![], Some(false))).is_empty());
    assert_eq!(app.inspect(&ctx)["fullscreen"], json!(false));
}

#[test]
fn the_window_leaving_full_screen_by_itself_is_followed() {
    // macOS's green button or its own View › Exit Full Screen change the window, not the command.
    let (ctx, mut app) = rig();
    app.run("window.fullscreen", json!({"value": true})).unwrap();
    frame(&ctx, &mut app, 0.0, vec![], Some(false));
    frame(&ctx, &mut app, 0.1, vec![], Some(true));
    let cmds = frame(&ctx, &mut app, 0.2, vec![], Some(false));
    assert!(!app.ui.fullscreen, "the checkbox follows the window");
    assert!(fullscreen_cmds(&cmds).is_empty(), "and the app does not fight it");
    // And entering by the system is followed too.
    frame(&ctx, &mut app, 0.3, vec![], Some(true));
    assert!(app.ui.fullscreen);
}

#[test]
fn the_full_screen_item_is_a_check_item_in_the_window_menu_with_its_shortcut() {
    let (_, mut app) = rig();
    let tree = menus::tree();
    let window = tree.iter().find(|n| n.label == "Window").unwrap();
    let labels: Vec<&str> = window.children.iter().map(|n| n.label.as_str()).take(4).collect();
    assert_eq!(labels[1..], ["Mix", "Edit", "Full Screen"], "in the Main windows group, after Mix and Edit");
    let extra = menus::ui_aliases();
    let id = menus::command_for_path("Window > Full Screen", &extra);
    assert_eq!(id.as_deref(), Some("window.fullscreen"));
    let path = "Window > Full Screen";
    assert_eq!(menus::item_state(&app, path, id.as_deref()), (true, false));
    app.run("window.fullscreen", json!({})).unwrap();
    assert_eq!(menus::item_state(&app, path, id.as_deref()), (true, true), "checked while full screen");
    if !cfg!(target_os = "macos") {
        assert_eq!(menus::shortcut_text(&app, "window.fullscreen", false), "Shift+F11");
    }
    assert_eq!(menus::shortcut_text(&app, "window.fullscreen", true), shortcuts::shortcut_label(fullscreen::SHORTCUT, true));
}

#[test]
fn the_shortcut_is_free_toggles_full_screen_and_can_be_moved() {
    let (ctx, mut app) = rig();
    let none = shortcuts::Overrides::new();
    assert_eq!(shortcuts::default_shortcut("window.fullscreen"), Some(fullscreen::SHORTCUT));
    assert!(shortcuts::editable("window.fullscreen"));
    assert_eq!(shortcuts::holder(&none, fullscreen::SHORTCUT, "window.fullscreen"), None, "no other command or fixed key has it");
    // Not plain F11: in a session with a control room that is the talkback key.
    assert!(!shortcuts::same_shortcut(fullscreen::SHORTCUT, "F11"));
    let (mods, key) = shortcuts::parse(fullscreen::SHORTCUT).unwrap();
    // The window takes keys once its fonts are in (the second frame).
    for t in 0..3 {
        frame(&ctx, &mut app, -1.0 + t as f64 * 0.1, vec![], Some(false));
    }
    frame(&ctx, &mut app, 0.0, keys(key, mods), Some(false));
    assert!(app.ui.fullscreen);
    frame(&ctx, &mut app, 0.1, keys(key, mods), Some(true));
    assert!(!app.ui.fullscreen, "the same key brings the window back");
    // Moved to plain F11 in Setup › Keyboard Shortcuts: F11 toggles, the old key does nothing.
    shortcuts::set_override(&mut app.ui.shortcuts, "window.fullscreen", Some("F11"));
    frame(&ctx, &mut app, 0.2, keys(key, mods), Some(false));
    assert!(!app.ui.fullscreen);
    frame(&ctx, &mut app, 0.3, keys(Key::F11, Modifiers::NONE), Some(false));
    assert!(app.ui.fullscreen);
    assert_eq!(app.ui.status, "Press F11 to leave full screen", "the hint names the key in effect");
    // Removed: the hint names the menu item instead.
    shortcuts::set_override(&mut app.ui.shortcuts, "window.fullscreen", Some(""));
    assert_eq!(fullscreen::hint_text(&app, false), "Window › Full Screen leaves full screen");
}

#[test]
fn escape_does_not_leave_full_screen() {
    let (ctx, mut app) = rig();
    for t in 0..3 {
        frame(&ctx, &mut app, t as f64 * 0.1, vec![], Some(false));
    }
    app.run("window.fullscreen", json!({})).unwrap();
    frame(&ctx, &mut app, 1.0, vec![], Some(false));
    for t in 0..3 {
        let cmds = frame(&ctx, &mut app, 1.1 + t as f64 * 0.1, keys(Key::Escape, Modifiers::NONE), Some(true));
        assert!(fullscreen_cmds(&cmds).is_empty());
    }
    assert!(app.ui.fullscreen);
}

#[test]
fn full_screen_is_never_saved() {
    let (_, mut app) = rig();
    app.run("window.fullscreen", json!({"value": true})).unwrap();
    let saved = serde_json::to_value(&app.ui).unwrap();
    assert!(saved.get("fullscreen").is_none());
    let back: soundcraft_ui_egui::UiState = serde_json::from_value(saved).unwrap();
    assert!(!back.fullscreen, "FreeDAWm opens windowed");
}
