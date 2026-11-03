//! Chords off the half-bar grid (docs/superpowers/specs/2026-10-26-key-ruler-midi-zoom.md, 1b):
//! `band.chord.add` / `band.chord.set` with `snap: false` keep the exact tick (an Alt-drag on the
//! Chords ruler, a chord on a sung downbeat); without it they snap to half bars as before.

use serde_json::{Value, json};
use soundcraft_engine::Engine;

/// A half bar of 4/4, in ticks.
const HALF: i64 = 1920;

fn ok(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute(id, &p).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn samples(e: &Engine, tick: i64) -> i64 {
    let s = e.session();
    s.tempo.tick_to_samples(tick, s.sample_rate)
}

fn spans(e: &Engine) -> Vec<(i64, i64)> {
    e.session().band.chords.iter().map(|c| (c.tick, c.length_ticks)).collect()
}

#[test]
fn snap_false_keeps_the_exact_ticks() {
    let mut e = Engine::default();
    let (a, b) = (samples(&e, 700), samples(&e, 2 * HALF + 300));
    // Snapped by default.
    let id = ok(&mut e, "band.chord.add", json!({"at": a, "end": b, "chord": "C"}))["chord"].as_u64().unwrap();
    assert_eq!(spans(&e), [(0, 2 * HALF)]);
    ok(&mut e, "edit.undo", json!({}));
    let id2 = ok(&mut e, "band.chord.add", json!({"at": a, "end": b, "chord": "C", "snap": false}))["chord"].as_u64().unwrap();
    assert_eq!(spans(&e), [(700, 2 * HALF + 300 - 700)]);
    let _ = id;
    let (t1000, t5000) = (samples(&e, 1000), samples(&e, 5000));
    // A move keeps its length; a resize moves one end; both exact.
    ok(&mut e, "band.chord.set", json!({"chord": id2, "at": t1000, "snap": false}));
    assert_eq!(spans(&e), [(1000, 2 * HALF + 300 - 700)]);
    ok(&mut e, "band.chord.set", json!({"chord": id2, "at": t1000, "end": t5000, "snap": false}));
    assert_eq!(spans(&e), [(1000, 4000)]);
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(spans(&e), [(1000, 2 * HALF + 300 - 700)], "one undo step");
    // Snapped again without `snap: false`.
    ok(&mut e, "band.chord.set", json!({"chord": id2, "at": t1000}));
    assert_eq!(spans(&e)[0].0, HALF, "1000 ticks is nearest the first half bar");
    assert!(e.execute("band.chord.set", &json!({"chord": id2, "at": 0, "snap": "no"})).is_err());
}
