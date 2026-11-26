//! Internet invites on the booth's internet listener (loopback here, as the bridge reaches it):
//! one-time ones as before, reusable ones for several performers, revoking, a restart, the limit
//! on wrong secrets, and a performer coming back on a later day without a new invite.

use serde_json::{Value, json};
use soundcraft_net::config::NetConfig;
use soundcraft_net::follower::{pair_invite, pair_invite_as};
use soundcraft_net::hub::{Hub, MAX_BAD_INVITES};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

fn booth(path: Option<std::path::PathBuf>) -> (Hub, String) {
    let cfg = path.as_deref().map(NetConfig::load).unwrap_or_default();
    let hub = Hub::new(cfg, path);
    let port = hub.listen_internet().unwrap();
    (hub, format!("127.0.0.1:{port}"))
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("freedawm-invites-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("network.json")
}

/// `hello` with `token` over the internet listener at `addr`: the answer.
fn hello(addr: &str, token: &str) -> Value {
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut r = BufReader::new(c.try_clone().unwrap());
    writeln!(c, "{}", json!({"id": 1, "method": "hello", "params": {"token": token}})).unwrap();
    let mut s = String::new();
    r.read_line(&mut s).unwrap();
    serde_json::from_str(&s).unwrap()
}

fn notices(hub: &Hub) -> Vec<String> {
    hub.notices().into_iter().map(|(_, t)| t).collect()
}

#[test]
fn a_one_time_invite_still_pairs_once() {
    let (hub, addr) = booth(None);
    let secret = hub.new_invite_secret();
    assert_eq!(secret.len(), 32, "128 random bits as hex");
    let list = hub.invites();
    assert_eq!(list.len(), 1);
    assert!(!list[0].reusable && list[0].secs_left > 29.0 * 60.0);
    pair_invite(&addr, &secret, "Friend PC").unwrap();
    assert!(pair_invite(&addr, &secret, "Someone else").is_err(), "once");
    assert!(hub.invites().is_empty());
    assert!(notices(&hub).contains(&"Friend PC joined with an invite".to_string()));
    hub.stop_internet();
}

#[test]
fn a_reusable_invite_pairs_its_joins_then_refuses() {
    let (hub, addr) = booth(None);
    let secret = hub.new_reusable_invite(86_400, 3);
    let mut tokens = Vec::new();
    for name in ["Alto PC", "Bass PC", "Drums PC"] {
        tokens.push(pair_invite(&addr, &secret, name).unwrap().0);
    }
    tokens.sort();
    tokens.dedup();
    assert_eq!(tokens.len(), 3, "three devices, three tokens");
    let e = pair_invite(&addr, &secret, "Fourth PC").unwrap_err();
    assert!(e.contains("expired or was already used"), "{e}");
    assert_eq!(hub.config().paired.len(), 3);
    assert!(hub.invites().is_empty(), "used up");
    assert!(notices(&hub).contains(&"Bass PC joined with the reusable invite".to_string()));
    hub.stop_internet();
}

#[test]
fn a_reusable_invite_expires_and_can_be_revoked_and_its_devices_stay() {
    let (hub, addr) = booth(None);
    let hour = hub.new_reusable_invite(3600, 10);
    let token = pair_invite(&addr, &hour, "Alto PC").unwrap().0;
    let open = hub.invites();
    assert!(open[0].reusable && open[0].joins == 1 && open[0].max_joins == 10);
    hub.age_invites(3601.0);
    assert!(pair_invite(&addr, &hour, "Late PC").is_err(), "expired");
    let day = hub.new_reusable_invite(86_400, 10);
    let id = hub.invites().into_iter().find(|i| i.reusable).unwrap().id;
    hub.revoke_invite(&id);
    assert!(pair_invite(&addr, &day, "Bass PC").is_err(), "revoked at once");
    assert_eq!(hello(&addr, &token)["ok"], true, "a device that joined stays paired");
    hub.stop_internet();
}

#[test]
fn a_reusable_invite_survives_a_restart_and_the_file_holds_no_live_secret() {
    let path = temp("restart");
    let (hub, addr) = booth(Some(path.clone()));
    let secret = hub.new_reusable_invite(86_400, 5);
    pair_invite(&addr, &secret, "Alto PC").unwrap();
    hub.stop_internet();
    drop(hub);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(&secret), "only its hash is kept");
    let (again, addr) = booth(Some(path.clone()));
    let open = again.invites();
    assert_eq!((open.len(), open[0].joins), (1, 1));
    pair_invite(&addr, &secret, "Bass PC").unwrap();
    assert_eq!(again.config().paired.len(), 2);
    again.stop_internet();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn wrong_secrets_are_limited() {
    let (hub, addr) = booth(None);
    let secret = hub.new_reusable_invite(86_400, 10);
    for i in 0..MAX_BAD_INVITES {
        assert!(pair_invite(&addr, &format!("guess{i}"), "Guesser").is_err());
    }
    let e = pair_invite(&addr, &secret, "Alto PC").unwrap_err();
    assert!(e.contains("too many wrong invites"), "even the right one waits: {e}");
    hub.forget_bad_invites();
    pair_invite(&addr, &secret, "Alto PC").unwrap();
    hub.stop_internet();
}

#[test]
fn the_same_device_rejoining_is_recognised_without_using_a_join() {
    let (hub, addr) = booth(None);
    let secret = hub.new_reusable_invite(86_400, 2);
    let (token, _) = pair_invite(&addr, &secret, "Alto PC").unwrap();
    let (again, _) = pair_invite_as(&addr, &secret, "Alto PC", Some(&token)).unwrap();
    assert_eq!(again, token, "the same pairing");
    assert_eq!(hub.invites()[0].joins, 1, "no join used");
    assert_eq!(hub.config().paired.len(), 1);
    // A token the booth does not know is just a new join.
    let (other, _) = pair_invite_as(&addr, &secret, "Bass PC", Some("not-a-token")).unwrap();
    assert_ne!(other, token);
    assert!(hub.invites().is_empty(), "both joins used");
    hub.stop_internet();
}

#[test]
fn a_performer_comes_back_another_day_without_a_new_invite() {
    // Paired once by invite; the booth restarts (its settings kept) and opens another session:
    // the performer's token is all he needs, over the internet listener as before.
    let path = temp("comeback");
    let (hub, addr) = booth(Some(path.clone()));
    let secret = hub.new_invite_secret();
    let (token, _) = pair_invite(&addr, &secret, "Friend PC").unwrap();
    hub.stop_internet();
    drop(hub);
    let (again, addr) = booth(Some(path.clone()));
    assert!(again.invites().is_empty(), "no invite open");
    let v = hello(&addr, &token);
    assert_eq!(v["ok"], true, "{v}");
    again.stop_internet();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_dial_only_endpoint_joins_by_invite_and_answers_no_one() {
    // FreeDAWm Recorder's endpoint: it reaches the booth through the bridge, as the DAW's does,
    // and pairs with the invite; another endpoint dialling it gets no answer.
    use soundcraft_net::internet::{Internet, Invite, Relays, new_key};
    let (hub, _) = booth(None);
    let host = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    host.serve_to(hub.listen_internet().unwrap());
    let code = Invite { addr: host.addr(Duration::from_secs(1)), secret: hub.new_invite_secret(), name: "Booth".into() }.code();
    let me = Internet::start_dial_only(new_key(), Relays::LocalOnly).unwrap();
    let inv = Invite::parse(&code).unwrap();
    let bridge = me.bridge_to_json(&serde_json::to_string(&inv.addr).unwrap()).unwrap().to_string();
    let (token, booth_name) = pair_invite(&bridge, &inv.secret, "Recorder PC").unwrap();
    assert_eq!(booth_name, hub.pc_name());
    assert_eq!(hello(&bridge, &token)["ok"], true, "it follows through the bridge");
    assert_eq!(me.quality().len(), 1, "one connection, the one it dialled");

    // Dialled, it refuses: its address (direct, on this machine) taken by another endpoint.
    let stranger = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    let mine = me.addr(Duration::from_millis(200));
    assert!(mine.ip_addrs().next().is_some(), "{mine:?}");
    let back = stranger.bridge_to(mine).unwrap().to_string();
    assert!(pair_invite(&back, &inv.secret, "Stranger").is_err(), "nothing answers on the dial-only endpoint");
    assert!(stranger.quality().is_empty(), "no connection was made to it");
    hub.stop_internet();
}
