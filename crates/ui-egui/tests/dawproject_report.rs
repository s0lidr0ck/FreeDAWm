//! File › Import › DAWproject… opens the import report window; the menus offer import and export.

use serde_json::json;
use soundcraft_engine::dawproject::archive::ArchiveWriter;
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn song(dir: &std::path::Path) -> std::path::PathBuf {
    let xml = std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../engine/tests/fixtures/dawproject/bitwig-complex.xml"),
    )
    .unwrap();
    let mut w = ArchiveWriter::new(std::io::Cursor::new(Vec::new()));
    w.add("project.xml", xml.as_bytes()).unwrap();
    let path = dir.join("Song.dawproject");
    std::fs::write(&path, w.finish().unwrap().into_inner()).unwrap();
    path
}

#[test]
fn an_import_shows_its_report_and_asks_to_save_on_close() {
    let dir = std::env::temp_dir().join(format!("sc-ui-dawproject-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = song(&dir);
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let v = app.run_sync("file.import_dawproject", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(v["tracks"], 4);
    let r = app.dawproject_report.as_ref().expect("the report window opens");
    assert_eq!(r.file, "Song.dawproject");
    assert!(app.ui.status.starts_with("Imported Song.dawproject"), "{}", app.ui.status);
    assert!(app.engine.is_dirty(), "a new unsaved session");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_menus_offer_dawproject_import_and_export() {
    let extra = menus::ui_aliases();
    let mut found = Vec::new();
    fn walk(nodes: &[menus::MenuNode], out: &mut Vec<String>) {
        for n in nodes {
            out.push(n.path.clone());
            walk(&n.children, out);
        }
    }
    walk(&menus::tree(), &mut found);
    for path in ["File > Import > DAWproject...", "File > Export > DAWproject..."] {
        let p = found.iter().find(|p| p.as_str() == path).unwrap_or_else(|| panic!("{path} not in the menus"));
        assert!(menus::command_for_path(p, &extra).is_some(), "{path} runs nothing");
    }
}
