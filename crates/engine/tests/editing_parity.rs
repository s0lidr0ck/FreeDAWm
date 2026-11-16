//! Regression tests for the editing bugs in `docs/daw-parity/04-editing-audio.md`.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{Clip, ClipId, Fade, FadeShape, Source, SourceId, TrackId};

fn source(id: u64, frames: u64) -> Source {
    Source {
        id: SourceId(id),
        name: "src".into(),
        path: "src.wav".into(),
        channels: 1,
        frames,
        sample_rate: 48_000,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    }
}

fn new_track(e: &mut Engine) -> TrackId {
    TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap())
}

/// A track of clips (start, length, file offset) on a long file, in `mode`.
fn track(e: &mut Engine, spans: &[(i64, i64, i64)]) -> (TrackId, Vec<ClipId>) {
    let t = new_track(e);
    let s = e.session_mut();
    if s.source(SourceId(8)).is_none() {
        s.sources.push(source(8, 1_000_000));
    }
    let ids: Vec<ClipId> = spans.iter().map(|_| s.new_clip_id()).collect();
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips =
        spans.iter().zip(&ids).map(|(&(start, len, offset), id)| Clip::audio(*id, "c", SourceId(8), offset, start, len)).collect();
    (t, ids)
}

fn engine(mode: &str) -> Engine {
    let mut e = Engine::default();
    e.execute("edit.mode", &json!({"mode": mode})).unwrap();
    e
}

fn clips(e: &Engine, t: TrackId) -> Vec<(i64, i64, i64)> {
    e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.length, c.source_offset())).collect()
}

fn run(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute(id, &p).unwrap()
}

// ---- bug 1: deleting two selected clips also deleted what lay between them ----------------

#[test]
fn deleting_selected_clips_leaves_the_clip_between_them() {
    for mode in ["slip", "shuffle"] {
        let mut e = engine(mode);
        let (t, c) = track(&mut e, &[(0, 1000, 0), (1000, 1000, 1000), (2000, 1000, 2000)]);
        let (u, d) = track(&mut e, &[(0, 3000, 5000)]);
        run(&mut e, "edit.select", json!({"clips": [c[0].0, c[2].0]}));
        assert_eq!(run(&mut e, "edit.clear", json!({}))["removed"], 2, "{mode}");
        let want = if mode == "slip" { vec![(1000, 1000, 1000)] } else { vec![(0, 1000, 1000)] };
        assert_eq!(clips(&e, t), want, "{mode}: B stays");
        assert_eq!(clips(&e, u), vec![(0, 3000, 5000)], "{mode}: other tracks are untouched");
        assert_eq!(e.session().find_clip(d[0]).map(|(tr, _)| tr), Some(u));
        // A second Delete has nothing left to remove.
        assert!(e.execute("edit.clear", &json!({})).map_or(true, |v| v["removed"] == 0), "{mode}");
        assert_eq!(clips(&e, t), want, "{mode}: B survives a second Delete");
    }
}

#[test]
fn cutting_and_copying_selected_clips_takes_only_those_clips() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 1000, 0), (1000, 1000, 1000), (2000, 1000, 2000)]);
    run(&mut e, "edit.select", json!({"clips": [c[0].0, c[2].0]}));
    assert_eq!(run(&mut e, "edit.copy", json!({}))["clips"], 2);
    assert_eq!(clips(&e, t).len(), 3, "Copy changes nothing");
    run(&mut e, "edit.select", json!({"clips": [c[0].0, c[2].0]}));
    assert_eq!(run(&mut e, "edit.cut", json!({}))["clips"], 2);
    assert_eq!(clips(&e, t), vec![(1000, 1000, 1000)]);
    // Pasting puts back the two clips and leaves the gap's material where it is.
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 0}));
    run(&mut e, "edit.paste", json!({}));
    assert_eq!(clips(&e, t), vec![(0, 1000, 0), (1000, 1000, 1000), (2000, 1000, 2000)]);
}

#[test]
fn a_range_passed_or_chosen_still_clears_the_range() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 1000, 0), (1000, 1000, 1000), (2000, 1000, 2000)]);
    run(&mut e, "edit.select", json!({"clips": [c[0].0, c[2].0]}));
    run(&mut e, "edit.clear", json!({"start": 500, "end": 2500}));
    assert_eq!(clips(&e, t), vec![(0, 500, 0), (2500, 500, 2500)]);
}

// ---- bug 2: placing a crossfaded pair cut the first clip off -------------------------------

fn ids(v: &[ClipId]) -> Vec<u64> {
    v.iter().map(|c| c.0).collect()
}

