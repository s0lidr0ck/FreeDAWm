//! ChamberMade's slim editor (View > Mix Window Views > Slim Plugin Editors): its width and
//! header, the one display (plan or echogram) per module, the plan's Add rows and the inspector
//! following them, Mix as one undo step, and More (the plugin's own) per module.

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

/// The lowest node labelled `label` inside `f` (the More disclosure under the header's More).
fn last(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).filter(|r| f.expand(1.0).contains_rect(*r)).max_by(|a, b| a.top().total_cmp(&b.top()))
}

fn has(h: &Harness<SoundApp>, f: Rect, label: &str) -> bool {
    at(h, f, label).is_some()
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

fn two() -> (Harness<'static, SoundApp>, (TrackId, ModuleId), (TrackId, ModuleId)) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let a = add(&mut app, "Pad", "fs_chamber");
    let b = add(&mut app, "Bass", "fs_chamber");
    app.ui.drawers.insert(a.0, a.1);
    app.ui.drawers.insert(b.0, b.1);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1100.0), "dark");
    h.run_steps(6);
    (h, a, b)
}

#[test]
fn the_slim_chamber_is_the_drawers_width_with_its_header_inside() {
    let (h, (t, m), _) = two();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["More", "Bypass", "Room plan", "Mix", "Decay", "Add Glass Gobo"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(f.height() < 900.0, "about a strip tall: {}", f.height());
}

#[test]
fn mix_is_one_undo_step() {
    let (mut h, (t, m), _) = two();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = raw(&h, t, m, "mix");
    let k = at(&h, f, "Mix").unwrap();
    let from = egui::pos2(k.center().x, k.top() + 30.0);
    drag(&mut h, from, from - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "mix") > before, "Mix went up");
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn add_puts_an_object_in_and_the_inspector_follows() {
    let (mut h, (t, m), _) = two();
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Height"), "the Room's inspector");
    let n = steps(&h);
    let add_ = at(&h, f, "Add Pillar").unwrap().center();
    click(&mut h, add_);
    h.run_steps(3);
    assert_eq!(raw(&h, t, m, "o1t"), 1.0, "a pillar in the first slot");
    assert_eq!(steps(&h), n + 1, "one undo step");
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Size") && has(&h, f, "Remove"), "the pillar's inspector");
    let rm = last(&h, f, "Remove").unwrap().center();
    click(&mut h, rm);
    h.run_steps(3);
    assert_eq!(raw(&h, t, m, "o1t"), 0.0, "removed");
    assert!(has(&h, frame(&h, t, m), "Height"), "back to the Room");
}

#[test]
fn the_display_and_more_are_per_module() {
    let (mut h, (t1, m1), (t2, m2)) = two();
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Room plan") && has(&h, f2, "Room plan"));
    let tog = at(&h, f1, "Plan|Echoes").expect("the display's toggle");
    click(&mut h, egui::pos2(tog.right() - tog.width() / 4.0, tog.center().y));
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Echogram") && !has(&h, f1, "Room plan"), "the first shows its echoes");
    assert!(has(&h, f2, "Room plan"), "the second still shows its plan");

    assert!(!has(&h, f1, "Sound paths"), "More starts closed");
    let more = last(&h, f1, "More").unwrap().center();
    click(&mut h, more);
    h.run_steps(3);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Sound paths") && has(&h, f1, "Pre-delay"), "More's buttons and knobs");
    assert!(!has(&h, f2, "Sound paths"), "the other Chamber's More stays closed");
    // More is the plugin's own (its uiState): the full editor opens with its card.
    let session = h.state().engine.session();
    let bytes = session.track(t1).and_then(|t| t.mixer.module(m1)).and_then(|i| i.state_bytes()).expect("the uiState");
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v.get("more"), Some(&json!(true)));
}

#[test]
fn the_full_editor_is_unchanged_with_slim_off() {
    let (mut h, (t, m), _) = two();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(1190.0, 648.0), "the VST's size");
}
