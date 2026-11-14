//! Lull's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its header, the
//! scope's threshold, the main dials and its More section.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::editors::lull::{scope_level_y, slim_scope_rect};

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "lull");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1100.0), "dark");
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
    for label in ["Undo", "A", "B", "Listen", "Threshold", "Range", "Release", "Expand|Gate", "More"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(at(&h, f, "Attack").is_none(), "Attack is in More");
}

#[test]
fn the_scope_and_the_dials_work_as_in_the_full_editor() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let scope = slim_scope_rect(f);
    let x = scope.center().x;
    drag(&mut h, pos2(x, scope_level_y(scope, -40.0)), pos2(x, scope_level_y(scope, -20.0)), Modifiers::NONE);
    assert!((raw(&h, t, m, "threshold") + 20.0).abs() < 1.5, "{}", raw(&h, t, m, "threshold"));
    assert_eq!(steps(&h), n + 1, "a threshold drag is one undo step");
    let before = param(&h, t, m, "range");
    let k = at(&h, f, "Range").expect("the Range knob").center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "range") - (before + 0.2)).abs() < 0.01, "25 px up is +0.2, as the full editor's");
    let listen = at(&h, f, "Listen").expect("Listen").center();
    click(&mut h, listen);
    assert_eq!(raw(&h, t, m, "listen"), 1.0);
}

#[test]
fn more_holds_the_rest() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let closed = f.height();
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(4);
    let f = frame(&h, t, m);
    assert!(f.height() > closed + 200.0, "the editor grew: {closed} -> {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in [
        "Ratio",
        "Knee",
        "Hysteresis",
        "Attack",
        "Hold",
        "Lookahead",
        "High-Pass",
        "Low-Pass",
        "Guard",
        "Guard Delay",
        "Guard|Trigger",
        "Link",
        "Mix",
        "Trim",
    ] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in More"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    let key = at(&h, f, "Guard|Trigger").unwrap();
    click(&mut h, key.center() + vec2(key.width() * 0.25, 0.0));
    assert_eq!(raw(&h, t, m, "key"), 1.0);
    // The setting switches layouts.
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::lull::SIZE);
}

#[test]
fn hostile_parameters_draw_without_panicking() {
    let (mut h, t, m) = slim();
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert!((frame(&h, t, m).width() - W).abs() <= 1.0);
}
