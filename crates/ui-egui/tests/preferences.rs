//! Setup › Preferences…: the window opens on a tab from the menu or `window.preferences`, and its
//! controls run the same commands as the menus (so both always agree).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::preferences::HOLD_UNTIL_CLEARED;
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn app() -> SoundApp {
    SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h);
    h
}

fn steps(h: &mut Harness<'_, SoundApp>) {
    for _ in 0..6 {
        h.step();
    }
}

#[test]
fn the_menu_opens_preferences_on_a_tab() {
    let mut app = app();
    let extra = menus::ui_aliases();
    assert_eq!(menus::command_for_path("Setup > Preferences...", &extra).as_deref(), Some("window.preferences"));
    menus::invoke_menu(&mut app, "window.preferences", "Setup > Preferences...");
    assert!(app.ui.show_preferences);
    assert_eq!(menus::item_state(&app, "Setup > Preferences...", Some("window.preferences")), (true, true));
    let r = app.run("window.preferences", json!({"tab": "Metering"})).unwrap();
    assert_eq!((r["tab"].as_str(), r["value"].as_bool()), (Some("metering"), Some(true)));
    assert!(app.run("window.preferences", json!({"tab": "Sounds"})).is_err());
    app.run("window.preferences", json!({"value": false})).unwrap();
    assert!(!app.ui.show_preferences);
}

#[test]
fn its_controls_run_the_menu_commands() {
    let mut a = app();
    a.run("window.preferences", json!({"tab": "operation"})).unwrap();
    let mut h = harness(a);
    let before = h.state().engine.session().edit.loop_record;
    h.get_by_label("Loop Record").click();
    steps(&mut h);
    assert_ne!(h.state().engine.session().edit.loop_record, before);

    // The Editing tab: Mirrored MIDI Editing.
    h.get_by_label("Editing").click();
    steps(&mut h);
    let before = h.state().engine.session().edit.mirrored_midi;
    h.query_all_by_label("Mirrored MIDI Editing").next().unwrap().click();
    steps(&mut h);
    assert_ne!(h.state().engine.session().edit.mirrored_midi, before);

    // The Mixing tab: Narrow Mix is the View menu's.
    h.get_by_label("Mixing").click();
    steps(&mut h);
    let before = h.state().ui.narrow_mix;
    h.query_all_by_label("Narrow Mix").next().unwrap().click();
    steps(&mut h);
    assert_ne!(h.state().ui.narrow_mix, before);
}

#[test]
fn meter_ballistics_are_set_and_kept_in_range() {
    let mut app = app();
    let r = app.run("ui.meter_ballistics", json!({})).unwrap();
    assert_eq!((r["release_db_s"].as_f64(), r["hold_secs"].as_f64()), (Some(26.0), Some(2.0)), "the defaults");
    let r = app.run("ui.meter_ballistics", json!({"release_db_s": 500.0, "hold_secs": 1e9})).unwrap();
    assert_eq!(r["release_db_s"].as_f64(), Some(120.0));
    assert_eq!(r["hold_secs"].as_f64(), Some(f64::from(HOLD_UNTIL_CLEARED)));
    // Saved with the UI settings.
    let saved = serde_json::to_value(&app.ui).unwrap();
    assert_eq!(saved["meter_release_db_s"].as_f64(), Some(120.0));
}
