//! The encrypted local link: two in-process peers on this machine, the booth's QUIC endpoint
//! bridged to its encrypted-link listener, the remote PC's bridged to a local port.
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use soundcraft_net::audio::{b64_decode, b64_encode};
use soundcraft_net::config::NetConfig;
use soundcraft_net::follower::{UPDATE_BOOTH, WRONG_OR_BETWEEN, pair_confirm, pair_request};
use soundcraft_net::hub::{Hub, LinkKind, UPDATE_PC, http_request};
use soundcraft_net::internet::{Internet, lan_addr, new_key};
use soundcraft_net::pairing::PendingPair;
use soundcraft_net::pairing::exchange::{Exchange, LinkKeys, Side};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// A booth listening on both links: its hub, the plain TCP address, its encrypted endpoint and
/// that endpoint's address (key, `127.0.0.1:udp`).
struct Booth {
    hub: Hub,
    plain: SocketAddr,
    _lan: Internet,
    key: String,
    udp: SocketAddr,
}

fn booth(cfg: NetConfig) -> Booth {
    let hub = Hub::new(cfg, None);
    let port = hub.listen(0).unwrap();
    let lan = Internet::start_lan(new_key(), 0).unwrap();
    lan.serve_lan_to(hub.listen_secure().unwrap());
    let udp = SocketAddr::from(([127, 0, 0, 1], lan.udp_port().unwrap()));
    let key = lan.key_hex();
    Booth { hub, plain: SocketAddr::from(([127, 0, 0, 1], port)), _lan: lan, key, udp }
}

/// A remote PC's line connection to `addr` (a bridge or the plain port).
struct Conn {
    c: TcpStream,
    r: BufReader<TcpStream>,
}

impl Conn {
    fn open(addr: &str) -> Conn {
        let c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let r = BufReader::new(c.try_clone().unwrap());
        Conn { c, r }
    }

    /// One request and its answer; `None` when the link closed (nothing came back).
    fn call(&mut self, v: Value) -> Option<Value> {
        writeln!(self.c, "{v}").ok()?;
        let mut s = String::new();
        match self.r.read_line(&mut s) {
            Ok(n) if n > 0 => serde_json::from_str(&s).ok(),
            _ => None,
        }
    }
}

/// The two link keys as `piano` sees them dialling the booth whose key is `booth_key`.
fn keys(booth_key: &str, piano: &Internet) -> LinkKeys {
    LinkKeys::from_hex(booth_key, &piano.key_hex()).unwrap()
}

/// Pairs `piano` through its bridge to the booth with key `booth_key`, reading the code off the
/// booth's screen: the token.
fn pair(hub: &Hub, bridge: &str, piano: &Internet, booth_key: &str, name: &str) -> String {
    let id = pair_request(bridge, name).unwrap();
    let code = hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap().code;
    pair_confirm(bridge, &id, code, keys(booth_key, piano)).unwrap().0
}

/// The booth's request `id` as its screen shows it (`None`: gone, or its last try under way).
fn shown(hub: &Hub, id: &str) -> Option<PendingPair> {
    hub.pending_pairings().into_iter().find(|p| p.id == id)
}

fn wrong_code(real: &str) -> String {
    if real == "0000" { "1111".into() } else { "0000".into() }
}

