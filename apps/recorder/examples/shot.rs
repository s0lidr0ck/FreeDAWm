//! Renders the recorder's window offscreen in each of its screens and states, as PNGs (no
//! window, no devices):
//!
//! `cargo run -p soundcraft-recorder --example shot -- <out dir> [scale]`
//!
//! The devices are the tests' (`tests/common`): an input fed by hand, a MIDI keyboard that
//! exists only here. The studio's pairing prompts come from a real booth hub on this machine
//! (loopback), with and without a booth PIN; its other states are shown as the link would report
//! them. Also prints how long the first frame and the devices took (the test devices: real
//! ones take longer, but only the first frame comes before the window shows).

#[path = "../tests/common/mod.rs"]
mod common;

use common::{offline, ramp};
use egui_kittest::kittest::Queryable;
use soundcraft_net::config::NetConfig;
use soundcraft_net::discovery::Found;
use soundcraft_net::follower::StreamHealth;
use soundcraft_net::glue::FollowBadge;
use soundcraft_net::hub::Hub;
use soundcraft_recorder::booth::BoothView;
use soundcraft_recorder::rig::InputChoice;
use soundcraft_recorder::settings::Prefs;
use soundcraft_recorder::ui::{COUNT_STEP, RecorderApp, Screen, look};
use std::time::{Duration, Instant};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };

fn view(badge: FollowBadge, connected: bool, queued: usize, progress: Option<f64>, stream: Option<StreamHealth>) -> BoothView {
    BoothView {
        badge,
        connected,
        state: if connected { "connected".into() } else { "looking for the booth".into() },
        link: "encrypted local link",
        session: "Amen".into(),
        transport: "Stopped",
        position: 754.0,
        queued,
        progress,
        stream,
    }
}

type Harness = egui_kittest::Harness<'static, RecorderApp>;

struct Shots {
    out: std::path::PathBuf,
    scale: f32,
}

impl Shots {
    fn harness(&self, app: RecorderApp) -> Harness {
        let h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(look::WIDTH, look::HEIGHT))
            .with_pixels_per_point(self.scale)
            .wgpu()
            .build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), app);
        look::apply(&h.ctx);
        h
    }

    /// The window `height` points tall (the whole settings screen at once).
    fn tall(&self, app: RecorderApp, height: f32) -> Harness {
        let h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(look::WIDTH, height))
            .with_pixels_per_point(self.scale)
            .wgpu()
            .build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), app);
        look::apply(&h.ctx);
        h
    }

    fn save(&self, h: &mut Harness, name: &str) {
        h.run_steps(2);
        match h.render() {
            Ok(img) => {
                if let Err(e) = img.save(self.out.join(format!("{name}.png"))) {
                    eprintln!("{name}: {e}");
                }
            }
            Err(e) => eprintln!("{name}: render failed: {e}"),
        }
    }
}

/// An app on the test devices (an input and, with `keys`, a keyboard), its ideas in `ideas`.
fn app(keys: bool, ideas: &std::path::Path, extra: impl FnOnce(&mut Prefs)) -> (RecorderApp, common::Offline) {
    let ports: &[&str] = if keys { &["Keystation 49"] } else { &[] };
    let mut o = offline(1, ports, MONO);
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    let mut prefs = Prefs { ideas_folder: ideas.to_string_lossy().into_owned(), ..Prefs::default() };
    extra(&mut prefs);
    (RecorderApp::with_rig(rig, prefs, None), o)
}

/// Frames while the device runs (`secs`), loud enough to see.
fn run(h: &mut Harness, o: &mut common::Offline, secs: f64, loud: f32) {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut k = 0usize;
    while Instant::now() < end {
        for _ in 0..4 {
            // A phrase: louder and softer, so the bars and the shape move.
            o.block(&|i, c| {
                let env = 0.25 + 0.75 * ((i as f32 / 9000.0).sin() * 0.5 + 0.5);
                loud * env * ramp(i, c) * 2.0
            });
        }
        k += 1;
        h.step();
        let _ = k;
    }
}

