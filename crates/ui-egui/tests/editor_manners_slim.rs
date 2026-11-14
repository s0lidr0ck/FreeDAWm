//! Manners' slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its dials,
//! the More section and the range edges, with the full editor's own gestures.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_manners");
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
    assert!(f.height() > 300.0 && f.height() < 700.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Tame", "Recover", "Delta"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(at(&h, f, "Bias").is_none(), "Bias is under More");
}

#[test]
fn a_dial_drag_is_one_undo_step_and_the_editors_undo_restores_it() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "tame");
    let k = at(&h, f, "Tame").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "tame") > before + 0.1);
    assert_eq!(steps(&h), n + 1, "one undo step");
    let u = at(&h, frame(&h, t, m), "Undo").unwrap().center();
    click(&mut h, u);
    assert!((param(&h, t, m, "tame") - before).abs() < 1e-4, "the editor's Undo");
}

#[test]
fn more_holds_the_tone_dials_and_grows_the_editor() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let more = at(&h, f, "More").unwrap().center();
    click(&mut h, more);
    h.run_steps(4);
    let open = frame(&h, t, m);
    assert!(open.height() > f.height() + 60.0, "{} -> {}", f.height(), open.height());
    let inner = open.shrink2(vec2(4.0, 0.0));
    for label in ["Bias", "Brighten", "Boost"] {
        let r = at(&h, open, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {open:?}");
    }
    let k = at(&h, open, "Brighten").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 20.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "brighten") > 10.0);
}

#[test]
fn delta_and_the_range_edges_work_here_too() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let d = at(&h, f, "Delta").unwrap().center();
    click(&mut h, d);
    assert_eq!(raw(&h, t, m, "delta"), 1.0);
    // The low edge is the plot's left side (8 px inside the display, itself 10 px in), half way
    // down the display, which starts under the two header rows.
    let display_top = at(&h, f, "Undo").unwrap().bottom() + 10.0;
    let edge = egui::pos2(f.left() + 18.0, display_top + 85.0);
    drag(&mut h, edge, edge + vec2(80.0, 0.0), Modifiers::NONE);
    let lo = raw(&h, t, m, "rangeLo");
    assert!(lo > 100.0 && lo < 400.0, "{lo}");
    assert_eq!(raw(&h, t, m, "rangeHi"), 20_000.0);
}

#[test]
fn slim_and_full_keep_their_own_sizes() {
    let (mut h, t, m) = slim();
    assert!((frame(&h, t, m).width() - W).abs() <= 1.0);
    h.state_mut().ui.slim_editors = false;
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::manners::SIZE);
}
