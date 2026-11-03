//! Saved templates in File › New and File › New from Template… (docs/superpowers/specs/
//! 2026-10-25-presets-markers-templates.md, part 3): Save As Template writes into the user's
//! templates folder, the New dialog lists them (newest first) under My templates with a details
//! line, Browse… takes a template from anywhere, and Create opens a new untitled session.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::dialogs::{Dialog, Start};
use soundcraft_ui_egui::menus;
use soundcraft_ui_egui::user_files::UserDirs;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-ui-templates-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn app_in(dir: &Path) -> SoundApp {
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

/// A template with `tracks` audio tracks at `rate`, written `age` seconds ago.
fn template(dir: &Path, name: &str, tracks: u32, rate: u32, age: u64) -> PathBuf {
    let mut e = soundcraft_engine::Engine::default();
    e.execute("session.new", &json!({"sample_rate": rate})).unwrap();
    e.execute("track.new", &json!({"count": tracks})).unwrap();
    let file = dir.join("Templates").join(format!("{name}.scraft"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    e.execute("session.save_template", &json!({"path": file.to_string_lossy()})).unwrap();
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(SystemTime::now() - Duration::from_secs(age)).unwrap();
    file
}

fn new_dialog(app: &SoundApp) -> &soundcraft_ui_egui::dialogs::NewSession {
    match &app.dialogs.open {
        Some(Dialog::NewSession(n)) => n,
        other => panic!("not the New dialog: {other:?}"),
    }
}

#[test]
fn save_as_template_writes_into_the_templates_folder() {
    let dir = scratch("save");
    let mut app = app_in(&dir);
    menus::invoke_menu(&mut app, "session.save_template", "File > Save As Template...");
    let Some(Dialog::PathPrompt { path, .. }) = &app.dialogs.open else { panic!("{:?}", app.dialogs.open_name()) };
    let want = dir.join("Templates").join("Midnight Groove.fdawm");
    assert_eq!(PathBuf::from(path.as_str()), want);
    assert!(!dir.join("Templates").exists(), "the folder is made on save, not when the prompt opens");
    let mut h = harness(app);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "OK").click();
    h.run_steps(3);
    assert!(want.is_file(), "written where File › New lists templates");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn new_lists_templates_newest_first_and_creates_from_one() {
    let dir = scratch("new");
    let band = template(&dir, "Band tracking", 3, 44_100, 10);
    template(&dir, "Podcast 2-mic", 2, 48_000, 1000);
    template(&dir, "Vocal comp", 1, 48_000, 5000);
    let mut app = app_in(&dir);
    menus::invoke_menu(&mut app, "session.new", "File > New...");
    let names: Vec<&str> = new_dialog(&app).templates.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Band tracking", "Podcast 2-mic", "Vocal comp"]);
    assert_eq!(new_dialog(&app).start, Start::Blank, "File › New… starts on Blank");

    let mut h = harness(app);
    for row in ["Blank", "Demo", "Band tracking", "Podcast 2-mic", "Vocal comp", "Browse…"] {
        assert!(h.query_by_label(row).is_some(), "{row}");
    }
    h.get_by_label("Band tracking").click();
    h.run_steps(3);
    let details = h.query_by_label_contains("3 tracks · 44.1 kHz · saved").is_some();
    assert!(details, "the details line names the template's tracks, rate and date");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Create").click();
    h.run_steps(3);
    // The template loads behind the progress window (a worker reads it): frames until it is in.
    for _ in 0..2_000 {
        if h.state().open.is_none() {
            break;
        }
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(h.state().open.is_none(), "the open finished");
    assert!(h.state().dialogs.open.is_none(), "{:?}", h.state().dialogs.open_name());
    let app = h.state();
    let s = app.engine.session();
    assert_eq!(s.tracks.len(), 3);
    assert_eq!(s.sample_rate.hz(), 44_100);
    assert_eq!(s.name, "Untitled");
    assert!(app.engine.path.is_none(), "a new untitled session, not the template file");
    assert!(band.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn new_from_template_opens_the_same_list_on_the_newest() {
    let dir = scratch("from");
    template(&dir, "Old", 1, 48_000, 500);
    let newest = template(&dir, "Newest", 2, 48_000, 5);
    let mut app = app_in(&dir);
    let extra = menus::ui_aliases();
    let id = menus::command_for_path("File > New from Template...", &extra).unwrap();
    assert_eq!(id, "ui.new_from_template");
    menus::invoke_menu(&mut app, &id, "File > New from Template...");
    assert_eq!(new_dialog(&app).start, Start::Template(newest));
    assert_eq!(new_dialog(&app).templates.len(), 2);
    // Down walks the list.
    let mut h = harness(app);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(2);
    assert!(matches!(&new_dialog(h.state()).start, Start::Template(p) if p.ends_with("Old.scraft")));
    // Agents call session.new directly; the id only opens the dialog from the menu.
    assert!(h.state_mut().run("ui.new_from_template", json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn browse_takes_a_template_from_anywhere_without_an_os_dialog() {
    let dir = scratch("browse");
    let elsewhere = scratch("browse-elsewhere");
    let file = template(&elsewhere, "Somewhere", 4, 48_000, 1);
    let mut app = app_in(&dir);
    menus::invoke_menu(&mut app, "session.new", "File > New...");
    let mut h = harness(app);
    assert!(h.query_by_label_contains("No saved templates yet").is_some());
    h.get_by_label("Browse…").click();
    h.run_steps(3);
    let Some(Dialog::NewSession(n)) = &mut h.state_mut().dialogs.open else { panic!() };
    assert_eq!(n.browse.as_deref(), Some(""), "an in-app path field");
    n.browse = Some(file.to_string_lossy().into_owned());
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Use").click();
    h.run_steps(3);
    assert_eq!(new_dialog(h.state()).start, Start::Template(file.clone()));
    assert!(h.query_by_label_contains("4 tracks · 48 kHz").is_some());
    assert_eq!(new_dialog(h.state()).params()["template"], json!(file.to_string_lossy()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&elsewhere);
}
