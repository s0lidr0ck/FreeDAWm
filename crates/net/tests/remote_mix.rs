//! Remote mixing over a loopback port: the remote PC gets the host's session, a fader drag
//! there lands at the host as one undo step, the transport runs only at the host, editing is
//! refused on both ends, and the meters come across.
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use soundcraft_engine::{Engine, Relay};
use soundcraft_model::{TrackId, TrackKind};
use soundcraft_net::config::{NetConfig, PairedDevice};
use soundcraft_net::hub::{Hub, HubRequest};
use soundcraft_net::mixer::{Meter, MeterFrame, MixerClient, MixerTarget, REFUSED, Route, pieces, route};
use std::time::{Duration, Instant};

const TOKEN: &str = "mixer-token";

fn host() -> (Hub, String, Engine) {
    let mut cfg = NetConfig::default();
    cfg.paired.push(PairedDevice { name: "Mixer PC".into(), kind: "pc".into(), token: TOKEN.into(), follows: false });
    let hub = Hub::new(cfg, None);
    let port = hub.listen(0).unwrap();
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 3, "format": "Mono"})).unwrap();
    (hub, format!("127.0.0.1:{port}"), e)
}

/// One frame of the host app: answer the mixer's commands as the app's control handler does,
/// and publish the session when it changed.
fn host_frame(hub: &Hub, e: &mut Engine, published: &mut u64) {
    for r in hub.drain() {
        if let HubRequest::Control { method, params, reply } = r {
            assert_eq!(method, "engine.execute");
            let id = params["command"].as_str().unwrap_or("");
            let p = params.get("params").cloned().unwrap_or(json!({}));
            let got = match params.get("merge").and_then(Value::as_str) {
                Some(k) => e.execute_merged(id, &p, k),
                None => e.execute(id, &p),
            };
            let _ = reply.send(match got {
                Ok(v) => json!({"ok": true, "result": v}),
                Err(err) => json!({"ok": false, "error": err.to_string()}),
            });
        }
    }
    if *published != e.revision && !hub.mixers().is_empty() {
        hub.publish_mix(pieces(e.session()).unwrap());
        *published = e.revision;
    }
}

fn relayed(client: &MixerClient) -> Engine {
    let mut m = Engine::default();
    let c = client.clone();
    m.relay = Some(Box::new(move |id: &str, p: &Value, merge: Option<&str>| match route(id) {
        Route::Local => Relay::Local,
        Route::Refuse => Relay::Refuse(REFUSED.into()),
        Route::Both | Route::Remote if !c.send_command(id, p, merge) => Relay::Refuse("not connected".into()),
        Route::Both => Relay::Both,
        Route::Remote => Relay::Remote,
    }));
    m
}

/// Runs host frames until `done` (at most 10 s).
fn until(hub: &Hub, host: &mut Engine, published: &mut u64, mut done: impl FnMut(&mut Engine) -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        host_frame(hub, host, published);
        if done(host) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out");
}

fn first_track(e: &Engine) -> TrackId {
    e.session().tracks.iter().find(|t| t.kind != TrackKind::Master).unwrap().id
}

#[test]
fn a_remote_pc_mixes_the_host() {
    let (hub, addr, mut host) = host();
    let mut published = 0;
    let client = MixerClient::start(MixerTarget { host_name: "Booth".into(), addr, token: TOKEN.into(), my_name: "Mixer PC".into() });
    let mut mirror = relayed(&client);
    // The host's session arrives.
    until(&hub, &mut host, &mut published, |_| {
        if let Some(s) = client.take_session() {
            mirror.mirror_session(s.unwrap());
            true
        } else {
            false
        }
    });
    assert_eq!(hub.mixers(), ["Mixer PC"]);
    let names = |e: &Engine| e.session().tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&mirror), names(&host));
    let t = first_track(&host);

    // A fader drag on the remote PC: it moves there at once and lands at the host as one step.
    let steps = host.undo_history().len();
    for db in [-1.0, -3.0, -6.0] {
        mirror.execute_merged("mix.volume", &json!({"track": t.0, "db": db}), "fader").unwrap();
        assert_eq!(mirror.session().track(t).unwrap().mixer.volume_db, db as f32);
    }
    until(&hub, &mut host, &mut published, |h| h.session().track(t).unwrap().mixer.volume_db == -6.0);
    assert_eq!(host.undo_history().len(), steps + 1, "a remote drag is one undo step at the host");

    // A change made at the host comes back to the remote PC.
    host.execute("mix.pan", &json!({"track": t.0, "pan": -0.5})).unwrap();
    until(&hub, &mut host, &mut published, |_| {
        if let Some(s) = client.take_session() {
            mirror.mirror_session(s.unwrap());
        }
        mirror.session().track(t).unwrap().mixer.pan.first() == Some(&-0.5)
    });
    assert_eq!(mirror.session().track(t).unwrap().mixer.volume_db, -6.0, "the remote drag stays");

    // The transport runs only at the host; editing is refused on the remote PC.
    assert_eq!(mirror.execute("transport.play", &json!({})).unwrap(), json!({"relayed": true}));
    assert!(mirror.transport_requests.is_empty());
    until(&hub, &mut host, &mut published, |h| !h.transport_requests.is_empty());
    let err = mirror.execute("track.new", &json!({"count": 1})).unwrap_err().to_string();
    assert!(err.contains("only mix, automation and transport"), "{err}");
    assert!(!mirror.is_dirty(), "the copy is the host's, never this PC's to save");

    // The meters and transport come across.
    hub.publish_meters(&MeterFrame {
        tracks: vec![Meter { id: t.0, l: 0.5, r: 0.25, gr: 0.0 }],
        main: Meter { id: 0, l: 0.7, r: 0.7, gr: 0.0 },
        playing: true,
        recording: false,
        position: 48_000,
    });
    until(&hub, &mut host, &mut published, |_| client.meters(Duration::ZERO).is_some_and(|f| f.playing && f.position == 48_000));
    client.stop();
    hub.stop();
}

