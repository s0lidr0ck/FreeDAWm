//! The piano roll's basic operations: a move keeps the selection; either edge resizes every
//! selected note; Alt-drag copies; Ctrl+C, V and D copy, paste and duplicate; the Pencil drags
//! out a note's length and reuses the last length and velocity; a clicked note is auditioned.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_midi::Note;
use soundcraft_model::{ChannelFormat, ClipContent, ClipId, TrackKind};
use soundcraft_time::{GridValue, NoteValue};
use soundcraft_ui_egui::{Services, SoundApp, midi_editor};

/// A one-bar Keys clip (grid: eighths, 480 ticks) with C4, E4 and G4 an eighth long on beats
/// 1, 2 and 3, open in the MIDI Editor.
fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    for (pitch, start) in [(60, 0), (64, 960), (67, 1920)] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": 480})).unwrap();
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

fn notes(h: &Harness<'_, SoundApp>) -> Vec<Note> {
    let s = h.state().engine.session();
    match &s.find_clip(clip(h)).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.notes.clone(),
        _ => panic!("not MIDI"),
    }
}

/// (pitch, start, length) of every note.
fn psl(h: &Harness<'_, SoundApp>) -> Vec<(u8, i64, i64)> {
    notes(h).iter().map(|n| (n.pitch, n.start, n.length)).collect()
}

/// A point on the roll: `tick` from the clip start, in `pitch`'s row.
fn at(h: &Harness<'_, SoundApp>, tick: i64, pitch: i32) -> Pos2 {
    let g = h.state().midi.drawn.unwrap();
    pos2(g.x_of(tick), g.y_of(pitch) + g.row_h / 2.0)
}

fn button(h: &mut Harness<'_, SoundApp>, p: Pos2, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers });
    h.step();
}

fn drag_with(h: &mut Harness<'_, SoundApp>, a: Pos2, b: Pos2, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(a));
    h.step();
    button(h, a, true, modifiers);
    for k in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (k as f32 / 6.0)));
        h.step();
    }
    button(h, b, false, modifiers);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn drag(h: &mut Harness<'_, SoundApp>, a: Pos2, b: Pos2) {
    drag_with(h, a, b, Modifiers::NONE);
}

fn click(h: &mut Harness<'_, SoundApp>, p: Pos2) {
    // Well after any earlier click, so it is not read as a double-click.
    for _ in 0..40 {
        h.step();
    }
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    button(h, p, true, Modifiers::NONE);
    button(h, p, false, Modifiers::NONE);
}

fn key(h: &mut Harness<'_, SoundApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.step();
}

#[test]
fn a_move_keeps_the_selection_on_the_moved_notes() {
    let mut h = harness();
    h.state_mut().midi.audition = false;
    // Drag C4 three eighths later: it lands after E4 in the list.
    let p0 = at(&h, 240, 60);
    let p1 = at(&h, 240 + 1440, 60);
    drag(&mut h, p0, p1);
    assert_eq!(psl(&h), vec![(64, 960, 480), (60, 1440, 480), (67, 1920, 480)]);
    assert_eq!(h.state().midi.selected, vec![1]);
    // An arrow nudge right after moves the same note (and the selection follows it again).
    let p = at(&h, 3000, 70);
    h.input_mut().events.push(Event::PointerMoved(p));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(psl(&h), vec![(64, 960, 480), (60, 1920, 480), (67, 1920, 480)]);
    assert_eq!(h.state().midi.selected, vec![1]);
}

#[test]
fn either_edge_resizes_every_selected_note() {
    let mut h = harness();
    h.state_mut().midi.audition = false;
    h.state_mut().midi.selected = vec![0, 1];
    h.step();
    // C4's right edge out to the next beat: both selected notes grow by an eighth.
    let p0 = at(&h, 480, 60) - vec2(2.0, 0.0);
    let p1 = at(&h, 960, 60);
    drag(&mut h, p0, p1);
    assert_eq!(psl(&h), vec![(60, 0, 960), (64, 960, 960), (67, 1920, 480)]);
    assert_eq!(h.state().midi.selected, vec![0, 1]);
    // E4's left edge an eighth earlier: both starts move, their ends stay.
    let p0 = at(&h, 960, 64) + vec2(2.0, 0.0);
    let p1 = at(&h, 480, 64);
    drag(&mut h, p0, p1);
    assert_eq!(psl(&h), vec![(60, 0, 960), (64, 480, 1440), (67, 1920, 480)]);
    // The resized length is the Pencil's next.
    assert_eq!(h.state().midi.last_length, Some(1440));
}

