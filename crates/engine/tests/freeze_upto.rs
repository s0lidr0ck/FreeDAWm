//! Track › Freeze up to here: the render holds the chain down to and including the freeze point;
//! the modules after it stay live (editable, automated, latency compensated) and process the
//! render. Edits to the frozen part are refused; moving the point re-renders as one undo step.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{Session, TrackId};
use soundcraft_time::Range;
use std::path::PathBuf;

fn id(e: &Engine, name: &str) -> TrackId {
    e.session().tracks.iter().find(|t| t.name == name).unwrap().id
}

/// The main mix from the start to 16 s: the demo's Bass plays from about 9 s.
fn mix(s: &Session) -> Vec<Vec<f32>> {
    soundcraft_mix::render_range(s, Range::new(0, s.sample_rate.samples(16.0)), 1024)
}

/// The Bass is in the mix: muting it changes the mix.
fn assert_bass_heard(s: &Session) {
    let mut muted = s.clone();
    let bass = muted.tracks.iter_mut().find(|t| t.name == "Bass").unwrap();
    bass.mixer.mute = true;
    assert!(worst(&mix(s), &mix(&muted)) > 1e-3, "the Bass is heard");
}

fn thawed(s: &Session) -> Session {
    let mut s = s.clone();
    for t in &mut s.tracks {
        t.frozen = false;
        t.freeze = None;
    }
    s
}

fn worst(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
    assert_eq!(a.len(), b.len(), "channels");
    a.iter().zip(b).fold(0.0f32, |w, (x, y)| {
        assert_eq!(x.len(), y.len(), "length");
        x.iter().zip(y).fold(w, |w, (p, q)| w.max((p - q).abs()))
    })
}

fn assert_close(a: &[Vec<f32>], b: &[Vec<f32>], what: &str) {
    let d = worst(a, b);
    assert!(d < 1e-4, "{what}: differs by {d}");
    assert!(a.iter().flatten().any(|v| v.abs() > 1e-3), "{what}: silent");
}

/// The demo with the Bass chain emptied and rebuilt from `chain` (plugin, params); the new
/// modules' ids in order. The Verb aux is made inactive: its plate reverb's delay modulation runs
/// free, so it sounds a little different whenever the mix's delay compensation changes (a latent
/// module frozen, bypassed or removed alike), which is not what these tests measure.
fn bass_with(chain: &[(&str, Value)]) -> (Engine, Vec<u64>) {
    let mut e = soundcraft_engine::demo::demo_engine();
    let bass = id(&e, "Bass");
    e.session_mut().track_mut(bass).unwrap().mixer.inserts.clear();
    let verb = id(&e, "Verb");
    e.session_mut().track_mut(verb).unwrap().inactive = true;
    let ids = chain
        .iter()
        .map(|(plugin, params)| {
            let m = e.execute("mix.module_add", &json!({"track": "Bass", "plugin": plugin})).unwrap()["module"].as_u64().unwrap();
            if let Some(map) = params.as_object() {
                for (k, v) in map {
                    e.execute("mix.module_param", &json!({"track": "Bass", "module": m, "param": k, "value": v})).unwrap();
                }
            }
            m
        })
        .collect();
    (e, ids)
}

/// Gain (+4 dB), a working compressor, an EQ cut: three modules that each change the sound.
fn three() -> (Engine, Vec<u64>) {
    bass_with(&[("gain", json!({"gain": 4.0})), ("compressor", json!({"threshold": -30.0, "ratio": 4.0})), ("gain", json!({"gain": -2.0}))])
}

fn freeze_upto(e: &mut Engine, m: u64) -> Value {
    e.execute("track.freeze", &json!({"tracks": ["Bass"], "upto": m})).unwrap()
}

fn frozen_modules(e: &Engine) -> usize {
    e.session().track(id(e, "Bass")).unwrap().frozen_modules()
}

