//! File › Import › Pro Tools Session…: the menu item next to AAF, found by Search, asks for the
//! file, and the import's summary shows in the status line with the full report in the Notes tab.

use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp, menus};

const ITEM: &str = "File > Import > Pro Tools Session...";

#[test]
fn the_import_menu_offers_pro_tools_sessions_beside_aaf() {
    let extra = menus::ui_aliases();
    fn walk(nodes: &[menus::MenuNode], out: &mut Vec<String>) {
        for n in nodes {
            out.push(n.path.clone());
            walk(&n.children, out);
        }
    }
    let mut paths = Vec::new();
    walk(&menus::tree(), &mut paths);
    let at = paths.iter().position(|p| p == ITEM).unwrap_or_else(|| panic!("{ITEM} not in the menus"));
    assert_eq!(paths.get(at - 1).map(String::as_str), Some("File > Import > AAF..."), "it sits after AAF");
    assert_eq!(menus::command_for_path(ITEM, &extra).as_deref(), Some("file.import_ptx"));
    // Search finds it by "pro tools" and by "ptx"-free words of its label.
    let hits = menus::search_hits("pro tools session", &extra);
    assert!(hits.iter().any(|h| h.2 == "file.import_ptx"), "{hits:?}");
}

#[test]
fn choosing_it_asks_for_the_file_then_imports_and_reports() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    // The menu item opens the path prompt for its file.
    assert!(app.dialogs.open_for_command(&app.engine, "file.import_ptx"));
    match &app.dialogs.open {
        Some(Dialog::PathPrompt { cmd, key, .. }) => assert_eq!((cmd.as_str(), key.as_str()), ("file.import_ptx", "path")),
        _ => panic!("no path prompt"),
    }
    app.dialogs.open = None;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ptx/tests/fixtures/CR_min.ptx");
    let v = app.run("file.import_ptx", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(app.ui.status.starts_with("Pro Tools CR_min.ptx: 4 tracks, 3 clips, 2 markers"), "{}", app.ui.status);
    assert_eq!(v["clips"], 3);
    let s = app.engine.session();
    assert!(s.tracks.iter().any(|t| t.name == "Au_Lead"));
    assert!(s.notes.iter().any(|n| n.text.contains("Not carried: \"Au_Lead\": the automation of EQ3 7-Band")));
    // A file that is not a session says so in the status line and changes nothing.
    let n = app.engine.session().tracks.len();
    assert!(app.run("file.import_ptx", json!({"path": env!("CARGO_MANIFEST_DIR").to_string() + "/Cargo.toml"})).is_err());
    assert!(!app.ui.status.is_empty());
    assert_eq!(app.engine.session().tracks.len(), n);
}
