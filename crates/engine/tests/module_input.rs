//! A chain module's MIDI output as a track input (`track.input {input: "module:<track>/<module>"}`).

use serde_json::{Value, json};
use soundcraft_engine::{Engine, EngineError};
use soundcraft_model::{ChannelFormat, ModuleId, ModuleRef, Route, TrackId, TrackKind};

fn ok(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute(id, &p).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn refused(e: &mut Engine, id: &str, p: Value) -> String {
    let undo = e.undo_history().len();
    let err = match e.execute(id, &p) {
        Ok(v) => panic!("{id} {p} was accepted: {v}"),
        Err(err) => err,
    };
    assert!(!matches!(err, EngineError::Internal(..)), "{id} panicked: {err}");
    assert_eq!(e.undo_history().len(), undo, "{id} {p}: a refused edit is not in the history");
    err.to_string()
}

/// An audio track "Kick" with a Shale module, and an instrument track "Drums".
fn rig() -> (Engine, TrackId, ModuleId, TrackId) {
    let mut e = Engine::default();
    let kick = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Kick"));
    let drums = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Drums"));
    ok(&mut e, "mix.module_add", json!({"track": kick.0, "plugin": "fs_shale"}));
    let shale = e.session().track(kick).unwrap().mixer.inserts[0].id;
    (e, kick, shale, drums)
}

fn input(e: &Engine, t: TrackId) -> Route {
    e.session().track(t).unwrap().mixer.input.clone()
}

#[test]
fn module_input_parses_track_and_module_by_index_name_and_id() {
    let (mut e, kick, shale, drums) = rig();
    let want = Route::Modules(vec![ModuleRef { track: kick, module: shale }]);
    for spelling in
        ["module:Kick/1", "MODULE:Kick/Shale", "module:Kick/fs_shale", &format!("module:{}/#{}", kick.0, shale.0), " module:kick / shale "]
    {
        ok(&mut e, "track.input", json!({"tracks": [drums.0], "input": spelling}));
        assert_eq!(input(&e, drums), want, "{spelling}");
        ok(&mut e, "track.input", json!({"tracks": [drums.0], "input": "none"}));
    }
    assert_eq!(e.session().busses.len(), 0, "never a bus");
    // Undo puts the previous input back.
    ok(&mut e, "track.input", json!({"tracks": [drums.0], "input": "module:Kick/1"}));
    e.undo();
    assert_eq!(input(&e, drums), Route::None);
    e.redo();
    assert_eq!(input(&e, drums), want);
    // track.inspect shows it.
    let row = ok(&mut e, "track.inspect", json!({"track": drums.0}));
    assert_eq!(row["input"], json!({"Modules": [{"track": kick.0, "module": shale.0}]}), "{row}");
}

#[test]
fn several_modules_feed_one_track_as_a_list() {
    let (mut e, kick, shale, drums) = rig();
    let snare = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Snare"));
    ok(&mut e, "mix.module_add", json!({"track": snare.0, "plugin": "fs_shale"}));
    let snare_shale = e.session().track(snare).unwrap().mixer.inserts[0].id;
    let (a, b) = (ModuleRef { track: kick, module: shale }, ModuleRef { track: snare, module: snare_shale });
    ok(&mut e, "track.input", json!({"tracks": [drums.0], "input": ["module:Kick/Shale", "module:Snare/1", "module:Kick/1"]}));
    assert_eq!(input(&e, drums), Route::Modules(vec![a, b]), "in order, each once");
    let row = ok(&mut e, "track.inspect", json!({"track": drums.0}));
    assert_eq!(row["input"]["Modules"].as_array().map(Vec::len), Some(2));
    // A list holds modules only; an empty list is refused; one bad name refuses the whole call.
    for (bad, msg) in [
        (json!(["module:Kick/1", "musician:1"]), "a list of inputs holds module names"),
        (json!([]), "empty list"),
        (json!(["module:Kick/1", "module:Nope/1"]), "no track named"),
        (json!(["module:Kick/1", 7]), "not 7"),
    ] {
        let err = refused(&mut e, "track.input", json!({"tracks": [drums.0], "input": bad}));
        assert!(err.contains(msg), "{bad}: {err}");
    }
    assert_eq!(input(&e, drums), Route::Modules(vec![a, b]));
    // One of them feeding the track itself refuses the list.
    ok(&mut e, "mix.module_add", json!({"track": drums.0, "plugin": "fs_shale"}));
    let err = refused(&mut e, "track.input", json!({"tracks": [drums.0], "input": ["module:Kick/1", "module:Drums/1"]}));
    assert!(err.contains("feed a track into itself"), "{err}");
    // Undo restores the previous set.
    ok(&mut e, "track.input", json!({"tracks": [drums.0], "input": "module:Snare/1"}));
    assert_eq!(input(&e, drums), Route::Modules(vec![b]));
    e.undo();
    assert_eq!(input(&e, drums), Route::Modules(vec![a, b]));
}

#[test]
fn module_input_refusals() {
    let (mut e, kick, _shale, drums) = rig();
    let audio = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Other"));
    // Audio and aux tracks take no MIDI.
    let err = refused(&mut e, "track.input", json!({"tracks": [audio.0], "input": "module:Kick/1"}));
    assert!(err.contains("plays into a MIDI or instrument track"), "{err}");
    let err = refused(&mut e, "track.input", json!({"tracks": [drums.0, audio.0], "input": "module:Kick/1"}));
    assert!(err.contains("plays into a MIDI or instrument track"), "{err}");
    // Never an output.
    let err = refused(&mut e, "track.output", json!({"tracks": [drums.0], "output": "module:Kick/1"}));
    assert!(err.contains("can only be a track input"), "{err}");
    // A module feeding its own track.
    ok(&mut e, "mix.module_add", json!({"track": drums.0, "plugin": "fs_shale"}));
    let err = refused(&mut e, "track.input", json!({"tracks": [drums.0], "input": "module:Drums/1"}));
    assert!(err.contains("feed a track into itself"), "{err}");
    // A loop: Kick's Shale keyed from Drums, and Drums playing Kick's Shale.
    let shale = e.session().track(kick).unwrap().mixer.inserts[0].id;
    ok(&mut e, "mix.module_sidechain", json!({"track": kick.0, "module": shale.0, "source": "Drums"}));
    let err = refused(&mut e, "track.input", json!({"tracks": [drums.0], "input": "module:Kick/1"}));
    assert!(err.contains("feed a track into itself"), "{err}");
    ok(&mut e, "mix.module_sidechain", json!({"track": kick.0, "module": shale.0, "source": null}));
    // Unknown track, unknown module, a module with no MIDI output, a malformed name.
    ok(&mut e, "mix.module_add", json!({"track": kick.0, "plugin": "fs_cleanq"}));
    for (bad, msg) in [
        ("module:Nope/1", "no track named"),
        ("module:Kick/9", "has no module"),
        ("module:Kick/Tape", "has no module"),
        ("module:Kick/2", "has no MIDI output"),
        ("module:Kick/CleanQ", "has no MIDI output"),
        ("module:Kick", "module:<track>/<module>"),
    ] {
        let err = refused(&mut e, "track.input", json!({"tracks": [drums.0], "input": bad}));
        assert!(err.contains(msg), "{bad}: {err}");
    }
    assert_eq!(input(&e, drums), Route::None);
    // Freeze refuses a track playing a module live.
    ok(&mut e, "track.input", json!({"tracks": [drums.0], "input": "module:Kick/1"}));
    ok(&mut e, "mix.instrument", json!({"track": drums.0, "plugin": "subtractive_synth"}));
    let err = refused(&mut e, "track.freeze", json!({"tracks": [drums.0]}));
    assert!(err.contains("plays a module's MIDI live"), "{err}");
}

#[test]
fn old_sessions_and_hostile_inputs_load() {
    let (mut e, kick, shale, drums) = rig();
    ok(&mut e, "track.input", json!({"tracks": [drums.0], "input": "module:Kick/1"}));
    let text = serde_json::to_string(e.session()).unwrap();
    let mut s: soundcraft_model::Session = serde_json::from_str(&text).unwrap();
    s.sanitize();
    assert_eq!(s.track(drums).unwrap().mixer.input, Route::Modules(vec![ModuleRef { track: kick, module: shale }]));
    // Deleting the source track leaves the route pointing at nothing: it loads and plays silence.
    ok(&mut e, "track.delete", json!({"tracks": [kick.0]}));
    let text = serde_json::to_string(e.session()).unwrap();
    let s: soundcraft_model::Session = serde_json::from_str(&text).unwrap();
    assert!(s.track(drums).is_some());
}
