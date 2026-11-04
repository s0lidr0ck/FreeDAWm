//! Model downloads in the app, against a local HTTP server (no real network): the job dialog's
//! Download button, the personal-use confirm, and the control methods.
#![cfg(not(target_arch = "wasm32"))]

use egui::vec2;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use serde_json::json;
use soundcraft_engine::ml::ModelStatus;
use soundcraft_engine::ml::catalog::{Manifest, ManifestModel};
use soundcraft_ui_egui::{Services, SoundApp, ml_downloads, ml_ui};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type Routes = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// A small HTTP server on 127.0.0.1; its base URL.
fn serve(routes: Routes) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let routes = routes.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                }
                let body = routes.lock().unwrap().get(&path).cloned();
                let _ = match body {
                    Some(b) => stream
                        .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len()).as_bytes())
                        .and_then(|()| stream.write_all(&b)),
                    None => stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                };
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-ui-ml-downloads-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fake model zip for `id` and its manifest entry.
fn fake(scratch: &Path, id: &str, personal: bool) -> (Vec<u8>, ManifestModel) {
    let onnx = scratch.join(format!("{id}.onnx"));
    std::fs::write(&onnx, format!("fake {id} weights")).unwrap();
    let card = json!({"id": id, "file": "model.onnx", "sha256": soundcraft_engine::ml::sha256_file(&onnx).unwrap(),
        "license": if personal { "Weights: no licence stated by the authors." } else { "MIT" }, "exported_utc": "2026-10-22T00:00:00Z"});
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("card.json", o).unwrap();
    w.write_all(card.to_string().as_bytes()).unwrap();
    w.start_file("model.onnx", o).unwrap();
    w.write_all(&std::fs::read(&onnx).unwrap()).unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let z = scratch.join(format!("{id}.zip"));
    std::fs::write(&z, &bytes).unwrap();
    let entry = ManifestModel {
        id: id.into(),
        version: "2026-10-22T00:00:00Z".into(),
        url: format!("{id}.zip"),
        size: bytes.len() as u64,
        sha256: soundcraft_engine::ml::sha256_file(&z).unwrap(),
        license: if personal { "Licence not stated by the author — personal use".into() } else { "MIT".into() },
        personal_use_only: personal,
        ..Default::default()
    };
    (bytes, entry)
}

/// A server holding btc_chords, srs_small (MIT) and bs_roformer_sw (personal use); its manifest URL.
fn release(scratch: &Path) -> String {
    let routes: Routes = Arc::default();
    let base = serve(routes.clone());
    let mut entries = Vec::new();
    for (id, personal) in [("btc_chords", false), ("srs_small", false), ("bs_roformer_sw", true)] {
        let (bytes, e) = fake(scratch, id, personal);
        routes.lock().unwrap().insert(format!("/r/{id}.zip"), bytes);
        entries.push(e);
    }
    let m = Manifest { format: 1, release: "models-v1".into(), models: entries };
    routes.lock().unwrap().insert("/r/models.json".into(), serde_json::to_vec(&m).unwrap());
    format!("{base}/r/models.json")
}

fn installed(app: &SoundApp, dir: &Path, id: &str) -> bool {
    let _ = app;
    soundcraft_engine::ml::list_models(dir).iter().any(|m| m.model.id == id && matches!(m.status, ModelStatus::Installed(_)))
}

