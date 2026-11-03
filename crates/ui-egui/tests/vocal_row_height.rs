//! The Vocal Editor's vertical zoom: − / + row-height buttons in the title row, Ctrl+Shift+wheel
//! (and Ctrl+Alt+wheel), rows up to 64 px, kept in ui.json, cents labels never overlapping.

mod vocal_support;

use egui::{Event, Id, Modifiers, MouseWheelUnit, Rect, TouchPhase, vec2};
use soundcraft_ui_egui::vocal_editor::{ROW_H, ROW_H_RANGE, ROW_H_STEP};
use vocal_support::*;

fn row_h(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(Id::new("soundcraft-vocal-row-h"))).unwrap()
}

#[test]
fn the_buttons_step_the_row_height_within_bounds() {
    let Setup { ctx, mut app, .. } = setup();
    let (lo, hi) = ROW_H_RANGE;
    assert!(hi >= 48.0, "a row can be at least 48 px");
    assert_eq!(row_h(&ctx), ROW_H);
    click(&ctx, &mut app, control(&ctx, "row-taller").unwrap().center());
    assert!((row_h(&ctx) - ROW_H * ROW_H_STEP).abs() < 1e-3, "{}", row_h(&ctx));
    assert!((view(&ctx).row_h - row_h(&ctx)).abs() < 1e-3, "the roll draws at it");
    for _ in 0..20 {
        click(&ctx, &mut app, control(&ctx, "row-taller").unwrap().center());
    }
    assert_eq!(row_h(&ctx), hi, "stops at the top");
    for _ in 0..30 {
        click(&ctx, &mut app, control(&ctx, "row-shorter").unwrap().center());
    }
    assert_eq!(row_h(&ctx), lo, "stops at the bottom");
}

#[test]
fn the_row_height_persists_in_ui_json() {
    let Setup { ctx, mut app, .. } = setup();
    click(&ctx, &mut app, control(&ctx, "row-taller").unwrap().center());
    let text = serde_json::to_string(&app.ui).unwrap();
    assert!(text.contains("\"vocal.row_h\":25.0"), "{text}");
    let back: soundcraft_ui_egui::UiState = serde_json::from_str(&text).unwrap();
    assert_eq!(back.vocal_row_h, 25.0);
    // A ui.json from before the setting: the default.
    let old: soundcraft_ui_egui::UiState = serde_json::from_str(&text.replace("\"vocal.row_h\":25.0,", "")).unwrap();
    assert_eq!(old.vocal_row_h, ROW_H);
    // A restored value is what the roll draws at.
    app.ui.vocal_row_h = 40.0;
    settle(&ctx, &mut app);
    assert_eq!(view(&ctx).row_h, 40.0);
}

fn wheel(ctx: &egui::Context, app: &mut soundcraft_ui_egui::SoundApp, m: Modifiers, dy: f32) {
    let over = view(ctx).rect.center();
    frame_m(
        ctx,
        app,
        vec![
            Event::PointerMoved(over),
            Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, dy), phase: TouchPhase::Move, modifiers: m },
        ],
        m,
    );
    settle(ctx, app);
}

#[test]
fn ctrl_shift_and_ctrl_alt_wheel_change_the_row_height() {
    let Setup { ctx, mut app, .. } = setup();
    wheel(&ctx, &mut app, Modifiers::CTRL | Modifiers::SHIFT, 100.0);
    let a = row_h(&ctx);
    assert!(a > ROW_H, "Ctrl+Shift+wheel up: taller ({a})");
    assert_eq!(app.ui.vocal_row_h, a, "kept for ui.json");
    wheel(&ctx, &mut app, Modifiers::CTRL | Modifiers::ALT, -100.0);
    assert!(row_h(&ctx) < a, "Ctrl+Alt+wheel down: shorter");
    let b = row_h(&ctx);
    wheel(&ctx, &mut app, Modifiers::CTRL, 100.0);
    assert_eq!(row_h(&ctx), b, "Ctrl+wheel alone zooms time, not rows");
}

#[test]
fn cents_labels_never_overlap_at_the_tallest_rows() {
    let Setup { ctx, mut app, .. } = setup();
    for h in [ROW_H, 40.0, ROW_H_RANGE.1] {
        app.ui.vocal_row_h = h;
        settle(&ctx, &mut app);
        let labels: Vec<Rect> = ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-cent-labels"))).unwrap();
        assert!(!labels.is_empty(), "labels drawn at {h}");
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} overlaps {b:?} at {h} px rows");
            }
        }
    }
}
