//! `track.new_band`: from an empty session to a band that is heard, in one undo step.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{Route, Session, TrackId};
use soundcraft_time::Range;

/// The mix of the first `secs` seconds with only track `only` unmuted.
fn render_alone(e: &Engine, only: TrackId, secs: i64) -> Vec<f32> {
    let mut s: Session = e.session().clone();
    for t in &mut s.tracks {
        t.mixer.mute = t.id != only;
    }
    let s = std::sync::Arc::new(s);
    let mut rr = soundcraft_mix::RangeRender::new(&s, None, Range::new(0, secs * 48_000), 1024);
    assert!(rr.adopt_band_library(&s, &e.band_library()));
    let mut out = Vec::new();
    while let Some((chunk, n)) = rr.next_block(&s) {
        if let Some(c) = chunk.first() {
            out.extend_from_slice(&c[..n]);
        }
    }
    out
}

#[test]
fn a_new_band_is_heard_with_no_plugins_installed_and_undoes_in_one_step() {
    let mut e = Engine::default();
    let v = e.execute("track.new_band", &json!({"installed": []})).unwrap();
    assert_eq!(v["song"], "Pop Verse Chorus Bridge", "the empty lanes get a song: {v}");
    let s = e.session_arc();
    // The Factory Band: five musicians on, the sixth off.
    assert_eq!(s.tracks.len(), 5);
    let names: Vec<&str> = s.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Piano", "Drums", "Bass", "Strings", "Brass"]);
    for (k, t) in s.tracks.iter().enumerate() {
        assert_eq!(t.mixer.input, Route::Musician(k as u8), "{}", t.name);
    }
    let plugin = |n: &str| s.track_by_name(n).unwrap().instrument.as_ref().unwrap().plugin.clone();
    assert_eq!(plugin("Drums"), "juke", "the drummer plays a drum machine, not a synth");
    assert_eq!(plugin("Bass"), "subtractive_synth");
    assert_eq!(s.track_by_name("Bass").unwrap().instrument.as_ref().unwrap().preset, "Round Bass");
    assert!(!s.band.sections.is_empty() && !s.band.chords.is_empty());
    assert!(s.edit.rulers.iter().any(|r| r == "sections") && s.edit.rulers.iter().any(|r| r == "band_chords"));
    assert!(s.edit.rulers.iter().any(|r| r == "band"), "the Band ruler shows: {:?}", s.edit.rulers);
    // Every musician is heard on their own.
    for t in &s.tracks {
        let a = render_alone(&e, t.id, 4);
        let peak = a.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 1e-3, "{} is silent (peak {peak})", t.name);
    }
    // One undo step takes it all back.
    assert_eq!(e.undo_label(), Some("New Band"));
    assert!(e.undo());
    let s = e.session();
    assert!(s.tracks.is_empty() && s.band.sections.is_empty() && s.band.chords.is_empty());
    assert!(e.redo());
    assert_eq!(e.session().tracks.len(), 5);
    // Again: everyone who is on has a track.
    let err = e.execute("track.new_band", &json!({"installed": []})).unwrap_err().to_string();
    assert!(err.contains("already has a track"), "{err}");
}

#[test]
fn installed_targets_are_used_and_existing_musician_tracks_are_kept() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "name": "My Keys"})).unwrap();
    e.execute("track.input", &json!({"track": "My Keys", "input": "musician:1"})).unwrap();
    e.execute("band.song.factory", &json!({"name": "12-bar Blues"})).unwrap();
    let installed = json!([{"id": "vst3:modo-drum", "name": "MODO DRUM"}, {"id": "clap:other", "name": "Serum"}]);
    let v = e.execute("track.new_band", &json!({"installed": installed})).unwrap();
    assert!(v["song"].is_null(), "lanes already hold a song: {v}");
    let s = e.session();
    assert_eq!(s.tracks.len(), 5, "the piano already had a track");
    assert_eq!(s.tracks.iter().filter(|t| t.mixer.input == Route::Musician(0)).count(), 1);
    let drums = s.track_by_name("Drums").unwrap();
    assert_eq!(drums.instrument.as_ref().unwrap().plugin, "vst3:modo-drum");
    let m = v["musicians"].as_array().unwrap();
    assert!(m.iter().any(|x| x["instrument"] == "Drums" && x["installed"] == true));
    assert!(m.iter().all(|x| x["instrument"] == "Drums" || x["installed"] == false));
    // A named song replaces the lanes; an unknown one changes nothing.
    let before = e.session().clone();
    let mut e2 = Engine::default();
    let err = e2.execute("track.new_band", &json!({"installed": [], "song": "Polka"})).unwrap_err().to_string();
    assert!(err.contains("no factory song"), "{err}");
    assert!(e2.session().tracks.is_empty());
    assert_eq!(e.session().tracks.len(), before.tracks.len());
}
