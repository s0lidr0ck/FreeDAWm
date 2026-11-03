//! A plugin looked up and missing from the scan cache (a session holding plugins never scanned)
//! starts a background scan ("Resolving plugins…") instead of scanning on the UI thread.
mod editor_support;

use editor_support::app_with_tracks;
use egui::vec2;
use egui_kittest::Harness;
use soundcraft_ui_egui::SoundApp;

#[test]
fn a_missing_plugin_starts_a_background_resolve() {
    let app = app_with_tracks(&["Voice"]);
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    assert!(h.state().plugin_scan.is_none());
    // Creating it (as opening the session does) fails at once: it shows as missing.
    let started = std::time::Instant::now();
    assert!(soundcraft_vst3_host::create("vst3:0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F").is_none());
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "no scan on the UI thread");
    h.run_steps(1);
    assert!(h.state().plugin_scan.as_ref().is_some_and(|j| j.resolving()), "a background scan resolves it");
    // No scanner next to the test binary: the scan ends with that message, nothing loaded.
    let t0 = std::time::Instant::now();
    while h.state().plugin_scan.is_some() {
        assert!(t0.elapsed().as_secs() < 20, "the resolve ends");
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().ui.status.contains("plugin scanner"), "{}", h.state().ui.status);
    // Asked once: no new scan for the same plugin.
    assert!(soundcraft_vst3_host::create("vst3:0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F").is_none());
    h.run_steps(2);
    assert!(h.state().plugin_scan.is_none());
}
