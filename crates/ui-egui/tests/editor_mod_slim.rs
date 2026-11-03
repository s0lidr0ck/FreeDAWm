//! ModEngine's slim editor (View > Mix Window Views > Slim Plugin Editors): its width and header,
//! a dial as one undo step, the circuit menu with the editor's Undo, Link and the calibration
//! wizard, and More per module with the switch rows sending the full editor's commands.

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
    let (t, m) = add(&mut app, "Pad", "fs_mod");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1200.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

#[test]
fn the_slim_mod_is_the_drawers_width_with_its_header_inside() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Circuit", "Link", "Calibrate"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(f.height() < 900.0, "about a strip tall: {}", f.height());
}

#[test]
fn a_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = raw(&h, t, m, "depth");
    let k = at(&h, f, "Depth").expect("the Depth dial").center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "depth") > before, "Depth went up");
    assert_eq!(steps(&h), n + 1, "one undo step");
    // a trim too
    let before = raw(&h, t, m, "tone");
    let k = at(&h, f, "Tone").expect("the Tone dial").center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    // Tone is unused on the Dim D: greyed, it does not move.
    assert_eq!(raw(&h, t, m, "tone"), before, "an unused trim stays");
}

#[test]
fn the_circuit_menu_picks_with_the_editors_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let at_ = at(&h, f, "Circuit").unwrap().center();
    click(&mut h, at_);
    let vibe = h.get_by_label("CE-2").rect().center();
    click(&mut h, vibe);
    assert_eq!(raw(&h, t, m, "circuit"), 2.0, "CE-2");
    assert_eq!(steps(&h), n + 1, "one undo step");
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "Undo").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "circuit"), 0.0, "Dim D again");
}

#[test]
fn calibrate_opens_the_wizard_in_the_column() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let h0 = f.height();
    let at_ = at(&h, f, "Calibrate").unwrap().center();
    click(&mut h, at_);
    h.run_steps(3);
    let f = frame(&h, t, m);
    assert!(f.height() > h0, "the wizard's panel takes its room");
    let cancel = at(&h, f, "Cancel").expect("the wizard's Cancel").center();
    click(&mut h, cancel);
    h.run_steps(3);
    assert!(!has(&h, frame(&h, t, m), "Cancel"), "the wizard closed");
}

#[test]
fn more_holds_the_switch_rows_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_mod");
    let (t2, m2) = add(&mut app, "Bass", "fs_mod");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1200.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    const SHAPE: &str = "Stock|Sine|Triangle|Trapezoid|Square";
    assert!(!has(&h, f1, SHAPE), "More starts closed");
    let at_ = at(&h, f1, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, SHAPE), "the Shape row is in More");
    assert!(!has(&h, f2, SHAPE), "the other Mod's More stays closed");
    let n = steps(&h);
    // five options in two rows of three: Trapezoid is the second row's first
    let seg = at(&h, f1, SHAPE).unwrap();
    let trap = egui::pos2(seg.left() + seg.width() / 6.0, seg.top() + seg.height() * 0.75);
    click(&mut h, trap);
    assert_eq!(raw(&h, t1, m1, "shape"), 3.0, "Trapezoid at {trap:?} in {f1:?}");
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn the_full_editor_is_unchanged_with_slim_off() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).width(), 900.0, "the VST's size");
}
