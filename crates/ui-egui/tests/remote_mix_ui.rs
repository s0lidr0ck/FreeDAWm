//! File › Connect to a Studio… and the Remote Mix window: the dialog, the banner that says whose
//! mixer this is, the greyed reconnecting state, and a real connection to an in-process studio
//! whose strips show here.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_net::config::{KnownStudio, NetConfig};
use soundcraft_net::hub::{Hub, HubRequest};
use soundcraft_ui_egui::studio_editors as studio_ui_editors;
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

#[test]
fn mute_and_bypass_clicked_here_change_the_studio() {
    let Some(mut h) = app_with_net() else { return };
    let (studio, known) = Studio::start();
    let ctx = h.ctx.clone();
    studio_ui::connect_to(h.state_mut(), &ctx, known);
    run_until(&mut h, "the studio's mixer", |h| h.query_by_label("Mute Vocal").is_some());
    h.get_by_label("Mute Vocal").click();
    let vocal = |s: &Studio| s.engine.lock().unwrap().session().tracks.iter().find(|t| t.name == "Vocal").unwrap().clone();
    run_until(&mut h, "the studio's mute", |_| vocal(&studio).mixer.mute);
    // The EQ on the vocal: bypassed from here.
    let label = h
        .query_all_by_label_contains("Bypass")
        .map(|n| n.accesskit_node().label().unwrap_or_default().to_string())
        .find(|l| l.ends_with("on Vocal"))
        .expect("a bypass key on the vocal's inserts");
    h.get_by_label(&label).click();
    run_until(&mut h, "the studio's bypass", |_| vocal(&studio).mixer.inserts.first().is_some_and(|i| i.bypass));
    // Undo from here asks first (it is someone else's session).
    run_until(&mut h, "the undo label", |h| h.query_by_label_contains("Undo Bypass").is_some() || h.query_by_label_contains("Undo ").is_some());
    let undo = h
        .query_all_by_label_contains("Undo ")
        .map(|n| n.accesskit_node().label().unwrap_or_default().to_string())
        .find(|l| l.starts_with("Undo "))
        .unwrap();
    h.get_by_label(&undo).click();
    h.run_steps(3);
    assert!(h.query_by_label("Undo there").is_some(), "asked first");
    h.get_by_label("Undo there").click();
    run_until(&mut h, "undone on the studio", |_| vocal(&studio).mixer.inserts.first().is_some_and(|i| !i.bypass));
    assert!(vocal(&studio).mixer.mute, "one step back: the mute stays");
    h.state_mut().net.as_mut().unwrap().studio.disconnect();
}

#[test]
fn on_the_studio_a_remote_fader_drag_is_one_undo_step_through_the_real_control_path() {
    // This app is the studio: listening, a PC paired, its UI running the commands.
    let app = SoundApp::new(studio_ui::demo_studio_engine(), None, Services::default());
    let mut h = harness(app);
    h.run_steps(2);
    let Some(hub) = h.state().net.as_ref().map(|n| n.hub.clone()) else { return };
    hub.update_config(|c| c.encrypted_only = false);
    let port = hub.listen(0).unwrap();
    let addr = format!("127.0.0.1:{port}");
    let req = {
        let a = addr.clone();
        std::thread::spawn(move || soundcraft_net::follower::pair_request(&a, "Home PC"))
    };
    run_until(&mut h, "the pairing request", |h| !h.state().net.as_ref().unwrap().hub.pending_pairings().is_empty());
    let id = req.join().unwrap().unwrap();
    let code = hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap().code;
    let token = {
        let a = addr.clone();
        std::thread::spawn(move || soundcraft_net::follower::pair_confirm(&a, &id, code, soundcraft_net::pairing::exchange::LinkKeys::PLAIN))
    };
    run_until(&mut h, "paired", |h| !h.state().net.as_ref().unwrap().hub.config().paired.is_empty());
    let (token, _) = token.join().unwrap().unwrap();
    let remote =
        soundcraft_net::studio::Studio::start(soundcraft_net::studio::StudioTarget { name: String::new(), addr, token, my_name: "Home PC".into() });
    run_until(&mut h, "the remote's mirror", |_| remote.connected() && remote.mirror().has_state());
    let v = h.state().engine.session().tracks.iter().find(|t| t.name == "Vocal").unwrap().id.0;
    let vol = |h: &Harness<'static, SoundApp>| h.state().engine.session().tracks.iter().find(|t| t.id.0 == v).unwrap().mixer.volume_db;
    let before = vol(&h);
    let depth = h.state().engine.undo_depth();
    let key = soundcraft_net::studio::Control::Volume(v).gesture();
    for db in [-1.0f32, -2.0, -5.5] {
        remote.execute("mix.volume", json!({"tracks": [v], "db": db}), Some(&key));
        run_until(&mut h, "the move", |h| vol(h) == db);
        // Studio frames go by with no pointer down.
        h.run_steps(3);
    }
    remote.end_gesture(&key);
    run_until(&mut h, "the release", |h| !h.state().engine.gesture_open(&key));
    assert_eq!(h.state().engine.undo_depth(), depth + 1, "one undo step for the whole drag");
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(vol(&h), before);
    remote.stop();
    hub.stop();
}

