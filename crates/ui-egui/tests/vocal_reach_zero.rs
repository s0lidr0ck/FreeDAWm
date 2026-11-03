//! The Vocal Editor's roll reaches the session start, whatever the notes: a clip at 0 whose
//! first sung note is near 22 s.

mod vocal_support;

use egui::{Event, Modifiers, MouseWheelUnit, TouchPhase, vec2};
use serde_json::json;
use soundcraft_ui_egui::vocal_editor::{open_at, set_zoom_factor};
use vocal_support::*;

/// A clip at 0: 22 s of silence, then two sung notes.
fn late_singer() -> Setup {
    let mut take: Vec<f32> = gap(22.0).collect();
    take.extend(take2());
    setup_with(vec![(take, 0)], true)
}

fn t0(ctx: &egui::Context) -> i64 {
    view(ctx).t0
}

/// Wheels the roll sideways (Shift+wheel) by `steps` × 200 px towards the start.
fn wheel_left(ctx: &egui::Context, app: &mut soundcraft_ui_egui::SoundApp, steps: usize) {
    let over = view(ctx).rect.center();
    frame(ctx, app, vec![Event::PointerMoved(over)]);
    for _ in 0..steps {
        frame_m(
            ctx,
            app,
            vec![Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, 200.0), phase: TouchPhase::Move, modifiers: Modifiers::SHIFT }],
            Modifiers::SHIFT,
        );
    }
    settle(ctx, app);
}

/// Opened on the first note (Tune Vocal…) and zoomed in about it, the roll's left edge sits
/// ahead of the Edit window's: scrolling back still reaches 0.
#[test]
fn the_roll_scrolls_back_to_zero_after_a_zoom_about_a_late_note() {
    let Setup { ctx, mut app, track, .. } = late_singer();
    let ns = notes(&app, track);
    let (first, _) = span(&app, ns[0]);
    assert!(first > 21 * i64::from(RATE), "the first note is late: {first}");
    open_at(&mut app, track, first, Some(ns[0]));
    settle(&ctx, &mut app);
    set_zoom_factor(&mut app, 4.0);
    settle(&ctx, &mut app);
    let edit = app.engine.session().edit.zoom.scroll;
    assert!(t0(&ctx) > edit + i64::from(RATE) / 2, "the zoom kept the note in place, ahead of the Edit window: {} vs {edit}", t0(&ctx));
    wheel_left(&ctx, &mut app, 400);
    assert_eq!(app.engine.session().edit.zoom.scroll, 0, "the Edit window is at the start");
    assert_eq!(t0(&ctx), 0, "the roll reaches the session start too");
    // And it scrolls forward again from there.
    let over = view(&ctx).rect.center();
    frame_m(
        &ctx,
        &mut app,
        vec![
            Event::PointerMoved(over),
            Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, -200.0), phase: TouchPhase::Move, modifiers: Modifiers::SHIFT },
        ],
        Modifiers::SHIFT,
    );
    settle(&ctx, &mut app);
    assert!(t0(&ctx) > 0);
}

/// The Edit window scrolled (or returned) to the start takes the roll to 0 with it.
#[test]
fn the_edit_window_at_zero_takes_the_roll_to_zero() {
    let Setup { ctx, mut app, track, .. } = late_singer();
    let ns = notes(&app, track);
    let (first, _) = span(&app, ns[0]);
    open_at(&mut app, track, first, Some(ns[0]));
    settle(&ctx, &mut app);
    // A hand zoom about the pointer, far right: the roll's edge moves ahead of the Edit window's.
    let r = view(&ctx).rect;
    frame_m(
        &ctx,
        &mut app,
        vec![
            Event::PointerMoved(r.right_center() - vec2(10.0, 0.0)),
            Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, -600.0), phase: TouchPhase::Move, modifiers: Modifiers::CTRL },
        ],
        Modifiers::CTRL,
    );
    settle(&ctx, &mut app);
    assert!(t0(&ctx) > app.engine.session().edit.zoom.scroll);
    app.run("view.scroll", json!({"to": 0})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(t0(&ctx), 0, "the roll follows the Edit window to the start");
}
