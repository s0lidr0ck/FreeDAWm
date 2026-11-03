//! The Vocal Editor's zoom relative to the Edit window: the ¼×..4× control sets the roll's
//! samples per px to the Edit window's ÷ the factor, the anchor stays at the same x, the roll
//! follows the Edit window's zoom and scroll, a hand zoom (Ctrl+wheel) clears the selected
//! factor, Ctrl+Alt+Plus/Minus step it, and the setting persists in ui.json.

mod vocal_support;

use egui::{Event, Key, Modifiers, MouseWheelUnit, TouchPhase, vec2};
use serde_json::json;
use soundcraft_ui_egui::vocal_editor::{ZOOM_FACTORS, zoom_label};
use vocal_support::*;

fn spp(ctx: &egui::Context) -> f64 {
    let v = view(ctx);
    (v.t1 - v.t0) as f64 / f64::from(v.rect.width())
}

fn factor(ctx: &egui::Context) -> Option<f32> {
    ctx.data(|d| d.get_temp::<Option<f32>>(egui::Id::new("soundcraft-vocal-zoom-factor"))).flatten()
}

#[test]
fn labels_and_the_default() {
    assert_eq!(ZOOM_FACTORS.map(zoom_label), ["\u{bc}\u{d7}", "\u{bd}\u{d7}", "1\u{d7}", "2\u{d7}", "4\u{d7}"]);
    let Setup { ctx, app, .. } = setup();
    assert_eq!(app.ui.vocal_zoom_factor, 1.0);
    assert_eq!(factor(&ctx), Some(1.0), "1× lit by default");
    for f in ZOOM_FACTORS {
        assert!(control(&ctx, &format!("zoom-{}", zoom_label(f).trim_end_matches('\u{d7}'))).is_some(), "a {} button", zoom_label(f));
    }
    let edit = app.engine.session().edit.zoom.samples_per_px;
    assert!((spp(&ctx) - edit).abs() < 1e-3 * edit, "at 1× the roll is the Edit window's zoom: {} vs {edit}", spp(&ctx));
    assert_eq!(view(&ctx).t0, app.engine.session().edit.zoom.scroll, "and its scroll");
}

