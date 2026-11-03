//! Channel width for many channels at once (spec 2026-11-02 item 4): the bar at the top of the
//! Mix view, the channel menu and View › Channel Width, each one `mix.strip_size` (one undo step).

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::StripSize;
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn demo() -> SoundApp {
    SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
}

fn size(app: &SoundApp, name: &str) -> StripSize {
    app.engine.session().track_by_name(name).unwrap().mixer.strip_size
}

fn select(app: &mut SoundApp, names: &[&str]) {
    app.run("edit.select", json!({"tracks": names, "exact": true})).unwrap();
}

#[test]
fn the_commands_size_the_selection_or_all_in_one_undo_step() {
    let mut app = demo();
    assert!(app.run("ui.mix.width.slim", json!({})).is_err() || !app.engine.session().edit.selected_tracks.is_empty());
    select(&mut app, &["Kick", "Bass"]);
    let before = app.engine.undo_history().len();
    app.run("ui.mix.width.slim", json!({})).unwrap();
    assert_eq!((size(&app, "Kick"), size(&app, "Bass"), size(&app, "Keys")), (StripSize::Slim, StripSize::Slim, StripSize::Auto));
    assert_eq!(app.engine.undo_history().len(), before + 1);
    app.run("ui.mix.width.wide_all", json!({})).unwrap();
    assert!(app.engine.session().tracks.iter().all(|t| t.mixer.strip_size == StripSize::Wide));
    assert_eq!(app.engine.undo_history().len(), before + 2);
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(size(&app, "Keys"), StripSize::Auto);
    assert_eq!(size(&app, "Kick"), StripSize::Slim);
    // `tracks` names them outright.
    app.run("ui.mix.width.wide", json!({"tracks": ["Keys"]})).unwrap();
    assert_eq!(size(&app, "Keys"), StripSize::Wide);
    // Nothing selected: refused (the All ones still work).
    app.run("edit.select", json!({"tracks": [], "exact": true})).unwrap();
    app.engine.session_mut().edit.selected_tracks.clear();
    assert!(app.run("ui.mix.width.auto", json!({})).is_err());
    app.run("ui.mix.width.auto_all", json!({})).unwrap();
    assert!(app.engine.session().tracks.iter().all(|t| t.mixer.strip_size == StripSize::Auto));
}

#[test]
fn view_channel_width_items_run_them() {
    let mut app = demo();
    fn walk(n: &[menus::MenuNode], out: &mut Vec<(String, menus::MenuKind)>, parent: &str) {
        for x in n {
            let p = if parent.is_empty() { x.label.clone() } else { format!("{parent} > {}", x.label) };
            out.push((p.clone(), x.kind.clone()));
            walk(&x.children, out, &p);
        }
    }
    let mut shown = Vec::new();
    walk(&menus::tree(), &mut shown, "");
    for path in ["Slim", "Wide", "Auto", "All Slim", "All Wide", "All Auto"].map(|l| format!("View > Channel Width > {l}")) {
        assert!(shown.iter().any(|(p, _)| *p == path), "no {path}");
    }
    select(&mut app, &["Kick"]);
    let (on, _) = menus::item_state(&app, "View > Channel Width > Slim", Some("ui.mix.width.slim"));
    assert!(on);
    menus::invoke_menu(&mut app, "ui.mix.width.slim", "View > Channel Width > Slim");
    assert_eq!(size(&app, "Kick"), StripSize::Slim);
    menus::invoke_menu(&mut app, "ui.mix.width.wide_all", "View > Channel Width > All Wide");
    assert!(app.engine.session().tracks.iter().all(|t| t.mixer.strip_size == StripSize::Wide));
    app.engine.session_mut().edit.selected_tracks.clear();
    let (on, _) = menus::item_state(&app, "View > Channel Width > Slim", Some("ui.mix.width.slim"));
    assert!(!on, "greyed without a selection");
}

#[test]
fn the_bar_at_the_top_of_the_mix_view_sizes_the_selection_or_all() {
    let mut app = demo();
    app.run("window.mix", json!({})).unwrap();
    select(&mut app, &["Kick", "Bass"]);
    let mut h = harness(app);
    assert!(h.query_by_label("Selected (2)").is_some());
    h.get_by_label("Slim").click();
    h.run_steps(3);
    assert_eq!((size(h.state(), "Kick"), size(h.state(), "Bass"), size(h.state(), "Keys")), (StripSize::Slim, StripSize::Slim, StripSize::Auto));
    h.get_by_label("All").click();
    h.run_steps(3);
    assert!(h.state().ui.mix_width_all);
    h.get_by_label("Wide").click();
    h.run_steps(3);
    assert!(h.state().engine.session().tracks.iter().all(|t| t.mixer.strip_size == StripSize::Wide));
}

#[test]
fn the_strip_menu_sizes_the_selection_holding_it() {
    let mut app = demo();
    app.run("window.mix", json!({})).unwrap();
    select(&mut app, &["Kick", "Bass"]);
    let mut h = harness(app);
    h.get_by_label("Bass strip").click_secondary();
    h.run_steps(3);
    assert!(h.query_by_label("Channel width (2 channels)").is_some());
    // The menu's Wide (the bar's is a plain button too: the menu's is the later one).
    let wides = h.get_all_by_label("Wide").collect::<Vec<_>>();
    wides.last().unwrap().click();
    h.run_steps(3);
    assert_eq!((size(h.state(), "Kick"), size(h.state(), "Bass"), size(h.state(), "Keys")), (StripSize::Wide, StripSize::Wide, StripSize::Auto));
}
