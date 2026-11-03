//! ChamberMade's VST editor in the Mix window: its size, gestures through commands, hostile parameters
//! and undo made elsewhere.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Shape, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::SoundApp;

fn text_shown(h: &Harness<SoundApp>, want: &str) -> bool {
    h.output().shapes.iter().any(|c| matches!(&c.shape, Shape::Text(tx) if tx.galley.job.text == want))
}

use soundcraft_dsp::plugins::fs::chamber::{CHAMBER_PARAMS, build_chamber_for};

/// The speaker on the plan (`layout()` for the default room in the plan at (14, 66), 874 x 575).
fn speaker_at(f: egui::Rect) -> egui::Pos2 {
    let ch = build_chamber_for(&CHAMBER_PARAMS.iter().map(|p| p.default).collect::<Vec<_>>());
    let (w, h) = (874.0f32, 575.0f32);
    let (pad, pad_b) = (36.0, 66.0);
    let avail_h = h - pad - pad_b;
    let (wm, lm) = (ch.room.width_m.max(0.1), ch.room.length_m.max(0.1));
    let s0 = ((w - 2.0 * pad) / wm).min(avail_h / lm);
    let s1 = ((w - 2.0 * pad) / lm).min(avail_h / wm);
    let rot = s1 > s0 * 1.02;
    let sc = if rot { s1 } else { s0 };
    let ox = (w - if rot { lm } else { wm } * sc) * 0.5;
    let mid_y = pad + avail_h * 0.5;
    let oy = if rot { mid_y - wm * sc * 0.5 } else { mid_y + lm * sc * 0.5 };
    let s = ch.placement.speaker;
    let local = if rot { pos2(ox + s.y * sc, oy + s.x * sc) } else { pos2(ox + s.x * sc, oy - s.y * sc) };
    f.min + vec2(14.0, 66.0) + local.to_vec2()
}

#[test]
fn chamber_editor_is_its_vst_size() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("fs_chamber", vec2(2200.0, 1000.0), theme);
        assert_eq!(frame(&h, t, m).size(), vec2(1190.0, 648.0), "kDefaultWidth x kDefaultHeight");
        assert!(text_shown(&h, "Room") && text_shown(&h, "Painted Concrete"), "the Room inspector");
    }
}

#[test]
fn chamber_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_chamber", vec2(2200.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // Decay: a large ChamberKnob (75 px at (1067.5, 512)): 0.006 per pixel.
    let decay = f.min + vec2(1105.0, 549.0);
    let before = param(&h, t, m, "decay");
    drag(&mut h, decay, decay - vec2(0.0, 50.0), Modifiers::NONE);
    let after = param(&h, t, m, "decay");
    assert!((after - (before + 0.3)).abs() < 0.01, "50 px up is +0.3: {before} -> {after}");
    undo(&mut h);
    assert!((param(&h, t, m, "decay") - before).abs() < 1e-6, "one undo step");

    // A plan drag: the speaker, 40 px along, is one undo step through mix.module_params.
    let spk = speaker_at(f);
    let (x0, y0) = (raw(&h, t, m, "spkX"), raw(&h, t, m, "spkY"));
    drag(&mut h, spk, spk + vec2(40.0, 30.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "spkX") - x0).abs() > 0.01 || (raw(&h, t, m, "spkY") - y0).abs() > 0.01, "the speaker moved");
    assert!(text_shown(&h, "Speaker"), "pressing it selects it");
    undo(&mut h);
    assert!((raw(&h, t, m, "spkX") - x0).abs() < 1e-6 && (raw(&h, t, m, "spkY") - y0).abs() < 1e-6, "the drag is one undo step");

    // Add a Pot: the first free slot, selected; Cmd+Z (the editor's undo) takes it away.
    let pot = h.get_by_label("Add Pot").rect().center();
    click(&mut h, pot);
    assert_eq!(raw(&h, t, m, "o1t"), 2.0);
    assert!(text_shown(&h, "Ceramic Pot"));
    hover(&mut h, f.center());
    key(&mut h, egui::Key::Z, Modifiers::COMMAND);
    assert_eq!(raw(&h, t, m, "o1t"), 0.0, "the editor's own undo");
}

#[test]
fn chamber_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_chamber", vec2(2200.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(6);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(1190.0, 648.0));
    hover(&mut h, f.center());
    click(&mut h, f.center());
    assert!(frame(&h, t, m).is_positive());
}

#[test]
fn chamber_editor_follows_undo() {
    let (mut h, t, m) = open("fs_chamber", vec2(2200.0, 1000.0), "dark");
    assert!(text_shown(&h, "35 %"), "Mix at its default");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "mix", "value": 0.8})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "80 %") && !text_shown(&h, "35 %"));
    undo(&mut h);
    assert!(text_shown(&h, "35 %"), "the Mix knob reads its default again");
}
