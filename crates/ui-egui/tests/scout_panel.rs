//! Scout in the UI: the package (its shape against the owner's reference, the chain as the drawer
//! shows it, base and post), Copy, the Scout button and the panel's empty states.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Insert, Mixer, ScoutCapture, ScoutMetrics, ScoutStrip, Session, TrackId, TrackKind};
use soundcraft_time::Range;
use soundcraft_ui_egui::scout::package::{build_package, prompt_text};
use soundcraft_ui_egui::scout::panel::{NO_CAPTURES, NO_KEY};
use soundcraft_ui_egui::{Services, SoundApp};

const REFERENCE: &str = include_str!("fixtures/scout-example-package.json");

/// The reference package's base metrics ("Lead Vocals", 2 s), as `ScoutMetrics`.
fn reference_metrics() -> ScoutMetrics {
    let r: Value = serde_json::from_str(REFERENCE).unwrap();
    let c = &r["track"]["capture"];
    let f = |v: &Value| v.as_f64().unwrap() as f32;
    let mut m = ScoutMetrics { valid: true, duration_sec: 2.0, sample_rate: 48_000.0, activity_ratio: 1.0, ..ScoutMetrics::default() };
    m.peak_hold_dbfs = f(&c["capture_quality"]["peak_hold_dbfs"]);
    m.noise_floor_p10_dbfs = f(&c["capture_quality"]["noise_floor_p10_dbfs"]);
    m.peak_dbfs = f(&c["level"]["peak_dbfs"]);
    m.true_peak_dbtp = f(&c["level"]["true_peak_dbtp"]);
    m.rms_dbfs = f(&c["level"]["rms_dbfs"]);
    m.lufs_integrated = f(&c["level"]["lufs_integrated"]);
    m.crest_factor_db = f(&c["dynamics"]["crest_factor_db"]);
    m.dynamic_range_db = f(&c["dynamics"]["dynamic_range_db"]);
    m.lra = f(&c["dynamics"]["loudness_range_lra"]);
    m.peak_to_lufs_db = f(&c["dynamics"]["peak_to_lufs_db"]);
    m.transient_density = f(&c["dynamics"]["transient_density"]);
    m.lufs_p10 = f(&c["loudness_distribution"]["p10_lufs"]);
    m.lufs_p50 = f(&c["loudness_distribution"]["p50_lufs"]);
    m.lufs_p90 = f(&c["loudness_distribution"]["p90_lufs"]);
    m.lufs_momentary_max = f(&c["loudness_distribution"]["momentary_max_lufs"]);
    m.lufs_short_term_max = f(&c["loudness_distribution"]["short_term_max_lufs"]);
    for (d, s) in m.spectrum31.iter_mut().zip(c["spectrum_31_band"].as_array().unwrap()) {
        *d = f(s);
    }
    let z = &c["broad_zones"];
    (m.zone_sub, m.zone_low, m.zone_low_mid, m.zone_mid) = (f(&z["sub_20_60"]), f(&z["low_60_150"]), f(&z["low_mid_150_500"]), f(&z["mid_500_2000"]));
    (m.zone_high_mid, m.zone_high, m.zone_air) = (f(&z["high_mid_2000_6000"]), f(&z["high_6000_10000"]), f(&z["air_10000_20000"]));
    m.spectral_centroid_hz = f(&c["tonal"]["spectral_centroid_hz"]);
    m
}

fn strip(track: TrackId, name: &str, mixer: Mixer) -> ScoutStrip {
    let m = reference_metrics();
    ScoutStrip {
        track,
        name: name.into(),
        kind: TrackKind::Audio,
        channel_type: "channel".into(),
        role: "Channel".into(),
        channels: 1,
        mixer,
        automated: Vec::new(),
        base: m.clone(),
        post: m,
        note: None,
        target: None,
    }
}

/// A session with one mono track holding one stored capture of it, with `mixer` as its snapshot.
fn session_with(mixer: Mixer) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vocals"));
    s.scout.captures.push(ScoutCapture {
        id: 3,
        name: "Verse 2".into(),
        created: "2026-10-20T12:00:00Z".into(),
        sample_rate: 48_000.0,
        tempo: 92.0,
        played: vec![Range::new(48_000, 144_000)],
        loop_range: None,
        played_truncated: false,
        strips: vec![strip(t, "Lead Vocals", mixer)],
        bands_version: 2,
    });
    s.scout.next_id = 4;
    (s, t)
}

