//! Parity section 7: several selected MIDI clips in the MIDI Editor. The first is edited; the
//! others' notes show in their track's colour, and a click on one makes its clip the edited one
//! with that note selected; the header's Clip menu switches too.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::{ChannelFormat, ClipId, TrackKind};
use soundcraft_ui_egui::{Services, SoundApp};

/// Keys (C at beat 1) and Bass (G2 and A2 at beats 2 and 3) clips over the same bar, both selected.
fn harness() -> (Harness<'static, SoundApp>, u64, u64) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let keys = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let bass = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Bass"));
    let a = app.run("midi.new_clip", json!({"track": keys.0})).unwrap()["clip"].as_u64().unwrap();
    let b = app.run("midi.new_clip", json!({"track": bass.0})).unwrap()["clip"].as_u64().unwrap();
    app.run("midi.note_add", json!({"clip": a, "pitch": 60, "start_ticks": 0})).unwrap();
    for (pitch, start) in [(55, 960), (57, 1920)] {
        app.run("midi.note_add", json!({"clip": b, "pitch": pitch, "start_ticks": start})).unwrap();
    }
    app.run("edit.select", json!({"clips": [a, b]})).unwrap();
    app.ui.show_midi_editor = true;
    app.midi.audition = false;
    app.midi.ghosts = false;
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
    (h, a, b)
}

fn click(h: &mut Harness<'_, SoundApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.step();
}

/// Keys (C3 on beat 1 of bar 1) and Bass (G2 on beat 2 of bar 3, a bar after the Keys clip's
/// end), both selected, the grid at eighths.
fn apart() -> (Harness<'static, SoundApp>, u64, u64) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let keys = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let bass = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Bass"));
    let bar = {
        let s = app.engine.session();
        s.tempo.tick_to_samples(4 * soundcraft_time::TICKS_PER_QUARTER, s.sample_rate)
    };
    let a = app.run("midi.new_clip", json!({"track": keys.0, "start": 0, "end": bar})).unwrap()["clip"].as_u64().unwrap();
    let b = app.run("midi.new_clip", json!({"track": bass.0, "start": 2 * bar, "end": 3 * bar})).unwrap()["clip"].as_u64().unwrap();
    app.run("midi.note_add", json!({"clip": a, "pitch": 60, "start_ticks": 0, "length_ticks": 480})).unwrap();
    app.run("midi.note_add", json!({"clip": b, "pitch": 55, "start_ticks": 960, "length_ticks": 480})).unwrap();
    app.run("edit.select", json!({"clips": [a, b]})).unwrap();
    app.engine.session_mut().edit.grid =
        soundcraft_time::GridValue::Note { value: soundcraft_time::NoteValue::Eighth, dotted: false, triplet: false };
    app.ui.show_midi_editor = true;
    app.midi.audition = false;
    app.midi.ghosts = false;
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
    (h, a, b)
}

fn starts_pitches(h: &mut Harness<'_, SoundApp>, clip: u64) -> Vec<(i64, u64)> {
    let n = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    n["notes"].as_array().unwrap().iter().map(|n| (n["start"].as_i64().unwrap(), n["pitch"].as_u64().unwrap())).collect()
}

fn drag(h: &mut Harness<'_, SoundApp>, a: Pos2, b: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(a));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for k in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (k as f32 / 6.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.step();
}

