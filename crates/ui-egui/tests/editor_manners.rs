//! Manners' wide editor in the Mix window: its size, gestures through commands (dials, the range
//! edges, Delta, Bypass), hostile parameters and undo made elsewhere.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Shape, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::SoundApp;

fn text_shown(h: &Harness<SoundApp>, want: &str) -> bool {
    h.output().shapes.iter().any(|c| matches!(&c.shape, Shape::Text(tx) if tx.galley.job.text == want))
}

/// The Tame knob's centre in the editor: the first of six 132 px cells from x 34, its 104 px
/// knob from y 329.
const TAME: egui::Vec2 = vec2(34.0 + 66.0, 329.0 + 52.0);
/// The plot: x 36..824 (20 Hz..20 kHz), y 87..279.
const PLOT_MID_Y: f32 = 183.0;

#[test]
fn manners_editor_has_its_size_in_both_themes() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("fs_manners", vec2(1800.0, 1000.0), theme);
        assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::manners::SIZE);
        assert_eq!(frame(&h, t, m).size(), vec2(860.0, 518.0));
        for want in ["Manners", "Tame", "Recover", "Bias", "Brighten", "Boost", "Delta", "25 %", "Even", "Input", "Cut"] {
            assert!(text_shown(&h, want), "{want}");
        }
    }
    assert!(soundcraft_ui_egui::editors::has_wide("fs_manners"));
}

#[test]
fn manners_dials_go_through_commands_one_undo_step_each() {
    let (mut h, t, m) = open("fs_manners", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let before = param(&h, t, m, "tame");
    drag(&mut h, f.min + TAME, f.min + TAME - vec2(0.0, 25.0), Modifiers::NONE);
    let after = param(&h, t, m, "tame");
    assert!((after - (before + 0.2)).abs() < 0.01, "25 px up is +0.2: {before} -> {after}");
    assert!(text_shown(&h, "65 %"), "25 % and a fifth of the 200 % travel");
    undo(&mut h);
    assert!((param(&h, t, m, "tame") - before).abs() < 1e-6, "one undo step");

    // Bias, the third cell: down from the middle leans to Tame.
    let bias = f.min + vec2(34.0 + 132.0 * 2.5, 329.0 + 21.0 + 31.0);
    drag(&mut h, bias, bias + vec2(0.0, 25.0), Modifiers::NONE);
    let v = raw(&h, t, m, "bias");
    assert!(v < -10.0, "{v}");
    assert!(text_shown(&h, &format!("{} % Tame", (-v).round() as i32)));
}

#[test]
fn manners_range_edges_drag_and_keep_their_order() {
    let (mut h, t, m) = open("fs_manners", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // 20 Hz is the plot's left edge; 200 Hz a third of the way across (three decades).
    let x_200 = 36.0 + 788.0 / 3.0;
    drag(&mut h, f.min + vec2(36.0, PLOT_MID_Y), f.min + vec2(x_200, PLOT_MID_Y), Modifiers::NONE);
    let lo = raw(&h, t, m, "rangeLo");
    assert!((lo - 200.0).abs() < 4.0, "{lo}");
    assert_eq!(raw(&h, t, m, "rangeHi"), 20_000.0);
    assert!(text_shown(&h, &format!("Tame \u{2212}0.0 dB   Recover +0.0 dB   {} Hz \u{2013} 20.0 kHz", lo.round() as i32)));

    // The high edge dragged past the low one stops half an octave above it.
    drag(&mut h, f.min + vec2(824.0, PLOT_MID_Y), f.min + vec2(60.0, PLOT_MID_Y), Modifiers::NONE);
    let hi = raw(&h, t, m, "rangeHi");
    assert!((hi / lo - 2f32.sqrt()).abs() < 0.02, "{lo} .. {hi}");

    // Each drag is one undo step.
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "rangeHi"), 20_000.0);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "rangeLo"), 20.0);

    // A double-click on an edge opens it again.
    drag(&mut h, f.min + vec2(36.0, PLOT_MID_Y), f.min + vec2(x_200, PLOT_MID_Y), Modifiers::NONE);
    double_click(&mut h, f.min + vec2(x_200, PLOT_MID_Y));
    assert_eq!(raw(&h, t, m, "rangeLo"), 20.0);
}

#[test]
fn manners_delta_and_bypass_are_switches() {
    let (mut h, t, m) = open("fs_manners", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    assert!(text_shown(&h, "the whole signal"));
    // Delta: the middle of the sixth cell.
    click(&mut h, f.min + vec2(34.0 + 132.0 * 5.0 + 66.0, 329.0 + 67.0));
    assert_eq!(raw(&h, t, m, "delta"), 1.0);
    assert!(text_shown(&h, "only what it changes"));
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "delta"), 0.0);
    // Bypass: the header's last pill (the engine's own, a frame late so the latency holds).
    click(&mut h, f.min + vec2(860.0 - 24.0 - 20.0, 15.0 + 13.0));
    assert_eq!(raw(&h, t, m, "bypass"), 1.0);
}

#[test]
fn manners_editor_survives_hostile_params_and_follows_undo() {
    let (mut h, t, m) = open("fs_manners", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(6);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(860.0, 518.0));
    drag(&mut h, f.min + TAME, f.min + TAME + vec2(0.0, 20.0), Modifiers::NONE);
    let v = raw(&h, t, m, "tame");
    assert!(v.is_finite() && (0.0..=200.0).contains(&v), "{v}");

    let (mut h, t, m) = open("fs_manners", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "recover", "value": 80.0})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "80 %"));
    undo(&mut h);
    assert!(!text_shown(&h, "80 %"));
}

#[test]
fn manners_presets_step_from_the_header() {
    let (mut h, t, m) = open("fs_manners", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let info = soundcraft_dsp::plugin_info("fs_manners").unwrap();
    let groups = soundcraft_ui_egui::fs_presets::factory_groups(info);
    assert_eq!(groups.iter().map(|g| g.1.len()).sum::<usize>(), 7);
    let preset = |h: &Harness<SoundApp>| h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().preset.clone();
    // The arrows either side of the preset name, in the header's row.
    click(&mut h, f.min + vec2(454.0, 15.0 + 13.0));
    let first = preset(&h);
    assert_eq!(first, groups.first().unwrap().1.first().unwrap().0, "the right arrow steps forward from Init");
    click(&mut h, f.min + vec2(189.0, 15.0 + 13.0));
    assert_eq!(preset(&h), groups.last().unwrap().1.last().unwrap().0, "the left arrow steps back from the first, round to the last");
    // A preset sets what it lists and never the engine's bypass.
    assert_eq!(raw(&h, t, m, "bypass"), 0.0);
}
