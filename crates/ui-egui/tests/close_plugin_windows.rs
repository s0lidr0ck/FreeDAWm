//! Window › Layout › Close Plug-in Windows and its button beside the Mix window's Channel width
//! bar (`plugin_close`): every kind of plug-in window closes, other windows stay, the button is
//! greyed with none open and says how many it closes.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_playback::SlotKey;
use soundcraft_ui_egui::{Services, SoundApp, menus, plugin_close};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const HOSTED: &str = "vst3:fake.comp";
const HOSTED_CLIP: &str = "vst3:fake.clipfx";

/// A hosted plug-in's editor window that counts its teardowns.
struct FakeEditor {
    open: bool,
    closes: Arc<AtomicUsize>,
}

impl soundcraft_dsp::PluginEditor for FakeEditor {
    fn open(&mut self) -> Result<(), String> {
        self.open = true;
        Ok(())
    }
    fn close(&mut self) {
        if self.open {
            self.closes.fetch_add(1, Ordering::Relaxed);
        }
        self.open = false;
    }
    fn is_open(&self) -> bool {
        self.open
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        Vec::new()
    }
}

struct Rig {
    h: Harness<'static, SoundApp>,
    closes: Arc<AtomicUsize>,
    hosted: (soundcraft_model::TrackId, SlotKey),
    clip_slot: (soundcraft_model::TrackId, u64, u32),
}

/// The demo session in the Mix window with a hosted insert on Bass and a hosted clip effect on
/// Kick's first clip, each with a fake editor; nothing open yet.
fn rig() -> Rig {
    let mut engine = soundcraft_engine::demo::demo_engine();
    let bass = engine.session().track_by_name("Bass").unwrap().id;
    let kick = engine.session().track_by_name("Kick").unwrap().id;
    let m = engine.session_mut().track_mut(bass).unwrap().mixer.add_module(soundcraft_model::Insert::new(HOSTED), None).unwrap();
    let clip = {
        let t = engine.session_mut().track_mut(kick).unwrap();
        let c = t.playlists[t.active_playlist].clips.iter_mut().find(|c| c.is_audio()).unwrap();
        c.modules.push(soundcraft_model::ClipModule::new(1, HOSTED_CLIP));
        c.id.0
    };
    let player = soundcraft_playback::Player::new(engine.session_arc());
    let closes = Arc::new(AtomicUsize::new(0));
    let hosted = (bass, SlotKey::Module(m));
    let clip_slot = (kick, clip, 1);
    player.attach_editor(hosted.0, hosted.1, Box::new(FakeEditor { open: false, closes: closes.clone() }));
    player.attach_clip_editor(clip_slot, Box::new(FakeEditor { open: false, closes: closes.clone() }));
    let mut app = SoundApp::new(engine, Some(player), Services::default());
    app.run("window.mix", json!({})).unwrap();
    let h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    Rig { h, closes, hosted, clip_slot }
}

/// Opens one plug-in window of each kind this build can open without a real plug-in: a docked
/// and a detached module window (pinned), an instrument window, a full editor, a drawer, a
/// hosted insert's editor and a hosted clip effect's editor. Returns how many.
fn open_all(rig: &mut Rig) -> usize {
    let app = rig.h.state_mut();
    let s = app.engine.session();
    let kick = s.track_by_name("Kick").unwrap();
    let (kick_id, eq, comp) = (kick.id, kick.mixer.inserts[0].id, kick.mixer.inserts[1].id);
    let keys = s.tracks.iter().find(|t| t.instrument.is_some()).map(|t| t.id).unwrap();
    let bass = s.track_by_name("Bass").unwrap();
    let (bass_id, bass_comp) = (bass.id, bass.mixer.inserts[0].id);
    app.ui.plugin_windows = vec![(kick_id, SlotKey::Module(eq)), (kick_id, SlotKey::Module(comp)), (keys, SlotKey::Instrument)];
    app.ui.detached_plugins.push((kick_id, SlotKey::Module(comp)));
    app.ui.pinned_plugins.push((kick_id, SlotKey::Module(comp)));
    app.engine.execute("mix.module_add", &json!({"track": kick_id.0, "plugin": "fs_comp"})).unwrap();
    let wide = app.engine.session().track(kick_id).unwrap().mixer.inserts.iter().rev().find(|i| i.plugin == "fs_comp").unwrap().id;
    app.ui.full_editors.push((kick_id, wide));
    app.ui.drawers.insert(bass_id, bass_comp);
    let p = app.player.as_ref().unwrap();
    p.open_editor(rig.hosted.0, rig.hosted.1).unwrap();
    p.open_clip_editor(rig.clip_slot).unwrap();
    rig.h.run_steps(3);
    plugin_close::count(rig.h.state())
}

#[test]
fn the_button_sits_beside_the_channel_width_bar_and_is_greyed_with_nothing_open() {
    let mut rig = rig();
    rig.h.run_steps(3);
    let all = rig.h.get_by_label("All").rect();
    let button = rig.h.get_by_label("Close Plug-ins");
    let r = button.rect();
    assert!(r.left() > all.right() && r.left() - all.right() < 40.0, "right of All: {r:?} {all:?}");
    assert!((r.center().y - all.center().y).abs() < 2.0, "on the same row");
    assert!(button.accesskit_node().is_disabled(), "greyed: nothing to close");
    assert_eq!(plugin_close::tooltip(0), "No plug-in windows are open");
    assert_eq!(menus::availability(rig.h.state(), plugin_close::COMMAND), Err("No plug-in windows are open".into()));
}

