use serde_json::json;
use soundcraft_engine::{Engine, TransportRequest, TransportStatus};
use soundcraft_model::Session;
use soundcraft_time::Range;

#[test]
fn replacing_session_resets_status_and_discards_old_transport_requests() {
    let mut engine = Engine::default();
    engine.transport = TransportStatus { playing: true, recording: true, position: 48_000 };
    engine.execute("transport.play", &json!({"from": 96_000})).unwrap();
    engine.execute("transport.record", &json!({})).unwrap();
    let mut session = Session::default();
    session.edit.selection = Range::point(12_000);
    engine.replace_session(session);
    assert_eq!(engine.transport, TransportStatus { position: 12_000, ..TransportStatus::default() });
    assert_eq!(engine.transport_requests, vec![TransportRequest::ResetSession]);
    assert!(!engine.is_dirty());
}

#[test]
fn failed_new_session_keeps_the_current_transport() {
    let mut engine = Engine::default();
    engine.transport = TransportStatus { playing: true, position: 48_000, ..TransportStatus::default() };
    engine.execute("transport.play", &json!({})).unwrap();
    let status = engine.transport;
    let requests = engine.transport_requests.clone();
    assert!(engine.execute("session.new", &json!({"sample_rate": 0})).is_err());
    assert_eq!(engine.transport, status);
    assert_eq!(engine.transport_requests, requests);
}

#[test]
fn dynamic_transport_keeps_the_loop_and_play_start_apart_from_the_selection() {
    let mut engine = Engine::default();
    engine.execute("edit.select", &json!({"start": 1_000, "end": 2_000})).unwrap();
    assert_eq!(engine.session().edit.loop_range, Some(Range::new(1_000, 2_000)), "off: the selection is the loop");
    engine.execute("options.dynamic_transport", &json!({"value": true})).unwrap();
    engine.execute("edit.select", &json!({"start": 5_000, "end": 9_000})).unwrap();
    assert_eq!(engine.session().edit.loop_range, Some(Range::new(1_000, 2_000)), "on: a range selection leaves the loop");
    assert_eq!(soundcraft_engine::cmd::transport::play_start(engine.session()), None, "no marker yet");
    engine.execute("transport.set_play_start", &json!({"at": 7_000})).unwrap();
    assert_eq!(soundcraft_engine::cmd::transport::play_start(engine.session()), Some(7_000));
    assert_eq!(engine.transport.position, 7_000, "stopped, the playhead goes to the marker");
    assert_eq!(engine.session().edit.selection, Range::new(5_000, 9_000), "the selection stays");
    // Without `at`, the marker goes to the selection's start; Set Loop takes the selection.
    engine.execute("transport.set_play_start", &json!({})).unwrap();
    assert_eq!(soundcraft_engine::cmd::transport::play_start(engine.session()), Some(5_000));
    engine.execute("loop.set", &json!({})).unwrap();
    assert_eq!(engine.session().edit.loop_range, Some(Range::new(5_000, 9_000)));
    // Off again: the marker is not used.
    engine.execute("options.dynamic_transport", &json!({"value": false})).unwrap();
    assert_eq!(soundcraft_engine::cmd::transport::play_start(engine.session()), None);
}

#[test]
fn skip_ranges_join_and_clear() {
    let mut engine = Engine::default();
    assert!(engine.execute("transport.skip_add", &json!({})).is_err(), "an insertion point is no range");
    engine.execute("transport.skip_add", &json!({"start": 1_000, "end": 2_000})).unwrap();
    engine.execute("transport.skip_add", &json!({"start": 5_000, "end": 6_000})).unwrap();
    // Touching the first: joined with it.
    engine.execute("transport.skip_add", &json!({"start": 2_000, "end": 3_000})).unwrap();
    assert_eq!(engine.session().edit.skips, vec![Range::new(1_000, 3_000), Range::new(5_000, 6_000)]);
    engine.execute("transport.skip_clear", &json!({"at": 5_500})).unwrap();
    assert_eq!(engine.session().edit.skips, vec![Range::new(1_000, 3_000)]);
    engine.execute("transport.skip_clear", &json!({})).unwrap();
    assert!(engine.session().edit.skips.is_empty());
}

#[test]
fn audition_plays_around_the_selection_edges_once() {
    let mut engine = Engine::default();
    let sr = engine.session().sample_rate;
    let two = sr.samples(2.0);
    engine.session_mut().edit.selection = Range::new(5 * two, 8 * two);
    engine.session_mut().edit.loop_playback = true;
    engine.session_mut().edit.pre_roll = two / 2;
    engine.session_mut().edit.post_roll = 0;
    for (cmd, from, end) in [
        ("transport.play_to_start", 5 * two - two / 2, 5 * two),
        ("transport.play_from_start", 5 * two, 6 * two),
        ("transport.play_to_end", 8 * two - two / 2, 8 * two),
        ("transport.play_from_end", 8 * two, 9 * two),
    ] {
        engine.transport_requests.clear();
        let r = engine.execute(cmd, &json!({})).unwrap();
        assert_eq!(r, json!({"from": from, "end": end}), "{cmd}");
        // Once, never looped, whatever Loop Playback says.
        assert_eq!(engine.transport_requests, vec![TransportRequest::PlayRange { from, end: Some(end), looped: None }], "{cmd}");
    }
    // Before the session start the pre-roll is cut at zero; at zero there is nothing to play.
    engine.session_mut().edit.selection = Range::point(0);
    assert!(engine.execute("transport.play_to_start", &json!({})).is_err());
    engine.session_mut().edit.selection = Range::point(100);
    engine.execute("transport.play_to_start", &json!({})).unwrap();
    assert_eq!(engine.transport_requests.last(), Some(&TransportRequest::PlayRange { from: 0, end: Some(100), looped: None }));
}
