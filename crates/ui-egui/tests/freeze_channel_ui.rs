//! Freeze from the channel: the track menu (the Mix strip's name, the Edit track name) has a Freeze
//! submenu (up to each module, the whole chain, Unfreeze, the current point checked), and the Mix
//! strip has a snowflake key that freezes the whole chain and unfreezes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_model::ModuleId;
use soundcraft_ui_egui::{Services, SoundApp};

/// The Mix window, with a Gain and a Flanger added to the Keys (an instrument track).
fn harness() -> (Harness<'static, SoundApp>, u32, u32) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    let gain = app.run("mix.module_add", json!({"track": "Keys", "plugin": "gain"})).unwrap()["module"].as_u64().unwrap() as u32;
    let flanger = app.run("mix.module_add", json!({"track": "Keys", "plugin": "flanger"})).unwrap()["module"].as_u64().unwrap() as u32;
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

fn upto(h: &Harness<'_, SoundApp>, track: &str) -> Option<Option<ModuleId>> {
    let s = h.state().engine.session();
    s.tracks.iter().find(|t| t.name == track).unwrap().frozen_audio().map(|f| f.upto)
}

/// Opens `strip`'s name menu and its Freeze submenu.
fn open_freeze(h: &mut Harness<'_, SoundApp>, strip: &str) {
    h.get_by_label(strip).click_secondary();
    steps(h);
    h.get_by_label("Freeze ⏵").hover();
    steps(h);
}

fn pick(h: &mut Harness<'_, SoundApp>, strip: &str, item: &str) {
    open_freeze(h, strip);
    h.get_by_label(item).click();
    steps(h);
}

#[test]
fn the_channel_menu_freezes_up_to_a_module_the_whole_chain_and_unfreezes() {
    let (mut h, gain, _) = harness();
    open_freeze(&mut h, "Keys strip");
    assert!(h.query_by_label("Up to Gain").is_some(), "one item per module, in chain order");
    assert!(h.query_by_label("Up to Flanger").is_some());
    assert!(h.query_by_label("Whole chain").is_some());
    assert!(h.query_by_label("Unfreeze").is_none(), "only on a frozen track");
    h.get_by_label("Up to Gain").click();
    steps(&mut h);
    assert_eq!(upto(&h, "Keys"), Some(Some(ModuleId(gain))), "{}", h.state().ui.status);

    // The current point is checked.
    open_freeze(&mut h, "Keys strip");
    assert!(h.query_by_label("✔ Up to Gain").is_some(), "the freeze point is checked");
    h.get_by_label("Whole chain").click();
    steps(&mut h);
    assert_eq!(upto(&h, "Keys"), Some(None));

    open_freeze(&mut h, "Keys strip");
    assert!(h.query_by_label("✔ Whole chain").is_some());
    h.get_by_label("Unfreeze").click();
    steps(&mut h);
    assert_eq!(upto(&h, "Keys"), None);
}

#[test]
fn the_channel_menu_freezes_an_audio_track_too() {
    let (mut h, _, _) = harness();
    pick(&mut h, "Bass strip", "Whole chain");
    assert_eq!(upto(&h, "Bass"), Some(None), "{}", h.state().ui.status);
}

#[test]
fn the_freeze_submenu_is_disabled_on_a_track_that_cannot_freeze() {
    let (mut h, _, _) = harness();
    h.get_by_label("Verb strip").click_secondary();
    steps(&mut h);
    assert!(h.get_by_label("Freeze ⏵").accesskit_node().is_disabled(), "an aux has no instrument");
}

#[test]
fn the_edit_track_name_menu_has_the_submenu_too() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("window.edit", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1800.0, 1100.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h);
    pick(&mut h, "Keys name", "Whole chain");
    assert_eq!(upto(&h, "Keys"), Some(None), "{}", h.state().ui.status);
}

