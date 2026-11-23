//! Connect to a Studio: a PC at home mixing a studio's live stream. An in-process studio (a real
//! hub, a real engine, the UI's part played by a loop as `crates/ui-egui/src/control.rs` and
//! `net_ui::publish_mixer` do it) and remote PCs dialling it by code, by booth PIN, by invite
//! over the internet bridge, and again from their stored pairing.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::surface::feed;
use soundcraft_net::config::NetConfig;
use soundcraft_net::follower::{pair_confirm, pair_request, pair_request_info};
use soundcraft_net::hub::{Hub, HubRequest};
use soundcraft_net::internet::{Internet, Invite, Relays, lan_addr, new_key};
use soundcraft_net::pairing::exchange::LinkKeys;
use soundcraft_net::studio::{Studio, StudioTarget};
use soundcraft_playback::stream::{MixTap, StreamIn};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The studio: its hub, its engine (behind the "UI thread" loop), its mix tap.
struct StudioPc {
    hub: Hub,
    addr: SocketAddr,
    engine: Arc<Mutex<Engine>>,
    tap: Arc<MixTap>,
    stop: Arc<AtomicBool>,
    ui: Option<std::thread::JoinHandle<()>>,
}

/// What the studio's UI thread does with one control request (as `control::handle`).
fn handle(e: &mut Engine, method: &str, p: &Value) -> Value {
    let ok = |v: Value| json!({"ok": true, "result": v});
    let err = |e: String| json!({"ok": false, "error": e});
    match method {
        "engine.execute" => {
            let id = p.get("command").and_then(Value::as_str).unwrap_or("");
            let params = p.get("params").cloned().unwrap_or(json!({}));
            let r = match p.get("gesture").and_then(Value::as_str) {
                Some(key) => e.execute_gesture(id, &params, key),
                None => e.execute(id, &params),
            };
            r.map_or_else(|x| err(x.to_string()), ok)
        }
        "engine.end_gesture" => {
            match p.get("gesture").and_then(Value::as_str) {
                Some(k) => e.end_gesture(k),
                None => e.end_merge(),
            }
            ok(json!({}))
        }
        _ => err(format!("unknown {method}")),
    }
}

/// Levels for every strip: a steady tone on each, so meters are there to decode.
fn meters_of(e: &Engine, level: f32) -> Value {
    let levels: Vec<(u64, feed::StripLevels)> = feed::feed_tracks(e.session())
        .map(|t| (t.id.0, feed::StripLevels { peak: [level, level], rms: [level * 0.5, level * 0.5], gr_db: 0.0 }))
        .collect();
    feed::meters_json(0.0, &levels)
}

