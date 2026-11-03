//! View › Expanded Sends: one send shown on every Mix strip as a mini fader with its dB, pan, PRE
//! and M, as in Pro Tools; and the command behind it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_model::SendSlot;
use soundcraft_ui_egui::{Services, SoundApp};

fn app() -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    app.run("mix.send", json!({"track": "Kick", "slot": 0, "bus": "FX Bus", "level_db": -6.0})).unwrap();
    app
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1800.0, 1200.0)).with_step_dt(1.0 / 60.0).build_ui_state(
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

fn send(h: &Harness<'_, SoundApp>) -> SendSlot {
    h.state().engine.session().track_by_name("Kick").unwrap().mixer.sends[0].clone().unwrap()
}

fn drag(h: &mut Harness<'_, SoundApp>, from: Pos2, dx: f32) {
    let m = Modifiers::NONE;
    h.event(Event::PointerMoved(from));
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: m });
    h.step();
    for i in 1..=5 {
        h.event(Event::PointerMoved(from + vec2(dx * i as f32 / 5.0, 0.0)));
        h.step();
    }
    let to = from + vec2(dx, 0.0);
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: m });
    steps(h);
}

#[test]
fn the_menu_runs_the_sessions_expanded_send_flags() {
    use soundcraft_ui_egui::mix_window::send_expanded;
    let mut app = app();
    assert!((0..10).all(|k| !send_expanded(app.engine.session(), k)));
    // The menu items run the engine's flags and tick the ones on.
    let extra = soundcraft_ui_egui::menus::ui_aliases();
    let path = "View > Expanded Sends > Send C";
    let id = soundcraft_ui_egui::menus::command_for_path(path, &extra).unwrap();
    assert_eq!(id, "view.expanded_send_c");
    app.run(&id, json!({})).unwrap();
    assert!(send_expanded(app.engine.session(), 2) && !send_expanded(app.engine.session(), 0));
    assert_eq!(soundcraft_ui_egui::menus::item_state(&app, path, Some(&id)), (true, true));
    app.run("view.expanded_sends_all", json!({})).unwrap();
    assert!((0..10).all(|k| send_expanded(app.engine.session(), k)));
    app.run("view.expanded_sends_none", json!({})).unwrap();
    assert!((0..10).all(|k| !send_expanded(app.engine.session(), k)));
}

#[test]
fn an_expanded_send_reads_and_sets_level_pan_pre_and_mute_on_the_strip() {
    let mut app = app();
    app.run("view.expanded_send_a", json!({"value": true})).unwrap();
    let _ = app.run("mix.send_remove", json!({"track": "Bass", "slot": 0}));
    let mut h = harness(app);
    // Kick's send A has its controls; the Bass (no send A) has none, so no stray rows.
    let level = h.get_by_label("Kick Send A level");
    assert_eq!(level.accesskit_node().numeric_value(), Some(-6.0));
    assert!(h.query_by_label("Bass Send A level").is_none());

    h.get_by_label("Kick Send A mute").click();
    steps(&mut h);
    assert!(send(&h).mute);
    h.get_by_label("Kick Send A pre-fader").click();
    steps(&mut h);
    assert!(send(&h).pre_fader);

    let r = h.get_by_label("Kick Send A level").rect();
    drag(&mut h, r.center(), -20.0);
    assert!(send(&h).level_db < -6.5, "dragging left lowers the send: {}", send(&h).level_db);

    let r = h.get_by_label("Kick Send A pan").rect();
    drag(&mut h, r.center(), 20.0);
    assert!(send(&h).pan > 0.1, "dragging right pans right: {}", send(&h).pan);
    // Double-click centres.
    let c = h.get_by_label("Kick Send A pan").rect().center();
    // Half a second on, so the drag's release isn't counted as the first click.
    for _ in 0..30 {
        h.step();
    }
    h.event(Event::PointerMoved(c));
    h.step();
    for pressed in [true, false, true, false] {
        h.event(Event::PointerButton { pos: c, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    steps(&mut h);
    assert_eq!(send(&h).pan, 0.0);

    // None: the rows go.
    h.state_mut().run("view.expanded_sends_none", json!({})).unwrap();
    steps(&mut h);
    assert!(h.query_by_label("Kick Send A level").is_none());
}
