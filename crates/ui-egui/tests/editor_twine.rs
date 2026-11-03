//! Twine's VST editor in the Mix window: its size, its gestures as commands, hostile values and
//! undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::SoundApp;

/// The node labelled `label` inside the editor `f`.
fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Rect {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.contains_rect(*r)).expect(label)
}

#[test]
fn twine_editor_is_its_vst_size() {
    let (h, t, m) = open("fs_twine", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(860.0, 534.0), "kWidth x kHeight");
    for c in &h.output().shapes {
        let r = c.shape.visual_bounding_rect().intersect(c.clip_rect);
        if r.is_positive() && r.min.x >= f.left() + 1.0 && r.max.x <= f.right() - 1.0 {
            assert!(r.min.y <= f.bottom() + 0.5, "a shape below the editor: {r:?}");
        }
    }
}

#[test]
fn twine_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_twine", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // Drive: 50 px up is +0.4, one undo step.
    let steps = h.state().engine.undo_history().len();
    let knob = at(&h, f, "Drive").center();
    drag(&mut h, knob, knob - vec2(0.0, 50.0), Modifiers::NONE);
    assert!((param(&h, t, m, "drive") - 0.4).abs() < 0.02);
    assert_eq!(h.state().engine.undo_history().len(), steps + 1);

    // The crossover strip (58 px): the low divider at 150 Hz, dragged left.
    let xo = at(&h, f, "Crossover");
    assert!((xo.height() - 58.0).abs() < 0.5, "kStripH");
    let lo = raw(&h, t, m, "xoverLo");
    let plot_w = xo.width() - 8.0;
    let x150 = xo.left() + 4.0 + plot_w * (150f32 / 20.0).ln() / 1000f32.ln();
    let steps = h.state().engine.undo_history().len();
    drag(&mut h, pos2(x150, xo.center().y), pos2(x150 - 30.0, xo.center().y), Modifiers::NONE);
    assert!(raw(&h, t, m, "xoverLo") < lo - 0.05, "the low crossover moved down");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "one undo step");

    // Multiband, then the VST's Undo puts the crossover back (a segment pick pushes too).
    let multi = at(&h, f, "Multiband").center();
    click(&mut h, multi);
    assert_eq!(raw(&h, t, m, "multi"), 1.0);
    let undo_btn = at(&h, f, "Undo").center();
    click(&mut h, undo_btn);
    assert!((raw(&h, t, m, "xoverLo") - lo).abs() < 1e-6, "Undo restores the snapshot before the drag");
}

#[test]
fn twine_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_twine", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(860.0, 534.0));
    let knob = at(&h, f, "Ceiling").center();
    drag(&mut h, knob, knob - vec2(0.0, 30.0), Modifiers::NONE);
    let xo = at(&h, f, "Crossover").center();
    wheel_notches(&mut h, xo, 1.0);
    for id in ["ceiling", "xoverLo", "xoverHi"] {
        assert!(raw(&h, t, m, id).is_finite(), "{id}");
    }
}

#[test]
fn twine_editor_follows_undo() {
    let (mut h, t, m) = open("fs_twine", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let knob = at(&h, f, "Drive").center();
    drag(&mut h, knob, knob - vec2(0.0, 40.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "drive") > 0.0);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "drive"), 0.0);
    // the foot line follows a change made elsewhere
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"character": 2.0}})).unwrap();
    h.run_steps(2);
    let loud = h.output().shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(tx) if tx.galley.job.text == "L"));
    assert!(loud, "the Loud note is drawn (letter by letter)");
}

#[test]
fn twine_controls_carry_the_vst_tooltips() {
    let (mut h, _t, _m) = open("fs_twine", vec2(1800.0, 1000.0), "dark");
    let tip = "How fast the limiter lets go. Short is louder and can pump; long is smoother.";
    assert!(h.query_by_label_contains(tip).is_none());
    let at = h.get_by_label("Release").rect().center();
    hover(&mut h, at);
    h.run_steps(60);
    assert!(h.query_by_label_contains(tip).is_some(), "the Release dial's setTooltip");
    // A Segmented (no response handed back): its tip by id.
    let at = h.get_by_label("Hard|Soft|Tape").rect().center();
    hover(&mut h, at);
    h.run_steps(60);
    assert!(h.query_by_label_contains("The clipper's corner.").is_some(), "the Clip shape's setTooltip");
}
