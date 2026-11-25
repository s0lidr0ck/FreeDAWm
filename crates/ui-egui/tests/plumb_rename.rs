//! Plumb, the real-time pitch corrector, was called Auto Pitch in test builds. Sessions and user
//! presets store its id (`autopitch`, unchanged), never its name: one saved then opens, keeps its
//! settings and preset, and shows as Plumb.
mod editor_support;

use editor_support::{add, app_with_tracks, harness};
use egui::vec2;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::browser::catalog::build_catalog;
use soundcraft_ui_egui::browser::presets::presets_for;

#[test]
fn a_session_and_a_preset_saved_as_auto_pitch_open_as_plumb() {
    let dir = std::env::temp_dir().join(format!("sc-plumb-rename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("presets").join("autopitch")).unwrap();

    // A session with the module on "Voice", Hard tune, its preset name shown.
    let mut before = app_with_tracks(&["Voice"]);
    let (t, m) = add(&mut before, "Voice", "autopitch");
    before
        .engine
        .execute("mix.module_params", &json!({"track": t.0, "module": m.0, "params": {"style": 2.0, "retune": 7.0}, "preset": "Hard"}))
        .unwrap();
    let path = dir.join(format!("old.{}", soundcraft_model::SESSION_EXTENSION));
    soundcraft_engine::io::save_session(&mut before.engine, path.to_str().unwrap()).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("\"autopitch\"") && !saved.contains("Plumb"), "the file stores the id, not the name");

    // A user preset saved from the old editor (its own name is the user's and stays).
    let preset =
        json!({"format": "soundcraft-plugin-preset", "plugin": "autopitch", "name": "My Auto Pitch", "tags": "", "params": {"retune": 12.0}});
    std::fs::write(dir.join("presets").join("autopitch").join("My Auto Pitch.json"), preset.to_string()).unwrap();

    // Opened again in a fresh app.
    let mut app = app_with_tracks(&[]);
    soundcraft_engine::io::open_session(&mut app.engine, path.to_str().unwrap()).unwrap();
    app.preset_dir = Some(dir.join("presets"));
    let ins = app.engine.session().track_by_name("Voice").unwrap().mixer.inserts[0].clone();
    assert_eq!((ins.plugin.as_str(), ins.preset.as_str()), ("autopitch", "Hard"));
    assert_eq!((ins.params["style"], ins.params["retune"]), (2.0, 7.0));
    let info = soundcraft_dsp::plugin_info(&ins.plugin).unwrap();
    assert_eq!((info.name, info.short_name), ("Plumb", "Plumb"));
    assert_eq!(soundcraft_ui_egui::plugin_titles::titled(info), "Plumb - Pitch Correction");
    assert!(soundcraft_dsp::create(&ins.plugin).is_some(), "it is made, so it plays");

    // The user preset is still listed for it.
    let catalog = build_catalog(soundcraft_dsp::plugins(), &[], &[], &[], Vec::new());
    let card = catalog.items.iter().find(|i| i.id == "autopitch").unwrap().clone();
    assert_eq!(card.name, "Plumb");
    assert!(presets_for(&app, &card).iter().any(|(_, name, _)| name == "My Auto Pitch"));

    // The Mix window's tile says Plumb.
    let h = harness(app, vec2(1400.0, 900.0), "dark");
    assert!(h.query_by_label("Plumb module").is_some(), "the insert tile is labelled Plumb");
    assert!(h.query_by_label_contains("Auto Pitch").is_none() && h.query_by_label_contains("AutoPtch").is_none());
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