/// Every key path of `reference` exists in `ours` with the same JSON type (arrays: their first
/// elements compared). `preset_for_clean_q` is replaced by `guides`.
fn same_shape(reference: &Value, ours: &Value, path: &str, missing: &mut Vec<String>) {
    match (reference, ours) {
        (Value::Object(r), Value::Object(o)) => {
            for (k, rv) in r {
                let p = format!("{path}.{k}");
                if k == "preset_for_clean_q" {
                    if !o.contains_key("guides") {
                        missing.push(format!("{path}.guides"));
                    }
                    continue;
                }
                match o.get(k) {
                    Some(ov) => same_shape(rv, ov, &p, missing),
                    None => missing.push(p),
                }
            }
        }
        (Value::Array(r), Value::Array(o)) => {
            if let (Some(a), Some(b)) = (r.first(), o.first()) {
                same_shape(a, b, &format!("{path}[0]"), missing);
            } else if !r.is_empty() {
                missing.push(format!("{path}[] (empty)"));
            }
        }
        (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_))
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Null, Value::Null) => {}
        (r, o) => missing.push(format!("{path}: {r} vs {o}")),
    }
}

#[test]
fn package_matches_the_reference_shape() {
    let (s, _) = session_with(Mixer::new(ChannelFormat::Mono));
    let pkg = build_package(&egui::Context::default(), &s, &[3], "What processing would you suggest for this track?", "Lead vocal on top");
    let reference: Value = serde_json::from_str(REFERENCE).unwrap();
    let mut missing = Vec::new();
    same_shape(&reference, &pkg, "", &mut missing);
    assert!(missing.is_empty(), "{missing:#?}");
    assert_eq!(pkg["scout_payload_version"], "0.6.0-daw");
    assert_eq!(pkg["payload_type"], "single_capture", "one strip with no module: base only");
    // The derived values and flags are Scout2's: the reference's own numbers come back.
    let rt = &reference["track"];
    let t = &pkg["track"];
    assert_eq!(t["capture"]["tonal"]["tilt_db_per_octave"], rt["capture"]["tonal"]["tilt_db_per_octave"]);
    assert_eq!(t["capture"]["tonal"]["resonances_above_trend"], rt["capture"]["tonal"]["resonances_above_trend"]);
    assert_eq!(t["scout_flags"], rt["scout_flags"]);
    assert_eq!(t["capture"]["stereo"], rt["capture"]["stereo"]);
    assert_eq!(t["capture_duration_s"], json!(2.0), "from the played range");
    // The new fields.
    assert_eq!(pkg["session"]["sample_rate"], json!(48_000.0));
    assert_eq!(pkg["capture"]["name"], "Verse 2");
    assert_eq!(pkg["capture"]["played_ranges_s"], json!([[1.0, 3.0]]));
    assert_eq!(pkg["capture"]["played_truncated"], false);
    assert_eq!(pkg["analysis_request"]["goal"], "Lead vocal on top");
    assert!(t["routing"].get("sends").is_some() && t["routing"].get("output").is_some());
    let lim: Vec<&str> = pkg["known_limitations"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for l in [
        "Surround strips are measured on the ITU stereo fold-down",
        "Captures are at most 30 seconds",
        "Frames captured while the transport was stopped before Play are included (silence or live input)",
    ] {
        assert!(lim.contains(&l), "{l}");
    }
    let ins = &pkg["ai_response_instructions"];
    assert!(ins.get("preset_for_clean_q").is_none());
    assert_eq!(ins["guides"]["example"]["format"], "scout-guides");
    assert!(
        ins["constraints"].as_array().unwrap().iter().any(|c| c.as_str().is_some_and(|c| c.starts_with("Never tell the engineer to apply a preset")))
    );
}

#[test]
fn package_lists_only_drawer_visible_params_with_display_values() {
    let mut cq = Insert::new("fs_cleanq");
    cq.params.insert("b01_on".into(), 2.0);
    cq.params.insert("b01_dyn".into(), 1.0);
    cq.params.insert("b01_thr".into(), -18.0);
    cq.params.insert("ceilOn".into(), 0.0);
    let mut mixer = Mixer::new(ChannelFormat::Mono);
    mixer.inserts.push(cq);
    let (s, _) = session_with(mixer);
    let pkg = build_package(&egui::Context::default(), &s, &[3], "", "");
    let chain = &pkg["tracks"][0]["chain"];
    assert_eq!(chain[0]["plugin"], "CleanQ", "{chain}");
    assert_eq!(chain[0]["position"], 1);
    assert_eq!(chain[0]["bypassed"], false);
    let params = chain[0]["params"].as_array().unwrap();
    let get = |n: &str| params.iter().find(|p| p["name"] == n).map(|p| p["value"].clone());
    assert_eq!(get("B1 Threshold"), Some(json!("-18.0 dB")), "{params:?}");
    assert_eq!(get("B1 On"), Some(json!("Active")));
    assert!(get("Ceiling Dynamic").is_some(), "the switch itself says the ceiling is off");
    for gone in ["Ceiling Threshold", "Ceiling Amount", "B2 Freq", "B2 On", "Bypass", "B1 Slope"] {
        assert!(get(gone).is_none(), "{gone} is inert or never offered");
    }
    // A band that isn't dynamic has no threshold to report.
    let mut s2 = s.clone();
    s2.scout.captures[0].strips[0].mixer.inserts[0].params.insert("b01_dyn".into(), 0.0);
    let pkg = build_package(&egui::Context::default(), &s2, &[3], "", "");
    assert!(pkg["tracks"][0]["chain"][0]["params"].as_array().unwrap().iter().all(|p| p["name"] != "B1 Threshold"));
}

#[test]
fn base_and_post_share_the_track_name() {
    let mut mixer = Mixer::new(ChannelFormat::Mono);
    mixer.inserts.push(Insert::new("fs_cleanq"));
    let (s, t) = session_with(mixer);
    let pkg = build_package(&egui::Context::default(), &s, &[3], "", "");
    assert_eq!(pkg["payload_type"], "multi_capture");
    let tracks = pkg["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0]["name"], "Lead Vocals");
    assert_eq!(tracks[1]["name"], "Lead Vocals");
    assert_eq!(tracks[0]["capture_position"], "base");
    assert_eq!(tracks[1]["capture_position"], "post");
    assert_eq!(tracks[0]["track_id"], json!(t.0));
    assert_eq!(pkg["capture_sync"]["simultaneous"], true);
    assert_eq!(pkg["project"]["track_count"], 2);
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

#[test]
fn copy_puts_the_prompt_on_the_clipboard() {
    let (s, _) = session_with(Mixer::new(ChannelFormat::Mono));
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.run_steps(3);
    h.state_mut().scout.question = "Is the vocal harsh?".into();
    h.state_mut().run("ui.scout.copy", json!({})).unwrap();
    h.step();
    let copied: Vec<String> = h
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let expected = prompt_text(&build_package(&h.ctx, h.state().engine.session(), &[3], "Is the vocal harsh?", ""), "Is the vocal harsh?");
    assert_eq!(copied, vec![expected.clone()]);
    assert!(expected.starts_with("Is the vocal harsh?"));
    assert!(expected.contains("\"scout_payload_version\": \"0.6.0-daw\""));
    // The package command returns the same package.
    let pkg = h.state_mut().run("ui.scout.package", json!({"captures": [3], "question": "Is the vocal harsh?"})).unwrap();
    assert_eq!(prompt_text(&pkg, "Is the vocal harsh?"), expected);
}

#[test]
fn scout_button_starts_and_stops_a_capture() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    s.edit.selected_tracks = vec![t];
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.run_steps(5);
    assert!(h.query_by_label_contains("1 selected").is_some());
    h.get_by_label("Scout").click();
    h.run_steps(3);
    assert!(h.state().engine.scout_capture.is_some(), "{}", h.state().ui.status);
    assert!(h.query_by_label_contains("capturing · click to stop").is_some());
    h.get_by_label("Scout stop").click();
    h.run_steps(3);
    let st = h.state();
    assert_eq!(st.engine.scout_capture, None);
    assert_eq!(st.engine.session().scout.captures.len(), 1, "the rig came back and was stored");
    assert_eq!(st.engine.session().scout.captures[0].strips[0].name, "Vox");
    assert!(h.query_by_label("Scout").is_some());
}

#[test]
fn the_transport_stopping_ends_a_capture() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    s.edit.selected_tracks = vec![t];
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.run_steps(3);
    h.state_mut().run("transport.play", json!({})).unwrap();
    h.state_mut().run("scout.capture", json!({"action": "start"})).unwrap();
    h.run_steps(3);
    h.state_mut().run("transport.stop", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().engine.scout_capture, None);
    assert_eq!(h.state().engine.session().scout.captures.len(), 1);
}

#[test]
fn panel_shows_empty_states() {
    let mut h = harness(SoundApp::new(Engine::default(), None, Services::default()));
    h.run_steps(2);
    h.state_mut().run("window.scout", json!({})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label_contains(NO_CAPTURES).is_some());
    assert!(h.query_by_label_contains(NO_KEY).is_some());
    // With a capture it lists it, ticked by default.
    let (s, _) = session_with(Mixer::new(ChannelFormat::Mono));
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label_contains(NO_CAPTURES).is_none());
    assert!(h.query_by_label_contains("Verse 2").is_some());
    assert_eq!(h.state().scout.chosen(h.state().engine.session()), vec![3]);
}

