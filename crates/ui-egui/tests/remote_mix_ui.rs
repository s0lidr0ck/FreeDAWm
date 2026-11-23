//! File › Connect to a Studio… and the Remote Mix window: the dialog, the banner that says whose
//! mixer this is, the greyed reconnecting state, and a real connection to an in-process studio
//! whose strips show here.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_net::config::{KnownStudio, NetConfig};
use soundcraft_net::hub::{Hub, HubRequest};
use soundcraft_ui_egui::{Services, SoundApp, studio_ui};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1500.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

/// The app with its network state made (a frame run); `None` where the network cannot start.
fn app_with_net() -> Option<Harness<'static, SoundApp>> {
    let app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let mut h = harness(app);
    h.run_steps(2);
    if h.state().net.is_none() {
        eprintln!("SKIPPED: no network state");
        return None;
    }
    Some(h)
}

#[test]
fn the_dialog_offers_studios_found_known_and_the_box() {
    let Some(mut h) = app_with_net() else { return };
    h.state_mut().run("window.connect_studio", json!({})).unwrap();
    h.run_steps(3);
    assert!(h.state().ui.show_connect_studio);
    for text in ["Studios on this network", "Studios you have mixed before", "Paste the PIN, code or invite", "only dials out"] {
        assert!(h.query_by_label_contains(text).is_some(), "{text}");
    }
    // A studio mixed before: one click reconnects.
    studio_ui::demo(h.state_mut(), "dialog").unwrap();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Studio PC").is_some());
    assert!(h.query_by_label("Reconnect").is_some());
    // A code with no studio picked: the dialog says what to do.
    h.state_mut().net.as_mut().unwrap().studio.entry = "4821".into();
    h.run_steps(1);
    h.get_by_label("Connect").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Pick a studio found on this network first").is_some());
    // The local session is untouched by all this.
    assert!(!h.state().engine.is_dirty());
}

#[test]
fn the_remote_window_says_whose_mixer_it_is_and_draws_its_strips() {
    let Some(mut h) = app_with_net() else { return };
    studio_ui::demo(h.state_mut(), "connected").unwrap();
    h.run_steps(4);
    assert!(h.query_by_label_contains("Mixing Studio PC remotely · Sunday Service Live").is_some(), "the banner");
    assert!(h.query_by_label_contains("Round trip 38 ms").is_some(), "the link's quality");
    assert!(h.query_by_label("Disconnect").is_some());
    for strip in ["Vocal", "Guitar", "Keys", "Pastor Mic"] {
        assert!(h.query_by_label(&format!("Mute {strip}")).is_some(), "{strip}'s strip");
    }
    // Disconnect: the window goes, the studio's mix as it was (nothing sent).
    h.get_by_label("Disconnect").click();
    h.run_steps(3);
    assert!(!h.state().net.as_ref().unwrap().studio.active());
    assert!(h.query_by_label_contains("Mixing Studio PC remotely").is_none());
}

#[test]
fn a_lost_link_greys_the_window_keeps_the_last_state_and_says_reconnecting() {
    let Some(mut h) = app_with_net() else { return };
    studio_ui::demo(h.state_mut(), "reconnecting").unwrap();
    h.run_steps(4);
    assert!(h.query_by_label_contains("Reconnecting…").is_some());
    assert!(h.query_by_label_contains("2 changes were not sent").is_some());
    // The last state stays visible, its controls greyed.
    let mute = h.get_by_label("Mute Vocal");
    assert!(mute.accesskit_node().is_disabled(), "greyed while the link is down");
}

/// An in-process studio: a hub on loopback taking PCs over the plain link, an engine served by a
/// loop as the studio's UI thread does it. Its token for "Home PC".
struct Studio {
    hub: Hub,
    stop: Arc<AtomicBool>,
    engine: Arc<Mutex<soundcraft_engine::Engine>>,
}

impl Studio {
    fn start() -> (Studio, KnownStudio) {
        let hub = Hub::new(NetConfig { encrypted_only: false, ..NetConfig::default() }, None);
        let port = hub.listen(0).unwrap();
        let addr = format!("127.0.0.1:{port}");
        let id = soundcraft_net::follower::pair_request(&addr, "Home PC").unwrap();
        let code = hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap().code;
        let (token, name) = soundcraft_net::follower::pair_confirm(&addr, &id, code, soundcraft_net::pairing::exchange::LinkKeys::PLAIN).unwrap();
        let engine = Arc::new(Mutex::new(studio_ui::demo_studio_engine()));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (hub, engine, stop) = (hub.clone(), Arc::clone(&engine), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    {
                        let mut e = engine.lock().unwrap();
                        for r in hub.drain() {
                            if let HubRequest::Control { method, params, reply } = r {
                                let v = match method.as_str() {
                                    "engine.execute" => {
                                        let cmd = params["command"].as_str().unwrap_or("");
                                        let p = params.get("params").cloned().unwrap_or(json!({}));
                                        match params["gesture"].as_str() {
                                            Some(k) => e.execute_gesture(cmd, &p, k),
                                            None => e.execute(cmd, &p),
                                        }
                                        .map_or_else(|x| json!({"ok": false, "error": x.to_string()}), |v| json!({"ok": true, "result": v}))
                                    }
                                    "engine.end_gesture" => {
                                        e.end_gesture(params["gesture"].as_str().unwrap_or(""));
                                        json!({"ok": true, "result": {}})
                                    }
                                    _ => json!({"ok": false, "error": "unknown"}),
                                };
                                let _ = reply.send(v);
                            }
                        }
                        e.end_idle_merge();
                        if hub.mixer_subscribers() > 0 {
                            hub.publish_mixer(soundcraft_engine::surface::feed::mixer_state(&e));
                        }
                    }
                    std::thread::sleep(Duration::from_millis(16));
                }
            });
        }
        let known = KnownStudio { name, addr, token, ..KnownStudio::default() };
        (Studio { hub, stop, engine }, known)
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.hub.stop();
    }
}

fn run_until(h: &mut Harness<'static, SoundApp>, what: &str, mut done: impl FnMut(&mut Harness<'static, SoundApp>) -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    while Instant::now() < end {
        h.run_steps(1);
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out: {what}");
}

#[test]
fn a_real_studio_shows_its_strips_here_and_its_record_is_kept() {
    let Some(mut h) = app_with_net() else { return };
    let (studio, known) = Studio::start();
    let ctx = h.ctx.clone();
    studio_ui::connect_to(h.state_mut(), &ctx, known.clone());
    run_until(&mut h, "the studio's mixer", |h| h.query_by_label_contains("remotely · Sunday Service Live").is_some());
    assert!(h.query_by_label("Mute Vocal").is_some());
    assert!(h.query_by_label("Solo Keys").is_some());
    // Kept for a one-click reconnect, apart from any booth this PC follows.
    let cfg = h.state().net.as_ref().unwrap().hub.config();
    assert!(cfg.studios.iter().any(|s| s.token == known.token && s.name == known.name));
    assert!(cfg.follow.token.is_empty(), "Follow a booth is not touched");
    // The studio lists this PC as listening.
    assert!(studio.hub.remotes().iter().any(|r| r.state == "listening"), "{:?}", studio.hub.remotes());
    // This PC's own session: untouched.
    assert!(h.state().engine.session().tracks.iter().all(|t| t.name != "Vocal"));
    assert_eq!(studio.engine.lock().unwrap().session().name, "Sunday Service Live");
    h.state_mut().net.as_mut().unwrap().studio.disconnect();
}
