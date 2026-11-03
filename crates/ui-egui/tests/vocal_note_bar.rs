//! The note bar under the selected note: its place and fields, a sideways drag per field (one
//! undo step, Shift fine), the same delta on every selected note, double-click to type, Esc.

mod vocal_support;

use egui::{Event, Key, Modifiers, Pos2, vec2};
use serde_json::json;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::vocal_editor::note_bar::{BAR_H, BarField, FIELDS, dragged_values};
use soundcraft_vocal::NoteEdit;
use vocal_support::*;

fn edit(app: &SoundApp, r: NoteRef) -> NoteEdit {
    let s = app.engine.session();
    s.find_clip(r.clip).and_then(|(_, c)| c.tune_live()).and_then(|t| t.edits.get(&r.note)).cloned().unwrap_or_default()
}

fn field(ctx: &egui::Context, f: BarField) -> egui::Rect {
    control(ctx, &format!("bar-{}", f.key())).unwrap_or_else(|| panic!("no {} field", f.key()))
}

fn value(ctx: &egui::Context, f: BarField) -> String {
    text(ctx, &format!("bar-value-{}", f.key())).unwrap_or_default()
}

/// Drags from `from` by `dx` px sideways over 8 frames with `m` held.
fn drag_x(ctx: &egui::Context, app: &mut SoundApp, from: Pos2, dx: f32, m: Modifiers) {
    let to = from + vec2(dx, 0.0);
    frame_m(ctx, app, vec![Event::PointerMoved(from)], m);
    frame_m(ctx, app, vec![button(from, true, m)], m);
    for k in 1..=8 {
        frame_m(ctx, app, vec![Event::PointerMoved(from.lerp(to, k as f32 / 8.0))], m);
    }
    frame_m(ctx, app, vec![button(to, false, m)], m);
    settle(ctx, app);
}

fn double_click(ctx: &egui::Context, app: &mut SoundApp, at: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    for _ in 0..2 {
        frame(ctx, app, vec![button(at, true, Modifiers::NONE)]);
        frame(ctx, app, vec![button(at, false, Modifiers::NONE)]);
    }
    settle(ctx, app);
}

#[test]
fn field_maths() {
    assert_eq!(BarField::Cents.text(-5.0), "\u{2212}5c");
    assert_eq!(BarField::Semi.text(0.0), "+0 st");
    assert_eq!(BarField::Semi.text(-2.0), "\u{2212}2 st");
    assert_eq!(BarField::Drift.text(35.0), "35%");
    assert_eq!(BarField::Vibrato.text(0.0), "0%");
    assert_eq!(BarField::Vibrato.text(-10.0), "\u{2212}10%");
    assert_eq!(BarField::Level.text(-1.5), "\u{2212}1.5 dB");
    assert_eq!(BarField::Level.text(0.0), "0.0 dB");
    // A pixel: 1 c, 0.1 st, 1 %, 1 %, 0.1 dB.
    assert_eq!(FIELDS.map(BarField::per_px), [1.0, 0.1, 1.0, 1.0, 0.1]);
    // Clamped to each range; the same delta on every note.
    assert_eq!(dragged_values(BarField::Drift, &[30.0, 95.0], 10.0), vec![40.0, 100.0]);
    assert_eq!(dragged_values(BarField::Level, &[0.0, 11.5], 1.0), vec![1.0, 12.0]);
    assert_eq!(dragged_values(BarField::Cents, &[2390.0], 20.0), vec![2400.0]);
    assert_eq!(BarField::Semi.param(2.0), ("offset", json!(200.0)));
    assert_eq!(BarField::Level.param(-1.5), ("gain_db", json!(-1.5)));
    // Typed values take the unit or not, and either minus.
    assert_eq!(BarField::Cents.parse("\u{2212}15c"), Some(-15.0));
    assert_eq!(BarField::Level.parse(" -2.5 dB "), Some(-2.5));
    assert_eq!(BarField::Drift.parse("140"), Some(100.0));
    assert_eq!(BarField::Drift.parse("lots"), None);
    let e = NoteEdit { offset_cents: 249.0, drift: 0.35, gain_db: -1.5, ..NoteEdit::default() };
    assert_eq!(FIELDS.map(|f| f.of(&e)), [249.0, 2.0, 35.0, 0.0, -1.5]);
}

