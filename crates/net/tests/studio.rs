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

    fn id(&self, name: &str) -> u64 {
        self.engine().session().tracks.iter().find(|t| t.name == name).unwrap().id.0
    }

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

// ---- Stage two: moving the studio's mix from here ----

/// A cable that can be pulled: forwards connections to `to` while up; pulling it cuts them all
/// and refuses new ones (as in `rig.rs`).
struct Cable {
    addr: SocketAddr,
    up: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<std::net::TcpStream>>>,
}

impl Cable {
    fn new(to: SocketAddr) -> Cable {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let up = Arc::new(AtomicBool::new(true));
        let conns: Arc<Mutex<Vec<std::net::TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let (u, c) = (Arc::clone(&up), Arc::clone(&conns));
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                if !u.load(Ordering::SeqCst) {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                    continue;
                }
                let Ok(b) = std::net::TcpStream::connect(to) else { continue };
                let _ = (s.set_nodelay(true), b.set_nodelay(true));
                c.lock().unwrap().extend([s.try_clone().unwrap(), b.try_clone().unwrap()]);
                for (mut from, mut into) in [(s.try_clone().unwrap(), b.try_clone().unwrap()), (b, s)] {
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 8192];
                        while let Ok(n) = from.read(&mut buf) {
                            if n == 0 || into.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                        let _ = into.shutdown(std::net::Shutdown::Both);
                    });
                }
            }
        });
        Cable { addr, up, conns }
    }

    fn pull(&self) {
        self.up.store(false, Ordering::SeqCst);
        for s in self.conns.lock().unwrap().drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn plug(&self) {
        self.up.store(true, Ordering::SeqCst);
    }
}

fn strip_of(s: &Studio, id: u64) -> soundcraft_net::studio::RemoteStrip {
    s.mirror().feed_view().and_then(|v| v.strip(id).cloned()).unwrap()
}

#[test]
fn a_remote_fader_drag_is_one_undo_step_on_the_studio_and_comes_back_in_the_feed() {
    let studio = StudioPc::plain(live_session());
    let token = studio.pair_by_code("Home PC");
    let s = connect(&studio.addr.to_string(), &token, "Home PC");
    let v = studio.id("Vocal");
    let before = studio.volume(v);
    let key = soundcraft_net::studio::Control::Volume(v).gesture();
    // A drag: a move every 50 ms (the studio's frames go by in between), then the release.
    for db in [-1.0, -2.0, -3.0, -4.5] {
        assert!(s.execute("mix.volume", json!({"tracks": [v], "db": db}), Some(&key)));
        std::thread::sleep(Duration::from_millis(50));
    }
    s.end_gesture(&key);
    wait("the studio's fader", Duration::from_secs(5), || studio.volume(v) == -4.5);
    wait("the feed back here", Duration::from_secs(5), || strip_of(&s, v).volume_db == -4.5);
    wait("the step closed", Duration::from_secs(5), || !studio.engine().gesture_open(&key));
    let label = studio.engine().undo_label().map(str::to_string);
    wait("the studio's undo label here", Duration::from_secs(5), || s.mirror().feed_view().unwrap().undo == label);
    // One undo step on the studio, run from here.
    assert!(s.execute("edit.undo", json!({}), None));
    wait("undone", Duration::from_secs(5), || studio.volume(v) == before);
    wait("the feed says so", Duration::from_secs(5), || strip_of(&s, v).volume_db == before);
    assert!(s.take_refusals().is_empty());
    s.stop();
}

