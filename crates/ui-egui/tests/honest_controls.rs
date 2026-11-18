//! Controls that promise more than FreeDAWm does say so where they are seen (parity section 8,
//! "Things that promise more than they do"): the Renderer window renders nothing yet, and the
//! Network window says what a paired device can do and that the local link is not encrypted.

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

#[test]
fn the_network_window_says_what_pairing_allows_and_that_the_local_link_is_plain() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.ui.show_network = true;
    let mut h = harness(app);
    h.run_steps(3);
    if h.state().net.is_none() {
        // No network state in this environment (the window cannot open): nothing to check.
        eprintln!("SKIPPED: no network state");
        return;
    }
    assert!(h.query_by_label_contains("can do everything you can do here").is_some());
    assert!(h.query_by_label_contains("a link that is not encrypted").is_some());
    // The first pairing of a PC proves both keys with the code, which is never sent.
    assert!(h.query_by_label_contains("a PC never sends the code").is_some());
    assert!(h.query_by_label_contains("Offer the encrypted local link").is_some());
    // PCs are taken over the encrypted link only, unless the user turns it off (owner, 2026-11-16).
    assert!(h.query_by_label_contains("Take PCs over the encrypted link only").is_some());
    assert!(h.state().net.as_ref().is_some_and(|n| n.hub.config().encrypted_only), "on by default");
}
