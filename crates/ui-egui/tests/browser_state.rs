//! Browser state in `UiState`: favourites, recents and their persistence.
mod editor_support;

use editor_support::app_with_tracks;
use serde_json::json;
use soundcraft_ui_egui::UiState;
use soundcraft_ui_egui::browser::{BrowserState, BrowserTab, note_inserted};

#[test]
fn recent_caps_at_30_and_dedups() {
    let mut b = BrowserState::default();
    for i in 0..40 {
        note_inserted(&mut b, &format!("p{i}"));
    }
    assert_eq!(b.recent.len(), 30);
    assert_eq!(b.recent[0], "p39");
    note_inserted(&mut b, "p20");
    assert_eq!(b.recent.len(), 30);
    assert_eq!(b.recent[0], "p20");
    assert_eq!(b.recent.iter().filter(|p| *p == "p20").count(), 1);
}

#[test]
fn inserting_via_menu_updates_recent() {
    let mut app = app_with_tracks(&["Pad"]);
    app.run("mix.module_add", json!({"track": "Pad", "plugin": "compressor"})).unwrap();
    assert_eq!(app.ui.browser.recent[0], "compressor");
    assert!(app.take_recent_changed());
    // A failed insert is not a recent.
    assert!(app.run("mix.module_add", json!({"track": "Pad", "plugin": "no_such_plugin"})).is_err());
    assert_eq!(app.ui.browser.recent[0], "compressor");
}

#[test]
fn old_ui_json_without_browser_loads() {
    let ui: UiState = serde_json::from_str("{}").unwrap();
    assert!(!ui.show_plugin_browser);
    assert_eq!(ui.browser.tab, BrowserTab::Instruments);
    assert!(ui.browser.favourites.is_empty() && ui.browser.recent.is_empty());
    let partial: UiState = serde_json::from_str(r#"{"browser":{"favourites":["a"]}}"#).unwrap();
    assert_eq!(partial.browser.favourites, ["a"]);
}

#[test]
fn favourite_toggle_round_trips_through_serde() {
    let mut ui = UiState::default();
    ui.browser.favourites.push("fs_cleanq".into());
    ui.browser.category_overrides.insert("x".into(), soundcraft_ui_egui::browser::catalog::BrowserCategory::Reverb);
    ui.show_plugin_browser = true;
    let back: UiState = serde_json::from_str(&serde_json::to_string(&ui).unwrap()).unwrap();
    assert_eq!(back.browser.favourites, ["fs_cleanq"]);
    assert_eq!(back.browser.category_overrides.len(), 1);
    assert!(back.show_plugin_browser);
}
