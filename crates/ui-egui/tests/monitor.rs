//! The monitor stage in the app (`monitor_ui`): the master fader's EAR and SIM keys and their
//! commands. They change what is heard on this computer and nothing else: not the session, not
//! its undo history, and never a bounce.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::user_files::UserDirs;
use soundcraft_ui_egui::{Services, SoundApp, menus};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-ui-monitor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app(library: &Path) -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.dialogs.dirs = UserDirs::under(library);
    app
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(2400.0, 900.0)).with_max_steps(1_000).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn save_me(app: &mut SoundApp) {
    app.run(
        "monitor.ear_profile.save",
        json!({"name": "Me", "headphones": "HD 600", "left": [[2000, 3], [8000, 20]], "right": [[4000, 6]], "select": true}),
    )
    .unwrap();
}

#[test]
fn the_keys_change_neither_the_session_nor_its_undo_history() {
    let dir = scratch("no-edit");
    let mut app = app(&dir);
    let (rev, undo) = (app.engine.revision, app.engine.can_undo());
    save_me(&mut app);
    let r = app.run("monitor.ear", json!({})).unwrap();
    assert_eq!(r["ear"], json!({"value": true, "profile": "Me"}));
    let r = app.run("monitor.sim", json!({"profile": "IPHONE23"})).unwrap();
    assert_eq!(r["sim"], json!({"value": true, "profile": "iphone23"}));
    assert!(app.ui.monitor_ear && app.ui.monitor_sim);
    assert_eq!((app.engine.revision, app.engine.can_undo()), (rev, undo));
    assert!(!app.engine.is_dirty());
    // Off again; a value alone keeps the profile.
    assert_eq!(app.run("monitor.sim", json!({"value": false})).unwrap()["sim"], json!({"value": false, "profile": "iphone23"}));
    assert!(app.run("monitor.sim", json!({"profile": "No Such Thing"})).is_err());
    assert!(app.run("monitor.ear", json!({"profile": "Nobody"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn profiles_live_in_the_library_folder_and_are_listed_with_the_built_in_outputs() {
    let dir = scratch("library");
    let mut app = app(&dir);
    save_me(&mut app);
    assert!(dir.join("Monitor Profiles").join("Hearing").join("Me.json").is_file());
    let r = app.run("monitor.profiles", json!({})).unwrap();
    assert_eq!(r["hearing"], json!([{"name": "Me", "headphones": "HD 600"}]));
    let names: Vec<&str> = r["devices"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Mono") && names.contains(&"StandardCarDriver"), "{names:?}");
    assert!(r["devices"].as_array().unwrap().iter().all(|d| d["builtin"] == json!(true)));
    assert!(app.run("monitor.ear_profile.save", json!({"name": "", "left": []})).is_err());
    assert!(app.run("monitor.ear_profile.save", json!({"name": "x", "left": [[1000]]})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ear_without_a_profile_opens_the_hearing_test_to_make_one() {
    let dir = scratch("none");
    let mut h = harness(app(&dir));
    assert!(h.state_mut().run("monitor.ear", json!({})).is_err());
    assert!(!h.state().ui.monitor_ear);
    h.run_steps(3);
    h.get_by_label("Set your listening level");
    h.get_by_label("Step 1 of 20");
    h.state_mut().run("window.hearing_test", json!({"value": false})).unwrap();
    h.run_steps(2);
    assert!(h.query_by_label("Set your listening level").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_hearing_test_steps_through_balance_then_loudness() {
    use soundcraft_ui_egui::hearing_ui::{Kind, STEPS, kind};
    assert_eq!(STEPS, 20);
    assert_eq!(kind(0), Kind::Level);
    assert_eq!((kind(1), kind(10)), (Kind::Balance(0), Kind::Balance(9)));
    // Loudness skips the 1 kHz band, which is the reference.
    let loud: Vec<Kind> = (11..20).map(kind).collect();
    assert_eq!(loud, [0, 1, 2, 4, 5, 6, 7, 8, 9].map(Kind::Loudness));
    assert_eq!(kind(99), Kind::Loudness(9));
}

#[test]
fn the_hearing_test_ends_in_an_ear_profile_and_never_edits_the_session() {
    let dir = scratch("test");
    let mut app = app(&dir);
    let rev = app.engine.revision;
    assert!(app.run("hearing_test.next", json!({})).is_err());
    let r = app.run("window.hearing_test", json!({"value": true, "name": "Me", "headphones": "HD 600"})).unwrap();
    assert_eq!(r, json!({"open": true, "step": 1, "steps": 20, "kind": "level", "hz": 1000.0, "value": 0.0}));
    assert!(app.run("hearing_test.set", json!({"value": 3.0})).is_err());
    // Balance: the right ear needs 6 dB more at 4 kHz (step 8) and nothing elsewhere.
    for step in 2..=11 {
        let r = app.run("hearing_test.next", json!({})).unwrap();
        assert_eq!((r["step"].as_u64(), r["kind"].as_str()), (Some(step), Some("balance")));
        if r["hz"] == json!(4000.0) {
            assert_eq!(app.run("hearing_test.set", json!({"value": 6.0})).unwrap()["value"], json!(6.0));
        }
    }
    assert_eq!(app.run("hearing_test.back", json!({})).unwrap()["step"], json!(10));
    app.run("hearing_test.next", json!({})).unwrap();
    // Loudness: 8 kHz needs 10 dB more; an answer out of range is held at the range's end.
    for _ in 12..=20 {
        let r = app.run("hearing_test.next", json!({})).unwrap();
        assert_eq!(r["kind"], json!("loudness"));
        if r["hz"] == json!(8000.0) {
            app.run("hearing_test.set", json!({"value": 10.0})).unwrap();
        }
        if r["hz"] == json!(125.0) {
            assert_eq!(app.run("hearing_test.set", json!({"value": 900.0})).unwrap()["value"], json!(25.0));
            app.run("hearing_test.set", json!({"value": 0.0})).unwrap();
        }
    }
    assert_eq!(app.run("hearing_test.state", json!({})).unwrap()["step"], json!(20));
    let r = app.run("hearing_test.next", json!({})).unwrap();
    assert_eq!(r["finished"], json!(true));
    assert!(!app.ui.show_hearing_test && app.ui.show_ear_profile);
    let draft = app.monitor.draft.clone().unwrap();
    assert_eq!((draft.name.as_str(), draft.headphones.as_str()), ("Me", "HD 600"));
    // 4 kHz is band 6, 8 kHz band 8: the balance is split, the loudness shared.
    assert_eq!((draft.left[6], draft.right[6]), (-3.0, 3.0));
    assert_eq!((draft.left[8], draft.right[8]), (10.0, 10.0));
    assert_eq!((draft.left[0], draft.right[3]), (0.0, 0.0));
    assert_eq!(app.engine.revision, rev);
    assert!(!app.engine.is_dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_step_of_the_hearing_test_can_play_a_clipped_or_loud_sound() {
    use soundcraft_ui_egui::hearing_ui::{HearingTest, STEPS};
    let mut test = HearingTest::default();
    test.answers.balance = [20.0; 10];
    test.answers.loudness = [25.0; 10];
    for step in 0..STEPS {
        test.step = step;
        let [l, r] = test.clip();
        assert_eq!(l.len(), r.len());
        assert!(l.len() > 24_000 && l.len() < 96_000);
        let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.9, "step {step}: peak {peak}");
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        assert!(rms(&l).max(rms(&r)) < 0.26, "step {step}");
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }
}

#[test]
fn the_first_master_strip_has_the_keys_and_the_menu_runs_the_same_commands() {
    let dir = scratch("keys");
    let mut app = app(&dir);
    save_me(&mut app);
    app.run("window.mix", json!({})).unwrap();
    let mut h = harness(app);
    h.get_by_label("Ear correction").click();
    h.run_steps(2);
    assert!(h.state().ui.monitor_ear);
    h.get_by_label("Simulated output").click();
    h.run_steps(2);
    assert!(h.state().ui.monitor_sim);
    assert_eq!(h.state().ui.monitor_sim_profile, "Mono");
    let extra = menus::ui_aliases();
    assert_eq!(menus::command_for_path("Options > Monitoring > Ear Correction", &extra).as_deref(), Some("monitor.ear"));
    let app = h.state_mut();
    menus::invoke_menu(app, "monitor.ear", "Options > Monitoring > Ear Correction");
    assert!(!app.ui.monitor_ear);
    assert_eq!(menus::item_state(app, "Options > Monitoring > Simulated Output", Some("monitor.sim")), (true, true));
    let _ = std::fs::remove_dir_all(&dir);
}

fn bounce(h: &mut Harness<'_, SoundApp>, file: &Path) -> Vec<u8> {
    let app = h.state_mut();
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    if let Some(Dialog::Bounce { path, .. }) = &mut app.dialogs.open {
        *path = file.to_string_lossy().into_owned();
    }
    h.run_steps(2);
    h.get_by_label("Bounce").click();
    h.run_steps(3);
    assert!(h.state().bounce.is_some(), "{}", h.state().ui.status);
    h.run();
    assert!(h.state().bounce.is_none());
    without_stamp(std::fs::read(file).unwrap())
}

/// A bounced WAV with its BWF origination date and time (the wall clock when it was written)
/// blanked, so two bounces of the same audio compare equal byte for byte.
fn without_stamp(mut wav: Vec<u8>) -> Vec<u8> {
    let bext = wav.windows(4).take(4096).position(|w| w == b"bext").expect("a BWF bext chunk");
    // description 256, originator 32, originator reference 32, then date 10 and time 8.
    let stamp = bext + 8 + 256 + 32 + 32;
    assert!(wav[stamp..stamp + 18].iter().all(|b| b.is_ascii_digit() || *b == b'-' || *b == b':'), "the date and time");
    wav[stamp..stamp + 18].fill(0);
    wav
}

#[test]
fn a_bounce_is_the_same_file_with_both_keys_on() {
    let dir = scratch("bounce");
    let mut app = app(&dir);
    save_me(&mut app);
    let mut h = harness(app);
    let plain = bounce(&mut h, &dir.join("Plain.wav"));
    h.state_mut().run("monitor.ear", json!({"value": true})).unwrap();
    h.state_mut().run("monitor.sim", json!({"profile": "iphone23"})).unwrap();
    h.run_steps(3);
    assert!(h.state().ui.monitor_ear && h.state().ui.monitor_sim);
    let monitored = bounce(&mut h, &dir.join("Monitored.wav"));
    assert!(plain.len() > 100_000);
    assert!(plain == monitored, "the monitor stage must never reach a bounce");
    let _ = std::fs::remove_dir_all(&dir);
}
