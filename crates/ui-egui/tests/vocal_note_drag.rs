//! Dragging notes up and down in the Vocal Editor: 50 c a row, snapped onto a semitone within
//! ±15 c (Alt: no snap), a dead zone so a click never nudges pitch, the selection moved together, one undo
//! step per drag (none for a drag that ends where it began or is cancelled with Esc), Alt+Up/Down
//! steps, and the Tuning panel's "moved" line.

mod vocal_support;

use egui::{Event, Key, Modifiers, Pos2, pos2, vec2};
use soundcraft_engine::vocal::NoteRef;
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::vocal_editor::drag::{CENTS_PER_ROW, SNAP_CENTS, drag_offsets, readout_text, snap_target};
use soundcraft_vocal::{NoteEdit, tuned_centre};
use vocal_support::*;

fn edit(app: &SoundApp, r: NoteRef) -> NoteEdit {
    let s = app.engine.session();
    s.find_clip(r.clip).and_then(|(_, c)| c.tune_live()).and_then(|t| t.edits.get(&r.note)).cloned().unwrap_or_default()
}

/// The note's tuned centre (cents) as the engine computes it.
fn tuned(app: &SoundApp, r: NoteRef) -> f32 {
    let s = app.engine.session();
    let (_, c) = s.find_clip(r.clip).unwrap();
    let a = soundcraft_engine::vocal::analysis_for(s, c).unwrap();
    let n = a.notes.iter().find(|n| n.id == r.note).unwrap();
    tuned_centre(&a, n, &edit(app, r), &soundcraft_engine::vocal::scale_at(s, c))
}

/// Drags from `from` to `to` over `steps` frames with `m` held; `mid` runs once halfway.
fn drag_m(ctx: &egui::Context, app: &mut SoundApp, from: Pos2, to: Pos2, steps: usize, m: Modifiers, mut mid: impl FnMut(&egui::Context, &SoundApp)) {
    frame_m(ctx, app, vec![Event::PointerMoved(from)], m);
    frame_m(ctx, app, vec![button(from, true, m)], m);
    for k in 1..=steps {
        frame_m(ctx, app, vec![Event::PointerMoved(from.lerp(to, k as f32 / steps as f32))], m);
        if k == steps / 2 {
            mid(ctx, app);
        }
    }
    frame_m(ctx, app, vec![button(to, false, m)], m);
    settle(ctx, app);
}

#[test]
fn drag_offsets_move_in_cents_and_snap_within_15_cents_of_a_semitone() {
    assert_eq!((CENTS_PER_ROW, SNAP_CENTS), (50.0, 15.0));
    // A −41 c note (6759) dragged 200 c lands 41 c off 7000: too far to snap, so it moves 200.
    let base = [(0.0, 6759.0), (0.0, 6820.0)];
    assert_eq!(drag_offsets(&base, 200.0, true), vec![200.0, 200.0]);
    // 230 c up puts it 11 c under 7000: snapped onto it (241); the +20 c note stays free (230).
    assert_eq!(drag_offsets(&base, 230.0, true), vec![241.0, 230.0]);
    // The snap engages at 15 c and releases past it.
    assert_eq!(drag_offsets(&[(0.0, 6759.0)], 226.0, true), vec![241.0]);
    assert_eq!(drag_offsets(&[(0.0, 6759.0)], 225.0, true), vec![225.0]);
    assert_eq!(drag_offsets(&[(0.0, 6759.0)], 256.0, true), vec![241.0]);
    assert_eq!(drag_offsets(&[(0.0, 6759.0)], 257.0, true), vec![257.0]);
    assert_eq!(snap_target(6759.0, 230.0), Some(7000.0));
    assert_eq!(snap_target(6759.0, 200.0), None);
    // No snap (Alt): 1-cent steps on top of any earlier move.
    assert_eq!(drag_offsets(&[(10.0, 6759.0)], -35.4, false), vec![-25.0]);
    assert_eq!(drag_offsets(&[(0.0, 6759.0)], 241.0, false), vec![241.0]);
    // Clamped to two octaves; a non-finite move moves nothing.
    assert_eq!(drag_offsets(&[(2300.0, 6800.0)], 500.0, false), vec![2400.0]);
    assert_eq!(drag_offsets(&[(0.0, 6800.0)], f32::NAN, true), vec![0.0]);
    // The readout: the tuned centre against its nearest semitone, and that semitone's name.
    assert_eq!(readout_text(6895.0), "\u{2212}5c \u{b7} A4");
    assert_eq!(readout_text(6912.0), "+12c \u{b7} A4");
    assert_eq!(readout_text(7000.0), "\u{b1}0c \u{b7} A#4");
}

