//! DistEngine's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its
//! header, the Circuit menu, the dials, Link and the calibration wizard, and the switch rows in
//! More.

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
    let (t, m) = add(&mut app, "Pad", "fs_dist");
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

fn open_more(h: &mut Harness<SoundApp>, t: TrackId, m: ModuleId) {
    let at_ = at(h, frame(h, t, m), "More").unwrap().center();
    click(h, at_);
    h.run_steps(2);
}

#[test]
fn it_is_the_drawers_width_and_the_header_fits() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 900.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Preset Init", "Circuit", "Link", "Calibrate"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "drive");
    let k = at(&h, f, "Drive").expect("the Drive dial").center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "drive") - (before + 0.2)).abs() < 0.01, "25 px up is +0.2");
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_circuit_menu_picks_with_undo_and_renames_the_dials() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "Circuit").unwrap().center();
    click(&mut h, at_);
    let rat = h.get_by_label("RAT Distortion \u{00B7} Filter \u{00B7} Volume").rect().center();
    click(&mut h, rat);
    assert_eq!(raw(&h, t, m, "circuit"), 1.0, "RAT");
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Distortion") && has(&h, f, "Filter"), "the dials take the pedal's names");
    let at_ = at(&h, f, "Undo").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "circuit"), 0.0, "Tube Screamer again, by the editor's Undo");
}

#[test]
fn link_locks_level_and_calibrate_shows_the_wizard() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "Link").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "link"), 1.0);
    let level = raw(&h, t, m, "level");
    let k = at(&h, frame(&h, t, m), "Level").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert_eq!(raw(&h, t, m, "level"), level, "Level is locked while Linked");

    assert!(!has(&h, frame(&h, t, m), "Cancel"), "no wizard yet");
    let h0 = frame(&h, t, m).height();
    let at_ = at(&h, frame(&h, t, m), "Calibrate").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Cancel") && has(&h, f, "Next"), "the wizard's buttons");
    assert!(f.height() > h0, "the wizard takes its own room");
    let at_ = at(&h, f, "Cancel").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    assert!(!has(&h, frame(&h, t, m), "Cancel"), "gone after Cancel");
}

/// Steps 3 and 4 add the A / B pill; in the drawer's width it gets a row of its own above Cancel
/// and Next (the shared `link_cal::panel`), inside the editor and clear of the other two.
#[test]
fn the_wizards_ab_pill_fits_the_drawer() {
    let (mut h, t, m) = slim();
    let at_ = at(&h, frame(&h, t, m), "Calibrate").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    for _ in 0..2 {
        let next = at(&h, frame(&h, t, m), "Next").unwrap().center();
        click(&mut h, next);
        h.run_steps(2);
    }
    for (step, ab) in [(3, "A / B  vs dry"), (4, "A / B  vs Drive 0")] {
        let f = frame(&h, t, m);
        let (Some(ab_r), Some(cancel), Some(next)) = (at(&h, f, ab), at(&h, f, "Cancel"), at(&h, f, if step == 3 { "Next" } else { "Done" })) else {
            panic!("step {step}: the wizard's three pills inside the editor");
        };
        assert!(!ab_r.intersects(cancel) && !ab_r.intersects(next), "step {step}: A / B clear of Cancel and Next");
        assert!(ab_r.bottom() <= cancel.top(), "step {step}: A / B on its own row above them");
        if step == 3 {
            click(&mut h, next.center());
            h.run_steps(2);
        }
    }
}

#[test]
fn switch_rows_are_in_more_and_send_their_parameter() {
    let (mut h, t, m) = slim();
    set(&mut h, t, m, json!({"circuit": 1.0}));
    assert!(!has(&h, frame(&h, t, m), "Chip"), "More starts closed");
    open_more(&mut h, t, m);
    let f = frame(&h, t, m);
    // RAT's Chip has five options: a menu.
    let n = steps(&h);
    let at_ = at(&h, f, "Chip").expect("the Chip menu").center();
    click(&mut h, at_);
    let tl = h.get_by_label("TL071").rect().center();
    click(&mut h, tl);
    assert_eq!(raw(&h, t, m, "ratChip"), 4.0, "TL071");
    assert_eq!(steps(&h), n + 1, "one undo step");
    // Fat has two: a segmented switch.
    let f = frame(&h, t, m);
    let seg = at(&h, f, "Off|On").expect("Fat's switch");
    click(&mut h, egui::pos2(seg.right() - seg.width() / 4.0, seg.center().y));
    assert_eq!(raw(&h, t, m, "ratFat"), 1.0);
}

#[test]
fn more_is_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_dist");
    let (t2, m2) = add(&mut app, "Bass", "fs_dist");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    open_more(&mut h, t1, m1);
    assert!(has(&h, frame(&h, t1, m1), "Model"), "the first one's switches show");
    assert!(!has(&h, frame(&h, t2, m2), "Model"), "the second's More stays closed");
}

#[test]
fn the_flag_picks_the_layout() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).width(), 860.0, "the full editor");
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": true})).unwrap();
    h.run_steps(4);
    assert!(!has(&h, frame(&h, t, m), "Model"), "a More tab sent to the full editor is not kept");
}
