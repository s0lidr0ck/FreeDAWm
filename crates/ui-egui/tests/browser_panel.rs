//! The plugin browser panel: docking, tabs, search, chips, the card grid and its menu.
mod editor_support;

use craft_ui::docking::Node;
use craft_ui::layout::SplitSize;
use editor_support::{app_with_tracks, right_click, type_text};
use egui::accesskit::Role;
use egui::{Rect, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::browser::BrowserTab;
use soundcraft_ui_egui::browser::catalog::{BrowserCategory, BrowserItem, Catalog, Format, Kind, ScanFailure, build_catalog};
use soundcraft_ui_egui::docking::Panel;
use soundcraft_ui_egui::{SoundApp, UiState};

/// An Edit window harness with the browser shown and the built-in catalog (no host scan).
fn browser(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut app = app;
    if app.browser_catalog.is_none() {
        app.browser_catalog = Some(build_catalog(soundcraft_dsp::plugins(), &[], &[], &[], Vec::new()));
    }
    app.run("ui.set", json!({"theme": "dark"})).unwrap();
    if !app.ui.show_plugin_browser {
        app.run("window.plugin_browser", json!({})).unwrap();
    }
    let mut h = Harness::builder().with_size(vec2(1700.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn has(h: &Harness<SoundApp>, role: Role, label: &str) -> bool {
    h.query_by_role_and_label(role, label).is_some()
}

fn card(h: &Harness<SoundApp>, name: &str) -> Rect {
    h.get_by_role_and_label(Role::Button, name).rect()
}

fn set_text(h: &mut Harness<SoundApp>, text: &str) {
    h.state_mut().ui.browser.text = text.into();
    h.run_steps(3);
}

#[test]
fn toggle_shows_and_hides_panel() {
    let mut h = browser(app_with_tracks(&["Voice"]));
    assert!(h.state().ui.show_plugin_browser);
    assert!(h.state().ui.docking.layout.contains(&Panel::Browser));
    assert!(has(&h, Role::Tab, "Plugin Browser"));
    // The menu item shows as checked while the panel is open.
    let path = "Window > Plugin Browser";
    assert!(soundcraft_ui_egui::menus::tree().iter().any(|n| n.label == "Window" && n.children.iter().any(|c| c.path == path)));
    assert!(soundcraft_ui_egui::menus::item_state(h.state(), path, Some("window.plugin_browser")).1);
    h.state_mut().run("window.plugin_browser", json!({})).unwrap();
    h.run_steps(4);
    assert!(!h.state().ui.show_plugin_browser);
    assert!(!h.state().ui.docking.layout.contains(&Panel::Browser));
    assert!(!has(&h, Role::Tab, "Plugin Browser"));
    // The lower dock strip's PLUGIN BROWSER tab opens it again.
    h.get_by_role_and_label(Role::Button, "Plugin Browser (Alt+B)").click();
    h.run_steps(4);
    assert!(h.state().ui.show_plugin_browser);
    // And the Mix window docks it as well.
    h.state_mut().run("window.mix", json!({})).unwrap();
    h.run_steps(5);
    assert!(h.state().ui.mix_docking.layout.contains(&Panel::Browser));
    assert!(has(&h, Role::Tab, "Plugin Browser"));
}

#[test]
fn tabs_switch_kind() {
    let mut h = browser(app_with_tracks(&["Voice"]));
    assert_eq!(h.state().ui.browser.tab, BrowserTab::Instruments);
    assert!(has(&h, Role::Button, "Subtractive Synth"));
    assert!(!has(&h, Role::Button, "CleanQ"));
    h.get_by_role_and_label(Role::RadioButton, "Effects").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.browser.tab, BrowserTab::Effects);
    set_text(&mut h, "Clean");
    assert!(has(&h, Role::Button, "CleanQ"));
    assert!(!has(&h, Role::Button, "Subtractive Synth"));
}

#[test]
fn search_filters_grid() {
    let mut app = app_with_tracks(&["Voice"]);
    app.ui.browser.tab = BrowserTab::Effects;
    let mut h = browser(app);
    // Typing with the pointer over the panel goes to its search field.
    let panel = h.state().browser_rect.unwrap().1;
    editor_support::hover(&mut h, panel.center());
    type_text(&mut h, "chamb");
    h.run_steps(2);
    assert_eq!(h.state().ui.browser.text, "chamb");
    assert!(has(&h, Role::Button, "ChamberMade"));
    assert!(!has(&h, Role::Button, "CleanQ"));
}

#[test]
fn favourite_chip_and_star() {
    let mut app = app_with_tracks(&["Voice"]);
    app.ui.browser.tab = BrowserTab::Effects;
    // A favourite the catalog no longer knows is skipped, not an error.
    app.ui.browser.favourites = vec!["vst3:gone".into()];
    let mut h = browser(app);
    h.state_mut().take_recent_changed();
    set_text(&mut h, "Clean");
    h.get_by_role_and_label(Role::CheckBox, "Favourite CleanQ").click();
    h.run_steps(3);
    assert!(h.state().ui.browser.favourites.iter().any(|f| f == "fs_cleanq"));
    assert!(h.state_mut().take_recent_changed(), "saved straight away, as Recent is");
    set_text(&mut h, "");
    h.get_by_role_and_label(Role::Button, "★ Favourites").click();
    h.run_steps(3);
    assert!(h.state().ui.browser.favourites_only);
    assert!(has(&h, Role::Button, "CleanQ"));
    assert!(!has(&h, Role::Button, "Vint"));
    assert!(!has(&h, Role::Button, "ChamberMade"));
    // Unstarring takes it out of the favourites again.
    h.get_by_role_and_label(Role::CheckBox, "Favourite CleanQ").click();
    h.run_steps(3);
    assert!(!h.state().ui.browser.favourites.iter().any(|f| f == "fs_cleanq"));
    assert!(!has(&h, Role::Button, "CleanQ"));
}

#[test]
fn category_override_from_card_menu() {
    let mut app = app_with_tracks(&["Voice"]);
    app.ui.browser.tab = BrowserTab::Effects;
    let mut h = browser(app);
    h.state_mut().take_recent_changed();
    set_text(&mut h, "Clean");
    let at = card(&h, "CleanQ").center();
    right_click(&mut h, at);
    h.get_by_label_contains("Category").hover();
    h.run_steps(3);
    h.get_by_role_and_label(Role::RadioButton, "Utility").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.browser.category_overrides.get("fs_cleanq"), Some(&BrowserCategory::Utility));
    assert!(h.state_mut().take_recent_changed(), "saved straight away");
    // The Utility chip now finds it, and the EQ chip does not.
    set_text(&mut h, "");
    h.get_by_role_and_label(Role::Button, "Utility").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.browser.category, Some(BrowserCategory::Utility));
    assert!(has(&h, Role::Button, "CleanQ"));
    h.get_by_role_and_label(Role::Button, "EQ").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.browser.category, Some(BrowserCategory::Eq));
    assert!(!has(&h, Role::Button, "CleanQ"));
    assert!(has(&h, Role::Button, "Vint"));
}