impl StudioPc {
    fn new(cfg: NetConfig, engine: Engine) -> StudioPc {
        let hub = Hub::new(cfg, None);
        let port = hub.listen(0).unwrap();
        let tap = Arc::new(MixTap::default());
        hub.set_tap(Some(Arc::clone(&tap)), 48_000);
        let engine = Arc::new(Mutex::new(engine));
        let stop = Arc::new(AtomicBool::new(false));
        let ui = {
            let (hub, engine, stop) = (hub.clone(), Arc::clone(&engine), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut level = 0.1f32;
                while !stop.load(Ordering::SeqCst) {
                    {
                        let mut e = engine.lock().unwrap();
                        for r in hub.drain() {
                            if let HubRequest::Control { method, params, reply } = r {
                                let _ = reply.send(handle(&mut e, &method, &params));
                            }
                        }
                        // A frame with no pointer down on the studio.
                        e.end_idle_merge();
                        if hub.mixer_subscribers() > 0 {
                            hub.publish_mixer(feed::mixer_state(&e));
                            level = if level > 0.8 { 0.1 } else { level + 0.01 };
                            hub.publish_meters(meters_of(&e, level));
                        }
                    }
                    std::thread::sleep(Duration::from_millis(16));
                }
            })
        };
        StudioPc { hub, addr: SocketAddr::from(([127, 0, 0, 1], port)), engine, tap, stop, ui: Some(ui) }
    }

    /// A studio that takes PCs over the plain link (the default refuses them there).
    fn plain(engine: Engine) -> StudioPc {
        StudioPc::new(NetConfig { encrypted_only: false, ..NetConfig::default() }, engine)
    }

    /// Pairs a PC by the code on the studio's screen (plain link): its token.
    fn pair_by_code(&self, name: &str) -> String {
        let a = self.addr.to_string();
        let id = pair_request(&a, name).unwrap();
        let code = self.hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap().code;
        pair_confirm(&a, &id, code, LinkKeys::PLAIN).unwrap().0
    }

    fn engine(&self) -> std::sync::MutexGuard<'_, Engine> {
        self.engine.lock().unwrap()
    }

    #[allow(dead_code)]
    fn id(&self, name: &str) -> u64 {
        self.engine().session().tracks.iter().find(|t| t.name == name).unwrap().id.0
    }

    #[allow(dead_code)]
    fn volume(&self, id: u64) -> f32 {
        self.engine().session().tracks.iter().find(|t| t.id.0 == id).unwrap().mixer.volume_db
    }

    /// `n` blocks of a tone into the tap with the transport stopped, as the audio thread would.
    fn play_stopped(&self, n: usize) {
        let block = 480usize;
        for k in 0..n {
            let l: Vec<f32> = (0..block).map(|i| 0.3 * ((k * block + i) as f32 * 0.05).sin()).collect();
            self.tap.push_block(&l, &l, block, 0, 0, false);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for StudioPc {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.ui.take() {
            let _ = h.join();
        }
        self.hub.stop();
        self.hub.stop_secure();
        self.hub.stop_internet();
    }
}

/// A session like a small live stream: a few channels, an EQ and a compressor on the vocal, a
/// verb bus, the master.
fn live_session() -> Engine {
    let mut e = Engine::default();
    for n in ["Vocal", "Guitar", "Keys", "Crowd"] {
        e.execute("track.new", &json!({"name": n, "kind": "audio"})).unwrap();
    }
    e.execute("track.new", &json!({"name": "Master", "kind": "master"})).unwrap();
    let v = e.session().tracks.iter().find(|t| t.name == "Vocal").unwrap().id.0;
    e.execute("mix.insert", &json!({"track": v, "plugin": "eq_7band"})).unwrap();
    e.execute("mix.insert", &json!({"track": v, "plugin": "fs_comp"})).unwrap();
    e.execute("mix.send", &json!({"track": v, "slot": 0, "bus": "Verb", "level_db": -12.0})).unwrap();
    e
}

fn wait(what: &str, d: Duration, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn connect(addr: &str, token: &str, name: &str) -> Studio {
    let s = Studio::start(StudioTarget { name: String::new(), addr: addr.to_string(), token: token.to_string(), my_name: name.to_string() });
    wait("the studio's mixer", Duration::from_secs(10), || s.connected() && s.mirror().has_state());
    s
}

/// The remote's mirror shows exactly the studio's mixer.
fn assert_mirrors(studio: &StudioPc, s: &Studio) {
    let want = feed::mixer_state(&studio.engine());
    wait("the mirror to match", Duration::from_secs(10), || s.mirror().state() == Some(&want));
}

#[test]
fn connect_by_code_and_the_snapshot_renders_the_studios_strips() {
    let studio = StudioPc::plain(live_session());
    let token = studio.pair_by_code("Home PC");
    let s = connect(&studio.addr.to_string(), &token, "Home PC");
    assert_mirrors(&studio, &s);
    let v = s.mirror().view().unwrap();
    let names: Vec<&str> = v.strips.iter().map(|x| x.name.as_str()).collect();
    for n in ["Vocal", "Guitar", "Keys", "Crowd"] {
        assert!(names.contains(&n), "{names:?}");
    }
    assert!(v.strips.iter().any(|x| x.kind == "master"));
    let vocal = v.strips.iter().find(|x| x.name == "Vocal").unwrap();
    assert!(vocal.inserts.iter().any(|i| i.plugin == "eq_7band" && !i.params.is_empty()));
    assert!(vocal.sends.iter().any(|x| x.slot == 0 && (x.level_db + 12.0).abs() < 0.01 && x.target == "Verb"), "{:?}", vocal.sends);
    // The studio lists it as listening; nothing waits for it.
    wait("listed", Duration::from_secs(5), || studio.hub.remotes().iter().any(|r| r.name == "Home PC" && r.state == "listening"));
    assert!(studio.hub.missing_followers().is_empty());
    // Meters arrive and decode.
    let vid = vocal.id;
    wait("meters", Duration::from_secs(5), || s.mirror().meters(vid).is_some_and(|m| m.peak[0] > 0.0));
    // A ping's round trip is measured.
    wait("a round trip", Duration::from_secs(5), || s.health().rtt_ms.is_some());
    eprintln!("loopback ping: {:.2} ms", s.health().rtt_ms.unwrap_or(0.0));
    s.stop();
}

#[test]
fn connect_with_the_booth_pin_over_the_encrypted_link() {
    let studio = StudioPc::new(NetConfig::default(), live_session());
    studio.hub.set_booth_pin("sunday-stream").unwrap();
    let lan = Internet::start_lan(new_key(), 0).unwrap();
    lan.serve_lan_to(studio.hub.listen_secure().unwrap());
    let udp = SocketAddr::from(([127, 0, 0, 1], lan.udp_port().unwrap()));
    let home = Internet::start_lan(new_key(), 0).unwrap();
    let bridge = home.bridge_to(lan_addr(&lan.key_hex(), udp).unwrap()).unwrap().to_string();
    let r = pair_request_info(&bridge, "Home PC").unwrap();
    assert!(r.pin, "the studio waits for its PIN");
    let keys = LinkKeys::from_hex(&lan.key_hex(), &home.key_hex()).unwrap();
    let (token, studio_name) = pair_confirm(&bridge, &r.request, "sunday-stream".into(), keys).unwrap();
    assert_eq!(studio_name, studio.hub.pc_name());
    let s = connect(&bridge, &token, "Home PC");
    assert_mirrors(&studio, &s);
    wait("listed", Duration::from_secs(5), || studio.hub.remotes().iter().any(|r| r.detail.contains("encrypted local link")));
    s.stop();
}

#[test]
fn connect_with_an_invite_through_the_internet_bridge_and_again_from_the_stored_pairing() {
    let studio = StudioPc::new(NetConfig::default(), live_session());
    let host = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    host.serve_to(studio.hub.listen_internet().unwrap());
    let code = Invite { addr: host.addr(Duration::from_secs(1)), secret: studio.hub.new_invite_secret(), name: "Studio".into() }.code();
    let home = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    let inv = Invite::parse(&code).unwrap();
    let addr_json = serde_json::to_string(&inv.addr).unwrap();
    let bridge = home.bridge_to_json(&addr_json).unwrap().to_string();
    let (token, _) = soundcraft_net::follower::pair_invite(&bridge, &inv.secret, "Home PC").unwrap();
    let s = connect(&bridge, &token, "Home PC");
    assert_mirrors(&studio, &s);
    s.stop();
    wait("gone from the studio", Duration::from_secs(5), || studio.hub.remotes().is_empty());
    // Later (another run of the app): the stored token, a new bridge, no invite.
    let home = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    let bridge = home.bridge_to_json(&addr_json).unwrap().to_string();
    let s = connect(&bridge, &token, "Home PC");
    assert_mirrors(&studio, &s);
    assert_eq!(s.health().connects, 1);
    s.stop();
}

#[test]
fn the_studios_master_is_heard_while_the_studio_is_stopped() {
    let studio = StudioPc::plain(live_session());
    let token = studio.pair_by_code("Home PC");
    let s = connect(&studio.addr.to_string(), &token, "Home PC");
    let ears = Arc::new(StreamIn::default());
    s.set_stream(true, Some(Arc::clone(&ears)), 48_000);
    wait("the studio's tap on", Duration::from_secs(5), || studio.tap.is_on());
    // The studio is stopped: a live stream mixes live inputs with nothing rolling.
    studio.play_stopped(60);
    wait("audio frames", Duration::from_secs(5), || s.health().packets > 5 && ears.queued() > 0);
    assert!(ears.is_on());
    let h = s.health();
    eprintln!("stream: {} packets, buffer {:?} ms, lost {}", h.packets, h.buffer_ms, h.lost);
    // Mute (volume 0) or turning it off: no more frames asked for.
    s.set_stream(false, Some(Arc::clone(&ears)), 48_000);
    wait("the stream off", Duration::from_secs(5), || studio.hub.stream_listeners() == 0);
    assert!(!ears.is_on());
    s.stop();
}

#[test]
fn an_unpaired_pc_is_told_so_and_never_sees_the_mixer() {
    let studio = StudioPc::plain(live_session());
    let s = Studio::start(StudioTarget { name: String::new(), addr: studio.addr.to_string(), token: "nope".into(), my_name: "Stranger".into() });
    wait("refused", Duration::from_secs(5), || s.health().state == "not paired");
    assert!(!s.mirror().has_state());
    assert_eq!(studio.hub.mixer_subscribers(), 0);
    s.stop();
}
