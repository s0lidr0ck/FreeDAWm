//! The first-run tour: closed unless the host opens it (a fresh app in a test shows none), a card
//! at a time with Next and Back, Skip closes it, and FreeDAWm › Tour of FreeDAWm opens it.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, tour};

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

#[test]
fn the_tour_steps_through_its_cards_and_closes() {
    let mut h = harness();
    h.run_steps(2);
    assert_eq!(h.state().ui.tour_step, None, "no tour unless the host asks for one");
    assert!(h.query_by_label(tour::STEPS[0].0).is_none());
    h.state_mut().run("window.tour", json!({})).unwrap();
    h.run_steps(2);
    assert!(h.query_by_label(tour::STEPS[0].0).is_some());
    h.get_by_label("Next").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.tour_step, Some(1));
    assert!(h.query_by_label(tour::STEPS[1].0).is_some());
    h.get_by_label("Back").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.tour_step, Some(0));
    h.get_by_label("Skip tour").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.tour_step, None);
    assert!(h.query_by_label(tour::STEPS[0].0).is_none());
    // The last card's Done closes it too.
    h.state_mut().ui.tour_step = Some(tour::STEPS.len() - 1);
    h.run_steps(2);
    h.get_by_label("Done").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.tour_step, None);
}