#[test]
fn a_partial_freeze_sounds_as_live_with_a_latent_module_before_or_after_the_point() {
    // A latent module after the point (live, compensated as usual) …
    let (mut e, m) = bass_with(&[("gain", json!({"gain": 4.0})), ("compressor", json!({"threshold": -30.0})), ("maximizer", json!({}))]);
    let live = mix(e.session());
    freeze_upto(&mut e, m[0]);
    assert_eq!(frozen_modules(&e), 1);
    assert_bass_heard(e.session());
    assert_close(&mix(e.session()), &live, "latent module after the point");

    // … and before it (its latency is in the render, removed as the mix removes it).
    let (mut e, m) = bass_with(&[("maximizer", json!({})), ("gain", json!({"gain": 4.0})), ("compressor", json!({"threshold": -30.0}))]);
    let live = mix(e.session());
    freeze_upto(&mut e, m[1]);
    assert_eq!(frozen_modules(&e), 2);
    assert_close(&mix(e.session()), &live, "latent module before the point");

    // The whole chain, latent module first.
    let (mut e, _) = bass_with(&[("maximizer", json!({})), ("gain", json!({"gain": 4.0}))]);
    let live = mix(e.session());
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    assert_close(&mix(e.session()), &live, "whole chain with a latent module");

    // Latent modules on both sides.
    let (mut e, m) = bass_with(&[("maximizer", json!({})), ("gain", json!({"gain": 4.0})), ("maximizer", json!({}))]);
    let live = mix(e.session());
    freeze_upto(&mut e, m[1]);
    assert_close(&mix(e.session()), &live, "latent modules on both sides");
}

#[test]
fn modules_after_the_point_stay_editable_and_are_heard() {
    let (mut e, m) = three();
    freeze_upto(&mut e, m[0]);
    let frozen = mix(e.session());
    let ok = |e: &mut Engine, cmd: &str, p: Value| e.execute(cmd, &p).unwrap_or_else(|err| panic!("{cmd}: {err}"));
    ok(&mut e, "mix.module_param", json!({"track": "Bass", "module": m[1], "param": "ratio", "value": 10.0}));
    ok(&mut e, "mix.module_param", json!({"track": "Bass", "module": m[2], "param": "gain", "value": -8.0}));
    let edited = mix(e.session());
    assert!(worst(&edited, &frozen) > 1e-3, "the edit is heard");
    assert_close(&edited, &mix(&thawed(e.session())), "edited after the point");
    ok(&mut e, "mix.module_bypass", json!({"track": "Bass", "module": m[2]}));
    ok(
        &mut e,
        "automation.write_range",
        json!({"track": "Bass", "param": format!("plugin:m{}:gain", m[2]), "start": 0, "end": 48000, "value": -6.0}),
    );
    let added = ok(&mut e, "mix.module_add", json!({"track": "Bass", "plugin": "eq_7band"}))["module"].as_u64().unwrap();
    ok(&mut e, "mix.module_add", json!({"track": "Bass", "plugin": "gain", "at": 1}));
    ok(&mut e, "mix.module_move", json!({"track": "Bass", "module": added, "to": 1}));
    ok(&mut e, "mix.module_remove", json!({"track": "Bass", "module": m[2]}));
    assert!(e.session().track(id(&e, "Bass")).unwrap().frozen_audio().is_some(), "still frozen");
    assert_eq!(frozen_modules(&e), 1);
    assert_close(&mix(e.session()), &mix(&thawed(e.session())), "after all the edits");
}

#[test]
fn edits_at_or_before_the_point_are_refused() {
    let (mut e, m) = three();
    freeze_upto(&mut e, m[1]);
    let bass = id(&e, "Bass");
    let before = e.session().track(bass).unwrap().clone();
    let undo = e.undo_history().len();
    for (cmd, p) in [
        ("mix.module_param", json!({"track": "Bass", "module": m[1], "param": "ratio", "value": 10.0})),
        ("mix.module_param", json!({"track": "Bass", "module": m[0], "param": "gain", "value": 9.0})),
        ("mix.module_bypass", json!({"track": "Bass", "module": m[0]})),
        ("mix.module_remove", json!({"track": "Bass", "module": m[1]})),
        ("automation.write_range", json!({"track": "Bass", "param": format!("plugin:m{}:gain", m[0]), "start": 0, "end": 48000, "value": -6.0})),
        // Inserting before or at the point (at the point pushes it down).
        ("mix.module_add", json!({"track": "Bass", "plugin": "gain", "at": 0})),
        ("mix.module_add", json!({"track": "Bass", "plugin": "gain", "at": 1})),
        // Moving across the point, either way.
        ("mix.module_move", json!({"track": "Bass", "module": m[2], "to": 0})),
        ("mix.module_move", json!({"track": "Bass", "module": m[0], "to": 2})),
        ("mix.trim", json!({"track": "Bass", "db": 3.0})),
    ] {
        let err = e.execute(cmd, &p).expect_err(cmd).to_string();
        assert!(err.contains("Unfreeze the track to edit it"), "{cmd} {p}: {err}");
    }
    assert_eq!(e.session().track(bass).unwrap(), &before, "nothing changed");
    assert_eq!(e.undo_history().len(), undo, "no undo steps");
    // Inserting right after the point is fine.
    e.execute("mix.module_add", &json!({"track": "Bass", "plugin": "gain", "at": 2})).unwrap();
}

