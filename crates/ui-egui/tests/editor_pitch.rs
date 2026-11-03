//! PitchEngine's VST editor in the Mix window: its size per style, its gestures as commands,
//! hostile state, and following undo.

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

/// The centre of the main row's dial `col` of `n` (`resized()`: pad 22 / 14, header 26, gap 13,
/// style 30, blurb 16, gap 13; the knob is min(w, 136 - 30) square at the column's top).
fn main_knob(f: Rect, col: usize, n: usize) -> Pos2 {
    let w = ((f.width() - 44.0) as i32 / n as i32) as f32;
    let s = w.min(106.0);
    pos2(f.left() + 22.0 + w * col as f32 + w * 0.5, f.top() + 14.0 + 26.0 + 13.0 + 30.0 + 16.0 + 13.0 + s * 0.5)
}

/// The centre of style segment `i` of 5.
fn style_seg(f: Rect, i: usize) -> Pos2 {
    let w = (f.width() - 44.0) / 5.0;
    pos2(f.left() + 22.0 + w * (i as f32 + 0.5), f.top() + 14.0 + 26.0 + 13.0 + 15.0)
}

/// A button of the editor by its label.
fn button(h: &Harness<SoundApp>, f: Rect, label: &str) -> Pos2 {
    h.get_all_by_label(label).map(|n| n.rect()).find(|r| f.contains_rect(*r)).expect("the button is in the editor").center()
}

#[test]
fn pitch_editor_is_its_vst_size() {
    let (mut h, t, m) = open("fs_pitch", vec2(1800.0, 1000.0), "dark");
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 412.0), "POG with the 5th down: no switch row");
    set(&mut h, t, m, json!({"fifth": 0.5}));
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 451.0), "the 5th up shows the 5th Mode row");
    set(&mut h, t, m, json!({"fifth": 0.0, "style": 4.0}));
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 451.0), "Whammy has its Interval row");
    set(&mut h, t, m, json!({"style": 2.0}));
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 412.0), "OC-2 has none");
}

#[test]
fn pitch_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_pitch", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // The +1 Oct dial (POG's main row: Sub, -1 Oct, Dry, +1 Oct, +2 Oct, 5th): 0.008 per pixel.
    let at = main_knob(f, 3, 6);
    assert!((raw(&h, t, m, "octUp") - 0.6).abs() < 1e-6);
    drag(&mut h, at, at - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "octUp") - 0.8).abs() < 0.01, "25 px up is +0.2: {}", raw(&h, t, m, "octUp"));
    // Shift is fine: 0.002 per pixel.
    drag(&mut h, at, at + vec2(0.0, 50.0), Modifiers::SHIFT);
    assert!((raw(&h, t, m, "octUp") - 0.7).abs() < 0.01, "50 px down with Shift is -0.1: {}", raw(&h, t, m, "octUp"));
    // A drag is one undo step.
    undo(&mut h);
    assert!((raw(&h, t, m, "octUp") - 0.8).abs() < 0.01);
    // Double-click: back to the default.
    double_click(&mut h, at);
    assert!((raw(&h, t, m, "octUp") - 0.6).abs() < 1e-4);
    // The wheel scrolls the Mix window; the knob ignores it (spec 2026-11-02 item 3).
    wheel_notches(&mut h, at, 1.0);
    assert!((raw(&h, t, m, "octUp") - 0.6).abs() < 1e-4);

    // The style Segmented: OC-2.
    click(&mut h, style_seg(f, 2));
    assert_eq!(raw(&h, t, m, "style"), 2.0);
    assert!(h.query_by_label("Direct").is_some(), "OC-2 renames Dry");
    // The editor's own Undo (`doUndo`) takes the style back.
    let undo_at = button(&h, f, "Undo");
    click(&mut h, undo_at);
    assert_eq!(raw(&h, t, m, "style"), 0.0);
    // Bypass: the engine's own bypass.
    let f = frame(&h, t, m);
    let by = button(&h, f, "Bypass");
    click(&mut h, by);
    assert_eq!(raw(&h, t, m, "bypass"), 1.0);
    // A/B: B starts as a copy; a change on B; back to A recalls A.
    set(&mut h, t, m, json!({"dry": 0.5}));
    let b = button(&h, f, "B");
    click(&mut h, b);
    set(&mut h, t, m, json!({"dry": 0.1}));
    let a = button(&h, f, "A");
    click(&mut h, a);
    assert!((raw(&h, t, m, "dry") - 0.5).abs() < 1e-4, "A recalled");
    click(&mut h, b);
    assert!((raw(&h, t, m, "dry") - 0.1).abs() < 1e-4, "B recalled");
}

#[test]
fn pitch_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_pitch", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    let f = frame(&h, t, m);
    assert_eq!(f.width(), 900.0);
    assert!(f.height() == 412.0 || f.height() == 451.0, "{f:?}");
    for style in 0..5 {
        set(&mut h, t, m, json!({"style": style as f64}));
        assert!(frame(&h, t, m).height() >= 412.0);
    }
}

#[test]
fn pitch_editor_follows_undo() {
    let (mut h, t, m) = open("fs_pitch", vec2(1800.0, 1000.0), "dark");
    set(&mut h, t, m, json!({"style": 4.0}));
    assert_eq!(frame(&h, t, m).height(), 451.0);
    assert!(h.query_by_label("Whammy").is_some(), "Whammy names the Voice dial");
    undo(&mut h);
    assert_eq!(frame(&h, t, m).height(), 412.0, "the next frame follows undo");
    assert!(h.query_by_label("Whammy").is_none());
    assert!(h.query_by_label("+1 Oct").is_some());
}