/// Two clips crossfaded over 1000..1200, and a long clip on a second track.
fn crossfaded(mode: &str) -> (Engine, TrackId, Vec<ClipId>) {
    let mut e = engine(mode);
    let (t, c) = track(&mut e, &[(0, 1200, 0), (1000, 1000, 5000)]);
    let s = e.session_mut();
    s.find_clip_mut(c[0]).unwrap().fade_out = Fade { len: 200, shape: FadeShape::EqualPower, curvature: 0.0 };
    s.find_clip_mut(c[1]).unwrap().fade_in = Fade { len: 200, shape: FadeShape::EqualPower, curvature: 0.0 };
    (e, t, c)
}

fn pair_at(e: &Engine, t: TrackId, at: i64) -> bool {
    let c = clips(e, t);
    c.contains(&(at, 1200, 0)) && c.contains(&(at + 1000, 1000, 5000))
}

#[test]
fn paste_duplicate_and_repeat_keep_a_crossfaded_pair_whole() {
    let (mut e, t, _) = crossfaded("slip");
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 0, "end": 2000}));
    run(&mut e, "edit.copy", json!({}));
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 10_000}));
    run(&mut e, "edit.paste", json!({}));
    assert!(pair_at(&e, t, 10_000), "paste: {:?}", clips(&e, t));

    let (mut e, t, _) = crossfaded("slip");
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 0, "end": 2000}));
    run(&mut e, "edit.duplicate", json!({}));
    assert!(pair_at(&e, t, 2000), "duplicate: {:?}", clips(&e, t));

    let (mut e, t, _) = crossfaded("slip");
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 0, "end": 2000}));
    run(&mut e, "edit.repeat", json!({"count": 2}));
    assert!(pair_at(&e, t, 2000) && pair_at(&e, t, 4000), "repeat: {:?}", clips(&e, t));
}

#[test]
fn copying_a_crossfaded_pair_to_another_track_keeps_it_whole() {
    let (mut e, t, c) = crossfaded("slip");
    let u = new_track(&mut e);
    run(&mut e, "clip.copy_to", json!({"clips": ids(&c), "track": u.0}));
    assert!(pair_at(&e, u, 0), "copy_to: {:?}", clips(&e, u));
    assert!(pair_at(&e, t, 0));
}

// ---- bug 3: trimming or splitting a stretched clip read the wrong part of the file ---------

/// A clip at twice its length (stretch 2): 2000 timeline samples play file samples 1000..2000.
fn stretched() -> (Engine, TrackId, ClipId) {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 2000, 1000)]);
    e.session_mut().find_clip_mut(c[0]).unwrap().stretch = 2.0;
    (e, t, c[0])
}

#[test]
fn splitting_a_stretched_clip_keeps_each_piece_on_its_audio() {
    let (mut e, t, _) = stretched();
    run(&mut e, "edit.separate", json!({"tracks": [t.0], "at": 1000}));
    // The right half starts 1000 timeline samples in: 500 file samples at stretch 2.
    assert_eq!(clips(&e, t), vec![(0, 1000, 1000), (1000, 1000, 1500)]);
}

#[test]
fn trimming_a_stretched_clip_keeps_the_rest_in_place() {
    let (mut e, t, id) = stretched();
    run(&mut e, "edit.trim_clip", json!({"clip": id.0, "start": 400}));
    assert_eq!(clips(&e, t), vec![(400, 1600, 1200)]);
    // Back out past the file's start (1000 file samples, 2000 on the timeline): stops at 0.
    run(&mut e, "edit.trim_clip", json!({"clip": id.0, "start": 0}));
    assert_eq!(clips(&e, t), vec![(0, 2000, 1000)]);
}

// ---- bug 4: an edit-locked clip could still be cut, cleared, separated, trimmed... ---------

/// Clip A (0..1000) edit-locked, clip B (1000..2000) not, on one track.
fn locked() -> (Engine, TrackId, Vec<ClipId>) {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 1000, 0), (1000, 1000, 1000)]);
    e.session_mut().find_clip_mut(c[0]).unwrap().edit_locked = true;
    (e, t, c)
}

