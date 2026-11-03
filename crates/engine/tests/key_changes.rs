//! Key changes on the Key ruler (docs/superpowers/specs/2026-10-26-key-ruler-midi-zoom.md,
//! part 1): add (validated by `vocal::parse_key`), edit (rename and move), delete (one, a range,
//! all). Each is one undo step; a refused request changes nothing.

use serde_json::{Value, json};
use soundcraft_engine::Engine;

fn ok(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute(id, &p).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn keys(e: &Engine) -> Vec<(i64, String)> {
    e.session().key_signatures.clone()
}

fn with_keys() -> Engine {
    let mut e = Engine::default();
    for (at, k) in [(0, "C major"), (48_000, "A minor"), (96_000, "Eb major")] {
        ok(&mut e, "event.add_key_change", json!({"at": at, "key": k}));
    }
    e
}

#[test]
fn add_refuses_an_unparsable_key() {
    let mut e = Engine::default();
    assert!(e.execute("event.add_key_change", &json!({"at": 0, "key": "Q blorp"})).is_err());
    assert!(e.execute("event.add_key_change", &json!({"at": 0, "key": "C dorian"})).is_err());
    assert!(e.execute("event.add_key_change", &json!({"at": 0, "key": 5})).is_err());
    assert!(keys(&e).is_empty());
    ok(&mut e, "event.add_key_change", json!({"at": 100, "key": " F# minor "}));
    assert_eq!(keys(&e), [(100, "F# minor".to_string())], "stored trimmed, as spelled");
    ok(&mut e, "edit.undo", json!({}));
    assert!(keys(&e).is_empty(), "one undo step");
}

#[test]
fn edit_renames_and_moves_one_key_change() {
    let mut e = with_keys();
    ok(&mut e, "event.key_change_edit", json!({"at": 48_000, "key": "D minor"}));
    assert_eq!(keys(&e)[1], (48_000, "D minor".to_string()));
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(keys(&e)[1], (48_000, "A minor".to_string()));
    // Move, and rename at once; the list stays in time order.
    let v = ok(&mut e, "event.key_change_edit", json!({"at": 48_000, "to": 120_000, "key": "G major"}));
    assert_eq!(v["at"], 120_000);
    assert_eq!(keys(&e), [(0, "C major".into()), (96_000, "Eb major".into()), (120_000, "G major".into())]);
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(keys(&e), with_keys().session().key_signatures);
    // Moving onto another key change is refused: nothing is lost.
    let err = e.execute("event.key_change_edit", &json!({"at": 0, "to": 96_000})).unwrap_err();
    assert!(err.to_string().contains("A key change is already there"), "{err}");
    assert_eq!(keys(&e), with_keys().session().key_signatures);
}

/// An older session can hold two key changes at one tick: edit and delete `{at}` act on one (the
/// last), never both.
#[test]
fn duplicate_ticks_are_edited_and_deleted_one_at_a_time() {
    let mut e = Engine::default();
    e.session_mut().key_signatures = vec![(0, "C major".into()), (0, "D major".into()), (100, "E major".into())];
    ok(&mut e, "event.key_change_edit", json!({"at": 0, "key": "G major"}));
    assert_eq!(keys(&e), [(0, "C major".into()), (0, "G major".into()), (100, "E major".into())]);
    ok(&mut e, "event.key_change_edit", json!({"at": 0, "to": 50}));
    assert_eq!(keys(&e), [(0, "C major".into()), (50, "G major".into()), (100, "E major".into())]);
    ok(&mut e, "edit.undo", json!({}));
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(ok(&mut e, "event.key_change_delete", json!({"at": 0}))["deleted"], 1);
    assert_eq!(keys(&e), [(0, "C major".into()), (100, "E major".into())]);
}

#[test]
fn edit_refuses_bad_requests() {
    let mut e = with_keys();
    let before = keys(&e);
    for p in [
        json!({"at": 1, "key": "D minor"}),       // no key change there
        json!({"at": 48_000, "key": "nonsense"}), // unparsable
        json!({"at": 48_000}),                    // nothing to change
        json!({"key": "D minor"}),                // no `at`
        json!({"at": 48_000, "to": -5}),          // before the start
    ] {
        assert!(e.execute("event.key_change_edit", &p).is_err(), "{p}");
    }
    assert_eq!(keys(&e), before);
}

#[test]
fn delete_one_a_range_or_all() {
    let mut e = with_keys();
    assert_eq!(ok(&mut e, "event.key_change_delete", json!({"at": 48_000}))["deleted"], 1);
    assert_eq!(keys(&e).len(), 2);
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(ok(&mut e, "event.key_change_delete", json!({"range": {"start": 1, "end": 96_001}}))["deleted"], 2);
    assert_eq!(keys(&e), [(0, "C major".to_string())]);
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(ok(&mut e, "event.key_change_delete", json!({"all": true}))["deleted"], 3);
    assert!(keys(&e).is_empty());
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(keys(&e).len(), 3, "one undo step");
    // Matching nothing, or asking for nothing, is refused.
    assert!(e.execute("event.key_change_delete", &json!({"at": 5})).is_err());
    assert!(e.execute("event.key_change_delete", &json!({"range": {"start": 1, "end": 2}})).is_err());
    assert!(e.execute("event.key_change_delete", &json!({})).is_err());
    assert!(e.execute("event.key_change_delete", &json!({"all": "yes"})).is_err());
    assert_eq!(keys(&e).len(), 3);
}
