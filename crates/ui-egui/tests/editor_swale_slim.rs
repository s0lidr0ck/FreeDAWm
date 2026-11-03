//! Swale's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, a stage dial,
//! the contextual curve-row controls and Sens / Rate, and More (Swale's own `showMore`, per module
//! and shared with the full editor).

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
    let (t, m) = add(&mut app, "Pad", "fs_swale");
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
    assert!(f.height() < 760.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "Bypass"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_stage_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "attack");
    let k = at(&h, f, "Attack").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "attack") > before + 0.1);
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_contextual_controls_follow_the_mode() {
    let (mut h, t, m) = slim();
    // Pick: Sens and Per Note / Phrase; no Peak on Beat, no Rate.
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Sens") && !has(&h, f, "Rate"));
    assert!(has(&h, f, "Per Note|Phrase") && !has(&h, f, "Peak on Beat"));
    // The mode row: Rhythmic is one undo step and swaps them.
    let n = steps(&h);
    let modes = at(&h, f, "Pick|One-Shot|Rhythmic").unwrap();
    click(&mut h, egui::pos2(modes.right() - modes.width() / 6.0, modes.center().y));
    assert_eq!(raw(&h, t, m, "mode"), 2.0);
    assert_eq!(steps(&h), n + 1);
    h.run_steps(2);
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Rate") && !has(&h, f, "Sens"));
    assert!(has(&h, f, "Peak on Beat") && !has(&h, f, "Per Note|Phrase"));
    // Peak on Beat sends the full editor's toggle.
    let pb = at(&h, f, "Peak on Beat").unwrap().center();
    let before = raw(&h, t, m, "peakBeat");
    let n = steps(&h);
    click(&mut h, pb);
    assert_ne!(raw(&h, t, m, "peakBeat"), before, "flipped");
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn more_is_swales_own_and_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_swale");
    let (t2, m2) = add(&mut app, "Bass", "fs_swale");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    assert!(!has(&h, f1, "Alternate"), "More starts closed");
    let h0 = f1.height();
    let more = at(&h, f1, "More").unwrap().center();
    click(&mut h, more);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Alternate") && has(&h, f1, "Clean|Slow Gear"), "the More panel");
    assert!(has(&h, f1, "Host|Manual"), "the tempo foot");
    assert!(!has(&h, f2, "Alternate"), "the other Swale's More stays closed");
    assert!(f1.height() > h0);
    let n = steps(&h);
    let k = at(&h, f1, "Alternate").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t1, m1, "alternate") > 0.1);
    assert_eq!(steps(&h), n + 1, "one undo step");
    // The full editor opens with its More panel too (`showMore` is the module's).
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t1, m1).height(), 739.0, "the full editor with More");
    assert_eq!(frame(&h, t2, m2).height(), 640.0, "and the other without");
}

#[test]
fn the_full_editor_is_unchanged_without_the_flag() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(860.0, 640.0));
    let _ = set;
}
