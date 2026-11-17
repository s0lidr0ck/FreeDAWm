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

// ---- bug 6: Heal joined pieces whose settings differed and dropped the second piece's -------

/// One clip split at 1000 into two pieces, the second changed by `change`.
fn split_pair(change: impl Fn(&mut Clip)) -> (Engine, TrackId) {
    let mut e = engine("slip");
    let (t, _) = track(&mut e, &[(0, 2000, 0)]);
    run(&mut e, "edit.separate", json!({"tracks": [t.0], "at": 1000}));
    let right = e.session().track(t).unwrap().clips()[1].id;
    change(e.session_mut().find_clip_mut(right).unwrap());
    (e, t)
}

#[test]
fn heal_leaves_pieces_that_sound_different_apart() {
    let changes: Vec<(&str, Box<dyn Fn(&mut Clip)>)> = vec![
        ("mute", Box::new(|c: &mut Clip| c.muted = true)),
        ("stretch", Box::new(|c: &mut Clip| c.stretch = 2.0)),
        ("channel", Box::new(|c: &mut Clip| c.source_channel = Some(1))),
        ("effects", Box::new(|c: &mut Clip| c.set_effect("gain", -3.0))),
        ("fade at the join", Box::new(|c: &mut Clip| c.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 })),
    ];
    for (what, change) in changes {
        let (mut e, t) = split_pair(change);
        let before = e.session().track(t).unwrap().clips().to_vec();
        run(&mut e, "edit.heal", json!({"tracks": [t.0], "start": 0, "end": 2000}));
        assert_eq!(e.session().track(t).unwrap().clips(), before.as_slice(), "{what}: left apart, nothing dropped");
    }
}

#[test]
fn heal_joins_the_clip_gain_lines_of_both_pieces() {
    let (mut e, t) = split_pair(|c| c.gain_env = vec![(0, -6.0), (500, -12.0)]);
    run(&mut e, "edit.heal", json!({"tracks": [t.0], "start": 0, "end": 2000}));
    let c = e.session().track(t).unwrap().clips().to_vec();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].gain_env, vec![(999, 0.0), (1000, -6.0), (1500, -12.0)]);
    assert_eq!((c[0].start, c[0].length, c[0].source_offset()), (0, 2000, 0));
    // A plain split heals back to the one clip it was.
    let (mut e, t) = split_pair(|_| {});
    run(&mut e, "edit.heal", json!({"tracks": [t.0], "start": 0, "end": 2000}));
    assert_eq!(clips(&e, t), vec![(0, 2000, 0)]);
    assert!(e.session().track(t).unwrap().clips()[0].gain_env.is_empty());
}

// ---- bug 9: deleting fades in a selection took apart every crossfade on the track ----------

#[test]
fn deleting_fades_leaves_crossfades_outside_the_selection() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 1100, 0), (1000, 1100, 5000), (2000, 1000, 9000)]);
    let x = Fade { len: 100, shape: FadeShape::EqualPower, curvature: 0.0 };
    let s = e.session_mut();
    s.find_clip_mut(c[0]).unwrap().fade_out = x;
    s.find_clip_mut(c[1]).unwrap().fade_in = x;
    s.find_clip_mut(c[1]).unwrap().fade_out = x;
    s.find_clip_mut(c[2]).unwrap().fade_in = x;
    run(&mut e, "edit.fades_delete", json!({"tracks": [t.0], "start": 900, "end": 1200}));
    let got: Vec<_> = e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end(), c.fade_in.len, c.fade_out.len)).collect();
    assert_eq!(got, vec![(0, 1050, 0, 0), (1050, 2100, 0, 100), (2000, 3000, 100, 0)], "the second crossfade stays");
}

// ---- bug 7: Insert and Cut Time left other playlists, tempo, meter and keys behind ----------

fn all_playlists(e: &Engine, t: TrackId) -> Vec<Vec<(i64, i64)>> {
    e.session().track(t).unwrap().playlists.iter().map(|p| p.clips.iter().map(|c| (c.start, c.length)).collect()).collect()
}

/// A track with a clip at one second on two playlists, a tempo change and a key change at two
/// seconds (120 bpm: a bar is two seconds at 48 kHz).
fn timeline() -> (Engine, TrackId) {
    let mut e = engine("slip");
    let (t, _) = track(&mut e, &[(48_000, 48_000, 0)]);
    run(&mut e, "track.playlist_duplicate", json!({"track": t.0}));
    let s = e.session_mut();
    s.tempo.set_tempo(4 * soundcraft_time::TICKS_PER_QUARTER, 90.0).unwrap();
    s.key_signatures.push((96_000, "D".into()));
    (e, t)
}