#[test]
fn the_button_closes_every_plugin_window_and_leaves_the_others() {
    let mut rig = rig();
    let n = open_all(&mut rig);
    // 3 windows, 1 full editor, 1 drawer, the hosted insert's editor and the clip effect's.
    assert_eq!(n, 7, "{:?}", plugin_close::open(rig.h.state()));
    assert_eq!(plugin_close::tooltip(n), "Close 7 plug-in windows (pinned ones too)");
    assert!(menus::availability(rig.h.state(), plugin_close::COMMAND).is_ok());
    // Windows that are not plug-ins.
    {
        let app = rig.h.state_mut();
        app.ui.show_transport = true;
        app.ui.show_big_counter = true;
        app.ui.show_midi_editor = true;
        app.ui.show_undo_history = true;
    }
    rig.h.run_steps(2);
    let button = rig.h.get_by_label("Close Plug-ins");
    assert!(!button.accesskit_node().is_disabled());
    button.click();
    rig.h.run_steps(3);
    let app = rig.h.state();
    assert_eq!(plugin_close::count(app), 0, "{:?}", plugin_close::open(app));
    assert!(app.ui.plugin_windows.is_empty() && app.ui.full_editors.is_empty() && app.ui.drawers.is_empty());
    let p = app.player.as_ref().unwrap();
    assert!(!p.editor_open(rig.hosted.0, rig.hosted.1) && !p.clip_editor_open(rig.clip_slot));
    assert_eq!(rig.closes.load(Ordering::Relaxed), 2, "each hosted editor torn down once, by its own close");
    // Its instance and editor stay: it opens again.
    assert!(p.has_editor(rig.hosted.0, rig.hosted.1));
    // The others stay open, and so do the Mix window and the windows' own choices (detach, pin).
    assert!(app.ui.show_transport && app.ui.show_big_counter && app.ui.show_midi_editor && app.ui.show_undo_history);
    assert_eq!(app.ui.window, soundcraft_ui_egui::MainWindow::Mix);
    assert_eq!(app.ui.pinned_plugins.len(), 1);
    assert_eq!(app.ui.detached_plugins.len(), 1);
    assert!(app.ui.status.contains("Closed 7 plug-in windows"), "{}", app.ui.status);
    assert!(rig.h.get_by_label("Close Plug-ins").accesskit_node().is_disabled(), "greyed again");
}

#[test]
fn the_command_and_the_menu_item_close_them_too() {
    let mut rig = rig();
    let n = open_all(&mut rig);
    let extra = menus::ui_aliases();
    assert_eq!(menus::command_for_path("Window > Layout > Close Plug-in Windows", &extra).as_deref(), Some(plugin_close::COMMAND));
    let r = rig.h.state_mut().run(plugin_close::COMMAND, json!({})).unwrap();
    assert_eq!(r["closed"], json!(n));
    assert_eq!(r["hosted"], json!(1));
    assert_eq!(r["clip_hosted"], json!(1));
    assert_eq!(r["plugin_windows"], json!(3));
    assert_eq!(plugin_close::count(rig.h.state()), 0);
    // Nothing open: the item is greyed with its reason, and the command closes nothing.
    assert!(menus::availability(rig.h.state(), plugin_close::COMMAND).is_err());
    let r = rig.h.state_mut().run(plugin_close::COMMAND, json!({})).unwrap();
    assert_eq!(r["closed"], json!(0));
    // Its key is listed with it.
    let (_, label, _, key) = menus::UI_COMMANDS.iter().find(|c| c.0 == plugin_close::COMMAND).unwrap();
    assert_eq!((*label, *key), ("Close Plug-in Windows", Some("Cmd+Alt+W")));
}

#[test]
fn the_remote_mix_windows_editor_closes_too() {
    let app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1500.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    if h.state().net.is_none() {
        eprintln!("SKIPPED: no network state");
        return;
    }
    soundcraft_ui_egui::studio_ui::demo(h.state_mut(), "connected").unwrap();
    h.run_steps(2);
    let view = h.state().net.as_ref().unwrap().studio.demo.as_ref().unwrap().view.clone();
    let vocal = view.strips.iter().find(|s| s.name == "Vocal").unwrap();
    let eq = vocal.inserts.iter().find(|i| i.plugin == "eq_7band").unwrap();
    soundcraft_ui_egui::studio_ui::open_editor(h.state_mut(), vocal.id, eq.module).unwrap();
    h.run_steps(2);
    assert_eq!(plugin_close::count(h.state()), 1);
    let r = h.state_mut().run(plugin_close::COMMAND, json!({})).unwrap();
    assert_eq!(r["remote"], json!(1));
    h.run_steps(2);
    let net = h.state().net.as_ref().unwrap();
    assert!(net.studio.editor.is_none());
    assert!(net.studio.window, "the Remote Mix window itself stays open");
}
