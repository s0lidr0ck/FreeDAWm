//! Renders the recorder's window offscreen in its main states, as PNGs (no window, no devices):
//!
//! `cargo run -p soundcraft-recorder --example shot -- <out dir>`
//!
//! Writes `no-devices.png`, `idle.png`, `recording.png`, `booth-away.png`, `sending.png` and
//! `listening.png`. The devices are the tests' (`tests/common`): an input fed by hand, a MIDI
//! port that exists only here; the booth's states are shown as the link would report them.

#[path = "../tests/common/mod.rs"]
mod common;

use common::{offline, ramp};
use soundcraft_net::follower::StreamHealth;
use soundcraft_net::glue::FollowBadge;
use soundcraft_recorder::booth::BoothView;
use soundcraft_recorder::rig::InputChoice;
use soundcraft_recorder::settings::Prefs;
use soundcraft_recorder::ui::{RecorderApp, look};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };

fn view(
    badge: FollowBadge,
    connected: bool,
    transport: &'static str,
    queued: usize,
    progress: Option<f64>,
    stream: Option<StreamHealth>,
) -> BoothView {
    BoothView {
        badge,
        connected,
        state: if connected { "connected".into() } else { "looking for the booth".into() },
        link: "encrypted local link",
        session: "Sunday Service".into(),
        transport,
        position: 754.0,
        queued,
        progress,
        stream,
    }
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "recorder-shots".into()));
    std::fs::create_dir_all(&out).unwrap();
    let ideas = std::env::temp_dir().join(format!("freedawm-recorder-shots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ideas);
    std::fs::create_dir_all(&ideas).unwrap();
    let prefs = Prefs { ideas_folder: ideas.to_string_lossy().into_owned(), ..Prefs::default() };

    let shoot = |mut o: common::Offline, prefs: Prefs, name: &str, setup: &dyn Fn(&mut RecorderApp, &mut common::Offline)| {
        let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
        let mut app = RecorderApp::with_rig(rig, prefs, None);
        setup(&mut app, &mut o);
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(520.0, 700.0))
            .with_pixels_per_point(1.5)
            .wgpu()
            .build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), app);
        look::apply(&h.ctx);
        for _ in 0..8 {
            o.block(&|i, c| 0.5 * ramp(i, c));
            h.run_steps(1);
        }
        match h.render() {
            Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
            Err(e) => eprintln!("{name}: render failed: {e}"),
        }
    };

    shoot(offline(0, &[], MONO), prefs.clone(), "no-devices", &|_, _| {});

    // Two takes in the folder.
    let mut o = offline(1, &["Keystation 49"], MONO);
    o.rig.start(&ideas, "Idea 2026-11-22 14-03-10", false, 0).unwrap();
    o.run(1.2, &|i, c| 0.5 * ramp(i, c));
    o.keys.send("Keystation 49", &[0x90, 60, 100]);
    o.run(0.2, &ramp);
    o.keys.send("Keystation 49", &[0x80, 60, 0]);
    o.rig.stop();
    o.rig.start(&ideas, "Chorus idea", false, 0).unwrap();
    o.run(0.6, &|i, c| 0.5 * ramp(i, c));
    o.rig.stop();
    shoot(o, prefs.clone(), "idle", &|_, _| {});

    let o = offline(1, &["Keystation 49"], MONO);
    shoot(o, prefs.clone(), "recording", &|app, o| {
        app.toggle_record();
        o.run(0.4, &|i, c| 0.5 * ramp(i, c));
    });

    let o = offline(1, &["Keystation 49"], MONO);
    shoot(o, Prefs { listen: true, ..prefs.clone() }, "booth-away", &|app, _| {
        app.link.cfg.follow.token = "paired".into();
        app.link.cfg.follow.booth_name = "Booth".into();
        app.link.cfg.follow.enabled = true;
        app.link.demo = Some(view(FollowBadge::Away("Booth".into()), false, "Stopped", 1, None, None));
    });

    let o = offline(1, &["Keystation 49"], MONO);
    shoot(o, prefs.clone(), "sending", &|app, _| {
        app.link.cfg.follow.token = "paired".into();
        app.link.cfg.follow.booth_name = "Booth".into();
        app.link.cfg.follow.enabled = true;
        app.link.demo = Some(view(FollowBadge::Sending("Booth".into(), Some(45)), true, "Stopped", 1, Some(0.45), None));
    });

    let o = offline(1, &["Keystation 49"], MONO);
    let health = StreamHealth { buffer_ms: 80.0, lost: 0, late: 1, packets: 4210, starved_ms: 0.0, heard_pos: Some(754.0) };
    shoot(o, Prefs { listen: true, ..prefs.clone() }, "listening", &|app, _| {
        app.link.cfg.follow.token = "paired".into();
        app.link.cfg.follow.booth_name = "Booth".into();
        app.link.cfg.follow.enabled = true;
        app.link.demo = Some(view(FollowBadge::Following("Booth".into()), true, "Playing", 0, None, Some(health.clone())));
    });
    let _ = std::fs::remove_dir_all(&ideas);
    println!("shots in {}", out.display());
}
