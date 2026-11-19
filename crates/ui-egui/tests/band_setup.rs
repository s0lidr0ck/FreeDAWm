//! Setting the band up: Track ▸ New Band, the Studio's empty state and musicians list, a
//! musician's controls in the Studio, and Musician Settings… from the Edit window.

use egui::{Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::Route;
use soundcraft_ui_egui::band::studio::{self, StudioTab};
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn texts(h: &Harness<SoundApp>) -> Vec<String> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) => Some(t.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

fn has_text(h: &Harness<SoundApp>, needle: &str) -> bool {
    texts(h).iter().any(|t| t.contains(needle))
}

fn blank() -> SoundApp {
    SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default())
}

#[test]
fn track_new_band_is_in_the_track_menu() {
    assert_eq!(menus::command_for_path("Track > New Band", &menus::ui_aliases()).as_deref(), Some("track.new_band"));
    assert!(menus::tree().iter().any(|n| n.label == "Track" && n.children.iter().any(|c| c.path == "Track > New Band")));
}

#[test]
fn the_empty_studio_says_what_to_do_and_new_band_does_it_in_one_undo_step() {
    let mut h = harness(blank());
    h.state_mut().run("window.studio", json!({"value": true})).unwrap();
    h.run_steps(4);
    assert!(has_text(&h, "No track plays the band yet."), "the empty state: {:?}", texts(&h));
    h.get_by_label("New Band").click();
    h.run_steps(6);
    let s = h.state().engine.session_arc();
    assert_eq!(s.tracks.len(), 5, "a track per musician who is on");
    assert!(s.tracks.iter().enumerate().all(|(k, t)| t.mixer.input == Route::Musician(k as u8)));
    assert!(!s.band.chords.is_empty(), "a song is laid on the empty lanes");
    let st = &h.state().studio;
    assert!(st.show && st.tab == StudioTab::Musicians);
    assert!(st.message.as_deref().is_some_and(|m| m.starts_with("New Band: 5 track(s)")), "{:?}", st.message);
    assert!(h.state().ui.status.contains("Press Play"), "{}", h.state().ui.status);
    // The musicians list replaces the empty state: each one's track and instrument.
    assert!(!has_text(&h, "No track plays the band yet."));
    assert!(has_text(&h, "MUSICIANS"));
    assert!(has_text(&h, "Juke — Drum Machine"), "the drummer's instrument is shown: {:?}", texts(&h));
    assert!(has_text(&h, "Round Bass"), "with its preset");
    assert!(has_text(&h, "Off: not heard"), "the sixth musician is off");
    // One undo step.
    assert_eq!(h.state().engine.undo_label(), Some("New Band"));
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(4);
    assert!(h.state().engine.session().tracks.is_empty());
    assert!(has_text(&h, "No track plays the band yet."));
}

#[test]
fn a_musicians_settings_open_in_the_studio_and_fold_away() {
    let mut h = harness(blank());
    h.state_mut().run("track.new_band", json!({"installed": []})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Fold the band drawer").is_none());
    h.get_by_label("Musician 2 settings").click();
    h.run_steps(4);
    assert_eq!(h.state().studio.musician, Some(1));
    assert!(has_text(&h, "MUSICIAN 2"), "the drummer's drawer is at the window's right");
    // An edit there is a band edit like the Mix drawer's.
    h.get_by_label("Fold the band drawer").click();
    h.run_steps(4);
    assert_eq!(h.state().studio.musician, None);
    assert!(h.state().ui.band_drawers.is_empty(), "the Mix window's drawers are untouched");
    // From the Edit window: Musician Settings… is window.studio with the musician.
    h.state_mut().run("window.studio", json!({"value": false})).unwrap();
    h.state_mut().run("window.studio", json!({"value": true, "musician": 3})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().studio.musician, Some(2));
    assert!(has_text(&h, "MUSICIAN 3"));
    assert!(h.state_mut().run("window.studio", json!({"musician": 7})).is_err());
}

#[test]
fn the_list_says_who_is_not_heard_and_why() {
    let mut app = blank();
    app.run("track.new", json!({"kind": "instrument", "name": "Beats"})).unwrap();
    app.run("track.input", json!({"track": "Beats", "input": "musician:2"})).unwrap();
    let mut h = harness(app);
    h.state_mut().run("window.studio", json!({"value": true})).unwrap();
    h.run_steps(4);
    let s = h.state().engine.session_arc();
    let lib = h.state().engine.band_library();
    let (status, problem) = studio::musician_status(&s, &lib, 1);
    assert!(problem && status.starts_with("Drums on a pitched instrument"), "{status}");
    let (status, problem) = studio::musician_status(&s, &lib, 0);
    assert!(problem && status.starts_with("No track plays them"), "{status}");
    assert!(has_text(&h, "4 musician(s) who are on have no track"), "{:?}", texts(&h));
    // Add their tracks: the four, the drummer's own track kept.
    h.get_by_label("Add their tracks").click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().tracks.len(), 5);
    assert!(!has_text(&h, "who are on have no track"));
}
