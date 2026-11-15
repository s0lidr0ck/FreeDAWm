//! CopyQ's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, what it shows,
//! the More section, and the full editor's own gestures.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_dsp::plugins::fs::copyq::CopyQState;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_copyq");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1400.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

fn targets(h: &Harness<SoundApp>, t: TrackId, m: ModuleId) -> usize {
    let s = h.state().engine.session();
    let ins = s.track(t).unwrap().mixer.module(m).unwrap();
    ins.state_bytes().and_then(|b| CopyQState::read(&b)).map_or(0, |s| s.targets.len())
}

#[test]
fn it_is_the_drawers_width_and_everything_fits() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() > 400.0 && f.height() < 900.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "A", "B", "Bypass", "Amount", "Detail", "Max Gain", "Capture", "Save", "Profile Pink (-3 dB/oct)"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(at(&h, f, "Dynamics").is_none(), "Dynamics is under More");
}

#[test]
fn a_dial_drag_is_one_undo_step_and_a_profile_is_picked() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "detail");
    let k = at(&h, f, "Detail").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "detail") > before + 0.1);
    assert_eq!(steps(&h), n + 1, "one undo step");
    let pick = at(&h, frame(&h, t, m), "Profile Warm (-4.5 dB/oct)").unwrap().center();
    click(&mut h, pick);
    assert_eq!(targets(&h, t, m), 1);
}

#[test]
fn more_holds_the_rest_and_grows_the_editor() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let more = at(&h, f, "More").unwrap().center();
    click(&mut h, more);
    h.run_steps(4);
    let open = frame(&h, t, m);
    assert!(open.height() > f.height() + 200.0, "{} -> {}", f.height(), open.height());
    let inner = open.shrink2(vec2(4.0, 0.0));
    for label in [
        "Dynamics",
        "Window",
        "Gate",
        "Output",
        "Low",
        "High Mid",
        "Stereo|Mid/Side|Left/Right",
        "Linear Phase|Minimum Phase",
        "Match Level",
        "Delta",
        "Follow",
        "Off|Send|Match|Group",
        "No group",
        "Freeze",
        "Reset",
        "From Selection",
        "Send to CleanQ",
    ] {
        let r = at(&h, open, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {open:?}");
    }
    let phase = at(&h, open, "Linear Phase|Minimum Phase").unwrap();
    click(&mut h, phase.right_center() - vec2(10.0, 0.0));
    assert_eq!(raw(&h, t, m, "phase"), 1.0);
}
