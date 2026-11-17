//! Export › Stems renders behind a progress window, a slice per frame, and can be cancelled
//! (mix bug 10: it ran in the window's own thread, with no progress or cancel).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1200.0, 800.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
}

fn dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("freedawm-stems-{tag}-{}", std::process::id()))
}

#[test]
fn stems_render_behind_a_progress_window_and_finish() {
    let out = dir("done");
    let _ = std::fs::remove_dir_all(&out);
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    // A short range keeps the test quick.
    let r = app.run("file.bounce_stems", json!({"dir": out.to_string_lossy(), "start": 0, "end": 24_000})).unwrap();
    assert_eq!(r["rendering"], true, "the command returns at once: {r}");
    assert!(app.stems.is_some() && app.rendering());
    assert!(app.run("transport.play", json!({})).is_err(), "nothing plays under it");
    let mut h = harness(app);
    for _ in 0..2000 {
        h.step();
        if h.state().stems.is_none() {
            break;
        }
    }
    assert!(h.state().stems.is_none(), "the export finished");
    assert!(h.state().ui.status.starts_with("Exported"), "{}", h.state().ui.status);
    assert!(std::fs::read_dir(&out).unwrap().count() > 1);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_stems_export_can_be_cancelled() {
    let out = dir("cancel");
    let _ = std::fs::remove_dir_all(&out);
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("file.bounce_stems", json!({"dir": out.to_string_lossy()})).unwrap();
    let mut h = harness(app);
    h.step();
    assert!(h.state().stems.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().stems.is_none());
    assert_eq!(h.state().ui.status, "Stems export cancelled");
    let _ = std::fs::remove_dir_all(&out);
}
