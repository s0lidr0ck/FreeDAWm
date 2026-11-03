//! Shale's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its header,
//! the Mic menu, the dials, Kit Link and its phase row, and its More section (Shale's own More
//! drawer).

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

fn slim_on(names: &[(&str, bool)]) -> (Harness<'static, SoundApp>, Vec<(TrackId, ModuleId)>) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let mut ids = Vec::new();
    for (track, open) in names {
        let (t, m) = add(&mut app, track, "fs_shale");
        if *open {
            app.ui.drawers.insert(t, m);
        }
        ids.push((t, m));
    }
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1400.0), "dark");
    h.run_steps(6);
    (h, ids)
}

fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let (h, ids) = slim_on(&[("Pad", true)]);
    let (t, m) = ids[0];
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
    assert!(f.height() < 900.0, "about a strip tall with More closed: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Undo", "Bypass", "Preset Init", "<", ">", "Mic", "Kit Link", "Phase Ref"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the editor"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_main_dial_drag_is_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let before = param(&h, t, m, "threshold");
    let k = at(&h, f, "Threshold").expect("the Threshold dial").center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "threshold") - (before + 0.2)).abs() < 0.02, "25 px up is +0.2");
    assert_eq!(steps(&h), n + 1, "one undo step");
}

#[test]
fn the_mic_menu_picks_with_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let at_ = at(&h, f, "Mic").unwrap().center();
    click(&mut h, at_);
    let kick = h.get_by_label("Kick In").rect().center();
    click(&mut h, kick);
    assert_eq!(raw(&h, t, m, "instrument"), 0.0, "Kick In");
    assert_eq!(steps(&h), n + 1, "one DAW undo step");
    let at_ = at(&h, frame(&h, t, m), "Undo").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "instrument"), 3.0, "Snare Top again, by the editor's Undo");
}

#[test]
fn kit_link_gates_its_controls() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let group = at(&h, f, "A|B|C|D").expect("the link group");
    let b = egui::pos2(group.left() + group.width() * 0.375, group.center().y);
    click(&mut h, b);
    assert_eq!(raw(&h, t, m, "linkGroup"), 0.0, "inactive while Kit Link is off");
    let at_ = at(&h, f, "Kit Link").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "link"), 1.0);
    let n = steps(&h);
    let group = at(&h, frame(&h, t, m), "A|B|C|D").unwrap();
    click(&mut h, egui::pos2(group.left() + group.width() * 0.375, group.center().y));
    assert_eq!(raw(&h, t, m, "linkGroup"), 1.0, "group B");
    assert_eq!(steps(&h), n + 1);
    let at_ = at(&h, frame(&h, t, m), "Phase Ref").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "phaseRef"), 1.0, "Phase Ref takes clicks with Kit Link on");
}

#[test]
fn more_holds_the_drawer_and_is_per_module() {
    let (mut h, ids) = slim_on(&[("Pad", true), ("Bass", true)]);
    let ((t1, m1), (t2, m2)) = (ids[0], ids[1]);
    let f1 = frame(&h, t1, m1);
    let h0 = f1.height();
    assert!(!has(&h, f1, "Hold"), "More starts closed");
    let at_ = at(&h, f1, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(f1.height() > h0, "the editor grew");
    for label in [
        "Hold",
        "Retrigger",
        "Vel Track",
        "Mix",
        "MIDI Note",
        "Det Low",
        "Det High",
        "Crossover",
        "Input",
        "Output",
        "Auto Band",
        "SC Listen",
        "MIDI Out",
    ] {
        assert!(has(&h, f1, label), "{label} is in More");
    }
    assert!(!has(&h, f2, "Hold"), "the other module's More stays closed");
    // A More dial: the full drawer's command, one step.
    let n = steps(&h);
    let before = param(&h, t1, m1, "hold");
    let k = at(&h, f1, "Hold").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert!(param(&h, t1, m1, "hold") > before + 0.1);
    assert_eq!(steps(&h), n + 1);
    // Auto Band holds the detector band.
    set(&mut h, t1, m1, json!({"autoBand": 1.0}));
    let low = raw(&h, t1, m1, "detLow");
    let k = at(&h, frame(&h, t1, m1), "Det Low").unwrap().center();
    drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
    assert_eq!(raw(&h, t1, m1, "detLow"), low, "held by Auto Band");
}

#[test]
fn the_slim_more_is_not_the_full_editors() {
    let (mut h, t, m) = slim();
    let at_ = at(&h, frame(&h, t, m), "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(940.0, 640.0), "the full editor, its More drawer closed");
    // A More tab sent to the full editor opens its own drawer, not the slim section.
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(940.0, 896.0));
}