#[test]
fn alt_drag_leaves_the_note_and_drops_a_copy() {
    let mut h = harness();
    h.state_mut().midi.audition = false;
    let row = h.state().midi.drawn.unwrap().row_h;
    let a = at(&h, 2160, 67);
    drag_with(&mut h, a, a + vec2(0.0, -2.0 * row), Modifiers::ALT);
    assert_eq!(psl(&h), vec![(60, 0, 480), (64, 960, 480), (67, 1920, 480), (69, 1920, 480)]);
    assert_eq!(h.state().midi.selected, vec![3], "the copy is selected");
}

#[test]
fn copy_paste_at_the_cursor_and_duplicate() {
    let mut h = harness();
    h.state_mut().midi.audition = false;
    let p0 = at(&h, 240, 60);
    click(&mut h, p0);
    assert_eq!(h.state().midi.selected, vec![0]);
    key(&mut h, Key::C, Modifiers::COMMAND);
    // A click on empty space puts the paste cursor there (on the grid).
    let p0 = at(&h, 2900, 72);
    click(&mut h, p0);
    assert_eq!(h.state().midi.cursor, Some(2880));
    key(&mut h, Key::V, Modifiers::COMMAND);
    assert_eq!(psl(&h), vec![(60, 0, 480), (64, 960, 480), (67, 1920, 480), (60, 2880, 480)]);
    assert_eq!(h.state().midi.selected, vec![3]);
    // Duplicate: right after the selection.
    key(&mut h, Key::D, Modifiers::COMMAND);
    assert_eq!(psl(&h).last(), Some(&(60, 3360, 480)));
    assert_eq!(h.state().midi.selected, vec![4]);
    // The Edit window's own Copy and Duplicate did not run on the clip.
    let s = h.state().engine.session();
    assert_eq!(s.track_by_name("Keys").unwrap().clips().len(), 1);
    // Cut: the selected note goes.
    key(&mut h, Key::X, Modifiers::COMMAND);
    assert_eq!(notes(&h).len(), 4);
}

#[test]
fn the_pencil_drags_out_a_length_and_reuses_it_with_the_last_velocity() {
    let mut h = harness();
    h.state_mut().midi.audition = false;
    h.state_mut().engine.execute("edit.tool", &json!({"tool": "pencil"})).unwrap();
    h.state_mut().midi.last_velocity = 77;
    h.step();
    let p0 = at(&h, 2880, 72) + vec2(2.0, 0.0);
    let p1 = at(&h, 2880 + 1440, 72);
    drag(&mut h, p0, p1);
    assert_eq!(psl(&h).last(), Some(&(72, 2880, 1440)));
    assert_eq!(h.state().midi.last_length, Some(1440));
    let p0 = at(&h, 500, 74);
    click(&mut h, p0);
    let n = notes(&h);
    let added = n.iter().find(|n| n.pitch == 74).unwrap();
    assert_eq!((added.start, added.length, added.velocity), (480, 1440, 77));
    assert!(n.iter().all(|n| n.pitch < 72 || n.velocity == 77));
}

#[test]
fn a_clicked_note_is_auditioned_and_the_document_comes_back() {
    let mut h = harness();
    let sel = h.state().engine.session().edit.selection;
    let p0 = at(&h, 1100, 64);
    click(&mut h, p0);
    assert_eq!(h.state().midi.selected, vec![1]);
    let heard = h.state().audition_session.clone().expect("an audition plays");
    assert!(h.state().is_playing());
    let (_, c) = heard.find_clip(clip(&h)).unwrap();
    let ClipContent::Midi { sequence } = &c.content else { panic!() };
    assert_eq!(sequence.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![64]);
    assert!(heard.tracks.iter().find(|t| t.name == "Keys").unwrap().mixer.solo);
    // When it stops, the player hears the document and the selection is as it was.
    h.state_mut().run("transport.stop", json!({})).unwrap();
    h.step();
    h.step();
    assert!(h.state().audition_session.is_none());
    assert_eq!(h.state().engine.session().edit.selection, sel);
    // With Audition off nothing plays.
    h.state_mut().midi.audition = false;
    let p0 = at(&h, 240, 60);
    click(&mut h, p0);
    assert!(h.state().audition_session.is_none() && !h.state().is_playing());
}

#[test]
fn pure_helpers() {
    use midi_editor::{Edge, duplicate_offset, edge_at};
    assert_eq!(edge_at(2.0, 100.0), Edge::Start);
    assert_eq!(edge_at(50.0, 100.0), Edge::Move);
    assert_eq!(edge_at(98.0, 100.0), Edge::End);
    // A narrow note has no left edge to grab.
    assert_eq!(edge_at(2.0, 10.0), Edge::Move);
    let n = |start, length| Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start, length };
    assert_eq!(duplicate_offset(&[n(0, 480), n(480, 300)], 480), 960);
    assert_eq!(duplicate_offset(&[n(100, 480)], 480), 480);
    let p = midi_editor::placed(&[n(500, 10), n(700, 10)], 2000);
    assert_eq!((p[0]["start"].as_i64(), p[1]["start"].as_i64()), (Some(2000), Some(2200)));
}