fn freeze_key(h: &Harness<'_, SoundApp>, track: &str) -> bool {
    h.get_by_label(&format!("{track} freeze")).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

#[test]
fn the_strip_snowflake_freezes_the_whole_chain_and_unfreezes() {
    let (mut h, gain, _) = harness();
    assert!(!freeze_key(&h, "Keys"), "not lit while live");
    h.get_by_label("Keys freeze").click();
    steps(&mut h);
    assert_eq!(upto(&h, "Keys"), Some(None), "{}", h.state().ui.status);
    assert!(freeze_key(&h, "Keys"), "lit while frozen");
    h.get_by_label("Keys freeze").click();
    steps(&mut h);
    assert_eq!(upto(&h, "Keys"), None, "a second click unfreezes");
    // Partly frozen: lit too, and a click unfreezes.
    h.state_mut().run("track.freeze", json!({"tracks": ["Keys"], "upto": gain})).unwrap();
    steps(&mut h);
    assert!(freeze_key(&h, "Keys"));
    h.get_by_label("Keys freeze").click();
    steps(&mut h);
    assert_eq!(upto(&h, "Keys"), None);
    // Audio tracks have one; an aux (nothing to freeze) has none.
    assert!(h.query_by_label("Bass freeze").is_some());
    assert!(h.query_by_label("Verb freeze").is_none());
}

#[test]
fn strips_stay_lined_up_with_the_snowflake() {
    let (mut h, _, _) = harness();
    let top = |h: &Harness<'_, SoundApp>, label: &str| h.get_by_label(label).rect().min.y;
    // Audio, instrument and aux strips: the solo/mute row and the record key line up.
    let solo = top(&h, "Bass solo");
    for t in ["Keys", "Verb", "Pad"] {
        assert!((top(&h, &format!("{t} solo")) - solo).abs() < 0.5, "{t}: solo row");
    }
    let rec: Vec<f32> = h.get_all_by_label("Record enable").map(|n| n.rect().min.y).collect();
    assert!(rec.windows(2).all(|w| (w[0] - w[1]).abs() < 0.5), "record keys: {rec:?}");
    let flakes: Vec<f32> = ["Bass", "Keys", "Pad"].iter().map(|t| top(&h, &format!("{t} freeze"))).collect();
    assert!(flakes.iter().all(|y| (y - rec[0]).abs() < 0.5), "the snowflake sits in the record row: {flakes:?} vs {}", rec[0]);
    // Slim strips too.
    h.state_mut().run("view.mix_slim", json!({})).unwrap();
    steps(&mut h);
    let slim = top(&h, "Bass solo");
    for t in ["Keys", "Verb"] {
        assert!((top(&h, &format!("{t} solo")) - slim).abs() < 0.5, "{t}: slim solo row");
    }
    let r = h.get_by_label("Keys freeze").rect();
    assert!(r.width() >= 14.0, "the slim snowflake key is still a key: {r:?}");
}

/// "Whole chain" and "Unfreeze" act on the selection track by track: a track that can't take the
/// action is skipped and named in the status line; one frozen up to a module moves to the whole
/// chain.
#[test]
fn whole_chain_and_unfreeze_on_a_mixed_selection_act_per_track() {
    let (mut h, gain, _) = harness();
    h.state_mut().run("edit.select", json!({"tracks": ["Keys", "Verb"]})).unwrap();
    steps(&mut h);
    pick(&mut h, "Keys strip", "Whole chain");
    assert_eq!(upto(&h, "Keys"), Some(None), "{}", h.state().ui.status);
    assert_eq!(upto(&h, "Verb"), None, "an aux can't be frozen");
    let status = h.state().ui.status.clone();
    assert!(status.starts_with("Froze 1 track, skipped `Verb`: Freeze works on audio tracks"), "{status}");

    // Keys frozen whole, Bass frozen up to its first module, Pad live: Whole chain freezes Bass
    // and Pad whole and leaves Keys as it is.
    let bass_first = h.state().engine.session().track_by_name("Bass").unwrap().mixer.inserts[0].id.0;
    h.state_mut().run("track.freeze", json!({"tracks": ["Bass"], "upto": bass_first})).unwrap();
    h.state_mut().run("edit.select", json!({"tracks": ["Keys", "Bass", "Pad"]})).unwrap();
    steps(&mut h);
    pick(&mut h, "Bass strip", "Whole chain");
    assert_eq!(upto(&h, "Bass"), Some(None), "{}", h.state().ui.status);
    assert_eq!(upto(&h, "Pad"), Some(None));
    assert_eq!(upto(&h, "Keys"), Some(None));
    assert_eq!(h.state().ui.status, "Froze 2 tracks");

    // Unfreeze on Keys + Verb: Keys thaws, Verb (not frozen) is named.
    h.state_mut().run("edit.select", json!({"tracks": ["Keys", "Verb"]})).unwrap();
    steps(&mut h);
    pick(&mut h, "Keys strip", "Unfreeze");
    assert_eq!(upto(&h, "Keys"), None, "{}", h.state().ui.status);
    assert_eq!(h.state().ui.status, "Unfroze 1 track, skipped `Verb`: it is not frozen");
    let _ = gain;
}
