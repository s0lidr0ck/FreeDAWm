//! The encrypted local link: two in-process peers on this machine, the booth's QUIC endpoint
//! bridged to its encrypted-link listener, the remote PC's bridged to a local port.
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use soundcraft_net::config::NetConfig;
use soundcraft_net::hub::{Hub, LinkKind, http_request};
use soundcraft_net::internet::{Internet, lan_addr, new_key};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
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

/// Pairs over `conn`, reading the code off the booth's screen: the token.
fn pair(hub: &Hub, conn: &mut Conn, name: &str) -> String {
    let v = conn.call(json!({"id": 1, "method": "pair.request", "params": {"name": name}})).unwrap();
    let id = v["result"]["request"].as_str().unwrap_or_else(|| panic!("{v}")).to_string();
    let code = hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap().code;
    let v = conn.call(json!({"id": 2, "method": "pair.confirm", "params": {"request": id, "code": code}})).unwrap();
    v["result"]["token"].as_str().unwrap_or_else(|| panic!("{v}")).to_string()
}

#[test]
fn a_pc_pairs_over_the_encrypted_link_and_is_bound_to_its_key() {
    let b = booth(NetConfig::default());
    let piano = Internet::start_lan(new_key(), 0).unwrap();
    let bridge = piano.bridge_to(lan_addr(&b.key, b.udp).unwrap()).unwrap().to_string();
    let mut conn = Conn::open(&bridge);
    // The code on the booth's screen is still what pairs, with its attempt limit.
    let v = conn.call(json!({"id": 1, "method": "pair.request", "params": {"name": "Piano PC"}})).unwrap();
    let id = v["result"]["request"].as_str().unwrap().to_string();
    let real = b.hub.pending_pairings()[0].code.clone();
    let wrong = if real == "0000" { "1111" } else { "0000" };
    let v = conn.call(json!({"id": 2, "method": "pair.confirm", "params": {"request": id, "code": wrong}})).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    let v = conn.call(json!({"id": 3, "method": "pair.confirm", "params": {"request": id, "code": real}})).unwrap();
    let token = v["result"]["token"].as_str().unwrap().to_string();
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
    let mut conn = Conn::open(&bridge);
    let token = pair(&b.hub, &mut conn, "Piano PC");
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
