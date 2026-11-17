//! Parity section 7, scales and chords in the MIDI Editor: rows folded to the key's scale, notes
//! drawn and dragged kept to it, and the Pencil stamping chords.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_midi::Note;
use soundcraft_model::{ChannelFormat, TrackKind};
use soundcraft_ui_egui::midi_edit_tools::{Chord, fold_mask, nearest_in, stamp};
use soundcraft_ui_egui::{Services, SoundApp};

/// A Keys clip in C major with an F# (out of the key) at beat 1, open in the MIDI Editor.
fn harness() -> (Harness<'static, SoundApp>, u64) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    app.run("event.add_key_change", json!({"key": "C major", "at": 0})).unwrap();
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    app.run("midi.note_add", json!({"clip": clip, "pitch": 66, "start_ticks": 0})).unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.ui.show_midi_editor = true;
    app.midi.audition = false;
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
    (h, clip)
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

fn pitches(h: &mut Harness<'_, SoundApp>, clip: u64) -> Vec<i64> {
    let r = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    r["notes"].as_array().unwrap().iter().map(|n| n["pitch"].as_i64().unwrap()).collect()
}

#[test]
fn chords_scales_and_folding_helpers() {
    let c_major = soundcraft_engine::vocal::key_set(0, 0);
    assert_eq!(nearest_in(c_major, 61), 60, "C# goes down to C on a tie");
    assert_eq!(nearest_in(c_major, 66), 65);
    let n = Note { probability: 100, pitch: 62, velocity: 90, muted: false, release_velocity: 64, channel: 0, start: 0, length: 10 };
    let p = |v: Vec<Note>| v.iter().map(|n| n.pitch).collect::<Vec<_>>();
    assert_eq!(p(stamp(None, None, n)), vec![62]);
    assert_eq!(p(stamp(Some(Chord::Minor), None, n)), vec![62, 65, 69]);
    assert_eq!(p(stamp(Some(Chord::InKey), Some((0, 0)), n)), vec![62, 65, 69], "D minor in C major");
    assert_eq!(p(stamp(Some(Chord::InKey), Some((0, 0)), Note { pitch: 67, ..n })), vec![67, 71, 74], "G major in C major");
    assert_eq!(p(stamp(Some(Chord::Major), None, Note { pitch: 125, ..n })), vec![125], "notes off the keyboard are dropped");
    let seq = soundcraft_midi::Sequence { notes: vec![Note { pitch: 66, ..n }], ctrls: Vec::new() };
    let m = fold_mask(c_major, &seq);
    assert_eq!(m.count_ones(), 75 + 1, "C major's 75 keys and the F#");
}

#[test]
fn folded_rows_keep_the_scale_and_the_pencil_stamps_a_chord_in_key() {
    let (mut h, clip) = harness();
    h.state_mut().midi.fold_scale = true;
    h.state_mut().midi.keep_scale = true;
    h.step();
    let g = h.state().midi.drawn.unwrap();
    assert_eq!(g.rows(), 76);
    // Folded, E and F are neighbouring rows, and F# keeps its own.
    assert_eq!(g.y_of(65) - g.y_of(64), -g.row_h);
    assert_eq!(g.y_of(66) - g.y_of(65), -g.row_h);
    assert_eq!(g.pitch_at(g.y_of(67) + 1.0), 67);
    assert_eq!(g.pitch_at(g.y_of(67) + g.row_h + 1.0), 66);
    // The Pencil on the D row at beat 3 draws D F A.
    h.state_mut().midi.chord = Some(Chord::InKey);
    h.state_mut().engine.session_mut().edit.tool = soundcraft_model::Tool::Pencil;
    h.step();
    let g = h.state().midi.drawn.unwrap();
    click(&mut h, pos2(g.x_of(1920 + 60), g.y_of(62) + g.row_h / 2.0));
    assert_eq!(pitches(&mut h, clip), vec![66, 62, 65, 69]);
}
