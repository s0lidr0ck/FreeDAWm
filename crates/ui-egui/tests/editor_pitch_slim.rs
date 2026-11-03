//! PitchEngine's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its
//! header, the Style and Interval menus, the 5th Mode switch, the knobs and its More section.

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
    let (t, m) = add(&mut app, "Pad", "fs_pitch");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
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
fn it_is_the_drawers_width_and_the_header_fits() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 900.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Preset Init", "Style"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_main_knob_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "dry");
    let k = at(&h, f, "Dry").expect("the Dry knob").center();
    drag(&mut h, k, k + vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "dry") - (before - 0.2)).abs() < 0.01, "25 px down is -0.2");
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_style_menu_picks_with_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let at_ = at(&h, f, "Style").unwrap().center();
    click(&mut h, at_);
    let whammy = h.get_by_label("Whammy").rect().center();
    click(&mut h, whammy);
    assert_eq!(raw(&h, t, m, "style"), 4.0, "Whammy");
    assert_eq!(steps(&h), n + 1, "one DAW undo step");
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "Undo").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "style"), 0.0, "POG again, by the editor's Undo");
}

#[test]
fn interval_and_fifth_mode_show_only_where_they_apply() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Interval"), "no Interval on POG");
    assert!(!has(&h, f, "+5th|+Oct 5th|-5th"), "no 5th Mode with the 5th off");
    set(&mut h, t, m, json!({"fifth": 0.6}));
    let f = frame(&h, t, m);
    let seg = at(&h, f, "+5th|+Oct 5th|-5th").expect("5th Mode with the 5th up");
    click(&mut h, egui::pos2(seg.right() - seg.width() / 6.0, seg.center().y));
    assert_eq!(raw(&h, t, m, "fifthMode"), 2.0, "-5th");

    set(&mut h, t, m, json!({"style": 4.0}));
    let f = frame(&h, t, m);
    let n = steps(&h);
    let at_ = at(&h, f, "Interval").expect("Whammy's Interval").center();
    click(&mut h, at_);
    let fifth = h.get_by_label("+5th").rect().center();
    click(&mut h, fifth);
    assert_eq!(raw(&h, t, m, "interval"), 2.0, "+5th");
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn more_holds_the_shaping_row_and_is_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_pitch");
    let (t2, m2) = add(&mut app, "Bass", "fs_pitch");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    assert!(!has(&h, f1, "Filter"), "More starts closed");
    let at_ = at(&h, f1, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    for label in ["Detune", "Attack", "Filter", "Q", "Width", "Grain", "Input", "Level"] {
        assert!(has(&h, f1, label), "{label} is in More");
    }
    assert!(!has(&h, f2, "Filter"), "the other module's More stays closed");
    let n = steps(&h);
    let before = param(&h, t1, m1, "lpf");
    let k = at(&h, f1, "Filter").unwrap().center();
    drag(&mut h, k, k + vec2(0.0, 25.0), Modifiers::NONE);
    assert!(param(&h, t1, m1, "lpf") < before - 0.1, "the Filter dial moves lpf");
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_flag_picks_the_layout() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 412.0), "the full editor");
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": true})).unwrap();
    h.run_steps(4);
    assert!(!has(&h, frame(&h, t, m), "Filter"), "a More tab sent to the full editor is not kept");
}
