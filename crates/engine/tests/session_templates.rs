//! `session.new {template: <path>}` (docs/superpowers/specs/2026-10-25-presets-markers-templates.md,
//! part 3): a template saved with Save As Template opens as a new untitled session, and the
//! template file is never written over.

use serde_json::json;
use soundcraft_engine::Engine;
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sc-templates-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_template_opens_as_a_new_untitled_session() {
    let dir = scratch("open");
    let mut e = soundcraft_engine::demo::demo_engine();
    e.execute("session.new", &json!({"name": "Band", "sample_rate": 96_000})).unwrap();
    e.execute("track.new", &json!({"count": 3, "name": "Gtr"})).unwrap();
    e.execute("markers.add", &json!({"name": "Intro", "at": 0})).unwrap();
    let file = dir.join("Band tracking.scraft");
    let r = e.execute("session.save_template", &json!({"path": file.to_string_lossy()})).unwrap();
    assert_eq!(r["path"].as_str().map(PathBuf::from), Some(file.clone()));
    let saved = std::fs::read(&file).unwrap();

    let mut e = Engine::default();
    let r = e.execute("session.new", &json!({"name": "Friday", "template": file.to_string_lossy(), "sample_rate": 44_100})).unwrap();
    assert_eq!(r["tracks"], 3);
    assert_eq!(r["sample_rate"], 96_000, "the template's own rate");
    let s = e.session();
    assert_eq!(s.name, "Friday");
    assert_eq!(s.tracks.len(), 3);
    assert_eq!(s.markers.len(), 1);
    assert!(s.folder.is_none(), "no folder yet: nothing is written next to the template");
    assert!(e.path.is_none(), "untitled");
    assert!(!e.is_dirty());
    // Save has no path to write to, so the template can't be replaced by mistake.
    assert!(e.execute("session.save", &json!({})).is_err());
    e.execute("track.new", &json!({})).unwrap();
    assert!(e.execute("session.save", &json!({})).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), saved, "the template file is untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn blank_demo_and_bad_templates() {
    let dir = scratch("bad");
    let mut e = Engine::default();
    let r = e.execute("session.new", &json!({"template": "blank", "sample_rate": 44_100})).unwrap();
    assert_eq!(r["sample_rate"], 44_100);
    assert!(e.session().tracks.is_empty());
    e.execute("session.new", &json!({"template": "demo"})).unwrap();
    assert!(!e.session().tracks.is_empty());
    let before = e.session().tracks.len();
    let junk = dir.join("junk.scraft");
    std::fs::write(&junk, "not a session").unwrap();
    for t in [dir.join("missing.scraft"), junk, dir.clone()] {
        assert!(e.execute("session.new", &json!({"template": t.to_string_lossy()})).is_err(), "{}", t.display());
        assert_eq!(e.session().tracks.len(), before, "a refused template leaves the open session");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
