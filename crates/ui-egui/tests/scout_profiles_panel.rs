//! Scout ▸ Profiles: the library tab (search, role filter, the cards), Import Reference…, saving a
//! capture's strip as a profile and Capture per stem ending without hanging the UI.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions};
use soundcraft_engine::Engine;
use soundcraft_engine::scout_profiles as lib;
use soundcraft_model::{ProfileKind, ScoutMetrics, Session};
use soundcraft_ui_egui::scout::Tab;
use soundcraft_ui_egui::scout::profiles::NO_PROFILES;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("scout-profiles-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app_with(dir: &std::path::Path, s: Session) -> SoundApp {
    let services = Services { profiles_dir: Some(dir.to_path_buf()), ..Services::default() };
    let mut app = SoundApp::new(Engine::new(s), None, services);
    app.scout.show = true;
    app.scout.tab = Tab::Profiles;
    app
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

fn metrics(lufs: f32) -> ScoutMetrics {
    let mut m = ScoutMetrics { valid: true, lufs_integrated: lufs, stereo_width: 0.3, lra: 4.0, crest_factor_db: 10.0, ..ScoutMetrics::default() };
    for (i, b) in m.spectrum31.iter_mut().enumerate() {
        *b = -20.0 - i as f32 * 0.8;
    }
    m
}

#[test]
fn the_profiles_tab_lists_searches_and_filters_the_library() {
    let dir = scratch("tab");
    let mut h = harness(app_with(&dir, Session::default()));
    h.run_steps(3);
    assert!(h.query_by_label(NO_PROFILES).is_some());
    for (name, role, kind) in
        [("Warm Bass", "bass", ProfileKind::Track), ("Airy Vox", "vocal", ProfileKind::Track), ("Loud Master", "mix", ProfileKind::Mix)]
    {
        let mut p = lib::make(name, kind, role, vec!["pop".into()], metrics(-14.0), String::new());
        lib::save_new(&dir, &mut p).unwrap();
    }
    h.state_mut().scout.profiles.entries = None;
    h.run_steps(3);
    for n in ["Warm Bass", "Airy Vox", "Loud Master"] {
        assert!(h.query_by_label(n).is_some(), "{n}");
    }
    h.state_mut().scout.profiles.role = "vocal".into();
    h.run_steps(2);
    assert!(h.query_by_label("Airy Vox").is_some());
    assert!(h.query_by_label("Warm Bass").is_none());
    h.state_mut().scout.profiles.role.clear();
    h.state_mut().scout.profiles.query = "loud".into();
    h.run_steps(2);
    assert!(h.query_by_label("Loud Master").is_some());
    assert!(h.query_by_label("Airy Vox").is_none());
    // Delete from the card.
    h.state_mut().scout.profiles.query.clear();
    h.run_steps(2);
    h.state_mut().run("scout.profile_delete", json!({"name": "Airy Vox", "dir": dir.to_string_lossy()})).unwrap();
    h.state_mut().scout.profiles.entries = None;
    h.run_steps(2);
    assert!(h.query_by_label("Airy Vox").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_reference_measures_a_file_into_the_library() {
    let dir = scratch("ref");
    let wav = dir.join("Reference Song.wav");
    let n = 48_000;
    let tone: Vec<f32> = (0..n).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
    let bytes = soundcraft_audio_io::encode(
        &AudioBuffer { sample_rate: 48_000, channels: vec![tone.clone(), tone] },
        &EncodeOptions { bit_depth: BitDepth::Float32, ..Default::default() },
    )
    .unwrap();
    std::fs::write(&wav, bytes).unwrap();
    let lib_dir = dir.join("profiles");
    let mut app = app_with(&lib_dir, Session::default());
    app.scout.show = false;
    app.scout.tab = Tab::Captures;
    // No dialog in tests: nothing happens.
    assert!(app.run("ui.scout.import_reference", json!({})).unwrap()["path"].is_null());
    let r = app.run("ui.scout.import_reference", json!({"path": wav.to_string_lossy()})).unwrap();
    assert_eq!(r["profiles"][0]["name"], "Reference Song");
    assert_eq!(r["profiles"][0]["kind"], "mix");
    assert!(app.scout.show && app.scout.tab == Tab::Profiles, "the library opens");
    assert_eq!(lib::list(&lib_dir).len(), 1);
    assert!(app.engine.session().tracks.is_empty(), "not added to the session");
    // Export, then import back under a new name.
    let out = dir.join("out.json");
    app.run("ui.scout.profile_export", json!({"name": "Reference Song", "path": out.to_string_lossy()})).unwrap();
    let r = app.run("ui.scout.profile_import", json!({"path": out.to_string_lossy()})).unwrap();
    assert_eq!(r["name"], "Reference Song 2");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_per_stem_runs_off_the_ui_thread_and_reports_its_end() {
    let dir = scratch("stems");
    let mut s = Session::default();
    let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Song"));
    s.edit.selected_tracks = vec![t];
    let mut h = harness(app_with(&dir, s));
    h.run_steps(2);
    // The track has no audio: the worker ends with an error, the panel stays usable.
    h.state_mut().run("ui.scout.capture_stems", json!({})).unwrap();
    assert!(h.state().scout.profiles.busy());
    for _ in 0..200 {
        h.step();
        if !h.state().scout.profiles.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!h.state().scout.profiles.busy());
    assert!(h.state().ui.status.contains("audio"), "{}", h.state().ui.status);
    let _ = std::fs::remove_dir_all(&dir);
}

fn captured_session() -> Session {
    use soundcraft_model::{ChannelFormat, Mixer, ScoutCapture, ScoutStrip, TrackKind};
    let mut s = Session::default();
    let v = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let master = Some(s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master")));
    let strip = |track, name: &str, ct: &str, lufs: f32| ScoutStrip {
        track,
        name: name.into(),
        kind: TrackKind::Audio,
        channel_type: ct.into(),
        role: "Channel".into(),
        channels: 1,
        mixer: Mixer::new(ChannelFormat::Mono),
        automated: Vec::new(),
        base: metrics(lufs),
        post: metrics(lufs),
        note: None,
        target: None,
    };
    let mut strips = vec![strip(v, "Vox", "channel", -20.0)];
    if let Some(m) = master {
        strips.push(strip(m, "Master", "master", -16.0));
    }
    s.scout.captures.push(ScoutCapture {
        id: 1,
        name: "Chorus".into(),
        created: String::new(),
        sample_rate: 48_000.0,
        tempo: 120.0,
        played: vec![soundcraft_time::Range::new(0, 48_000)],
        loop_range: None,
        played_truncated: false,
        strips,
        bands_version: 2,
    });
    s
}

#[test]
fn a_target_travels_in_the_package_and_the_prompt() {
    use soundcraft_ui_egui::scout::package::{build_package, prompt_text};
    let dir = scratch("target");
    let mut target = lib::make("Hit Vocal", ProfileKind::Track, "vocal", Vec::new(), metrics(-14.0), String::new());
    target.metrics.spectrum31[20] += 6.0;
    lib::save_new(&dir, &mut target).unwrap();
    let mut app = app_with(&dir, captured_session());
    let d = dir.to_string_lossy().to_string();
    app.run("scout.target_set", json!({"capture": 1, "track": "Vox", "profile": "Hit Vocal", "dir": d})).unwrap();
    let st = &app.engine.session().scout.captures[0].strips[0];
    assert_eq!(st.target.as_ref().unwrap().name, "Hit Vocal");
    let ctx = egui::Context::default();
    let pkg = build_package(&ctx, app.engine.session(), &[1], "", "");
    let vox = pkg["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Vox").unwrap();
    let tg = &vox["target"];
    assert_eq!(tg["name"], "Hit Vocal");
    assert_eq!(tg["capture"]["level"]["lufs_integrated"], -14.0, "the target's numbers next to the strip's");
    assert_eq!(vox["capture"]["level"]["lufs_integrated"], -20.0);
    assert_eq!(tg["difference_target_minus_this"]["lufs_integrated"], 6.0);
    let band = tg["difference_target_minus_this"]["spectrum_shape_31_band"][20].as_f64().unwrap();
    assert!(band > 5.0, "{band}");
    let text = prompt_text(&pkg, "");
    assert!(text.contains("Compare Vox with its target and move it toward the target; the target is Hit Vocal"), "{}", &text[..300]);
    assert!(text.contains("compare its numbers with target.capture"));
    // A mix profile on the master is a mastering target; undo takes the target back off.
    let mut master = lib::make("Loud Master", ProfileKind::Mix, "", Vec::new(), metrics(-9.0), String::new());
    master.source.file = "ref.wav".into();
    app.run("scout.target_set", json!({"capture": 1, "track": "Master", "profile": serde_json::to_value(&master).unwrap()})).unwrap();
    let pkg = build_package(&ctx, app.engine.session(), &[1], "", "");
    let text = prompt_text(&pkg, "Louder?");
    assert!(text.contains("Compare Master with its mastering target and move it toward the target; the target is Loud Master (ref.wav)."));
    app.run("edit.undo", json!({})).unwrap();
    assert!(app.engine.session().scout.captures[0].strips[1].target.is_none());
    app.run("scout.target_set", json!({"capture": 1, "track": "Vox", "profile": null})).unwrap();
    let pkg = build_package(&ctx, app.engine.session(), &[1], "", "");
    assert!(!prompt_text(&pkg, "").contains("target is"));
    assert!(app.run("scout.target_set", json!({"capture": 1, "track": "Nope", "profile": null})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