#[test]
fn a_factor_sets_the_zoom_relative_to_the_edit_window_and_keeps_the_anchor() {
    // The take at 4 s with the Edit view from 3 s: room on both sides of the anchor.
    let rate = i64::from(RATE);
    let Setup { ctx, mut app, track, .. } = setup_with(vec![(take2(), 4 * rate)], true);
    app.run("view.scroll", json!({"to": 3 * rate})).unwrap();
    settle(&ctx, &mut app);
    let ns = notes(&app, track);
    // The first selected note is the anchor.
    app.vocal.selected = [ns[1]].into_iter().collect();
    settle(&ctx, &mut app);
    let (start, _) = span(&app, ns[1]);
    let x_before = view(&ctx).x(start);
    assert!(x_before > view(&ctx).rect.left() && x_before < view(&ctx).rect.right(), "the anchor is in view");
    let edit = app.engine.session().edit.zoom.samples_per_px;
    click(&ctx, &mut app, control(&ctx, "zoom-2").unwrap().center());
    assert_eq!(app.ui.vocal_zoom_factor, 2.0);
    assert_eq!(factor(&ctx), Some(2.0));
    assert!((spp(&ctx) - edit / 2.0).abs() < 1e-3 * edit, "2×: half the samples per px: {} vs {}", spp(&ctx), edit / 2.0);
    assert!((view(&ctx).x(start) - x_before).abs() < 1.5, "the note stays at the same x: {} vs {x_before}", view(&ctx).x(start));
    assert_eq!(app.engine.session().edit.zoom.samples_per_px, edit, "the Edit window's zoom is untouched");
    click(&ctx, &mut app, control(&ctx, "zoom-\u{bd}").unwrap().center());
    assert!((spp(&ctx) - edit * 2.0).abs() < 1e-3 * edit, "½×: twice");
    assert!((view(&ctx).x(start) - x_before).abs() < 1.5, "still at the same x: {} vs {x_before}", view(&ctx).x(start));
    // The Edit window zooms: the roll follows at the factor.
    app.run("view.zoom_by", json!({"factor": 0.5, "x": 0.0})).unwrap();
    settle(&ctx, &mut app);
    let edit2 = app.engine.session().edit.zoom.samples_per_px;
    assert!((edit2 - edit * 2.0).abs() < 1e-3 * edit, "factor 0.5 zooms out: {edit} -> {edit2}");
    assert!((spp(&ctx) - edit2 * 2.0).abs() < 1e-3 * edit2, "follows the Edit window: {} vs {}", spp(&ctx), edit2 * 2.0);
    // And scrolls with it.
    let t0 = view(&ctx).t0;
    let scroll = app.engine.session().edit.zoom.scroll;
    app.run("view.scroll", json!({"to": scroll + 12_000})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(view(&ctx).t0, t0 + 12_000, "moved by the Edit window's scroll");
    // No selection: the playhead is the anchor.
    app.vocal.selected.clear();
    let v = view(&ctx);
    let ph = v.t0 + (v.t1 - v.t0) / 3;
    app.run("transport.locate", json!({"at": ph})).unwrap();
    settle(&ctx, &mut app);
    let xp = view(&ctx).x(ph);
    click(&ctx, &mut app, control(&ctx, "zoom-4").unwrap().center());
    assert!((view(&ctx).x(ph) - xp).abs() < 1.5, "the playhead stays put: {} vs {xp}", view(&ctx).x(ph));
    assert!((spp(&ctx) - edit2 / 4.0).abs() < 1e-3 * edit2);
}

#[test]
fn a_hand_zoom_clears_the_factor_and_the_keys_step_it() {
    let Setup { ctx, mut app, .. } = setup();
    let edit = app.engine.session().edit.zoom.samples_per_px;
    let v = view(&ctx);
    let at = v.rect.center();
    frame(&ctx, &mut app, vec![Event::PointerMoved(at)]);
    frame_m(
        &ctx,
        &mut app,
        vec![Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, -200.0), phase: TouchPhase::Move, modifiers: Modifiers::CTRL }],
        Modifiers::CTRL,
    );
    settle(&ctx, &mut app);
    assert!(app.vocal.hand_zoom);
    assert_eq!(factor(&ctx), None, "no button lit after a hand zoom");
    assert!(spp(&ctx) < edit, "zoomed in by hand: {} vs {edit}", spp(&ctx));
    assert_eq!(app.engine.session().edit.zoom.samples_per_px, edit, "the Edit window's zoom is untouched");
    let hand = spp(&ctx);
    // The Edit window zooming does not move a hand zoom.
    app.run("view.zoom_by", json!({"factor": 0.5, "x": 0.0})).unwrap();
    settle(&ctx, &mut app);
    assert!((spp(&ctx) - hand).abs() < 1e-6 * hand, "keeps its own zoom");
    // A factor clicked again: following once more.
    click(&ctx, &mut app, control(&ctx, "zoom-1").unwrap().center());
    assert!(!app.vocal.hand_zoom);
    let edit = app.engine.session().edit.zoom.samples_per_px;
    assert!((spp(&ctx) - edit).abs() < 1e-3 * edit);
    // Ctrl+Alt+Plus/Minus step the factor along ¼×..4×.
    let over = view(&ctx).rect.center();
    press(&ctx, &mut app, over, Key::Plus, Modifiers::CTRL | Modifiers::ALT);
    assert_eq!(app.ui.vocal_zoom_factor, 2.0);
    press(&ctx, &mut app, over, Key::Plus, Modifiers::CTRL | Modifiers::ALT);
    press(&ctx, &mut app, over, Key::Plus, Modifiers::CTRL | Modifiers::ALT);
    assert_eq!(app.ui.vocal_zoom_factor, 4.0, "stops at 4×");
    for _ in 0..5 {
        press(&ctx, &mut app, over, Key::Minus, Modifiers::CTRL | Modifiers::ALT);
    }
    assert_eq!(app.ui.vocal_zoom_factor, 0.25, "stops at ¼×");
    assert!((spp(&ctx) - edit * 4.0).abs() < 1e-3 * edit);
}

#[test]
fn the_factor_persists_in_ui_json() {
    let Setup { ctx, mut app, .. } = setup();
    click(&ctx, &mut app, control(&ctx, "zoom-2").unwrap().center());
    let text = serde_json::to_string(&app.ui).unwrap();
    assert!(text.contains("\"vocal.zoom_factor\":2.0"), "{text}");
    assert!(text.contains("\"vocal.preview\":true"), "{text}");
    let back: soundcraft_ui_egui::UiState = serde_json::from_str(&text).unwrap();
    assert_eq!(back.vocal_zoom_factor, 2.0);
    // A ui.json from before the setting: 1× and Preview on.
    let old: soundcraft_ui_egui::UiState =
        serde_json::from_str(&text.replace("\"vocal.zoom_factor\":2.0,", "").replace("\"vocal.preview\":true,", "")).unwrap();
    assert_eq!((old.vocal_zoom_factor, old.vocal_preview), (1.0, true));
}
