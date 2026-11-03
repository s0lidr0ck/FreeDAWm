//! Amp's VST editor in the Mix window: its size, its gestures as commands, hostile values and
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
fn amp_editor_is_its_vst_size() {
    let (h, t, m) = open("fs_amp", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(900.0, 495.0), "kWidth x kHeight");
    for c in &h.output().shapes {
        let r = c.shape.visual_bounding_rect().intersect(c.clip_rect);
        if r.is_positive() && r.min.x >= f.left() + 1.0 && r.max.x <= f.right() - 1.0 {
            assert!(r.min.y <= f.bottom() + 0.5, "a shape below the editor: {r:?}");
        }
    }
}

#[test]
fn amp_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_amp", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    assert!(at(&h, f, "Master").is_some(), "the Twin's face");
    // The amp select: the third segment is the Deluxe; its face has no Master.
    let sel = h
        .query_all_by_label("Twin Reverb '77|JTM45 '66|Deluxe Reverb '65|Super Reverb '65|Princeton Reverb '65|AC30 Top Boost '64|OR120 Graphic '72")
        .next()
        .unwrap()
        .rect();
    let seg_w = sel.width() / 7.0;
    click(&mut h, egui::pos2(sel.left() + seg_w * 2.5, sel.center().y));
    assert_eq!(raw(&h, t, m, "amp"), 2.0);
    assert!(at(&h, f, "Master").is_none(), "the Deluxe has no Master");
    // KEY_KNOBS[Deluxe][0] is dlxVolume: its dial ("Volume") drags it.
    let before = param(&h, t, m, "dlxVolume");
    let knob = at(&h, f, "Volume").unwrap().center();
    drag(&mut h, knob, knob - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "dlxVolume") - (before + 0.2)).abs() < 0.02);
    // the VST's Undo: back before the drag
    let undo_btn = at(&h, f, "Undo").unwrap().center();
    click(&mut h, undo_btn);
    assert!((param(&h, t, m, "dlxVolume") - before).abs() < 1e-6);
}

#[test]
fn amp_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_amp", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(900.0, 495.0));
    for a in 0..7 {
        h.state_mut().engine.session_mut().track_mut(t).unwrap().mixer.module_mut(m).unwrap().params.insert("amp".into(), a as f32);
        h.run_steps(2);
    }
    let knob = at(&h, f, "Input").unwrap().center();
    drag(&mut h, knob, knob - vec2(0.0, 30.0), Modifiers::NONE);
    wheel_notches(&mut h, knob, 1.0);
}

#[test]
fn amp_editor_follows_undo() {
    let (mut h, t, m) = open("fs_amp", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"amp": 5.0}})).unwrap();
    h.run_steps(2);
    assert!(at(&h, f, "Brilliant").is_some(), "the AC30's face");
    undo(&mut h);
    assert!(at(&h, f, "Master").is_some(), "the Twin's again");
}
