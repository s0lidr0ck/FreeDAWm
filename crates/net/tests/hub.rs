//! The listener on a loopback port: pairing, the status feed, the page, the control channel.
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use soundcraft_net::config::NetConfig;
use soundcraft_net::hub::{Hub, HubRequest, http_request, read_events};
use soundcraft_net::proto::{RigStatus, TransportState};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

fn hub() -> (Hub, SocketAddr) {
    let hub = Hub::new(NetConfig::default(), None);
    let port = hub.listen(0).unwrap();
    (hub, SocketAddr::from(([127, 0, 0, 1], port)))
}

/// Pairs a browser through the HTTP API, reading the code off "the booth's screen".
fn pair_browser(hub: &Hub, addr: SocketAddr, name: &str) -> String {
    let (code, r) = http_request(addr, "POST", "/api/pair/request", None, Some(&json!({"name": name}))).unwrap();
    assert_eq!(code, 200, "{r}");
    let id = r["request"].as_str().unwrap().to_string();
    let shown = hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap();
    assert_eq!(shown.name, name);
    let (code, r) = http_request(addr, "POST", "/api/pair/confirm", None, Some(&json!({"request": id, "code": shown.code}))).unwrap();
    assert_eq!(code, 200, "{r}");
    r["token"].as_str().unwrap().to_string()
}

#[test]
fn an_unpaired_browser_gets_only_the_pairing_prompt() {
    let (hub, addr) = hub();
    let (code, page) = http_request(addr, "GET", "/", None, None).unwrap();
    assert_eq!(code, 200);
    assert!(page.as_str().unwrap().contains("FreeDAWm"), "the page itself is served");
    for (method, path) in [("GET", "/api/status"), ("GET", "/api/events"), ("POST", "/api/action")] {
        let (code, r) = http_request(addr, method, path, None, Some(&json!({"action": "record"}))).unwrap();
        assert_eq!(code, 401, "{path}");
        assert_eq!(r["pair"], true);
        let (code, _) = http_request(addr, method, path, Some("guess"), Some(&json!({"action": "record"}))).unwrap();
        assert_eq!(code, 401, "a made-up token is refused too");
    }
    // Nothing reached the app.
    assert!(hub.drain().is_empty());
    hub.stop();
}

#[test]
fn a_wrong_code_does_not_pair_and_the_right_one_does() {
    let (hub, addr) = hub();
    let (_, r) = http_request(addr, "POST", "/api/pair/request", None, Some(&json!({"name": "Phone"}))).unwrap();
    let id = r["request"].as_str().unwrap();
    let real = hub.pending_pairings()[0].code.clone();
    let wrong = if real == "0000" { "1111" } else { "0000" };
    let (code, r) = http_request(addr, "POST", "/api/pair/confirm", None, Some(&json!({"request": id, "code": wrong}))).unwrap();
    assert_eq!(code, 403);
    assert_eq!(r["left"], 2);
    assert!(hub.config().paired.is_empty());
    let (code, r) = http_request(addr, "POST", "/api/pair/confirm", None, Some(&json!({"request": id, "code": real}))).unwrap();
    assert_eq!(code, 200);
    let token = r["token"].as_str().unwrap();
    assert_eq!(hub.config().paired.len(), 1, "remembered");
    assert_eq!(hub.config().paired[0].name, "Phone");
    let (code, _) = http_request(addr, "GET", "/api/status", Some(token), None).unwrap();
    assert_eq!(code, 200);
    // Forget: the token no longer works.
    hub.forget(token);
    let (code, _) = http_request(addr, "GET", "/api/status", Some(token), None).unwrap();
    assert_eq!(code, 401);
    hub.stop();
}

