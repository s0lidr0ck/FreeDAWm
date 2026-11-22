//! The booth PIN: devices pair by typing a PIN set on the booth, with nobody at the booth to read
//! out a code; wrong PINs are counted across requests; never over the internet.

use serde_json::{Value, json};
use soundcraft_net::config::NetConfig;
use soundcraft_net::follower::{pair_confirm, pair_request, pair_request_info};
use soundcraft_net::hub::{Hub, PinState, http_request};
use soundcraft_net::internet::{Internet, lan_addr, new_key};
use soundcraft_net::pairing::exchange::LinkKeys;
use soundcraft_net::proto::RigStatus;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const PIN: &str = "482113";

/// A booth with the booth PIN set that still takes PCs over the plain link.
fn booth(extra: impl FnOnce(&mut NetConfig)) -> (Hub, SocketAddr) {
    let mut cfg = NetConfig { encrypted_only: false, ..NetConfig::default() };
    extra(&mut cfg);
    let hub = Hub::new(cfg, None);
    hub.set_booth_pin(PIN).unwrap();
    let port = hub.listen(0).unwrap();
    (hub, SocketAddr::from(([127, 0, 0, 1], port)))
}

/// A phone asks to pair: the booth's answer.
fn ask(addr: SocketAddr, name: &str) -> Value {
    let (code, r) = http_request(addr, "POST", "/api/pair/request", None, Some(&json!({"name": name}))).unwrap();
    assert_eq!(code, 200, "{r}");
    r
}

/// The phone types `typed` for its request: the status code and answer.
fn type_in(addr: SocketAddr, request: &Value, typed: &str) -> (u16, Value) {
    http_request(addr, "POST", "/api/pair/confirm", None, Some(&json!({"request": request["request"], "code": typed}))).unwrap()
}

/// One wrong PIN from a phone (a fresh request each time).
fn wrong_from_phone(addr: SocketAddr) {
    let r = ask(addr, "Guest");
    assert_eq!(r["pin"], true, "PIN pairing open: {r}");
    let (code, _) = type_in(addr, &r, "000000");
    assert_eq!(code, 403);
}

fn notices(hub: &Hub) -> Vec<String> {
    hub.notices().into_iter().map(|(_, t)| t).collect()
}

#[test]
fn without_a_pin_requests_get_a_code_as_before() {
    let hub = Hub::new(NetConfig::default(), None);
    let port = hub.listen(0).unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let r = ask(addr, "Phone");
    assert!(r.get("pin").is_none(), "the answer is unchanged: {r}");
    assert_eq!(hub.pending_pairings()[0].code.len(), 4);
    assert_eq!(hub.pin_state(), PinState::Unset);
    hub.stop();
}

#[test]
fn a_pc_pairs_with_the_right_pin_without_anyone_at_the_booth() {
    let (hub, addr) = booth(|_| {});
    let a = addr.to_string();
    let r = pair_request_info(&a, "Piano PC").unwrap();
    assert!(r.pin && !r.ask, "{r:?}");
    // Nothing is read off the booth's screen: the PIN is the exchange's password.
    let (token, _) = pair_confirm(&a, &r.request, PIN.to_string(), LinkKeys::PLAIN).unwrap();
    assert!(hub.config().paired.iter().any(|d| d.token == token && d.name == "Piano PC"));
    assert_eq!(notices(&hub), vec!["Piano PC paired with the booth PIN".to_string()]);
    hub.stop();
}

#[test]
fn a_pc_pairs_with_the_pin_over_the_encrypted_link_bound_to_its_key() {
    let hub = Hub::new(NetConfig::default(), None);
    hub.set_booth_pin(PIN).unwrap();
    hub.listen(0).unwrap();
    let lan = Internet::start_lan(new_key(), 0).unwrap();
    lan.serve_lan_to(hub.listen_secure().unwrap());
    let udp = SocketAddr::from(([127, 0, 0, 1], lan.udp_port().unwrap()));
    let piano = Internet::start_lan(new_key(), 0).unwrap();
    let bridge = piano.bridge_to(lan_addr(&lan.key_hex(), udp).unwrap()).unwrap().to_string();
    let id = pair_request(&bridge, "Piano PC").unwrap();
    let keys = LinkKeys::from_hex(&lan.key_hex(), &piano.key_hex()).unwrap();
    let (token, _) = pair_confirm(&bridge, &id, PIN.to_string(), keys).unwrap();
    let dev = hub.config().paired.into_iter().find(|d| d.token == token).unwrap();
    assert_eq!(dev.key, piano.key_hex());
    hub.stop();
    hub.stop_secure();
}

