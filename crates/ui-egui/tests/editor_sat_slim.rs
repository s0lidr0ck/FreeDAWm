//! Sat's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its header, a
//! dial drag, the circuit menu, the display toggle and More (per module), and the full editor.

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

fn has(h: &Harness<SoundApp>, f: Rect, label: &str) -> bool {
    at(h, f, label).is_some()
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_sat");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

#[test]
fn the_header_stays_inside_the_drawer() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 760.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Circuit"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the header"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = raw(&h, t, m, "drive");
    let k = at(&h, f, "Drive").expect("the Drive dial").center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "drive") > before, "Drive went up");
    assert_eq!(steps(&h), n + 1, "one undo step");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "drive"), before);
    // A small one too.
    let f = frame(&h, t, m);
    let before = raw(&h, t, m, "air");
    let k = at(&h, f, "Air").expect("the Air dial").center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "air") > before, "Air went up");
}

#[test]
fn the_circuit_menu_picks_with_the_editors_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let start = raw(&h, t, m, "circuit");
    let n = steps(&h);
    let c = at(&h, f, "Circuit").unwrap();
    click(&mut h, egui::pos2(c.right() - 30.0, c.center().y));
    let fet = h.get_by_label("FET").rect().center();
    click(&mut h, fet);
    assert_eq!(raw(&h, t, m, "circuit"), 4.0, "FET");
    assert_eq!(steps(&h), n + 1, "one undo step");
    // The VST's own Undo (the pick pushed a snapshot) puts it back.
    let f = frame(&h, t, m);
    let u = at(&h, f, "Undo").unwrap().center();
    click(&mut h, u);
    assert_eq!(raw(&h, t, m, "circuit"), start);
}

#[test]
fn the_variant_switch_stays_a_segmented_control() {
    let (mut h, t, m) = slim();
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"circuit": 0.0}})).unwrap();
    h.run_steps(4);
    let f = frame(&h, t, m);
    let r = at(&h, f, "Nickel|Steel|Ferrite").expect("Iron's variants");
    click(&mut h, egui::pos2(r.right() - r.width() / 6.0, r.center().y));
    assert_eq!(raw(&h, t, m, "variant"), 2.0, "Ferrite");
}

#[test]
fn more_opens_and_closes() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Eco|Normal|Insane"), "More starts closed");
    let h0 = f.height();
    let at_ = at(&h, f, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Eco|Normal|Insane") && has(&h, f, "Stereo|M/S") && has(&h, f, "Auto Gain"), "the foot is in More");
    assert!(f.height() > h0, "the editor grew");
    // Quality is one undo step, as in the full editor.
    let n = steps(&h);
    let q = at(&h, f, "Eco|Normal|Insane").unwrap();
    click(&mut h, egui::pos2(q.right() - 10.0, q.center().y));
    assert_eq!(raw(&h, t, m, "quality"), 2.0, "Insane");
    assert_eq!(steps(&h), n + 1);
    let at_ = at(&h, frame(&h, t, m), "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    assert!(!has(&h, frame(&h, t, m), "Eco|Normal|Insane"), "closed again");
}

#[test]
fn two_sats_keep_their_own_display_and_more() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_sat");
    let (t2, m2) = add(&mut app, "Bass", "fs_sat");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Harmonic signature") && has(&h, f2, "Harmonic signature"));
    let tog = at(&h, f1, "Harmonics|Meters").unwrap();
    click(&mut h, egui::pos2(tog.right() - tog.width() / 4.0, tog.center().y));
    let at_ = at(&h, f1, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "In / out meters"), "the first shows its meters");
    assert!(has(&h, f2, "Harmonic signature"), "the second still shows its harmonics");
    assert!(!has(&h, f1, "100 Hz|1 kHz|5 kHz"), "the tone tabs belong to the harmonics view");
    assert!(has(&h, f1, "Eco|Normal|Insane") && !has(&h, f2, "Eco|Normal|Insane"), "More is per module");
}

#[test]
fn the_full_editor_is_unchanged_without_the_flag() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 466.0), "the full editor");
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": true})).unwrap();
    h.run_steps(4);
    assert!(!has(&h, frame(&h, t, m), "Eco|Normal|Insane"), "a More tab sent to the full editor is not kept for the slim one");
}