#[test]
fn dragging_up_towards_a_semitone_two_up_snaps_the_tuned_centre_onto_it() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let before = tuned(&app, ns[0]);
    let target = (before / 100.0).round() * 100.0;
    let b = bar(&ctx, ns[0]);
    let row = view(&ctx).row_h;
    let undo = app.engine.undo_history().len();
    // Up to 8 c under the semitone two up: 50 c a row, and the last 8 c are the snap's.
    let d = target + 200.0 - 8.0 - before;
    let to = b.center() - vec2(0.0, d / CENTS_PER_ROW * row);
    let mut mid_readout = None;
    let mut mid_bar = b;
    let mut mid_offset = 0.0;
    drag_m(&ctx, &mut app, b.center(), to, 10, Modifiers::NONE, |ctx, app| {
        mid_readout = text(ctx, "drag-readout");
        mid_bar = bar(ctx, ns[0]);
        mid_offset = edit(app, ns[0]).offset_cents;
    });
    let mid = mid_readout.expect("a readout mid-drag");
    assert!(
        mid.contains("c \u{b7} ") && mid.ends_with(&soundcraft_ui_egui::vocal_style::note_name(target + 100.0)),
        "the readout follows the pointer: {mid}"
    );
    assert!(mid_bar.center().y < b.center().y - 0.5 * row, "the bar moves with the pointer: {mid_bar:?} from {b:?}");
    assert!(mid_offset > 0.0, "the drag previews: offset {mid_offset} mid-drag");
    let after = tuned(&app, ns[0]);
    assert!((after - (target + 200.0)).abs() < 0.01, "tuned centre {before} -> {after}: snapped onto the semitone");
    assert_eq!(edit(&app, ns[1]).offset_cents, 0.0, "the other note stays");
    assert_eq!(app.engine.undo_history().len(), undo + 1, "one drag, one undo step");
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[0]], "the dragged note is selected");
    // The bar is redrawn two rows up, on its new semitone: placed from the engine's tuned centre,
    // so it reads 0 c there (the mean of its smoothed curve would fall a few cents short).
    let moved = shapes(&ctx).into_iter().find(|s| s.r == ns[0]).unwrap();
    assert!(moved.tuned_offset.abs() < 0.01 && (moved.target - (target + 200.0)).abs() < 0.01, "{moved:?}");
    assert!((view(&ctx).y(target + 200.0) - moved.rect.center().y).abs() < 0.5, "the bar sits on the row: {moved:?}");
    assert!(app.engine.undo());
    assert_eq!(edit(&app, ns[0]).offset_cents, 0.0);
}

