//! Joining a studio from the studio screen, against a real booth hub on this machine: by the
//! code it shows, by its booth PIN (a wrong one said plainly), and by an internet invite through
//! the dial-only endpoint (no relay here), reconnecting from the saved settings later.

mod common;

use common::{offline, temp_dir};
use egui_kittest::{Harness, kittest::Queryable};
use soundcraft_net::config::NetConfig;
use soundcraft_net::hub::Hub;
use soundcraft_net::internet::{Internet, Invite, Relays, new_key};
use soundcraft_recorder::booth::Link;
use soundcraft_recorder::rig::InputChoice;
use soundcraft_recorder::settings::{Paths, Prefs};
use soundcraft_recorder::ui::{RecorderApp, Screen, look};
use std::time::{Duration, Instant};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };
const PIN: &str = "482113";

fn harness(dir: &std::path::Path, paths: Option<Paths>) -> (Harness<'static, RecorderApp>, common::Offline) {
    let mut o = offline(1, &[], MONO);
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    let prefs = Prefs { ideas_folder: dir.join("Ideas").to_string_lossy().into_owned(), ..Prefs::default() };
    let mut app = RecorderApp::with_rig(rig, prefs, paths);
    app.link.cfg.pc_name = "Test Recorder".into();
    let h = Harness::builder().with_size(egui::vec2(look::WIDTH, look::HEIGHT)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), app);
    look::apply(&h.ctx);
    (h, o)
}

fn has(h: &Harness<'_, RecorderApp>, text: &str) -> bool {
    h.query_by_label_contains(text).is_some()
}

fn until(h: &mut Harness<'_, RecorderApp>, what: &str, done: impl Fn(&RecorderApp) -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !done(h.state()) {
        assert!(Instant::now() < end, "timed out: {what} (said {:?})", h.state().link.pair_status);
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.run_steps(1);
}

fn join_with_box(h: &mut Harness<'_, RecorderApp>, text: &str) {
    h.state_mut().studio_box = text.into();
    h.run_steps(1);
    h.get_by_label("Join").click();
    h.run_steps(1);
}

#[test]
fn joining_by_the_code_on_the_studio_screen() {
    let dir = temp_dir("studio-code");
    let hub = Hub::new(NetConfig { pc_name: "Studio A".into(), encrypted_only: false, ..NetConfig::default() }, None);
    let port = hub.listen(0).unwrap();
    let (mut h, _o) = harness(&dir, None);
    h.state_mut().go(Screen::Studio);
    h.state_mut().link.stop_finding();
    h.run_steps(1);
    // Typed by address (a studio not found by looking).
    join_with_box(&mut h, &format!("127.0.0.1:{port}"));
    until(&mut h, "the studio shows a code", |a| a.link.wants_code());
    assert!(has(&h, "Type the code showing on the studio's screen"));
    assert!(!h.state().link.pair_pin);
    let code = hub.pending_pairings().first().unwrap().code.clone();
    join_with_box(&mut h, &code);
    until(&mut h, "joined", |a| a.link.paired());
    assert!(has(&h, "Studio A"), "the studio's own name, once joined");
    assert!(h.query_by_label("Leave this studio").is_some());
    assert!(hub.config().paired.iter().any(|d| d.name == "Test Recorder"));
    // Leaving forgets it on this side.
    h.get_by_label("Leave this studio").click();
    h.run_steps(2);
    assert!(!h.state().link.paired());
    hub.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn joining_by_the_studio_pin_and_a_wrong_one_said_plainly() {
    let dir = temp_dir("studio-pin");
    let hub = Hub::new(NetConfig { pc_name: "Studio A".into(), encrypted_only: false, ..NetConfig::default() }, None);
    hub.set_booth_pin(PIN).unwrap();
    let port = hub.listen(0).unwrap();
    let (mut h, _o) = harness(&dir, None);
    h.state_mut().go(Screen::Studio);
    h.state_mut().link.stop_finding();
    join_with_box(&mut h, &format!("127.0.0.1:{port}"));
    until(&mut h, "the studio asks for its PIN", |a| a.link.wants_code());
    assert!(h.state().link.pair_pin);
    assert!(has(&h, "Type the studio's PIN"));
    join_with_box(&mut h, "000000");
    until(&mut h, "the wrong PIN refused", |a| !a.link.busy() && !a.link.wants_code());
    assert!(has(&h, "That didn't match. Check the code or PIN and type it again."), "{:?}", h.state().link.pair_status);
    assert!(!h.state().link.paired());
    // The PIN typed straight into the box: the studio asked before is asked again, the PIN goes
    // as soon as it answers.
    join_with_box(&mut h, PIN);
    until(&mut h, "joined with the PIN", |a| a.link.paired());
    assert!(hub.config().paired.iter().any(|d| d.name == "Test Recorder"));
    hub.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn joining_by_an_internet_invite_and_reconnecting_from_the_saved_settings() {
    let dir = temp_dir("studio-invite");
    let hub = Hub::new(NetConfig { pc_name: "Studio A".into(), ..NetConfig::default() }, None);
    let host = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    host.serve_to(hub.listen_internet().unwrap());
    let code = Invite { addr: host.addr(Duration::from_secs(1)), secret: hub.new_invite_secret(), name: "Studio A".into() }.code();
    let paths = Paths::in_dir(&dir.join("settings"));
    let (mut h, _o) = harness(&dir, Some(paths.clone()));
    h.state_mut().link.invite_relays = Relays::LocalOnly;
    h.state_mut().go(Screen::Studio);
    h.state_mut().link.stop_finding();
    // Pasted from a chat window, wrapped.
    let wrapped: String = code.chars().enumerate().flat_map(|(i, c)| if i % 40 == 39 { vec![c, '\n'] } else { vec![c] }).collect();
    join_with_box(&mut h, &wrapped);
    until(&mut h, "joined over the internet", |a| a.link.paired() && a.link.follower.as_ref().is_some_and(|f| f.connected()));
    assert!(has(&h, "Studio A"));
    let saved = NetConfig::load(&paths.network);
    assert!(!saved.follow.internet.is_empty() && !saved.follow.token.is_empty() && saved.follow.enabled);
    assert!(!saved.enabled && !saved.internet_host, "never a listener");
    h.state_mut().link.shutdown();
    drop(h);

    // Opened again later: it reconnects by the address it kept.
    let mut again = Link::with_relays(Some(paths.network.clone()), None, Some(Relays::LocalOnly));
    let end = Instant::now() + Duration::from_secs(20);
    while !again.follower.as_ref().is_some_and(|f| f.connected()) {
        assert!(Instant::now() < end, "reconnected ({:?})", again.error);
        std::thread::sleep(Duration::from_millis(20));
    }
    again.shutdown();

    // A damaged invite is said plainly.
    let mut l = Link::with_relays(None, None, Some(Relays::LocalOnly));
    l.join_invite(&code[..code.len() - 10]);
    assert!(soundcraft_recorder::words::studio_error(&l.pair_status).contains("Part of the invite is missing"), "{}", l.pair_status);
    hub.stop_internet();
    hub.stop();
    let _ = std::fs::remove_dir_all(dir);
}
