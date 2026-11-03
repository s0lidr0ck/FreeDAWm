//! Track › Freeze renders an audio track's clips through its module chain, pre-fader, and plays
//! that render in place of the clips and modules; the fader section stays live, edits to what was
//! frozen are refused, and the frozen render survives save and load.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{Session, TrackId};
use soundcraft_time::Range;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-freeze-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The demo with a gain module (+4 dB) and a compressor on the mono Bass and the stereo Pad.
fn engine() -> Engine {
    let mut e = soundcraft_engine::demo::demo_engine();
    for t in ["Bass", "Pad"] {
        e.execute("mix.insert", &json!({"track": t, "plugin": "gain", "params": {"gain": 4.0}})).unwrap();
        e.execute("mix.insert", &json!({"track": t, "plugin": "compressor"})).unwrap();
    }
    e
}

/// Where the gain module sits in `track`'s chain.
fn gain_slot(e: &Engine, track: &str) -> usize {
    e.session().track(id(e, track)).unwrap().mixer.inserts.iter().position(|i| i.plugin == "gain").unwrap()
}

fn id(e: &Engine, name: &str) -> TrackId {
    e.session().tracks.iter().find(|t| t.name == name).unwrap().id
}

fn mix(s: &Session) -> Vec<Vec<f32>> {
    soundcraft_mix::render_range(s, Range::new(0, s.sample_rate.samples(6.0)), 1024)
}

/// `s` with every track thawed: what live playback would be.
fn thawed(s: &Session) -> Session {
    let mut s = s.clone();
    for t in &mut s.tracks {
        t.frozen = false;
        t.freeze = None;
    }
    s
}

fn assert_close(a: &[Vec<f32>], b: &[Vec<f32>], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: channels");
    let mut worst = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.len(), y.len(), "{what}: length");
        worst = x.iter().zip(y).fold(worst, |w, (p, q)| w.max((p - q).abs()));
    }
    assert!(worst < 1e-4, "{what}: differs by {worst}");
    assert!(a.iter().flatten().any(|v| v.abs() > 1e-3), "{what}: silent");
}

#[test]
fn a_frozen_track_sounds_as_it_did_live_and_its_fader_stays_live() {
    let mut e = engine();
    let live = mix(e.session());
    let r = e.execute("track.freeze", &json!({"tracks": ["Bass", "Pad"]})).unwrap();
    assert_eq!(r["frozen"].as_array().map(Vec::len), Some(2), "{r}");
    for t in ["Bass", "Pad"] {
        let tr = e.session().track(id(&e, t)).unwrap();
        assert!(tr.frozen && tr.frozen_audio().is_some(), "{t}");
        let src = tr.freeze.unwrap().source;
        assert!(e.session().sources.iter().any(|x| x.id == src && x.path.starts_with("Audio Files/")), "{t}: the render is a session audio file");
        // The render covers the clips and at least the shortest tail.
        let frames = e.session().pool.get(src).unwrap().frames() as i64;
        let end = tr.clips().iter().map(|c| c.end()).max().unwrap();
        assert!(frames >= end + e.session().sample_rate.samples(2.0), "{t}: {frames} frames for content to {end}");
    }
    assert_close(&mix(e.session()), &live, "frozen vs live");

    // The fader, pan and mute stay live: same as live with the same moves.
    e.execute("mix.volume", &json!({"track": "Bass", "db": -9.0})).unwrap();
    e.execute("mix.pan", &json!({"track": "Pad", "pan": 0.3, "index": 0})).unwrap();
    let moved = mix(e.session());
    assert_close(&moved, &mix(&thawed(e.session())), "fader moves after freeze");
    assert!(moved.iter().zip(&live).any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-3)), "the fader moved the mix");
}

#[test]
fn edits_to_what_was_frozen_are_refused() {
    let mut e = engine();
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    let bass = id(&e, "Bass");
    let clip = e.session().track(bass).unwrap().clips()[0].id.0;
    let before = e.session().track(bass).unwrap().clone();
    let undo = e.undo_history().len();
    let slot = gain_slot(&e, "Bass");
    for (cmd, p) in [
        ("mix.insert_param", json!({"track": "Bass", "slot": slot, "param": "gain", "value": 9.0})),
        ("mix.insert", json!({"track": "Bass", "plugin": "eq_7band"})),
        ("mix.insert_remove", json!({"track": "Bass", "slot": 0})),
        ("mix.insert_bypass", json!({"track": "Bass", "slot": 0})),
        ("mix.insert_move", json!({"track": "Bass", "from": 0, "to": 1})),
        ("mix.trim", json!({"track": "Bass", "db": 3.0})),
        ("clip.gain", json!({"clips": [clip], "db": -6.0})),
        ("track.input", json!({"tracks": ["Bass"], "input": "Verb"})),
    ] {
        let err = e.execute(cmd, &p).expect_err(cmd).to_string();
        assert!(err.contains("Unfreeze the track to edit it"), "{cmd}: {err}");
    }
    assert_eq!(e.session().track(bass).unwrap(), &before, "nothing changed");
    assert_eq!(e.undo_history().len(), undo, "no undo steps");
    // The fader section, and other tracks, stay editable.
    for (cmd, p) in [
        ("mix.volume", json!({"track": "Bass", "db": -3.0})),
        ("mix.solo", json!({"track": "Bass"})),
        ("mix.mute", json!({"track": "Bass"})),
        ("mix.insert", json!({"track": "Pad", "plugin": "eq_7band"})),
    ] {
        e.execute(cmd, &p).unwrap_or_else(|err| panic!("{cmd}: {err}"));
    }
}

