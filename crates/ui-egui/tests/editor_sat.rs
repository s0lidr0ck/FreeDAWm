//! Sat's VST editor in the Mix window: its size, gestures through commands, hostile parameters
//! and undo made elsewhere.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Shape, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::SoundApp;

fn text_shown(h: &Harness<SoundApp>, want: &str) -> bool {
    h.output().shapes.iter().any(|c| matches!(&c.shape, Shape::Text(tx) if tx.galley.job.text == want))
}

#[test]
fn sat_editor_is_its_vst_size() {
    let (h, t, m) = open("fs_sat", vec2(1800.0, 1000.0), "dark");
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 466.0), "setSize (860, kDefaultH)");
    // The same editor in the light theme.
    let (h, t, m) = open("fs_sat", vec2(1800.0, 1000.0), "light");
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 466.0));
    assert!(text_shown(&h, "Valve") && text_shown(&h, "Triode"), "the circuit and variant rows");
}

#[test]
fn sat_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_sat", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // The Drive knob: (113, 233) in the editor (resized(): the knob panel's left half).
    let drive = f.min + vec2(113.0, 233.0);
    let before = param(&h, t, m, "drive");
    drag(&mut h, drive, drive - vec2(0.0, 50.0), Modifiers::NONE);
    let after = param(&h, t, m, "drive");
    assert!((after - (before + 0.4)).abs() < 0.01, "50 px up is +0.4: {before} -> {after}");
    undo(&mut h);
    assert!((param(&h, t, m, "drive") - before).abs() < 1e-6, "the drag is one undo step");

    // The second Variant segment (the row at y 110..138, the segments from x 94 to 836).
    let seg_w = (836.0 - 94.0) / 3.0;
    click(&mut h, f.min + vec2(94.0 + seg_w * 1.5, 124.0));
    assert_eq!(raw(&h, t, m, "variant"), 1.0, "Pentode");
    assert!(text_shown(&h, "Clean until it isn't, then odd-order and hard. More bite."), "the variant's blurb follows");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "variant"), 0.0, "one undo step");

    // Auto Gain (the foot's first pill) toggles.
    click(&mut h, f.min + vec2(24.0 + 42.0, 425.0 + 13.0));
    assert_eq!(raw(&h, t, m, "autoGain"), 0.0);
}

#[test]
fn sat_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_sat", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(6);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(860.0, 466.0));
    // Gestures on hostile values still land on legal ones.
    let drive = f.min + vec2(113.0, 233.0);
    drag(&mut h, drive, drive - vec2(0.0, 20.0), Modifiers::NONE);
    let v = raw(&h, t, m, "drive");
    assert!(v.is_finite() && (0.0..=1.0).contains(&v));
}

#[test]
fn sat_editor_follows_undo() {
    let (mut h, t, m) = open("fs_sat", vec2(1800.0, 1000.0), "dark");
    let default_text = "+6.0 dB";
    assert!(text_shown(&h, default_text), "the default drive reads {default_text}");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "drive", "value": 0.9})).unwrap();
    h.run_steps(3);
    assert!(!text_shown(&h, default_text));
    undo(&mut h);
    assert!(text_shown(&h, default_text), "the Drive dial reads its default again");
    let _ = pos2(0.0, 0.0);
}
