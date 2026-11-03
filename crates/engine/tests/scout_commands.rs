//! `scout.*` commands: starting and stopping a capture, storing it, and the undoable edits.

use serde_json::{Value, json};
use soundcraft_engine::{Engine, TransportRequest};
use soundcraft_model::{ChannelFormat, ScoutMetrics, ScoutStrip, Session, TrackId, TrackKind};
use soundcraft_time::Range;

/// A session with two audio tracks ("Vox", "Gtr") and a master; Vox is selected.
fn engine() -> (Engine, TrackId, TrackId, TrackId) {
    let mut s = Session::default();
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let gtr = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
    let master = match s.master() {
        Some(m) => m.id,
        None => s.add_track(TrackKind::Master, ChannelFormat::Stereo, None),
    };
    s.edit.selected_tracks = vec![vox];
    (Engine::new(s), vox, gtr, master)
}

fn starts(e: &Engine) -> Vec<TransportRequest> {
    e.transport_requests.iter().filter(|r| matches!(r, TransportRequest::ScoutStart { .. })).cloned().collect()
}

fn strip(track: TrackId, name: &str) -> ScoutStrip {
    ScoutStrip {
        track,
        name: name.into(),
        kind: TrackKind::Audio,
        channel_type: "channel".into(),
        role: "Channel".into(),
        channels: 1,
        mixer: soundcraft_model::Mixer::new(ChannelFormat::Mono),
        automated: Vec::new(),
        base: ScoutMetrics::default(),
        post: ScoutMetrics::default(),
        note: None,
        target: None,
    }
}

fn store(e: &mut Engine, id: u64, track: TrackId, played: Range, loop_range: Option<Range>) -> Value {
    e.execute("scout.store", &json!({"capture": id, "strips": [strip(track, "Vox")], "played": [played], "loop_range": loop_range})).unwrap()
}

/// Starts a capture of `track`, stops it, and stores it; returns its id.
fn capture(e: &mut Engine, track: TrackId, played: Range, loop_range: Option<Range>) -> u64 {
    let id = e.execute("scout.capture", &json!({"action": "start", "strips": [track.0]})).unwrap()["capture"].as_u64().unwrap();
    e.execute("scout.capture", &json!({"action": "stop"})).unwrap();
    store(e, id, track, played, loop_range);
    id
}

