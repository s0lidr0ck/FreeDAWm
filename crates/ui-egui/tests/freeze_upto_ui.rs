//! Freeze up to here in the Mix window: a module tile's menu freezes the chain down to it (or the
//! whole chain) and unfreezes; the frozen modules are greyed and read-only, the ones after the
//! freeze point stay live and editable.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_model::ModuleId;
use soundcraft_ui_egui::{Services, SoundApp};

/// The Mix window, with a Gain and a Flanger added to the Bass (no other track has either).
fn harness() -> (Harness<'static, SoundApp>, u32, u32) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    let gain = app.run("mix.module_add", json!({"track": "Bass", "plugin": "gain"})).unwrap()["module"].as_u64().unwrap() as u32;
    let flanger = app.run("mix.module_add", json!({"track": "Bass", "plugin": "flanger"})).unwrap()["module"].as_u64().unwrap() as u32;
    let mut h = Harness::builder().with_size(vec2(1800.0, 1100.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h);
    (h, gain, flanger)
}

fn steps(h: &mut Harness<'_, SoundApp>) {
    for _ in 0..6 {
        h.step();
    }
}

fn upto(h: &Harness<'_, SoundApp>) -> Option<Option<ModuleId>> {
    let s = h.state().engine.session();
    s.tracks.iter().find(|t| t.name == "Bass").unwrap().frozen_audio().map(|f| f.upto)
}

fn disabled(h: &Harness<'_, SoundApp>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

/// Opens the tile's menu and clicks `item`.
fn menu(h: &mut Harness<'_, SoundApp>, tile: &str, item: &str) {
    h.get_by_label(tile).click_secondary();
    steps(h);
    h.get_by_label(item).click();
    steps(h);
}

#[test]
fn a_tiles_menu_freezes_up_to_it_and_only_the_modules_above_are_greyed() {
    let (mut h, gain, _) = harness();
    menu(&mut h, "Gain module", "Freeze up to here");
    assert_eq!(upto(&h), Some(Some(ModuleId(gain))), "{}", h.state().ui.status);
    assert!(h.state().ui.status.starts_with("Froze Bass"), "{}", h.state().ui.status);
    assert!(h.query_by_label("Bass frozen").is_some(), "the Frozen badge");
    assert!(h.query_by_label("Bass freeze point").is_some(), "the line under the freeze point");
    assert!(disabled(&h, "Bypass Gain"), "a frozen module is read-only");
    assert!(!disabled(&h, "Bypass Flanger"), "a module after the point is live");
    // Live modules work: bypassing the flanger from its tile is allowed.
    h.get_by_label("Bypass Flanger").click();
    steps(&mut h);
    let s = h.state().engine.session();
    assert!(s.tracks.iter().find(|t| t.name == "Bass").unwrap().mixer.inserts.iter().any(|i| i.plugin == "flanger" && i.bypass));

    // The whole chain: everything greyed, no line.
    menu(&mut h, "Flanger module", "Freeze whole chain");
    assert_eq!(upto(&h), Some(None));
    assert!(disabled(&h, "Bypass Flanger"));
    assert!(h.query_by_label("Bass freeze point").is_none());

    menu(&mut h, "Gain module", "Unfreeze");
    assert_eq!(upto(&h), None);
    assert!(!disabled(&h, "Bypass Gain"));
    assert!(h.query_by_label("Bass frozen").is_none());
}

#[test]
fn the_drawer_header_menu_freezes_too() {
    let (mut h, _, flanger) = harness();
    h.get_by_label("Flanger module").click();
    steps(&mut h);
    menu(&mut h, "Bass Flanger drawer", "Freeze up to here");
    assert_eq!(upto(&h), Some(Some(ModuleId(flanger))));
}

/// The Mix window, with a Gain and a Flanger added to the Keys (an instrument track).
fn keys_harness() -> (Harness<'static, SoundApp>, u32) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    let gain = app.run("mix.module_add", json!({"track": "Keys", "plugin": "gain"})).unwrap()["module"].as_u64().unwrap() as u32;
    app.run("mix.module_add", json!({"track": "Keys", "plugin": "flanger"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1800.0, 1100.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h);
    (h, gain)
}

fn keys_upto(h: &Harness<'_, SoundApp>) -> Option<Option<ModuleId>> {
    let s = h.state().engine.session();
    s.tracks.iter().find(|t| t.name == "Keys").unwrap().frozen_audio().map(|f| f.upto)
}

#[test]
fn an_instrument_tracks_tile_and_drawer_menus_freeze_too() {
    let (mut h, gain) = keys_harness();
    menu(&mut h, "Gain module", "Freeze up to here");
    assert_eq!(keys_upto(&h), Some(Some(ModuleId(gain))), "{}", h.state().ui.status);
    assert!(h.query_by_label("Keys frozen").is_some(), "the Frozen badge");
    assert!(h.query_by_label("Keys freeze point").is_some());
    assert!(disabled(&h, "Bypass Gain") && !disabled(&h, "Bypass Flanger"));
    menu(&mut h, "Flanger module", "Freeze whole chain");
    assert_eq!(keys_upto(&h), Some(None));
    menu(&mut h, "Gain module", "Unfreeze");
    assert_eq!(keys_upto(&h), None);
    // The drawer header's menu.
    h.get_by_label("Flanger module").click();
    steps(&mut h);
    menu(&mut h, "Keys Flanger drawer", "Freeze up to here");
    assert!(keys_upto(&h).is_some_and(|u| u.is_some()), "{}", h.state().ui.status);
}
