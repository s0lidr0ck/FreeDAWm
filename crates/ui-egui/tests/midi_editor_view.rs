//! The piano roll's grid and view: a grid that adapts to the zoom, dotted and tuplet grids, Snap
//! Moves to the Grid, the dragged note sounding at its new pitch, and zooming to the notes.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::{ChannelFormat, ClipContent, ClipId, TrackKind};
use soundcraft_time::{GridValue, NoteValue};
use soundcraft_ui_egui::midi_edit_tools::{MidiGrid, adaptive_ticks};
use soundcraft_ui_egui::{Services, SoundApp};

/// A one-bar Keys clip (grid: eighths) with C4, E4 and G4 an eighth long on beats 1, 2 and 3,
/// and C5 a 16th long 100 ticks into beat 4, open in the MIDI Editor.
fn harness() -> Harness<'static, SoundApp> {
    harness_bars(None)
}

/// [`harness`] with the clip `bars` long (at 120 bpm, 48 kHz) when given.
fn harness_bars(bars: Option<i64>) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let mut p = json!({"track": t.0});
    if let Some(b) = bars {
        p["start"] = json!(0);
        p["end"] = json!(b * 96_000);
    }
    let clip = app.run("midi.new_clip", p).unwrap()["clip"].as_u64().unwrap();
    for (pitch, start, len) in [(60, 0, 480), (64, 960, 480), (67, 1920, 480), (72, 2980, 240)] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": len})).unwrap();
    }
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.engine.session_mut().edit.grid = GridValue::Note { value: NoteValue::Eighth, dotted: false, triplet: false };
    app.ui.show_midi_editor = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..3 {
        h.step();
    }
    h
}

fn clip(h: &Harness<'_, SoundApp>) -> ClipId {
    h.state().midi.clip.unwrap()
}

fn psl(h: &Harness<'_, SoundApp>) -> Vec<(u8, i64, i64)> {
    match &h.state().engine.session().find_clip(clip(h)).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect(),
        _ => panic!("not MIDI"),
    }
}

fn at(h: &Harness<'_, SoundApp>, tick: i64, pitch: i32) -> Pos2 {
    let g = h.state().midi.drawn.unwrap();
    pos2(g.x_of(tick), g.y_of(pitch) + g.row_h / 2.0)
}

fn button(h: &mut Harness<'_, SoundApp>, p: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.step();
}

/// Press at `a`, move to `b` in steps (leaving the button down).
fn press_and_move(h: &mut Harness<'_, SoundApp>, a: Pos2, b: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(a));
    h.step();
    button(h, a, true);
    for k in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (k as f32 / 6.0)));
        h.step();
    }
}

#[test]
fn moves_keep_their_offset_unless_they_snap_to_the_grid() {
    let mut h = harness();
    h.state_mut().midi.audition = false;
    // C5 starts 100 ticks past a line; moved about an eighth back it keeps those 100 ticks.
    let (a, b) = (at(&h, 3000, 72), at(&h, 3000 - 500, 72));
    press_and_move(&mut h, a, b);
    button(&mut h, b, false);
    assert!(psl(&h).contains(&(72, 2500, 240)), "{:?}", psl(&h));
    h.state_mut().run("edit.undo", json!({})).unwrap();
    // With Snap Moves to the Grid, its start lands on the line.
    h.state_mut().midi.snap_moves = true;
    for _ in 0..40 {
        h.step();
    }
    press_and_move(&mut h, a, b);
    button(&mut h, b, false);
    assert!(psl(&h).contains(&(72, 2400, 240)), "{:?}", psl(&h));
}

#[test]
fn a_note_dragged_to_another_pitch_sounds_there_while_dragged() {
    let mut h = harness();
    assert!(h.state().midi.audition);
    let (a, b) = (at(&h, 240, 60), at(&h, 240, 62));
    press_and_move(&mut h, a, b);
    assert_eq!(h.state().midi.drag_heard, Some(62), "the new pitch sounds during the drag");
    assert!(h.state().midi.auditioning.is_some());
    button(&mut h, b, false);
    assert_eq!(h.state().midi.drag_heard, None, "and stops when the note is let go");
    assert!(psl(&h).contains(&(62, 0, 480)));
}

#[test]
fn zoom_to_notes_fits_the_selected_notes() {
    let mut h = harness();
    h.state_mut().midi.selected = vec![1, 2, 3];
    h.state_mut().midi.pending_zoom = Some(-1.0);
    h.step();
    h.step();
    let g = h.state().midi.drawn.unwrap();
    // E4 to C5 (960 to 3220) span the roll, with a little room either side.
    let (x0, x1) = (g.x_of(960), g.x_of(3220));
    assert!(x0 > g.roll.min.x && x0 - g.roll.min.x < g.roll.width() * 0.1, "{x0} in {:?}", g.roll);
    assert!(x1 < g.roll.max.x && g.roll.max.x - x1 < g.roll.width() * 0.1, "{x1} in {:?}", g.roll);
    // Its row in view.
    let y = g.y_of(64);
    assert!(g.roll.y_range().contains(y), "{y} in {:?}", g.roll);
}

#[test]
fn an_adaptive_grid_follows_the_zoom() {
    let mut h = harness_bars(Some(16));
    h.state_mut().midi.grid = MidiGrid::Adaptive(1);
    h.step();
    h.step();
    let g = h.state().midi.drawn.unwrap();
    assert_eq!(h.state().midi.grid, MidiGrid::Adaptive(adaptive_ticks(g.px_per_tick)));
    // Zoomed to one note, the lines get finer.
    let before = h.state().midi.grid.ticks(GridValue::default());
    h.state_mut().midi.selected = vec![3];
    h.state_mut().midi.pending_zoom = Some(-1.0);
    for _ in 0..3 {
        h.step();
    }
    let after = h.state().midi.grid.ticks(GridValue::default());
    assert!(after < before, "{after} against {before}");
    // A quintuplet grid snaps a drawn note's edge to fifths of a quarter.
    assert_eq!(MidiGrid::Tuplet { value: NoteValue::Sixteenth, n: 5 }.ticks(GridValue::default()), 192);
}