#[test]
fn capture_start_uses_the_selection_and_reserves_an_id() {
    let (mut e, vox, _, master) = engine();
    let before = e.session().scout.next_id;
    let r = e.execute("scout.capture", &json!({"action": "start"})).unwrap();
    let id = r["capture"].as_u64().unwrap() as u32;
    assert_eq!(id, before);
    assert_eq!(e.session().scout.next_id, before + 1, "the id is reserved");
    assert_eq!(starts(&e), vec![TransportRequest::ScoutStart { capture: id, strips: vec![vox], start: None, end: None }]);
    assert_eq!(e.scout_capture, Some(id));
    assert!(!e.can_undo(), "starting a capture is not an undo step");

    // Stop: the request goes out and the engine is free again.
    e.execute("scout.capture", &json!({"action": "stop"})).unwrap();
    assert!(e.transport_requests.contains(&TransportRequest::ScoutStop));
    assert_eq!(e.scout_capture, None);

    // Named strips, the master by name; the master is included when selected too.
    e.transport_requests.clear();
    let r = e.execute("scout.capture", &json!({"action": "start", "strips": ["Gtr", "Master"]})).unwrap();
    let id2 = r["capture"].as_u64().unwrap() as u32;
    assert_eq!(id2, id + 1);
    let gtr = e.session().track_by_name("Gtr").unwrap().id;
    assert_eq!(starts(&e), vec![TransportRequest::ScoutStart { capture: id2, strips: vec![gtr, master], start: None, end: None }]);
    e.execute("scout.capture", &json!({"action": "stop"})).unwrap();

    e.transport_requests.clear();
    e.session_mut().edit.selected_tracks = vec![master, vox];
    e.execute("scout.capture", &json!({"action": "start"})).unwrap();
    match starts(&e).first() {
        // The selection in session order.
        Some(TransportRequest::ScoutStart { strips, .. }) => {
            let order: Vec<TrackId> = e.session().tracks.iter().map(|t| t.id).filter(|t| *t == master || *t == vox).collect();
            assert_eq!(strips, &order);
            assert_eq!(strips.len(), 2);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn capture_refuses_while_running_and_without_strips() {
    let (mut e, ..) = engine();
    e.execute("scout.capture", &json!({"action": "start"})).unwrap();
    let err = e.execute("scout.capture", &json!({"action": "start"})).unwrap_err().to_string();
    assert!(err.contains("already capturing"), "{err}");
    assert_eq!(starts(&e).len(), 1);
    e.execute("scout.capture", &json!({"action": "stop"})).unwrap();

    e.session_mut().edit.selected_tracks.clear();
    let next = e.session().scout.next_id;
    let err = e.execute("scout.capture", &json!({"action": "start"})).unwrap_err().to_string();
    assert!(err.contains("Select tracks"), "{err}");
    assert_eq!(e.session().scout.next_id, next, "a refused start reserves nothing");

    for bad in [json!({"action": "start", "strips": ["Nope"]}), json!({"action": "start", "strips": []}), json!({"action": "go"}), json!({})] {
        assert!(e.execute("scout.capture", &bad).is_err(), "{bad}");
    }
    assert_eq!(starts(&e).len(), 1);
}

#[test]
fn loop_capture_refuses_a_loop_over_30_seconds_or_none() {
    let (mut e, ..) = engine();
    // No loop yet: a point selection sets none (Loop Playback is not needed, the sticky loop spec).
    e.execute("edit.select", &json!({"start": 1000})).unwrap();
    let err = e.execute("scout.capture", &json!({"action": "start", "loop": true})).unwrap_err().to_string();
    assert!(err.contains("Set a loop first"), "{err}");
    // Longer than 30 s.
    e.execute("edit.select", &json!({"start": 0, "end": 30 * 48_000 + 1})).unwrap();
    let err = e.execute("scout.capture", &json!({"action": "start", "loop": true})).unwrap_err().to_string();
    assert!(err.contains("Loop is longer than 30 seconds; shorten it to capture it exactly"), "{err}");
    assert!(starts(&e).is_empty(), "no capture started");
    assert_eq!(e.scout_capture, None);
    // Exactly 30 s is fine.
    e.execute("edit.select", &json!({"start": 0, "end": 30 * 48_000})).unwrap();
    e.execute("scout.capture", &json!({"action": "start", "loop": true})).unwrap();
}

#[test]
fn loop_capture_starts_and_ends_on_the_loop() {
    let (mut e, vox, ..) = engine();
    // The loop stays after the selection shrinks to an insertion (the selected tracks stay), and
    // Loop Playback off does not matter.
    e.execute("edit.select", &json!({"start": 96_000, "end": 144_000})).unwrap();
    e.execute("edit.select", &json!({"start": 200_000})).unwrap();
    let r = e.execute("scout.capture", &json!({"action": "start", "loop": true})).unwrap();
    let id = r["capture"].as_u64().unwrap() as u32;
    assert_eq!(starts(&e), vec![TransportRequest::ScoutStart { capture: id, strips: vec![vox], start: Some(96_000), end: Some(144_000) }]);
}

#[test]
fn store_rename_goal_delete_are_undoable() {
    let (mut e, vox, ..) = engine();
    let id = e.execute("scout.capture", &json!({"action": "start"})).unwrap()["capture"].as_u64().unwrap();
    store(&mut e, id, vox, Range::new(0, 48_000), None);
    assert_eq!(e.undo_label(), Some("Scout Capture"));
    assert_eq!(e.scout_capture, None, "storing the running capture frees the engine");
    let c = &e.session().scout.captures[0];
    assert_eq!((c.id as u64, c.name.as_str(), c.strips.len(), c.sample_rate), (id, "Capture 1", 1, 48_000.0));
    assert!(!c.played_truncated);

    e.execute("scout.rename", &json!({"capture": id, "name": "Verse vox"})).unwrap();
    assert_eq!(e.session().scout.captures[0].name, "Verse vox");
    e.execute("scout.goal", &json!({"text": "Make the vocal sit"})).unwrap();
    assert_eq!(e.session().scout.goal, "Make the vocal sit");
    e.execute("scout.delete", &json!({"capture": id})).unwrap();
    assert!(e.session().scout.captures.is_empty());

    assert!(e.undo());
    assert_eq!(e.session().scout.captures.len(), 1);
    assert!(e.undo());
    assert_eq!(e.session().scout.goal, "");
    assert!(e.undo());
    assert_eq!(e.session().scout.captures[0].name, "Capture 1");
    assert!(e.undo());
    assert!(e.session().scout.captures.is_empty());

    // Unknown captures and hostile params are refused, never panic.
    for (c, p) in [
        ("scout.rename", json!({"capture": 999, "name": "x"})),
        ("scout.rename", json!({"capture": id})),
        ("scout.delete", json!({"capture": "x"})),
        ("scout.goal", json!({"text": 3})),
        ("scout.store", json!({"capture": 5, "strips": "no", "played": []})),
        ("scout.store", json!({"capture": -1, "strips": [], "played": []})),
        ("scout.guides", json!({"turn": 12345})),
        ("scout.answer", json!({"turn": "x"})),
    ] {
        assert!(e.execute(c, &p).is_err(), "{c} {p}");
    }
}

#[test]
fn store_names_by_marker_and_keeps_truncation() {
    let (mut e, vox, ..) = engine();
    e.session_mut().add_marker("Chorus", soundcraft_model::MarkerKind::Marker, 48_000, 48_000);
    let id = e.execute("scout.capture", &json!({"action": "start"})).unwrap()["capture"].as_u64().unwrap();
    // Stopping does not end its claim: it is stored after the rig comes back.
    e.execute("scout.capture", &json!({"action": "stop"})).unwrap();
    e.execute(
        "scout.store",
        &json!({"capture": id, "strips": [strip(vox, "Vox")], "played": [Range::new(48_000, 96_000)], "played_truncated": true}),
    )
    .unwrap();
    let c = &e.session().scout.captures[0];
    assert!(c.played_truncated);
    assert_eq!(c.name, "Chorus");
    assert!(e.session().scout.next_id as u64 > id, "the counter stays above every id in use");
    // Storing it again, or an id never started, is refused.
    for bad in [id, id + 7] {
        let err = e.execute("scout.store", &json!({"capture": bad, "strips": [], "played": []})).unwrap_err().to_string();
        assert!(err.contains("That capture belongs to another session or was never started"), "{err}");
    }
}

#[test]
fn a_capture_of_a_replaced_session_is_not_stored() {
    let (mut e, vox, ..) = engine();
    let id = e.execute("scout.capture", &json!({"action": "start"})).unwrap()["capture"].as_u64().unwrap();
    let mut s = Session::default();
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    e.replace_session(s);
    assert_eq!(e.scout_capture, None);
    let err = e.execute("scout.store", &json!({"capture": id, "strips": [strip(vox, "Vox")], "played": []})).unwrap_err().to_string();
    assert!(err.contains("That capture belongs to another session or was never started"), "{err}");
    assert!(e.session().scout.captures.is_empty());
}

#[test]
fn a_pending_capture_keeps_its_id_through_an_undo() {
    let (mut e, vox, ..) = engine();
    e.execute("scout.goal", &json!({"text": "a"})).unwrap();
    let first = e.execute("scout.capture", &json!({"action": "start"})).unwrap()["capture"].as_u64().unwrap();
    e.execute("scout.capture", &json!({"action": "stop"})).unwrap();
    // Undoing the goal restores the snapshot taken before the id was reserved.
    assert!(e.undo());
    let second = e.execute("scout.capture", &json!({"action": "start"})).unwrap()["capture"].as_u64().unwrap();
    assert_ne!(first, second, "a pending id is never handed out again");
    store(&mut e, first, vox, Range::new(0, 10), None);
    store(&mut e, second, vox, Range::new(0, 10), None);
    assert_eq!(e.session().scout.captures.len(), 2);
}

#[test]
fn stop_with_nothing_running_does_nothing() {
    let (mut e, ..) = engine();
    let r = e.execute("scout.capture", &json!({"action": "stop"})).unwrap();
    assert_eq!(r, json!({"capture": null}));
    assert!(e.transport_requests.is_empty());
}

#[test]
fn same_section_is_detected() {
    let (mut e, vox, ..) = engine();
    let lr = Range::new(96_000, 144_000);
    let first = capture(&mut e, vox, lr, Some(lr));
    capture(&mut e, vox, Range::new(0, 1000), None);
    capture(&mut e, vox, lr, Some(lr));
    let l = e.execute("scout.list", &json!({})).unwrap();
    let caps = l["captures"].as_array().unwrap();
    assert_eq!(caps[0]["same_as"], Value::Null);
    assert_eq!(caps[1]["same_as"], Value::Null);
    assert_eq!(caps[2]["same_as"], json!(first));
    assert!(!serde_json::to_string(&e.session().scout).unwrap().contains("same_as"));
}

#[test]
fn answers_and_guides_are_undoable() {
    let (mut e, ..) = engine();
    let r = e.execute("scout.answer", &json!({"turn": {"question": "Too bright?", "captures": [1], "answer": "Yes.", "model": "m"}})).unwrap();
    let turn = r["turn"].as_u64().unwrap();
    assert_eq!(e.undo_label(), Some("Scout Answer"));
    e.execute("scout.guides", &json!({"turn": turn})).unwrap();
    assert_eq!(e.session().scout.guides_shown, Some(turn as u32));
    e.execute("scout.guides", &json!({"turn": null})).unwrap();
    assert_eq!(e.session().scout.guides_shown, None);
    assert!(e.undo());
    assert_eq!(e.session().scout.guides_shown, Some(turn as u32));
    assert!(e.undo());
    assert!(e.undo());
    assert!(e.session().scout.turns.is_empty());
}

#[test]
fn capture_ids_are_never_reused_after_an_undo_or_a_new_session() {
    let (mut e, vox, ..) = engine();
    let first = capture(&mut e, vox, Range::new(0, 10), None);
    // Undoing the store lowers the session's counter; the engine's never goes back.
    assert!(e.undo());
    assert!(e.session().scout.captures.is_empty());
    let second = e.execute("scout.capture", &json!({"action": "start"})).unwrap()["capture"].as_u64().unwrap();
    assert!(second > first, "an undone capture's id is not handed out again ({second} after {first})");
    e.execute("scout.capture", &json!({"action": "stop"})).unwrap();
    // A replaced session starts its counter at 0; a rig of the old one still on its way back
    // must not match a capture the new session starts.
    let mut s = Session::default();
    let v = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    s.edit.selected_tracks = vec![v];
    e.replace_session(s);
    let third = e.execute("scout.capture", &json!({"action": "start"})).unwrap()["capture"].as_u64().unwrap();
    assert!(third > second, "{third} after {second}");
    // An answer takes from the same counter.
    let turn = e.execute("scout.answer", &json!({"turn": {"question": "q", "answer": "a"}})).unwrap()["turn"].as_u64().unwrap();
    assert!(turn > third);
}

/// The CleanQ module's own guides (`uiState/guides` in its stored state), as `(freq, note)`.
fn cleanq_guides(e: &Engine, t: TrackId, m: soundcraft_model::ModuleId) -> Vec<(f64, String)> {
    use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling_state::CeilingState;
    let ins = e.session().track(t).unwrap().mixer.module(m).unwrap().clone();
    let st = ins.state_bytes().and_then(|b| CeilingState::read(&b)).unwrap_or_default();
    st.guides.list.iter().map(|g| (g.freq_hz, g.note.clone())).collect()
}

#[test]
fn cleanq_presets_neither_save_nor_overwrite_scout_guides() {
    // Scout's guides are session data (`scout.turns[].guides`, shown by `scout.guides`), drawn on
    // the Clean Q graph and never written into the module. What the CleanQ editor's presets touch
    // is the module: its parameters and its own guides (`mix.module_params {preset}`, merged with
    // `mix.module_guides` on a recall or a paste; `mix.module_params` alone for A/B and a save's
    // rename). So a save carries only the module's own guides, and no recall, A/B, paste or undo
    // of one moves Scout's.
    let (mut e, vox, ..) = engine();
    let cq = e.session_mut().track_mut(vox).unwrap().mixer.add_module(soundcraft_model::Insert::new("fs_cleanq"), None).unwrap();
    let mine = json!({"track": vox.0, "module": cq.0, "guides": [{"freq": 3000.0, "lo": 2000.0, "hi": 4000.0, "note": "mine"}]});
    e.execute("mix.module_guides", &mine).unwrap();
    let eq = json!({"kind": "eq", "track": "Vox", "lo_hz": 200.0, "hi_hz": 400.0, "gain_lo_db": -4.0, "gain_hi_db": -2.0, "label": "mud"});
    let turn = e.execute("scout.answer", &json!({"turn": {"question": "Muddy?", "answer": "Yes.", "guides": [eq], "model": "m"}})).unwrap()["turn"]
        .as_u64()
        .unwrap();
    e.execute("scout.guides", &json!({"turn": turn})).unwrap();
    let scout = serde_json::to_value(&e.session().scout).unwrap();
    assert_eq!(e.session().scout.turns[0].guides.len(), 1);
    // What a save would write (the module's own guides): Scout's are not among them.
    assert_eq!(cleanq_guides(&e, vox, cq), vec![(3000.0, "mine".to_string())]);
    // Recall a preset that carries guides, then one that carries none (the editor then writes no
    // guides): parameters and the module's guides change, Scout's stay.
    let recall = |e: &mut Engine, name: &str, guides: Option<Value>| {
        let p = json!({"track": vox.0, "module": cq.0, "params": {"outGain": 0.3}, "preset": name});
        e.execute_merged("mix.module_params", &p, "recall").unwrap();
        if let Some(g) = guides {
            e.execute_merged("mix.module_guides", &json!({"track": vox.0, "module": cq.0, "guides": g}), "recall").unwrap();
        }
        e.end_merge();
    };
    recall(&mut e, "Bright", Some(json!([{"freq": 8000.0, "note": "preset"}])));
    assert_eq!(cleanq_guides(&e, vox, cq), vec![(8000.0, "preset".to_string())]);
    recall(&mut e, "Plain", None);
    assert_eq!(cleanq_guides(&e, vox, cq), vec![(8000.0, "preset".to_string())]);
    // A/B and a save's rename: parameters only.
    e.execute("mix.module_params", &json!({"track": vox.0, "module": cq.0, "params": {"outGain": 0.6}})).unwrap();
    e.execute("mix.module_params", &json!({"track": vox.0, "module": cq.0, "params": {}, "preset": "Mine"})).unwrap();
    assert_eq!(serde_json::to_value(&e.session().scout).unwrap(), scout, "Scout's guides are untouched");
    // Undoing all of it brings the module back and still leaves Scout's guides shown.
    for _ in 0..4 {
        assert!(e.undo());
    }
    assert_eq!(cleanq_guides(&e, vox, cq), vec![(3000.0, "mine".to_string())]);
    assert_eq!(serde_json::to_value(&e.session().scout).unwrap(), scout);
    assert_eq!(e.session().scout.guides_shown, Some(turn as u32));
}
