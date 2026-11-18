//! Parity section 8, the show page (step 1): Window › Set List plays the songs of the session in
//! order. A song with Go On runs straight into the next; one without stops and waits on the next
//! song's start.

use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn the_set_plays_on_through_go_on_and_waits_after_a_song_without_it() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for (name, start) in [("Alpha", 0), ("Bravo", 48_000), ("Charlie", 96_000)] {
        let id = app.run("markers.add", json!({"name": name, "kind": "selection", "start": start, "end": start + 9_600})).unwrap()["id"].clone();
        app.run("setlist.add", json!({"location": id})).unwrap();
    }
    app.run("setlist.go_on", json!({"index": 0, "value": true})).unwrap();
    app.ui.show_set_list = true;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(0.05).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    h.get_by_label("Play Set").click();
    let (mut heard_alpha, mut heard_bravo) = (false, false);
    for _ in 0..60 {
        h.run_steps(1);
        let t = h.state().engine.transport;
        heard_alpha |= t.playing && t.position < 9_600;
        heard_bravo |= t.playing && (48_000..57_600).contains(&t.position);
    }
    assert!(heard_alpha && heard_bravo, "Alpha, then Bravo straight on");
    let t = h.state().engine.transport;
    assert!(!t.playing, "Bravo has no Go On: the set waits");
    assert_eq!(t.position, 96_000, "on Charlie's start");
    // Stage View over the whole window, and out of it.
    h.get_by_label("Stage View").click();
    h.run_steps(2);
    h.get_by_label("Leave").click();
    h.run_steps(2);
    assert!(h.query_by_label("Leave").is_none());
}