#[test]
fn failures_note_lists_reasons() {
    let mut app = app_with_tracks(&["Voice"]);
    let failures = vec![ScanFailure { path: "C:/VST3/Broken.vst3".into(), reason: "no GetPluginFactory export".into() }];
    app.browser_catalog = Some(build_catalog(soundcraft_dsp::plugins(), &[], &[], &[], failures));
    let mut h = browser(app);
    h.get_by_label("1 plugin failed to load").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("no GetPluginFactory export").is_some());
    assert!(h.query_by_label_contains("C:/VST3/Broken.vst3").is_some());
    // No failures, no note.
    h.state_mut().browser_catalog = Some(build_catalog(soundcraft_dsp::plugins(), &[], &[], &[], Vec::new()));
    h.run_steps(3);
    assert!(h.query_by_label_contains("failed to load").is_none());
}

#[test]
fn grid_virtualises_600_items() {
    let mut app = app_with_tracks(&["Voice"]);
    let items = (0..600)
        .map(|i| BrowserItem {
            id: format!("vst3:{i:04}"),
            name: format!("Plugin {i:04}"),
            vendor: format!("Vendor {}", i % 17),
            version: "1.0".into(),
            format: Format::Vst3,
            kind: Kind::Effect,
            category: BrowserCategory::ALL[i % BrowserCategory::ALL.len()],
        })
        .collect();
    app.browser_catalog = Some(Catalog { items, failures: Vec::new() });
    app.ui.browser.tab = BrowserTab::Effects;
    let mut h = browser(app);
    h.run_steps(2);
    let built = h.state().browser_debug.cards_built;
    assert!(built > 0 && built < 60, "built {built} cards for a frame");
    assert!(h.state().browser_debug.panel_ms < 50.0, "panel took {} ms", h.state().browser_debug.panel_ms);
    assert!(has(&h, Role::Button, "Plugin 0000"));
    assert!(!has(&h, Role::Button, "Plugin 0599"));
}