#[test]
fn moving_the_point_re_renders_as_one_undo_step() {
    let (mut e, m) = three();
    let live = mix(e.session());
    let bass = id(&e, "Bass");
    let fz = |e: &Engine| e.session().track(bass).unwrap().freeze.unwrap();
    let steps = e.undo_history().len();
    freeze_upto(&mut e, m[0]);
    let first = fz(&e);
    assert_eq!(first.upto.map(|x| u64::from(x.0)), Some(m[0]));
    assert_eq!(e.undo_history().len(), steps + 1);

    let r = freeze_upto(&mut e, m[1]);
    assert_eq!(r["frozen"].as_array().map(Vec::len), Some(1), "{r}");
    let second = fz(&e);
    assert_eq!(second.upto.map(|x| u64::from(x.0)), Some(m[1]));
    assert_ne!(second.source, first.source, "a new render");
    assert!(!e.session().sources.iter().any(|x| x.id == first.source), "the old render left");
    assert_eq!(e.undo_history().len(), steps + 2, "one undo step");
    assert_eq!(frozen_modules(&e), 2);
    assert_close(&mix(e.session()), &live, "moved point");

    // The same point again changes nothing.
    freeze_upto(&mut e, m[1]);
    assert_eq!(e.undo_history().len(), steps + 2);

    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(fz(&e), first, "undo puts the point back");
    assert!(e.session().pool.get(first.source).is_some());
    assert_close(&mix(e.session()), &live, "undone");
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(fz(&e), second, "redo moves it again");

    // Freeze whole chain on a partly frozen track; then the toggle thaws as before.
    e.execute("track.freeze", &json!({"tracks": ["Bass"], "freeze": true})).unwrap();
    assert_eq!(fz(&e).upto, None);
    assert_eq!(frozen_modules(&e), 3);
    assert_close(&mix(e.session()), &live, "whole chain");
    e.execute("track.freeze", &json!({"tracks": ["Bass"]})).unwrap();
    assert!(!e.session().track(bass).unwrap().frozen);
    // A point that is not on the track is refused.
    let err = e.execute("track.freeze", &json!({"tracks": ["Bass"], "upto": 999})).unwrap_err().to_string();
    assert!(err.contains("no module 999"), "{err}");
}

