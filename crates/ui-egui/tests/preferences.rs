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

/// The ten set-once settings that left the Options menu (2026-11 layout): each Preferences control
/// changes what its old menu item changed, through the same command. One app clicks the control,
/// a twin runs the old item (`invoke_menu` with the path it ran); their sessions then match, and
/// the old item's check mark reads the new state.
#[test]
fn the_settings_that_left_the_options_menu_work_from_preferences() {
    // (tab, control, command, the old menu item's path)
    const MOVED: [(&str, &str, &str, &str); 10] = [
        ("operation", "Feedback Protection", "options.feedback_protection", "Options > Recording > Feedback Protection"),
        ("editing", "Automatic Edge Fades", "options.auto_fades", "Options > Automatic Edge Fades"),
        ("editing", "Imported Loops Follow Tempo and Key", "options.loops_follow", "Options > More > Imported Loops Follow Tempo and Key"),
        ("editing", "Edit/Tool Mode Keyboard Lock", "options.keyboard_lock", "Options > Edit/Tool Mode Keyboard Lock"),
        ("mixing", "Pre-Fader Metering", "options.pre_fader_metering", "Options > Pre-Fader Metering"),
        ("mixing", "Pre-Fader Sends Ignore Mute", "mix.pre_sends_ignore_mute", "Options > More > Pre-Fader Sends Ignore Mute"),
        ("mixing", "Thin Automation After Pass", "automation.thin_after_pass", "Options > More > Thin Automation After Pass"),
        ("mixing", "Suspend Plug-ins on Silence", "options.suspend_on_silence", "Options > More > Suspend Plug-ins on Silence"),
        ("mixing", "-6 dB", "mix.pan_law", "Options > More > Pan Law > -6 dB"),
        ("mixing", "Unity (0 dB)", "mix.default_send_level", "Options > More > Default Send Level > 0 dB"),
    ];
    let ids: Vec<&str> = MOVED.iter().map(|m| m.2).collect();
    let mut want = soundcraft_ui_egui::preferences::FROM_OPTIONS.to_vec();
    let mut got = ids.clone();
    want.sort_unstable();
    got.sort_unstable();
    assert_eq!(got, want, "every moved setting is tried");
    for (tab, label, id, old) in MOVED {
        let open = |tab: &str| {
            let mut a = app();
            // The radio choices start away from the one clicked.
            a.run("mix.pan_law", json!({"law": -3})).unwrap();
            a.run("mix.default_send_level", json!({"db": -144.0})).unwrap();
            a.run("window.preferences", json!({"tab": tab})).unwrap();
            harness(a)
        };
        let (mut prefs, mut menu) = (open(tab), open(tab));
        let before = menus::item_state(prefs.state(), old, Some(id)).1;
        prefs.query_all_by_label(label).next().unwrap_or_else(|| panic!("no control {label}")).click();
        steps(&mut prefs);
        menus::invoke_menu(menu.state_mut(), id, old);
        steps(&mut menu);
        let after = menus::item_state(prefs.state(), old, Some(id)).1;
        assert_eq!(after, menus::item_state(menu.state(), old, Some(id)).1, "{label}: Preferences and the old item agree");
        if label.ends_with("dB)") || label.ends_with(" dB") {
            assert!(!before && after, "{label}: chosen");
        } else {
            assert_ne!(before, after, "{label}: toggled");
        }
        assert!(*prefs.state().engine.session() == *menu.state().engine.session(), "{label}: the same change as the old menu item");
    }
}

#[test]
fn plug_ins_in_their_own_process_is_this_computers_setting_off_by_default() {
    let mut a = app();
    assert!(!a.ui.plugin_process, "off by default");
    assert!(!soundcraft_mix::plugin_bridge::enabled());
    a.run("window.preferences", json!({"tab": "mixing"})).unwrap();
    let mut h = harness(a);
    h.get_by_label("Run CLAP and VST3 plug-ins in their own process").click();
    steps(&mut h);
    assert!(h.state().ui.plugin_process);
    assert!(soundcraft_mix::plugin_bridge::enabled(), "plug-ins loaded from now on are bridged");
    // Saved with this computer's settings, and older settings files get it off.
    let saved = serde_json::to_value(&h.state().ui).unwrap();
    assert_eq!(saved["plugin_process"].as_bool(), Some(true));
    let mut old = saved.clone();
    old.as_object_mut().unwrap().remove("plugin_process");
    assert!(!serde_json::from_value::<soundcraft_ui_egui::UiState>(old).unwrap().plugin_process);
    let r = h.state_mut().run("ui.plugin_process", json!({"value": false})).unwrap();
    assert_eq!((r["value"].as_bool(), r["in_effect"].as_bool()), (Some(false), Some(false)));
    steps(&mut h);
    assert!(!soundcraft_mix::plugin_bridge::enabled());
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