#[test]
fn mute_solo_pan_send_bypass_and_built_in_parameters_reach_the_studio() {
    let studio = StudioPc::plain(live_session());
    let token = studio.pair_by_code("Home PC");
    let s = connect(&studio.addr.to_string(), &token, "Home PC");
    let v = studio.id("Vocal");
    let g = studio.id("Guitar");
    let track = |id: u64| studio.engine().session().tracks.iter().find(|t| t.id.0 == id).unwrap().clone();
    s.execute("mix.mute", json!({"tracks": [g], "value": true}), None);
    s.execute("mix.solo", json!({"tracks": [v], "value": true}), None);
    s.execute("mix.pan", json!({"tracks": [g], "pan": -0.6, "index": 0}), Some("pan:g"));
    s.end_gesture("pan:g");
    s.execute("mix.send_level", json!({"track": v, "slot": 0, "db": -6.0}), Some("send"));
    s.end_gesture("send");
    s.execute("mix.insert_bypass", json!({"track": v, "slot": 1, "value": true}), None);
    let (eq, comp) = {
        let t = track(v);
        (t.mixer.inserts.iter().find(|i| i.plugin == "eq_7band").unwrap().id.0, t.mixer.inserts.iter().find(|i| i.plugin == "fs_comp").unwrap().id.0)
    };
    // Built-in parameters in plain units, as the feed shows them.
    let fv = strip_of(&s, v);
    let eq_params = fv.inserts.iter().find(|i| i.module == eq).unwrap().params.clone();
    let comp_params = fv.inserts.iter().find(|i| i.module == comp).unwrap().params.clone();
    let eq_info = soundcraft_dsp::plugin_info("eq_7band").unwrap();
    let eq_p = eq_info.params.iter().find(|p| eq_params.iter().any(|(k, _)| k == p.id) && p.max - p.min > 1.0).unwrap();
    let comp_info = soundcraft_dsp::plugin_info("fs_comp").unwrap();
    let comp_p = comp_info.params.iter().find(|p| comp_params.iter().any(|(k, _)| k == p.id) && p.max - p.min > 1.0).unwrap();
    let eq_v = eq_p.min + (eq_p.max - eq_p.min) * 0.3;
    let comp_v = comp_p.min + (comp_p.max - comp_p.min) * 0.7;
    s.execute("mix.module_params", json!({"track": v, "module": eq, "params": {eq_p.id: eq_v}}), Some("eq"));
    s.end_gesture("eq");
    s.execute("mix.module_params", json!({"track": v, "module": comp, "params": {comp_p.id: comp_v}}), Some("comp"));
    s.end_gesture("comp");
    let near = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-3);
    wait("everything on the studio", Duration::from_secs(5), || {
        let (tv, tg) = (track(v), track(g));
        tg.mixer.mute
            && tv.mixer.solo
            && (tg.mixer.pan[0] + 0.6).abs() < 1e-4
            && tv.mixer.sends[0].as_ref().is_some_and(|x| x.level_db == -6.0)
            && tv.mixer.inserts[1].bypass
            && tv.mixer.inserts.iter().find(|i| i.id.0 == eq).is_some_and(|i| near(i.params.get(eq_p.id).copied(), eq_v))
            && tv.mixer.inserts.iter().find(|i| i.id.0 == comp).is_some_and(|i| near(i.params.get(comp_p.id).copied(), comp_v))
    });
    assert_mirrors(&studio, &s);
    let fv = strip_of(&s, v);
    let shown = fv.inserts.iter().find(|i| i.module == eq).unwrap().params.iter().find(|(k, _)| k == eq_p.id).map(|(_, x)| *x);
    assert!(shown.is_some_and(|x| (x - eq_v).abs() < 0.011), "the EQ's new value is in the feed: {shown:?} vs {eq_v}");
    assert!(s.take_refusals().is_empty());
    s.stop();
}

#[test]
fn a_change_made_on_the_studio_shows_here() {
    let studio = StudioPc::plain(live_session());
    let token = studio.pair_by_code("Home PC");
    let s = connect(&studio.addr.to_string(), &token, "Home PC");
    let k = studio.id("Keys");
    studio.engine().execute("mix.volume", &json!({"tracks": [k], "db": -9.25})).unwrap();
    studio.engine().execute("mix.mute", &json!({"tracks": [k], "value": true})).unwrap();
    wait("the studio's change here", Duration::from_secs(5), || {
        let st = strip_of(&s, k);
        st.volume_db == -9.25 && st.mute
    });
    s.stop();
}

