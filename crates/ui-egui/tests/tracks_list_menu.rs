//! Right-click in the Tracks list (spec 2026-11-02 item 5), in Edit and in Mix: the same menu as
//! right-clicking the track itself, acting on that track, or on the selection when it holds it.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

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

fn select(app: &mut SoundApp, names: &[&str]) {
    app.run("edit.select", json!({"tracks": names, "exact": true})).unwrap();
}

/// The Tracks list's row of `name` (left of the strips/tracks).
fn list_row<'a>(h: &'a Harness<'static, SoundApp>, name: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_label(name).min_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x)).unwrap()
}

#[test]
fn right_click_in_the_tracks_list_shows_the_tracks_menu_in_edit_and_mix() {
    for window in ["window.edit", "window.mix"] {
        let mut app = demo();
        app.run(window, json!({})).unwrap();
        select(&mut app, &["Kick", "Snare"]);
        let mut h = harness(app);
        // Not in the selection: acts on that track alone.
        list_row(&h, "Bass").click_secondary();
        h.run_steps(3);
        for item in ["Rename...", "Duplicate...", "Make Inactive", "Hide", "Delete"] {
            assert!(h.query_by_label(item).is_some(), "{window}: no {item}");
        }
        // The strip's width items in Mix only (beside the bar's own "Channel width").
        let widths = h.query_all_by_label("Channel width").count();
        assert_eq!(widths, if window == "window.mix" { 2 } else { 0 }, "{window}");
        h.get_by_label("Hide").click();
        h.run_steps(3);
        let hidden = |h: &Harness<SoundApp>, n: &str| h.state().engine.session().track_by_name(n).unwrap().hidden;
        assert!(hidden(&h, "Bass"));
        assert!(!hidden(&h, "Kick") && !hidden(&h, "Snare"));
        // In the selection: acts on the selection.
        list_row(&h, "Kick").click_secondary();
        h.run_steps(3);
        h.get_by_label("Hide").click();
        h.run_steps(3);
        assert!(hidden(&h, "Kick") && hidden(&h, "Snare"), "{window}");
    }
}