#[test]
fn insert_time_moves_every_playlist_the_tempo_map_and_the_keys() {
    let (mut e, t) = timeline();
    // One bar (two seconds at 120 bpm) at the start.
    run(&mut e, "event.insert_time", json!({"start": 0, "length": 96_000}));
    assert_eq!(all_playlists(&e, t), vec![vec![(144_000, 48_000)]; 2]);
    let s = e.session();
    let bar = 4 * soundcraft_time::TICKS_PER_QUARTER;
    assert_eq!(s.tempo.tempos().iter().map(|x| (x.tick, x.bpm)).collect::<Vec<_>>(), vec![(0, 120.0), (2 * bar, 90.0)]);
    assert_eq!(s.tempo.tick_to_samples(2 * bar, s.sample_rate), 192_000, "the tempo change stays with the music after it");
    assert_eq!(s.key_signatures, vec![(192_000, "D".to_string())]);
    run(&mut e, "event.cut_time", json!({"start": 0, "end": 96_000}));
    assert_eq!(all_playlists(&e, t), vec![vec![(48_000, 48_000)]; 2]);
    let s = e.session();
    assert_eq!(s.tempo.tempos().iter().map(|x| (x.tick, x.bpm)).collect::<Vec<_>>(), vec![(0, 120.0), (bar, 90.0)]);
    assert_eq!(s.key_signatures, vec![(96_000, "D".to_string())]);
}

#[test]
fn time_is_not_inserted_inside_a_tempo_ramp() {
    let (mut e, _) = timeline();
    assert!(e.session_mut().tempo.set_ramp(0, true));
    assert!(e.execute("event.insert_time", &json!({"start": 48_000, "length": 1000})).is_err());
    assert!(e.execute("event.insert_time", &json!({"start": 200_000, "length": 1000})).is_ok());
}

// ---- bug 8: Identify Beat left the new tempo running past the selection --------------------

#[test]
fn identify_beat_restores_the_tempo_after_the_selection() {
    let mut e = engine("slip");
    let q = soundcraft_time::TICKS_PER_QUARTER;
    // 1.5 seconds called one bar of 4/4: 160 bpm.
    let r = run(&mut e, "event.identify_beat", json!({"start": 0, "end": 72_000, "bars": 1}));
    assert!((r["bpm"].as_f64().unwrap() - 160.0).abs() < 1e-6);
    let s = e.session();
    assert_eq!(s.tempo.samples_to_ticks(72_000, s.sample_rate), 4 * q, "the selection is the bar");
    assert_eq!(s.tempo.tempo_at_tick(4 * q), 120.0, "after it, the old tempo again");
    assert_eq!(s.tempo.samples_to_ticks(72_000 + 96_000, s.sample_rate), 8 * q, "two seconds later is one bar later, as before");
}

// ---- bug 16: Nudge moved clips the selection only touched, and closed gaps at zero ---------

#[test]
fn nudge_moves_only_the_clips_inside_the_selection() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 1000, 0), (1200, 200, 5000)]);
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 500, "end": 1500}));
    let by = run(&mut e, "edit.nudge", json!({"direction": 1}))["by"].as_i64().unwrap();
    assert!(by > 0);
    assert_eq!(e.session().find_clip(c[0]).unwrap().1.start, 0, "the clip the selection only touches stays");
    assert_eq!(e.session().find_clip(c[1]).unwrap().1.start, 1200 + by);
}

#[test]
fn clips_moved_past_the_session_start_keep_their_spacing() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(100, 100, 0), (1000, 100, 5000)]);
    run(&mut e, "edit.move_clips", json!({"clips": [c[0].0, c[1].0], "by": -500}));
    assert_eq!(clips(&e, t), vec![(0, 100, 0), (900, 100, 5000)]);
}

// ---- bug 17: a Shuffle trim moved clips from one point and automation from another ---------

#[test]
fn shuffle_trims_move_automation_with_the_crossfade_partner() {
    let mut e = engine("shuffle");
    let (t, c) = track(&mut e, &[(0, 1000, 0), (900, 1100, 5000)]);
    e.session_mut().edit.automation_follows_edit = true;
    for at in [950, 1500] {
        run(&mut e, "automation.set_point", json!({"track": t.0, "param": "volume", "at": at, "value": -6.0}));
    }
    run(&mut e, "edit.trim_clip", json!({"clip": c[0].0, "end": 800}));
    assert_eq!(clips(&e, t), vec![(0, 800, 0), (700, 1100, 5000)]);
    let pts: Vec<i64> = e.session().track(t).unwrap().lane(&soundcraft_model::AutoParam::Volume).unwrap().points.iter().map(|p| p.at).collect();
    assert_eq!(pts, vec![750, 1300], "both points moved with the partner");
}

// ---- bug 19: Delete Unused Playlists left the comp pointing at layers that were gone -------