#[test]
fn the_roll_spans_every_selected_clip_and_edits_act_on_all_in_one_step() {
    let (mut h, a, b) = apart();
    assert_eq!(soundcraft_ui_egui::midi_editor::target_clip(h.state()), Some(ClipId(a)));
    // The Bass clip starts two bars (7680 ticks) after the Keys clip: its note is in the roll.
    let g = h.state().midi.drawn.unwrap();
    let bass_note = pos2(g.x_of(7680 + 960 + 240), g.y_of(55) + g.row_h / 2.0);
    assert!(g.roll.contains(bass_note), "the roll spans both clips: {:?} not in {:?}", bass_note, g.roll);
    // Shift-click the Bass note: both notes selected, the Keys clip still the edited one.
    h.input_mut().events.push(Event::PointerMoved(pos2(g.x_of(240), g.y_of(60) + g.row_h / 2.0)));
    h.step();
    click(&mut h, pos2(g.x_of(240), g.y_of(60) + g.row_h / 2.0));
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.input_mut().events.push(Event::PointerMoved(bass_note));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: bass_note, button: PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT });
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    assert_eq!(soundcraft_ui_egui::midi_editor::target_clip(h.state()), Some(ClipId(a)));
    assert_eq!(h.state().midi.selected, vec![0]);
    assert_eq!(h.state().midi.others, vec![(ClipId(b), vec![0])]);
    // Drag the Bass note an eighth later and up a tone: both notes move, as one undo step.
    let steps = h.state().engine.undo_history().len();
    let to = pos2(g.x_of(7680 + 960 + 240 + 480), g.y_of(57) + g.row_h / 2.0);
    drag(&mut h, bass_note, to, Modifiers::NONE);
    assert_eq!(starts_pitches(&mut h, a), vec![(480, 62)]);
    assert_eq!(starts_pitches(&mut h, b), vec![(1440, 57)]);
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "one undo step");
    // The selection stayed on both notes.
    assert_eq!(h.state().midi.selected, vec![0]);
    assert_eq!(h.state().midi.others, vec![(ClipId(b), vec![0])]);
    // Quantize from the toolbar's function acts on both, in one step.
    h.state_mut().run("midi.notes_move", json!({"clip": a, "indices": [0], "ticks": 100})).unwrap();
    h.state_mut().run("midi.notes_move", json!({"clip": b, "indices": [0], "ticks": 100})).unwrap();
    let steps = h.state().engine.undo_history().len();
    soundcraft_ui_egui::midi_editor::run_on_selection(h.state_mut(), ClipId(a), "event.quantize", json!({"grid": 480, "strength": 100}));
    assert_eq!(starts_pitches(&mut h, a), vec![(480, 62)]);
    assert_eq!(starts_pitches(&mut h, b), vec![(1440, 57)]);
    assert_eq!(h.state().engine.undo_history().len(), steps + 1);
    // Undo puts both back at once.
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(starts_pitches(&mut h, a), vec![(580, 62)]);
    assert_eq!(starts_pitches(&mut h, b), vec![(1540, 57)]);
    // Delete takes the selected notes of both clips, in one step.
    h.step();
    let steps = h.state().engine.undo_history().len();
    h.input_mut().events.push(Event::PointerMoved(g.roll.center()));
    h.step();
    h.input_mut().events.push(Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    h.step();
    assert!(starts_pitches(&mut h, a).is_empty() && starts_pitches(&mut h, b).is_empty());
    assert_eq!(h.state().engine.undo_history().len(), steps + 1);
}

#[test]
fn the_pencil_draws_into_the_clip_under_it() {
    let (mut h, a, b) = apart();
    h.state_mut().run("edit.tool", json!({"tool": "pencil"})).unwrap();
    h.step();
    let g = h.state().midi.drawn.unwrap();
    // Beat 3 of bar 3: inside the Bass clip only.
    click(&mut h, pos2(g.x_of(7680 + 1920 + 10), g.y_of(52) + g.row_h / 2.0));
    assert_eq!(starts_pitches(&mut h, b), vec![(960, 55), (1920, 52)]);
    assert_eq!(starts_pitches(&mut h, a), vec![(0, 60)]);
}

#[test]
fn a_click_on_another_selected_clips_note_edits_that_clip() {
    let (mut h, a, b) = harness();
    assert_eq!(soundcraft_ui_egui::midi_editor::target_clip(h.state()), Some(ClipId(a)));
    let s = h.state().engine.session();
    let siblings = soundcraft_ui_egui::midi_ghosts::siblings(s, ClipId(a));
    assert_eq!(siblings.iter().map(|g| (g.clip.0, g.index)).collect::<Vec<_>>(), vec![(b, 0), (b, 1)]);
    // Ghosts off: the other selected clip's notes still show (they are not ghosts).
    let g = h.state().midi.drawn.unwrap();
    let p = pos2(g.x_of(1920 + 120), g.y_of(57) + g.row_h / 2.0);
    click(&mut h, p);
    assert_eq!(soundcraft_ui_egui::midi_editor::target_clip(h.state()), Some(ClipId(b)));
    assert_eq!(h.state().midi.selected, vec![1]);
    // An edit now acts on that clip's note.
    let sel = h.state().midi.selected.clone();
    h.state_mut().run("midi.notes_move", json!({"clip": b, "indices": sel, "semitones": 2})).unwrap();
    let notes = h.state_mut().run("midi.notes", json!({"clip": b})).unwrap();
    assert_eq!(notes["notes"][1]["pitch"], 59, "{notes}");
    // Back to the first through the switcher's function.
    soundcraft_ui_egui::midi_editor::activate_clip(h.state_mut(), ClipId(a), Vec::new());
    h.step();
    assert_eq!(soundcraft_ui_egui::midi_editor::target_clip(h.state()), Some(ClipId(a)));
    // A single selected clip has no siblings.
    h.state_mut().run("edit.select", json!({"clips": [a]})).unwrap();
    assert!(soundcraft_ui_egui::midi_ghosts::siblings(h.state().engine.session(), ClipId(a)).is_empty());
}