#[test]
fn the_bar_sits_under_the_selected_note_and_hides_without_one_or_off_screen() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    assert!(control(&ctx, "note-bar").is_none(), "nothing selected: no bar");
    click(&ctx, &mut app, bar(&ctx, ns[1]).center());
    let b = bar(&ctx, ns[1]);
    let nb = control(&ctx, "note-bar").expect("the bar under the selected note");
    assert!((nb.left() - b.left()).abs() < 0.5 && nb.top() > b.bottom() && nb.top() < b.bottom() + 30.0, "{nb:?} under {b:?}");
    assert_eq!(nb.height(), BAR_H);
    for f in FIELDS {
        let r = field(&ctx, f);
        assert!(nb.contains_rect(r), "{} inside the bar", f.key());
    }
    assert_eq!(value(&ctx, BarField::Cents), "\u{b1}0c");
    assert_eq!(value(&ctx, BarField::Semi), "+0 st");
    assert_eq!(value(&ctx, BarField::Drift), "0%");
    assert_eq!(value(&ctx, BarField::Vibrato), "0%");
    assert_eq!(value(&ctx, BarField::Level), "0.0 dB");
    assert_eq!(text(&ctx, "bar-count").as_deref(), Some(""));
    // The Edit view scrolls the note off screen: the bar goes; back, it returns.
    let scroll = app.engine.session().edit.zoom.scroll;
    app.engine.session_mut().edit.zoom.scroll = 60 * i64::from(RATE);
    settle(&ctx, &mut app);
    assert!(control(&ctx, "note-bar").is_none(), "off screen: no bar");
    app.engine.session_mut().edit.zoom.scroll = scroll;
    settle(&ctx, &mut app);
    assert!(control(&ctx, "note-bar").is_some());
    // The bar follows the first selected note.
    app.vocal.selected = ns.iter().copied().collect();
    settle(&ctx, &mut app);
    let nb = control(&ctx, "note-bar").unwrap();
    assert!((nb.left() - bar(&ctx, ns[0]).left()).abs() < 0.5, "under the first selected note");
    assert_eq!(text(&ctx, "bar-count").as_deref(), Some("2 notes"));
}

#[test]
fn dragging_each_field_edits_its_parameter_in_one_undo_step() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    let undo = app.engine.undo_history().len();
    // 20 px to the right on each field.
    drag_x(&ctx, &mut app, field(&ctx, BarField::Cents).center(), 20.0, Modifiers::NONE);
    assert_eq!(edit(&app, ns[0]).offset_cents, 20.0, "cents: 1 c a pixel");
    assert_eq!(value(&ctx, BarField::Cents), "+20c");
    assert_eq!(app.engine.undo_history().len(), undo + 1, "one undo step");
    drag_x(&ctx, &mut app, field(&ctx, BarField::Semi).center(), 20.0, Modifiers::NONE);
    assert_eq!(edit(&app, ns[0]).offset_cents, 200.0, "semi: 0.1 st a pixel, whole semitones");
    assert_eq!(value(&ctx, BarField::Semi), "+2 st");
    assert_eq!(value(&ctx, BarField::Cents), "+200c", "cents follow");
    drag_x(&ctx, &mut app, field(&ctx, BarField::Drift).center(), 20.0, Modifiers::NONE);
    assert!((edit(&app, ns[0]).drift - 0.2).abs() < 1e-6, "drift: 1 % a pixel");
    drag_x(&ctx, &mut app, field(&ctx, BarField::Vibrato).center(), -20.0, Modifiers::NONE);
    assert!((edit(&app, ns[0]).vibrato + 0.2).abs() < 1e-6, "vibrato: 1 % a pixel");
    drag_x(&ctx, &mut app, field(&ctx, BarField::Level).center(), -20.0, Modifiers::NONE);
    assert!((edit(&app, ns[0]).gain_db + 2.0).abs() < 1e-6, "level: 0.1 dB a pixel");
    assert_eq!(value(&ctx, BarField::Level), "\u{2212}2.0 dB");
    assert_eq!(app.engine.undo_history().len(), undo + 5, "a step per gesture");
    // Shift: fine, ×0.1.
    drag_x(&ctx, &mut app, field(&ctx, BarField::Cents).center(), 20.0, Modifiers::SHIFT);
    assert_eq!(edit(&app, ns[0]).offset_cents, 202.0);
    // Undo takes a whole gesture back.
    assert!(app.engine.undo());
    assert_eq!(edit(&app, ns[0]).offset_cents, 200.0);
    assert!(app.engine.undo());
    assert_eq!(edit(&app, ns[0]).gain_db, 0.0);
}