/// Steps the downloads until the queue is empty.
fn settle(app: &mut SoundApp) {
    let ctx = egui::Context::default();
    for _ in 0..400 {
        ml_ui::step(app, &ctx);
        let d = &app.ml.downloads;
        if d.active.is_none() && d.queue.is_empty() && !matches!(d.manifest, ml_downloads::ManifestState::Loading(_)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the downloads did not finish");
}

#[test]
fn a_job_dialog_downloads_its_missing_model_and_then_can_run() {
    let scratch = tmp("dialog-src");
    let dir = tmp("dialog-models");
    let url = release(&scratch);
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    ml_downloads::set_source(&mut app, dir.clone(), Some(url));
    let track = app.engine.session().tracks.iter().find(|t| t.name == "Bass").map(|t| t.id.0).unwrap();
    ml_ui::open(&mut app, "ml.chords", Some((json!({"track": track}), "Bass".into()))).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    assert!(h.query_by_label_contains("Not installed: btc_chords").is_some());
    h.get_by_label("Download").click();
    for _ in 0..400 {
        h.run_steps(1);
        if installed(h.state(), &dir, "btc_chords") && h.state().ml.downloads.active.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    h.run_steps(3);
    assert!(installed(h.state(), &dir, "btc_chords"), "downloaded from the dialog");
    assert!(h.query_by_label_contains("Not installed").is_none(), "the dialog no longer says it's missing");
    assert!(h.state().ml.dialog.is_some(), "the dialog stays open to run");
    assert!(!h.get_by_label("Run").accesskit_node().is_disabled(), "and Run is enabled");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn only_personal_use_models_ask_first_and_control_calls_need_the_confirm() {
    let scratch = tmp("confirm-src");
    let dir = tmp("confirm-models");
    let url = release(&scratch);
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    ml_downloads::set_source(&mut app, dir.clone(), Some(url));

    let v = app.run("ml.models_available", json!({})).unwrap();
    assert_eq!(v["manifest"], "ready");
    let row = |v: &serde_json::Value, id: &str| v["models"].as_array().unwrap().iter().find(|m| m["id"] == id).cloned().unwrap();
    assert_eq!(row(&v, "bs_roformer_sw")["personal_use_only"], true);
    assert_eq!(row(&v, "srs_small")["available"], true);
    assert_eq!(row(&v, "whisper_lyrics")["available"], false);

    // A click on an MIT model downloads it at once; no confirm.
    ml_downloads::request(&mut app, &["srs_small".into()], false);
    assert!(app.ml.downloads.confirm.is_none());
    settle(&mut app);
    assert!(installed(&app, &dir, "srs_small"));

    // Download all with a personal-use model in it: the confirm names only that one.
    ml_downloads::request(&mut app, &["btc_chords".into(), "bs_roformer_sw".into()], false);
    let c = app.ml.downloads.confirm.clone().unwrap();
    assert_eq!((c.personal, c.others), (vec!["bs_roformer_sw".to_string()], vec!["btc_chords".to_string()]));
    assert!(app.ml.downloads.active.is_none() && app.ml.downloads.queue.is_empty(), "nothing starts before the answer");
    app.ml.downloads.confirm = None;

    // Programmatic calls never open the confirm: they need confirm_personal_use.
    let err = app.run("ml.download", json!({"ids": ["bs_roformer_sw"]})).unwrap_err();
    assert!(err.contains("personal use"), "{err}");
    assert!(app.run("ml.download", json!({"id": "whisper_lyrics"})).is_err(), "not in the release");
    app.run("ml.download", json!({"ids": ["bs_roformer_sw"], "confirm_personal_use": true})).unwrap();
    settle(&mut app);
    assert!(installed(&app, &dir, "bs_roformer_sw"));
    app.run("ml.remove", json!({"id": "bs_roformer_sw"})).unwrap();
    assert!(!installed(&app, &dir, "bs_roformer_sw"));
    assert!(app.run("ml.download_cancel", json!({})).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn an_unreachable_release_says_so_and_keeps_the_manual_route() {
    let dir = tmp("offline");
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    // A port nothing listens on.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    ml_downloads::set_source(&mut app, dir.clone(), Some(format!("http://127.0.0.1:{port}/models.json")));
    app.run("window.ml_models", json!({"value": true})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    for _ in 0..200 {
        h.run_steps(1);
        if matches!(h.state().ml.downloads.manifest, ml_downloads::ManifestState::Failed(_)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    h.run_steps(2);
    assert!(h.query_by_label_contains("Can't reach the FreeDAWm models release").is_some());
    assert!(h.query_by_label_contains("cargo xtask ml-models").is_some(), "the manual route stays");
    assert!(h.query_by_label("Try again").is_some());
    let buttons = h.query_all_by_label_contains("Download").filter(|n| n.accesskit_node().role() == egui::accesskit::Role::Button).count();
    assert_eq!(buttons, 0, "no Download buttons without the release");
    let _ = std::fs::remove_dir_all(&dir);
}
