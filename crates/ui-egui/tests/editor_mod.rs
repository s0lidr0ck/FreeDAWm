//! ModEngine's VST editor in the Mix window: its size, its gestures as commands (greyed dials
//! ignore them), the calibration wizard, hostile state and following undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

fn set(h: &mut Harness<SoundApp>, t: TrackId, m: ModuleId, params: serde_json::Value) {
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": params})).unwrap();
    h.run_steps(3);
}

/// Row one's knob `col` (pad 24 / 15, header 26, gap 14, circuit 58, 4, blurb 16, gap 14; six
/// 120 px cells beside the 128 px Link column; knob 82).
fn row1_knob(f: Rect, col: usize) -> Pos2 {
    pos2(f.left() + 24.0 + 120.0 * col as f32 + 60.0, f.top() + 147.0 + 41.0)
}

/// Row two's knob `col` (a 60 px knob in a 62 x 90 dial at the row's top).
fn row2_knob(f: Rect, col: usize) -> Pos2 {
    pos2(f.left() + 24.0 + 120.0 * col as f32 + 60.0, f.top() + 147.0 + 112.0 + 14.0 + 30.0)
}

fn button(h: &Harness<SoundApp>, f: Rect, label: &str) -> Pos2 {
    h.get_all_by_label(label).map(|n| n.rect()).find(|r| f.contains_rect(*r)).expect("the button is in the editor").center()
}

#[test]
fn mod_editor_is_its_vst_size() {
    let (h, t, m) = open("fs_mod", vec2(1800.0, 1000.0), "dark");
    // 424 + 5 switch rows x (26 + 3 + 16) + 4 x 5.
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 669.0));
}

#[test]
fn mod_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_mod", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let rate = row1_knob(f, 2);
    drag(&mut h, rate, rate - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "rate") - 0.7).abs() < 0.01, "Rate 0.5 + 25 x 0.008: {}", raw(&h, t, m, "rate"));
    undo(&mut h);
    assert!((raw(&h, t, m, "rate") - 0.5).abs() < 1e-4, "a drag is one undo step");
    // Regen is greyed on Dim D: a drag does nothing.
    let regen = row2_knob(f, 0);
    drag(&mut h, regen, regen - vec2(0.0, 40.0), Modifiers::NONE);
    assert_eq!(raw(&h, t, m, "regen"), 0.0, "an inactive dial ignores a drag");
    // On Mistress it is live.
    set(&mut h, t, m, json!({"circuit": 6.0}));
    drag(&mut h, regen, regen - vec2(0.0, 40.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "regen") - 0.32).abs() < 0.01);
    // Rate is greyed once synced; Tempo comes alive.
    set(&mut h, t, m, json!({"sync": 1.0}));
    drag(&mut h, rate, rate - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "rate") - 0.5).abs() < 1e-4, "Rate is inactive while synced");
    // The editor's Undo restores the snapshot before the regen drag.
    let u = button(&h, f, "Undo");
    click(&mut h, u);
    assert_eq!(raw(&h, t, m, "regen"), 0.0);
}

#[test]
fn mod_calibration_writes_the_set_points_and_links() {
    let (mut h, t, m) = open("fs_mod", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    set(&mut h, t, m, json!({"depth": 0.3}));
    let cal = button(&h, f, "Calibrate");
    click(&mut h, cal);
    assert_eq!(raw(&h, t, m, "depth"), 0.5, "step 1: Depth at 5");
    let next = button(&h, f, "Next");
    click(&mut h, next);
    assert_eq!(raw(&h, t, m, "bypass"), 1.0, "step 2: bypassed");
    click(&mut h, next);
    assert_eq!((raw(&h, t, m, "bypass"), raw(&h, t, m, "depth")), (0.0, 0.0), "step 3: on, Depth 0");
    set(&mut h, t, m, json!({"level": 0.6}));
    click(&mut h, next);
    assert!((raw(&h, t, m, "linkLo") - 0.6).abs() < 1e-4);
    assert_eq!(raw(&h, t, m, "depth"), 1.0, "step 4: Depth 10");
    set(&mut h, t, m, json!({"level": 0.8}));
    let done = button(&h, f, "Done");
    click(&mut h, done);
    assert!((raw(&h, t, m, "linkHi") - 0.8).abs() < 1e-4);
    assert_eq!(raw(&h, t, m, "link"), 1.0, "Link on after a completed calibration");
    assert!((raw(&h, t, m, "depth") - 0.3).abs() < 1e-4, "Depth restored");
    assert!(h.query_by_label("Done").is_none(), "the panel closed");
}

#[test]
fn mod_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_mod", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 669.0));
    for c in 0..14 {
        set(&mut h, t, m, json!({"circuit": c as f64, "variant": 5.0}));
    }
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 669.0));
}

#[test]
fn mod_editor_follows_undo() {
    let (mut h, t, m) = open("fs_mod", vec2(1800.0, 1000.0), "dark");
    set(&mut h, t, m, json!({"circuit": 8.0}));
    assert!(h.query_by_label("Script|Block").is_some(), "Phase 90's variants");
    undo(&mut h);
    assert!(h.query_by_label("Script|Block").is_none());
    assert!(h.query_by_label("1|2|3|4|1 + 4|2 + 3").is_some(), "Dim D's on the next frame");
}
