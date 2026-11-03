//! Transient's VST editor in the Mix window: its size, gestures through commands, hostile parameters
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
fn transient_editor_is_its_vst_size() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("fs_transient", vec2(1800.0, 1000.0), theme);
        assert_eq!(frame(&h, t, m).size(), vec2(860.0, 441.0), "setSize (860, kDefaultH)");
        assert!(text_shown(&h, "Drums") && text_shown(&h, "Signal"));
    }
}

#[test]
fn transient_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_transient", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // The Attack knob: its 104 px knob at (72, 126) in the editor.
    let attack = f.min + vec2(72.0 + 52.0, 126.0 + 52.0);
    let before = param(&h, t, m, "attack");
    drag(&mut h, attack, attack - vec2(0.0, 25.0), Modifiers::NONE);
    let after = param(&h, t, m, "attack");
    assert!((after - (before + 0.2)).abs() < 0.01, "25 px up is +0.2: {before} -> {after}");
    undo(&mut h);
    assert!((param(&h, t, m, "attack") - before).abs() < 1e-6, "one undo step");

    // Source: Guitar, the third of seven segments from x 98 to 836 (y 55..83).
    let seg_w = (836.0 - 98.0) / 7.0;
    click(&mut h, f.min + vec2(98.0 + seg_w * 2.5, 69.0));
    assert_eq!(raw(&h, t, m, "source"), 2.0);
    assert!(text_shown(&h, "Listens to the pick, 1.2-8 kHz. Put Attack on High for more or less pick, same body."));
    // The Attack Focus: High, the fourth of four from x 42 to 206 (y 266..290).
    click(&mut h, f.min + vec2(42.0 + 41.0 * 3.5, 278.0));
    assert_eq!(raw(&h, t, m, "attackFocus"), 3.0);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "attackFocus"), 0.0);
}

#[test]
fn transient_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_transient", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(6);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(860.0, 441.0));
    // Attack is +1e30 (clamped to the top): a drag down lands on a legal value.
    let attack = f.min + vec2(124.0, 178.0);
    drag(&mut h, attack, attack + vec2(0.0, 20.0), Modifiers::NONE);
    let v = raw(&h, t, m, "attack");
    assert!(v.is_finite() && (0.0..=1.0).contains(&v));
}

#[test]
fn transient_editor_follows_undo() {
    let (mut h, t, m) = open("fs_transient", vec2(1800.0, 1000.0), "dark");
    assert!(text_shown(&h, "0.0 dB"), "Output at its default");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "output", "value": 0.75})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "+6.0 dB") && !text_shown(&h, "0.0 dB"));
    undo(&mut h);
    assert!(text_shown(&h, "0.0 dB"), "the Output dial reads its default again");
    let _ = pos2(0.0, 0.0);
}
