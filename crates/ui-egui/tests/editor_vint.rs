//! Vint's VST editor in the Mix window: its size, gestures through commands, hostile parameters
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

/// The Passive's low Boost knob (a 119 px Fluted knob at (185, 155), centre 67.5 px down).
fn low_boost(f: egui::Rect) -> egui::Pos2 {
    f.min + vec2(244.5, 222.5)
}

#[test]
fn vint_editor_is_its_vst_size() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("fs_vint", vec2(1800.0, 1000.0), theme);
        assert_eq!(frame(&h, t, m).size(), vec2(1000.0, 643.0), "kDefaultW x kDefaultH");
        assert!(text_shown(&h, "EQP-1A") && text_shown(&h, "PROGRAM EQUALIZER"), "the Passive's face");
    }
}

#[test]
fn vint_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_vint", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // A VintKnob drag: Dial's 0.008 per pixel.
    let k = low_boost(f);
    drag(&mut h, k, k - vec2(0.0, 50.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "pasLowBoost") - 0.4).abs() < 0.01, "50 px up is +0.4");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "pasLowBoost"), 0.0, "one undo step");
    // A switch snaps: Low Frequency (60 of 20/30/60/100) dragged 30 px up lands on 100.
    let sw = f.min + vec2(310.0, 344.0);
    drag(&mut h, sw, sw - vec2(0.0, 30.0), Modifiers::NONE);
    assert_eq!(raw(&h, t, m, "pasLowFreq"), 3.0);
    // The circuit selector, in its own order: the third button is Console.
    let seg_w = (976.0 - 98.0) / 8.0;
    click(&mut h, f.min + vec2(98.0 + seg_w * 2.5, 69.0));
    assert_eq!(raw(&h, t, m, "circuit"), 1.0);
    assert!(text_shown(&h, "MIC."), "the 1073's face");
    // Its EQL plate button bypasses the unit.
    let eql = f.min + vec2((24.0 + 958.0 * 0.952f32).round(), (127.2 + 138.0 * 0.952f32).round());
    click(&mut h, eql);
    assert_eq!(raw(&h, t, m, "bypass"), 1.0);
}

#[test]
fn vint_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_vint", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(6);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(1000.0, 643.0));
    hover(&mut h, f.center());
    for c in 0..8 {
        h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "circuit", "value": c})).unwrap();
        h.run_steps(2);
        assert!(frame(&h, t, m).is_positive());
    }
}

#[test]
fn vint_editor_follows_undo() {
    let (mut h, t, m) = open("fs_vint", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "pasLowBoost", "value": 0.9})).unwrap();
    hover(&mut h, low_boost(f));
    h.run_steps(2);
    assert!(text_shown(&h, "9.0"), "the hover readout follows the command");
    undo(&mut h);
    hover(&mut h, low_boost(f));
    assert!(text_shown(&h, "0.0") && !text_shown(&h, "9.0"), "and the undo");
    let _ = pos2(0.0, 0.0);
}

/// A point of the 1000 x 300 panel, on the editor's frame (the panel is 952 px wide at (24, 127.2)).
fn on_panel(f: egui::Rect, x: f32, y: f32) -> egui::Pos2 {
    f.min + vec2(24.0 + x * 0.952, 127.2 + y * 0.952)
}

#[test]
fn the_maximizer_wears_the_882s_front() {
    let (mut h, t, m) = open("fs_vint", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"circuit": 4, "enhType": 0}})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "PROCESS") && !text_shown(&h, "SONIC MAXIMIZER"), "the Exciter keeps its columns");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "enhType", "value": 1})).unwrap();
    h.run_steps(3);
    for word in ["882", "SONIC MAXIMIZER", "LO CONTOUR", "PROCESS", "FUNCTION", "TUNE", "CLIP"] {
        assert!(text_shown(&h, word), "{word}");
    }
    let f = frame(&h, t, m);
    // The PROCESS pot: a knob drag, 0.008 per pixel, one undo step.
    let k = on_panel(f, 566.0, 166.0);
    drag(&mut h, k, k - vec2(0.0, 50.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "enhAmount") - 0.4).abs() < 0.01);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "enhAmount"), 0.0);
    // LO CONTOUR and TUNE are the other two.
    let k = on_panel(f, 432.0, 166.0);
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "enhLow") - 0.2).abs() < 0.01);
    let k = on_panel(f, 858.0, 166.0);
    drag(&mut h, k, k + vec2(0.0, 25.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "enhTune") - 0.3).abs() < 0.01);
    // FUNCTION takes the unit out and back in.
    let b = on_panel(f, 712.0, 178.0);
    click(&mut h, b);
    assert_eq!(raw(&h, t, m, "bypass"), 1.0);
    click(&mut h, b);
    assert_eq!(raw(&h, t, m, "bypass"), 0.0);
    // The Type selector above the unit goes back to the other two: its third segment is the Vitalizer.
    click(&mut h, f.min + vec2(24.0 + 62.0 + 330.0 * 5.0 / 6.0, 127.2 - 13.6 + 14.0 + 13.0));
    assert_eq!(raw(&h, t, m, "enhType"), 2.0);
    assert!(!text_shown(&h, "SONIC MAXIMIZER"));
}

#[test]
fn the_vitalizer_shows_all_its_controls() {
    let (mut h, t, m) = open("fs_vint", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"circuit": 4, "enhType": 2}})).unwrap();
    h.run_steps(3);
    for word in ["Drive", "Mid-Hi Tune", "Process", "Bass Sound", "Bass Comp", "LC-EQ", "Intensity", "Stereo Exp."] {
        assert!(text_shown(&h, word), "{word}");
    }
    // Its readouts are its own: Mid-Hi Tune at its centre, Bass Sound off, the LC stage's frequency.
    for text in ["4.92 kHz", "Off", "6.32 kHz"] {
        assert!(text_shown(&h, text), "{text}");
    }
    // The Exciter keeps the Enhancer's three dials.
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "enhType", "value": 0})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "Amount") && !text_shown(&h, "Bass Comp"));
}
