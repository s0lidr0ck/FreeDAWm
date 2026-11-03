//! Amp's slim editor (View > Mix Window Views > Slim Plugin Editors): its width and header, a
//! face dial as one undo step, the amp menu with the editor's Undo, each amp's own front panel,
//! and More (cabinet, oversampling) per module.

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
    let (t, m) = add(&mut app, "Pad", "fs_amp");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1100.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

fn set(h: &mut Harness<SoundApp>, t: TrackId, m: ModuleId, params: serde_json::Value) {
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": params})).unwrap();
    h.run_steps(4);
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

#[test]
fn the_slim_amp_is_the_drawers_width_with_its_header_inside() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "Bypass", "Amp", "Auto Level"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(f.height() < 900.0, "about a strip tall: {}", f.height());
}

#[test]
fn a_face_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = raw(&h, t, m, "treble");
    let k = at(&h, f, "Treble").expect("the Twin's Treble").center();
    drag(&mut h, k, k + vec2(0.0, 30.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "treble") < before, "Treble went down");
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_amp_menu_picks_with_the_editors_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let at_ = at(&h, f, "Amp").unwrap().center();
    click(&mut h, at_);
    let ac30 = h.get_by_label("AC30 Top Boost '64 30 W \u{00B7} EL84s").rect().center();
    click(&mut h, ac30);
    assert_eq!(raw(&h, t, m, "amp"), 5.0, "the AC30");
    assert_eq!(steps(&h), n + 1, "one undo step");
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Brilliant") && has(&h, f, "Cut"), "the AC30's own panel");
    let at_ = at(&h, f, "Undo").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "amp"), 0.0, "the Twin again");
}

#[test]
fn each_amp_shows_its_own_front_panel() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Pull Boost") && has(&h, f, "Bright"), "the Twin's Pull Boost and Bright");
    let n = steps(&h);
    let pb = at(&h, f, "Pull Boost").unwrap().center();
    click(&mut h, pb);
    assert_eq!(raw(&h, t, m, "pullBoost"), 1.0, "the Pull Boost pulled");
    assert_eq!(steps(&h), n + 1);
    set(&mut h, t, m, json!({"amp": 4.0}));
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Pull Boost") && !has(&h, f, "Bright"), "the Princeton has neither");
    assert!(has(&h, f, "Intensity"));
}

#[test]
fn more_holds_the_cabinet_and_oversampling_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_amp");
    let (t2, m2) = add(&mut app, "Bass", "fs_amp");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1100.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    assert!(!has(&h, f1, "Load IR..."), "More starts closed");
    let at_ = at(&h, f1, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Load IR..."), "the cabinet is in More");
    assert!(!has(&h, f2, "Load IR..."), "the other Amp's More stays closed");
    let n = steps(&h);
    let q = at(&h, f1, "2x|4x").expect("Oversampling");
    click(&mut h, egui::pos2(q.right() - q.width() / 4.0, q.center().y));
    assert_eq!(raw(&h, t1, m1, "quality"), 1.0, "4x");
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn the_full_editor_is_unchanged_with_slim_off() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).width(), 900.0, "the VST's size");
}