#[test]
fn the_status_feed_pushes_changes_to_every_client() {
    let (hub, addr) = hub();
    let phone = pair_browser(&hub, addr, "Phone");
    let stage = pair_browser(&hub, addr, "Stage PC");
    hub.publish(RigStatus { pc: "Booth".into(), session: "Sunday".into(), ..RigStatus::default() });
    let watch = |token: String| {
        std::thread::spawn(move || {
            read_events(addr, &format!("/api/events?token={token}"), Duration::from_secs(10), |v| v["transport"] == "recording").unwrap()
        })
    };
    let (a, b) = (watch(phone), watch(stage));
    // Both see the first status, then the change.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(hub.clients().len(), 2, "{:?}", hub.clients());
    hub.publish(RigStatus {
        pc: "Booth".into(),
        session: "Sunday".into(),
        transport: TransportState::Recording,
        record_elapsed: 1.5,
        ..RigStatus::default()
    });
    for h in [a, b] {
        let v = h.join().unwrap();
        assert_eq!(v["session"], "Sunday");
        assert_eq!(v["record_elapsed"], 1.5);
        let clients: Vec<String> = serde_json::from_value(v["clients"].clone()).unwrap();
        assert!(clients.contains(&"Phone".to_string()) && clients.contains(&"Stage PC".to_string()));
    }
    hub.stop();
}

