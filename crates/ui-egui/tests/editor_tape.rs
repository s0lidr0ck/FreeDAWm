//! Tape's VST editor in the Mix window: its size, gestures through commands, hostile parameters
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

fn count_shown(h: &Harness<SoundApp>, want: &str) -> usize {
    h.output().shapes.iter().filter(|c| matches!(&c.shape, Shape::Text(tx) if tx.galley.job.text == want)).count()
}

#[test]
fn tape_editor_is_its_vst_size() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("fs_tape", vec2(1800.0, 1000.0), theme);
        assert_eq!(frame(&h, t, m).size(), vec2(860.0, 466.0), "setSize (860, kDefaultH)");
        assert!(text_shown(&h, "Multitrack") && text_shown(&h, "Standard") && text_shown(&h, "15 ips"));
    }
}

#[test]
fn tape_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_tape", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // The Drive knob: (113, 233) in the editor, as Sat's.
    let drive = f.min + vec2(113.0, 233.0);
    let before = param(&h, t, m, "drive");
    // In the Mix window the wheel scrolls; the knob ignores it (spec 2026-11-02 item 3).
    wheel_notches(&mut h, drive, 1.0);
    assert!((param(&h, t, m, "drive") - before).abs() < 1e-6, "the wheel leaves the knob alone");
    drag(&mut h, drive, drive - egui::vec2(0.0, 30.0), egui::Modifiers::NONE);
    let after = param(&h, t, m, "drive");
    assert!(after > before + 1e-3, "a drag up turns it: {before} -> {after}");
    double_click(&mut h, drive);
    assert!((param(&h, t, m, "drive") - before).abs() < 1e-6, "double-click returns to the default");
    undo(&mut h);
    assert!((param(&h, t, m, "drive") - after).abs() < 1e-4, "the double-click is one undo step");
    undo(&mut h);
    assert!((param(&h, t, m, "drive") - before).abs() < 1e-6, "the drag is one undo step");

    // Machine: Cassette (the fourth of four segments from x 98 to 836) disables Speed.
    let seg_w = (836.0 - 98.0) / 4.0;
    click(&mut h, f.min + vec2(98.0 + seg_w * 3.5, 69.0));
    assert_eq!(raw(&h, t, m, "machine"), 3.0);
    assert!(text_shown(&h, "Type I"), "the cassette's tapes");
    // Speed (x 621..836, y 110..138) does nothing while it is disabled.
    let speed_w = (836.0 - 621.0) / 3.0;
    click(&mut h, f.min + vec2(621.0 + speed_w * 2.5, 124.0));
    assert_eq!(raw(&h, t, m, "speed"), 1.0);
}

#[test]
fn tape_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_tape", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(6);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(860.0, 466.0));
    let drive = f.min + vec2(113.0, 233.0);
    drag(&mut h, drive, drive - vec2(0.0, 20.0), Modifiers::NONE);
    let v = raw(&h, t, m, "drive");
    assert!(v.is_finite() && (0.0..=1.0).contains(&v));
}

#[test]
fn tape_editor_follows_undo() {
    let (mut h, t, m) = open("fs_tape", vec2(1800.0, 1000.0), "dark");
    assert_eq!(count_shown(&h, "Off"), 2, "Wow & Flutter and Hiss read Off");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "flutter", "value": 0.5})).unwrap();
    h.run_steps(3);
    assert_eq!(count_shown(&h, "Off"), 1);
    undo(&mut h);
    assert_eq!(count_shown(&h, "Off"), 2, "Wow & Flutter reads Off again");
    let _ = pos2(0.0, 0.0);
}