// ---- Stage three: the built-in effect editors on the studio's inserts ----

fn pointer(h: &mut Harness<'static, SoundApp>, pos: egui::Pos2, pressed: Option<bool>) {
    h.input_mut().events.push(egui::Event::PointerMoved(pos));
    if let Some(pressed) = pressed {
        h.input_mut().events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    h.step();
}

/// Press at `from`, move to `to` in small steps, release.
fn drag(h: &mut Harness<'static, SoundApp>, from: egui::Pos2, to: egui::Pos2) {
    pointer(h, from, None);
    pointer(h, from, Some(true));
    let d = to - from;
    let n = (d.length() / 8.0).ceil().max(1.0) as usize;
    for k in 1..=n {
        pointer(h, from + d * (k as f32 / n as f32), None);
    }
    pointer(h, to, Some(false));
    h.run_steps(2);
}

#[test]
fn every_full_editor_opens_over_a_mirror_and_leaves_this_session_alone() {
    let Some(mut h) = app_with_net() else { return };
    let rev = h.state().engine.revision;
    let tracks = h.state().engine.session().tracks.len();
    for plugin in studio_ui_editors::FULL_EDITORS {
        let mut e = soundcraft_engine::Engine::default();
        e.execute("track.new", &json!({"name": "Vocal", "kind": "audio"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let m = e.execute("mix.insert", &json!({"track": t, "plugin": plugin})).unwrap()["module"].as_u64().unwrap() as u32;
        let mut mirror = soundcraft_net::studio::MixMirror::default();
        mirror.on_event("mixer.snapshot", &json!({"seq": 1, "state": soundcraft_engine::surface::feed::mixer_state(&e)}));
        studio_ui::demo(h.state_mut(), "connected").unwrap();
        h.state_mut().net.as_mut().unwrap().studio.demo.as_mut().unwrap().view = mirror.view().unwrap();
        studio_ui::open_editor(h.state_mut(), t, m).unwrap_or_else(|e| panic!("{plugin}: {e}"));
        h.run_steps(3);
        let ed = h.state().net.as_ref().unwrap().studio.editor.as_ref().map(|e| (e.plugin.clone(), e.kind));
        assert_eq!(ed, Some((plugin.to_string(), studio_ui_editors::EditorKind::Full)), "{plugin} stays open");
        assert!(h.query_by_label_contains("on Vocal (on Studio PC)").is_some(), "{plugin}: its window");
    }
    assert_eq!(h.state().engine.revision, rev, "this PC's session never changed");
    assert_eq!(h.state().engine.session().tracks.len(), tracks);
}

#[test]
fn the_comp_editor_drags_the_studios_compressor_as_one_undo_step() {
    let Some(mut h) = app_with_net() else { return };
    let (studio, known) = Studio::start();
    let ctx = h.ctx.clone();
    studio_ui::connect_to(h.state_mut(), &ctx, known);
    run_until(&mut h, "the studio's mixer", |h| h.query_by_label("Mute Vocal").is_some());
    let (v, comp, before, depth) = {
        let e = studio.engine.lock().unwrap();
        let t = e.session().tracks.iter().find(|t| t.name == "Vocal").unwrap();
        let c = t.mixer.inserts.iter().find(|i| i.plugin == "fs_comp").unwrap();
        (t.id.0, c.id.0, c.params.get("amount").copied().unwrap_or(0.0), e.undo_depth())
    };
    // A click on its name opens its own editor, working on the studio's insert.
    let name = h
        .query_all_by_label_contains("on Vocal")
        .map(|n| n.accesskit_node().label().unwrap_or_default().to_string())
        .find(|l| !l.starts_with("Bypass") && l.contains("Comp"))
        .expect("the comp's row");
    h.get_by_label(&name).click();
    run_until(&mut h, "its editor", |h| h.query_by_label("Threshold").is_some());
    assert_eq!(h.state().net.as_ref().unwrap().studio.editor.as_ref().map(|e| (e.strip, e.module)), Some((v, comp)));
    let knob = h.get_by_label("Threshold").rect().center();
    drag(&mut h, knob, knob - egui::vec2(0.0, 50.0));
    let amount = |s: &Studio| {
        let e = s.engine.lock().unwrap();
        e.session()
            .tracks
            .iter()
            .find(|t| t.id.0 == v)
            .unwrap()
            .mixer
            .inserts
            .iter()
            .find(|i| i.id.0 == comp)
            .unwrap()
            .params
            .get("amount")
            .copied()
            .unwrap_or(0.0)
    };
    run_until(&mut h, "the studio's compressor moved", |_| (amount(&studio) - (before + 0.4)).abs() < 0.03);
    run_until(&mut h, "one step", |_| studio.engine.lock().unwrap().undo_depth() == depth + 1);
    run_until(&mut h, "the drag closed", |_| !studio.engine.lock().unwrap().gesture_open(&format!("editor:{v}:{comp}")));
    // The studio's change comes back into the editor (here: undone there).
    studio.engine.lock().unwrap().execute("edit.undo", &json!({})).unwrap();
    run_until(&mut h, "the editor follows the studio", |h| {
        let ed = h.state().net.as_ref().unwrap().studio.editor.as_ref().map(|e| e.title.clone());
        ed.is_some() && (amount(&studio) - before).abs() < 1e-4
    });
    h.state_mut().net.as_mut().unwrap().studio.disconnect();
}

#[test]
fn a_built_in_without_a_full_editor_opens_as_a_list_and_a_third_party_one_does_not_open() {
    let Some(mut h) = app_with_net() else { return };
    studio_ui::demo(h.state_mut(), "connected").unwrap();
    h.run_steps(2);
    let view = h.state().net.as_ref().unwrap().studio.demo.as_ref().unwrap().view.clone();
    let vocal = view.strips.iter().find(|s| s.name == "Vocal").unwrap();
    let eq = vocal.inserts.iter().find(|i| i.plugin == "eq_7band").unwrap();
    studio_ui::open_editor(h.state_mut(), vocal.id, eq.module).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().net.as_ref().unwrap().studio.editor.as_ref().map(|e| e.kind), Some(studio_ui_editors::EditorKind::Generic));
    let info = soundcraft_dsp::plugin_info("eq_7band").unwrap();
    let first = info.params.iter().find(|p| eq.params.iter().any(|(k, _)| k == p.id)).unwrap();
    assert!(h.query_all_by_label(first.name).count() >= 2, "its parameters by name (its name and its slider)");
    // A third-party plug-in: name and bypass only.
    let mut hosted = vocal.clone();
    hosted.inserts = vec![soundcraft_net::studio::RemoteInsert {
        slot: 0,
        module: 99,
        plugin: "clap:com.acme.verb".into(),
        name: "Acme Verb".into(),
        bypass: false,
        hosted: true,
        params: Vec::new(),
    }];
    {
        let d = h.state_mut().net.as_mut().unwrap().studio.demo.as_mut().unwrap();
        if let Some(s) = d.view.strips.iter_mut().find(|s| s.id == vocal.id) {
            *s = hosted;
        }
    }
    assert!(studio_ui::open_editor(h.state_mut(), vocal.id, 99).is_err(), "no editor here");
    h.run_steps(3);
    assert!(h.query_by_label("Bypass Acme Verb on Vocal").is_some());
}
