//! "Do to all" and "do to selected" on the Mix strip and the Edit header (`scope`): Alt = every
//! track, Alt+Shift = the selected tracks, Ctrl-click on S solos exclusively, Ctrl+Shift-click (Cmd
//! on macOS) toggles Solo Safe; faders, pans and module bypass follow the same rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{Track, TrackKind};
use soundcraft_ui_egui::scope::{Scope, SoloClick};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

const ALT: Modifiers = Modifiers { alt: true, ..Modifiers::NONE };
const ALT_SHIFT: Modifiers = Modifiers { alt: true, shift: true, ..Modifiers::NONE };
const CTRL: Modifiers = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, command: true, shift: true, ..Modifiers::NONE };

fn app(window: &str) -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run(window, json!({})).unwrap();
    app
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1800.0, 1100.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h);
    h
}

fn steps(h: &mut Harness<'_, SoundApp>) {
    for _ in 0..6 {
        h.step();
    }
}

fn click(h: &mut Harness<'_, SoundApp>, at: Pos2, modifiers: Modifiers) {
    // Clear of any earlier click, so two never count as a double-click.
    for _ in 0..30 {
        h.step();
    }
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerMoved(at));
    h.step();
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers });
    h.step();
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    steps(h);
}

fn key(h: &Harness<'_, SoundApp>, label: &str) -> Pos2 {
    h.get_by_label(label).rect().center()
}

fn tracks(h: &Harness<'_, SoundApp>) -> Vec<Track> {
    h.state().engine.session().tracks.clone()
}

fn named<'a>(ts: &'a [Track], name: &str) -> &'a Track {
    ts.iter().find(|t| t.name == name).unwrap()
}

#[test]
fn the_modifiers_pick_the_scope() {
    assert_eq!(Scope::of(Modifiers::NONE), Scope::One);
    assert_eq!(Scope::of(ALT), Scope::All);
    assert_eq!(Scope::of(ALT_SHIFT), Scope::Selected);
    assert_eq!(SoloClick::of(CTRL, false), SoloClick::Exclusive);
    assert_eq!(SoloClick::of(CTRL_SHIFT, false), SoloClick::Safe(Scope::One));
    assert_eq!(SoloClick::of(ALT, false), SoloClick::Solo(Scope::All));
    // macOS: Cmd-click is Solo Safe, Ctrl-click exclusive.
    let cmd = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
    assert_eq!(SoloClick::of(cmd, true), SoloClick::Safe(Scope::One));
    assert_eq!(SoloClick::of(Modifiers { ctrl: true, ..Modifiers::NONE }, true), SoloClick::Exclusive);
}

#[test]
fn mix_strip_keys_act_on_all_selected_or_exclusively() {
    let mut h = harness(app("window.mix"));
    // Alt-click M: every track with an M takes the clicked track's new state.
    let at = key(&h, "Kick mute");
    click(&mut h, at, ALT);
    let ts = tracks(&h);
    assert!(ts.iter().filter(|t| t.kind != TrackKind::Master).all(|t| t.mixer.mute), "Alt mutes every track");
    let at = key(&h, "Snare mute");
    click(&mut h, at, ALT);
    assert!(tracks(&h).iter().all(|t| !t.mixer.mute), "and unmutes them all");

    // Alt+Shift-click S on a selected track: the selected tracks only.
    h.state_mut().run("edit.select", json!({"tracks": ["Kick", "Snare"]})).unwrap();
    steps(&mut h);
    let at = key(&h, "Kick solo");
    click(&mut h, at, ALT_SHIFT);
    let ts = tracks(&h);
    assert!(named(&ts, "Kick").mixer.solo && named(&ts, "Snare").mixer.solo);
    assert!(!named(&ts, "Hats").mixer.solo && !named(&ts, "Bass").mixer.solo);

    // Ctrl-click S: this track alone.
    let bass = h.query_all_by_label_contains("Bass solo").next().unwrap().rect().center();
    click(&mut h, bass, CTRL);
    let soloed: Vec<String> = tracks(&h).iter().filter(|t| t.mixer.solo).map(|t| t.name.clone()).collect();
    assert_eq!(soloed, ["Bass"]);

    // Ctrl+Shift-click S: Solo Safe, marked in the key's name.
    let hats = h.query_all_by_label_contains("Hats solo").next().unwrap().rect().center();
    click(&mut h, hats, CTRL_SHIFT);
    let ts = tracks(&h);
    assert!(named(&ts, "Hats").mixer.solo_safe && !named(&ts, "Hats").mixer.solo);
    assert!(h.query_by_label_contains("Hats solo, solo safe").is_some());
}

