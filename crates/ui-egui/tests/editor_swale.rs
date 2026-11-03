//! Swale's VST editor in the Mix window: its size (More grows the window as the VST does), its
//! gestures going through commands, hostile values, and following undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, vec2};
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::editors::swale;

#[test]
fn swale_editor_is_its_vst_size() {
    let (mut h, t, m) = open("fs_swale", vec2(1800.0, 1000.0), "dark");
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 640.0), "max (640, requiredHeight())");
    // More: editor state (no command), the window gets the row back (640 + 99), the screen keeps
    // its size; closing it gives the row back.
    let steps = h.state().engine.undo_history().len();
    let more = h.get_by_label("More").rect().center();
    click(&mut h, more);
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 739.0));
    assert_eq!(h.state().engine.undo_history().len(), steps, "More is not an undo step");
    let more = h.get_by_label("More").rect().center();
    click(&mut h, more);
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 640.0));
}

#[test]
fn swale_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_swale", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let steps = h.state().engine.undo_history().len();
    let at = swale::spot(f, false, "attack").unwrap();
    let before = param(&h, t, m, "attack");
    drag(&mut h, at, at - vec2(0.0, 30.0), Modifiers::NONE);
    let after = param(&h, t, m, "attack");
    assert!((after - (before + 30.0 * 0.008)).abs() < 0.02, "{before} -> {after}");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "a drag is one undo step");
    // The wheel on Mix: one step per notch.
    let mix = swale::spot(f, false, "mix").unwrap();
    drag(&mut h, mix, mix + vec2(0.0, 50.0), Modifiers::NONE);
    assert!(param(&h, t, m, "mix") < 0.7);
    // Rhythmic swaps Sens for Rate.
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"mode": 2.0}})).unwrap();
    h.run_steps(3);
    let rate = swale::spot(f, false, "sens").unwrap();
    let r0 = raw(&h, t, m, "rate");
    drag(&mut h, rate, rate - vec2(0.0, 40.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "rate") > r0, "the Rate dial took the slot");
}

#[test]
fn swale_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_swale", vec2(1800.0, 1000.0), "light");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    let more = h.get_by_label("More").rect().center();
    click(&mut h, more);
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 739.0));
}

#[test]
fn swale_editor_follows_undo() {
    let (mut h, t, m) = open("fs_swale", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"atkSync": 1.0}})).unwrap();
    h.run_steps(3);
    // Synced: the Attack slot is the note dial (a drag moves atkNote, not attack).
    let f = frame(&h, t, m);
    undo(&mut h);
    let at = swale::spot(f, false, "attack").unwrap();
    let a0 = param(&h, t, m, "attack");
    drag(&mut h, at, at - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "attack") > a0, "after undo the time dial is back in the slot");
}