#[test]
fn deleting_unused_playlists_keeps_the_comps_layers_and_their_references() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 1000, 0)]);
    for _ in 0..2 {
        run(&mut e, "track.playlist_new", json!({"track": t.0}));
    }
    let s = e.session_mut();
    let tr = s.track_mut(t).unwrap();
    tr.active_playlist = 0;
    tr.record_layer = Some(1);
    tr.playlists[2].clips = vec![Clip::audio(ClipId(9_000), "take", SourceId(8), 0, 0, 1000)];
    s.find_clip_mut(c[0]).unwrap().from_layer = Some(2);
    run(&mut e, "track.playlist_delete_unused", json!({"track": t.0}));
    let tr = e.session().track(t).unwrap();
    assert_eq!(tr.playlists.len(), 2, "the layer the comp came from stays");
    assert_eq!(tr.clips()[0].from_layer, Some(1), "and the comp still points at it");
    assert_eq!(tr.playlists[1].clips[0].name, "take");
    assert_eq!(tr.record_layer, None, "the record target that went is cleared");
}

// ---- bug 20: processing kept fades and warp markers that no longer fit ----------------------

#[test]
fn reverse_swaps_the_fades_and_processing_drops_the_warp_markers() {
    let mut e = engine("slip");
    let (_, c) = track(&mut e, &[(0, 1000, 0)]);
    let s = e.session_mut();
    let clip = s.find_clip_mut(c[0]).unwrap();
    clip.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
    clip.warp = vec![(0, 0), (500, 400)];
    clip.sync_point = 200;
    run(&mut e, "process.run", json!({"process": "reverse", "clips": [c[0].0]}));
    let c = e.session().tracks.iter().flat_map(|t| t.clips()).find(|x| x.start == 0).unwrap().clone();
    assert_eq!((c.fade_in.len, c.fade_out.len), (0, 100));
    assert!(c.warp.is_empty());
    assert_eq!(c.sync_point, 800);
}

// ---- bug 21: coalescing volume to clip gain left the other playlists behind -----------------

#[test]
fn coalescing_volume_to_clip_gain_covers_every_playlist() {
    let mut e = engine("slip");
    let (t, _) = track(&mut e, &[(0, 1000, 0)]);
    run(&mut e, "track.playlist_duplicate", json!({"track": t.0}));
    run(&mut e, "automation.set_point", json!({"track": t.0, "param": "volume", "at": 0, "value": -6.0}));
    run(&mut e, "edit.automation_coalesce_volume_to_clip_gain", json!({"tracks": [t.0]}));
    let tr = e.session().track(t).unwrap();
    assert_eq!(tr.mixer.volume_db, 0.0);
    for pl in &tr.playlists {
        // −6 dB as clip gain, however it is written (a level or a line).
        let db = |c: &Clip| 20.0 * c.gain_at(500).log10();
        assert!(pl.clips.iter().all(|c| (db(c) + 6.0).abs() < 1e-2), "{}: {:?}", pl.name, pl.clips.iter().map(db).collect::<Vec<_>>());
    }
}

// ---- bug 15: moving a folder track left its members behind ---------------------------------

#[test]
fn a_moved_folder_takes_its_members() {
    let mut e = engine("slip");
    let ids: Vec<TrackId> = (0..4).map(|_| new_track(&mut e)).collect();
    // Track 1 is a folder holding tracks 2 and 3; track 0 is above it.
    for m in &ids[2..] {
        e.session_mut().track_mut(*m).unwrap().folder = Some(ids[1]);
    }
    assert_eq!(soundcraft_engine::edit::folder_block(e.session(), ids[1]), 3);
    run(&mut e, "track.move", json!({"track": ids[1].0, "to": 0}));
    let order: Vec<TrackId> = e.session().tracks.iter().map(|t| t.id).filter(|t| ids.contains(t)).collect();
    assert_eq!(order, vec![ids[1], ids[2], ids[3], ids[0]]);
}

// ---- bug 24 (part): marker names, stray marker lengths, view-only locations ----------------

#[test]
fn new_markers_get_unused_names_and_no_stray_length() {
    let mut e = engine("slip");
    run(&mut e, "edit.select", json!({"start": 1000}));
    for at in [100, 200] {
        run(&mut e, "markers.add", json!({"at": at}));
    }
    run(&mut e, "markers.delete", json!({"number": 1}));
    // The selection starts after where a playing transport would add it: still a point.
    run(&mut e, "markers.add", json!({}));
    let s = e.session();
    let names: Vec<&str> = s.markers.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names.iter().filter(|n| **n == "Marker 2").count(), 1, "{names:?}");
    assert!(s.markers.iter().all(|m| m.end == m.start), "point markers have no length");
}

