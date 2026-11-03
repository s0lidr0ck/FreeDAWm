//! Track presets from the menus (docs/superpowers/specs/2026-10-25-presets-markers-templates.md,
//! part 1): Save Track Preset… asks for a name and writes into the user's track presets folder;
//! Load Track Preset lists them (newest first) in the Track menu's dialog and in the track's
//! context menu (Mix window channel name, Edit window track name), and loading is one undo step.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::menus;
use soundcraft_ui_egui::user_files::UserDirs;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-track-presets-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn app_in(dir: &std::path::Path) -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.dialogs.dirs = UserDirs::under(dir);
    app
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn click(h: &mut Harness<'static, SoundApp>, label: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label).click();
    h.run_steps(4);
}

fn hats_volume(app: &SoundApp) -> f32 {
    app.engine.session().track_by_name("Hats").unwrap().mixer.volume_db
}

#[test]
fn save_asks_for_a_name_and_load_lists_it_in_the_track_menu_dialog() {
    let dir = scratch("menu");
    let mut app = app_in(&dir);
    app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
    menus::invoke_menu(&mut app, "track.save_preset", "Track > Save Track Preset...");
    let Some(Dialog::SaveTrackPreset { name, .. }) = &mut app.dialogs.open else { panic!("no name dialog: {:?}", app.dialogs.open_name()) };
    assert_eq!(name, "Bass", "starts from the track's name");
    *name = "Bass warm".into();
    let mut h = harness(app);
    click(&mut h, "Save");
    let file = dir.join("Track Presets").join("Bass warm.json");
    assert!(file.is_file(), "written into the track presets folder");
    assert!(h.state().dialogs.open.is_none());

    // Track › Load Track Preset… on Hats: the list holds it; picking it and Load applies it.
    let before = hats_volume(h.state());
    h.state_mut().engine.execute("edit.select", &json!({"tracks": ["Hats"]})).unwrap();
    let extra = menus::ui_aliases();
    let id = menus::command_for_path("Track > Load Track Preset...", &extra).unwrap();
    assert_eq!(id, "track.load_preset");
    menus::invoke_menu(h.state_mut(), &id, "Track > Load Track Preset...");
    assert_eq!(h.state().dialogs.open_name(), Some("load_track_preset"));
    // The list is read when the dialog opens, not every frame.
    std::fs::write(dir.join("Track Presets").join("Later.json"), "{}").unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("Later").is_none(), "listed as the folder was at open");
    h.run_steps(3);
    h.get_by_label("Bass warm").click();
    h.run_steps(3);
    click(&mut h, "Load");
    let hats = h.state().engine.session().track_by_name("Hats").unwrap();
    assert_eq!(hats.mixer.volume_db, -5.0, "the Bass preset's fader");
    assert_eq!(hats.mixer.inserts.first().map(|i| i.plugin.as_str()), Some("compressor"));
    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(hats_volume(h.state()), before, "one undo step");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_mix_channel_menu_saves_and_loads_presets() {
    let dir = scratch("strip");
    let presets = dir.join("Track Presets");
    std::fs::create_dir_all(&presets).unwrap();
    let mut app = app_in(&dir);
    // A preset saved earlier (from Bass), then the Mix window.
    app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
    app.engine.execute("track.save_preset", &json!({"path": presets.join("Low end.json").to_string_lossy()})).unwrap();
    app.engine.execute("edit.select", &json!({"tracks": []})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    let mut h = harness(app);
    h.run_steps(4);

    // Right-click the Hats channel name: Save Track Preset… and Load Track Preset… are there.
    h.get_by_label("Hats strip").click_secondary();
    h.run_steps(4);
    assert!(h.query_by_label("Rename...").is_some(), "the channel menu is open");
    h.get_by_label_contains("Load Track Preset").hover();
    h.run_steps(4);
    assert!(h.query_by_label("Browse…").is_some(), "Browse… ends the submenu");
    h.get_by_label("Low end").click();
    h.run_steps(4);
    assert_eq!(hats_volume(h.state()), -5.0, "loaded onto the right-clicked channel");

    h.get_by_label("Hats strip").click_secondary();
    h.run_steps(4);
    click(&mut h, "Save Track Preset…");
    match &h.state().dialogs.open {
        Some(Dialog::SaveTrackPreset { name, track }) => {
            assert_eq!(name, "Hats");
            assert_eq!(Some(*track), h.state().engine.session().track_by_name("Hats").map(|t| t.id));
        }
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_existing_name_asks_to_replace() {
    let dir = scratch("replace");
    let presets = dir.join("Track Presets");
    std::fs::create_dir_all(&presets).unwrap();
    std::fs::write(presets.join("Kick in.json"), "{}").unwrap();
    let mut app = app_in(&dir);
    app.engine.execute("edit.select", &json!({"tracks": ["Kick"]})).unwrap();
    menus::invoke_menu(&mut app, "track.save_preset", "Track > Save Track Preset...");
    let Some(Dialog::SaveTrackPreset { name, .. }) = &mut app.dialogs.open else { panic!() };
    *name = "Kick in".into();
    let mut h = harness(app);
    click(&mut h, "Replace");
    let text = std::fs::read_to_string(presets.join("Kick in.json")).unwrap();
    assert!(text.contains("soundcraft-track-preset"), "replaced: {text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_presets_folder_save_falls_back_to_a_path() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
    menus::invoke_menu(&mut app, "track.save_preset", "Track > Save Track Preset...");
    assert_eq!(app.dialogs.open_name(), Some("path"));
}
