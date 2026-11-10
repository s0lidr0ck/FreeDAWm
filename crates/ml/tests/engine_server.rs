//! Song engines run by FreeDAWm (`engine_server`): an installed engine folder starts its server on
//! a free local port and waits for `/health`; a second song reuses it; it is not stopped for
//! idleness while a song holds it, and is once released; a server that dies while starting is an
//! error naming its log; `shutdown_all` stops everything.
//!
//! The stand-in server is this test program itself, copied into the engine folder's `bin/` and
//! started by the engine's card as `fake_server_main <port> --exact --include-ignored`: libtest
//! takes the port as a second (non-matching) filter, and `fake_server_main` reads it back.

use soundcraft_ml::engine_server;
use soundcraft_ml::models::{self, InstalledModel};
use soundcraft_ml::{Cancel, Device, MlError};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

/// The stand-in server: answers `/health` on the port given after its name, until killed. Exits at
/// once when that port is 0 (a server that dies while starting).
#[test]
#[ignore = "run by the engine_server tests as a stand-in song server"]
fn fake_server_main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(port) = args.iter().skip_while(|a| *a != "fake_server_main").nth(1).and_then(|p| p.parse::<u16>().ok()) else { return };
    if port == 0 || std::env::var_os("FAKE_SERVER_DIE").is_some() {
        eprintln!("fake server: no models here");
        std::process::exit(3);
    }
    let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
    for s in l.incoming().flatten() {
        let mut s = s;
        let mut line = String::new();
        let _ = BufReader::new(s.try_clone().unwrap()).read_line(&mut line);
        let body = r#"{"status":"ok"}"#;
        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    }
}

/// An engine folder `<models>/<id>/` holding this test program as its server.
fn engine(models_dir: &Path, id: &str, extra: &[&str]) -> InstalledModel {
    let dir = models_dir.join(id);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let exe = std::env::current_exe().unwrap();
    let name = if cfg!(windows) { "fake-server.exe" } else { "fake-server" };
    std::fs::copy(&exe, dir.join("bin").join(name)).unwrap();
    let mut args = vec!["fake_server_main", "{port}", "--exact", "--include-ignored", "--nocapture"];
    args.extend_from_slice(extra);
    let card = serde_json::json!({
        "id": id, "name": format!("Test {id} engine"), "file": format!("bin/{name}"), "task": "song_server",
        // The CPU flag must be one the stand-in (the test harness) accepts.
        "server_args": args, "device_arg": "--test-threads", "cpu_device": "1", "license": "MIT",
    });
    std::fs::write(dir.join("card.json"), serde_json::to_string_pretty(&card).unwrap()).unwrap();
    models::InstalledModel { card: models::read_card(models_dir, id).unwrap(), dir }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-engines-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn an_engine_starts_is_reused_and_stops() {
    let dir = scratch("lifecycle");
    let m = engine(&dir, "acestep", &[]);
    let said = std::sync::Mutex::new(Vec::new());
    let say = |w: &str| said.lock().unwrap().push(w.to_string());
    let a = engine_server::acquire(&m, Device::Auto, &Cancel::new(), &say).unwrap();
    assert!(a.url.starts_with("http://127.0.0.1:"), "{}", a.url);
    assert!(said.lock().unwrap().iter().any(|w| w.starts_with("Starting Test acestep engine")));
    // The command line: the port in place, and the CPU flag only when asked for.
    let (exe, args) = engine_server::command_line(&m, 4321, Device::Cpu).unwrap();
    assert!(exe.ends_with(if cfg!(windows) { "fake-server.exe" } else { "fake-server" }));
    assert_eq!(args.get(1).map(String::as_str), Some("4321"));
    assert!(args.ends_with(&["--test-threads".to_string(), "1".to_string()]));
    let (_, args) = engine_server::command_line(&m, 4321, Device::Auto).unwrap();
    assert!(!args.contains(&"--test-threads".to_string()));
    // A second song reuses it.
    let b = engine_server::acquire(&m, Device::Auto, &Cancel::new(), &|_| {}).unwrap();
    assert_eq!(a.url, b.url);
    // Held: not stopped for idleness.
    engine_server::stop_idle(Duration::ZERO);
    assert_eq!(engine_server::running().len(), 1);
    drop(a);
    drop(b);
    // Released and idle: stopped.
    engine_server::stop_idle(Duration::ZERO);
    assert!(engine_server::running().iter().all(|(id, _)| id != "acestep"));
    // Another device starts it again (the CPU flag is part of its command line).
    let c = engine_server::acquire(&m, Device::Cpu, &Cancel::new(), &|_| {}).unwrap();
    assert!(c.url.starts_with("http://127.0.0.1:"));
    drop(c);
    engine_server::shutdown_all();
    assert!(engine_server::running().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_server_that_dies_while_starting_is_an_error_with_its_log() {
    let dir = scratch("dies");
    // The stand-in exits at once for port 0: give it 0 instead of the port.
    let m = engine(&dir, "yue2", &[]);
    let card_path = dir.join("yue2").join("card.json");
    let card = std::fs::read_to_string(&card_path).unwrap().replace("{port}", "0");
    std::fs::write(&card_path, card).unwrap();
    let m = InstalledModel { card: models::read_card(&dir, "yue2").unwrap(), ..m };
    let e = engine_server::acquire(&m, Device::Auto, &Cancel::new(), &|_| {}).unwrap_err();
    assert!(matches!(e, MlError::Runtime(ref w) if w.contains("stopped while starting") && w.contains("no models here")), "{e:?}");
    // A card without server_args is refused, not run.
    let mut bad = m.clone();
    bad.card.extra.remove("server_args");
    assert!(engine_server::acquire(&bad, Device::Auto, &Cancel::new(), &|_| {}).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
