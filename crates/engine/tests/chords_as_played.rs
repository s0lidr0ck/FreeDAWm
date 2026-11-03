//! Detect Chords "As played" (docs/superpowers/specs/2026-10-26-key-ruler-midi-zoom.md, 1c):
//! `Grid::Free` keeps each detected change where the detector heard it, merges changes closer
//! than 1/8 bar, and runs each chord to the next change. The chords are ordinary lane chords at
//! off-grid ticks, which the band's chord commands still edit.

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::cmd::chord_lane::{Grid, KeyChoice, LaneOptions, TimedChord, place_chords};
use soundcraft_model::Session;

/// 120 bpm 4/4 at 48 kHz: a bar is 2 s (3840 ticks), an eighth of a bar 0.25 s (480 ticks).
fn session() -> Session {
    let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
    s.tempo.set_tempo(0, 120.0).unwrap();
    s
}

fn tc(a: f64, b: f64, n: &str) -> TimedChord {
    TimedChord { start: a, end: b, name: n.into() }
}

/// Seconds to ticks at 120 bpm.
fn t(secs: f64) -> i64 {
    (secs * 1920.0).round() as i64
}

fn lane(s: &Session) -> Vec<(i64, i64)> {
    let mut v: Vec<(i64, i64)> = s.band.chords.iter().map(|c| (c.tick, c.length_ticks)).collect();
    v.sort();
    v
}

#[test]
fn grid_free_parses_and_names_itself() {
    assert_eq!(Grid::parse("free"), Some(Grid::Free));
    assert_eq!(Grid::parse("as played"), Some(Grid::Free));
    assert_eq!(Grid::Free.id(), "free");
}

#[test]
fn as_played_keeps_off_grid_changes_and_merges_close_ones() {
    let mut s = session();
    let chords = [
        tc(0.10, 1.30, "Am"),
        // A gap (no chord heard): Am runs on to the next change.
        tc(1.45, 2.90, "F"),
        // 0.15 s of G, under 1/8 bar (0.25 s): merged into what follows.
        tc(2.90, 3.05, "G"),
        tc(3.05, 4.70, "C"),
        tc(4.70, 6.00, "C"),
    ];
    let opts = LaneOptions { grid: Grid::Free, replace: true, key: KeyChoice::Keep };
    let v = place_chords(&mut s, 0, 48_000 * 6, &chords, &opts).unwrap();
    assert_eq!(v["grid"], "free");
    assert_eq!(lane(&s), [(t(0.10), t(1.45) - t(0.10)), (t(1.45), t(2.90) - t(1.45)), (t(2.90), t(6.0) - t(2.90))], "{v}");
    assert!(s.band.chords.iter().any(|c| c.tick % 1920 != 0), "off the half-bar grid");
}

#[test]
fn half_bars_still_snap() {
    let mut s = session();
    let chords = [tc(0.10, 1.30, "Am"), tc(1.45, 4.0, "F")];
    place_chords(&mut s, 0, 48_000 * 4, &chords, &LaneOptions { grid: Grid::HalfBar, replace: true, key: KeyChoice::Keep }).unwrap();
    assert!(s.band.chords.iter().all(|c| c.tick % 1920 == 0 && c.length_ticks % 1920 == 0));
}

#[test]
fn off_grid_chords_take_the_chord_commands() {
    let mut s = session();
    let chords = [tc(0.10, 1.30, "Am"), tc(1.45, 4.0, "F")];
    place_chords(&mut s, 0, 48_000 * 4, &chords, &LaneOptions { grid: Grid::Free, replace: true, key: KeyChoice::Keep }).unwrap();
    let mut e = Engine::new(s);
    let am = e.session().band.chords.iter().min_by_key(|c| c.tick).unwrap().id;
    e.execute("band.chord.set", &json!({"chord": am, "chord_text": "Dm"})).unwrap();
    let c = e.session().band.chords.iter().find(|c| c.id == am).unwrap().clone();
    assert_eq!((c.tick, c.length_ticks), (t(0.10), t(1.45) - t(0.10)), "renaming keeps its off-grid span");
    e.execute("band.chord.remove", &json!({"chord": am})).unwrap();
    assert_eq!(e.session().band.chords.len(), 1);
    e.execute("band.chords.delete", &json!({"all": true})).unwrap();
    assert!(e.session().band.chords.is_empty());
}
