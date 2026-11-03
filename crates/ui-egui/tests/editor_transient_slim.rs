//! Transient's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its header,
//! the Source menu, the knobs and Focus selectors, and its More section.

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
    let (t, m) = add(&mut app, "Pad", "fs_transient");
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
fn it_is_the_drawers_width_and_the_header_fits() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 900.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Preset Init", "Source"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_big_knob_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "attack");
    let k = at(&h, f, "Attack").expect("the Attack knob").center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "attack") - (before + 0.2)).abs() < 0.01, "25 px up is +0.2, as the full editor's");
    assert_eq!(steps(&h), n + 1, "one undo step");
    undo(&mut h);
    assert!((param(&h, t, m, "attack") - before).abs() < 1e-6);
}

#[test]
fn the_source_menu_picks_with_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let at_ = at(&h, f, "Source").unwrap().center();
    click(&mut h, at_);
    let guitar = h.get_by_label("Guitar").rect().center();
    click(&mut h, guitar);
    assert_eq!(raw(&h, t, m, "source"), 2.0, "Guitar");
    assert_eq!(steps(&h), n + 1, "one DAW undo step");
    // The editor's own Undo (the family's pushUndo) puts Drums back.
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "Undo").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "source"), 0.0, "Drums again");
}

#[test]
fn the_focus_selectors_set_their_own_parameter() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let segs: Vec<Rect> = h.query_all_by_label("Full|Low|Mid|High").map(|n| n.rect()).filter(|r| f.contains_rect(*r)).collect();
    assert_eq!(segs.len(), 2, "Attack and Sustain each have their Focus");
    let (a, s) = if segs[0].left() < segs[1].left() { (segs[0], segs[1]) } else { (segs[1], segs[0]) };
    click(&mut h, egui::pos2(a.left() + a.width() * 7.0 / 8.0, a.center().y));
    assert_eq!(raw(&h, t, m, "attackFocus"), 3.0, "High");
    click(&mut h, egui::pos2(s.left() + s.width() * 3.0 / 8.0, s.center().y));
    assert_eq!(raw(&h, t, m, "sustainFocus"), 1.0, "Low");
}

#[test]
fn more_holds_the_small_dials_and_is_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_transient");
    let (t2, m2) = add(&mut app, "Bass", "fs_transient");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    let h0 = f1.height();
    assert!(!has(&h, f1, "Mix"), "More starts closed");
    let at_ = at(&h, f1, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(f1.height() > h0, "the editor grew");
    for label in ["Attack Time", "Sustain Time", "Mix", "Output"] {
        assert!(has(&h, f1, label), "{label} is in More");
    }
    assert!(!has(&h, f2, "Mix"), "the other module's More stays closed");
    // A small dial sends what the full editor's does: one undo step on its parameter.
    let n = steps(&h);
    let before = param(&h, t1, m1, "mix");
    let k = at(&h, f1, "Mix").unwrap().center();
    drag(&mut h, k, k + vec2(0.0, 25.0), Modifiers::NONE);
    assert!(param(&h, t1, m1, "mix") < before - 0.1);
    assert_eq!(steps(&h), n + 1);
    let at_ = at(&h, frame(&h, t1, m1), "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    assert!(!has(&h, frame(&h, t1, m1), "Mix"), "closed again");
}

#[test]
fn the_flag_picks_the_layout() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 441.0), "the full editor");
    // A More tab sent to the full editor is not kept for the slim one.
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": true})).unwrap();
    h.run_steps(4);
    assert!(!has(&h, frame(&h, t, m), "Mix"));
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(4);
    assert!(has(&h, frame(&h, t, m), "Mix"), "the slim layout takes it");
}
