//! FilterEngine's VST editor in the Mix window: its height per circuit, its gestures as commands
//! (Cutoff, the step strip), hostile state and following undo.

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

/// `requiredHeight()`: 522, plus 13 + n x 50 + (n - 1) x 6 for n switch rows.
fn height(rows: f32) -> f32 {
    if rows == 0.0 { 522.0 } else { 522.0 + 13.0 + rows * 50.0 + (rows - 1.0) * 6.0 }
}

/// The top of the main row with the Q-Tron's two switch rows (pad 14, header 26, gap 13, circuit
/// 30, 4, blurb 30, gap 13, 106 of rows, gap 13).
const MAIN_TOP: f32 = 14.0 + 26.0 + 13.0 + 30.0 + 4.0 + 30.0 + 13.0 + 106.0 + 13.0;

fn cutoff_knob(f: Rect) -> Pos2 {
    pos2(f.left() + 22.0 + 219.0 * 0.5, f.top() + MAIN_TOP + 53.0)
}

/// Mod source segment `i` of four (300 px wide at the left of the mod row).
fn mod_seg(f: Rect, i: usize) -> Pos2 {
    pos2(f.left() + 22.0 + 75.0 * (i as f32 + 0.5), f.top() + MAIN_TOP + 136.0 + 13.0 + 13.0)
}

/// The step strip on Steps: right of the 230 px knob block and 14 px, inside the panel's 16 / 12
/// inset, 6 px in from the top and bottom.
fn strip(f: Rect) -> Rect {
    let top = f.top() + MAIN_TOP + 136.0 + 13.0 + 26.0 + 8.0 + 12.0 + 6.0;
    Rect::from_min_max(pos2(f.left() + 22.0 + 16.0 + 230.0 + 14.0, top), pos2(f.right() - 22.0 - 16.0, top + 96.0 - 12.0))
}

#[test]
fn filter_editor_is_its_vst_size() {
    let (mut h, t, m) = open("fs_filter", vec2(1800.0, 1000.0), "dark");
    assert_eq!(frame(&h, t, m).size(), vec2(920.0, height(2.0)), "Q-Tron: Range and Mode");
    set(&mut h, t, m, json!({"circuit": 2.0}));
    assert_eq!(frame(&h, t, m).height(), height(2.0), "Cry Baby: Inductor and Sweep");
    set(&mut h, t, m, json!({"circuit": 8.0}));
    assert_eq!(frame(&h, t, m).height(), height(1.0), "Wasp: Mode");
}

#[test]
fn filter_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_filter", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let at = cutoff_knob(f);
    drag(&mut h, at, at - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "cutoff") - 0.7).abs() < 0.01, "{}", raw(&h, t, m, "cutoff"));
    assert!(h.query_by_label("Sweep").is_some(), "the Q-Tron names Cutoff");
    undo(&mut h);
    assert!((raw(&h, t, m, "cutoff") - 0.5).abs() < 1e-4, "a drag is one undo step");

    // Steps: the strip appears; a drag on the first cell sets step 1 to its height.
    click(&mut h, mod_seg(f, 3));
    assert_eq!(raw(&h, t, m, "modSrc"), 3.0);
    let s = strip(f);
    let cell = s.width() / 8.0;
    let from = pos2(s.left() + cell * 0.5, s.top() + s.height() * 0.25);
    drag(&mut h, from, pos2(s.left() + cell * 1.5, s.top() + s.height() * 0.25), Modifiers::NONE);
    assert!((raw(&h, t, m, "step1") - 0.75).abs() < 0.03, "step 1 from the press: {}", raw(&h, t, m, "step1"));
    assert!((raw(&h, t, m, "step2") - 0.75).abs() < 0.03, "step 2 from the drag: {}", raw(&h, t, m, "step2"));
    undo(&mut h);
    assert!((raw(&h, t, m, "step1") - 0.2).abs() < 1e-4, "the strip gesture is one undo step");
    assert!((raw(&h, t, m, "step2") - 0.8).abs() < 1e-4);
    // Manual: no Depth.
    click(&mut h, mod_seg(f, 0));
    assert_eq!(raw(&h, t, m, "modSrc"), 0.0);
    assert!(h.query_by_label("Depth").is_none(), "Depth does nothing on Manual");
}

#[test]
fn filter_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_filter", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).width(), 920.0);
    for c in 0..9 {
        for src in 0..4 {
            set(&mut h, t, m, json!({"circuit": c as f64, "modSrc": src as f64}));
        }
        assert!(frame(&h, t, m).height() >= height(1.0));
    }
}

#[test]
fn filter_editor_follows_undo() {
    let (mut h, t, m) = open("fs_filter", vec2(1800.0, 1000.0), "dark");
    set(&mut h, t, m, json!({"circuit": 8.0}));
    assert_eq!(frame(&h, t, m).height(), height(1.0));
    assert!(h.query_by_label("Freq").is_some());
    undo(&mut h);
    assert_eq!(frame(&h, t, m).height(), height(2.0), "the next frame follows undo");
    assert!(h.query_by_label("Freq").is_none());
}