#[test]
fn a_drag_back_to_where_it_began_moves_nothing_and_leaves_no_undo_step() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    // Something to redo: a step taken and undone before the drag.
    app.vocal.selected = [ns[1]].into_iter().collect();
    settle(&ctx, &mut app);
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::ArrowUp, Modifiers::ALT);
    assert!(app.engine.undo());
    assert!(app.engine.can_redo());
    settle(&ctx, &mut app);
    let b = bar(&ctx, ns[0]);
    let row = view(&ctx).row_h;
    let undo = app.engine.undo_history().len();
    let up = b.center() - vec2(0.0, 2.0 * row);
    frame_m(&ctx, &mut app, vec![Event::PointerMoved(b.center())], Modifiers::NONE);
    frame_m(&ctx, &mut app, vec![button(b.center(), true, Modifiers::NONE)], Modifiers::NONE);
    for k in 1..=4 {
        frame_m(&ctx, &mut app, vec![Event::PointerMoved(b.center().lerp(up, k as f32 / 4.0))], Modifiers::NONE);
    }
    assert!(edit(&app, ns[0]).offset_cents > 60.0, "most of two rows (100 c) up has been sent: {}", edit(&app, ns[0]).offset_cents);
    // The pointer leaves the window: the notes stay where they are, no jump to the origin.
    frame_m(&ctx, &mut app, vec![Event::PointerGone], Modifiers::NONE);
    assert!(edit(&app, ns[0]).offset_cents > 60.0, "pointer gone keeps the last move: {}", edit(&app, ns[0]).offset_cents);
    let readout = text(&ctx, "drag-readout").unwrap_or_default();
    assert!(readout.contains("c \u{b7} "), "{readout}");
    // Back to where it began and released: the offset is as it was, not a snap.
    for k in 1..=4 {
        frame_m(&ctx, &mut app, vec![Event::PointerMoved(up.lerp(b.center(), k as f32 / 4.0))], Modifiers::NONE);
    }
    assert!(text(&ctx, "drag-readout").is_some_and(|t| t.contains("c \u{b7} ")));
    frame_m(&ctx, &mut app, vec![button(b.center(), false, Modifiers::NONE)], Modifiers::NONE);
    settle(&ctx, &mut app);
    assert_eq!(edit(&app, ns[0]), NoteEdit::default());
    assert_eq!(app.engine.undo_history().len(), undo, "nothing to undo");
    assert!(app.engine.can_redo(), "a no-op drag keeps what there was to redo");
}

#[test]
fn escape_mid_drag_cancels_the_move() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.vocal.selected = [ns[0]].into_iter().collect();
    settle(&ctx, &mut app);
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::ArrowDown, Modifiers::ALT);
    assert!(app.engine.undo());
    assert!(app.engine.can_redo());
    settle(&ctx, &mut app);
    let b = bar(&ctx, ns[0]);
    let row = view(&ctx).row_h;
    let undo = app.engine.undo_history().len();
    let up = b.center() - vec2(0.0, 2.0 * row);
    frame_m(&ctx, &mut app, vec![Event::PointerMoved(b.center())], Modifiers::NONE);
    frame_m(&ctx, &mut app, vec![button(b.center(), true, Modifiers::NONE)], Modifiers::NONE);
    for k in 1..=6 {
        frame_m(&ctx, &mut app, vec![Event::PointerMoved(b.center().lerp(up, k as f32 / 6.0))], Modifiers::NONE);
    }
    assert!(edit(&app, ns[0]).offset_cents > 60.0, "two rows up: {}", edit(&app, ns[0]).offset_cents);
    frame_m(&ctx, &mut app, key(Key::Escape, Modifiers::NONE), Modifiers::NONE);
    assert_eq!(edit(&app, ns[0]), NoteEdit::default(), "Esc puts the note back");
    assert!(app.vocal.drag.is_none());
    // The pointer, still down, moves on and lets go: nothing follows it.
    frame_m(&ctx, &mut app, vec![Event::PointerMoved(up - vec2(0.0, row))], Modifiers::NONE);
    frame_m(&ctx, &mut app, vec![button(up - vec2(0.0, row), false, Modifiers::NONE)], Modifiers::NONE);
    settle(&ctx, &mut app);
    assert_eq!(edit(&app, ns[0]), NoteEdit::default());
    assert_eq!(app.engine.undo_history().len(), undo, "nothing to undo");
    assert!(app.engine.can_redo(), "a cancelled drag keeps what there was to redo");
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[0]], "Esc mid-drag keeps the selection");
}

