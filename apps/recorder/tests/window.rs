//! The window's screens and states, drawn headless: ready, no devices, the devices still opening,
//! recording, saved (named, played, thrown away), an input unplugged mid-take, Space, the
//! studio screen's states, the settings, the place it remembers.

mod common;

use common::{offline, ramp, temp_dir};
use egui_kittest::{Harness, kittest::Queryable};
use soundcraft_net::discovery::Found;
use soundcraft_net::glue::FollowBadge;
use soundcraft_recorder::booth::BoothView;
use soundcraft_recorder::rig::InputChoice;
use soundcraft_recorder::settings::{Paths, Prefs};
use soundcraft_recorder::ui::{RENAME_AFTER, RecorderApp, Screen, look};
use std::time::{Duration, Instant};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };

fn prefs(ideas: &std::path::Path) -> Prefs {
    Prefs { ideas_folder: ideas.to_string_lossy().into_owned(), ..Prefs::default() }
}

fn app(channels: usize, ports: &[&str], ideas: &std::path::Path) -> (RecorderApp, common::Offline) {
    let mut o = offline(channels, ports, MONO);
    // The rig moves into the app; the test keeps the devices' other ends.
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    (RecorderApp::with_rig(rig, prefs(ideas), None), o)
}

fn harness(app: RecorderApp) -> Harness<'static, RecorderApp> {
    let h = Harness::builder().with_size(egui::vec2(look::WIDTH, look::HEIGHT)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), app);
    look::apply(&h.ctx);
    h
}

fn has(h: &Harness<'_, RecorderApp>, text: &str) -> bool {
    h.query_by_label_contains(text).is_some()
}

/// Frames while the test's device runs (`secs` of it).
fn play(h: &mut Harness<'_, RecorderApp>, o: &mut common::Offline, secs: f64) {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        for _ in 0..4 {
            o.block(&|i, c| 0.5 * ramp(i, c));
        }
        h.step();
    }
}

