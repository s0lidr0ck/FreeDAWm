//! The Mix window's slim top bar: transport keys, the main counter (its format menu) and the edit
//! selection, so time and Play/Record are in reach while mixing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    app.run("edit.select", json!({"tracks": ["Kick"], "start": {"seconds": 4.0}, "end": {"seconds": 12.0}})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h);
    h
}

fn steps(h: &mut Harness<'_, SoundApp>) {
    for _ in 0..6 {
        h.step();
    }
}

#[test]
fn the_mix_window_has_transport_counter_and_selection() {
    let mut h = harness();
    // The selection's Start, End and Length read in the main counter's format (Bars|Beats).
    assert!(h.query_by_label_contains("Selection Start 2|").is_some(), "start at 4 s is in bar 2");
    assert!(h.query_by_label_contains("Selection End").is_some() && h.query_by_label_contains("Selection Length").is_some());
    assert!(h.query_by_label_contains("Main counter").is_some());

    h.get_by_label("Play").click();
    steps(&mut h);
    assert!(h.state().is_playing(), "{}", h.state().ui.status);
    h.get_by_label("Stop").click();
    steps(&mut h);
    assert!(!h.state().is_playing());

    let looped = h.state().engine.session().edit.loop_playback;
    h.get_by_label("Loop Playback (Cmd+Shift+L)").click();
    steps(&mut h);
    assert_ne!(h.state().engine.session().edit.loop_playback, looped);

    // The counter's menu switches its format; the selection follows.
    h.get_by_label_contains("Main counter").click();
    steps(&mut h);
    h.get_by_label("Min:Secs").click();
    steps(&mut h);
    assert_eq!(h.state().engine.session().edit.main_counter, soundcraft_time::TimeFormat::MinSecs);
    assert!(h.query_by_label_contains("Selection Start 0:04").is_some());
}