#[test]
fn escape_off_the_editor_cancels_and_the_next_escape_over_it_still_clears() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let b = bar(&ctx, ns[0]);
    let row = view(&ctx).row_h;
    let undo = app.engine.undo_history().len();
    frame_m(&ctx, &mut app, vec![Event::PointerMoved(b.center())], Modifiers::NONE);
    frame_m(&ctx, &mut app, vec![button(b.center(), true, Modifiers::NONE)], Modifiers::NONE);
    for k in 1..=4 {
        frame_m(&ctx, &mut app, vec![Event::PointerMoved(b.center().lerp(b.center() - vec2(0.0, 2.0 * row), k as f32 / 4.0))], Modifiers::NONE);
    }
    assert!(edit(&app, ns[0]).offset_cents > 60.0);
    // Dragged out of the editor (the track list, top left) and Esc pressed there.
    let outside = pos2(5.0, 5.0);
    frame_m(&ctx, &mut app, vec![Event::PointerMoved(outside)], Modifiers::NONE);
    frame_m(&ctx, &mut app, key(Key::Escape, Modifiers::NONE), Modifiers::NONE);
    assert_eq!(edit(&app, ns[0]), NoteEdit::default(), "cancelled off the editor");
    assert!(app.vocal.drag.is_none());
    frame_m(&ctx, &mut app, vec![button(outside, false, Modifiers::NONE)], Modifiers::NONE);
    settle(&ctx, &mut app);
    assert_eq!(app.engine.undo_history().len(), undo);
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[0]]);
    // The next Esc over the editor clears the selection as ever: nothing stale swallows it.
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::Escape, Modifiers::NONE);
    assert!(app.vocal.selected.is_empty(), "Esc over the editor clears the selection");
}

#[test]
fn the_snap_engages_within_15_cents_and_alt_turns_it_off() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let before = tuned(&app, ns[0]);
    let target = (before / 100.0).round() * 100.0;
    let b = bar(&ctx, ns[0]);
    let row = view(&ctx).row_h;
    // The note sits 30 c under its semitone: 0.4 rows (20 c) up leaves it 10 c under, within the
    // snap, so it lands on the semitone (offset 30) and the key column shows the tick.
    let to = b.center() - vec2(-9.0, 0.4 * row);
    let mut snapped = None;
    drag_m(&ctx, &mut app, b.center(), to, 6, Modifiers::NONE, |_, _| {});
    frame_m(&ctx, &mut app, vec![], Modifiers::NONE);
    assert!((tuned(&app, ns[0]) - target).abs() < 0.01, "snapped onto {target}: {}", tuned(&app, ns[0]));
    assert!((edit(&app, ns[0]).offset_cents - (target - before)).abs() < 1.0);
    assert!(app.engine.undo());
    settle(&ctx, &mut app);
    // The same drag with Alt: 20 c, no snap, no tick.
    drag_m(&ctx, &mut app, b.center(), to, 6, Modifiers::ALT, |ctx, _| {
        snapped = ctx.data(|d| d.get_temp::<Option<f32>>(egui::Id::new("soundcraft-vocal-drag-snapped"))).flatten();
    });
    assert_eq!(edit(&app, ns[0]).offset_cents, 20.0);
    assert_eq!(snapped, None, "Alt: no snap tick");
    assert!(app.engine.undo());
    settle(&ctx, &mut app);
    // A row up (50 c) takes it 20 c past the semitone: released from the snap.
    let mut tick = None;
    drag_m(&ctx, &mut app, b.center(), b.center() - vec2(0.0, row), 10, Modifiers::NONE, |ctx, _| {
        // Halfway (25 c up: 5 c under the semitone) the snap holds and the tick shows.
        tick = ctx.data(|d| d.get_temp::<Option<f32>>(egui::Id::new("soundcraft-vocal-drag-snapped"))).flatten();
    });
    assert_eq!(tick, Some(target), "the tick on the key column while snapped");
    assert_eq!(edit(&app, ns[0]).offset_cents, 50.0, "past the snap: free again");
}

#[test]
fn alt_drag_moves_in_cents() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let b = bar(&ctx, ns[0]);
    let row = view(&ctx).row_h;
    // Half a row down (25 c), with a sideways wobble that moves nothing.
    let to = b.center() + vec2(9.0, 0.5 * row);
    drag_m(&ctx, &mut app, b.center(), to, 6, Modifiers::ALT, |_, _| {});
    assert_eq!(edit(&app, ns[0]).offset_cents, -25.0);
}