#[test]
fn unfreeze_plays_live_again_and_each_is_one_undo_step() {
    let mut e = engine();
    let live = mix(e.session());
    let bass = id(&e, "Bass");
    let steps = e.undo_history().len();
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(e.session().track(bass).unwrap().frozen);
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(!e.session().track(bass).unwrap().frozen, "undo thaws");
    e.execute("edit.redo", &json!({})).unwrap();
    assert!(e.session().track(bass).unwrap().frozen_audio().is_some(), "redo freezes again");

    e.execute("track.unfreeze", &json!({"tracks": ["Bass"]})).unwrap();
    assert_eq!(e.undo_history().len(), steps + 2);
    let tr = e.session().track(bass).unwrap();
    assert!(!tr.frozen && tr.freeze.is_none());
    assert_close(&mix(e.session()), &live, "thawed");
    // Live again: the modules are editable.
    e.execute("mix.insert_param", &json!({"track": "Bass", "slot": gain_slot(&e, "Bass"), "param": "gain", "value": 9.0})).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(e.session().track(bass).unwrap().frozen_audio().is_some(), "undoing the unfreeze plays the render again");
    // track.freeze toggles: on a frozen track it thaws.
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    assert!(!e.session().track(bass).unwrap().frozen);
}

#[test]
fn only_tracks_with_audio_or_an_instrument_and_clips_freeze() {
    let mut e = engine();
    let err = e.execute("track.freeze", &json!({"tracks": ["Verb"]})).unwrap_err().to_string();
    assert!(err.contains("Freeze works on audio tracks and tracks with an instrument: `Verb` is an Aux Input with no instrument"), "{err}");
    e.execute("track.new", &json!({"name": "Empty"})).unwrap();
    let err = e.execute("track.freeze", &json!({"tracks": ["Empty"]})).unwrap_err().to_string();
    assert!(err.contains("nothing audible to freeze: it has no clips"), "{err}");
    assert!(e.session().tracks.iter().all(|t| !t.frozen), "a refused freeze freezes nothing");
}

