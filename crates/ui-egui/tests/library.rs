//! The Library: what the user saves to use in other sessions, in one folder and one browser tab.
//! Track › Save Plug-in Chain… asks for a name and writes into the Library's `Chains/`; the
//! browser's Library tab lists chains, track presets, plugin presets and templates, and a
//! double-click uses one on the selected track.

#[path = "editor_support/mod.rs"]
mod editor_support;

use editor_support::double_click;
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::browser::BrowserTab;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::menus;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fd-library-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn app_in(dir: &Path) -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.library_default = Some(dir.to_path_buf());
    app.apply_library_dir();
    app
}

fn harness(mut app: SoundApp) -> Harness<'static, SoundApp> {
    app.run("ui.set", json!({"theme": "dark"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1700.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn plugins(app: &SoundApp, track: &str) -> Vec<String> {
    app.engine.session().track_by_name(track).unwrap().mixer.inserts.iter().map(|i| i.plugin.clone()).collect()
}

#[test]
fn a_chain_saved_from_the_track_menu_is_used_from_the_library_tab() {
    let dir = scratch("chain");
    let mut app = app_in(&dir);
    app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
    let bass = plugins(&app, "Bass");
    assert!(!bass.is_empty());
    menus::invoke_menu(&mut app, "track.save_chain", "Track > Save Plug-in Chain...");
    let Some(Dialog::SaveTrackPreset { name, chain: true, .. }) = &mut app.dialogs.open else {
        panic!("no name dialog: {:?}", app.dialogs.open_name())
    };
    assert_eq!(name, "Bass chain", "starts from the track's name");
    *name = "Low end".into();
    let mut h = harness(app);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save").click();
    h.run_steps(4);
    assert!(dir.join("Chains").join("Low end.json").is_file(), "written into the Library's chains folder");
    assert!(h.state().dialogs.open.is_none());

    // A track preset and a template beside it, then the Library tab with Hats selected.
    let presets = dir.join("Track Presets");
    std::fs::create_dir_all(&presets).unwrap();
    h.state_mut().engine.execute("track.save_preset", &json!({"path": presets.join("Bass strip.json").to_string_lossy()})).unwrap();
    std::fs::create_dir_all(dir.join("Templates")).unwrap();
    std::fs::write(dir.join("Templates").join("Band.fdawm"), "{}").unwrap();
    h.state_mut().engine.execute("edit.select", &json!({"tracks": ["Hats"]})).unwrap();
    h.state_mut().ui.browser.tab = BrowserTab::Library;
    h.state_mut().run("window.plugin_browser", json!({})).unwrap();
    h.run_steps(5);
    for label in ["Plug-in Chains", "Track Presets", "Session Templates", "Bass strip", "Band"] {
        assert!(h.query_by_label(label).is_some(), "{label} is listed");
    }
    let before = h.state().engine.session().track_by_name("Hats").unwrap().mixer.clone();
    let at = h.get_by_label("Low end").rect().center();
    // Well after the click on Save: egui counts a click within 0.6 s of the one before last as a
    // triple click, wherever that one was.
    h.run_steps(60);
    double_click(&mut h, at);
    h.run_steps(3);
    assert_eq!(plugins(h.state(), "Hats"), bass, "the chain is on the selected track: {}", h.state().ui.status);
    let hats = h.state().engine.session().track_by_name("Hats").unwrap();
    assert_eq!(hats.mixer.volume_db, before.volume_db, "a chain leaves the fader alone");
    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(h.state().engine.session().track_by_name("Hats").unwrap().mixer, before, "one undo step");

    // The search narrows the list; a kind chip shows one kind.
    h.state_mut().ui.browser.library_text = "strip".into();
    h.run_steps(3);
    assert!(h.query_by_label("Low end").is_none());
    assert!(h.query_by_label("Bass strip").is_some());
    h.state_mut().ui.browser.library_text.clear();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Templates").click();
    h.run_steps(3);
    assert!(h.query_by_label("Bass strip").is_none());
    assert!(h.query_by_label("Band").is_some());

    // A template opens the New dialog on it.
    h.run_steps(60);
    let at = h.get_by_label("Band").rect().center();
    double_click(&mut h, at);
    h.run_steps(3);
    match &h.state().dialogs.open {
        Some(Dialog::NewSession(n)) => {
            assert_eq!(n.start, soundcraft_ui_egui::dialogs::Start::Template(dir.join("Templates").join("Band.fdawm")));
        }
        _ => panic!("{:?}", h.state().dialogs.open_name()),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_no_track_selected_the_library_says_so() {
    let dir = scratch("none");
    std::fs::create_dir_all(dir.join("Chains")).unwrap();
    let mut app = app_in(&dir);
    app.engine.execute("track.save_chain", &json!({"path": dir.join("Chains").join("Low end.json").to_string_lossy(), "tracks": ["Bass"]})).unwrap();
    app.engine.execute("edit.select", &json!({"tracks": []})).unwrap();
    app.ui.browser.tab = BrowserTab::Library;
    app.run("window.plugin_browser", json!({})).unwrap();
    let mut h = harness(app);
    let at = h.get_by_label("Low end").rect().center();
    double_click(&mut h, at);
    h.run_steps(3);
    assert_eq!(h.state().ui.status, "Select a track first");
    let _ = std::fs::remove_dir_all(&dir);
}