#[test]
fn a_phone_pairs_with_the_pin_and_a_wrong_one_counts() {
    let (hub, addr) = booth(|_| {});
    let r = ask(addr, "Phone");
    assert_eq!(r["pin"], true);
    let (code, v) = type_in(addr, &r, "111111");
    assert_eq!(code, 403);
    assert_eq!(v["left"], 2);
    assert!(hub.config().paired.is_empty());
    assert_eq!(hub.config().pin_wrong, 1, "counted, and kept in the settings");
    let (code, v) = type_in(addr, &r, &format!(" {PIN} "));
    assert_eq!(code, 200, "{v}");
    assert_eq!(hub.config().paired[0].name, "Phone");
    assert_eq!(hub.config().pin_wrong, 0, "a success starts the count again");
    assert!(notices(&hub).contains(&"Phone paired with the booth PIN".to_string()));
    hub.stop();
}

#[test]
fn five_wrong_pause_the_pause_doubles_and_the_right_pin_works_after_it() {
    let (hub, addr) = booth(|_| {});
    for _ in 0..5 {
        wrong_from_phone(addr);
    }
    match hub.pin_state() {
        PinState::Paused { secs_left, from } => {
            assert!(secs_left > 290.0 && secs_left <= 300.0, "{secs_left}");
            assert_eq!(from, "Guest");
        }
        s => panic!("{s:?}"),
    }
    assert!(notices(&hub).iter().any(|n| n == "PIN pairing paused for 5 minutes: 5 wrong tries from Guest"), "{:?}", notices(&hub));
    // Paused: even the right PIN does not pair; a request now gets a code on the booth's screen.
    let r = ask(addr, "Phone");
    assert!(r.get("pin").is_none(), "{r}");
    let (code, _) = type_in(addr, &r, PIN);
    assert_eq!(code, 403);
    hub.deny_pairing(r["request"].as_str().unwrap());
    // Over: five more pause twice as long.
    hub.age_pin_pause(301.0);
    assert_eq!(hub.pin_state(), PinState::On);
    for _ in 0..5 {
        wrong_from_phone(addr);
    }
    assert!(matches!(hub.pin_state(), PinState::Paused { secs_left, .. } if secs_left > 590.0));
    hub.age_pin_pause(601.0);
    let r = ask(addr, "Phone");
    let (code, v) = type_in(addr, &r, PIN);
    assert_eq!(code, 200, "{v}");
    hub.stop();
}

#[test]
fn twenty_wrong_turn_pin_pairing_off_until_the_owner_turns_it_back_on() {
    let dir = std::env::temp_dir().join(format!("freedawm-boothpin-{}", std::process::id()));
    let path = dir.join("network.json");
    let hub = Hub::new(NetConfig { encrypted_only: false, ..NetConfig::default() }, Some(path.clone()));
    hub.set_booth_pin(PIN).unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], hub.listen(0).unwrap()));
    for round in 0..4 {
        for _ in 0..5 {
            wrong_from_phone(addr);
        }
        if round < 3 {
            hub.age_pin_pause(3600.0);
        }
    }
    assert_eq!(hub.pin_state(), PinState::Off { from: "Guest".into() });
    assert!(notices(&hub).last().unwrap().starts_with("PIN pairing turned off after 20 wrong tries"));
    hub.stop();
    // Still off after the app restarts (the count is in the settings).
    let again = Hub::new(NetConfig::load(&path), Some(path.clone()));
    assert!(matches!(again.pin_state(), PinState::Off { .. }));
    let addr = SocketAddr::from(([127, 0, 0, 1], again.listen(0).unwrap()));
    let r = ask(addr, "Phone");
    assert!(r.get("pin").is_none(), "a code on the booth's screen instead: {r}");
    // The owner turns it back on.
    again.reset_pin_guard();
    assert_eq!(again.pin_state(), PinState::On);
    let r = ask(addr, "Phone");
    assert_eq!(type_in(addr, &r, PIN).0, 200);
    again.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pin_shorter_than_six_is_refused() {
    let hub = Hub::new(NetConfig::default(), None);
    let e = hub.set_booth_pin("12345").unwrap_err();
    assert!(e.contains("at least 6"), "{e}");
    assert_eq!(hub.pin_state(), PinState::Unset);
    hub.set_booth_pin("sunday-choir").unwrap();
    assert_eq!(hub.config().booth_pin, "sunday-choir");
}