#[test]
fn edits_that_would_change_an_edit_locked_clip_are_refused() {
    let range = json!({"start": 200, "end": 1500});
    let cases: Vec<(&str, Value)> = vec![
        ("edit.clear", range.clone()),
        ("edit.cut", range.clone()),
        ("edit.separate", json!({"at": 500})),
        ("edit.trim_to_selection", json!({"start": 200, "end": 800})),
        ("edit.trim_start_to_insertion", json!({"at": 300})),
        ("edit.trim_end_to_insertion", json!({"at": 700})),
        ("edit.fades_create", json!({"start": 0, "end": 300})),
        ("edit.insert_silence", json!({"start": 500, "end": 600})),
        ("edit.paste", json!({"at": 500})),
        ("clip.gain", json!({"db": -6.0})),
        ("edit.mute_clips", json!({})),
    ];
    for (id, p) in cases {
        let (mut e, t, c) = locked();
        run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 0, "end": 2000}));
        run(&mut e, "edit.copy", json!({}));
        run(&mut e, "edit.select", json!({"clips": [c[0].0, c[1].0]}));
        let before = clips(&e, t);
        let mut p = p;
        p["tracks"] = json!([t.0]);
        let r = e.execute(id, &p);
        assert!(r.as_ref().is_err_and(|err| err.to_string().contains("edit-locked")), "{id}: {r:?}");
        assert_eq!(clips(&e, t), before, "{id} leaves the track as it was");
        let a = e.session().find_clip(c[0]).unwrap().1;
        assert!(a.gain_db == 0.0 && !a.muted && a.fade_in.len == 0, "{id}");
    }
}

#[test]
fn a_locked_clip_can_be_renamed_unlocked_moved_by_insert_time_and_deleted_with_its_track() {
    let (mut e, t, c) = locked();
    run(&mut e, "clip.rename", json!({"clip": c[0].0, "name": "kept"}));
    run(&mut e, "event.insert_time", json!({"tracks": [t.0], "start": 0, "length": 500}));
    assert_eq!(clips(&e, t), vec![(500, 1000, 0), (1500, 1000, 1000)]);
    // The unlocked neighbour can be edited as before.
    run(&mut e, "edit.clear", json!({"tracks": [t.0], "start": 1500, "end": 2500}));
    assert_eq!(clips(&e, t), vec![(500, 1000, 0)]);
    run(&mut e, "clip.edit_lock", json!({"clips": [c[0].0]}));
    run(&mut e, "edit.separate", json!({"tracks": [t.0], "at": 1000}));
    assert_eq!(clips(&e, t).len(), 2, "unlocked, it splits");
    let (mut e, t, _) = locked();
    run(&mut e, "track.delete", json!({"tracks": [t.0]}));
    assert!(e.session().track(t).is_none());
}

// ---- bug 5: clip effects were stored by clip id, so a split's right half and copies lost them

fn effects(e: &Engine, t: TrackId) -> Vec<Option<f64>> {
    e.session().track(t).unwrap().clips().iter().map(|c| c.live_effects().and_then(|_| c.effect("gain"))).collect()
}

#[test]
fn a_split_and_copies_keep_the_clip_effects() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 2000, 0)]);
    run(&mut e, "clip.effects_set", json!({"clips": [c[0].0], "params": {"gain": -6.0}}));
    run(&mut e, "edit.separate", json!({"tracks": [t.0], "at": 1000}));
    assert_eq!(effects(&e, t), vec![Some(-6.0), Some(-6.0)], "both halves");
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 0, "end": 2000}));
    run(&mut e, "edit.duplicate", json!({}));
    assert_eq!(effects(&e, t), vec![Some(-6.0); 4], "the copies too");
    let u = new_track(&mut e);
    let first = e.session().track(t).unwrap().clips()[0].id;
    run(&mut e, "clip.copy_to", json!({"clips": [first.0], "track": u.0}));
    assert_eq!(effects(&e, u), vec![Some(-6.0)]);
    run(&mut e, "clip.effects_bypass", json!({"clips": [first.0], "value": true}));
    assert_eq!(effects(&e, t)[0], None, "bypassed: not heard");
}

#[test]
fn sessions_with_effects_kept_by_clip_id_load_them_onto_the_clips() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 2000, 0), (3000, 500, 0)]);
    let mut s = e.session().clone();
    s.edit.values.insert(format!("clip_fx.{}.gain", c[0].0), -3.0);
    s.edit.values.insert(format!("clip_fx.{}.eq.hpf_on", c[1].0), 1.0);
    s.edit.values.insert("clip_fx.999999.gain".into(), 1.0);
    s.edit.values.insert("other.value".into(), 2.0);
    s.edit.set_flag(&format!("clip_fx.bypass.{}", c[1].0), true);
    s.sanitize();
    let got: Vec<_> = s.track(t).unwrap().clips().iter().map(|c| (c.effects.clone(), c.effects_bypass)).collect();
    assert_eq!(got, vec![(vec![("gain".into(), -3.0)], false), (vec![("eq.hpf_on".into(), 1.0)], true)]);
    assert!(!s.edit.values.keys().chain(s.edit.flags.iter()).any(|k| k.starts_with("clip_fx.")), "the old keys are gone");
    assert_eq!(s.edit.values.get("other.value"), Some(&2.0));
}
