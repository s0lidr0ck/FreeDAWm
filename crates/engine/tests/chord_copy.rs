//! Copying Band chords: `band.chords.copy` / `paste`, `copy_to` (a copy-drag) and `duplicate`.
//! Each edit is one undo step; a copy keeps the chord's slot, bass and length and the gaps between
//! several; copies cut what they land on as any chord laid on the lane does; bad requests are
//! refused and change nothing.

use serde_json::{Value, json};
use soundcraft_engine::Engine;

const BAR: i64 = 3840;

fn ok(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute(id, &p).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn at(tick: i64) -> i64 {
    let e = Engine::default();
    let s = e.session();
    s.tempo.tick_to_samples(tick, s.sample_rate)
}

/// (tick, length, slot, bass) of every chord, in time order.
fn lane(e: &Engine) -> Vec<(i64, i64, u8, i8)> {
    let mut v: Vec<_> = e.session().band.chords.iter().map(|c| (c.tick, c.length_ticks, c.slot, c.bass)).collect();
    v.sort();
    v
}

/// C (slot 0) over bar 1, a G/B (slot 4, bass 11) over the first half of bar 2, a gap, then
/// slot 9 over bar 3.
fn with_chords() -> (Engine, [u32; 3]) {
    let mut e = Engine::default();
    let mut ids = [0; 3];
    for (i, (a, b, slot, bass)) in [(0, BAR, 0, -1), (BAR, BAR + BAR / 2, 4, 11), (2 * BAR, 3 * BAR, 9, -1)].into_iter().enumerate() {
        let r = ok(&mut e, "band.chord.add", json!({"at": at(a), "end": at(b), "chord": slot, "bass": bass}));
        ids[i] = r["chord"].as_u64().unwrap() as u32;
    }
    (e, ids)
}

#[test]
fn copy_and_paste_one_chord_keeps_it_whole_at_the_half_bar() {
    let (mut e, [_, g, _]) = with_chords();
    assert_eq!(ok(&mut e, "band.chords.copy", json!({"chords": [g]}))["copied"], 1);
    let steps = e.undo_history().len();
    // A little past bar 5: snapped back to it.
    let r = ok(&mut e, "band.chords.paste", json!({"at": at(4 * BAR + 100)}));
    assert_eq!(r["chords"].as_array().unwrap().len(), 1);
    assert_eq!(e.undo_history().len(), steps + 1, "one undo step");
    assert!(lane(&e).contains(&(4 * BAR, BAR / 2, 4, 11)), "{:?}", lane(&e));
    // Pasting again makes another copy; the clipboard stays.
    ok(&mut e, "band.chords.paste", json!({"at": at(6 * BAR)}));
    assert!(lane(&e).contains(&(6 * BAR, BAR / 2, 4, 11)));
    ok(&mut e, "edit.undo", json!({}));
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(lane(&e).len(), 3);
}

#[test]
fn a_run_pastes_with_its_gaps_and_cuts_what_it_lands_on() {
    let (mut e, ids) = with_chords();
    ok(&mut e, "band.chords.copy", json!({"chords": ids}));
    // Onto bar 2: the C copy covers the G/B (it goes), the G/B copy lands on bar 3's chord (its
    // second half stays) and the slot-9 copy fills bar 4.
    ok(&mut e, "band.chords.paste", json!({"at": at(BAR)}));
    assert_eq!(lane(&e), [(0, BAR, 0, -1), (BAR, BAR, 0, -1), (2 * BAR, BAR / 2, 4, 11), (2 * BAR + BAR / 2, BAR / 2, 9, -1), (3 * BAR, BAR, 9, -1)]);
}

#[test]
fn paste_off_the_grid_keeps_the_tick() {
    let (mut e, [c, _, _]) = with_chords();
    ok(&mut e, "band.chords.copy", json!({"chords": [c]}));
    ok(&mut e, "band.chords.paste", json!({"at": at(5 * BAR + 240), "snap": false}));
    assert!(lane(&e).contains(&(5 * BAR + 240, BAR, 0, -1)), "{:?}", lane(&e));
}

#[test]
fn copy_to_leaves_the_original_in_one_step() {
    let (mut e, [_, _, last]) = with_chords();
    let steps = e.undo_history().len();
    ok(&mut e, "band.chords.copy_to", json!({"chords": [last], "to": at(6 * BAR)}));
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(lane(&e).contains(&(2 * BAR, BAR, 9, -1)), "the original stays");
    assert!(lane(&e).contains(&(6 * BAR, BAR, 9, -1)), "the copy");
    ok(&mut e, "edit.undo", json!({}));
    assert_eq!(lane(&e).len(), 3);
}

#[test]
fn duplicate_goes_straight_after_and_repeats() {
    let (mut e, [_, g, _]) = with_chords();
    let r = ok(&mut e, "band.chords.duplicate", json!({"chords": [g]}));
    let copy = r["chords"][0].as_u64().unwrap();
    // Straight after the G/B: the second half of bar 2 (the gap there).
    assert!(lane(&e).contains(&(BAR + BAR / 2, BAR / 2, 4, 11)), "{:?}", lane(&e));
    // Again from the copy: it lands on bar 3's chord, cutting its first half.
    ok(&mut e, "band.chords.duplicate", json!({"chords": [copy]}));
    assert!(lane(&e).contains(&(2 * BAR, BAR / 2, 4, 11)));
    assert!(lane(&e).contains(&(2 * BAR + BAR / 2, BAR / 2, 9, -1)));
}

#[test]
fn duplicate_a_run_after_its_last_chord() {
    let (mut e, ids) = with_chords();
    ok(&mut e, "band.chords.duplicate", json!({"chords": [ids[2], ids[0]]}));
    // The run spans bars 1-3: its copy starts at bar 4.
    let l = lane(&e);
    assert!(l.contains(&(3 * BAR, BAR, 0, -1)) && l.contains(&(5 * BAR, BAR, 9, -1)), "{l:?}");
    assert_eq!(l.len(), 5, "only the chords named are copied: {l:?}");
}

#[test]
fn any_other_copy_replaces_the_copied_chords() {
    let (mut e, [c, _, _]) = with_chords();
    ok(&mut e, "band.chords.copy", json!({"chords": [c]}));
    assert_eq!(e.clipboard.chords.len(), 1);
    // A clip copy takes the clipboard.
    let t = ok(&mut e, "track.new", json!({"kind": "instrument", "name": "Keys"}))["tracks"][0].as_u64().unwrap();
    ok(&mut e, "midi.new_clip", json!({"track": t, "start": 0, "length": at(BAR)}));
    ok(&mut e, "edit.copy", json!({"tracks": [t], "start": 0, "end": at(BAR)}));
    assert!(e.clipboard.chords.is_empty());
    assert!(!e.clipboard.tracks.is_empty(), "the clip is what Paste pastes");
    assert!(e.execute("band.chords.paste", &json!({"at": 0})).is_err(), "no chords copied any more");
    // And a chord copy takes it back.
    ok(&mut e, "band.chords.copy", json!({"chords": [c]}));
    assert!(e.clipboard.tracks.is_empty() && e.clipboard.chords.len() == 1);
}

#[test]
fn bad_requests_are_refused_and_change_nothing() {
    let (mut e, ids) = with_chords();
    let before = lane(&e);
    for (id, p) in [
        ("band.chords.paste", json!({"at": 0})),
        ("band.chords.copy", json!({})),
        ("band.chords.copy", json!({"chords": []})),
        ("band.chords.copy", json!({"chords": [9999]})),
        ("band.chords.copy", json!({"chords": ["x", -1]})),
        ("band.chords.copy", json!({"chords": 3})),
        ("band.chords.copy_to", json!({"chords": [ids[0]]})),
        ("band.chords.copy_to", json!({"chords": [ids[0]], "to": "nowhere"})),
        ("band.chords.copy_to", json!({"chords": [ids[0]], "to": 0, "snap": "yes"})),
        ("band.chords.duplicate", json!({"chords": [u64::MAX]})),
        ("band.chords.duplicate", json!(null)),
    ] {
        assert!(e.execute(id, &p).is_err(), "{id} {p} should be refused");
    }
    ok(&mut e, "band.chords.copy", json!({"chords": [ids[0]]}));
    for p in [json!({}), json!({"at": [1, 2]}), json!({"at": 0, "snap": 1})] {
        assert!(e.execute("band.chords.paste", &p).is_err(), "{p} should be refused");
    }
    assert_eq!(lane(&e), before, "nothing changed");
    // Far-out positions never crash.
    let _ = e.execute("band.chords.paste", &json!({"at": i64::MAX}));
    let _ = e.execute("band.chords.copy_to", &json!({"chords": [ids[0]], "to": i64::MIN}));
    let _ = e.execute("band.chords.duplicate", &json!({"chords": ids}));
}
