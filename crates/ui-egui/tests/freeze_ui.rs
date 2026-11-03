//! A frozen track says so on its Mix strip (a "Frozen" badge over its greyed module chain) and its
//! Edit header (a snowflake on the name plate); unfreezing takes both away. The solo scope the
//! strips and headers share is worked out once per session revision.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};
use std::sync::Arc;

fn harness(window: &str) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run(window, json!({})).unwrap();
    app.run("mix.insert", json!({"track": "Bass", "plugin": "compressor"})).unwrap();
    app.run("track.freeze", json!({"tracks": ["Bass"]})).unwrap();
    assert!(app.ui.status.starts_with("Froze Bass"), "{}", app.ui.status);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        h.step();
    }
    h
}

fn thaw(h: &mut Harness<'_, SoundApp>) {
    h.state_mut().run("track.unfreeze", json!({"tracks": ["Bass"]})).unwrap();
    assert_eq!(h.state().ui.status, "Unfroze Bass");
    for _ in 0..4 {
        h.step();
    }
}

#[test]
fn the_mix_strip_shows_a_frozen_badge() {
    let mut h = harness("window.mix");
    assert!(h.query_by_label("Bass frozen").is_some(), "the Frozen badge");
    assert!(h.query_by_label("Pad frozen").is_none(), "only on the frozen track");
    thaw(&mut h);
    assert!(h.query_by_label("Bass frozen").is_none(), "gone once unfrozen");
}

#[test]
fn the_edit_header_shows_a_snowflake() {
    let mut h = harness("window.edit");
    assert!(h.query_by_label("Bass frozen").is_some(), "the snowflake");
    thaw(&mut h);
    assert!(h.query_by_label("Bass frozen").is_none());
}

#[test]
fn the_solo_scope_is_made_once_per_revision() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let a = app.solo_scope();
    assert!(Arc::ptr_eq(&a, &app.solo_scope()), "the same scope while the session is unchanged");
    app.run("mix.solo", json!({"track": "Bass"})).unwrap();
    let b = app.solo_scope();
    assert!(!Arc::ptr_eq(&a, &b) && b.any_solo(), "a new one after the solo");
}

/// Record-enable keys that are greyed out (disabled).
fn disabled_rec_keys(h: &Harness<'_, SoundApp>) -> usize {
    h.get_all_by_label("Record enable").filter(|n| n.accesskit_node().is_disabled()).count()
}

#[test]
fn a_frozen_tracks_record_key_is_disabled() {
    for window in ["window.mix", "window.edit"] {
        let mut h = harness(window);
        assert_eq!(disabled_rec_keys(&h), 1, "{window}: only the frozen Bass");
        thaw(&mut h);
        assert_eq!(disabled_rec_keys(&h), 0, "{window}: live again");
    }
}
