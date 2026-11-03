//! The per-machine record offset (`setup.record_offset`), the loopback Measure's outcome, and the
//! rate warning.
use serde_json::json;
use soundcraft_playback::Latency;
use soundcraft_ui_egui::record_offset::{NOT_MEASURED, take_latency};
use soundcraft_ui_egui::{RecordOffset, Services, SoundApp, UiState, rate_warning, record_latency};

fn app() -> SoundApp {
    SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default())
}

#[test]
fn record_offset_is_per_system_device_buffer_and_rate() {
    let mut app = app();
    let other = soundcraft_playback::devices::latency_key("ASIO", "X32", Some(128), 48_000);
    app.ui.record_offsets.insert(other.clone(), RecordOffset { manual: 7, measured: Some(250), ..RecordOffset::default() });
    let here = app.record_offset_key();
    assert_ne!(here, other);

    let r = app.run("setup.record_offset", json!({"samples": 10})).unwrap();
    assert_eq!((r["manual"].clone(), r["key"].clone(), r["measured_here"].clone()), (json!(10), json!(here), json!(false)));
    assert_eq!(app.ui.record_offsets[&other], RecordOffset { manual: 7, measured: Some(250), ..RecordOffset::default() }, "untouched");

    let r = app.run("setup.record_offset", json!({})).unwrap();
    assert_eq!(r["manual"], 10, "no samples: it only reads");
    assert_eq!(r["total"], 10, "no device reports anything here");
}

#[test]
fn measured_replaces_the_reported_estimate_but_not_manual_or_mix() {
    let reported = Latency { input: 100, output: 100, feed: 0, mix: 64 };
    assert_eq!(record_latency(&RecordOffset { manual: 5, measured: Some(300), ..RecordOffset::default() }, reported), 369);
    assert_eq!(record_latency(&RecordOffset { manual: 5, measured: None, ..RecordOffset::default() }, reported), 269);
}

#[test]
fn a_take_uses_the_feed_and_mix_at_its_first_frame() {
    // Measured through a feed holding 40; this take's feed holds 70, and LLM dropped the mix's
    // delay to 0 when it started.
    let o = RecordOffset { manual: 0, measured: Some(340), measured_feed: 40 };
    let reported = Latency { input: 0, output: 0, feed: 55, mix: 64 };
    assert_eq!(take_latency(&o, reported, Some(70), Some(0)), 370);
    assert_eq!(take_latency(&o, reported, None, None), 300 + 55 + 64, "no first frame yet: the reported figures");
}

#[test]
fn record_offset_clamps_hostile_numbers() {
    let mut app = app();
    assert_eq!(app.run("setup.record_offset", json!({"samples": 1e12})).unwrap()["manual"], 96_000);
    assert_eq!(app.run("setup.record_offset", json!({"samples": -1e12})).unwrap()["manual"], -96_000);
    assert_eq!(app.run("setup.record_offset", json!({"samples": 12.6})).unwrap()["manual"], 13);
    let err = app.run("setup.record_offset", json!({"samples": "lots"})).unwrap_err();
    assert_eq!(err, "`samples` must be a number");
    assert_eq!(app.record_offset().manual, 13, "a refused value changes nothing");
}

#[test]
fn a_failed_measure_keeps_the_previous_value() {
    let mut app = app();
    let key = app.record_offset_key();
    app.finish_measure(Ok(300));
    assert_eq!(app.ui.record_offsets[&key].measured, Some(300));
    assert!(app.ui.status.starts_with("Round trip at the default buffer: 300 samples ("), "{}", app.ui.status);

    for err in ["No click came back on input 1: patch output 1 to it", "Measure timed out: the audio output stopped"] {
        app.finish_measure(Err(err.into()));
        assert_eq!(app.ui.status, err);
        assert_eq!(app.ui.record_offsets[&key].measured, Some(300), "{err}: the previous value stays");
    }
    // Without a device Measure is refused, and nothing changes.
    assert_eq!(app.run("setup.measure_latency", json!({})).unwrap_err(), "no audio device");
    assert!(app.run("setup.measure_latency", json!({"output": "Nowhere"})).unwrap_err().contains("Nowhere"));
    assert!(!app.measuring());
    assert_eq!(app.ui.record_offsets[&key].measured, Some(300));
}

#[test]
fn the_record_button_says_whether_the_offset_is_measured() {
    let mut app = app();
    app.run("setup.record_offset", json!({"samples": 12})).unwrap();
    assert_eq!(app.record_offset_summary(), format!("offset 12 samples ({NOT_MEASURED})"));
    app.finish_measure(Ok(200));
    assert_eq!(app.record_offset_summary(), "offset 212 samples (measured)");
}

#[test]
fn rate_warning_copy() {
    assert_eq!(rate_warning(44_100, 48_000).as_deref(), Some("Device runs at 44.1 kHz, session is 48 kHz: takes are resampled and may drift"));
    assert_eq!(rate_warning(96_000, 88_200).as_deref(), Some("Device runs at 96 kHz, session is 88.2 kHz: takes are resampled and may drift"));
    assert_eq!(rate_warning(48_000, 48_000), None);
}

#[test]
fn record_offsets_survive_a_prefs_round_trip() {
    let mut ui = UiState::default();
    ui.record_offsets.insert("ASIO|X32|96|48000".into(), RecordOffset { manual: -3, measured: Some(412), measured_feed: 0 });
    ui.record_offsets.insert("WASAPI|Speakers + Mic|default|44100".into(), RecordOffset { manual: 20, measured: None, measured_feed: 0 });
    let back: UiState = serde_json::from_str(&serde_json::to_string(&ui).unwrap()).unwrap();
    assert_eq!(back.record_offsets, ui.record_offsets);
    // Preferences saved before the offsets existed load with none.
    let old: UiState = serde_json::from_value(json!({"status": "x"})).unwrap();
    assert!(old.record_offsets.is_empty());
}

fn step(h: &mut egui_kittest::Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

#[test]
fn the_playback_engine_window_shows_the_offset_not_512() {
    use egui_kittest::kittest::Queryable;
    let engine = soundcraft_engine::Engine::default();
    let player = soundcraft_playback::Player::new(engine.session_arc());
    let mut app = SoundApp::new(engine, Some(player), Services::default());
    app.ui.show_playback_engine = true;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    assert!(h.query_by_label(NOT_MEASURED).is_some(), "unmeasured here");
    assert_eq!(h.query_all_by_label("Measure").count(), 2, "the row and its button");
    assert!(h.query_by_label("Underruns").is_some() && h.query_by_label("DSP load").is_some());
    assert!(h.query_by_label_contains("512-sample").is_none(), "no fixed 512 any more");
    h.state_mut().finish_measure(Ok(300));
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(h.query_by_label("(measured)").is_some());
    assert!(h.query_by_label(NOT_MEASURED).is_none());
}
