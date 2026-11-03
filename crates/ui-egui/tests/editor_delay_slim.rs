//! DelayEngine's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, the style
//! menu, the main dials, the contextual Heads, the More section with the drawer's tabs, and two
//! Delays keeping their own More.

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
    let (t, m) = add(&mut app, "Pad", "fs_delay");
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
    for label in ["Undo", "A", "B", "Bypass", "Sync", "Tap", "Reverse", "Freeze"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_main_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "feedback");
    let k = at(&h, f, "Feedback").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "feedback") > before + 0.1);
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_style_menu_picks_with_the_editors_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "1|2|3|4|1+2|1+3|2+4|All"), "no Heads on Digital");
    let n = steps(&h);
    let menu = at(&h, f, "Style").unwrap().center();
    click(&mut h, menu);
    let tape = h.get_by_label("Tape").rect().center();
    click(&mut h, tape);
    assert_eq!(raw(&h, t, m, "style"), 3.0, "Tape");
    assert_eq!(steps(&h), n + 1, "one undo step");
    h.run_steps(2);
    let f = frame(&h, t, m);
    assert!(has(&h, f, "1|2|3|4|1+2|1+3|2+4|All"), "Tape shows its Heads");
    // The editor's Undo puts Digital back.
    let u = at(&h, f, "Undo").unwrap().center();
    click(&mut h, u);
    assert_eq!(raw(&h, t, m, "style"), 0.0);
    h.run_steps(2);
    assert!(!has(&h, frame(&h, t, m), "1|2|3|4|1+2|1+3|2+4|All"), "the Heads go again");
}

#[test]
fn more_holds_the_tabs_and_is_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_delay");
    let (t2, m2) = add(&mut app, "Bass", "fs_delay");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    assert!(!has(&h, f1, "Low Cut"), "More starts closed");
    let h0 = f1.height();
    let more = at(&h, f1, "More").unwrap().center();
    click(&mut h, more);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Low Cut"), "the Tone tab's dials");
    assert!(!has(&h, f2, "Low Cut"), "the other Delay's More stays closed");
    assert!(f1.height() > h0, "the editor grew");
    // A tab switch is editor state, then its dial sends the full editor's command.
    let n = steps(&h);
    // the tabs: two rows of three, Dynamics the second of the first row
    let tabs = at(&h, f1, "Tone|Dynamics|Motion|Rhythm|Stereo|Setup").unwrap();
    let dyn_ = egui::pos2(tabs.center().x, tabs.top() + tabs.height() / 4.0);
    click(&mut h, dyn_);
    h.run_steps(2);
    assert_eq!(steps(&h), n, "a tab switch is not an undo step");
    let f1 = frame(&h, t1, m1);
    let duck = at(&h, f1, "Duck").unwrap().center();
    drag(&mut h, duck, duck - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t1, m1, "duck") > 0.2);
    assert_eq!(steps(&h), n + 1);
    let more = at(&h, frame(&h, t1, m1), "More").unwrap().center();
    click(&mut h, more);
    h.run_steps(2);
    assert!(!has(&h, frame(&h, t1, m1), "Duck"), "closed again");
}

#[test]
fn sync_swaps_time_for_the_note() {
    let (mut h, t, m) = slim();
    assert!(has(&h, frame(&h, t, m), "Time"));
    set(&mut h, t, m, json!({"sync": 1.0}));
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Note") && !has(&h, f, "Time"), "Sync shows the Note");
}

#[test]
fn the_full_editor_is_unchanged_without_the_flag() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 496.0));
}