#[test]
fn multi_capture_tracks_match_the_reference_track_shape() {
    let mut mixer = Mixer::new(ChannelFormat::Mono);
    mixer.inserts.push(Insert::new("fs_cleanq"));
    let (s, _) = session_with(mixer);
    let pkg = build_package(&egui::Context::default(), &s, &[3], "Too harsh?", "Lead vocal on top");
    assert_eq!(pkg["payload_type"], "multi_capture");
    let reference: Value = serde_json::from_str(REFERENCE).unwrap();
    let mut missing = Vec::new();
    for (i, t) in pkg["tracks"].as_array().unwrap().iter().enumerate() {
        same_shape(&reference["track"], t, &format!("tracks[{i}]"), &mut missing);
    }
    // Everything else at the top level keeps the reference's shape too.
    for (k, v) in reference.as_object().unwrap() {
        if k != "track" && k != "payload_type" {
            match pkg.get(k) {
                Some(o) => same_shape(v, o, k, &mut missing),
                None => missing.push(k.clone()),
            }
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
    assert!(pkg.get("track").is_none(), "multi_capture carries tracks, not track");
}

#[test]
fn captured_strips_light_pre_and_post_and_the_others_do_not() {
    let mut s = Session::default();
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Gtr"));
    s.edit.selected_tracks = vec![vox];
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.state_mut().ui.window = soundcraft_ui_egui::MainWindow::Mix;
    h.run_steps(3);
    assert_eq!(h.query_all_by_label("Scout PRE/POST").count(), 0, "off until Scout is pressed");
    h.state_mut().run("scout.capture", json!({"action": "start"})).unwrap();
    h.run_steps(3);
    assert_eq!(h.query_all_by_label("Scout PRE/POST").count(), 1, "only the captured strip");
    h.state_mut().run("scout.capture", json!({"action": "stop"})).unwrap();
    h.run_steps(3);
    assert_eq!(h.query_all_by_label("Scout PRE/POST").count(), 0);
}

#[test]
fn old_and_new_band_captures_are_told_apart() {
    // A v2 capture shows "v2 bands"; one stored before the 2026-10-21 bands shows "v1 bands".
    let (mut s, _) = session_with(Mixer::new(ChannelFormat::Mono));
    let pkg = build_package(&egui::Context::default(), &s, &[3], "?", "");
    assert_eq!(pkg["capture"]["spectrum_bands_version"], 2);
    let mut h = harness(SoundApp::new(Engine::new(s.clone()), None, Services::default()));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("v2 bands").is_some());
    assert!(h.query_by_label("v1 bands").is_none());

    s.scout.captures[0].bands_version = 1;
    let pkg = build_package(&egui::Context::default(), &s, &[3], "?", "");
    assert_eq!(pkg["capture"]["spectrum_bands_version"], 1);
    let mut h = harness(SoundApp::new(Engine::new(s.clone()), None, Services::default()));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("v1 bands").is_some());
    assert!(h.query_by_label("v2 bands").is_none());

    // A later design shows its own number.
    s.scout.captures[0].bands_version = 3;
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.state_mut().run("window.scout", json!({"value": true})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("v3 bands").is_some());
}