#[test]
fn a_view_only_location_leaves_the_playhead() {
    let mut e = engine("slip");
    let n = run(&mut e, "markers.add", json!({"at": 5000, "kind": "none"}))["number"].as_u64().unwrap();
    e.session_mut().edit.playhead = 100;
    run(&mut e, "markers.recall", json!({"number": n}));
    assert_eq!(e.session().edit.playhead, 100);
}

// ---- tier 2: undo leaves the view and the tool alone ----------------------------------------

#[test]
fn undo_keeps_the_zoom_scroll_and_tool() {
    let mut e = engine("slip");
    let (t, _) = track(&mut e, &[(0, 1000, 0)]);
    run(&mut e, "edit.separate", json!({"tracks": [t.0], "at": 500}));
    let z = &mut e.session_mut().edit;
    z.zoom.samples_per_px = 7.0;
    z.zoom.scroll = 123;
    z.tool = soundcraft_model::Tool::from_id("trim").unwrap();
    run(&mut e, "edit.undo", json!({}));
    let ed = &e.session().edit;
    assert_eq!((ed.zoom.samples_per_px, ed.zoom.scroll, ed.tool), (7.0, 123, soundcraft_model::Tool::from_id("trim").unwrap()));
    assert_eq!(clips(&e, t).len(), 1, "the edit itself is undone");
}

// ---- tier 2 (first slice): clip groups select, move and delete as one ----------------------

#[test]
fn a_clip_group_selects_moves_and_deletes_as_one() {
    let mut e = engine("slip");
    let (t, a) = track(&mut e, &[(0, 1000, 0), (3000, 500, 0)]);
    let (u, b) = track(&mut e, &[(500, 1000, 5000)]);
    run(&mut e, "clip.group", json!({"clips": [a[0].0, b[0].0]}));
    run(&mut e, "edit.select", json!({"clips": [a[0].0]}));
    let mut sel: Vec<u64> = e.session().edit.selected_clips.iter().map(|c| c.0).collect();
    sel.sort_unstable();
    assert_eq!(sel, vec![a[0].0, b[0].0], "the group comes along");
    run(&mut e, "edit.move_clips", json!({"by": 100}));
    assert_eq!(clips(&e, t), vec![(100, 1000, 0), (3000, 500, 0)]);
    assert_eq!(clips(&e, u), vec![(600, 1000, 5000)]);
    run(&mut e, "edit.select", json!({"clips": [b[0].0]}));
    run(&mut e, "edit.clear", json!({}));
    assert_eq!(clips(&e, t), vec![(3000, 500, 0)], "the ungrouped clip stays");
    assert!(clips(&e, u).is_empty());
    // `exact` picks one member alone.
    run(&mut e, "edit.undo", json!({}));
    run(&mut e, "edit.select", json!({"clips": [b[0].0], "exact": true}));
    assert_eq!(e.session().edit.selected_clips, vec![b[0]]);
}

#[test]
fn a_clip_groups_trims_and_fades_act_on_every_member() {
    let mut e = engine("slip");
    let (t, a) = track(&mut e, &[(1000, 1000, 1000)]);
    let (u, b) = track(&mut e, &[(1200, 1000, 5000)]);
    let (_, lone) = track(&mut e, &[(1000, 1000, 9000)]);
    run(&mut e, "clip.group", json!({"clips": [a[0].0, b[0].0]}));
    run(&mut e, "edit.trim_clip", json!({"clip": a[0].0, "start": 1100}));
    assert_eq!(clips(&e, t), vec![(1100, 900, 1100)]);
    assert_eq!(clips(&e, u), vec![(1300, 900, 5100)], "the other member's start moves as far");
    run(&mut e, "edit.trim_clip", json!({"clip": b[0].0, "end": 2100}));
    assert_eq!(clips(&e, u), vec![(1300, 800, 5100)]);
    assert_eq!(clips(&e, t), vec![(1100, 800, 1100)]);
    run(&mut e, "edit.trim_clip", json!({"clip": b[0].0, "end": 2200, "alone": true}));
    assert_eq!(clips(&e, t), vec![(1100, 800, 1100)], "alone: the group stays");
    let r = run(&mut e, "edit.fades_create", json!({"clip": a[0].0, "side": "in", "length": 200}));
    assert_eq!(r["fades"], 2);
    let s = e.session();
    assert_eq!(s.find_clip(b[0]).unwrap().1.fade_in.len, 200);
    assert_eq!(s.find_clip(lone[0]).unwrap().1.fade_in.len, 0, "a clip outside the group is left alone");
    assert_eq!(soundcraft_engine::edit::group_members(s, lone[0]), vec![lone[0]]);
}

// ---- tier 2: snapping to clip edges, markers and zero crossings ------------------------------

