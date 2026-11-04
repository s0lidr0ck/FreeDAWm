//! Model downloads end to end against a local HTTP server (no real network): a tiny fake model
//! zip and its manifest, served from 127.0.0.1.

use soundcraft_ml::catalog::{Manifest, ManifestModel};
use soundcraft_ml::download::{self, DownloadProgress};
use soundcraft_ml::models::{self, ModelStatus};
use soundcraft_ml::{Cancel, MlError};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What the server answers for a path: the body, and how many bytes of it to send before
/// hanging up (`None`: all of it).
type Routes = Arc<Mutex<HashMap<String, (Vec<u8>, Option<usize>)>>>;

/// A one-thread HTTP/1.0 server; returns its base URL (`http://127.0.0.1:<port>`).
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
                let found = routes.lock().unwrap().get(&path).cloned();
                match found {
                    Some((body, cut)) => {
                        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(&body[..cut.unwrap_or(body.len()).min(body.len())]);
                    }
                    None => {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-ml-download-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A fake `srs_small` model zip: card.json + model.onnx (+ `card_sha` overriding the card's sha256).
fn model_zip(onnx: &[u8], version: &str, card_sha: Option<&str>) -> Vec<u8> {
    let card = serde_json::json!({"id": "srs_small", "name": "Fake", "file": "model.onnx", "sha256": card_sha.map_or_else(|| sha(onnx), str::to_string),
        "license": "MIT (test)", "source": "https://example.invalid/fake", "exported_utc": version});
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("card.json", o).unwrap();
    w.write_all(card.to_string().as_bytes()).unwrap();
    w.start_file("model.onnx", o).unwrap();
    w.write_all(onnx).unwrap();
    w.finish().unwrap().into_inner()
}

fn entry(zip: &[u8], version: &str) -> ManifestModel {
    ManifestModel {
        id: "srs_small".into(),
        name: "Fake".into(),
        version: version.into(),
        url: "srs_small.zip".into(),
        size: zip.len() as u64,
        sha256: sha(zip),
        license: "MIT".into(),
        ..Default::default()
    }
}

fn manifest_json(models: &[ManifestModel]) -> Vec<u8> {
    serde_json::to_vec(&Manifest { format: 1, release: "models-v1".into(), models: models.to_vec() }).unwrap()
}

fn status(dir: &Path) -> ModelStatus {
    models::list(dir).into_iter().find(|m| m.model.id == "srs_small").unwrap().status
}

/// Nothing but the model folders: no staging, old, part or download folders left over.
fn no_leftovers(dir: &Path) {
    let left: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(left.iter().all(|n| !n.starts_with('.')), "{left:?}");
}

#[test]
fn a_model_downloads_from_the_manifest_and_installs_checked() {
    let routes: Routes = Arc::default();
    let base = serve(routes.clone());
    let zip = model_zip(b"fake onnx weights", "2026-10-22T05:18:26Z", None);
    let e = entry(&zip, "2026-10-22T05:18:26Z");
    routes.lock().unwrap().insert("/srs_small.zip".into(), (zip.clone(), None));
    routes.lock().unwrap().insert("/models.json".into(), (manifest_json(std::slice::from_ref(&e)), None));
    let dir = tmp("e2e");
    let url = format!("{base}/models.json");

    let m = download::fetch_manifest(&url).unwrap();
    assert_eq!(m.get("srs_small"), Some(&e));
    let p = DownloadProgress::new();
    download::download(&dir, m.get("srs_small").unwrap(), &url, &Cancel::new(), &p).unwrap();
    assert_eq!(p.get(), (zip.len() as u64, zip.len() as u64));
    let ModelStatus::Installed(card) = status(&dir) else { panic!("{:?}", status(&dir)) };
    assert_eq!(download::card_version(&card), "2026-10-22T05:18:26Z");
    assert!(models::load(&dir, "srs_small").is_ok());
    no_leftovers(&dir);

    // An update replaces the folder; Remove takes it away.
    let zip2 = model_zip(b"newer fake onnx weights", "2026-11-01T00:00:00Z", None);
    let e2 = entry(&zip2, "2026-11-01T00:00:00Z");
    assert!(soundcraft_ml::catalog::is_newer(&e2.version, &download::card_version(&card)));
    routes.lock().unwrap().insert("/srs_small.zip".into(), (zip2, None));
    download::download(&dir, &e2, &url, &Cancel::new(), &DownloadProgress::new()).unwrap();
    let ModelStatus::Installed(card) = status(&dir) else { panic!() };
    assert_eq!(download::card_version(&card), "2026-11-01T00:00:00Z");
    no_leftovers(&dir);
    download::remove(&dir, "srs_small").unwrap();
    assert_eq!(status(&dir), ModelStatus::Missing);
    no_leftovers(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wrong_sha256_is_rejected_and_cleaned_up() {
    let routes: Routes = Arc::default();
    let base = serve(routes.clone());
    let zip = model_zip(b"weights", "v1", None);
    let mut e = entry(&zip, "v1");
    e.sha256 = "0".repeat(64);
    routes.lock().unwrap().insert("/srs_small.zip".into(), (zip, None));
    let dir = tmp("sha");
    let err = download::download(&dir, &e, &format!("{base}/models.json"), &Cancel::new(), &DownloadProgress::new()).unwrap_err();
    assert!(err.to_string().contains("does not match"), "{err}");
    assert_eq!(status(&dir), ModelStatus::Missing);
    no_leftovers(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_interrupted_download_never_leaves_a_half_model_and_keeps_the_old_one() {
    let routes: Routes = Arc::default();
    let base = serve(routes.clone());
    let url = format!("{base}/models.json");
    let dir = tmp("cut");
    let zip1 = model_zip(b"old weights", "v1", None);
    routes.lock().unwrap().insert("/srs_small.zip".into(), (zip1.clone(), None));
    download::download(&dir, &entry(&zip1, "v1"), &url, &Cancel::new(), &DownloadProgress::new()).unwrap();

    // The server hangs up half way through the newer zip.
    let zip2 = model_zip(&vec![7u8; 200_000], "v2", None);
    let half = zip2.len() / 2;
    routes.lock().unwrap().insert("/srs_small.zip".into(), (zip2.clone(), Some(half)));
    let err = download::download(&dir, &entry(&zip2, "v2"), &url, &Cancel::new(), &DownloadProgress::new()).unwrap_err();
    assert!(matches!(err, MlError::Io(_)), "{err}");
    let ModelStatus::Installed(card) = status(&dir) else { panic!("{:?}", status(&dir)) };
    assert_eq!(download::card_version(&card), "v1", "the old model stays");
    no_leftovers(&dir);

    // A zip whose file does not match its own card: the old model stays too.
    let bad = model_zip(b"tampered", "v3", Some(&"1".repeat(64)));
    routes.lock().unwrap().insert("/srs_small.zip".into(), (bad.clone(), None));
    assert!(download::download(&dir, &entry(&bad, "v3"), &url, &Cancel::new(), &DownloadProgress::new()).is_err());
    let ModelStatus::Installed(card) = status(&dir) else { panic!() };
    assert_eq!(download::card_version(&card), "v1");
    no_leftovers(&dir);

    // Not a zip at all (its sha256 matches the manifest): refused, nothing installed.
    let _ = download::remove(&dir, "srs_small");
    let junk = b"this is not a zip file".to_vec();
    routes.lock().unwrap().insert("/srs_small.zip".into(), (junk.clone(), None));
    let err = download::download(&dir, &entry(&junk, "v4"), &url, &Cancel::new(), &DownloadProgress::new()).unwrap_err();
    assert!(err.to_string().contains("not a model zip"), "{err}");
    assert_eq!(status(&dir), ModelStatus::Missing);
    no_leftovers(&dir);

    // A swap that stopped half way (the old folder renamed aside, the new one not in place) is put back.
    std::fs::create_dir_all(dir.join(".srs_small.old")).unwrap();
    std::fs::write(dir.join(".srs_small.old").join("card.json"), r#"{"file": "m.onnx"}"#).unwrap();
    std::fs::write(dir.join(".srs_small.old").join("m.onnx"), b"x").unwrap();
    let cancel = Cancel::new();
    cancel.cancel();
    assert!(matches!(download::download(&dir, &entry(&zip1, "v1"), &url, &cancel, &DownloadProgress::new()), Err(MlError::Cancelled)));
    assert!(matches!(status(&dir), ModelStatus::Installed(_)));
    no_leftovers(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zips_cannot_write_outside_the_model_folder() {
    let dir = tmp("escape");
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("../evil.txt", o).unwrap();
    w.write_all(b"x").unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let z = dir.join("evil.zip");
    std::fs::write(&z, bytes).unwrap();
    assert!(download::install_zip(&dir, "srs_small", &z).is_err());
    assert!(!dir.join("evil.txt").exists() && !dir.parent().unwrap().join("evil.txt").exists());
    assert!(download::install_zip(&dir, "deepfilternet3", &z).is_err(), "built-in models are not downloaded");
    assert!(download::install_zip(&dir, "../x", &z).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_manifest_parses_and_refuses_bad_entries() {
    let text = r#"{"format": 1, "release": "models-v1", "models": [
        {"id": "htdemucs", "version": "2026-10-22T05:00:18Z", "url": "https://x/htdemucs.zip", "size": 10, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "license": "MIT"},
        {"id": "whisper_lyrics", "manual": true, "manual_note": "too big"}]}"#;
    let m = Manifest::parse(text).unwrap();
    assert!(m.get("htdemucs").unwrap().downloadable());
    assert!(!m.get("whisper_lyrics").unwrap().downloadable());
    assert!(Manifest::parse(r#"{"models": [{"id": "x", "url": "u", "size": 1}]}"#).is_err(), "no sha256");
    assert!(Manifest::parse(r#"{"models": [{"id": "../x", "manual": true}]}"#).is_err());
    assert!(Manifest::parse(r#"{"format": 99, "models": []}"#).is_err());
    assert!(Manifest::parse("not json").is_err());
    assert_eq!(soundcraft_ml::catalog::resolve_url("http://h/r/models.json", "a.zip"), "http://h/r/a.zip");
    assert_eq!(soundcraft_ml::catalog::resolve_url("http://h/r/models.json", "https://o/a.zip"), "https://o/a.zip");
    assert!(!soundcraft_ml::catalog::is_newer("v2", ""), "no installed version: no update");
}

/// The environment variables pick the manifest and the folder: run in a child process (setting
/// variables in this one would need `unsafe`).
#[test]
fn freedawm_models_url_and_dir_drive_a_download() {
    let routes: Routes = Arc::default();
    let base = serve(routes.clone());
    let zip = model_zip(b"env weights", "v1", None);
    routes.lock().unwrap().insert("/release/srs_small.zip".into(), (zip.clone(), None));
    routes.lock().unwrap().insert("/release/models.json".into(), (manifest_json(&[entry(&zip, "v1")]), None));
    let dir = tmp("env");
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "env_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env("FREEDAWM_MODELS_URL", format!("{base}/release/models.json"))
        .env("FREEDAWM_MODELS_DIR", &dir)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && text.contains("1 passed"), "{text}");
    assert!(matches!(status(&dir), ModelStatus::Installed(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "run by freedawm_models_url_and_dir_drive_a_download with the variables set"]
fn env_child() {
    let url = download::manifest_url();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let dir = models::models_dir();
    let m = download::fetch_manifest(&url).unwrap();
    download::download(&dir, m.get("srs_small").unwrap(), &url, &Cancel::new(), &DownloadProgress::new()).unwrap();
}
