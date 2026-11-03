//! LevelUp's VST editor in the Mix window: its size, its gestures as commands, hostile values and
//! undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::SoundApp;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.contains_rect(*r))
}

#[test]
fn levelup_editor_is_its_vst_size() {
    let (h, t, m) = open("fs_levelup", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(820.0, 580.0), "kW x kH");
    for c in &h.output().shapes {
        let r = c.shape.visual_bounding_rect().intersect(c.clip_rect);
        if r.is_positive() && r.min.x >= f.left() + 1.0 && r.max.x <= f.right() - 1.0 {
            assert!(r.min.y <= f.bottom() + 0.5, "a shape below the editor: {r:?}");
        }
    }
}

#[test]
fn levelup_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_levelup", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // the Target dial, one undo step
    let before = param(&h, t, m, "target");
    let steps = h.state().engine.undo_history().len();
    let knob = at(&h, f, "Target").unwrap().center();
    drag(&mut h, knob, knob - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "target") - (before + 0.2).min(1.0)).abs() < 0.02);
    assert_eq!(h.state().engine.undo_history().len(), steps + 1);
    // Deliver's Reset runs mix.module_action (the tab first)
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "Deliver"})).unwrap();
    h.run_steps(3);
    let reset = at(&h, f, "Reset").expect("the Deliver tab's Reset").center();
    let steps = h.state().engine.undo_history().len();
    click(&mut h, reset);
    assert!(h.state().ui.status.is_empty(), "the action ran: {}", h.state().ui.status);
    assert_eq!(h.state().engine.undo_history().len(), steps, "an action, not a parameter");
    // a platform row picks the platform; again clears it
    let spotify = at(&h, f, "Spotify").unwrap().center();
    click(&mut h, spotify);
    assert_eq!(raw(&h, t, m, "platform"), 1.0);
    click(&mut h, spotify);
    assert_eq!(raw(&h, t, m, "platform"), 0.0);
}

#[test]
fn levelup_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_levelup", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    for tab in ["Level", "Reference", "Deliver"] {
        h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": tab})).unwrap();
        h.run_steps(3);
        assert_eq!(frame(&h, t, m).size(), vec2(820.0, 580.0));
    }
}

#[test]
fn levelup_editor_follows_undo() {
    let (mut h, t, m) = open("fs_levelup", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let knob = at(&h, f, "Gate").unwrap().center();
    let before = raw(&h, t, m, "gate");
    drag(&mut h, knob, knob - vec2(0.0, 30.0), Modifiers::NONE);
    assert_ne!(raw(&h, t, m, "gate"), before);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "gate"), before);
}