#[test]
fn edit_points_snap_to_clip_edges_markers_and_zero_crossings() {
    use soundcraft_engine::edit::{magnet_target, snap_point, zero_crossing_near};
    let mut e = engine("slip");
    let (t, _) = track(&mut e, &[(10_000, 5_000, 0), (20_000, 1_000, 0)]);
    run(&mut e, "markers.add", json!({"at": 30_000}));
    // Nothing pulls until asked.
    assert_eq!(magnet_target(e.session(), 10_050, 100, &[]), None);
    run(&mut e, "edit.snap_to_clips", json!({}));
    let s = e.session();
    assert_eq!(magnet_target(s, 10_050, 100, &[]), Some(10_000));
    assert_eq!(magnet_target(s, 14_960, 100, &[]), Some(15_000), "a clip's end");
    assert_eq!(magnet_target(s, 10_500, 100, &[]), None, "out of reach");
    assert_eq!(magnet_target(s, 30_010, 100, &[]), None, "markers are off");
    let first = s.track(t).unwrap().clips()[0].id;
    assert_eq!(magnet_target(s, 10_050, 100, &[first]), None, "a dragged clip does not pull itself");
    // A grid line nearer than the edge wins; an edge nearer than the grid line wins.
    assert_eq!(snap_point(s, 10_050, 10_040, 100, None, &[]), 10_040);
    assert_eq!(snap_point(s, 10_020, 10_100, 100, None, &[]), 10_000);
    run(&mut e, "edit.snap_to_markers", json!({}));
    assert_eq!(magnet_target(e.session(), 29_950, 100, &[]), Some(30_000));

    // Zero crossings: a sine of period 100 in the clip's file, crossing every 50 samples.
    let wave: Vec<f32> = (0..100_000).map(|i| ((i as f64 + 0.5) * std::f64::consts::TAU / 100.0).sin() as f32).collect();
    let audio = soundcraft_model::SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![wave] });
    e.session_mut().pool.insert(SourceId(8), std::sync::Arc::new(audio));
    let z = zero_crossing_near(e.session(), t, 10_120, 240).unwrap();
    assert!((10_099..=10_100).contains(&z), "the nearest crossing to file sample 120 is at 100: {z}");
    assert_eq!(zero_crossing_near(e.session(), t, 17_000, 240), None, "no audio there");
    // Off, an insertion stays put; on, it moves to the crossing.
    assert_eq!(snap_point(e.session(), 10_120, 10_120, 0, Some(t), &[]), 10_120);
    run(&mut e, "edit.snap_to_zero_crossings", json!({}));
    assert_eq!(snap_point(e.session(), 10_120, 10_120, 0, Some(t), &[]), z);
    // Undo keeps the snap settings, like the Edit mode.
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 0, "end": 100}));
    run(&mut e, "edit.clear", json!({}));
    run(&mut e, "edit.undo", json!({}));
    assert!(e.session().edit.flag(soundcraft_engine::edit::SNAP_TO_ZERO));
}

// ---- tier 2: fades with a shape for each side, and batch fades ---------------------------------

#[test]
fn fades_take_a_shape_for_each_side_and_batch_fades_reach_clips_inside() {
    let mut e = engine("slip");
    // A butted pair (crossfaded at 2000) and a clip standing apart inside the range.
    let (t, c) = track(&mut e, &[(1000, 1000, 1000), (2000, 1000, 2000), (5000, 1000, 9000)]);
    run(
        &mut e,
        "edit.fades_create",
        json!({"tracks": [t.0], "start": 1900, "end": 6500, "in_shape": "s-curve", "out_shape": "exponential", "out_curvature": 0.5, "batch_length": 100}),
    );
    let s = e.session();
    let (a, b, d) = (s.find_clip(c[0]).unwrap().1, s.find_clip(c[1]).unwrap().1, s.find_clip(c[2]).unwrap().1);
    assert_eq!((a.fade_out.shape, a.fade_out.curvature), (FadeShape::Exponential, 0.5), "the outgoing side");
    assert_eq!(b.fade_in.shape, FadeShape::SCurve, "the incoming side");
    assert_eq!((d.fade_in.len, d.fade_out.len), (100, 100), "the clip standing inside gets both");
    assert_eq!((d.fade_in.shape, d.fade_out.shape), (FadeShape::SCurve, FadeShape::Exponential));
    // Without a batch length, a clip standing apart gets none (as before).
    run(&mut e, "edit.undo", json!({}));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 1900, "end": 6500}));
    let d = e.session().find_clip(c[2]).unwrap().1;
    assert_eq!((d.fade_in.len, d.fade_out.len), (0, 0));
    assert!(e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": 1900, "end": 2100, "in_shape": "wobbly"})).is_err());
}

// ---- tier 2: processing keeps handles ---------------------------------------------------------