#[test]
fn a_mixer_cannot_run_anything_but_mix_and_transport_at_the_host() {
    let (hub, addr, mut host) = host();
    let client = MixerClient::start(MixerTarget { host_name: "Booth".into(), addr, token: TOKEN.into(), my_name: "Mixer PC".into() });
    let mut published = 0;
    until(&hub, &mut host, &mut published, |_| client.connected());
    // Sent around the relay, as a modified client would.
    let tracks = host.session().tracks.len();
    assert!(client.send_command("track.delete", &json!({"tracks": [first_track(&host).0]}), None));
    assert!(client.send_command("session.save", &json!({}), None));
    let mut errors = Vec::new();
    until(&hub, &mut host, &mut published, |_| {
        errors.extend(client.take_errors());
        errors.len() == 2
    });
    assert!(errors.iter().all(|e| e.contains("a remote mixer can't run")), "{errors:?}");
    assert_eq!(host.session().tracks.len(), tracks);
    client.stop();
    hub.stop();
}

#[test]
fn an_unpaired_pc_is_not_let_in() {
    let (hub, addr, mut host) = host();
    let client = MixerClient::start(MixerTarget { host_name: "Booth".into(), addr, token: "guess".into(), my_name: "Stranger".into() });
    let mut published = 0;
    until(&hub, &mut host, &mut published, |_| client.status() == "not paired");
    assert!(hub.mixers().is_empty());
    assert!(!client.send_command("mix.volume", &json!({"db": 0.0}), None));
    client.stop();
    hub.stop();
}

/// Over the internet transport (iroh, direct on this machine, no relay): a mixer's invite pairs
/// the remote PC, which mixes the host through a local bridge; the host knows it is an internet
/// mixer and how late its moves arrive, and its moves land.
#[test]
fn a_remote_pc_mixes_the_host_over_the_internet_with_an_invite() {
    use soundcraft_net::internet::{Internet, Invite, ROLE_MIX, Relays, new_key};
    let (hub, _, mut host) = host();
    let mut published = 0;
    let inet = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    inet.serve_to(hub.listen_internet().unwrap());
    let code =
        Invite { addr: inet.addr(Duration::from_secs(1)), secret: hub.new_invite_secret(), name: "Booth".into(), role: ROLE_MIX.into() }.code();
    // The mixer pastes it.
    let got = Invite::parse(&code).unwrap();
    assert_eq!(got.role, ROLE_MIX);
    let mine = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    let bridge = mine.bridge_to(got.addr.clone()).unwrap().to_string();
    let (token, booth) = soundcraft_net::follower::pair_invite(&bridge, &got.secret, "Home PC").unwrap();
    let client = MixerClient::start(MixerTarget { host_name: booth, addr: bridge, token, my_name: "Home PC".into() });
    let mut mirror = relayed(&client);
    until(&hub, &mut host, &mut published, |_| {
        if let Some(s) = client.take_session() {
            mirror.mirror_session(s.unwrap());
            true
        } else {
            false
        }
    });
    let links = hub.mixer_links();
    assert_eq!(links.len(), 1);
    assert!(links[0].internet, "the host knows this mixer is across the internet");
    let t = first_track(&host);
    for db in [-2.0, -4.0] {
        mirror.execute_merged("mix.volume", &json!({"track": t.0, "db": db}), "fader").unwrap();
    }
    until(&hub, &mut host, &mut published, |h| h.session().track(t).unwrap().mixer.volume_db == -4.0);
    // It reports how late its moves arrive (its round trip, without the stream).
    until(&hub, &mut host, &mut published, |_| hub.mixer_delay() > Duration::ZERO);
    assert!(client.rtt_ms().is_some());
    assert!(hub.mixer_delay() < Duration::from_millis(500), "{:?}", hub.mixer_delay());
    client.stop();
    until(&hub, &mut host, &mut published, |_| hub.mixers().is_empty());
    assert_eq!(hub.mixer_delay(), Duration::ZERO);
    hub.stop();
}