/// The path (false = first, true = second) to the split whose second side is the browser.
fn browser_split(node: &Node<Panel>, path: &mut Vec<bool>) -> Option<SplitSize> {
    let Node::Split { first, second, size, .. } = node else { return None };
    if matches!(&**second, Node::Tabs { panels, .. } if panels.contains(&Panel::Browser)) {
        return Some(*size);
    }
    for (side, child) in [(false, first), (true, second)] {
        path.push(side);
        if let Some(size) = browser_split(child, path) {
            return Some(size);
        }
        path.pop();
    }
    None
}

#[test]
fn panel_state_persists() {
    let mut h = browser(app_with_tracks(&["Voice"]));
    let mut path = Vec::new();
    let size = browser_split(h.state().ui.docking.layout.root.as_ref().unwrap(), &mut path).unwrap();
    assert_eq!(size, SplitSize::FixedSecond(320.0), "opens 320 px wide");
    h.state_mut().run("ui.dock", json!({"operation": "resizeSplit", "path": path, "size": {"FixedSecond": 400.0}})).unwrap();
    h.run_steps(3);
    h.get_by_role_and_label(Role::RadioButton, "Effects").click();
    h.run_steps(2);
    // Every built-in matches "a" (its vendor), so the grid has rows to scroll.
    set_text(&mut h, "a");
    assert!(has(&h, Role::Button, "Amp"), "the first effect is in view");
    let grid = h.state().browser_rect.unwrap().1.center();
    editor_support::wheel_notches(&mut h, grid, -6.0);
    h.run_steps(3);
    let scroll = h.state().ui.browser.scroll[BrowserTab::Effects.index()];
    assert!(scroll > 200.0, "scrolled {scroll}");
    let saved = serde_json::to_string(&h.state().ui).unwrap();
    let restored: UiState = serde_json::from_str(&saved).unwrap();
    assert!(restored.show_plugin_browser);
    assert_eq!(restored.browser.tab, BrowserTab::Effects);
    assert_eq!(restored.browser.text, "a");
    assert_eq!(restored.browser.scroll[BrowserTab::Effects.index()], scroll);
    let mut again = Vec::new();
    assert_eq!(browser_split(restored.docking.layout.root.as_ref().unwrap(), &mut again), Some(SplitSize::FixedSecond(400.0)));
    // A reloaded app opens the grid where it was.
    let mut app = app_with_tracks(&["Voice"]);
    app.ui = restored;
    let h = browser(app);
    assert!(!has(&h, Role::Button, "Amp"), "the grid opened scrolled");
    let now = h.state().ui.browser.scroll[BrowserTab::Effects.index()];
    assert!((now - scroll).abs() < 1.0, "reopened at {now}, saved {scroll}");
}

#[test]
fn one_browser_while_mix_is_a_separate_window() {
    let mut app = app_with_tracks(&["Voice"]);
    app.ui.mix_separate = true;
    let mut h = browser(app);
    h.run_steps(4);
    assert!(h.state().ui.show_plugin_browser);
    assert!(h.state().ui.docking.layout.contains(&Panel::Browser), "it lives in Edit");
    assert!(!h.state().ui.mix_docking.layout.contains(&Panel::Browser), "not also in the separate Mix window");
    assert_eq!(h.state().browser_rect.map(|(v, _)| v), Some(egui::ViewportId::ROOT));
    // Mix back in the main window hosts it there.
    h.state_mut().ui.mix_separate = false;
    h.state_mut().run("window.mix", json!({})).unwrap();
    h.run_steps(5);
    assert!(h.state().ui.mix_docking.layout.contains(&Panel::Browser));
    assert!(h.state().ui.show_plugin_browser);
}

#[test]
fn scan_progress_and_cancel() {
    use soundcraft_ui_egui::browser::scan_job::ScanJob;
    let mut app = app_with_tracks(&["Voice"]);
    // A fake scan: 312 of 767 bundles done, then waiting for Cancel.
    let job = ScanJob::start_with(|p| {
        p.add_total(767);
        for _ in 0..312 {
            p.step();
        }
        while !p.cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err("Plugin scan cancelled".into())
    });
    let t0 = std::time::Instant::now();
    while job.progress() != (312, 767) {
        assert!(t0.elapsed().as_secs() < 10, "the fake scan started");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.plugin_scan = Some(job);
    let mut h = browser(app);
    assert!(h.query_by_label_contains("Scanning 312 / 767").is_some(), "progress is shown");
    assert!(h.query_by_label("Scan plugins").is_none(), "no second scan while one runs");
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    let t0 = std::time::Instant::now();
    while h.state().plugin_scan.is_some() {
        assert!(t0.elapsed().as_secs() < 10, "the scan ends after Cancel");
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.query_by_label_contains("Scanning").is_none());
    assert_eq!(h.state().ui.status, "Plugin scan cancelled");
}
