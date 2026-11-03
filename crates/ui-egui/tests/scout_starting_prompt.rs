//! Scout ▸ ☰ ▸ Copy Starting Prompt / Save Starting Prompt…: the FireStation starting prompt on the
//! clipboard or in a Markdown file, the same text the engine's `scout.starting_prompt` returns.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::Session;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

fn copied_texts(h: &Harness<'static, SoundApp>) -> Vec<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn engine_prompt(app: &mut SoundApp) -> String {
    app.engine.execute("scout.starting_prompt", &json!({})).unwrap()["text"].as_str().unwrap().to_string()
}

#[test]
fn the_menu_copies_the_starting_prompt() {
    let mut app = SoundApp::new(Engine::new(Session::default()), None, Services::default());
    app.scout.show = true;
    let expected = engine_prompt(&mut app);
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("☰").click();
    h.run_steps(2);
    h.get_by_label("Copy Starting Prompt").click();
    h.run_steps(2);
    assert_eq!(copied_texts(&h), vec![expected.clone()]);
    assert!(expected.starts_with("# FreeDAWm + Scout: starting prompt"));
    assert!(expected.contains("(`fs_cleanq`"));
    assert!(expected.contains("scout-guides"));
}

#[test]
fn save_writes_the_prompt_as_markdown() {
    let dir = std::env::temp_dir().join(format!("scout-prompt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("prompt.md");
    let mut app = SoundApp::new(Engine::new(Session::default()), None, Services::default());
    let expected = engine_prompt(&mut app);
    let r = app.run("ui.scout.starting_prompt_save", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["chars"].as_u64().unwrap() as usize, expected.chars().count());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    // Without a path or a save dialog (tests, web): nothing is written, nothing fails.
    let r = app.run("ui.scout.starting_prompt_save", json!({})).unwrap();
    assert!(r["path"].is_null());
    // A folder that can't be written is an error, not a crash.
    assert!(app.run("ui.scout.starting_prompt_save", json!({"path": dir.join("no/such/dir/p.md").to_string_lossy()})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
