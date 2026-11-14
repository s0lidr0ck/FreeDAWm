//! Sift's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its two Amount
//! dials, Listen and the More section, with the full editor's own gestures.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;
const LISTEN: &str = "Off|Sibilance|Plosives|Removed";

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Vox", "Bass"]);
    let (t, m) = add(&mut app, "Vox", "fs_sift");
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
    for label in ["Undo", "A", "B", "Bypass", "Sibilance", "Plosives", LISTEN] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(at(&h, f, "Plosive Ceiling").is_none(), "the ceiling is under More");
}

#[test]
fn a_dial_drag_is_one_undo_step_and_the_editors_undo_restores_it() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    for (label, id) in [("Sibilance", "sibAmount"), ("Plosives", "ploAmount")] {
        let n = steps(&h);
        let before = param(&h, t, m, id);
        let k = at(&h, f, label).unwrap().center();
        drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
        assert!(param(&h, t, m, id) > before + 0.1, "{id}");
        assert_eq!(steps(&h), n + 1, "one undo step");
        let u = at(&h, frame(&h, t, m), "Undo").unwrap().center();
        click(&mut h, u);
        assert!((param(&h, t, m, id) - before).abs() < 1e-4, "the editor's Undo");
    }
}

#[test]
fn listen_picks_a_part() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let r = at(&h, f, LISTEN).unwrap();
    click(&mut h, pos2(r.left() + r.width() * 2.5 / 4.0, r.center().y));
    assert_eq!(raw(&h, t, m, "listen"), 2.0);
    click(&mut h, pos2(r.left() + r.width() * 0.5 / 4.0, r.center().y));
    assert_eq!(raw(&h, t, m, "listen"), 0.0);
}

#[test]
fn more_holds_the_rest_and_grows_the_editor() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let more = at(&h, f, "More").unwrap().center();
    click(&mut h, more);
    h.run_steps(4);
    let open = frame(&h, t, m);
    assert!(open.height() > f.height() + 60.0, "{} -> {}", f.height(), open.height());
    let inner = open.shrink2(vec2(4.0, 0.0));
    for label in ["Sibilance Range", "Sibilance Low", "Sibilance High", "Plosive Range", "Plosive Ceiling", "Split|Wide"] {
        let r = at(&h, open, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {open:?}");
    }
    let k = at(&h, open, "Plosive Ceiling").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 20.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "ploHigh") > 180.0);
    let mode = at(&h, open, "Split|Wide").unwrap();
    click(&mut h, pos2(mode.left() + mode.width() * 0.75, mode.center().y));
    assert_eq!(raw(&h, t, m, "mode"), 1.0);
}

#[test]
fn slim_and_full_keep_their_own_sizes() {
    let (mut h, t, m) = slim();
    assert!((frame(&h, t, m).width() - W).abs() <= 1.0);
    h.state_mut().ui.slim_editors = false;
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::sift::SIZE);
}
