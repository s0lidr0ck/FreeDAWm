//! Reverb's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, a dial drag,
//! the machine menu, the era row and Interval only where they apply, the tabs (per module), and
//! the full editor.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;
const TABS: &str = "Tone|Time|Motion|Dynamics|Setup";
const ERAS: &str = "1970s|1980s|Now";

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn has(h: &Harness<SoundApp>, f: Rect, label: &str) -> bool {
    at(h, f, label).is_some()
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_reverb");
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

/// Clicks segment `i` of the `n` in `r`.
fn pick(h: &mut Harness<SoundApp>, r: Rect, i: usize, n: usize) {
    click(h, egui::pos2(r.left() + r.width() * (i as f32 + 0.5) / n as f32, r.center().y));
}

#[test]
fn it_is_the_drawers_width_and_a_strip_tall() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 760.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Machine", TABS, "Freeze"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_main_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = raw(&h, t, m, "decay");
    let k = at(&h, f, "Decay").expect("the Decay dial").center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "decay") > before, "Decay went up");
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_machine_menu_picks_with_the_editors_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let start = raw(&h, t, m, "machine");
    let n = steps(&h);
    let c = at(&h, f, "Machine").unwrap().center();
    click(&mut h, c);
    let plate = h.get_by_label("Plate").rect().center();
    click(&mut h, plate);
    assert_eq!(raw(&h, t, m, "machine"), 2.0, "Plate");
    assert_eq!(steps(&h), n + 1, "one undo step");
    let u = at(&h, frame(&h, t, m), "Undo").unwrap().center();
    click(&mut h, u);
    assert_eq!(raw(&h, t, m, "machine"), start);
}

#[test]
fn the_era_and_interval_show_only_where_they_apply() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let era = at(&h, f, ERAS).expect("the era row on the Hall");
    pick(&mut h, era, 0, 3);
    assert_eq!(raw(&h, t, m, "color"), 0.0, "1970s");
    set(&mut h, t, m, json!({"machine": 3.0}));
    assert!(!has(&h, frame(&h, t, m), ERAS), "no era on the Spring");
    // Interval: on the Shimmer's Motion tab only.
    let tabs = at(&h, frame(&h, t, m), TABS).unwrap();
    pick(&mut h, tabs, 2, 5);
    assert!(!has(&h, frame(&h, t, m), "+Oct|+5th|-Oct"), "no Interval on the Spring");
    set(&mut h, t, m, json!({"machine": 5.0}));
    let f = frame(&h, t, m);
    let iv = at(&h, f, "+Oct|+5th|-Oct").expect("Interval on the Shimmer");
    pick(&mut h, iv, 2, 3);
    assert_eq!(raw(&h, t, m, "interval"), 2.0, "-Oct");
}

#[test]
fn two_reverbs_keep_their_own_tab() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_reverb");
    let (t2, m2) = add(&mut app, "Bass", "fs_reverb");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Low Cut") && has(&h, f2, "Low Cut"), "both open on Tone");
    let tabs = at(&h, f1, TABS).unwrap();
    pick(&mut h, tabs, 4, 5);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Manual BPM") && !has(&h, f1, "Low Cut"), "the first shows Setup");
    assert!(has(&h, f2, "Low Cut"), "the second still shows Tone");
    // Setup's pills are the full editor's: one undo step each.
    let n = steps(&h);
    let trails = raw(&h, t1, m1, "trails");
    let r = at(&h, f1, "Trails").unwrap().center();
    click(&mut h, r);
    assert_ne!(raw(&h, t1, m1, "trails"), trails);
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn the_full_editor_is_unchanged_without_the_flag() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 527.0), "the full editor");
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "Machine"})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": true})).unwrap();
    h.run_steps(4);
    assert!(h.query_all_by_label("Room").next().is_none(), "a Machine tab sent to the full editor does not open the slim menu");
}
