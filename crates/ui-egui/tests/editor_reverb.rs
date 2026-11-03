//! ReverbEngine's VST editor in the Mix window: its size (with and without the era row), its
//! gestures going through commands, hostile values, and following undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::editors::reverb;

fn ins(
    h: &egui_kittest::Harness<soundcraft_ui_egui::SoundApp>,
    t: soundcraft_model::TrackId,
    m: soundcraft_model::ModuleId,
) -> soundcraft_model::Insert {
    h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().clone()
}

#[test]
fn reverb_editor_is_its_vst_size() {
    let (mut h, t, m) = open("fs_reverb", vec2(1800.0, 1000.0), "dark");
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 527.0), "Hall: the era row shows");
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"machine": 3.0}})).unwrap();
    h.run_steps(3);
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 496.0), "the Spring has no era");
}

#[test]
fn reverb_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_reverb", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let steps = h.state().engine.undo_history().len();
    let at = reverb::dial_centre(&ins(&h, t, m), f, 0, "decay").unwrap();
    let before = param(&h, t, m, "decay");
    drag(&mut h, at, at - vec2(0.0, 40.0), Modifiers::NONE);
    let after = param(&h, t, m, "decay");
    assert!((after - (before + 40.0 * 0.008)).abs() < 0.02, "{before} -> {after}");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "a drag is one undo step");

    // The machine row: Plate, one undo step.
    let (machine, tabs) = reverb::rows(&ins(&h, t, m), f);
    click(&mut h, pos2(machine.left() + machine.width() * (2.5 / 6.0), machine.center().y));
    assert_eq!(raw(&h, t, m, "machine"), 2.0);
    assert_eq!(h.state().engine.undo_history().len(), steps + 2);

    // A tab switch is editor state.
    click(&mut h, pos2(tabs.left() + tabs.width() * (3.5 / 5.0), tabs.center().y));
    assert_eq!(h.state().engine.undo_history().len(), steps + 2);
    let duck = reverb::dial_centre(&ins(&h, t, m), f, 3, "duck").unwrap();
    drag(&mut h, duck, duck - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "duck") > 0.2, "the Dynamics tab's Duck dial is live");
}

#[test]
fn reverb_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_reverb", vec2(1800.0, 1000.0), "light");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    let f = frame(&h, t, m);
    for tab in 0..5 {
        let (_, tabs) = reverb::rows(&ins(&h, t, m), f);
        click(&mut h, pos2(tabs.left() + tabs.width() * ((tab as f32 + 0.5) / 5.0), tabs.center().y));
    }
    let _ = frame(&h, t, m);
}

#[test]
fn reverb_editor_follows_undo() {
    let (mut h, t, m) = open("fs_reverb", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"machine": 3.0}})).unwrap();
    h.run_steps(3);
    assert_eq!(frame(&h, t, m).height(), 496.0);
    undo(&mut h);
    assert_eq!(frame(&h, t, m).height(), 527.0, "undo brings the era row back on the next frame");
}
