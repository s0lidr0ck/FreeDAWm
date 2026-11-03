//! Twine's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its dials,
//! the crossovers only in Multiband, the More section (per module) and the VST's own Undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn has(h: &Harness<SoundApp>, f: Rect, label: &str) -> bool {
    at(h, f, label).is_some()
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_twine");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

fn set(h: &mut Harness<SoundApp>, t: TrackId, m: ModuleId, params: serde_json::Value) {
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": params})).unwrap();
    h.run_steps(4);
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

#[test]
fn it_is_the_drawers_width_and_the_header_fits() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 700.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "Bypass", "<", ">", "Match", "Chaff", "Multiband"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_dial_drag_is_one_undo_step_and_the_vsts_undo_restores_it() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "drive");
    let k = at(&h, f, "Drive").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "drive") > before + 0.1);
    assert_eq!(steps(&h), n + 1, "one undo step");
    let u = at(&h, frame(&h, t, m), "Undo").unwrap().center();
    click(&mut h, u);
    assert!((param(&h, t, m, "drive") - before).abs() < 1e-4, "the VST's Undo");
}

#[test]
fn the_crossovers_show_only_in_multiband() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Crossover"));
    let mb = at(&h, f, "Multiband").unwrap().center();
    click(&mut h, mb);
    assert_eq!(raw(&h, t, m, "multi"), 1.0);
    h.run_steps(2);
    assert!(has(&h, frame(&h, t, m), "Crossover"));
    set(&mut h, t, m, json!({"multi": 0.0}));
    assert!(!has(&h, frame(&h, t, m), "Crossover"));
}

#[test]
fn more_holds_the_shape_and_the_limiter_and_is_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_twine");
    let (t2, m2) = add(&mut app, "Bass", "fs_twine");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    let shapes = "Hard|Soft|Tape";
    assert!(!has(&h, f1, shapes), "More starts closed");
    let more = at(&h, f1, "More").unwrap().center();
    click(&mut h, more);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, shapes) && has(&h, f1, "Transparent|Punchy|Loud"));
    assert!(!has(&h, f2, shapes), "the other Twine's More stays closed");
    // Tape: one undo step, and the VST's Undo puts the shape back.
    let before = raw(&h, t1, m1, "shape");
    let n = steps(&h);
    let r = at(&h, f1, shapes).unwrap();
    click(&mut h, egui::pos2(r.right() - r.width() / 6.0, r.center().y));
    assert_eq!(raw(&h, t1, m1, "shape"), 2.0);
    assert_eq!(steps(&h), n + 1);
    let u = at(&h, frame(&h, t1, m1), "Undo").unwrap().center();
    click(&mut h, u);
    assert_eq!(raw(&h, t1, m1, "shape"), before);
}

#[test]
fn the_full_editor_is_unchanged_without_the_flag() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::twine::SIZE);
}
