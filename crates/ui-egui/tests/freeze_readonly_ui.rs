//! A frozen instrument track's own controls say so: the instrument's plugin window is greyed with
//! the reason, and a velocity drag in the MIDI editor shows the refusal in the status line.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::accesskit::Role;
use egui::{Event, Id, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn sliders(h: &Harness<'_, SoundApp>) -> (usize, usize) {
    let (mut on, mut off) = (0, 0);
    for n in h.get_all_by_role(Role::Slider) {
        if n.accesskit_node().is_disabled() { off += 1 } else { on += 1 }
    }
    (on, off)
}

#[test]
fn the_instrument_window_is_greyed_while_the_track_is_frozen() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    app.ui.plugin_windows = vec![(keys, soundcraft_mix::SlotKey::Instrument)];
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let (on, off) = sliders(&h);
    assert!(on > 0 && off == 0, "live: the synth's sliders work ({on} on, {off} off)");
    h.state_mut().run("track.freeze", json!({"tracks": [keys.0], "freeze": true})).unwrap();
    h.run_steps(4);
    let (on2, off2) = sliders(&h);
    assert!(on2 == 0 && off2 == on, "frozen: every slider is greyed ({on2} on, {off2} off)");
    h.state_mut().run("track.unfreeze", json!({"tracks": [keys.0]})).unwrap();
    h.run_steps(4);
    assert_eq!(sliders(&h), (on, 0), "thawed: the sliders work again");
}

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

fn click(ctx: &egui::Context, app: &mut SoundApp, at: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    frame(ctx, app, vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }]);
    frame(ctx, app, vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }]);
    frame(ctx, app, vec![]);
}

/// Clicks along the velocity lane (a click lands on the note nearest it, within 8 px) until one
/// changes the session, or sets the status; where that was.
fn click_a_velocity(ctx: &egui::Context, app: &mut SoundApp) -> Option<Pos2> {
    let vel = ctx.data(|d| d.get_temp::<Rect>(Id::new("soundcraft-midi-vel"))).expect("the velocity lane");
    let before = app.engine.session().clone();
    let mut x = vel.min.x + 2.0;
    while x < vel.max.x {
        let at = pos2(x, vel.min.y + vel.height() * 0.25);
        click(ctx, app, at);
        if *app.engine.session() != before || !app.ui.status.is_empty() {
            return Some(at);
        }
        x += 6.0;
    }
    None
}

#[test]
fn a_velocity_drag_on_a_frozen_track_says_so_in_the_status_line() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    let (keys, cid) = {
        let t = app.engine.session().track_by_name("Keys").unwrap();
        (t.id, t.clips()[0].id)
    };
    app.run("edit.select", json!({"clips": [cid.0]})).unwrap();
    let ctx = egui::Context::default();
    for _ in 0..6 {
        frame(&ctx, &mut app, vec![]);
    }
    // Live, a click in the lane sets a note's velocity.
    let before = app.engine.session().clone();
    let at = click_a_velocity(&ctx, &mut app).expect("a click reaches a note's velocity");
    assert!(*app.engine.session() != before, "live: the velocity changed");
    assert!(app.ui.status.is_empty(), "live: no complaint: {}", app.ui.status);
    app.run("edit.undo", json!({})).unwrap();
    app.run("track.freeze", json!({"tracks": [keys.0], "freeze": true})).unwrap();
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let frozen = app.engine.session().clone();
    click(&ctx, &mut app, at);
    assert!(*app.engine.session() == frozen, "frozen: the notes are read-only");
    assert!(app.ui.status.contains("`Keys` is frozen") && app.ui.status.contains("Unfreeze the track to edit it"), "status: {}", app.ui.status);
}
