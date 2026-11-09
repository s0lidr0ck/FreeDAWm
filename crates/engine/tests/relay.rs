//! Remote mixing's relay (`Engine::relay`): each command runs here, at the host, both, or is
//! refused; a merged gesture tells the relay its key; a mirrored document keeps this PC's view.

use serde_json::{Value, json};
use soundcraft_engine::{Engine, Relay};
use std::sync::{Arc, Mutex};

type Sent = Arc<Mutex<Vec<(String, Value, Option<String>)>>>;

fn relayed(e: &mut Engine) -> Sent {
    let sent: Sent = Arc::default();
    let log = Arc::clone(&sent);
    e.relay = Some(Box::new(move |id, p, merge| {
        let d = if id.starts_with("transport.") {
            Relay::Remote
        } else if id.starts_with("mix.") {
            Relay::Both
        } else if id == "track.new" {
            Relay::Refuse("not while mixing remotely".into())
        } else {
            return Relay::Local;
        };
        log.lock().unwrap().push((id.to_string(), p.clone(), merge.map(str::to_string)));
        d
    }));
    sent
}

#[test]
fn the_relay_decides_where_each_command_runs() {
    let mut e = Engine::default();
    let t = e.execute("track.new", &json!({"count": 1, "format": "Mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    let sent = relayed(&mut e);
    // Both: here at once, and sent.
    e.execute("mix.volume", &json!({"track": t, "db": -6.0})).unwrap();
    assert_eq!(e.session().tracks.iter().find(|x| x.id.0 == t).unwrap().mixer.volume_db, -6.0);
    // Remote: only the host runs it.
    let v = e.execute("transport.play", &json!({})).unwrap();
    assert_eq!(v, json!({"relayed": true}));
    assert!(e.transport_requests.is_empty(), "the remote PC's transport does not start");
    // Refused, with the reason.
    let err = e.execute("track.new", &json!({"count": 1})).unwrap_err().to_string();
    assert!(err.contains("not while mixing remotely"), "{err}");
    let ids: Vec<String> = sent.lock().unwrap().iter().map(|s| s.0.clone()).collect();
    assert_eq!(ids, ["mix.volume", "transport.play", "track.new"]);
}

#[test]
fn a_merged_gesture_tells_the_relay_its_key() {
    let mut e = Engine::default();
    let t = e.execute("track.new", &json!({"count": 1, "format": "Mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    let sent = relayed(&mut e);
    for db in [-1.0, -2.0, -3.0] {
        e.execute_merged("mix.volume", &json!({"track": t, "db": db}), "fader").unwrap();
    }
    e.execute("mix.volume", &json!({"track": t, "db": 0.0})).unwrap();
    let keys: Vec<Option<String>> = sent.lock().unwrap().iter().map(|s| s.2.clone()).collect();
    assert_eq!(keys, [Some("fader".into()), Some("fader".into()), Some("fader".into()), None]);
}

#[test]
fn a_mirrored_document_keeps_the_view_and_is_not_modified() {
    let mut host = Engine::default();
    host.execute("track.new", &json!({"count": 2, "format": "Mono"})).unwrap();
    let t = host.session().tracks.iter().find(|x| x.kind != soundcraft_model::TrackKind::Master).unwrap().id;
    host.execute("mix.volume", &json!({"track": t.0, "db": -9.0})).unwrap();
    let mut remote = Engine::default();
    let zoom = remote.session().edit.zoom;
    let rev = remote.revision;
    remote.mirror_session((*host.session_arc()).clone());
    assert_ne!(remote.revision, rev);
    assert!(!remote.is_dirty());
    assert_eq!(remote.session().edit.zoom, zoom);
    assert_eq!(remote.session().track(t).unwrap().mixer.volume_db, -9.0);
    assert!(!remote.can_undo());
}
