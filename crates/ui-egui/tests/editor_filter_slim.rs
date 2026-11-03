//! FilterEngine's slim editor (View > Mix Window Views > Slim Plugin Editors): its width and
//! header, a main dial as one undo step, the circuit menu with the editor's Undo, the switch rows
//! and the modulation panel following the circuit and the source, and More per module.

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
    let (t, m) = add(&mut app, "Pad", "fs_filter");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1100.0), "dark");
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
fn the_slim_filter_is_the_drawers_width_with_its_header_inside() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Circuit"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the header"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(f.height() < 900.0, "about a strip tall: {}", f.height());
}

#[test]
fn a_main_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = raw(&h, t, m, "drive");
    let k = at(&h, f, "Drive").expect("the Drive dial").center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "drive") > before, "Drive went up");
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_circuit_menu_picks_with_the_editors_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let at_ = at(&h, f, "Circuit").unwrap().center();
    click(&mut h, at_);
    let moog = h.get_by_label("Moog Moog transistor ladder").rect().center();
    click(&mut h, moog);
    assert_eq!(raw(&h, t, m, "circuit"), 4.0, "Moog");
    assert_eq!(steps(&h), n + 1, "one undo step");
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "Undo").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "circuit"), 0.0, "Q-Tron again");
}

#[test]
fn the_panel_follows_the_source_and_the_rows_the_circuit() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    // Envelope by default: Sens, Attack, Release.
    assert!(has(&h, f, "Sens") && has(&h, f, "Depth"));
    set(&mut h, t, m, json!({"modSrc": 0.0}));
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Sens") && !has(&h, f, "Depth"), "Manual: no panel dials, no Depth");
    set(&mut h, t, m, json!({"modSrc": 3.0}));
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Steps") && has(&h, f, "Glide"));
    // The step strip: a drag over it is one undo step.
    let n = steps(&h);
    let glide = at(&h, f, "Glide").unwrap();
    let y = glide.bottom() + 70.0;
    drag(&mut h, egui::pos2(f.left() + 30.0, y), egui::pos2(f.left() + 30.0, y - 20.0), Modifiers::NONE);
    assert_eq!(steps(&h), n + 1, "the step drag is one undo step");
    // The Wasp has one switch row (Mode), the Q-Tron two (Range, Mode).
    let h0 = frame(&h, t, m).height();
    set(&mut h, t, m, json!({"circuit": 8.0}));
    h.run_steps(2);
    assert!(frame(&h, t, m).height() < h0, "one switch row fewer");
}

#[test]
fn more_holds_the_setup_row_per_module() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_filter");
    let (t2, m2) = add(&mut app, "Bass", "fs_filter");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1100.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    assert!(!has(&h, f1, "BPM"), "More starts closed");
    let at_ = at(&h, f1, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "BPM") && has(&h, f1, "Use Manual BPM"), "the setup row is in More");
    assert!(!has(&h, f2, "BPM"), "the other Filter's More stays closed");
    let n = steps(&h);
    let pill = at(&h, f1, "Use Manual BPM").unwrap().center();
    click(&mut h, pill);
    assert_eq!(raw(&h, t1, m1, "tempoSrc"), 1.0);
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn the_full_editor_is_unchanged_with_slim_off() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).width(), 920.0, "the VST's size");
}