#[test]
fn actions_reach_the_app_and_come_back() {
    let (hub, addr) = hub();
    let token = pair_browser(&hub, addr, "Phone");
    let h2 = hub.clone();
    let app = std::thread::spawn(move || {
        for _ in 0..200 {
            for r in h2.drain() {
                if let HubRequest::Action { action, device, reply } = r {
                    let _ = reply.send(Ok(json!({"did": format!("{action:?}"), "by": device})));
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let (code, r) = http_request(addr, "POST", "/api/action", Some(&token), Some(&json!({"action": "record"}))).unwrap();
    app.join().unwrap();
    assert_eq!(code, 200, "{r}");
    assert_eq!(r["result"]["did"], "Record");
    assert_eq!(r["result"]["by"], "Phone");
    let (code, _) = http_request(addr, "POST", "/api/action", Some(&token), Some(&json!({"action": "erase_disk"}))).unwrap();
    assert_eq!(code, 409, "unknown actions are refused");
    hub.stop();
}

fn line(c: &mut TcpStream, r: &mut BufReader<TcpStream>, v: Value) -> Value {
    writeln!(c, "{v}").unwrap();
    let mut s = String::new();
    r.read_line(&mut s).unwrap();
    serde_json::from_str(&s).unwrap()
}

#[test]
fn a_pc_pairs_over_the_control_channel_and_then_controls() {
    let (hub, addr) = hub();
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut r = BufReader::new(c.try_clone().unwrap());
    let v = line(&mut c, &mut r, json!({"id": 1, "method": "engine.execute", "params": {"command": "transport.record"}}));
    assert_eq!(v["ok"], false, "unpaired: nothing but pairing");
    assert_eq!(v["pair"], true);
    let v = line(&mut c, &mut r, json!({"id": 2, "method": "hello", "params": {"token": "nope"}}));
    assert_eq!(v["ok"], false);
    let v = line(&mut c, &mut r, json!({"id": 3, "method": "pair.request", "params": {"name": "Piano PC"}}));
    let id = v["result"]["request"].as_str().unwrap().to_string();
    let code = hub.pending_pairings()[0].code.clone();
    assert_eq!(hub.pending_pairings()[0].kind, "pc");
    let v = line(&mut c, &mut r, json!({"id": 4, "method": "pair.confirm", "params": {"request": id, "code": code}}));
    let token = v["result"]["token"].as_str().unwrap().to_string();
    let v = line(&mut c, &mut r, json!({"id": 5, "method": "hello", "params": {"token": token, "name": "Piano PC"}}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["id"], 5);
    // Any control-protocol method now goes to the app.
    let h2 = hub.clone();
    let app = std::thread::spawn(move || {
        for _ in 0..200 {
            for req in h2.drain() {
                if let HubRequest::Control { method, reply, .. } = req {
                    let _ = reply.send(json!({"ok": true, "result": {"method": method}}));
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let v = line(&mut c, &mut r, json!({"id": 6, "method": "session.inspect"}));
    app.join().unwrap();
    assert_eq!(v["result"]["method"], "session.inspect");
    assert_eq!(v["id"], 6);
    hub.stop();
}

/// A paired PC on the control channel: its stream, reader and the hub.
fn paired_pc(hub: &Hub, addr: SocketAddr) -> (TcpStream, BufReader<TcpStream>) {
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut r = BufReader::new(c.try_clone().unwrap());
    let v = line(&mut c, &mut r, json!({"id": 1, "method": "pair.request", "params": {"name": "Guitar PC"}}));
    let id = v["result"]["request"].as_str().unwrap().to_string();
    let code = hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap().code;
    let v = line(&mut c, &mut r, json!({"id": 2, "method": "pair.confirm", "params": {"request": id, "code": code}}));
    let token = v["result"]["token"].as_str().unwrap().to_string();
    let v = line(&mut c, &mut r, json!({"id": 3, "method": "hello", "params": {"token": token, "name": "Guitar PC"}}));
    assert_eq!(v["ok"], true);
    (c, r)
}

#[test]
fn an_audio_transfer_resumes_where_it_was_cut_off() {
    use soundcraft_net::audio::{b64_encode, hash};
    let (hub, addr) = hub();
    let dir = std::env::temp_dir().join(format!("freedawm-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    hub.set_project(soundcraft_net::hub::ProjectInfo { folder: Some(dir.clone()), rate: 48_000, bits: 24 });
    let data: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let offer =
        json!({"id": 10, "method": "take.offer", "params": {"id": "g-1-a1", "file": "g-1-a1.flac", "bytes": data.len(), "hash": hash(&data)}});
    let (mut c, mut r) = paired_pc(&hub, addr);
    assert_eq!(line(&mut c, &mut r, offer.clone())["result"]["have"], 0);
    let v =
        line(&mut c, &mut r, json!({"id": 11, "method": "take.chunk", "params": {"id": "g-1-a1", "offset": 0, "data": b64_encode(&data[..40_000])}}));
    assert_eq!(v["result"]["have"], 40_000);
    // Out of order: refused, the booth says where it is.
    let v = line(
        &mut c,
        &mut r,
        json!({"id": 12, "method": "take.chunk", "params": {"id": "g-1-a1", "offset": 60_000, "data": b64_encode(&data[60_000..])}}),
    );
    assert_eq!(v["result"]["have"], 40_000);
    // The connection drops; a new one offers again and goes on from 40 000.
    drop((c, r));
    let (mut c, mut r) = paired_pc(&hub, addr);
    assert_eq!(line(&mut c, &mut r, offer)["result"]["have"], 40_000);
    let v = line(
        &mut c,
        &mut r,
        json!({"id": 13, "method": "take.chunk", "params": {"id": "g-1-a1", "offset": 40_000, "data": b64_encode(&data[40_000..])}}),
    );
    assert_eq!(v["result"]["have"], 100_000);
    let got = std::fs::read(dir.join("Remote Takes").join("g-1-a1.flac")).unwrap();
    assert_eq!(got, data, "whole and in the project's Remote Takes");
    // A take whose file never arrived is refused (and its sender tries again later).
    let env = json!({"id": "g-2-a1", "booth_take": 2, "pc": "Guitar PC", "anchor": {"play_from": 0.0, "record_start": 0.0}, "sync": [],
        "payload": {"kind": "audio", "input": "In 1", "rate": 48000, "channels": 1, "t0": 0.0, "file": "g-2-a1.flac", "bytes": 10}});
    let v = line(&mut c, &mut r, json!({"id": 14, "method": "take.transfer", "params": env}));
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("has not arrived"), "{v}");
    hub.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