#[test]
fn a_frozen_track_saves_and_loads_and_a_missing_render_loads_thawed() {
    let mut e = engine();
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    let frozen_mix = mix(e.session());
    let dir = scratch("save");
    let path = dir.join("Song.scraft");
    soundcraft_engine::io::save_session(&mut e, path.to_str().unwrap()).unwrap();

    let mut back = Engine::default();
    let missing = soundcraft_engine::io::open_session(&mut back, path.to_str().unwrap()).unwrap();
    assert!(missing.is_empty(), "{missing:?}");
    let bass = id(&back, "Bass");
    assert!(back.session().track(bass).unwrap().frozen_audio().is_some());
    assert_close(&mix(back.session()), &frozen_mix, "loaded");

    // Delete the render: the session loads with the track thawed, and says so.
    let src = back.session().track(bass).unwrap().freeze.unwrap().source;
    let file = back.session().sources.iter().find(|x| x.id == src).unwrap().path.clone();
    std::fs::remove_file(dir.join(&file)).unwrap();
    let mut thawed = Engine::default();
    let missing = soundcraft_engine::io::open_session(&mut thawed, path.to_str().unwrap()).unwrap();
    assert!(!missing.is_empty());
    let tr = thawed.session().track(id(&thawed, "Bass")).unwrap();
    assert!(!tr.frozen && tr.freeze.is_none(), "loads thawed");
    assert!(thawed.messages.iter().any(|m| m.contains("Bass") && m.contains("unfrozen")), "{:?}", thawed.messages);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_frozen_track_is_not_recorded_on() {
    let mut e = engine();
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    let err = e.execute("mix.record_arm", &json!({"track": "Bass", "value": true})).unwrap_err().to_string();
    assert!(err.contains("Unfreeze the track to record on it"), "{err}");
    assert!(!e.session().track(id(&e, "Bass")).unwrap().mixer.record_arm);
    // Armed before it froze (or by a session): recording refuses to start on it.
    let bass = id(&e, "Bass");
    e.session_mut().track_mut(bass).unwrap().mixer.record_arm = true;
    let err = soundcraft_engine::io::plan_takes(&e, &std::env::temp_dir(), 2).unwrap_err();
    assert!(err.contains("Bass") && err.contains("Unfreeze the track to record on it"), "{err}");
    // Disarming is fine.
    e.execute("mix.record_arm", &json!({"track": "Bass", "value": false})).unwrap();
}

#[test]
fn edits_that_do_not_change_the_sound_are_allowed_while_frozen() {
    let mut e = engine();
    let bass = id(&e, "Bass");
    // An inactive playlist to edit.
    e.execute("track.playlist_duplicate", &json!({"track": "Bass"})).unwrap();
    e.execute("track.playlist_select", &json!({"track": "Bass", "index": 0})).unwrap();
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    let clip = e.session().track(bass).unwrap().clips()[0].id.0;
    let other = e.session().track(bass).unwrap().playlists[1].clips[0].id.0;
    let module = e.session().track(bass).unwrap().mixer.inserts[gain_slot(&e, "Bass")].id.0;
    for (cmd, p) in [
        ("clip.rename", json!({"clip": clip, "name": "Low end"})),
        ("clip.color", json!({"clips": [clip], "color": [10, 20, 30]})),
        ("clip.rating", json!({"clips": [clip], "rating": 4})),
        ("clip.gain", json!({"clips": [other], "db": -6.0})),
        ("track.rename", json!({"track": "Bass", "name": "Bass DI"})),
        ("track.playlist_duplicate", json!({"track": "Bass DI"})),
        ("mix.module_knobs", json!({"track": "Bass DI", "module": module, "knobs": ["gain"]})),
    ] {
        e.execute(cmd, &p).unwrap_or_else(|err| panic!("{cmd}: {err}"));
    }
    assert!(e.session().track(bass).unwrap().frozen_audio().is_some());
    // Switching to a playlist that sounds different, or an automation mode change, is refused.
    for (cmd, p) in [("track.playlist_new", json!({"track": "Bass DI"})), ("mix.automation_mode", json!({"tracks": ["Bass DI"], "mode": "off"}))] {
        let err = e.execute(cmd, &p).expect_err(cmd).to_string();
        assert!(err.contains("Unfreeze the track to edit it"), "{cmd}: {err}");
    }
}

#[test]
fn freezing_names_a_sidechain_key_it_renders_silent() {
    let mut e = engine();
    let bass = id(&e, "Bass");
    e.execute("mix.insert", &json!({"track": "Bass", "plugin": "fs_shale"})).unwrap();
    let shale = e.session().track(bass).unwrap().mixer.inserts.iter().find(|i| i.plugin == "fs_shale").unwrap().id.0;
    e.execute("mix.module_sidechain", &json!({"track": "Bass", "module": shale, "source": "Kick"})).unwrap();
    let r = e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    let warnings: Vec<String> = r["warnings"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_string()).collect();
    assert!(warnings.iter().any(|w| w.contains("rendered without the sidechain key from `Kick`")), "{warnings:?}");
}

#[test]
fn a_track_with_only_muted_clips_has_nothing_audible_to_freeze() {
    let mut e = engine();
    let bass = id(&e, "Bass");
    for c in &mut e.session_mut().track_mut(bass).unwrap().playlists[0].clips {
        c.muted = true;
    }
    let err = e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap_err().to_string();
    assert!(err.contains("nothing audible to freeze"), "{err}");
}

#[test]
fn deleting_a_frozen_track_drops_its_render_but_not_a_source_a_sampler_plays() {
    let mut e = engine();
    e.execute("track.freeze", &json!({"tracks": ["Bass", "Pad"]})).unwrap();
    let render = |e: &Engine, name: &str| e.session().track(id(e, name)).unwrap().freeze.unwrap().source;
    let (bass, pad) = (render(&e, "Bass"), render(&e, "Pad"));
    e.execute("track.delete", &json!({"tracks": ["Bass"]})).unwrap();
    assert!(!e.session().sources.iter().any(|s| s.id == bass) && e.session().pool.get(bass).is_none(), "the render left");
    // A module that plays the Pad's render as its sample keeps it when the Pad thaws.
    let keys = id(&e, "Keys");
    if let Some(ins) = e.session_mut().track_mut(keys).unwrap().mixer.inserts.first_mut() {
        ins.sample = Some(pad);
    } else {
        let mut ins = soundcraft_model::Insert::new("gain");
        ins.sample = Some(pad);
        e.session_mut().track_mut(keys).unwrap().mixer.add_module(ins, None).unwrap();
    }
    e.execute("track.unfreeze", &json!({"tracks": ["Pad"]})).unwrap();
    assert!(e.session().sources.iter().any(|s| s.id == pad) && e.session().pool.get(pad).is_some(), "still played by a sampler");
}

#[test]
fn a_latent_module_freezes_in_line_with_the_other_tracks() {
    let mut e = engine();
    e.execute("mix.insert", &json!({"track": "Bass", "plugin": "maximizer"})).unwrap();
    let live = mix(e.session());
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    assert_close(&mix(e.session()), &live, "frozen with a latent module");
}
