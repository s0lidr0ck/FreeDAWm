//! Vint's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, a faceplate knob
//! drag on two circuits as one undo step, the circuit menu's pick, and every circuit drawn.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

/// Vint on "Pad" in the slim layout, its drawer open, on `circuit`.
fn slim(circuit: i32) -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_vint");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    app.run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"circuit": circuit}})).unwrap();
    // tall enough that the whole column is on screen
    let mut h = harness(app, vec2(1400.0, 2000.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

#[test]
fn every_circuit_draws_in_the_drawers_width() {
    for c in 0..8 {
        let (h, t, m) = slim(c);
        let f = frame(&h, t, m);
        assert!((f.width() - W).abs() <= 1.0, "circuit {c}: the classic drawer's width: {}", f.width());
        for label in ["Undo", "A", "B", "Bypass", "Circuit", "Mix", "Output"] {
            let r = at(&h, f, label).unwrap_or_else(|| panic!("circuit {c}: {label}"));
            assert!(f.shrink2(vec2(4.0, 0.0)).contains_rect(r), "circuit {c}: {label} at {r:?} leaves the drawer {f:?}");
        }
    }
}

fn knob_drag_is_one_step(circuit: i32, label: &str, id: &str) {
    let (mut h, t, m) = slim(circuit);
    let f = frame(&h, t, m);
    let before = raw(&h, t, m, id);
    let n = steps(&h);
    let k = at(&h, f, label).unwrap_or_else(|| panic!("{label} on the face")).center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(raw(&h, t, m, id) > before + 0.1, "{id} went up: {} -> {}", before, raw(&h, t, m, id));
    assert_eq!(steps(&h), n + 1, "the drag is one undo step");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, id), before, "undone");
}

#[test]
fn a_passive_face_knob_drag_is_one_undo_step() {
    knob_drag_is_one_step(0, "Bandwidth", "pasBandwidth");
}

#[test]
fn a_british_face_knob_drag_is_one_undo_step() {
    knob_drag_is_one_step(3, "briHmFreq", "briHmFreq");
}

#[test]
fn the_circuit_menu_picks_with_one_undo_step() {
    let (mut h, t, m) = slim(0);
    let f = frame(&h, t, m);
    let n = steps(&h);
    let menu = at(&h, f, "Circuit").expect("the circuit menu").center();
    click(&mut h, menu);
    h.run_steps(2);
    let item = h.query_all_by_label("British SSL 4000 E").map(|n| n.rect()).next().expect("British in the menu").center();
    click(&mut h, item);
    h.run_steps(2);
    assert_eq!(raw(&h, t, m, "circuit"), 3.0, "British picked");
    assert_eq!(steps(&h), n + 1, "one undo step");
    let f = frame(&h, t, m);
    assert!(at(&h, f, "briLowGain").is_some(), "the SSL strip's face");
}

#[test]
fn the_maximizer_and_the_vitalizer_keep_their_fronts_in_the_drawer() {
    // (the Enhancer's type, every control on its unit, a knob to drag, its bypass button)
    let units: [(i32, &[&str], (&str, &str), &str); 2] = [
        (1, &["Lo Contour", "Process", "Tune", "Function"], ("Process", "enhAmount"), "Function"),
        (2, &["vitDrive", "vitBass", "vitComp", "enhTune", "enhAmount", "vitLcFreq", "vitIntensity", "vitWidth", "On"], ("vitComp", "vitComp"), "On"),
    ];
    for (enh_type, controls, (knob, id), button) in units {
        let (mut h, t, m) = slim(4);
        h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "enhType", "value": enh_type})).unwrap();
        h.run_steps(6);
        let f = frame(&h, t, m);
        assert!((f.width() - W).abs() <= 1.0, "type {enh_type}: the classic drawer's width: {}", f.width());
        for label in controls.iter().chain(&["Undo", "Bypass", "Circuit", "Mix", "Output"]) {
            let r = at(&h, f, label).unwrap_or_else(|| panic!("type {enh_type}: {label}"));
            assert!(f.shrink2(vec2(4.0, 0.0)).contains_rect(r), "type {enh_type}: {label} at {r:?} leaves the drawer {f:?}");
        }
        // a knob on the unit: one drag, one undo step
        let n = steps(&h);
        let k = at(&h, f, knob).unwrap().center();
        drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
        assert!(raw(&h, t, m, id) > 0.2, "type {enh_type}: {id} went up: {}", raw(&h, t, m, id));
        assert_eq!(steps(&h), n + 1, "type {enh_type}: the drag is one undo step");
        // the unit's own button takes it out
        let b = at(&h, frame(&h, t, m), button).unwrap().center();
        click(&mut h, b);
        assert_eq!(raw(&h, t, m, "bypass"), 1.0, "type {enh_type}");
        // back on the Exciter, the Enhancer's dials
        h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "enhType", "value": 0})).unwrap();
        h.run_steps(6);
        assert!(at(&h, frame(&h, t, m), "Amount").is_some(), "type {enh_type}: the Enhancer's dials");
    }
}