#[test]
fn sidechain_warnings_name_only_frozen_modules() {
    let (mut e, m) = three();
    let bass = id(&e, "Bass");
    let shale = e.execute("mix.module_add", &json!({"track": "Bass", "plugin": "fs_shale"})).unwrap()["module"].as_u64().unwrap();
    e.execute("mix.module_sidechain", &json!({"track": "Bass", "module": shale, "source": "Kick"})).unwrap();
    let r = freeze_upto(&mut e, m[0]);
    assert_eq!(r["warnings"].as_array().map(Vec::len), Some(0), "a live module still hears its key: {r}");
    e.execute("track.unfreeze", &json!({"tracks": ["Bass"]})).unwrap();
    let r = freeze_upto(&mut e, shale);
    assert!(r["warnings"][0].as_str().unwrap().contains("sidechain key from `Kick`"), "{r}");
    assert!(e.session().track(bass).unwrap().frozen_audio().is_some());
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-freeze-upto-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_point_saves_and_loads_and_a_lost_point_loads_thawed() {
    let (mut e, m) = three();
    freeze_upto(&mut e, m[1]);
    let frozen = mix(e.session());
    let dir = scratch("save");
    let path = dir.join("Song.scraft");
    soundcraft_engine::io::save_session(&mut e, path.to_str().unwrap()).unwrap();
    let mut back = Engine::default();
    let missing = soundcraft_engine::io::open_session(&mut back, path.to_str().unwrap()).unwrap();
    assert!(missing.is_empty(), "{missing:?}");
    let tr = back.session().track(id(&back, "Bass")).unwrap();
    assert_eq!(tr.frozen_audio().and_then(|f| f.upto).map(|x| u64::from(x.0)), Some(m[1]));
    assert_eq!(tr.frozen_modules(), 2);
    assert_close(&mix(back.session()), &frozen, "loaded");
    let _ = std::fs::remove_dir_all(dir);

    // A point naming no module of the chain (a hand-edited file): the track loads thawed.
    let mut s = e.session().clone();
    let bass = id(&e, "Bass");
    s.track_mut(bass).unwrap().mixer.inserts.remove(1);
    let notes = soundcraft_engine::io::thaw_missing_renders(&mut s);
    assert!(notes.iter().any(|n| n.contains("Bass") && n.contains("unfrozen")), "{notes:?}");
    assert!(s.track(bass).unwrap().frozen_audio().is_none());
}

#[test]
fn a_duplicate_without_its_modules_plays_live_and_blocks_nothing() {
    let (mut e, m) = three();
    freeze_upto(&mut e, m[1]);
    let r = e.execute("track.duplicate", &json!({"tracks": ["Bass"], "inserts": false})).unwrap();
    let dup = TrackId(r["tracks"][0].as_u64().unwrap());
    assert!(e.session().track(dup).unwrap().frozen_audio().is_none(), "the copy has no chain to match its render: it plays live");
    // Unrelated edits anywhere stay possible.
    e.execute("mix.volume", &json!({"track": "Kick", "db": -2.0})).unwrap();
    e.execute("track.rename", &json!({"track": "Kick", "name": "Kick In"})).unwrap();
    // A full duplicate keeps the render (it sounds the same).
    let r = e.execute("track.duplicate", &json!({"tracks": ["Bass"]})).unwrap();
    let full = TrackId(r["tracks"][0].as_u64().unwrap());
    assert_eq!(e.session().track(full).unwrap().frozen_audio(), e.session().track(id(&e, "Bass")).unwrap().frozen_audio());
}

#[test]
fn a_frozen_track_with_a_lost_point_does_not_block_other_edits() {
    let (mut e, m) = three();
    freeze_upto(&mut e, m[1]);
    // However it came about (a hand-edited file loaded without the load check), a point naming
    // no module only blocks edits to that track, as a whole-chain freeze would.
    let bass = id(&e, "Bass");
    e.session_mut().track_mut(bass).unwrap().mixer.inserts.remove(1);
    e.execute("mix.volume", &json!({"track": "Kick", "db": -2.0})).unwrap();
    e.execute("mix.volume", &json!({"track": "Bass", "db": -2.0})).unwrap();
    let err = e.execute("mix.module_param", &json!({"track": "Bass", "module": m[2], "param": "gain", "value": 3.0})).unwrap_err().to_string();
    assert!(err.contains("Unfreeze the track to edit it"), "{err}");
}

#[test]
fn bypass_all_on_a_partly_frozen_track_bypasses_the_live_modules() {
    let (mut e, m) = three();
    freeze_upto(&mut e, m[0]);
    e.execute("track.bypass_inserts", &json!({"tracks": ["Bass"], "bypass": true})).unwrap();
    let ins = &e.session().track(id(&e, "Bass")).unwrap().mixer.inserts;
    assert_eq!(ins.iter().map(|i| i.bypass).collect::<Vec<_>>(), [false, true, true]);
    e.execute("track.bypass_inserts_ae", &json!({"tracks": ["Bass"], "bypass": false})).unwrap();
    let ins = &e.session().track(id(&e, "Bass")).unwrap().mixer.inserts;
    assert_eq!(ins.iter().map(|i| i.bypass).collect::<Vec<_>>(), [false, false, false]);
}