#[test]
fn edit_header_keys_follow_the_same_rule() {
    let mut a = app("window.edit");
    a.run("track.height", json!({"tracks": ["Kick", "Snare", "Hats"], "height": "medium"})).unwrap();
    let mut h = harness(a);
    let at = key(&h, "Kick mute");
    click(&mut h, at, ALT);
    assert!(tracks(&h).iter().filter(|t| t.kind != TrackKind::Master).all(|t| t.mixer.mute));
    h.state_mut().run("mix.clear_mutes", json!({})).unwrap();
    let at = key(&h, "Snare solo");
    click(&mut h, at, CTRL);
    let kick = h.query_all_by_label_contains("Kick solo").next().unwrap().rect().center();
    click(&mut h, kick, CTRL);
    let soloed: Vec<String> = tracks(&h).iter().filter(|t| t.mixer.solo).map(|t| t.name.clone()).collect();
    assert_eq!(soloed, ["Kick"]);
}

#[test]
fn faders_pans_and_bypass_follow_the_scope() {
    let mut a = app("window.mix");
    let ids: Vec<_> = a.engine.session().tracks.iter().map(|t| (t.name.clone(), t.id, t.mixer.volume_db)).collect();
    let kick = ids.iter().find(|t| t.0 == "Kick").unwrap();
    // Alt-drag: every fader moves by the same amount.
    soundcraft_ui_egui::scope::volume(&mut a, MainWindow::Mix, kick.1, kick.2, kick.2 - 3.0, false, ALT);
    a.engine.end_merge();
    for (name, id, before) in &ids {
        let now = a.engine.session().track(*id).unwrap().mixer.volume_db;
        if a.engine.session().track(*id).unwrap().kind != TrackKind::Folder {
            assert!((now - (before - 3.0).clamp(-144.0, 12.0)).abs() < 0.01, "{name}: {before} → {now}");
        }
    }
    a.engine.execute("edit.undo", &json!({})).unwrap();
    assert!((a.engine.session().track(kick.1).unwrap().mixer.volume_db - kick.2).abs() < 0.01, "one undo step");

    // Alt+Shift: the selected tracks' pans.
    a.run("edit.select", json!({"tracks": ["Kick", "Snare"]})).unwrap();
    soundcraft_ui_egui::scope::pan(&mut a, MainWindow::Mix, kick.1, 0, -0.5, ALT_SHIFT);
    let s = a.engine.session();
    assert_eq!(s.track_by_name("Kick").unwrap().mixer.pan[0], -0.5);
    assert_eq!(s.track_by_name("Snare").unwrap().mixer.pan[0], -0.5);
    assert_ne!(s.track_by_name("Hats").unwrap().mixer.pan[0], -0.5);

    // Alt on a module: the module at that position on every track that has one, one undo step.
    let first = |a: &SoundApp, n: &str| a.engine.session().track_by_name(n).unwrap().mixer.inserts.first().map(|i| i.bypass);
    let kick_mod = a.engine.session().track_by_name("Kick").unwrap().mixer.inserts[0].id;
    soundcraft_ui_egui::scope::bypass(&mut a, MainWindow::Mix, kick.1, kick_mod, ALT);
    let with: Vec<String> = a.engine.session().tracks.iter().filter(|t| !t.mixer.inserts.is_empty()).map(|t| t.name.clone()).collect();
    assert!(with.len() > 1);
    assert!(with.iter().all(|n| first(&a, n) == Some(true)), "{with:?}");
    a.engine.execute("edit.undo", &json!({})).unwrap();
    assert!(with.iter().all(|n| first(&a, n) == Some(false)), "one undo step");
}
