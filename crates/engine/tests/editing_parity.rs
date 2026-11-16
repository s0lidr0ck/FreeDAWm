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