#[test]
fn a_pc_pairs_over_the_encrypted_link_and_is_bound_to_its_key() {
    let b = booth(NetConfig::default());
    let secret = new_key();
    let piano = Internet::start_lan(secret, 0).unwrap();
    let bridge = piano.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    let mut conn = Conn::open(&bridge);
    // The code on the booth's screen is still what pairs, with its attempt limit; it is never
    // sent, only proved by the exchange.
    let id = pair_request(&bridge, "Piano PC").unwrap();
    let real = shown(&b.hub, &id).unwrap().code;
    let err = pair_confirm(&bridge, &id, wrong_code(&real), keys(&b.key, &piano)).unwrap_err();
    assert_eq!(err, WRONG_OR_BETWEEN);
    // The connection the booth answered on is closed by the PC, the try spent.
    let until = Instant::now() + Duration::from_secs(5);
    while shown(&b.hub, &id).is_some_and(|p| p.wrong == 0) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(shown(&b.hub, &id).unwrap().wrong, 1, "a wrong code spends a try");
    assert!(b.hub.config().paired.is_empty());
    let (token, booth_name) = pair_confirm(&bridge, &id, real, keys(&b.key, &piano)).unwrap();
    assert_eq!(booth_name, b.hub.pc_name());
    assert!(shown(&b.hub, &id).is_none(), "used");
    let dev = b.hub.config().paired.into_iter().find(|d| d.token == token).unwrap();
    assert_eq!(dev.key, piano.key_hex(), "bound to the key it paired with");
    // Over the encrypted link: in, and the booth says which link.
    let v = conn.call(json!({"id": 4, "method": "hello", "params": {"token": token, "role": "follower"}})).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    let until = Instant::now() + Duration::from_secs(5);
    while b.hub.links().is_empty() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(b.hub.links(), vec![("Piano PC".to_string(), LinkKind::Encrypted)]);
    assert!(b.hub.remotes()[0].detail.contains("encrypted local link"), "{:?}", b.hub.remotes());
    // The same token on the plain link (sniffed, say) is refused.
    let mut plain = Conn::open(&b.plain.to_string());
    let v = plain.call(json!({"id": 5, "method": "hello", "params": {"token": token}})).unwrap();
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("encrypted local link"), "{v}");
    // And from another PC's key over the encrypted link.
    let thief = Internet::start_lan(new_key(), 0).unwrap();
    let tb = thief.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    let mut t = Conn::open(&tb);
    let v = t.call(json!({"id": 6, "method": "hello", "params": {"token": token}})).unwrap();
    assert_eq!(v["ok"], false, "another key: {v}");
    // Reconnecting as before: a new connection, and the PC started again with its kept key.
    drop(conn);
    let mut again = Conn::open(&bridge);
    let v = again.call(json!({"id": 7, "method": "hello", "params": {"token": token}})).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    drop((again, piano));
    let piano = Internet::start_lan(secret, 0).unwrap();
    let bridge = piano.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    let mut again = Conn::open(&bridge);
    let v = again.call(json!({"id": 8, "method": "hello", "params": {"token": token}})).unwrap();
    assert_eq!(v["ok"], true, "the same key after a restart: {v}");
    b.hub.stop();
    b.hub.stop_secure();
}

#[test]
fn a_pc_dialling_another_key_reaches_nobody() {
    let b = booth(NetConfig::default());
    let piano = Internet::start_lan(new_key(), 0).unwrap();
    // A booth that answers at this address but holds another key (a PC posing as the booth).
    let pretend = Internet::start_lan(new_key(), 0).unwrap().key_hex();
    let bridge = piano.bridge_to(lan_addr(&pretend, b.udp).unwrap()).unwrap().to_string();
    let mut conn = Conn::open(&bridge);
    let v = conn.call(json!({"id": 1, "method": "pair.request", "params": {"name": "Piano PC"}}));
    assert!(v.is_none(), "the QUIC handshake fails: nothing answers ({v:?})");
    assert!(b.hub.pending_pairings().is_empty(), "no pairing request reached the booth");
    b.hub.stop();
    b.hub.stop_secure();
}

#[test]
fn encrypted_only_keeps_pcs_off_the_plain_link_and_the_page_on_it() {
    // On by default: a booth as it comes.
    let b = booth(NetConfig::default());
    let mut plain = Conn::open(&b.plain.to_string());
    let v = plain.call(json!({"id": 1, "method": "pair.request", "params": {"name": "Piano PC"}})).unwrap();
    assert_eq!(v["ok"], false);
    assert!(b.hub.pending_pairings().is_empty());
    // The phone's page is still served there.
    let (code, _) = http_request(b.plain, "GET", "/", None, None).unwrap();
    assert_eq!(code, 200);
    // Over the encrypted link, pairing works.
    let piano = Internet::start_lan(new_key(), 0).unwrap();
    let bridge = piano.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    let token = pair(&b.hub, &bridge, &piano, &b.key, "Piano PC");
    assert!(!token.is_empty());
    b.hub.stop();
    b.hub.stop_secure();
}

#[test]
fn the_encrypted_listener_wants_to_know_who_calls() {
    // A local process connecting to the encrypted-link listener without the bridge's first line.
    let b = booth(NetConfig::default());
    let port = b.hub.secure_port().unwrap();
    let mut c = Conn::open(&format!("127.0.0.1:{port}"));
    let v = c.call(json!({"id": 1, "method": "pair.request", "params": {"name": "Sneaky"}})).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    assert!(b.hub.pending_pairings().is_empty());
    b.hub.stop();
    b.hub.stop_secure();
}

/// A machine posing as the booth (it forged the booth's network record): its own key towards
/// the PC, another of its own towards the booth, every byte passed through both ways. Its key
/// and address are what the PC dials.
struct Relay {
    _front: Internet,
    _back: Internet,
    key: String,
    udp: SocketAddr,
}

