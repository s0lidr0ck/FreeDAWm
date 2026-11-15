//! Lull's editor in the Mix window: its size, the threshold dragged on the scope and on the
//! curve, the controls through commands (each one undo step), its presets, and hostile parameters.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, pos2, vec2};
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::editors::lull::{SIZE, curve_level_x, curve_rect, scope_level_y, scope_rect};

const SCREEN: egui::Vec2 = vec2(1800.0, 1100.0);

#[test]
fn lull_opens_its_own_editor_at_its_size() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("lull", SCREEN, theme);
        assert_eq!(frame(&h, t, m).size(), SIZE);
    }
}

#[test]
fn the_threshold_is_dragged_on_the_scope_and_on_the_curve() {
    let (mut h, t, m) = open("lull", SCREEN, "dark");
    let f = frame(&h, t, m);
    // On the scope: the line at -40 dB, grabbed in the middle and taken up to -20 dB.
    let scope = scope_rect(f);
    let x = scope.center().x;
    drag(&mut h, pos2(x, scope_level_y(scope, -40.0)), pos2(x, scope_level_y(scope, -20.0)), Modifiers::NONE);
    assert!((raw(&h, t, m, "threshold") + 20.0).abs() < 1.0, "{}", raw(&h, t, m, "threshold"));
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "threshold"), -40.0, "one undo step");
    // On the curve: the handle at -40 dB, grabbed near the bottom and taken left to -60 dB.
    let curve = curve_rect(f);
    let y = curve.bottom() - 20.0;
    drag(&mut h, pos2(curve_level_x(curve, -40.0), y), pos2(curve_level_x(curve, -60.0), y), Modifiers::NONE);
    assert!((raw(&h, t, m, "threshold") + 60.0).abs() < 1.5, "{}", raw(&h, t, m, "threshold"));
}

#[test]
fn the_controls_set_their_parameters() {
    let (mut h, t, m) = open("lull", SCREEN, "dark");
    let f = frame(&h, t, m);
    let within = |h: &egui_kittest::Harness<soundcraft_ui_egui::SoundApp>, label: &str| {
        h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r)).unwrap_or_else(|| panic!("{label} in the editor"))
    };
    // Range: a small dial; 25 px up is +0.2 of its travel (16 of its 80 dB).
    let range = within(&h, "Range").center();
    drag(&mut h, range, range - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "range") - 36.0).abs() < 1.0, "{}", raw(&h, t, m, "range"));
    // Gate: the right half of the mode switch.
    let mode = within(&h, "Expand|Gate");
    click(&mut h, mode.center() + vec2(mode.width() * 0.25, 0.0));
    assert_eq!(raw(&h, t, m, "mode"), 1.0);
    // Trigger: the right half of the key switch.
    let key = within(&h, "Guard|Trigger");
    click(&mut h, key.center() + vec2(key.width() * 0.25, 0.0));
    assert_eq!(raw(&h, t, m, "key"), 1.0);
    // Listen, in the header.
    let listen = within(&h, "Listen").center();
    click(&mut h, listen);
    assert_eq!(raw(&h, t, m, "listen"), 1.0);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "listen"), 0.0);
}

#[test]
fn the_preset_strip_steps_through_lulls_own_presets() {
    let (mut h, t, m) = open("lull", SCREEN, "dark");
    let preset = |h: &egui_kittest::Harness<soundcraft_ui_egui::SoundApp>| {
        h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().preset.clone()
    };
    let name = h.get_by_label_contains("Preset").rect();
    click(&mut h, name.right_center() + vec2(18.0, 0.0));
    let first = preset(&h);
    let presets = &soundcraft_dsp::plugins::lull::presets::LULL_PRESETS;
    let found = presets.iter().find(|p| p.name == first).unwrap_or_else(|| panic!("stepped to {first:?}"));
    // A preset sets every parameter: the module now holds that preset's Range.
    let range = found.set.iter().find(|(id, _)| *id == "range").map_or(20.0, |(_, v)| *v);
    assert_eq!(raw(&h, t, m, "range"), range);
    h.state_mut().run("ui.mix.drawer", serde_json::json!({"track": t.0, "module": m.0, "tab": "Presets"})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Preset card Plain Gate").is_some(), "the browser lists the factory presets");
}

#[test]
fn hostile_parameters_draw_without_panicking() {
    let (mut h, t, m) = open("lull", SCREEN, "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), SIZE);
}