#[test]
fn a_multi_selection_moves_by_the_same_delta_and_shows_the_mean() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.run("vocal.edit", json!({"notes": [ns[0]], "offset": 100, "drift": 40})).unwrap();
    app.vocal.selected = ns.iter().copied().collect();
    settle(&ctx, &mut app);
    assert_eq!(value(&ctx, BarField::Cents), "+50c", "the mean");
    assert_eq!(value(&ctx, BarField::Drift), "20%");
    let undo = app.engine.undo_history().len();
    drag_x(&ctx, &mut app, field(&ctx, BarField::Cents).center(), 20.0, Modifiers::NONE);
    assert_eq!((edit(&app, ns[0]).offset_cents, edit(&app, ns[1]).offset_cents), (120.0, 20.0));
    assert_eq!(app.engine.undo_history().len(), undo + 1, "one step for both notes");
    drag_x(&ctx, &mut app, field(&ctx, BarField::Drift).center(), 30.0, Modifiers::NONE);
    assert!((edit(&app, ns[0]).drift - 0.7).abs() < 1e-6 && (edit(&app, ns[1]).drift - 0.3).abs() < 1e-6);
    assert_eq!(value(&ctx, BarField::Drift), "50%");
    assert!(app.engine.undo());
    assert!((edit(&app, ns[0]).drift - 0.4).abs() < 1e-6 && edit(&app, ns[1]).drift == 0.0);
}

#[test]
fn double_click_types_a_value_and_esc_cancels() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    let undo = app.engine.undo_history().len();
    double_click(&ctx, &mut app, field(&ctx, BarField::Level).center());
    assert!(control(&ctx, "bar-edit").is_some(), "a text field opens: {:?}", app.vocal.bar_edit);
    assert!(app.vocal.bar_edit.is_some());
    frame(&ctx, &mut app, vec![Event::Text("-3.5".into())]);
    frame(&ctx, &mut app, key(Key::Enter, Modifiers::NONE));
    settle(&ctx, &mut app);
    assert_eq!(edit(&app, ns[0]).gain_db, -3.5, "typed: {:?}", app.vocal.bar_edit);
    assert!(app.vocal.bar_edit.is_none());
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    // Esc cancels the typing and leaves the selection alone.
    double_click(&ctx, &mut app, field(&ctx, BarField::Cents).center());
    frame(&ctx, &mut app, vec![Event::Text("40".into())]);
    frame(&ctx, &mut app, key(Key::Escape, Modifiers::NONE));
    settle(&ctx, &mut app);
    assert!(app.vocal.bar_edit.is_none());
    assert_eq!(edit(&app, ns[0]).offset_cents, 0.0, "cancelled");
    assert_eq!(app.vocal.selected.len(), 1, "Esc in the field keeps the selection");
    assert_eq!(app.engine.undo_history().len(), undo + 1);
}

#[test]
fn esc_mid_drag_puts_the_values_back() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    let undo = app.engine.undo_history().len();
    let from = field(&ctx, BarField::Cents).center();
    frame(&ctx, &mut app, vec![Event::PointerMoved(from)]);
    frame(&ctx, &mut app, vec![button(from, true, Modifiers::NONE)]);
    for k in 1..=6 {
        frame(&ctx, &mut app, vec![Event::PointerMoved(from + vec2(5.0 * k as f32, 0.0))]);
    }
    assert!(edit(&app, ns[0]).offset_cents > 10.0, "sent mid-drag: {}", edit(&app, ns[0]).offset_cents);
    frame(&ctx, &mut app, key(Key::Escape, Modifiers::NONE));
    assert_eq!(edit(&app, ns[0]).offset_cents, 0.0, "Esc puts it back");
    frame(&ctx, &mut app, vec![button(from + vec2(40.0, 0.0), false, Modifiers::NONE)]);
    settle(&ctx, &mut app);
    assert_eq!(edit(&app, ns[0]).offset_cents, 0.0);
    assert_eq!(app.engine.undo_history().len(), undo, "nothing to undo");
    assert_eq!(app.vocal.selected.len(), 1);
}
