//! Session file extensions after the FreeDAWm rename: new sessions, templates and bundles are
//! `.fdawm` / `.fdawmz`; `.scraft` / `.scraftz` files still open, and a session opened from one
//! saves back to the same file, so a project never splits in two silently.

use serde_json::json;
use soundcraft_engine::{Engine, io};
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-extensions-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Files directly in `dir`, by name.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn the_extensions_are_fdawm_and_scraft_still_counts() {
    assert_eq!(soundcraft_engine::model::SESSION_EXTENSION, "fdawm");
    assert_eq!(io::BUNDLE_EXTENSION, "fdawmz");
    for p in ["a.fdawm", "A.FDAWM", "old.scraft", "Old.SCRAFT"] {
        assert!(soundcraft_engine::model::is_session_path(p), "{p}");
    }
    for p in ["a.fdawmz", "a.wav", "fdawm", "a.scraft.tmp"] {
        assert!(!soundcraft_engine::model::is_session_path(p), "{p}");
    }
}

#[test]
fn a_new_save_writes_fdawm() {
    let dir = scratch("new");
    let mut e = soundcraft_engine::demo::demo_engine();
    // No extension given: `.fdawm` is added.
    e.execute("session.save_as", &json!({"path": s(&dir.join("Song").join("Song"))})).unwrap();
    let file = dir.join("Song").join("Song.fdawm");
    assert!(file.is_file(), "{:?}", names(&dir.join("Song")));
    assert_eq!(e.path.as_deref().map(PathBuf::from), Some(file.clone()));
    assert!(!names(&dir.join("Song")).iter().any(|n| n.ends_with(".tmp")), "the temporary file is renamed away");
    // It opens again.
    let mut r = Engine::default();
    io::open_session(&mut r, &s(&file)).unwrap();
    assert_eq!(r.session().tracks.len(), e.session().tracks.len());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_scraft_session_opens_and_save_keeps_scraft() {
    let dir = scratch("legacy");
    // A session saved before the rename, under its old extension.
    let mut old = soundcraft_engine::demo::demo_engine();
    let file = dir.join("Old Song").join("Old Song.scraft");
    old.execute("session.save_as", &json!({"path": s(&file)})).unwrap();
    assert!(file.is_file());

    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": s(&file)})).unwrap();
    assert_eq!(e.session().tracks.len(), old.session().tracks.len(), "a .scraft file opens");
    assert_eq!(e.path.as_deref().map(PathBuf::from), Some(file.clone()));

    e.execute("track.new", &json!({"count": 1, "name": "Added"})).unwrap();
    e.execute("session.save", &json!({})).unwrap();
    let folder = names(&dir.join("Old Song"));
    assert!(!folder.iter().any(|n| n.ends_with(".fdawm")), "Save wrote no .fdawm beside it: {folder:?}");
    let mut r = Engine::default();
    io::open_session(&mut r, &s(&file)).unwrap();
    assert!(r.session().tracks.iter().any(|t| t.name == "Added"), "Save wrote back to the .scraft");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn templates_are_saved_as_fdawm() {
    let dir = scratch("template");
    let mut e = soundcraft_engine::demo::demo_engine();
    let r = e.execute("session.save_template", &json!({"path": s(&dir.join("Band"))})).unwrap();
    assert_eq!(r["path"].as_str().map(PathBuf::from), Some(dir.join("Band.fdawm")));
    assert_eq!(names(&dir), ["Band.fdawm"]);
    // An older .scraft template still makes a new session.
    std::fs::copy(dir.join("Band.fdawm"), dir.join("Old.scraft")).unwrap();
    let mut n = Engine::default();
    n.execute("session.new", &json!({"name": "Friday", "template": s(&dir.join("Old.scraft"))})).unwrap();
    assert_eq!(n.session().tracks.len(), e.session().tracks.len());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bundles_are_fdawmz_and_a_scraftz_keeps_its_name() {
    let e = soundcraft_engine::demo::demo_engine();
    let (bytes, _) = io::session_bundle(e.session()).unwrap();
    let inside = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap().file_names().map(|n| n.unwrap().into_owned()).collect::<Vec<_>>();
    assert!(inside.iter().any(|n| !n.contains('/') && n.ends_with(".fdawm")), "{inside:?}");
    assert_eq!(io::bundle_extension_for("Song"), "fdawmz", "a new bundle");
    assert_eq!(io::bundle_extension_for("Song.fdawmz"), "fdawmz");
    assert_eq!(io::bundle_extension_for("Song.SCRAFTZ"), "scraftz", "an old bundle keeps its extension");

    let mut r = Engine::default();
    io::open_session_bytes(&mut r, "Song.fdawmz", &bytes).unwrap();
    assert_eq!(r.path.as_deref(), Some("Song.fdawmz"));

    // A bundle made before the rename holds a .scraft and is named .scraftz.
    let mut old = std::io::Cursor::new(Vec::new());
    {
        let text = e.session().to_json().unwrap();
        let mut w = zip::ZipWriter::new(&mut old);
        w.start_file("Song.scraft", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut w, text.as_bytes()).unwrap();
        w.finish().unwrap();
    }
    let mut r = Engine::default();
    io::open_session_bytes(&mut r, "Song.scraftz", &old.into_inner()).unwrap();
    assert_eq!(r.session().tracks.len(), e.session().tracks.len());
    assert_eq!(r.path.as_deref(), Some("Song.scraftz"), "saved back under the same name");
}
