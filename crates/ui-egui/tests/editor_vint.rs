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