#[test]
fn a_processed_clip_keeps_handles_to_trim_open() {
    let mut e = engine("slip");
    // 1000 samples of the file before the clip, plenty after.
    let (t, c) = track(&mut e, &[(5000, 1000, 1000)]);
    run(&mut e, "process.run", json!({"process": "invert", "clips": [c[0].0]}));
    let s = e.session();
    let nc = s.track(t).unwrap().clips()[0].clone();
    assert_eq!((nc.start, nc.length, nc.source_offset()), (5000, 1000, 1000), "the clip sits where it was, 1000 into its new file");
    let src = s.sources.last().unwrap();
    assert_eq!(src.frames, 1000 + 1000 + 48_000, "1 s of handle after, all 1000 before");
    // It can be trimmed open again.
    run(&mut e, "edit.trim_clip", json!({"clip": nc.id.0, "start": 4000, "end": 7000}));
    assert_eq!(clips(&e, t), vec![(4000, 3000, 0)]);
    // A process that changes the length keeps none.
    let (u, d) = track(&mut e, &[(5000, 1000, 1000)]);
    run(&mut e, "process.run", json!({"process": "time_stretch", "params": {"ratio": 2.0}, "clips": [d[0].0]}));
    assert_eq!(e.session().track(u).unwrap().clips()[0].source_offset(), 0);
}

// ---- tier 2: quantize audio with strength and swing -------------------------------------------

#[test]
fn audio_quantize_takes_a_strength_and_a_swing() {
    // 120 bpm at 48 kHz, a 1/16 grid: lines every 6000 samples.
    let mut e = engine("slip");
    run(&mut e, "view.grid", json!({"value": "1/16"}));
    let (t, c) = track(&mut e, &[(6_900, 500, 0), (19_600, 500, 0)]);
    run(&mut e, "clip.quantize_to_grid", json!({"clips": [c[0].0], "strength": 50}));
    assert_eq!(clips(&e, t)[0].0, 6_450, "half way to the line at 6000");
    run(&mut e, "clip.quantize_to_grid", json!({"clips": [c[0].0]}));
    assert_eq!(clips(&e, t)[0].0, 6_000, "full strength by default");
    // Swing 100 %: every second sixteenth is a third of a step (2000) late, 20 000 not 18 000.
    run(&mut e, "clip.quantize_to_grid", json!({"clips": [c[1].0], "swing": 100}));
    assert_eq!(clips(&e, t)[1].0, 20_000);
    run(&mut e, "edit.undo", json!({}));
    run(&mut e, "clip.quantize_to_grid", json!({"clips": [c[1].0]}));
    assert_eq!(clips(&e, t)[1].0, 18_000, "no swing: the plain line");
}

// ---- tier 2: overlap modes (Options › Overlap) ---------------------------------------------

/// File 8 reads 0.5 up to sample 40 000 and −0.25 after: a clip from offset 0 plays 0.5, one
/// from offset 50 000 plays −0.25.
fn two_level_audio(e: &mut Engine) {
    let wave: Vec<f32> = (0..100_000).map(|i| if i < 40_000 { 0.5 } else { -0.25 }).collect();
    let audio = soundcraft_model::SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![wave] });
    e.session_mut().pool.insert(SourceId(8), std::sync::Arc::new(audio));
}

fn heard(e: &Engine, t: TrackId, at: i64) -> f32 {
    soundcraft_mix::render_clips(e.session(), t, soundcraft_time::Range::new(at, at + 1))[0][0]
}