#[test]
fn a_click_or_a_sideways_drag_changes_no_pitch() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let b = bar(&ctx, ns[0]);
    let undo = app.engine.undo_history().len();
    click(&ctx, &mut app, b.center());
    // Inside the 4 px dead zone up or down, far enough sideways to be a drag.
    drag_m(&ctx, &mut app, b.center(), b.center() + vec2(30.0, -3.0), 6, Modifiers::NONE, |_, _| {});
    assert_eq!(edit(&app, ns[0]), NoteEdit::default());
    assert_eq!(app.engine.undo_history().len(), undo, "nothing to undo");
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[0]]);
}

#[test]
fn dragging_a_selected_note_moves_the_selection() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.vocal.selected = ns.iter().copied().collect();
    settle(&ctx, &mut app);
    let b = bar(&ctx, ns[1]);
    let row = view(&ctx).row_h;
    let undo = app.engine.undo_history().len();
    // 1.4 rows down (70 c): the −30 c note lands on the semitone below (snapped), the +20 c
    // note ends 50 c under its own (free).
    drag_m(&ctx, &mut app, b.center(), b.center() + vec2(0.0, 1.4 * row), 8, Modifiers::NONE, |_, _| {});
    let tc = tuned(&app, ns[0]);
    assert!((tc / 100.0 - (tc / 100.0).round()).abs() < 1e-4, "{:?} lands on a semitone: {tc}", ns[0]);
    for r in &ns {
        assert!(edit(&app, *r).offset_cents < -60.0, "{r:?} moved down: {}", edit(&app, *r).offset_cents);
    }
    assert_eq!(app.engine.undo_history().len(), undo + 1);
}

#[test]
fn alt_arrows_step_the_selection() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let over = view(&ctx).rect.center();
    // Nothing selected: the keys go to the app as before.
    let undo = app.engine.undo_history().len();
    press(&ctx, &mut app, over, Key::ArrowUp, Modifiers::ALT);
    assert_eq!(edit(&app, ns[0]).offset_cents, 0.0);
    app.vocal.selected = [ns[0]].into_iter().collect();
    settle(&ctx, &mut app);
    let undo = app.engine.undo_history().len().max(undo);
    press(&ctx, &mut app, over, Key::ArrowUp, Modifiers::ALT);
    assert_eq!(edit(&app, ns[0]).offset_cents, 100.0);
    press(&ctx, &mut app, over, Key::ArrowUp, Modifiers::ALT);
    press(&ctx, &mut app, over, Key::ArrowDown, Modifiers::ALT);
    assert_eq!(edit(&app, ns[0]).offset_cents, 100.0);
    assert_eq!(app.engine.undo_history().len(), undo + 3, "a step each");
    assert_eq!(edit(&app, ns[1]).offset_cents, 0.0);
    // Alt+Shift+Up/Down is the app's (Copy Selection to Target/New Playlist), not a fine step.
    press(&ctx, &mut app, over, Key::ArrowDown, Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(edit(&app, ns[0]).offset_cents, 100.0, "no 10 c step");
    // Not over the editor: the editor leaves the keys alone.
    press(&ctx, &mut app, pos2(5.0, 5.0), Key::ArrowUp, Modifiers::ALT);
    assert_eq!(edit(&app, ns[0]).offset_cents, 100.0);
}

#[test]
fn the_panel_shows_the_move_and_clears_it() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.vocal.selected = [ns[0]].into_iter().collect();
    settle(&ctx, &mut app);
    assert_eq!(text(&ctx, "moved").as_deref(), Some(""));
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::ArrowUp, Modifiers::ALT);
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::ArrowUp, Modifiers::ALT);
    assert_eq!(text(&ctx, "moved").as_deref(), Some("moved +200c"));
    click(&ctx, &mut app, control(&ctx, "moved-clear").expect("the × button").center());
    assert_eq!(edit(&app, ns[0]).offset_cents, 0.0);
    assert_eq!(text(&ctx, "moved").as_deref(), Some(""));
    // Reset clears a move too.
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::ArrowDown, Modifiers::ALT);
    click(&ctx, &mut app, control(&ctx, "reset").unwrap().center());
    assert_eq!(edit(&app, ns[0]), NoteEdit::default());
}