fn relay(b: &Booth) -> Relay {
    let back = Internet::start_lan(new_key(), 0).unwrap();
    let to_booth = back.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(s) = s else { break };
            std::thread::spawn(move || {
                let mut from_pc = BufReader::new(s.try_clone().unwrap());
                // Its own bridge's first line (the PC's key, as it saw it) is not passed on.
                let mut first = String::new();
                from_pc.read_line(&mut first).unwrap();
                let up = TcpStream::connect(to_booth).unwrap();
                let mut up_w = up.try_clone().unwrap();
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut from_pc, &mut up_w);
                    let _ = up_w.shutdown(std::net::Shutdown::Write);
                });
                let (mut up_r, mut to_pc) = (up, s);
                let _ = std::io::copy(&mut up_r, &mut to_pc);
                let _ = to_pc.shutdown(std::net::Shutdown::Both);
            });
        }
    });
    let front = Internet::start_lan(new_key(), 0).unwrap();
    front.serve_lan_to(port);
    let udp = SocketAddr::from(([127, 0, 0, 1], front.udp_port().unwrap()));
    let key = front.key_hex();
    Relay { _front: front, _back: back, key, udp }
}

#[test]
fn a_machine_in_the_middle_fails_the_confirmation_on_both_sides_even_with_the_right_code() {
    let b = booth(NetConfig::default());
    let m = relay(&b);
    let piano = Internet::start_lan(new_key(), 0).unwrap();
    // The PC dials the forged key; the request reaches the booth through the relay.
    let bridge = piano.bridge_to(lan_addr(&m.key, m.udp).unwrap()).unwrap().to_string();
    let id = pair_request(&bridge, "Piano PC").unwrap();
    let code = shown(&b.hub, &id).expect("the request reached the booth").code;
    // The right code typed on the PC: its check of the booth's confirmation fails (the booth
    // confirmed the relay's key, not the one the PC dialled), so it sends nothing more.
    let err = pair_confirm(&bridge, &id, code.clone(), keys(&m.key, &piano)).unwrap_err();
    assert_eq!(err, WRONG_OR_BETWEEN);
    assert_eq!(shown(&b.hub, &id).unwrap().wrong, 1, "the attempt counts on the booth");
    // A PC that sends its confirmation anyway: the booth's check fails too.
    let mut conn = Conn::open(&bridge);
    let (ex, mine) = Exchange::start(Side::Pc, &code, keys(&m.key, &piano));
    let v = conn.call(json!({"id": 1, "method": "pair.exchange", "params": {"request": id, "msg": b64_encode(&mine)}})).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    let theirs = b64_decode(v["result"]["msg"].as_str().unwrap()).unwrap();
    let c = ex.finish(&theirs).unwrap();
    let v = conn.call(json!({"id": 2, "method": "pair.finish", "params": {"confirm": b64_encode(&c.tag())}})).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap().contains("another computer is in between"), "{v}");
    assert!(conn.call(json!({"id": 3, "method": "hello", "params": {"token": "x"}})).is_none(), "and drops the connection");
    assert_eq!(shown(&b.hub, &id).unwrap().wrong, 2);
    assert!(b.hub.config().paired.is_empty(), "nothing paired");
    // The PC dialling the real booth with the same request still pairs (one try was left).
    let real = piano.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    let (token, _) = pair_confirm(&real, &id, code, keys(&b.key, &piano)).unwrap();
    assert_eq!(b.hub.config().device(&token).unwrap().key, piano.key_hex());
    b.hub.stop();
    b.hub.stop_secure();
}

#[test]
fn the_attempt_limit_still_locks_a_request() {
    let b = booth(NetConfig::default());
    let piano = Internet::start_lan(new_key(), 0).unwrap();
    let bridge = piano.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    let id = pair_request(&bridge, "Piano PC").unwrap();
    let real = shown(&b.hub, &id).unwrap().code;
    for _ in 0..3 {
        assert_eq!(pair_confirm(&bridge, &id, wrong_code(&real), keys(&b.key, &piano)).unwrap_err(), WRONG_OR_BETWEEN);
    }
    assert!(shown(&b.hub, &id).is_none(), "gone from the screen");
    let err = pair_confirm(&bridge, &id, real, keys(&b.key, &piano)).unwrap_err();
    assert!(err.contains("no tries left"), "even the right code is too late: {err}");
    assert!(b.hub.config().paired.is_empty());
    b.hub.stop();
    b.hub.stop_secure();
}

#[test]
fn a_pc_with_an_older_freedawm_is_told_to_update() {
    let b = booth(NetConfig::default());
    let piano = Internet::start_lan(new_key(), 0).unwrap();
    let bridge = piano.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    let mut conn = Conn::open(&bridge);
    // An older PC asks without a version: told to update, and no code is shown.
    let v = conn.call(json!({"id": 1, "method": "pair.request", "params": {"name": "Old PC"}})).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], UPDATE_PC);
    assert!(v["error"].as_str().unwrap().contains("update FreeDAWm on this computer"));
    assert!(b.hub.pending_pairings().is_empty());
    // The code in the clear is never compared, not even for a request a new PC made.
    let id = pair_request(&bridge, "Piano PC").unwrap();
    let code = shown(&b.hub, &id).unwrap().code;
    let v = conn.call(json!({"id": 2, "method": "pair.confirm", "params": {"request": id, "code": code}})).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], UPDATE_PC);
    assert!(b.hub.config().paired.is_empty());
    assert_eq!(shown(&b.hub, &id).unwrap().wrong, 0, "nothing was compared");
    // Nor over the phone's page.
    let (status, _) = http_request(b.plain, "POST", "/api/pair/confirm", None, Some(&json!({"request": id, "code": code}))).unwrap();
    assert_eq!(status, 403);
    assert!(b.hub.config().paired.is_empty());
    b.hub.stop();
    b.hub.stop_secure();
}

