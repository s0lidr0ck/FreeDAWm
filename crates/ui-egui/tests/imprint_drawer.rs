//! Imprint's drawer rows and its Save IR… command (`ui.imprint.save`) with no take to save: the
//! capture itself is covered where it runs (`soundcraft_dsp::plugins::imprint`).

mod editor_support;

use editor_support::*;
use egui::vec2;
use egui_kittest::kittest::Queryable;
use serde_json::json;

#[test]
fn a_recording_role_offers_save_ir_and_a_send_does_not() {
    let mut app = app_with_tracks(&["Capture"]);
    let (t, m) = add(&mut app, "Capture", "imprint");
    app.ui.drawers.insert(t, m);
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
    h.run_steps(6);
    assert!(h.query_by_label("Save IR…").is_some(), "Loop records: it has an IR to save");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "role", "value": 0})).unwrap();
    h.run_steps(6);
    assert!(h.query_by_label("Save IR…").is_none(), "a Send only plays the sweep");
}

#[test]
fn saving_with_no_take_says_why_and_writes_nothing() {
    let mut app = app_with_tracks(&["Capture"]);
    let (t, m) = add(&mut app, "Capture", "imprint");
    let (_, other) = add(&mut app, "Capture", "gain");
    let path = std::env::temp_dir().join(format!("imprint-none-{}.wav", std::process::id()));
    let err = app.run("ui.imprint.save", json!({"track": t.0, "module": m.0, "path": path.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("nothing captured yet"), "{err}");
    assert!(!path.exists());
    let err = app.run("ui.imprint.save", json!({"track": t.0, "module": other.0, "path": path.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("captures no impulse response"), "{err}");
}