#[test]
fn asking_first_needs_allow_on_the_booth() {
    let (hub, addr) = booth(|_| {});
    hub.set_pin_ask(true);
    let a = addr.to_string();
    let r = pair_request_info(&a, "Piano PC").unwrap();
    assert!(r.pin && r.ask);
    let shown = hub.pending_pairings();
    assert!(shown[0].pin && !shown[0].allowed && shown[0].code.is_empty(), "the request window shows Allow / Deny");
    let e = pair_confirm(&a, &r.request, PIN.to_string(), LinkKeys::PLAIN).unwrap_err();
    assert!(e.contains("Allow"), "{e}");
    assert!(hub.config().paired.is_empty());
    assert_eq!(hub.config().pin_wrong, 0, "not a try");
    hub.allow_pairing(&r.request);
    pair_confirm(&a, &r.request, PIN.to_string(), LinkKeys::PLAIN).unwrap();
    assert_eq!(hub.config().paired.len(), 1);
    hub.stop();
}

#[test]
fn the_pin_cannot_pair_over_the_internet_listener() {
    let (hub, _) = booth(|_| {});
    let port = hub.listen_internet().unwrap();
    let a = format!("127.0.0.1:{port}");
    let e = pair_request(&a, "Stranger").unwrap_err();
    assert!(e.contains("invite"), "{e}");
    assert!(hub.pending_pairings().is_empty());
    // Not even with the exchange sent straight away.
    let mut c = TcpStream::connect(&a).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut r = BufReader::new(c.try_clone().unwrap());
    writeln!(c, "{}", json!({"id": 1, "method": "pair.exchange", "params": {"request": "x", "msg": ""}})).unwrap();
    let mut s = String::new();
    r.read_line(&mut s).unwrap();
    let v: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["ok"], false);
    assert!(hub.config().paired.is_empty());
    hub.stop_internet();
    hub.stop();
}

#[test]
fn changing_or_clearing_the_pin_keeps_paired_devices() {
    let (hub, addr) = booth(|_| {});
    let r = ask(addr, "Phone");
    let token = type_in(addr, &r, PIN).1["token"].as_str().unwrap().to_string();
    hub.set_booth_pin("another-one").unwrap();
    assert_eq!(http_request(addr, "GET", "/api/status", Some(&token), None).unwrap().0, 200);
    hub.clear_booth_pin();
    assert_eq!(http_request(addr, "GET", "/api/status", Some(&token), None).unwrap().0, 200);
    assert_eq!(hub.pin_state(), PinState::Unset);
    // The old PIN no longer pairs once changed.
    hub.set_booth_pin("another-one").unwrap();
    let r = ask(addr, "Guest");
    assert_eq!(type_in(addr, &r, PIN).0, 403);
    // Forget all: every device must pair again.
    hub.forget_all();
    assert_eq!(http_request(addr, "GET", "/api/status", Some(&token), None).unwrap().0, 401);
    hub.stop();
}

#[test]
fn nothing_made_from_the_pin_is_announced_or_published() {
    let (hub, addr) = booth(|_| {});
    let r = ask(addr, "Phone");
    let token = type_in(addr, &r, PIN).1["token"].as_str().unwrap().to_string();
    hub.publish(RigStatus { pc: "Booth".into(), ..RigStatus::default() });
    let (_, status) = http_request(addr, "GET", "/api/status", Some(&token), None).unwrap();
    let (_, page) = http_request(addr, "GET", "/", None, None).unwrap();
    let asked = ask(addr, "Guest");
    let txt = soundcraft_net::discovery::txt("booth", Some((48100, &"ab".repeat(32))));
    for text in [status.to_string(), page.to_string(), asked.to_string(), format!("{txt:?}"), format!("{:?}", hub.pending_pairings())] {
        assert!(!text.contains(PIN), "{text}");
        assert!(!text.contains(&sha2_free_hex(PIN)), "{text}");
    }
    assert_eq!(txt.iter().map(|(k, _)| *k).collect::<Vec<_>>(), vec!["role", "v", "q", "k"]);
    hub.stop();
}

/// The PIN's bytes as hex (a crude stand-in for "anything made from it" in a test).
fn sha2_free_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}
