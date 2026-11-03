//! Deleting markers and Band chords from the rulers (docs/superpowers/specs/
//! 2026-10-25-presets-markers-templates.md, part 2): one, those in a range, or all of them, each
//! one undo step; a request that matches nothing is refused and changes nothing.

use serde_json::{Value, json};
use soundcraft_engine::Engine;

fn ok(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute(id, &p).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn bar(e: &Engine) -> i64 {
    let s = e.session();
    s.tempo.tick_to_samples(3840, s.sample_rate)
}

fn names(e: &Engine) -> Vec<String> {
    let mut v: Vec<String> = e.session().markers.iter().map(|m| m.name.clone()).collect();
    v.sort();
    v
}

/// Markers A, B, C on ruler 1 at bars 1, 3 and 5, and D on ruler 2 at bar 3.
fn with_markers() -> Engine {
    let mut e = Engine::default();
    let b = bar(&e);
    for (name, at, ruler) in [("A", 0, 1), ("B", 2 * b, 1), ("C", 4 * b, 1), ("D", 2 * b, 2)] {
        ok(&mut e, "markers.add", json!({"name": name, "at": at, "ruler": ruler}));
    }
    e
}

#[test]
fn markers_delete_one_by_id_as_before() {
    let mut e = with_markers();
    let id = e.session().markers.iter().find(|m| m.name == "B").unwrap().id.0;
    assert_eq!(ok(&mut e, "markers.delete", json!({"id": id}))["deleted"], 1);
    assert_eq!(names(&e), ["A", "C", "D"]);
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(names(&e), ["A", "B", "C", "D"]);
}

#[test]
fn markers_delete_in_a_range_on_one_ruler() {
    let mut e = with_markers();
    let b = bar(&e);
    let r = ok(&mut e, "markers.delete", json!({"range": {"start": b, "end": 5 * b}, "ruler": 1}));
    assert_eq!(r["deleted"], 2);
    assert_eq!(names(&e), ["A", "D"], "B and C start inside; D is on ruler 2");
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(names(&e), ["A", "B", "C", "D"], "one undo step");
    // Without a ruler, every ruler; the end is not inside.
    let r = ok(&mut e, "markers.delete", json!({"range": {"start": b, "end": 4 * b}}));
    assert_eq!(r["deleted"], 2);
    assert_eq!(names(&e), ["A", "C"]);
}

#[test]
fn markers_delete_all_on_a_ruler_or_everywhere() {
    let mut e = with_markers();
    assert_eq!(ok(&mut e, "markers.delete", json!({"all": true, "ruler": 2}))["deleted"], 1);
    assert_eq!(names(&e), ["A", "B", "C"]);
    assert_eq!(ok(&mut e, "markers.delete", json!({"all": true}))["deleted"], 3);
    assert!(e.session().markers.is_empty());
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(names(&e), ["A", "B", "C"]);
    let ids: Vec<u64> = e.session().markers.iter().filter(|m| m.name != "B").map(|m| m.id.0).collect();
    assert_eq!(ok(&mut e, "markers.delete", json!({"ids": ids}))["deleted"], 2);
    assert_eq!(names(&e), ["B"]);
}

#[test]
fn markers_delete_refuses_bad_or_empty_requests() {
    let mut e = with_markers();
    let undo = e.undo_history().len();
    for p in [
        json!({"all": true, "ruler": 4}),
        json!({"range": {"start": 999_999_999, "end": 9_999_999_999_i64}}),
        json!({"all": true, "ruler": 9}),
        json!({"all": "yes"}),
        json!({"ids": "x"}),
        json!({"range": {"start": 0}}),
        json!({"ids": []}),
    ] {
        assert!(e.execute("markers.delete", &p).is_err(), "{p}");
    }
    assert_eq!(names(&e), ["A", "B", "C", "D"]);
    assert_eq!(e.undo_history().len(), undo);
}

/// Chords at bars 1, 2, 3 and 4 (one bar each).
fn with_chords() -> Engine {
    let mut e = Engine::default();
    let b = bar(&e);
    for (k, c) in ["C", "Am", "F", "G"].iter().enumerate() {
        let at = b * k as i64;
        ok(&mut e, "band.chord.add", json!({"at": at, "end": at + b, "chord": c}));
    }
    assert_eq!(e.session().band.chords.len(), 4);
    e
}

fn chord_ticks(e: &Engine) -> Vec<i64> {
    let mut v: Vec<i64> = e.session().band.chords.iter().map(|c| c.tick).collect();
    v.sort();
    v
}

#[test]
fn chords_delete_listed_in_range_and_all() {
    let mut e = with_chords();
    let b = bar(&e);
    let first = e.session().band.chords.iter().find(|c| c.tick == 0).unwrap().id;
    assert_eq!(ok(&mut e, "band.chords.delete", json!({"chords": [first]}))["deleted"], 1);
    assert_eq!(chord_ticks(&e), [3840, 7680, 11520]);
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(chord_ticks(&e).len(), 4);

    // In a selection: chords that start inside it.
    let r = ok(&mut e, "band.chords.delete", json!({"range": {"start": b / 2, "end": 3 * b}}));
    assert_eq!(r["deleted"], 2);
    assert_eq!(chord_ticks(&e), [0, 11520]);
    ok(&mut e, "edit.undo", json!({}));

    assert_eq!(ok(&mut e, "band.chords.delete", json!({"all": true}))["deleted"], 4);
    assert!(e.session().band.chords.is_empty());
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(chord_ticks(&e).len(), 4, "Clear All is one undo step");
}

#[test]
fn chords_delete_refuses_bad_or_empty_requests() {
    let mut e = with_chords();
    let before = e.session().band.clone();
    let undo = e.undo_history().len();
    for p in [
        json!({}),
        json!({"chords": [999]}),
        json!({"chords": "x"}),
        json!({"chords": [-1]}),
        json!({"all": 1}),
        json!({"range": {"end": 10}}),
        json!({"range": {"start": 100 * 48_000, "end": 200 * 48_000}}),
    ] {
        assert!(e.execute("band.chords.delete", &p).is_err(), "{p}");
    }
    assert_eq!(e.session().band, before);
    assert_eq!(e.undo_history().len(), undo);
    let mut empty = Engine::default();
    assert!(empty.execute("band.chords.delete", &json!({"all": true})).is_err());
}
