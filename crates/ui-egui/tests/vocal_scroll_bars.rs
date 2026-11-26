//! The Vocal Editor's scroll bars: time under the roll, pitch at its right, as the MIDI Editor's.
//! The thumbs are the share in view, dragging them moves the view, a click beside a thumb pages,
//! the wheel moves them, and nothing overlaps or panics in a short dock or without clips.

mod vocal_support;

use egui::{Event, Modifiers, MouseWheelUnit, Pos2, Rect, TouchPhase, vec2};
use serde_json::json;
use soundcraft_ui_egui::vocal_editor::scroll::Span;
use vocal_support::*;

/// Two takes 20 s apart: a long track, 3 s of it in view.
fn long_take() -> Setup {
    let rate = i64::from(RATE);
    setup_with(vec![(take2(), rate / 4), (take2(), 20 * rate)], true)
}

fn bars(ctx: &egui::Context) -> (Rect, Rect, Rect, Rect) {
    (control(ctx, "hbar").unwrap(), control(ctx, "vbar").unwrap(), control(ctx, "hbar-thumb").unwrap(), control(ctx, "vbar-thumb").unwrap())
}

fn spans(ctx: &egui::Context) -> (Span, Span) {
    ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-bar-spans"))).expect("the bars were drawn")
}

fn wheel(ctx: &egui::Context, app: &mut soundcraft_ui_egui::SoundApp, at: Pos2, m: Modifiers, d: egui::Vec2) {
    frame_m(ctx, app, vec![Event::PointerMoved(at)], m);
    frame_m(ctx, app, vec![Event::MouseWheel { unit: MouseWheelUnit::Point, delta: d, phase: TouchPhase::Move, modifiers: m }], m);
    settle(ctx, app);
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.intersect(b).is_positive()
}

#[test]
fn both_bars_are_there_beside_the_roll_and_overlap_nothing() {
    let Setup { ctx, .. } = long_take();
    let (h, v, ht, vt) = bars(&ctx);
    let roll = view(&ctx).rect;
    let lane = control(&ctx, "lane").unwrap();
    assert!(h.top() >= roll.bottom() && h.bottom() <= lane.top(), "the time bar between the roll and the Level lane: {h:?} {roll:?} {lane:?}");
    assert!((h.left() - roll.left()).abs() < 0.5 && (h.right() - roll.right()).abs() < 0.5, "as wide as the roll");
    assert!(
        v.left() >= roll.right() && (v.top() - roll.top()).abs() < 0.5 && (v.bottom() - roll.bottom()).abs() < 0.5,
        "the pitch bar at the roll's right: {v:?}"
    );
    let tuning = control(&ctx, "tuning").unwrap();
    for r in [h, v] {
        assert!(!overlap(r, roll) && !overlap(r, lane), "{r:?}");
        assert!(r.width() > 0.0 && r.height() > 0.0);
    }
    // The Tuning panel starts right of the pitch bar (its toggle lit, the panel shown).
    let panel_left =
        ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("soundcraft-vocal-area"))).unwrap().right() - soundcraft_ui_egui::vocal_editor::PANEL_W;
    assert!(v.right() <= panel_left + 0.5, "{v:?} vs the panel at {panel_left}");
    assert!(tuning.is_positive());
    assert!(h.contains_rect(ht) && v.contains_rect(vt), "the thumbs inside their bars");
    assert!(ht.width() < h.width() * 0.5, "3 s of a 21 s track: a short thumb ({} of {})", ht.width(), h.width());
}

#[test]
fn the_thumbs_are_the_share_in_view_and_follow_the_zoom() {
    let Setup { ctx, mut app, .. } = long_take();
    let check = |ctx: &egui::Context| {
        let (h, v, ht, vt) = bars(ctx);
        let (hs, vs) = spans(ctx);
        let rv = view(ctx);
        assert!((hs.len - (rv.t1 - rv.t0) as f64).abs() < 2.0, "the time span is the view's");
        assert!((ht.width() / h.width() - hs.fraction() as f32).abs() < 0.01, "{} vs {}", ht.width() / h.width(), hs.fraction());
        let rows = rv.rect.height() / rv.row_h;
        assert!((vs.len as f32 - rows).abs() < 0.01 && (vs.hi - vs.lo - 128.0).abs() < 1e-6, "the pitch span is the rows in view of 128");
        assert!((vt.height() / v.height() - vs.fraction() as f32).abs() < 0.01);
        (ht.width(), vt.height())
    };
    let (w0, h0) = check(&ctx);
    // Ctrl+wheel up zooms time in: a shorter thumb.
    let at = view(&ctx).rect.center();
    wheel(&ctx, &mut app, at, Modifiers::CTRL, vec2(0.0, 200.0));
    let (w1, _) = check(&ctx);
    assert!(w1 < w0 * 0.8, "zoomed in: {w0} -> {w1}");
    // Taller rows: fewer in view, a shorter pitch thumb.
    wheel(&ctx, &mut app, at, Modifiers::CTRL | Modifiers::SHIFT, vec2(0.0, 100.0));
    let (_, h1) = check(&ctx);
    assert!(h1 < h0, "taller rows: {h0} -> {h1}");
}

