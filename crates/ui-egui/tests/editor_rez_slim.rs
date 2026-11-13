//! Rez's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its header, the
//! graph's gestures, the main dials and its More section.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::editors::rez::slim_band_pos;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "rez");
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
    assert!(f.height() < 600.0, "about a strip tall with More closed: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Delta", "Depth", "Sharpness", "Selectivity", "Soft|Hard", "More"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(at(&h, f, "Attack").is_none(), "Attack is in More");
}

#[test]
fn the_graph_and_the_dials_work_as_in_the_full_editor() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    drag(&mut h, slim_band_pos(f, 2500.0, 0.0), slim_band_pos(f, 5000.0, 6.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "b3_freq") / 5000.0).log2().abs() < 0.08, "{}", raw(&h, t, m, "b3_freq"));
    assert!((raw(&h, t, m, "b3_gain") - 6.0).abs() < 0.6, "{}", raw(&h, t, m, "b3_gain"));
    assert_eq!(steps(&h), n + 1, "a band drag is one undo step");

    let before = param(&h, t, m, "depth");
    let k = at(&h, f, "Depth").expect("the Depth knob").center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "depth") - (before + 0.2)).abs() < 0.01, "25 px up is +0.2, as the full editor's");

    let delta = at(&h, f, "Delta").expect("Delta").center();
    click(&mut h, delta);
    assert_eq!(raw(&h, t, m, "delta"), 1.0);
}

#[test]
fn more_holds_the_timing_stereo_and_output_controls() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let closed = f.height();
    let more = at(&h, f, "More").expect("More").center();
    click(&mut h, more);
    h.run_steps(4);
    let f = frame(&h, t, m);
    assert!(f.height() > closed + 150.0, "the editor grew: {closed} -> {}", f.height());
    assert!(f.height() < 900.0, "still about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Attack", "Release", "Mix", "Trim", "Link", "Balance", "L/R|M/S"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in More"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    // M/S: the right half of the stereo switch.
    let seg = at(&h, f, "L/R|M/S").unwrap();
    click(&mut h, seg.center() + vec2(seg.width() * 0.25, 0.0));
    assert_eq!(raw(&h, t, m, "stereo"), 1.0);
}

#[test]
fn the_drawer_command_opens_more_and_the_setting_switches_layouts() {
    let (mut h, t, m) = slim();
    let closed = frame(&h, t, m).height();
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(4);
    assert!(frame(&h, t, m).height() > closed + 150.0);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::rez::SIZE);
}

#[test]
fn hostile_parameters_draw_without_panicking() {
    let (mut h, t, m) = slim();
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert!((frame(&h, t, m).width() - W).abs() <= 1.0);
}
