//! LevelUp's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its dials,
//! the tabs (editor state, per module) and what each shows, a platform pick, and the full editor
//! unchanged without the flag.

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
    let (t, m) = add(&mut app, "Pad", "fs_levelup");
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

const TABS: &str = "Level|Reference|Deliver";

/// A click on tab `i` of the Level / Reference / Deliver row of the editor in `f`.
fn tab(h: &mut Harness<SoundApp>, f: Rect, i: usize) {
    let r = at(h, f, TABS).unwrap();
    click(h, egui::pos2(r.left() + r.width() * (i as f32 + 0.5) / 3.0, r.center().y));
    h.run_steps(2);
}

#[test]
fn it_is_the_drawers_width_and_the_header_fits() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 760.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Bypass", TABS, "Off|IN|OUT", "A|B|C|D", "Learn", "Normalize", "Freeze", "Ceiling"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "maxGain");
    let k = at(&h, f, "Max Gain").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "maxGain") > before + 0.1);
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_role_and_pair_send_the_full_editors_commands() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let r = at(&h, f, "Off|IN|OUT").unwrap();
    click(&mut h, egui::pos2(r.left() + r.width() * 0.5, r.center().y));
    assert_eq!(raw(&h, t, m, "link"), 1.0, "IN");
    assert_eq!(steps(&h), n + 1);
    let r = at(&h, frame(&h, t, m), "A|B|C|D").unwrap();
    click(&mut h, egui::pos2(r.right() - r.width() / 8.0, r.center().y));
    assert_eq!(raw(&h, t, m, "linkGroup"), 3.0, "D");
}

#[test]
fn the_tabs_are_editor_state_and_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_levelup");
    let (t2, m2) = add(&mut app, "Bass", "fs_levelup");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let n = steps(&h);
    let f1 = frame(&h, t1, m1);
    tab(&mut h, f1, 1);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert_eq!(steps(&h), n, "a tab is not an undo step");
    assert!(has(&h, f1, "Reference 1") && has(&h, f1, "Mix|Reference"), "the reference slots");
    assert!(!has(&h, f1, "Target"), "the Level tab's dials are gone");
    assert!(has(&h, f2, "Target") && !has(&h, f2, "Reference 1"), "the other LevelUp stays on Level");
    // A slot click selects it.
    let r3 = at(&h, f1, "Reference 3").unwrap();
    click(&mut h, egui::pos2(r3.left() + 40.0, r3.center().y));
    assert_eq!(raw(&h, t1, m1, "refSlot"), 2.0);
    // Deliver: the platforms, a pick is one undo step, a second click puts it back to none.
    let f1 = frame(&h, t1, m1);
    tab(&mut h, f1, 2);
    let f1 = frame(&h, t1, m1);
    assert!(has(&h, f1, "Off|24-bit|16-bit"), "Dither");
    let n = steps(&h);
    let sp = at(&h, f1, "Spotify").unwrap().center();
    click(&mut h, sp);
    assert_eq!(raw(&h, t1, m1, "platform"), 1.0);
    assert_eq!(steps(&h), n + 1);
    click(&mut h, sp);
    assert_eq!(raw(&h, t1, m1, "platform"), 0.0);
}

#[test]
fn learn_is_a_momentary_press() {
    let (mut h, t, m) = slim();
    let n = steps(&h);
    let l = at(&h, frame(&h, t, m), "Learn").unwrap().center();
    click(&mut h, l);
    h.run_steps(20);
    assert_eq!(raw(&h, t, m, "learn"), 0.0, "let go after 150 ms");
    assert!(steps(&h) > n, "the press is undoable (Learn may also write Target back)");
}

#[test]
fn the_full_editor_is_unchanged_without_the_flag() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(820.0, 580.0));
    let _ = set;
}
