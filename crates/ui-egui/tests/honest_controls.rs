//! Controls that promise more than FreeDAWm does say so where they are seen (parity section 8,
//! "Things that promise more than they do"): the Renderer window renders nothing yet.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1600.0, 1200.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

#[test]
fn the_renderer_window_says_it_renders_nothing_yet() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.extra.show_renderer = true;
    let mut h = harness(app);
    h.run_steps(3);
    assert!(h.query_by_label_contains("objects are not rendered yet").is_some());
}
