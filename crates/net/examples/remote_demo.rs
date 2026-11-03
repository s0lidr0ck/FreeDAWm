//! Serves the service remote page with a made-up rig, to look at it in a browser:
//! `cargo run -p soundcraft-net --example remote_demo [port]`, then open
//! `http://localhost:<port>/?demo` (the browser is paired as "Phone" by the page's own token
//! `demo`; type `localStorage.setItem("freedawm.remote.token","demo")` once, or pair with the
//! code printed here). Runs until killed.

use soundcraft_net::config::{NetConfig, PairedDevice};
use soundcraft_net::follower::{FollowTarget, Follower};
use soundcraft_net::hub::{Hub, HubRequest};
use soundcraft_net::proto::{FollowerReport, MarkerStatus, RigStatus, TrackStatus, TransportState};
use std::time::{Duration, Instant};

fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(48199);
    let mut cfg = NetConfig { enabled: true, pc_name: "Booth".into(), ..NetConfig::default() };
    cfg.paired.push(PairedDevice { name: "Phone".into(), kind: "browser".into(), token: "demo".into(), follows: false });
    cfg.paired.push(PairedDevice { name: "Piano PC".into(), kind: "pc".into(), token: "piano".into(), follows: true });
    let hub = Hub::new(cfg, None);
    let port = match hub.listen(port) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    println!("http://localhost:{port}/");
    let f = Follower::start(
        FollowTarget { booth_name: String::new(), addr: format!("127.0.0.1:{port}"), token: "piano".into(), my_name: "Piano PC".into() },
        None,
        None,
    );
    f.set_report(FollowerReport {
        state: "recording".into(),
        notes: 1284,
        disk_free: Some(88_000_000_000),
        queued: 0,
        ports: vec!["Digital Piano".into()],
        ..FollowerReport::default()
    });
    let start = Instant::now();
    let tracks = [
        ("Vocal 1", true, None),
        ("Vocal 2", true, None),
        ("Acoustic", true, None),
        ("Piano MIDI", true, Some("Piano PC")),
        ("Room L/R", false, None),
    ];
    loop {
        for r in hub.drain() {
            match r {
                HubRequest::Action { action, reply, .. } => {
                    println!("action: {action:?}");
                    let _ = reply.send(Ok(serde_json::json!({"id": 1})));
                }
                HubRequest::Control { reply, .. } => {
                    let _ = reply.send(serde_json::json!({"ok": false, "error": "demo"}));
                }
                HubRequest::Take { reply, .. } => {
                    let _ = reply.send(Err("demo".into()));
                }
            }
        }
        for p in hub.pending_pairings() {
            println!("pairing code for {}: {}", p.name, p.code);
        }
        let t = start.elapsed().as_secs_f64();
        let wobble = |k: f64| (0.45 + 0.25 * (t * 3.0 + k).sin()).clamp(0.0, 1.0) as f32;
        hub.publish(RigStatus {
            pc: "Booth".into(),
            session: "Sunday 11-02".into(),
            transport: TransportState::Recording,
            position: 4325.0 + t,
            bar: 612,
            record_elapsed: 2537.0 + t,
            tracks: tracks
                .iter()
                .enumerate()
                .map(|(i, (n, armed, remote))| TrackStatus {
                    id: i as u64 + 1,
                    name: (*n).into(),
                    armed: *armed,
                    level: if *armed { wobble(i as f64) } else { 0.0 },
                    remote: remote.map(str::to_string),
                })
                .collect(),
            markers: vec![
                MarkerStatus { id: 1, number: 1, name: "Welcome".into(), at: 1800.0 },
                MarkerStatus { id: 2, number: 2, name: "Sermon".into(), at: 3900.0 },
            ],
            disk_free: Some(412_000_000_000),
            record_hours_left: Some(61.0),
            ..RigStatus::default()
        });
        std::thread::sleep(Duration::from_millis(66));
    }
}
