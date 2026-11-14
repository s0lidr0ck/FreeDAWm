//! Sprawl's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its header,
//! the bands' gestures, the main dials and its More section.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_dsp::plugins::sprawl::SPLIT_DEFAULTS;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::editors::sprawl::slim_band_pos;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "sprawl");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

#[test]
fn it_is_the_drawers_width_and_everything_fits() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 700.0, "about a strip tall with More closed: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Mono Safe", "Width", "Stereoize", "Trim", "More"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(at(&h, f, "Low Mid").is_none(), "the band dials are in More");
}

#[test]
fn the_bands_and_the_dials_work_as_in_the_full_editor() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    drag(&mut h, slim_band_pos(f, &SPLIT_DEFAULTS, 3, 100.0), slim_band_pos(f, &SPLIT_DEFAULTS, 3, 150.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "b4_width") - 150.0).abs() < 4.0, "{}", raw(&h, t, m, "b4_width"));
    assert_eq!(steps(&h), n + 1, "a band drag is one undo step");

    let before = param(&h, t, m, "width");
    let k = at(&h, f, "Width").expect("the Width knob").center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "width") - (before + 0.2)).abs() < 0.01, "25 px up is +0.2, as the full editor's");

    let safe = at(&h, f, "Mono Safe").expect("Mono Safe").center();
    click(&mut h, safe);
    assert_eq!(raw(&h, t, m, "safe"), 1.0);
}

#[test]
fn more_holds_the_band_dials() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let closed = f.height();
    let more = at(&h, f, "More").expect("More").center();
    click(&mut h, more);
    h.run_steps(4);
    let f = frame(&h, t, m);
    assert!(f.height() > closed + 100.0, "the editor grew: {closed} -> {}", f.height());
    assert!(f.height() < 900.0, "still about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Low", "Low Mid", "High Mid", "High"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in More"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn the_drawer_command_opens_more_and_the_setting_switches_layouts() {
    let (mut h, t, m) = slim();
    let closed = frame(&h, t, m).height();
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(4);
    assert!(frame(&h, t, m).height() > closed + 100.0);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::sprawl::SIZE);
}

#[test]
fn hostile_parameters_draw_without_panicking() {
    let (mut h, t, m) = slim();
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert!((frame(&h, t, m).width() - W).abs() <= 1.0);
}
