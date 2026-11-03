//! Comp's VST editor in the Mix window: its size, its gestures as commands, hostile values and
//! undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::SoundApp;

/// The node labelled `label` inside the editor `f` (the DAW has its own "Input" elsewhere).
fn at(h: &Harness<SoundApp>, f: egui::Rect, label: &str) -> Option<egui::Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.contains_rect(*r))
}

#[test]
fn comp_editor_is_its_vst_size() {
    let (h, t, m) = open("fs_comp", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(960.0, 620.0), "setSize (960, 620)");
    // Nothing painted below the editor in its column: the space is blank.
    for c in &h.output().shapes {
        let r = c.shape.visual_bounding_rect().intersect(c.clip_rect);
        if r.is_positive() && r.min.x >= f.left() + 1.0 && r.max.x <= f.right() - 1.0 {
            assert!(r.min.y <= f.bottom() + 0.5, "a shape below the editor: {r:?}");
        }
    }
}

#[test]
fn comp_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_comp", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // The Threshold dial (lane A's amount): 50 px up is +0.4.
    let before = param(&h, t, m, "amount");
    let knob = h.get_by_label("Threshold").rect().center();
    drag(&mut h, knob, knob - vec2(0.0, 50.0), Modifiers::NONE);
    let after = param(&h, t, m, "amount");
    assert!((after - (before + 0.4)).abs() < 0.02, "amount {before} -> {after}");

    // Select lane B in the mixer (its header), then the same knob edits bAmount.
    let mixer = h.get_by_label("Lane mixer").rect();
    let w = (mixer.width() / 4.0).floor();
    click(&mut h, pos2(mixer.left() + w * 2.5, mixer.top() + 8.0));
    let b_before = param(&h, t, m, "bAmount");
    let a_now = param(&h, t, m, "amount");
    let knob = at(&h, f, "Input").unwrap().center(); // lane B runs the 1176: "Input"
    drag(&mut h, knob, knob - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "bAmount") - (b_before + 0.2)).abs() < 0.02, "lane B's amount moved");
    assert_eq!(param(&h, t, m, "amount"), a_now, "lane A's did not");

    // A crossover drag moves its split parameter, as one undo step.
    let xo = h.get_by_label("Crossover").rect();
    let lo = raw(&h, t, m, "xoverLo");
    let steps = h.state().engine.undo_history().len();
    // the low divider sits at ~180 Hz: 20..20k across the plot
    let plot_w = xo.width() - 8.0;
    let x180 = xo.left() + 4.0 + plot_w * (180f32 / 20.0).ln() / 1000f32.ln();
    drag(&mut h, pos2(x180, xo.center().y), pos2(x180 - 30.0, xo.center().y), Modifiers::NONE);
    assert!(raw(&h, t, m, "xoverLo") < lo - 0.05, "the low crossover moved down");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "one undo step");

    // The style strip picks the selected lane's circuit; Undo (the VST's) puts it back.
    let strip_y = f.top() + 12.0 + 30.0 + 10.0 + 17.0;
    click(&mut h, pos2(f.left() + 22.0 + 916.0 * 0.45, strip_y)); // the 5th cell: G Bus
    assert_eq!(raw(&h, t, m, "bStyle"), 4.0, "lane B is now G Bus");
    let undo_btn = h.get_by_label("Undo").rect().center();
    click(&mut h, undo_btn);
    assert_eq!(raw(&h, t, m, "bStyle"), 1.0, "the VST's Undo restores the 1176");
    let _ = json!({});
}

#[test]
fn comp_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_comp", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(960.0, 620.0));
    // gestures over hostile values stay finite
    let knob = h.get_by_label("Threshold").rect().center();
    drag(&mut h, knob, knob - vec2(0.0, 30.0), Modifiers::NONE);
    wheel_notches(&mut h, knob, 2.0);
    let xo = h.get_by_label("Crossover").rect().center();
    wheel_notches(&mut h, xo, -1.0);
    double_click(&mut h, xo);
    h.run_steps(2);
    for id in ["amount", "xoverLo", "xoverHi"] {
        assert!(raw(&h, t, m, id).is_finite(), "{id}");
    }
}

#[test]
fn comp_editor_follows_undo() {
    let (mut h, t, m) = open("fs_comp", vec2(1800.0, 1000.0), "dark");
    let knob = h.get_by_label("Threshold").rect().center();
    let before = raw(&h, t, m, "amount");
    drag(&mut h, knob, knob - vec2(0.0, 40.0), Modifiers::NONE);
    assert_ne!(raw(&h, t, m, "amount"), before);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "amount"), before, "one undo step for the drag");
    // A change made elsewhere shows on the next frame: the 1176's knob names.
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"style": 1.0}})).unwrap();
    h.run_steps(2);
    let f = frame(&h, t, m);
    assert!(at(&h, f, "Input").is_some(), "the 1176 calls its amount Input");
    undo(&mut h);
    assert!(at(&h, f, "Threshold").is_some());
}

#[test]
fn comp_slim_editor_fits_the_drawer_and_its_knobs_work() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_comp");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
    h.run_steps(6);
    let f = frame(&h, t, m);
    assert!((f.width() - soundcraft_ui_egui::module_drawer::DRAWER_W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 900.0, "about a strip tall: {}", f.height());
    // The same knob, the same command and undo step as the full editor.
    let before = param(&h, t, m, "amount");
    let knob = at(&h, f, "Threshold").expect("Threshold in the slim layout").center();
    drag(&mut h, knob, knob - vec2(0.0, 50.0), Modifiers::NONE);
    let after = param(&h, t, m, "amount");
    assert!((after - (before + 0.4)).abs() < 0.02, "amount {before} -> {after}");
    undo(&mut h);
    assert_eq!(param(&h, t, m, "amount"), before, "one undo step");
    // A slot's knob is its lane's level.
    let lvl = raw(&h, t, m, "aLevel");
    let k = at(&h, f, "A level").expect("lane A's level knob").center();
    drag(&mut h, k, k + vec2(0.0, 25.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "aLevel") < lvl - 0.1, "lane A's level went down");
    // The flag off: the full editor again.
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(960.0, 620.0));
}