/// Two ideas in `dir`: one named by hand today, one the Recorder named yesterday.
fn two_ideas(dir: &std::path::Path) {
    let mut o = offline(1, &["Keystation 49"], MONO);
    o.rig.start(dir, "Chorus idea", false, 0).unwrap();
    o.run(1.0, &|i, c| 0.5 * ramp(i, c));
    o.keys.send("Keystation 49", &[0x90, 60, 100]);
    o.run(0.2, &ramp);
    o.keys.send("Keystation 49", &[0x80, 60, 0]);
    o.rig.stop();
    // Yesterday evening, local time.
    let y = soundcraft_recorder::settings::local_stamp(soundcraft_recorder::settings::now_secs() - 86_400);
    let stem = format!("Idea {}", soundcraft_recorder::settings::stamp_text((y.0, y.1, y.2, 21, 41, 0)));
    o.rig.start(dir, &stem, false, 0).unwrap();
    o.run(0.6, &|i, c| 0.3 * ramp(i, c));
    o.rig.stop();
    // The list is newest first: yesterday's file is made older.
    if let Ok(f) = std::fs::File::options().write(true).open(dir.join(format!("{stem}.wav"))) {
        let _ = f.set_modified(std::time::SystemTime::now() - Duration::from_secs(86_400));
    }
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "recorder-shots".into()));
    let scale: f32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1.5);
    std::fs::create_dir_all(&out).unwrap();
    let shots = Shots { out: out.clone(), scale };
    let root = std::env::temp_dir().join(format!("freedawm-recorder-shots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ideas = root.join("FreeDAWm Ideas");
    std::fs::create_dir_all(&ideas).unwrap();
    two_ideas(&ideas);

    // 1: ready, two ideas.
    let (a, mut o) = app(true, &ideas, |_| {});
    let mut h = shots.harness(a);
    run(&mut h, &mut o, 0.3, 0.35);
    shots.save(&mut h, "1-ready");
    // 1, Record pressed before the devices were ready; the devices still opening.
    let t0 = Instant::now();
    let mut h = shots.harness(RecorderApp::without_rig(Prefs { ideas_folder: ideas.to_string_lossy().into_owned(), ..Prefs::default() }));
    h.run_steps(1);
    println!("first frame: {:.1} ms after the app was made", t0.elapsed().as_secs_f64() * 1000.0);
    shots.save(&mut h, "1-looking-for-microphone");
    h.get_by_label("Record").click();
    shots.save(&mut h, "1-record-pressed-early");
    // 1, no devices at all.
    let empty = root.join("empty");
    let mut o0 = offline(0, &[], MONO);
    let rig = std::mem::replace(&mut o0.rig, offline(0, &[], MONO).rig);
    let mut h = shots.harness(RecorderApp::with_rig(rig, Prefs { ideas_folder: empty.to_string_lossy().into_owned(), ..Prefs::default() }, None));
    shots.save(&mut h, "1-no-microphone");

    // 2: recording.
    let (a, mut o) = app(true, &ideas, |_| {});
    let mut h = shots.harness(a);
    h.state_mut().toggle_record();
    run(&mut h, &mut o, 0.4, 0.6);
    for n in [60u8, 64, 67] {
        o.keys.send("Keystation 49", &[0x90, n, 100]);
        run(&mut h, &mut o, 0.15, 0.6);
        o.keys.send("Keystation 49", &[0x80, n, 0]);
    }
    run(&mut h, &mut o, 0.5, 0.6);
    shots.save(&mut h, "2-recording");

    // 3: saved.
    h.state_mut().toggle_record();
    shots.save(&mut h, "3-saved");
    h.get_by_label("Throw away").click();
    shots.save(&mut h, "3-throw-away-asks");
    if let Some(i) = &mut h.state_mut().idea {
        i.confirm_throw = false;
        i.name = "Chorus idea".into();
        i.name_error = Some("Another idea is already called “Chorus idea”".into());
    }
    shots.save(&mut h, "3-name-taken");
    // 3, opened from the list.
    h.state_mut().open_idea("Chorus idea");
    shots.save(&mut h, "3-idea-from-list");
    // 3, the input unplugged mid-take.
    let (a, mut o) = app(false, &ideas, |_| {});
    let mut h = shots.harness(a);
    h.state_mut().toggle_record();
    run(&mut h, &mut o, 0.5, 0.5);
    o.input.as_ref().unwrap().unplug();
    let end = Instant::now() + Duration::from_secs(5);
    while h.state().recording() && Instant::now() < end {
        h.step();
    }
    shots.save(&mut h, "3-saved-microphone-unplugged");
    // The list after: no microphone, a keyboard only.
    h.state_mut().go(Screen::Main);
    shots.save(&mut h, "1-after-unplug");

    // Settings.
    let (a, _o) = app(true, &ideas, |_| {});
    let mut h = shots.harness(a);
    h.state_mut().go(Screen::Settings);
    shots.save(&mut h, "settings");

    // 4: the studio screen, looking, found, the code, the PIN, a wrong PIN.
    let (a, _o) = app(true, &ideas, |_| {});
    let mut h = shots.harness(a);
    h.state_mut().go(Screen::Studio);
    h.state_mut().link.stop_finding();
    shots.save(&mut h, "4-studio-looking");
    h.state_mut().link.demo_found = Some(vec![Found { name: "Studio A".into(), addr: "127.0.0.1:1".into(), role: "booth".into(), lan: None }]);
    shots.save(&mut h, "4-studio-found");
    for (pin, name) in [(None, "4-studio-code"), (Some("482113"), "4-studio-pin")] {
        let hub = Hub::new(NetConfig { pc_name: "Studio A".into(), encrypted_only: false, ..NetConfig::default() }, None);
        if let Some(p) = pin {
            hub.set_booth_pin(p).unwrap();
        }
        let port = hub.listen(0).unwrap();
        let (a, _o) = app(true, &ideas, |_| {});
        let mut h = shots.harness(a);
        h.state_mut().go(Screen::Studio);
        h.state_mut().link.stop_finding();
        h.state_mut().link.demo_found =
            Some(vec![Found { name: "Studio A".into(), addr: format!("127.0.0.1:{port}"), role: "booth".into(), lan: None }]);
        h.run_steps(1);
        h.get_by_label("Join Studio A").click();
        let end = Instant::now() + Duration::from_secs(10);
        while !h.state().link.wants_code() && Instant::now() < end {
            h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        shots.save(&mut h, name);
        if pin.is_some() {
            h.state_mut().studio_box = "000000".into();
            h.get_by_label("Join").click();
            let end = Instant::now() + Duration::from_secs(10);
            while (h.state().link.wants_code() || h.state().link.busy()) && Instant::now() < end {
                h.step();
                std::thread::sleep(Duration::from_millis(10));
            }
            shots.save(&mut h, "4-studio-wrong-pin");
        }
        hub.stop();
    }
    // 4, joined: waiting, sending, away; the main screen while joined.
    let health = StreamHealth { buffer_ms: 80.0, lost: 0, late: 0, packets: 4210, starved_ms: 0.0, heard_pos: Some(754.0) };
    for (badge, connected, queued, progress, name) in [
        (FollowBadge::Following("Studio A".into()), true, 0, None, "4-joined-waiting"),
        (FollowBadge::Sending("Studio A".into(), Some(40)), true, 1, Some(0.4), "4-joined-sending"),
        (FollowBadge::Away("Studio A".into()), false, 2, None, "4-joined-studio-away"),
    ] {
        let (a, mut o) = app(true, &ideas, |p| p.listen = true);
        let mut h = shots.harness(a);
        {
            let s = h.state_mut();
            s.link.cfg.follow.token = "paired".into();
            s.link.cfg.follow.booth_name = "Studio A".into();
            s.link.demo = Some(view(badge, connected, queued, progress, Some(health.clone())));
            s.go(Screen::Studio);
        }
        run(&mut h, &mut o, 0.2, 0.3);
        shots.save(&mut h, name);
        if name == "4-joined-waiting" {
            h.state_mut().go(Screen::Main);
            shots.save(&mut h, "1-ready-joined");
        }
    }

    // Layers: Add a layer, the 3-2-1, recording a layer, the layers saved, one asking to go,
    // the list, their settings.
    let (a, mut o) = app(true, &ideas, |_| {});
    let mut h = shots.harness(a);
    h.state_mut().open_idea("Chorus idea");
    shots.save(&mut h, "5-idea-add-a-layer");
    h.state_mut().add_layer();
    shots.save(&mut h, "5-layer-count-in");
    run(&mut h, &mut o, 3.0 * COUNT_STEP + 0.1, 0.5);
    run(&mut h, &mut o, 1.6, 0.5);
    shots.save(&mut h, "5-layer-recording");
    h.state_mut().toggle_record();
    shots.save(&mut h, "5-layers-saved");
    h.state_mut().set_muted(2, true);
    if let Some(i) = &mut h.state_mut().idea {
        i.confirm_layer = Some(3);
    }
    shots.save(&mut h, "5-layer-throw-asks");
    h.state_mut().go(Screen::Main);
    shots.save(&mut h, "5-list-with-layers");
    let (a, _o) = app(true, &ideas, |p| p.layer_nudge_ms = 5.0);
    let mut h = shots.tall(a, 1500.0);
    h.state_mut().go(Screen::Settings);
    h.state_mut().line_up_note = Some(("Lined up. Sound takes 12 ms to go out and come back in on this computer.".into(), false));
    shots.save(&mut h, "settings-full");
    // The keyboard's sound: the card on the ready screen, its screen (a preset with named knobs),
    // all of it, and off.
    let (a, mut o) = app(true, &ideas, |p| p.keys = soundcraft_recorder::keys::KeySound::from_preset("subtractive_synth", "Breathing Pad"));
    let mut h = shots.harness(a);
    run(&mut h, &mut o, 0.2, 0.3);
    shots.save(&mut h, "1-ready-keyboard-sound");
    h.state_mut().go(Screen::Keys);
    shots.save(&mut h, "6-keyboard-sound");
    let (a, _o) = app(true, &ideas, |p| p.keys = soundcraft_recorder::keys::KeySound::from_preset("subtractive_synth", "Breathing Pad"));
    let mut h = shots.tall(a, 1500.0);
    h.state_mut().go(Screen::Keys);
    shots.save(&mut h, "6-keyboard-sound-full");
    let (a, _o) = app(true, &ideas, |p| p.keys_on = false);
    let mut h = shots.harness(a);
    h.state_mut().go(Screen::Keys);
    shots.save(&mut h, "6-keyboard-sound-off");

    // The click: its settings, the bar before a take.
    let (a, _o) = app(true, &ideas, |p| (p.click_on, p.click_bpm, p.click_beats) = (true, 96.0, 4));
    let mut h = shots.tall(a, 1700.0);
    h.state_mut().go(Screen::Settings);
    shots.save(&mut h, "settings-click");
    let (a, mut o) = app(true, &ideas, |p| (p.click_on, p.click_bpm) = (true, 96.0));
    let mut h = shots.harness(a);
    h.state_mut().toggle_record();
    run(&mut h, &mut o, 1.0, 0.0);
    shots.save(&mut h, "2-click-count-in");

    let _ = std::fs::remove_dir_all(&root);
    println!("shots in {}", out.display());
}
