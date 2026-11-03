//! Shale's VST editor in the Mix window: its size (640, 896 with More), its gestures going
//! through commands (the threshold dial and line, a mic button), hostile values, and following
//! undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, vec2};
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::editors::shale;

#[test]
fn shale_editor_is_its_vst_size() {
    let (mut h, t, m) = open("fs_shale", vec2(1800.0, 1000.0), "dark");
    assert_eq!(frame(&h, t, m).size(), vec2(940.0, 640.0), "max (640, requiredHeight() = 637)");
    let steps = h.state().engine.undo_history().len();
    let more = h.get_by_label("More").rect().center();
    click(&mut h, more);
    assert_eq!(frame(&h, t, m).size(), vec2(940.0, 896.0), "637 + 259 with More");
    assert_eq!(h.state().engine.undo_history().len(), steps, "More is editor state");
}

#[test]
fn shale_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_shale", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let steps = h.state().engine.undo_history().len();
    // The Threshold dial: one undo step per drag.
    let at = shale::spot(f, false, "threshold").unwrap();
    let before = param(&h, t, m, "threshold");
    drag(&mut h, at, at - vec2(0.0, 25.0), Modifiers::NONE);
    let after = param(&h, t, m, "threshold");
    assert!((after - (before + 25.0 * 0.008)).abs() < 0.02, "{before} -> {after}");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1);
    // The threshold line on the display: the same parameter, one step.
    let from = shale::spot(f, false, "db:-30").unwrap();
    let to = shale::spot(f, false, "db:-12").unwrap();
    drag(&mut h, from, to, Modifiers::NONE);
    let v = param(&h, t, m, "threshold");
    assert!((v - 54.0 / 66.0).abs() < 0.03, "the line put the threshold at -12 dB: {v}");
    assert_eq!(h.state().engine.undo_history().len(), steps + 2);
    // A mic button: Kick In (index 0), one step.
    let kick = shale::spot(f, false, "mic:0").unwrap();
    click(&mut h, kick);
    assert_eq!(raw(&h, t, m, "instrument"), 0.0);
    assert_eq!(h.state().engine.undo_history().len(), steps + 3);
    // The editor's own Undo puts the mic back (as one DAW step).
    let undo_btn = h.get_by_label("Undo").rect().center();
    click(&mut h, undo_btn);
    assert_eq!(raw(&h, t, m, "instrument"), 3.0, "Undo restores the snapshot");
}

#[test]
fn shale_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_shale", vec2(1800.0, 1000.0), "light");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    let more = h.get_by_label("More").rect().center();
    click(&mut h, more);
    assert_eq!(frame(&h, t, m).size(), vec2(940.0, 896.0));
}

#[test]
fn shale_editor_follows_undo() {
    let (mut h, t, m) = open("fs_shale", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"link": 1.0}})).unwrap();
    h.run_steps(3);
    // Kit Link on: the group row takes clicks.
    let f = frame(&h, t, m);
    let l = shale::layout(f, false);
    let b = egui::pos2(l.group.left() + l.group.width() * 0.375, l.group.center().y);
    click(&mut h, b);
    assert_eq!(raw(&h, t, m, "linkGroup"), 1.0, "group B");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "linkGroup"), 0.0);
    undo(&mut h);
    // Kit Link off again: the group row is inactive (no clicks).
    click(&mut h, b);
    assert_eq!(raw(&h, t, m, "linkGroup"), 0.0, "inactive while Kit Link is off");
}