#[test]
fn overlap_modes_overwrite_layer_or_crossfade_what_a_clip_lands_on() {
    // Overwrite (the default): what the moved clip covers is cut away, as always.
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 10_000, 0), (20_000, 2_000, 50_000)]);
    run(&mut e, "edit.move_clips", json!({"clips": [c[1].0], "to": 4_000}));
    assert_eq!(clips(&e, t), vec![(0, 4_000, 0), (4_000, 2_000, 50_000), (6_000, 4_000, 6_000)]);
    assert!(e.session().track(t).unwrap().clips().iter().all(|c| c.layer == 0));

    // Layer: the clip underneath stays whole; the moved one is heard where it covers it, the
    // lower one again after it.
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 10_000, 0), (20_000, 2_000, 50_000)]);
    two_level_audio(&mut e);
    run(&mut e, "edit.overlap_mode", json!({"mode": "layer"}));
    run(&mut e, "edit.move_clips", json!({"clips": [c[1].0], "to": 4_000}));
    assert_eq!(clips(&e, t), vec![(0, 10_000, 0), (4_000, 2_000, 50_000)], "both whole");
    assert_eq!(e.session().find_clip(c[1]).unwrap().1.layer, 1);
    assert_eq!(heard(&e, t, 3_000), 0.5);
    assert_eq!(heard(&e, t, 5_000), -0.25, "only the top clip is heard where it covers");
    assert_eq!(heard(&e, t, 7_000), 0.5, "the lower clip returns where it is uncovered");
    // Send to Back puts it under; Bring to Front back on top.
    run(&mut e, "clip.send_to_back", json!({"clips": [c[1].0]}));
    assert_eq!(heard(&e, t, 5_000), 0.5);
    run(&mut e, "clip.bring_to_front", json!({"clips": [c[1].0]}));
    assert_eq!(heard(&e, t, 5_000), -0.25);
    // A move is one undo step; the mode itself is kept by undo, like the Edit mode.
    run(&mut e, "edit.undo", json!({}));
    run(&mut e, "edit.undo", json!({}));
    run(&mut e, "edit.undo", json!({}));
    assert_eq!(clips(&e, t), vec![(0, 10_000, 0), (20_000, 2_000, 50_000)]);
    assert_eq!(soundcraft_engine::edit::Overlap::of(e.session()), soundcraft_engine::edit::Overlap::Layer);
    // Moving it clear of everything brings it back to layer 0.
    run(&mut e, "edit.move_clips", json!({"clips": [c[1].0], "to": 4_000}));
    run(&mut e, "edit.move_clips", json!({"clips": [c[1].0], "to": 30_000}));
    assert_eq!(e.session().find_clip(c[1]).unwrap().1.layer, 0);
    assert_eq!(heard(&e, t, 5_000), 0.5);

    // Crossfade: overwrite, then a crossfade at each join out of the handles.
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 10_000, 0), (20_000, 2_000, 50_000)]);
    run(&mut e, "edit.overlap_mode", json!({"mode": "crossfade", "crossfade_ms": 10}));
    run(&mut e, "edit.move_clips", json!({"clips": [c[1].0], "to": 4_000}));
    let cl = e.session().track(t).unwrap().clips().to_vec();
    assert_eq!(cl.len(), 3);
    let (left, mid, right) = (&cl[0], &cl[1], &cl[2]);
    assert_eq!(mid.id, c[1]);
    // 10 ms at 48 kHz: 240 samples each side of each join.
    assert_eq!((left.end(), left.fade_out.len), (4_240, 480), "the old audio runs under the new clip's start");
    assert_eq!((mid.start, mid.fade_in.len, mid.end(), mid.fade_out.len), (3_760, 480, 6_240, 480));
    assert_eq!((right.start, right.fade_in.len), (5_760, 480));
    assert_eq!(left.fade_out.shape, FadeShape::default());
    // One undo step takes the move and its crossfades back.
    run(&mut e, "edit.undo", json!({}));
    assert_eq!(clips(&e, t), vec![(0, 10_000, 0), (20_000, 2_000, 50_000)]);
    assert!(e.execute("edit.overlap_mode", &json!({"mode": "stack"})).is_err());
}

// ---- tier 2: edit groups edit together ----------------------------------------------------