#[test]
fn no_devices_says_what_to_do_and_record_is_off() {
    let dir = temp_dir("ui-none");
    let (a, _o) = app(0, &[], &dir);
    let mut h = harness(a);
    h.run_steps(3);
    assert!(has(&h, "No microphone found"));
    assert!(has(&h, "Plug in a microphone, a headset or a keyboard, then press Try again."));
    assert!(h.query_by_label("Try again").is_some());
    assert!(has(&h, "Nothing to record from yet"));
    // Record is greyed: pressing it (or Space) does nothing.
    h.get_by_label("Record").click();
    h.run_steps(2);
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    assert!(!h.state().recording());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ready_then_recording_then_saved() {
    let dir = temp_dir("ui-rec");
    let (a, mut o) = app(1, &["Keystation 49"], &dir);
    let mut h = harness(a);
    h.run_steps(3);
    // Screen 1.
    assert!(has(&h, "FreeDAWm Recorder"));
    assert!(h.query_by_label("Settings").is_some(), "the gear");
    assert!(has(&h, "Click, or press Space"));
    assert!(has(&h, "Recording from: Synthetic input + Piano keyboard"));
    assert!(h.query_by_label("Change").is_some());
    assert!(has(&h, "YOUR IDEAS") && has(&h, "No ideas yet"));
    assert!(h.query_by_label("Record with a studio").is_some());
    // Screen 2.
    h.get_by_label("Record").click();
    h.run_steps(2);
    assert!(h.state().recording());
    assert!(h.query_by_label("Stop").is_some());
    assert!(has(&h, "Recording") && has(&h, "Sound coming in"));
    play(&mut h, &mut o, 0.3);
    o.keys.send("Keystation 49", &[0x90, 60, 100]);
    play(&mut h, &mut o, 0.2);
    o.keys.send("Keystation 49", &[0x80, 60, 0]);
    play(&mut h, &mut o, 0.3);
    assert!(has(&h, "1 piano note so far · saved as you go"), "the notes are counted as they come");
    assert!(has(&h, "Recorded 0:0"), "the clock");
    // Screen 3.
    h.get_by_label("Stop").click();
    h.run_steps(3);
    assert!(!h.state().recording());
    assert_eq!(h.state().screen, Screen::Idea);
    assert!(has(&h, "Saved · 0:0"));
    assert!(has(&h, "Name it (optional)"));
    assert!(h.query_by_label("Play this idea").is_some());
    assert!(has(&h, "Your voice, plus the piano notes you played (1 note)"));
    assert!(h.query_by_label("Record another").is_some());
    assert!(h.query_by_label("FreeDAWm not found").is_some(), "no FreeDAWm beside the test: the button says so");
    assert!(h.query_by_label("Show the file").is_some());
    assert!(has(&h, "Kept in "));
    assert!(h.query_by_label("All my ideas").is_some() && h.query_by_label("Throw away").is_some());
    let idea = h.state().idea.clone().unwrap();
    assert!(idea.name.starts_with("Idea · ") && idea.name.ends_with('m'), "the name box starts with the friendly name: {}", idea.name);
    assert!(idea.take.stem.starts_with("Idea 20"), "the file keeps a safe name: {}", idea.take.stem);
    assert!(!idea.take.stem.contains(':'));
    assert_eq!(idea.shape.len(), 22, "the take's shape, from its file");
    assert!(idea.shape.iter().any(|p| *p > 0.1), "{:?}", idea.shape);
    // All my ideas: the take is in the list.
    h.get_by_label("All my ideas").click();
    h.run_steps(3);
    assert_eq!(h.state().screen, Screen::Main);
    assert!(has(&h, "Today · 0:0") && has(&h, "voice + piano notes"), "listed with when and what");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn space_records_and_stops_but_not_while_typing_a_name() {
    let dir = temp_dir("ui-space");
    let (a, mut o) = app(1, &[], &dir);
    let mut h = harness(a);
    h.run_steps(2);
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    assert!(h.state().recording(), "Space records");
    play(&mut h, &mut o, 0.3);
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    assert!(!h.state().recording(), "Space stops");
    assert_eq!(h.state().screen, Screen::Idea);
    // In the name box Space is a space.
    h.get_by_role(egui::accesskit::Role::TextInput).focus();
    h.run_steps(1);
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    assert!(!h.state().recording(), "typing a space in the name does not record");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn typing_a_name_renames_the_files_and_enter_applies_it() {
    let dir = temp_dir("ui-name");
    let (a, mut o) = app(1, &["Keys"], &dir);
    let mut h = harness(a);
    h.run_steps(2);
    h.state_mut().toggle_record();
    play(&mut h, &mut o, 0.2);
    o.keys.send("Keys", &[0x90, 64, 90]);
    play(&mut h, &mut o, 0.2);
    h.state_mut().toggle_record();
    h.run_steps(2);
    let first = h.state().idea.clone().unwrap();
    assert!(first.take.mid.is_some());
    // Typed, then a pause: the files take the name.
    let field = h.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    h.run_steps(1);
    h.key_combination_modifiers(egui::Modifiers::COMMAND, &[egui::Key::A]);
    h.get_by_role(egui::accesskit::Role::TextInput).type_text("Chorus idea");
    h.run_steps(1);
    assert!(dir.join(format!("{}.wav", first.take.stem)).exists(), "not while still typing");
    let steps = (RENAME_AFTER / 0.25).ceil() as usize + 4;
    h.run_steps(steps);
    assert!(dir.join("Chorus idea.wav").exists() && dir.join("Chorus idea.mid").exists(), "renamed after the pause");
    assert!(!dir.join(format!("{}.wav", first.take.stem)).exists());
    // A name with characters Windows refuses: the file name is made safe, the box keeps it.
    h.key_combination_modifiers(egui::Modifiers::COMMAND, &[egui::Key::A]);
    h.get_by_role(egui::accesskit::Role::TextInput).type_text("Verse: take 2?");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(
        dir.join("Verse- take 2-.wav").exists(),
        "Enter applies at once: {:?}",
        std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect::<Vec<_>>()
    );
    // The default name typed back: the Recorder's own name again.
    h.state_mut().idea.as_mut().unwrap().name = first.default_name.clone();
    h.state_mut().apply_name();
    assert!(dir.join(format!("{}.wav", first.take.stem)).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn throw_away_asks_first() {
    let dir = temp_dir("ui-throw");
    let (a, mut o) = app(1, &[], &dir);
    let mut h = harness(a);
    h.run_steps(2);
    h.state_mut().toggle_record();
    play(&mut h, &mut o, 0.2);
    h.state_mut().toggle_record();
    h.run_steps(2);
    let wav = h.state().idea.clone().unwrap().take.wav.unwrap();
    h.get_by_label("Throw away").click();
    h.run_steps(2);
    assert!(has(&h, "Throw this idea away? It can't be undone."));
    assert!(wav.exists(), "nothing is thrown away yet");
    h.get_by_label("Keep it").click();
    h.run_steps(2);
    assert!(!has(&h, "Throw this idea away?"));
    h.get_by_label("Throw away").click();
    h.run_steps(2);
    h.get_by_label("Yes, throw it away").click();
    h.run_steps(2);
    assert!(!wav.exists());
    assert_eq!(h.state().screen, Screen::Main);
    assert!(has(&h, "Thrown away."));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_input_unplugged_mid_take_keeps_the_take_and_says_so() {
    let dir = temp_dir("ui-unplug");
    let (a, mut o) = app(1, &[], &dir);
    let mut h = harness(a);
    h.run_steps(2);
    h.state_mut().toggle_record();
    play(&mut h, &mut o, 0.3);
    o.input.as_ref().unwrap().unplug();
    let end = Instant::now() + Duration::from_secs(5);
    while h.state().recording() {
        assert!(Instant::now() < end);
        h.step();
    }
    h.run_steps(2);
    assert_eq!(h.state().screen, Screen::Idea);
    assert!(has(&h, "Your microphone stopped (unplugged?). Everything up to then is saved."));
    assert!(h.state().idea.as_ref().unwrap().take.wav.as_ref().is_some_and(|w| w.exists()), "the take is kept");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_window_paints_before_the_devices_open_and_record_waits_for_them() {
    let dir = temp_dir("ui-early");
    let t0 = Instant::now();
    let mut h = harness(RecorderApp::without_rig(prefs(&dir)));
    h.run_steps(1);
    let first_paint = t0.elapsed();
    eprintln!("first frame drawn {:.1} ms after the app was made", first_paint.as_secs_f64() * 1000.0);
    assert!(has(&h, "Looking for your microphone…"));
    assert!(!h.state().ready());
    // Record pressed before the devices are ready: it says so, and starts when they are.
    h.get_by_label("Record").click();
    h.run_steps(2);
    assert!(has(&h, "Starting as soon as your microphone is ready…"));
    assert!(!h.state().recording());
    let mut o = offline(1, &[], MONO);
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    h.state_mut().set_rig(rig);
    h.run_steps(2);
    assert!(h.state().recording(), "recording the moment the devices are ready");
    let ready = h.state().ready_in.unwrap();
    eprintln!("devices ready {:.1} ms after the app was made (test devices)", ready.as_secs_f64() * 1000.0);
    h.state_mut().toggle_record();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_setting_starts_recording_when_it_opens() {
    let dir = temp_dir("ui-auto");
    let mut o = offline(1, &[], MONO);
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    let a = RecorderApp::with_rig(rig, Prefs { record_on_open: true, ..prefs(&dir) }, None);
    let mut h = harness(a);
    h.run_steps(2);
    assert!(h.state().recording());
    h.state_mut().toggle_record();
    // Off by default.
    assert!(!Prefs::default().record_on_open);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_settings_say_what_it_records_from_and_where() {
    let dir = temp_dir("ui-settings");
    let (a, _o) = app(2, &["Keys"], &dir);
    let mut h = harness(a);
    h.run_steps(2);
    h.get_by_label("Change").click();
    h.run_steps(2);
    assert_eq!(h.state().screen, Screen::Settings);
    for t in [
        "MICROPHONE OR INPUT",
        "KEYBOARDS",
        "WHERE IDEAS ARE KEPT",
        "PLAY BACK THROUGH",
        "Start recording as soon as the Recorder opens",
        "Not joined to a studio",
    ] {
        assert!(has(&h, t), "{t}");
    }
    h.get_by_label("Start recording as soon as the Recorder opens").click();
    h.run_steps(1);
    assert!(h.state().prefs.record_on_open);
    h.get_by_label("Keys").click();
    h.run_steps(1);
    assert_eq!(h.state().rig.as_ref().unwrap().midi_ports(), vec![("Keys".to_string(), false)]);
    // Escape goes back.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().screen, Screen::Main);
    let _ = std::fs::remove_dir_all(dir);
}

fn view(badge: FollowBadge, connected: bool, queued: usize, progress: Option<f64>) -> BoothView {
    BoothView {
        badge,
        connected,
        state: if connected { "connected".into() } else { "looking for the booth".into() },
        link: "encrypted local link",
        session: "Amen".into(),
        transport: "Stopped",
        position: 0.0,
        queued,
        progress,
        stream: None,
    }
}

#[test]
fn the_studio_screen_finds_joins_and_follows() {
    let dir = temp_dir("ui-studio");
    let (mut a, _o) = app(1, &[], &dir);
    a.link.demo_found = Some(vec![Found { name: "Studio A".into(), addr: "127.0.0.1:1".into(), role: "booth".into(), lan: None }]);
    let mut h = harness(a);
    h.run_steps(2);
    h.get_by_label("Record with a studio").click();
    h.run_steps(2);
    assert_eq!(h.state().screen, Screen::Studio);
    assert!(h.state().link.finding(), "looking for studios since the screen opened");
    assert!(has(&h, "When the studio presses Record, you record too."));
    assert!(has(&h, "Found on this network"));
    assert!(h.query_by_label("Join Studio A").is_some());
    assert!(has(&h, "Somewhere else? Paste the PIN or invite they sent you"));
    assert!(has(&h, "AFTER YOU JOIN") && has(&h, "Waiting for the studio to press Record") && has(&h, "Hear the studio's mix"));
    // Not a PIN, not an invite.
    h.state_mut().studio_box = "hello".into();
    h.get_by_label("Join").click();
    h.run_steps(2);
    assert!(has(&h, "That isn't a PIN or an invite."));
    // Escape backs out, and the looking stops.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().screen, Screen::Main);
    assert!(!h.state().link.finding(), "nothing more is sent once the screen closes");

    // Joined: waiting, sending, sent, away.
    {
        let s = h.state_mut();
        s.link.cfg.follow.token = "paired".into();
        s.link.cfg.follow.booth_name = "Studio A".into();
        s.link.demo = Some(view(FollowBadge::Following("Studio A".into()), true, 0, None));
    }
    h.run_steps(2);
    assert!(h.query_by_label("Studio: Studio A").is_some(), "the main screen says which studio");
    h.state_mut().go(Screen::Studio);
    h.run_steps(2);
    assert!(has(&h, "Studio A · song “Amen”") && has(&h, "Waiting for the studio to press Record"));
    assert!(h.query_by_label("Leave this studio").is_some());
    h.state_mut().link.demo = Some(view(FollowBadge::Sending("Studio A".into(), Some(40)), true, 1, Some(0.4)));
    h.run_steps(2);
    assert!(has(&h, "Sending your take… 40%"));
    h.state_mut().link.demo = Some(view(FollowBadge::Sent("Studio A".into()), true, 0, None));
    h.run_steps(2);
    assert!(has(&h, "Sent"));
    h.state_mut().link.demo = Some(view(FollowBadge::Away("Studio A".into()), false, 2, None));
    h.run_steps(2);
    assert!(has(&h, "The studio is away. Waiting for it…"));
    assert!(has(&h, "2 takes are kept here until the studio has them."));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_window_remembers_its_place() {
    let dir = temp_dir("ui-place");
    let paths = Paths::in_dir(&dir.join("settings"));
    let mut o = offline(1, &[], MONO);
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    let mut a = RecorderApp::with_rig(rig, prefs(&dir), Some(paths.clone()));
    a.window = Some(([140.0, 90.0], [500.0, 700.0]));
    a.shutdown();
    let p = Prefs::load(&paths.prefs);
    assert_eq!(p.window_pos, Some([140.0, 90.0]));
    assert_eq!(p.window_size, Some([500.0, 700.0]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_wider_window_keeps_the_column_centred() {
    let dir = temp_dir("ui-wide");
    let (a, _o) = app(1, &[], &dir);
    let mut h = Harness::builder().with_size(egui::vec2(900.0, 700.0)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), a);
    look::apply(&h.ctx);
    h.run_steps(2);
    let r = h.get_by_label("Record").rect();
    assert!((r.center().x - 450.0).abs() < 1.0, "{r:?}");
    let s = h.get_by_label("Record with a studio").rect();
    assert!((s.width() - (look::WIDTH - 2.0 * look::PAD_X)).abs() < 1.0, "the column keeps the design's width: {s:?}");
    let _ = std::fs::remove_dir_all(dir);
}
