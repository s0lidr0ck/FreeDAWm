//! Bounce Mix renders across frames behind a progress window instead of freezing the app.
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_max_steps(1_000).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bounce_from_dialog(h: &mut Harness<'_, SoundApp>, file: &Path) {
    let app = h.state_mut();
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    if let Some(Dialog::Bounce { path, .. }) = &mut app.dialogs.open {
        *path = file.to_string_lossy().into_owned();
    }
    h.run_steps(2);
    h.get_by_label("Bounce").click();
    h.run_steps(3);
    assert!(h.state().bounce.is_some(), "{}", h.state().ui.status);
    assert!(h.query_by_label("Bounce Mix").is_none());
}

#[test]
fn a_bounce_renders_across_frames_with_progress_until_its_file_is_written() {
    let dir = scratch("bounce-frames");
    let mut h = harness();
    bounce_from_dialog(&mut h, &dir.join("Mix.wav"));
    let frames = h.state().bounce.as_ref().map_or(0, |b| b.range().len());
    let mut progress = Vec::new();
    for _ in 0..4 {
        progress.push(h.state().bounce.as_ref().map_or(1.0, |b| b.progress()));
        h.get_by_label("Cancel");
        h.step();
    }
    assert!(progress.is_sorted() && progress[0] < progress[3] && progress[3] < 1.0, "{progress:?}");
    h.run();
    assert!(h.state().bounce.is_none());
    assert!(h.state().ui.status.starts_with("Bounced Mix.wav (peak"), "{}", h.state().ui.status);
    let (_, file) = soundcraft_audio_io::decode(&std::fs::read(dir.join("Mix.wav")).unwrap(), None).unwrap();
    assert_eq!(file.frames() as i64, frames);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bounce_keeps_going_while_the_window_is_hidden() {
    let dir = scratch("bounce-hidden");
    let mut h = harness();
    bounce_from_dialog(&mut h, &dir.join("Hidden.wav"));
    let ctx = h.ctx.clone();
    let mut calls = 0;
    while h.state().bounce.is_some() {
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| h.state_mut().logic(ctx));
        assert!(h.state().bounce.is_none() || ctx.has_requested_repaint());
        calls += 1;
        assert!(calls < 10_000);
    }
    assert!(calls > 3, "{calls}");
    assert!(h.state().ui.status.starts_with("Bounced Hidden.wav"), "{}", h.state().ui.status);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_session_cannot_be_replaced_while_it_bounces() {
    let dir = scratch("bounce-replace");
    let mut h = harness();
    let tracks = h.state().engine.session().tracks.len();
    bounce_from_dialog(&mut h, &dir.join("Mix.wav"));
    for id in ["session.new", "session.open", "session.close", "session.revert"] {
        let err = h.state_mut().run(id, json!({"path": dir.join("Other.scraft").to_string_lossy()})).unwrap_err();
        assert_eq!(err, soundcraft_ui_egui::render::RENDERING, "{id}: {err}");
    }
    assert_eq!(h.state().engine.session().tracks.len(), tracks);
    assert!(h.state().bounce.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state_mut().run("session.close", json!({})).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_or_escape_stops_a_bounce_and_leaves_no_file() {
    let dir = scratch("bounce-cancel");
    let mut h = harness();
    bounce_from_dialog(&mut h, &dir.join("Clicked.wav"));
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().bounce.is_none());
    assert_eq!(h.state().ui.status, "Bounce cancelled");
    bounce_from_dialog(&mut h, &dir.join("Escaped.wav"));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().bounce.is_none());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_mp3_bounce_offers_a_bitrate_instead_of_a_bit_depth() {
    let dir = scratch("bounce-mp3");
    let mut h = harness();
    let app = h.state_mut();
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    if let Some(Dialog::Bounce { path, format, .. }) = &mut app.dialogs.open {
        *path = dir.join("Mix.wav").to_string_lossy().into_owned();
        *format = "mp3".into();
    }
    h.run_steps(2);
    assert!(h.query_by_label("Bitrate").is_some() && h.query_by_label("Bit Depth").is_none());
    match &h.state().dialogs.open {
        Some(Dialog::Bounce { bitrate, .. }) => assert_eq!(*bitrate, 320),
        other => panic!("{other:?}"),
    }
    if let Some(Dialog::Bounce { bitrate, .. }) = &mut h.state_mut().dialogs.open {
        *bitrate = 128;
    }
    h.run_steps(2);
    h.get_by_label("Bounce").click();
    h.run();
    assert!(h.state().ui.status.starts_with("Bounced Mix.mp3 (peak"), "{}", h.state().ui.status);
    let bytes = std::fs::read(dir.join("Mix.mp3")).unwrap();
    let (info, file) = soundcraft_audio_io::decode(&bytes, None).unwrap();
    assert_eq!((info.format, info.channels), (soundcraft_audio_io::FileFormat::Mp3, 2));
    let kbps = bytes.len() as f64 * 8.0 / file.duration_secs() / 1000.0;
    assert!((kbps - 128.0).abs() < 16.0, "{kbps:.0} kbps");
    let _ = std::fs::remove_dir_all(&dir);
}

fn harness_with_player() -> (Harness<'static, SoundApp>, soundcraft_playback::StalledAudio) {
    let engine = soundcraft_engine::demo::demo_engine();
    let (player, audio) = soundcraft_playback::Player::stalled(engine.session_arc(), 48_000, 512, 2);
    let app = SoundApp::new(engine, Some(player), Services::default());
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    (h, audio)
}

/// Bounce Mix › In real time: the live player plays the range and prints it; the file is what an
/// offline bounce of the range writes, and the transport stops at the end.
#[test]
fn a_real_time_bounce_plays_the_range_and_writes_what_the_offline_bounce_writes() {
    let dir = scratch("bounce-realtime");
    let (mut h, mut audio) = harness_with_player();
    let (start, end) = (480_000, 480_000 + 24_000);
    let params = |name: &str, realtime: bool| json!({"path": dir.join(name).to_string_lossy(), "bit_depth": "32f", "start": start, "end": end, "realtime": realtime});
    soundcraft_ui_egui::bounce::start(h.state_mut(), &params("Live.wav", true));
    assert!(h.state().print.is_some(), "{}", h.state().ui.status);
    assert!(h.state().rendering(), "editing and the transport wait for the pass");
    let mut out = vec![0.0f32; 2 * 512];
    for _ in 0..200 {
        audio.fill(&mut out, 2);
        h.step();
        if h.state().print.is_none() && h.state().bounce.is_none() {
            break;
        }
    }
    assert!(h.state().bounce.is_none(), "{}", h.state().ui.status);
    assert!(h.state().ui.status.starts_with("Bounced Live.wav"), "{}", h.state().ui.status);
    audio.fill(&mut out, 2);
    assert!(!h.state().player.as_ref().unwrap().is_playing(), "the transport stopped");
    // The same range offline, through instances of its own.
    let offline = soundcraft_engine::cmd::bounce_job(&h.state().engine, &params("Offline.wav", false)).unwrap().run().unwrap();
    let read = |p: &Path| soundcraft_audio_io::decode(&std::fs::read(p).unwrap(), None).unwrap().1;
    let (live, off) = (read(&dir.join("Live.wav")), read(&offline.files[0]));
    assert_eq!(live.frames(), 24_000);
    assert_eq!(live.channels.len(), off.channels.len());
    let worst = live.channels.iter().zip(&off.channels).flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs())).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "the print is the mix: {worst}");
    assert!(live.channels[0].iter().any(|x| x.abs() > 1e-3), "something played");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancel during the pass stops the transport and writes nothing.
#[test]
fn cancelling_a_real_time_bounce_stops_it_and_writes_nothing() {
    let dir = scratch("bounce-realtime-cancel");
    let (mut h, mut audio) = harness_with_player();
    let p = json!({"path": dir.join("Live.wav").to_string_lossy(), "start": 0, "end": 480_000, "realtime": true});
    soundcraft_ui_egui::bounce::start(h.state_mut(), &p);
    let mut out = vec![0.0f32; 2 * 512];
    for _ in 0..4 {
        audio.fill(&mut out, 2);
        h.step();
    }
    assert!(h.state().print.is_some());
    h.get_by_label("Cancel").click();
    h.step();
    h.step();
    assert!(h.state().print.is_none() && h.state().bounce.is_none());
    assert_eq!(h.state().ui.status, "Bounce cancelled");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}
