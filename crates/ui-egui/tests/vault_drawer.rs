//! Vault's drawer: its response picker, and that the module is offered in the plugin lists.

mod editor_support;

use editor_support::*;
use egui::vec2;
use egui_kittest::kittest::Queryable;

#[test]
fn the_drawer_has_the_response_picker() {
    let mut app = app_with_tracks(&["Vocal"]);
    let (t, m) = add(&mut app, "Vocal", "vault");
    app.ui.drawers.insert(t, m);
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
    h.run_steps(6);
    assert!(h.query_by_label("Built-in space · load an impulse response…").is_some(), "the picker, with nothing loaded");
}

#[test]
fn vault_is_listed_though_stock_reverbs_are_hidden() {
    let listed: Vec<&str> = soundcraft_dsp::listed_plugins().map(|p| p.id).collect();
    assert!(listed.contains(&"vault"), "{listed:?}");
    assert!(!listed.contains(&"plate_reverb"));
}
