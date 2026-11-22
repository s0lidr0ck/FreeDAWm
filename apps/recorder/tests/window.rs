//! The window's main states, drawn headless: no devices, idle, recording, the booth away, a take
//! being sent.

mod common;

use common::{offline, ramp, temp_dir};
use egui_kittest::{Harness, kittest::Queryable};
use soundcraft_net::glue::FollowBadge;
use soundcraft_recorder::booth::BoothView;
use soundcraft_recorder::rig::InputChoice;
use soundcraft_recorder::settings::Prefs;
use soundcraft_recorder::ui::RecorderApp;

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };

fn app(channels: usize, ports: &[&str], ideas: &std::path::Path) -> (RecorderApp, common::Offline) {
    let mut o = offline(channels, ports, MONO);
    // The rig moves into the app; the test keeps the devices' other ends.
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    let prefs = Prefs { ideas_folder: ideas.to_string_lossy().into_owned(), ..Prefs::default() };
    (RecorderApp::with_rig(rig, prefs, None), o)
}

fn harness(app: RecorderApp) -> Harness<'static, RecorderApp> {
    Harness::builder().with_size(egui::vec2(520.0, 900.0)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), app)
}

fn has(h: &Harness<'_, RecorderApp>, text: &str) -> bool {
    h.query_by_label_contains(text).is_some()
}

#[test]
fn no_devices_says_so_and_record_is_off() {
    let dir = temp_dir("ui-none");
    let (a, _o) = app(0, &[], &dir);
    let mut h = harness(a);
    h.run_steps(3);
    assert!(has(&h, "No input devices found"));
    assert!(has(&h, "No MIDI input"));
    // Pressing Record does nothing (it is greyed).
    h.get_by_label("Record").click();
    h.run_steps(3);
    assert!(!h.state().rig.recording());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn idle_recording_and_saved() {
    let dir = temp_dir("ui-rec");
    let (a, mut o) = app(1, &["Keys"], &dir);
    let mut h = harness(a);
    h.run_steps(3);
    assert!(h.query_by_label("Record").is_some());
    assert!(has(&h, "Keys"), "the MIDI port is listed, on");
    assert!(has(&h, "No takes yet"));
    assert!(has(&h, "Nothing is sent anywhere until you pair"));
    h.get_by_label("Record").click();
    h.run_steps(3);
    assert!(h.state().rig.recording());
    assert!(h.query_by_label("Stop").is_some());
    // The device runs; the take records.
    for _ in 0..60 {
        o.block(&ramp);
    }
    h.get_by_label("Stop").click();
    h.run_steps(3);
    assert!(!h.state().rig.recording());
    assert!(has(&h, "Saved Idea "), "the window says what it saved");
    assert!(has(&h, "WAV 0:00"), "the take is in the list");
    let _ = std::fs::remove_dir_all(dir);
}

fn booth_view(badge: FollowBadge, connected: bool, queued: usize, progress: Option<f64>) -> BoothView {
    BoothView {
        badge,
        connected,
        state: if connected { "connected".into() } else { "looking for the booth".into() },
        link: "encrypted local link",
        session: "Sunday".into(),
        transport: "Stopped",
        position: 0.0,
        queued,
        progress,
        stream: None,
    }
}

#[test]
fn the_booth_away_and_a_take_being_sent() {
    let dir = temp_dir("ui-booth");
    let (mut a, _o) = app(1, &[], &dir);
    a.link.demo = Some(booth_view(FollowBadge::Away("Booth".into()), false, 1, None));
    a.prefs.listen = true;
    let mut h = harness(a);
    h.run_steps(3);
    assert!(has(&h, "Looking for Booth…"));
    assert!(has(&h, "Outbox: 1 take waiting"));
    assert!(has(&h, "The booth is away."));
    h.state_mut().link.demo = Some(booth_view(FollowBadge::Sending("Booth".into(), Some(45)), true, 1, Some(0.45)));
    h.run_steps(3);
    assert!(has(&h, "Sending take to Booth 45%"));
    assert!(has(&h, "Sending take: 45%"));
    assert!(has(&h, "Session: Sunday"));
    let _ = std::fs::remove_dir_all(dir);
}