#[test]
fn an_edit_on_one_track_of_an_edit_group_applies_to_the_others() {
    let mut e = engine("slip");
    // Kick and snare in an edit group; room outside it.
    let (k, kc) = track(&mut e, &[(10_000, 5_000, 0), (30_000, 5_000, 20_000)]);
    let (n, nc) = track(&mut e, &[(10_200, 4_000, 100_000), (30_000, 5_000, 120_000)]);
    let (r, rc) = track(&mut e, &[(10_000, 5_000, 200_000)]);
    run(&mut e, "track.group", json!({"tracks": [k.0, n.0], "name": "Drums"}));

    // Move: grabbing the kick's clip takes the snare's clip under it, one undo step.
    run(&mut e, "edit.select", json!({"clips": [kc[0].0]}));
    let mut sel: Vec<u64> = e.session().edit.selected_clips.iter().map(|c| c.0).collect();
    sel.sort_unstable();
    assert_eq!(sel, vec![kc[0].0, nc[0].0], "the grouped track's clip comes along");
    run(&mut e, "edit.move_clips", json!({"clips": [kc[0].0], "by": 1_000}));
    assert_eq!(clips(&e, k)[0], (11_000, 5_000, 0));
    assert_eq!(clips(&e, n)[0], (11_200, 4_000, 100_000));
    assert_eq!(clips(&e, r), vec![(10_000, 5_000, 200_000)], "a track outside the group stays");
    run(&mut e, "edit.undo", json!({}));
    assert_eq!(clips(&e, n)[0], (10_200, 4_000, 100_000), "one undo step for both");
    // `alone` moves one.
    run(&mut e, "edit.move_clips", json!({"clips": [kc[0].0], "by": 1_000, "alone": true}));
    assert_eq!(clips(&e, n)[0], (10_200, 4_000, 100_000));
    run(&mut e, "edit.undo", json!({}));

    // Trim: the same edge of the grouped clip moves as far.
    run(&mut e, "edit.trim_clip", json!({"clip": kc[0].0, "end": 14_000}));
    assert_eq!(clips(&e, k)[0], (10_000, 4_000, 0));
    assert_eq!(clips(&e, n)[0], (10_200, 3_000, 100_000));
    assert_eq!(clips(&e, r)[0], (10_000, 5_000, 200_000));
    run(&mut e, "edit.undo", json!({}));

    // Fade: drawn on one, on both.
    run(&mut e, "edit.fades_create", json!({"clip": nc[1].0, "side": "out", "length": 500}));
    let s = e.session();
    assert_eq!(s.find_clip(kc[1]).unwrap().1.fade_out.len, 500);
    assert_eq!(s.find_clip(nc[1]).unwrap().1.fade_out.len, 500);
    run(&mut e, "edit.undo", json!({}));

    // Split and cut through the selection a grouped track's selection spreads to.
    run(&mut e, "edit.select", json!({"tracks": [k.0], "start": 32_000, "end": 33_000}));
    assert_eq!(e.session().edit.selected_tracks, vec![k, n]);
    run(&mut e, "edit.separate", json!({}));
    assert_eq!(clips(&e, k).len(), 4);
    assert_eq!(clips(&e, n).len(), 4);
    run(&mut e, "edit.undo", json!({}));
    run(&mut e, "edit.select", json!({"tracks": [k.0], "start": 32_000, "end": 33_000}));
    run(&mut e, "edit.cut", json!({}));
    assert_eq!(clips(&e, n), vec![(10_200, 4_000, 100_000), (30_000, 2_000, 120_000), (33_000, 2_000, 123_000)]);
    run(&mut e, "edit.undo", json!({}));

    // A group with Edit off, inactive, or every group suspended: the clip moves alone.
    let g = e.session().groups[0].id;
    run(&mut e, "track.group_modify", json!({"group": g.0, "edit": false}));
    run(&mut e, "edit.move_clips", json!({"clips": [kc[0].0], "by": 1_000}));
    assert_eq!(clips(&e, n)[0], (10_200, 4_000, 100_000));
    run(&mut e, "edit.undo", json!({}));
    run(&mut e, "track.group_modify", json!({"group": g.0, "edit": true}));
    run(&mut e, "track.suspend_groups", json!({"value": true}));
    run(&mut e, "edit.move_clips", json!({"clips": [kc[0].0], "by": 1_000}));
    assert_eq!(clips(&e, n)[0], (10_200, 4_000, 100_000));
    let _ = rc;
}

// ---- round three: the open rows ----------------------------------------------------------

#[test]
fn redo_history_lists_the_undone_steps_next_first() {
    let mut e = engine("slip");
    for _ in 0..3 {
        run(&mut e, "track.new", json!({"count": 1}));
    }
    let labels = e.undo_history();
    assert!(e.redo_history().is_empty());
    e.undo();
    e.undo();
    let n = labels.len();
    assert_eq!(e.redo_history(), vec![labels[n - 2].clone(), labels[n - 1].clone()]);
    e.redo();
    assert_eq!(e.redo_history(), vec![labels[n - 1].clone()]);
}

#[test]
fn a_locked_track_refuses_edits_to_its_clips_and_takes_none_new() {
    let mut e = engine("slip");
    let (t, c) = track(&mut e, &[(0, 1000, 0), (2000, 1000, 2000)]);
    let (u, d) = track(&mut e, &[(5000, 1000, 0)]);
    run(&mut e, "track.lock", json!({"tracks": [t.0]}));
    let before = clips(&e, t);
    run(&mut e, "edit.select", json!({"clips": [c[0].0]}));
    assert!(e.execute("edit.clear", &json!({})).is_err(), "delete refused");
    assert!(e.execute("edit.move_clips", &json!({"clips": [c[1].0], "by": 100})).is_err(), "move refused");
    assert!(e.execute("edit.move_clips", &json!({"clips": [d[0].0], "track": t.0})).is_err(), "nothing lands on it");
    assert!(e.execute("mix.record_arm", &json!({"tracks": [t.0], "value": true})).is_err(), "no recording onto it");
    assert_eq!(clips(&e, t), before);
    // Other tracks edit as before; Insert Time still moves the locked clips.
    run(&mut e, "edit.move_clips", json!({"clips": [d[0].0], "by": 100}));
    assert_eq!(clips(&e, u), vec![(5100, 1000, 0)]);
    run(&mut e, "event.insert_time", json!({"start": 1500, "length": 500}));
    assert_eq!(clips(&e, t), vec![(0, 1000, 0), (2500, 1000, 2000)]);
    run(&mut e, "track.lock", json!({"tracks": [t.0]}));
    run(&mut e, "edit.select", json!({"clips": [c[0].0]}));
    run(&mut e, "edit.clear", json!({}));
    assert_eq!(clips(&e, t).len(), 1, "unlocked, it edits again");
}