#[test]
fn a_booth_with_an_older_freedawm_is_never_sent_the_code() {
    // A booth that answers a request the old way (no version): the PC stops before any code.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = std::sync::Arc::clone(&seen);
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { break };
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
            log.lock().unwrap().push(line);
            writeln!(s, "{}", json!({"id": 1, "ok": true, "result": {"request": "abc", "pc": "Old booth"}})).unwrap();
        }
    });
    assert_eq!(pair_request(&addr, "Piano PC").unwrap_err(), UPDATE_BOOTH);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one call, no code");
    assert!(seen[0].contains("\"pairing\":2"), "{}", seen[0]);
}

#[test]
fn malformed_pairing_messages_are_refused_and_count() {
    let b = booth(NetConfig::default());
    let piano = Internet::start_lan(new_key(), 0).unwrap();
    let bridge = piano.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    // Without a request, or for one that does not exist: refused, nothing to count.
    for params in
        [json!({}), json!({"request": 5, "msg": 7}), json!({"request": "nope", "msg": "AAAA"}), json!({"request": "x".repeat(100)}), json!([1])]
    {
        let mut conn = Conn::open(&bridge);
        let v = conn.call(json!({"id": 1, "method": "pair.exchange", "params": params})).unwrap();
        assert_eq!(v["ok"], false, "{v}");
    }
    let mut conn = Conn::open(&bridge);
    let v = conn.call(json!({"id": 1, "method": "pair.finish", "params": {"confirm": "AAAA"}})).unwrap();
    assert_eq!(v["ok"], false, "a confirmation without an exchange: {v}");
    // For a real request: damaged messages each spend a try and end the connection.
    let id = pair_request(&bridge, "Piano PC").unwrap();
    let code = shown(&b.hub, &id).unwrap().code;
    let pc_msg = Exchange::start(Side::Pc, &code, keys(&b.key, &piano)).1;
    for (n, msg) in [json!("!!not base64!!"), json!(b64_encode(&pc_msg[..20]))].into_iter().enumerate() {
        let mut conn = Conn::open(&bridge);
        let v = conn.call(json!({"id": 1, "method": "pair.exchange", "params": {"request": id, "msg": msg}})).unwrap();
        assert_eq!(v["ok"], false, "{v}");
        assert!(conn.call(json!({"id": 2, "method": "hello", "params": {"token": "x"}})).is_none(), "closed");
        assert_eq!(usize::from(shown(&b.hub, &id).unwrap().wrong), n + 1);
    }
    // The last try: a good exchange and a damaged confirmation.
    let mut conn = Conn::open(&bridge);
    let v = conn.call(json!({"id": 1, "method": "pair.exchange", "params": {"request": id, "msg": b64_encode(&pc_msg)}})).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    let v = conn.call(json!({"id": 2, "method": "pair.finish", "params": {"confirm": [1, 2, 3]}})).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap().contains("0 tries left"), "{v}");
    assert!(shown(&b.hub, &id).is_none(), "locked");
    // An exchange interrupted by any other line fails too (a fresh request).
    let id = pair_request(&bridge, "Piano PC").unwrap();
    let code = shown(&b.hub, &id).unwrap().code;
    let mut conn = Conn::open(&bridge);
    let (ex, mine) = Exchange::start(Side::Pc, &code, keys(&b.key, &piano));
    let v = conn.call(json!({"id": 1, "method": "pair.exchange", "params": {"request": id, "msg": b64_encode(&mine)}})).unwrap();
    let c = ex.finish(&b64_decode(v["result"]["msg"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(conn.call(json!({"id": 2, "method": "sync.ping"})).unwrap()["ok"], false);
    let v = conn.call(json!({"id": 3, "method": "pair.finish", "params": {"confirm": b64_encode(&c.tag())}})).unwrap();
    assert_eq!(v["ok"], false, "the exchange was over: {v}");
    assert_eq!(shown(&b.hub, &id).unwrap().wrong, 1);
    // The booth still works: the right code pairs.
    let token = pair_confirm(&bridge, &id, code, keys(&b.key, &piano)).unwrap().0;
    assert!(b.hub.config().device(&token).is_some());
    b.hub.stop();
    b.hub.stop_secure();
}
