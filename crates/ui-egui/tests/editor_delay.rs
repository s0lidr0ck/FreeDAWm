//! DelayEngine's VST editor in the Mix window: its size (with and without the Heads row), its
//! gestures going through commands, hostile values, and following undo.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::editors::delay;

fn ins(
    h: &egui_kittest::Harness<soundcraft_ui_egui::SoundApp>,
    t: soundcraft_model::TrackId,
    m: soundcraft_model::ModuleId,
) -> soundcraft_model::Insert {
    h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().clone()
}

#[test]
fn delay_editor_is_its_vst_size() {
    let (mut h, t, m) = open("fs_delay", vec2(1800.0, 1000.0), "dark");
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 496.0), "setSize (900, requiredHeight())");
    // Tape has heads: the Heads row adds 9 + 28.
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"style": 3.0}})).unwrap();
    h.run_steps(3);
    assert_eq!(frame(&h, t, m).size(), vec2(900.0, 533.0));
}

#[test]
fn delay_gestures_go_through_commands() {
    let (mut h, t, m) = open("fs_delay", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let steps = h.state().engine.undo_history().len();

    // The Time dial: a drag up is one undo step.
    let at = delay::dial_centre(&ins(&h, t, m), f, 0, "time").unwrap();
    let before = param(&h, t, m, "time");
    drag(&mut h, at, at - vec2(0.0, 40.0), Modifiers::NONE);
    let after = param(&h, t, m, "time");
    assert!((after - (before + 40.0 * 0.008)).abs() < 0.02, "0.008 per pixel: {before} -> {after}");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "a drag is one undo step");

    // A drawer tab is editor state: no command, no undo step; its dials show.
    let (_, tabs, _) = delay::rows(&ins(&h, t, m), f);
    let dynamics = pos2(tabs.left() + tabs.width() * (1.5 / 6.0), tabs.center().y);
    click(&mut h, dynamics);
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "a tab switch is not an undo step");
    let duck = delay::dial_centre(&ins(&h, t, m), f, 1, "duck").unwrap();
    drag(&mut h, duck, duck - vec2(0.0, 30.0), Modifiers::NONE);
    assert!(param(&h, t, m, "duck") > 0.2, "the Dynamics tab's Duck dial is showing and live");

    // The style row: Tape is one undo step, and the Heads row appears (the editor grows).
    let (style, _, _) = delay::rows(&ins(&h, t, m), f);
    let n = h.state().engine.undo_history().len();
    click(&mut h, pos2(style.left() + style.width() * (3.5 / 6.0), style.center().y));
    assert_eq!(raw(&h, t, m, "style"), 3.0);
    assert_eq!(h.state().engine.undo_history().len(), n + 1);
    assert_eq!(frame(&h, t, m).height(), 533.0);

    // The foot's Sync pill swaps Time for the Note.
    let (_, _, foot) = delay::rows(&ins(&h, t, m), frame(&h, t, m));
    click(&mut h, pos2(foot.left() + 27.0, foot.center().y));
    assert_eq!(raw(&h, t, m, "sync"), 1.0);
    let f2 = frame(&h, t, m);
    assert!(delay::dial_centre(&ins(&h, t, m), f2, 0, "time").is_none() && delay::dial_centre(&ins(&h, t, m), f2, 0, "division").is_some());
}

#[test]
fn delay_editor_survives_hostile_params() {
    let (mut h, t, m) = open("fs_delay", vec2(1800.0, 1000.0), "light");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    let f = frame(&h, t, m);
    assert!(f.height() == 496.0 || f.height() == 533.0, "{f:?}");
    for tab in 0..6 {
        let (_, tabs, _) = delay::rows(&ins(&h, t, m), f);
        click(&mut h, pos2(tabs.left() + tabs.width() * ((tab as f32 + 0.5) / 6.0), tabs.center().y));
    }
    let _ = frame(&h, t, m);
}

#[test]
fn delay_editor_follows_undo() {
    let (mut h, t, m) = open("fs_delay", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"style": 4.0}})).unwrap();
    h.run_steps(3);
    assert_eq!(frame(&h, t, m).height(), 533.0, "Drum has heads");
    undo(&mut h);
    assert_eq!(frame(&h, t, m).height(), 496.0, "undo takes the Heads row away on the next frame");
}
