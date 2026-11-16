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