#[test]
fn two_remotes_on_one_control_the_first_touch_holds_it_until_released() {
    let studio = StudioPc::plain(live_session());
    let ta = studio.pair_by_code("Home PC");
    let tb = studio.pair_by_code("Helper PC");
    let a = connect(&studio.addr.to_string(), &ta, "Home PC");
    let b = connect(&studio.addr.to_string(), &tb, "Helper PC");
    let v = studio.id("Vocal");
    let key = soundcraft_net::studio::Control::Volume(v).gesture();
    a.execute("mix.volume", json!({"tracks": [v], "db": -2.0}), Some(&key));
    wait("a's move", Duration::from_secs(5), || studio.volume(v) == -2.0);
    // B grabs the same fader while A's hand is on it: refused, and told whose hand it is.
    b.execute("mix.volume", json!({"tracks": [v], "db": -20.0}), Some(&key));
    let mut refused = Vec::new();
    wait("b refused", Duration::from_secs(5), || {
        refused.extend(b.take_refusals());
        !refused.is_empty()
    });
    assert_eq!(refused[0].held_by.as_deref(), Some("Home PC"), "{refused:?}");
    assert_eq!(studio.volume(v), -2.0, "the studio's fader is A's");
    assert_eq!(studio.hub.holds(), vec![(key.clone(), "Home PC".to_string())]);
    // A lets go: B may take it after the short grace.
    a.end_gesture(&key);
    std::thread::sleep(soundcraft_net::hub::HOLD_GRACE + Duration::from_millis(150));
    b.execute("mix.volume", json!({"tracks": [v], "db": -8.0}), Some(&key));
    b.end_gesture(&key);
    wait("b's move", Duration::from_secs(5), || studio.volume(v) == -8.0);
    // Both see the same fader.
    wait("a sees b's move", Duration::from_secs(5), || strip_of(&a, v).volume_db == -8.0);
    a.stop();
    b.stop();
}

#[test]
fn a_remote_gone_mid_drag_closes_its_undo_step_and_leaves_the_mix_as_last_set() {
    let studio = StudioPc::plain(live_session());
    let token = studio.pair_by_code("Home PC");
    let s = connect(&studio.addr.to_string(), &token, "Home PC");
    let v = studio.id("Vocal");
    let before = studio.volume(v);
    let key = soundcraft_net::studio::Control::Volume(v).gesture();
    for db in [-1.5, -2.5, -6.0] {
        s.execute("mix.volume", json!({"tracks": [v], "db": db}), Some(&key));
        std::thread::sleep(Duration::from_millis(40));
    }
    wait("the last move", Duration::from_secs(5), || studio.volume(v) == -6.0);
    assert!(studio.engine().gesture_open(&key), "mid-drag: the step is open");
    // The link goes with the hand still down.
    let t0 = Instant::now();
    s.stop();
    wait("the step closed by the studio", Duration::from_secs(5), || !studio.engine().gesture_open(&key));
    let closed_in = t0.elapsed();
    eprintln!("undo step closed {closed_in:?} after the link went");
    assert!(closed_in < Engine::GESTURE_LEASE, "closed when the link went, not by the lease");
    assert_eq!(studio.volume(v), -6.0, "the mix stays as last set");
    wait("its hand off", Duration::from_secs(5), || studio.hub.holds().is_empty());
    // One step: a studio change after it is its own.
    studio.engine().execute("mix.volume", &json!({"tracks": [v], "db": -10.0})).unwrap();
    studio.engine().execute("edit.undo", &json!({})).unwrap();
    assert_eq!(studio.volume(v), -6.0);
    studio.engine().execute("edit.undo", &json!({})).unwrap();
    assert_eq!(studio.volume(v), before, "the whole remote drag was one step");
}

