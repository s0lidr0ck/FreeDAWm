//! DistEngine's VST editor in the Mix window: its height per circuit, its gestures as commands,
//! A/B with the knob bank, the calibration wizard, hostile state and following undo.

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

/// `requiredHeight()`: 306 + n x 52 + (n - 1) x 6.
fn height(rows: f32) -> f32 {
    306.0 + rows * 52.0 + (rows - 1.0) * 6.0
}

/// The dial in cell `col` of four (pad 24 / 15, header 26, gap 16, circuit 62, gap 16; 170 px
/// cells beside the 132 px Link column; a 110 px knob).
fn knob(f: Rect, col: usize) -> Pos2 {
    pos2(f.left() + 24.0 + 170.0 * col as f32 + 85.0, f.top() + 135.0 + 55.0)
}

/// Circuit segment `i` (two rows of six from x + 24 + 62, y + 57, 774 x 62).
fn circuit_seg(f: Rect, i: usize) -> Pos2 {
    let w = (f.width() - 48.0 - 62.0) / 6.0;
    pos2(f.left() + 86.0 + w * ((i % 6) as f32 + 0.5), f.top() + 57.0 + 15.5 + 31.0 * (i / 6) as f32)
}

fn button(h: &Harness<SoundApp>, f: Rect, label: &str) -> Pos2 {
    h.get_all_by_label(label).map(|n| n.rect()).find(|r| f.contains_rect(*r)).expect("the button is in the editor").center()
}

#[test]
fn dist_editor_is_its_vst_size() {
    let (mut h, t, m) = open("fs_dist", vec2(1800.0, 1000.0), "dark");
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, height(2.0)), "Tube Screamer: Model and Diode");
    set(&mut h, t, m, json!({"circuit": 1.0}));
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, height(4.0)), "RAT: four rows");
    set(&mut h, t, m, json!({"circuit": 5.0}));
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, height(1.0)), "Klon: one row");
}

#[test]
fn dist_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_dist", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let drive = knob(f, 1);
    drag(&mut h, drive, drive - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "drive") - 0.7).abs() < 0.01, "{}", raw(&h, t, m, "drive"));
    undo(&mut h);
    assert!((raw(&h, t, m, "drive") - 0.5).abs() < 1e-4, "a drag is one undo step");
    // A circuit pick on the two-row Segmented: RAT, and the height follows on the next frame.
    click(&mut h, circuit_seg(f, 1));
    assert_eq!(raw(&h, t, m, "circuit"), 1.0);
    assert_eq!(frame(&h, t, m).height(), height(4.0));
    assert!(h.query_by_label("Distortion").is_some(), "the RAT names its Drive");
    // The second row: Guv'nor (7).
    click(&mut h, circuit_seg(f, 7));
    assert_eq!(raw(&h, t, m, "circuit"), 7.0);
    // The editor's Undo takes the circuit back to the RAT with the RAT's knobs.
    let u = button(&h, f, "Undo");
    click(&mut h, u);
    assert_eq!(raw(&h, t, m, "circuit"), 1.0);
    // Link does not push undo (Dist's attachPill), and locks Level.
    let link = button(&h, f, "Link");
    click(&mut h, link);
    assert_eq!(raw(&h, t, m, "link"), 1.0);
    let lvl = knob(f, 3);
    let before = raw(&h, t, m, "level");
    drag(&mut h, lvl, lvl - vec2(0.0, 30.0), Modifiers::NONE);
    assert_eq!(raw(&h, t, m, "level"), before, "a Linked Level takes no drag");
}

#[test]
fn dist_ab_takes_the_whole_rig() {
    let (mut h, t, m) = open("fs_dist", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    set(&mut h, t, m, json!({"drive": 0.3, "level": 0.4}));
    let b = button(&h, f, "B");
    click(&mut h, b);
    set(&mut h, t, m, json!({"circuit": 2.0}));
    set(&mut h, t, m, json!({"drive": 0.9, "level": 0.8}));
    let a = button(&h, f, "A");
    click(&mut h, a);
    assert_eq!((raw(&h, t, m, "circuit"), raw(&h, t, m, "drive"), raw(&h, t, m, "level")), (0.0, 0.3, 0.4), "A: circuit, drive and Level");
    click(&mut h, b);
    assert_eq!((raw(&h, t, m, "circuit"), raw(&h, t, m, "drive"), raw(&h, t, m, "level")), (2.0, 0.9, 0.8), "B, without a knob swap");
    // The bank came back with B: the Tube Screamer's slot holds A's knobs.
    set(&mut h, t, m, json!({"circuit": 0.0}));
    assert_eq!(raw(&h, t, m, "drive"), 0.3);
}

#[test]
fn dist_calibration_writes_this_circuits_set_points() {
    let (mut h, t, m) = open("fs_dist", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    set(&mut h, t, m, json!({"circuit": 1.0}));
    let cal = button(&h, f, "Calibrate");
    click(&mut h, cal);
    let next = button(&h, f, "Next");
    click(&mut h, next);
    click(&mut h, next);
    assert!((raw(&h, t, m, "drive") - 0.1).abs() < 1e-6, "step 3: the RAT's low anchor");
    set(&mut h, t, m, json!({"level": 0.55}));
    click(&mut h, next);
    assert!((raw(&h, t, m, "ratLo") - 0.55).abs() < 1e-4);
    set(&mut h, t, m, json!({"level": 0.35}));
    let done = button(&h, f, "Done");
    click(&mut h, done);
    assert!((raw(&h, t, m, "ratHi") - 0.35).abs() < 1e-4);
    assert_eq!(raw(&h, t, m, "link"), 1.0);
    // The wizard's own moves pushed nothing onto the editor's Undo.
    let u = button(&h, f, "Undo");
    click(&mut h, u);
    assert_eq!(raw(&h, t, m, "circuit"), 1.0, "Undo is still empty (dimmed): nothing restored");
}

#[test]
fn dist_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_dist", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).width(), 860.0);
    for c in 0..12 {
        set(&mut h, t, m, json!({"circuit": c as f64}));
        assert!(frame(&h, t, m).height() >= height(1.0));
    }
}

#[test]
fn dist_editor_follows_undo() {
    let (mut h, t, m) = open("fs_dist", vec2(1800.0, 1000.0), "dark");
    set(&mut h, t, m, json!({"circuit": 1.0}));
    assert_eq!(frame(&h, t, m).height(), height(4.0));
    undo(&mut h);
    assert_eq!(frame(&h, t, m).height(), height(2.0), "the next frame follows undo");
    assert!(h.query_by_label("Distortion").is_none());
}
