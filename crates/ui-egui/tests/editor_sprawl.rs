//! Sprawl's editor in the Mix window: its size, the bands' gestures (widths and splits) and the
//! controls through commands, each one undo step, the presets, and hostile parameters.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_dsp::plugins::sprawl::SPLIT_DEFAULTS;
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::editors::sprawl::{SIZE, band_pos, bands_rect, split_pos};

const SCREEN: egui::Vec2 = vec2(1800.0, 1000.0);

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

#[test]
fn sprawl_opens_its_own_editor_at_its_size() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("sprawl", SCREEN, theme);
        let f = frame(&h, t, m);
        assert_eq!(f.size(), SIZE);
        for label in ["Undo", "A", "B", "Mono Safe", "Width", "Stereoize", "Low", "Low Mid", "High Mid", "High", "Trim"] {
            let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
            assert!(f.contains_rect(r), "{label} at {r:?} leaves the editor {f:?}");
        }
    }
}

#[test]
fn dragging_a_band_sets_its_width_in_one_undo_step() {
    let (mut h, t, m) = open("sprawl", SCREEN, "dark");
    let f = frame(&h, t, m);
    drag(&mut h, band_pos(f, &SPLIT_DEFAULTS, 3, 100.0), band_pos(f, &SPLIT_DEFAULTS, 3, 160.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "b4_width") - 160.0).abs() < 2.0, "{}", raw(&h, t, m, "b4_width"));
    // Down to the bottom and beyond: mono, and no further.
    let plot = bands_rect(f);
    drag(&mut h, band_pos(f, &SPLIT_DEFAULTS, 0, 100.0), band_pos(f, &SPLIT_DEFAULTS, 0, 0.0) + vec2(0.0, plot.height()), Modifiers::NONE);
    assert_eq!(raw(&h, t, m, "b1_width"), 0.0);
    // The other bands did not move.
    assert_eq!((raw(&h, t, m, "b2_width"), raw(&h, t, m, "b3_width")), (100.0, 100.0));
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "b1_width"), 100.0, "one undo step");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "b4_width"), 100.0);
}

#[test]
fn a_double_click_puts_a_band_back_as_it_came() {
    let (mut h, t, m) = open("sprawl", SCREEN, "dark");
    let f = frame(&h, t, m);
    drag(&mut h, band_pos(f, &SPLIT_DEFAULTS, 2, 100.0), band_pos(f, &SPLIT_DEFAULTS, 2, 40.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "b3_width") - 40.0).abs() < 2.0, "{}", raw(&h, t, m, "b3_width"));
    // (Half a second on, so the drag's release is not taken for the first of the two clicks.)
    h.run_steps(30);
    let grip = band_pos(f, &SPLIT_DEFAULTS, 2, raw(&h, t, m, "b3_width"));
    double_click(&mut h, grip);
    assert_eq!(raw(&h, t, m, "b3_width"), 100.0);
}

#[test]
fn a_split_moves_between_its_neighbours_and_no_further() {
    let (mut h, t, m) = open("sprawl", SCREEN, "dark");
    let f = frame(&h, t, m);
    drag(&mut h, split_pos(f, 1000.0), split_pos(f, 2500.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "split2") / 2500.0).log2().abs() < 0.04, "{}", raw(&h, t, m, "split2"));
    // The low split dragged far past the middle one stops a ratio of 1.5 short of it.
    let middle = raw(&h, t, m, "split2");
    drag(&mut h, split_pos(f, 120.0), split_pos(f, 15_000.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "split1") - middle / 1.5).abs() < 1.0, "{} against {middle}", raw(&h, t, m, "split1"));
    assert_eq!((raw(&h, t, m, "split2"), raw(&h, t, m, "split3")), (middle, 6000.0), "its neighbours stay put");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "split1"), 120.0, "one undo step");
    // A double-click puts a split back.
    h.run_steps(30);
    double_click(&mut h, split_pos(f, middle));
    assert_eq!(raw(&h, t, m, "split2"), 1000.0);
}

#[test]
fn the_controls_set_their_parameters() {
    let (mut h, t, m) = open("sprawl", SCREEN, "dark");
    let f = frame(&h, t, m);
    // Width: 25 px up is +0.2 of its travel.
    let before = param(&h, t, m, "width");
    let k = at(&h, f, "Width").expect("the Width knob").center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "width") - (before + 0.2)).abs() < 0.01, "{}", param(&h, t, m, "width"));
    let k = at(&h, f, "Stereoize").expect("the Stereoize knob").center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "stereoize") > 10.0);
    // A band's dial and its bar are the same parameter.
    let k = at(&h, f, "Low").expect("the Low knob").center();
    drag(&mut h, k, k + vec2(0.0, 200.0), Modifiers::NONE);
    assert_eq!(raw(&h, t, m, "b1_width"), 0.0);
    let safe = at(&h, f, "Mono Safe").expect("Mono Safe").center();
    click(&mut h, safe);
    assert_eq!(raw(&h, t, m, "safe"), 1.0);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "safe"), 0.0);
}

#[test]
fn hostile_parameters_draw_without_panicking() {
    let (mut h, t, m) = open("sprawl", SCREEN, "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), SIZE);
}

#[test]
fn the_preset_strip_steps_through_sprawls_own_presets() {
    use soundcraft_dsp::plugins::sprawl::presets::SPRAWL_PRESETS;
    let (mut h, t, m) = open("sprawl", SCREEN, "dark");
    let preset = |h: &Harness<SoundApp>| h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().preset.clone();
    assert_eq!(preset(&h), "<factory default>");
    // The name button sits between the two arrows: the next arrow is just right of it.
    let name = h.get_by_label_contains("Preset").rect();
    click(&mut h, name.right_center() + vec2(18.0, 0.0));
    let first = preset(&h);
    let found = SPRAWL_PRESETS.iter().find(|p| p.name == first).unwrap_or_else(|| panic!("stepped to {first:?}"));
    // A preset sets every parameter: the module now holds what the preset says.
    for (id, v) in found.set {
        assert_eq!(raw(&h, t, m, id), *v, "{first}: {id}");
    }
    click(&mut h, name.right_center() + vec2(18.0, 0.0));
    assert!(preset(&h) != first && SPRAWL_PRESETS.iter().any(|p| p.name == preset(&h)));
    // The browser opens from the drawer command.
    h.state_mut().run("ui.mix.drawer", serde_json::json!({"track": t.0, "module": m.0, "tab": "Presets"})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Preset card A Little Wider").is_some(), "the browser lists the factory presets");
}