#[test]
fn after_a_drop_the_link_takes_a_fresh_snapshot_and_replays_nothing() {
    let studio = StudioPc::plain(live_session());
    let token = studio.pair_by_code("Home PC");
    let cable = Cable::new(studio.addr);
    let s = connect(&cable.addr.to_string(), &token, "Home PC");
    let v = studio.id("Vocal");
    let before = studio.volume(v);
    assert_eq!(s.mirror().snapshots, 1);
    cable.pull();
    wait("the drop noticed", Duration::from_secs(10), || !s.connected());
    assert_eq!(s.health().state, "reconnecting");
    // Moves made while it is down are not queued for later.
    assert!(!s.execute("mix.volume", json!({"tracks": [v], "db": -30.0}), None));
    assert!(!s.execute("mix.mute", json!({"tracks": [v], "value": true}), None));
    assert!(!s.execute("mix.pan", json!({"tracks": [v], "pan": 1.0}), None));
    assert_eq!(s.take_unsent(), 3, "3 changes were not sent");
    assert!(s.mirror().has_state(), "the last state stays visible");
    // Meanwhile the studio's engineer moves something.
    studio.engine().execute("mix.volume", &json!({"tracks": [v], "db": -12.0})).unwrap();
    cable.plug();
    wait("back", Duration::from_secs(15), || s.connected() && s.mirror().snapshots == 2);
    assert_mirrors(&studio, &s);
    assert_eq!(studio.volume(v), -12.0, "nothing replayed: the studio's own move stands");
    assert_ne!(studio.volume(v), before);
    assert!(!studio.engine().session().tracks.iter().find(|t| t.id.0 == v).unwrap().mixer.mute);
    assert_eq!(s.take_unsent(), 0);
    s.stop();
}

#[test]
fn round_trip_of_a_fader_change_and_bandwidth_on_loopback() {
    let mut e = Engine::default();
    for k in 0..31 {
        e.execute("track.new", &json!({"name": format!("Ch {k}"), "kind": "audio"})).unwrap();
    }
    e.execute("track.new", &json!({"name": "Master", "kind": "master"})).unwrap();
    let studio = StudioPc::plain(e);
    let token = studio.pair_by_code("Home PC");
    let s = connect(&studio.addr.to_string(), &token, "Home PC");
    let v = studio.id("Ch 3");
    let key = soundcraft_net::studio::Control::Volume(v).gesture();
    let mut rtts = Vec::new();
    for k in 0..20 {
        let db = -10.0 - k as f32 * 0.5;
        let t0 = Instant::now();
        s.execute("mix.volume", json!({"tracks": [v], "db": db}), Some(&key));
        wait("the move back in the feed", Duration::from_secs(5), || strip_of(&s, v).volume_db == db);
        rtts.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    s.end_gesture(&key);
    rtts.sort_by(f64::total_cmp);
    let median = rtts[rtts.len() / 2];
    eprintln!("fader change → studio → back in the feed, loopback, 32 strips: median {median:.1} ms, worst {:.1} ms", rtts[rtts.len() - 1]);
    let answered = s.command_rtts();
    eprintln!("command answered (studio applied it): {:?} ms", answered.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>());
    assert!(median < 250.0, "{median}");
    // Bandwidth: two seconds of a fader moving at 20 Hz (meters running), then two seconds still.
    let (in0, out0) = s.bytes();
    let t0 = Instant::now();
    let mut k = 0;
    while t0.elapsed() < Duration::from_secs(2) {
        s.execute("mix.volume", json!({"tracks": [v], "db": -20.0 + (k % 40) as f32 * 0.25}), Some(&key));
        k += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    s.end_gesture(&key);
    let (in1, out1) = s.bytes();
    std::thread::sleep(Duration::from_secs(2));
    let (in2, out2) = s.bytes();
    eprintln!(
        "mixing session, 32 strips, meters moving: fader moving {:.1} KB/s in, {:.1} KB/s out; fader still {:.1} KB/s in, {:.1} KB/s out (stream audio not included: 256 kb/s Opus = 32 KB/s while heard)",
        (in1 - in0) as f64 / 2048.0,
        (out1 - out0) as f64 / 2048.0,
        (in2 - in1) as f64 / 2048.0,
        (out2 - out1) as f64 / 2048.0
    );
    s.stop();
}
