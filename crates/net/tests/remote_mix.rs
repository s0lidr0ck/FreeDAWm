//! Groundwork for mixing from another PC: a listen-only link (the mix stream and the status,
//! never a recorder), the stream while the booth is stopped, and the pushed mixer feed.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::surface::feed;
use soundcraft_net::config::NetConfig;
use soundcraft_net::follower::{pair_confirm, pair_request};
use soundcraft_net::hub::Hub;
use soundcraft_net::pairing::exchange::LinkKeys;
use soundcraft_net::proto::RigStatus;
use soundcraft_playback::stream::MixTap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn hub() -> (Hub, SocketAddr) {
    let hub = Hub::new(NetConfig { encrypted_only: false, ..NetConfig::default() }, None);
    let port = hub.listen(0).unwrap();
    (hub, SocketAddr::from(([127, 0, 0, 1], port)))
}

struct Pc {
    c: TcpStream,
    r: BufReader<TcpStream>,
    /// Bytes read so far (the feed's bandwidth).
    bytes: usize,
}

impl Pc {
    /// A PC paired with `hub` and greeted with `hello` in `role` (none: a controlling PC).
    fn connect(hub: &Hub, addr: SocketAddr, name: &str, role: Option<&str>) -> Pc {
        let c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let r = BufReader::new(c.try_clone().unwrap());
        let id = pair_request(&addr.to_string(), name).unwrap();
        let code = hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap().code;
        let (token, _) = pair_confirm(&addr.to_string(), &id, code, LinkKeys::PLAIN).unwrap();
        let mut pc = Pc { c, r, bytes: 0 };
        let mut params = json!({"token": token, "name": name});
        if let Some(role) = role {
            params["role"] = json!(role);
        }
        let v = pc.call("hello", params);
        assert_eq!(v["ok"], true, "{v}");
        pc
    }

    fn send(&mut self, method: &str, params: Value) {
        writeln!(self.c, "{}", json!({"id": method, "method": method, "params": params})).unwrap();
    }

    /// The next line (event or reply).
    fn next(&mut self) -> Value {
        let mut s = String::new();
        self.r.read_line(&mut s).unwrap();
        self.bytes += s.len();
        serde_json::from_str(&s).unwrap_or(Value::Null)
    }

    /// Sends a request and returns its reply, events before it skipped.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.send(method, params);
        loop {
            let v = self.next();
            if v["id"] == method {
                return v;
            }
        }
    }

    /// Events until `until` takes one (or `d` passes).
    fn wait(&mut self, d: Duration, mut until: impl FnMut(&Value) -> bool) -> Option<Value> {
        let end = Instant::now() + d;
        while Instant::now() < end {
            let v = self.next();
            if until(&v) {
                return Some(v);
            }
        }
        None
    }
}