#[test]
fn dragging_the_time_thumb_moves_the_time_in_view() {
    let Setup { ctx, mut app, .. } = long_take();
    let t0 = view(&ctx).t0;
    let (h, _, ht, _) = bars(&ctx);
    let per_px = (spans(&ctx).0.hi - spans(&ctx).0.lo) / f64::from(h.width());
    drag(&ctx, &mut app, ht.center(), ht.center() + vec2(100.0, 0.0), 5);
    let moved = view(&ctx).t0 - t0;
    assert!(moved as f64 > 80.0 * per_px && (moved as f64) < 120.0 * per_px, "about 100 px of the bar: {moved} samples ({per_px}/px)");
    assert!(app.engine.session().edit.zoom.scroll > 0, "linked: the Edit window scrolled with it");
    // Back to the start, and no further.
    let (_, _, ht, _) = bars(&ctx);
    drag(&ctx, &mut app, ht.center(), ht.center() - vec2(2000.0, 0.0), 5);
    assert_eq!(view(&ctx).t0, 0);
    // Unlinked, the roll moves on its own.
    app.run("ui.vocal_editor", json!({"linked": false})).unwrap();
    settle(&ctx, &mut app);
    let edit = app.engine.session().edit.zoom.scroll;
    let (_, _, ht, _) = bars(&ctx);
    drag(&ctx, &mut app, ht.center(), ht.center() + vec2(60.0, 0.0), 3);
    assert!(view(&ctx).t0 > 0);
    assert_eq!(app.engine.session().edit.zoom.scroll, edit, "the Edit window stays");
}

#[test]
fn dragging_the_pitch_thumb_moves_the_pitches_in_view() {
    let Setup { ctx, mut app, .. } = long_take();
    let top = view(&ctx).top_cents;
    let (_, v, _, vt) = bars(&ctx);
    drag(&ctx, &mut app, vt.center(), vt.center() + vec2(0.0, 40.0), 4);
    let rows = (top - view(&ctx).top_cents) / 100.0;
    let want = 40.0 * 128.0 / v.height();
    assert!((rows - want).abs() < 1.0, "down {want} rows: {rows}");
    drag(&ctx, &mut app, bars(&ctx).3.center(), bars(&ctx).3.center() - vec2(0.0, 2000.0), 4);
    assert_eq!(view(&ctx).top_cents, 12_800.0, "up to the top row and no further");
}

#[test]
fn a_click_beside_a_thumb_pages() {
    let Setup { ctx, mut app, .. } = long_take();
    let rv = view(&ctx);
    let span = rv.t1 - rv.t0;
    let (h, v, _, vt) = bars(&ctx);
    click(&ctx, &mut app, h.right_center() - vec2(4.0, 0.0));
    let moved = view(&ctx).t0 - rv.t0;
    assert!((moved - span).abs() < span / 50, "one view later: {moved} vs {span}");
    // Below the pitch thumb: one view of rows down.
    let rows = rv.rect.height() / rv.row_h;
    let top = view(&ctx).top_cents;
    let below = Pos2::new(v.center().x, (vt.bottom() + v.bottom()) / 2.0);
    assert!(below.y > vt.bottom());
    click(&ctx, &mut app, below);
    let down = (top - view(&ctx).top_cents) / 100.0;
    assert!(down > 0.0 && down <= rows + 0.01, "{down} of {rows} rows");
}

#[test]
fn the_wheel_moves_the_thumbs_and_works_over_the_bars() {
    let Setup { ctx, mut app, .. } = long_take();
    let (_, _, ht, vt) = bars(&ctx);
    let at = view(&ctx).rect.center();
    // Plain wheel down: lower pitches, the pitch thumb moves down.
    wheel(&ctx, &mut app, at, Modifiers::NONE, vec2(0.0, -200.0));
    let (_, _, ht2, vt2) = bars(&ctx);
    assert!(vt2.top() > vt.top() + 1.0, "{vt:?} -> {vt2:?}");
    assert_eq!(ht2, ht, "time unchanged");
    // Shift+wheel down: later, the time thumb moves right.
    wheel(&ctx, &mut app, at, Modifiers::SHIFT, vec2(0.0, -200.0));
    let (h, _, ht3, _) = bars(&ctx);
    assert!(ht3.left() > ht2.left(), "{ht2:?} -> {ht3:?}");
    // Over the time bar itself, as over the roll.
    wheel(&ctx, &mut app, h.center(), Modifiers::SHIFT, vec2(0.0, -200.0));
    assert!(bars(&ctx).2.left() > ht3.left());
}

#[test]
fn a_short_dock_and_a_track_without_clips_do_not_break_the_bars() {
    let Setup { ctx, mut app, .. } = long_take();
    let mut path = Vec::new();
    assert!(vocal_split(app.ui.docking.layout.root.as_ref().unwrap(), &mut path));
    for h in [160.0, 90.0, 40.0] {
        app.run("ui.dock", json!({"operation": "resizeSplit", "path": path, "size": {"FixedSecond": h}})).unwrap();
        settle(&ctx, &mut app);
        if let (Some(hb), Some(vb), Some(lane)) = (control(&ctx, "hbar"), control(&ctx, "vbar"), control(&ctx, "lane")) {
            let roll = view(&ctx).rect;
            assert!(!overlap(hb, roll) && !overlap(vb, roll) && !overlap(hb, lane) && !overlap(vb, lane), "{h}: {hb:?} {vb:?} {roll:?} {lane:?}");
        }
    }
    // No clips at all: the editor says so, nothing panics.
    let Setup { ctx, mut app, .. } = setup_with(vec![], false);
    settle(&ctx, &mut app);
    assert!(text(&ctx, "message").is_some());
}

#[test]
fn spans_are_safe() {
    let s = Span::new(0.0, 0.0, 0.0, 0.0);
    assert!(s.fraction().is_finite());
    let (a, b) = s.thumb(10.0, 10.0);
    assert!(a.is_finite() && b.is_finite());
}
