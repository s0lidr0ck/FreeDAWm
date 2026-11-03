//! Window › Vocal Editor in Separate Window (and the title row's Window button): the editor
//! leaves the Edit window's dock for its own window (embedded as a floating window here, where
//! the test context has no second OS window), keeps following the Edit window, keeps its keys,
//! remembers the window in ui.json, and docks back when that window closes.

mod vocal_support;

use egui::{Key, Modifiers};
use serde_json::json;
use soundcraft_ui_egui::docking::Panel;
use soundcraft_ui_egui::vocal_editor::window::separate_closed;
use vocal_support::*;

fn docked(app: &soundcraft_ui_egui::SoundApp) -> bool {
    app.ui.docking.layout.contains(&Panel::Vocal)
}

#[test]
fn the_menu_command_moves_the_editor_to_its_own_window_and_closing_docks_it_back() {
    let Setup { ctx, mut app, .. } = setup();
    assert!(docked(&app));
    let docked_rect = view(&ctx).rect;
    app.run("window.vocal_separate", json!({})).unwrap();
    settle(&ctx, &mut app);
    assert!(app.ui.vocal_separate && app.ui.show_vocal_editor);
    assert!(!docked(&app), "the dock's Vocal Editor tab is gone");
    let own = view(&ctx).rect;
    assert_ne!(own, docked_rect, "the roll is drawn in the window now");
    // Closing the window docks it back.
    separate_closed(&mut app);
    settle(&ctx, &mut app);
    assert!(!app.ui.vocal_separate && app.ui.show_vocal_editor);
    assert!(docked(&app), "back in the dock");
}

#[test]
fn the_title_row_button_toggles_it() {
    let Setup { ctx, mut app, .. } = setup();
    click(&ctx, &mut app, control(&ctx, "separate").unwrap().center());
    assert!(app.ui.vocal_separate);
    assert!(!docked(&app));
    // Its button now reads Dock, in the window's title row.
    click(&ctx, &mut app, control(&ctx, "separate").unwrap().center());
    settle(&ctx, &mut app);
    assert!(!app.ui.vocal_separate);
    assert!(docked(&app));
}

#[test]
fn in_its_window_it_follows_the_edit_window_and_space_works() {
    let Setup { ctx, mut app, track, .. } = setup();
    app.run("window.vocal_separate", json!({})).unwrap();
    settle(&ctx, &mut app);
    let t0 = view(&ctx).t0;
    app.run("view.scroll", json!({"to": i64::from(RATE) / 2})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(view(&ctx).t0 - t0, i64::from(RATE) / 2, "the roll moves with the Edit window's scroll");
    app.run("view.scroll", json!({"to": 0})).unwrap();
    settle(&ctx, &mut app);
    // Space over the selected notes plays them.
    let ns = notes(&app, track);
    app.vocal.selected = [ns[0]].into_iter().collect();
    settle(&ctx, &mut app);
    let (s0, e0) = span(&app, ns[0]);
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::Space, Modifiers::NONE);
    assert!(app.is_playing());
    let pos = app.engine.transport.position;
    assert!(pos >= s0 && pos < e0, "from the selected note: {pos} in {s0}..{e0}");
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::Space, Modifiers::NONE);
    assert!(!app.is_playing());
}

#[test]
fn the_window_is_remembered_in_ui_json() {
    let Setup { mut app, .. } = setup();
    app.run("window.vocal_separate", json!({})).unwrap();
    app.ui.vocal_window = Some([100.0, 80.0, 900.0, 600.0]);
    let text = serde_json::to_string(&app.ui).unwrap();
    assert!(text.contains("\"vocal.separate\":true"), "{text}");
    assert!(text.contains("\"vocal.window\":[100.0,80.0,900.0,600.0]"), "{text}");
    let back: soundcraft_ui_egui::UiState = serde_json::from_str(&text).unwrap();
    assert!(back.vocal_separate);
    assert_eq!(back.vocal_window, Some([100.0, 80.0, 900.0, 600.0]));
    let old: soundcraft_ui_egui::UiState =
        serde_json::from_str(&text.replace("\"vocal.separate\":true,", "").replace("\"vocal.window\":[100.0,80.0,900.0,600.0],", "")).unwrap();
    assert!(!old.vocal_separate);
    assert_eq!(old.vocal_window, None);
}