/// `n` blocks of a tone into the tap, as the booth's audio thread would (stopped or rolling).
fn play(tap: &MixTap, from: i64, n: usize, rolling: bool) {
    let block = 480usize;
    for k in 0..n {
        let l: Vec<f32> = (0..block).map(|i| 0.3 * ((k * block + i) as f32 * 0.05).sin()).collect();
        let at = if rolling { from + (k * block) as i64 } else { from };
        tap.push_block(&l, &l, block, at, 1_000 + at, rolling);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_listening_pc_hears_the_mix_and_reads_the_status_but_never_records() {
    let (hub, addr) = hub();
    let tap = Arc::new(MixTap::default());
    hub.set_tap(Some(Arc::clone(&tap)), 48_000);
    let mut pc = Pc::connect(&hub, addr, "Home PC", Some("listener"));
    // The studio lists it as listening, never as a recorder it waits for.
    let r = hub.remotes();
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].name.as_str(), r[0].state.as_str(), r[0].warning), ("Home PC", "listening", false));
    assert!(hub.config().paired.iter().all(|d| !d.follows), "not made a follower");
    assert!(hub.config().remote_ports.is_empty() && hub.config().remote_inputs.is_empty());
    assert!(hub.missing_followers().is_empty());
    assert_eq!(hub.connected_pcs(), 1);
    // The status feed.
    let v = pc.call("status.subscribe", json!({"on": true}));
    assert_eq!(v["ok"], true, "{v}");
    hub.publish(RigStatus { session: "Sunday".into(), ..RigStatus::default() });
    let st = pc.wait(Duration::from_secs(5), |v| v["event"] == "status" && v["params"]["session"] == "Sunday").expect("status pushed");
    assert_eq!(st["params"]["remotes"][0]["state"], "listening");
    // The mix, while the booth is stopped.
    let v = pc.call("stream.subscribe", json!({"on": true, "live": true}));
    assert_eq!(v["result"]["live"], true);
    let t0 = Instant::now();
    while !tap.is_on() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    play(&tap, 0, 20, false);
    let p = pc.wait(Duration::from_secs(5), |v| v["event"] == "stream").expect("the mix while stopped");
    assert_eq!(p["params"]["idle"], true);
    assert!(hub.remotes()[0].detail.contains("hears the mix"));
    // It cannot record or send takes.
    for m in ["take.offer", "take.transfer", "feed.audio", "follower.report"] {
        let v = pc.call(m, json!({"id": "x", "file": "a.flac", "bytes": 10}));
        assert_eq!(v["ok"], false, "{m}: {v}");
    }
    assert!(hub.drain().is_empty(), "nothing reached the app");
    // It goes: forgotten at once, nothing waited for, nothing in the mix touched.
    drop(pc);
    let t0 = Instant::now();
    while (!hub.remotes().is_empty() || hub.stream_listeners() > 0 || hub.connected_pcs() > 0) && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(hub.remotes().is_empty(), "{:?}", hub.remotes());
    assert_eq!((hub.stream_listeners(), hub.connected_pcs()), (0, 0));
    assert!(hub.missing_followers().is_empty());
    hub.stop();
}