fn copied_texts(h: &Harness<'static, SoundApp>) -> Vec<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn header_open_shows_the_scout_window_and_keeps_it_open() {
    let mut h = harness(SoundApp::new(Engine::new(Session::default()), None, Services::default()));
    h.run_steps(3);
    assert!(!h.state().scout.show);
    h.get_by_label("Open Scout window").click();
    h.run_steps(3);
    assert!(h.state().scout.show);
    h.get_by_label("Open Scout window").click();
    h.run_steps(3);
    assert!(h.state().scout.show, "Open never toggles the window closed");
}

#[test]
fn header_copy_copies_the_newest_capture() {
    let (mut s, _) = session_with(Mixer::new(ChannelFormat::Mono));
    let mut newer = s.scout.captures[0].clone();
    newer.id = 4;
    newer.name = "Verse 3".into();
    s.scout.captures.push(newer);
    let mut h = harness(SoundApp::new(Engine::new(s), None, Services::default()));
    h.run_steps(3);
    h.get_by_label("Copy last Scout capture").click();
    h.run_steps(2);
    let copied = copied_texts(&h);
    let pkg = build_package(&h.ctx, h.state().engine.session(), &[4], "", "");
    assert_eq!(copied, vec![prompt_text(&pkg, "")], "{}", h.state().ui.status);
}

#[test]
fn header_copy_is_disabled_without_a_capture() {
    let mut h = harness(SoundApp::new(Engine::new(Session::default()), None, Services::default()));
    h.run_steps(3);
    h.get_by_label("Copy last Scout capture").click();
    h.run_steps(3);
    assert!(copied_texts(&h).is_empty());
    assert!(h.state().ui.status.is_empty() || !h.state().ui.status.contains("copied"), "{}", h.state().ui.status);
}