#[test]
fn stopped_the_live_listener_gets_the_mix_and_an_older_one_only_after_play() {
    let (hub, addr) = hub();
    let tap = Arc::new(MixTap::default());
    hub.set_tap(Some(Arc::clone(&tap)), 48_000);
    let mut live = Pc::connect(&hub, addr, "Home PC", Some("listener"));
    let mut old = Pc::connect(&hub, addr, "Old PC", Some("listener"));
    live.call("stream.subscribe", json!({"on": true, "live": true}));
    // An older FreeDAWm asks without `live`.
    old.call("stream.subscribe", json!({"on": true}));
    let t0 = Instant::now();
    while !tap.is_on() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    // Stopped, then Play, then Stop.
    play(&tap, 0, 30, false);
    play(&tap, 0, 30, true);
    play(&tap, 14_400, 30, false);
    let mut got = Vec::new();
    while let Some(v) = live.wait(Duration::from_secs(3), |v| v["event"] == "stream") {
        got.push(v["params"].clone());
        if got.len() > 5 && got.last().is_some_and(|p| p["idle"] == true) && got.iter().any(|p| p.get("idle").is_none()) {
            let idle_after = got.iter().rev().take_while(|p| p["idle"] == true).count();
            if idle_after >= 5 {
                break;
            }
        }
    }
    let seqs: Vec<u64> = got.iter().map(|p| p["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "no gap, no repeat: {seqs:?}");
    let epochs: Vec<(u64, bool)> = got.iter().map(|p| (p["epoch"].as_u64().unwrap(), p["idle"] == true)).collect();
    let mut runs: Vec<(u64, bool)> = epochs.clone();
    runs.dedup();
    assert_eq!(runs.len(), 3, "stopped, rolling, stopped: three epochs {runs:?}");
    assert!(runs.windows(2).all(|w| w[1].0 == w[0].0 + 1));
    assert_eq!(runs.iter().map(|r| r.1).collect::<Vec<_>>(), [true, false, true]);
    // The older listener: only the roll.
    let first = old.wait(Duration::from_secs(3), |v| v["event"] == "stream").expect("the roll");
    assert!(first["params"].get("idle").is_none(), "nothing while stopped: {first}");
    hub.stop();
}

/// A session with `n` audio tracks besides the master.
fn engine_with(n: usize) -> Engine {
    let mut e = Engine::default();
    for k in 0..n {
        e.execute("track.new", &json!({"name": format!("Ch {k}"), "kind": "audio"})).unwrap();
    }
    e
}

fn meters_of(e: &Engine, level: f32) -> Value {
    let levels: Vec<(u64, feed::StripLevels)> = feed::feed_tracks(e.session())
        .map(|t| (t.id.0, feed::StripLevels { peak: [level, level * 0.9], rms: [level * 0.5, level * 0.45], gr_db: 0.0 }))
        .collect();
    feed::meters_json(0.0, &levels)
}

#[test]
fn the_mixer_feed_sends_a_snapshot_then_deltas_and_meters() {
    let (hub, addr) = hub();
    let mut e = engine_with(3);
    hub.publish_mixer(feed::mixer_state(&e));
    let mut pc = Pc::connect(&hub, addr, "Home PC", None);
    assert_eq!(hub.mixer_subscribers(), 0);
    let v = pc.call("mixer.subscribe", json!({"on": true}));
    assert_eq!(v["ok"], true, "{v}");
    let snap = pc.wait(Duration::from_secs(5), |v| v["event"] == "mixer.snapshot").expect("a snapshot first");
    let mut mirror = snap["params"]["state"].clone();
    assert_eq!(mirror, feed::mixer_state(&e));
    assert_eq!(hub.mixer_subscribers(), 1);
    // A fader moves (on the studio's screen): a delta with just that.
    let t = e.session().tracks.iter().find(|t| t.name == "Ch 1").unwrap().id.0;
    e.execute_merged("mix.volume", &json!({"track": t, "db": -4.5}), "fader").unwrap();
    hub.publish_mixer(feed::mixer_state(&e));
    let d = pc.wait(Duration::from_secs(5), |v| v["event"] == "mixer.delta").expect("a delta");
    assert_eq!(d["params"]["strips"], json!([{"id": t, "volume_db": -4.5}]));
    assert_eq!(d["params"]["seq"].as_u64(), snap["params"]["seq"].as_u64().map(|s| s + 1));
    feed::apply_delta(&mut mirror, &d["params"]);
    assert_eq!(mirror, feed::mixer_state(&e), "the mirror is the studio's mixer");
    // Nothing changed: nothing sent.
    hub.publish_mixer(feed::mixer_state(&e));
    // Meters.
    hub.publish_meters(meters_of(&e, 0.5));
    let m = pc.wait(Duration::from_secs(5), |v| v["event"] == "mixer.meters").expect("meters");
    let n = feed::feed_tracks(e.session()).count();
    assert_eq!(m["params"]["ids"].as_array().unwrap().len(), n, "every strip");
    assert_eq!(m["params"]["lv"].as_str().unwrap().len(), (n * 5).div_ceil(3) * 4, "five bytes a strip");
    // The same meters again: nothing sent; new ones are.
    hub.publish_meters(meters_of(&e, 0.5));
    hub.publish_meters(meters_of(&e, 0.25));
    let m2 = pc.wait(Duration::from_secs(5), |v| v["event"] == "mixer.meters").expect("meters");
    assert_eq!(m2["params"]["seq"].as_u64(), m["params"]["seq"].as_u64().map(|s| s + 1), "an unchanged frame is not a new one");
    // Off: no more.
    pc.call("mixer.subscribe", json!({"on": false}));
    let t0 = Instant::now();
    while hub.mixer_subscribers() > 0 && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(hub.mixer_subscribers(), 0);
    hub.stop();
}

#[test]
fn a_stalled_subscriber_never_holds_up_the_studio_nor_grows_its_memory() {
    let (hub, addr) = hub();
    let mut e = engine_with(32);
    hub.publish_mixer(feed::mixer_state(&e));
    // One PC subscribes and then never reads again.
    let mut stalled = Pc::connect(&hub, addr, "Stalled PC", None);
    stalled.call("mixer.subscribe", json!({"on": true}));
    let mut fine = Pc::connect(&hub, addr, "Home PC", None);
    fine.call("mixer.subscribe", json!({"meters": false}));
    let t = e.session().tracks.iter().find(|t| t.name == "Ch 0").unwrap().id.0;
    // Two thousand fader moves and meters, as fast as the UI can make them.
    let t0 = Instant::now();
    let mut slowest = Duration::ZERO;
    for k in 0..2000 {
        e.execute_merged("mix.volume", &json!({"track": t, "db": -(k % 400) as f64 * 0.1}), "fader").unwrap();
        let s = Instant::now();
        hub.publish_mixer(feed::mixer_state(&e));
        hub.publish_meters(meters_of(&e, 0.25));
        slowest = slowest.max(s.elapsed());
    }
    eprintln!("2000 publishes in {:?}, the slowest {:?}", t0.elapsed(), slowest);
    assert!(slowest < Duration::from_millis(100), "publishing never waits on a subscriber: {slowest:?}");
    assert!(hub.mixer_backlog() <= 64, "bounded: {}", hub.mixer_backlog());
    // The healthy one still ends on the studio's mixer.
    let want = feed::mixer_state(&e);
    let mut mirror = Value::Null;
    let ok = fine.wait(Duration::from_secs(10), |v| {
        match v["event"].as_str() {
            Some("mixer.snapshot") => mirror = v["params"]["state"].clone(),
            Some("mixer.delta") => feed::apply_delta(&mut mirror, &v["params"]),
            _ => {}
        }
        mirror == want
    });
    assert!(ok.is_some(), "the healthy subscriber caught up");
    drop(stalled);
    hub.stop();
}

#[test]
fn the_mixer_feeds_bandwidth_for_32_strips() {
    let (hub, addr) = hub();
    let mut e = engine_with(32);
    hub.publish_mixer(feed::mixer_state(&e));
    let mut pc = Pc::connect(&hub, addr, "Home PC", None);
    pc.call("mixer.subscribe", json!({"on": true}));
    let snap = pc.wait(Duration::from_secs(5), |v| v["event"] == "mixer.snapshot").unwrap();
    let snapshot_bytes = snap.to_string().len();
    // The UI publishes every 50 ms (20 Hz): one second idle (meters moving), then one second
    // of a fader moving every tick.
    let ids: Vec<u64> = feed::feed_tracks(e.session()).map(|t| t.id.0).collect();
    let measure = |e: &mut Engine, pc: &mut Pc, moving: bool| -> usize {
        let before = pc.bytes;
        let reader = std::thread::scope(|sc| {
            let h = sc.spawn(|| {
                for k in 0..20 {
                    if moving {
                        e.execute_merged("mix.volume", &json!({"track": ids[3], "db": -10.0 + k as f64 * 0.37}), "fader").unwrap();
                    }
                    hub.publish_mixer(feed::mixer_state(e));
                    hub.publish_meters(meters_of(e, 0.1 + 0.02 * k as f32));
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let mut n = 0;
            let end = Instant::now() + Duration::from_millis(1100);
            pc.c.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            while Instant::now() < end {
                let mut s = String::new();
                if pc.r.read_line(&mut s).is_ok() {
                    n += s.len();
                }
            }
            h.join().unwrap();
            n
        });
        pc.bytes = before + reader;
        reader
    };
    let idle = measure(&mut e, &mut pc, false);
    let moving = measure(&mut e, &mut pc, true);
    eprintln!("{} strips: snapshot {snapshot_bytes} bytes; idle {idle} bytes/s; one fader moving {moving} bytes/s", ids.len());
    assert!(idle < 40_000 && moving < 60_000, "idle {idle}, moving {moving}");
    assert!(moving > idle, "the fader's deltas are on top of the meters");
    hub.stop();
}
