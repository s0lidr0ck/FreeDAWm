//! Parity section 7, selection and note mute: a click on a piano key selects that pitch's notes
//! (Shift adds), Select Notes in the MIDI Operations window selects by rule in the editor, and
//! a muted note stays in the clip.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::{ChannelFormat, TrackKind};
use soundcraft_ui_egui::{Services, SoundApp};

/// A Keys clip with notes C, E, C, G (velocities 100, 40, 100, 100), open in the MIDI Editor.
fn harness() -> (Harness<'static, SoundApp>, u64) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    for (pitch, start, velocity) in [(60, 0, 100), (64, 960, 40), (60, 1920, 100), (67, 2880, 100)] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start, "velocity": velocity})).unwrap();
    }
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

/// The middle of the piano key of `pitch`.
fn key(h: &Harness<'_, SoundApp>, pitch: i32) -> Pos2 {
    let g = h.state().midi.drawn.unwrap();
    pos2(g.roll.min.x - 12.0, g.y_of(pitch) + g.row_h / 2.0)
}

fn click(h: &mut Harness<'_, SoundApp>, p: Pos2, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers });
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

#[test]
fn a_piano_key_selects_its_notes() {
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_ui_egui::midi_edit_tools::key_selection;
    let n = |pitch, start| Note { probability: 100, pitch, velocity: 100, muted: false, release_velocity: 64, channel: 0, start, length: 10 };
    let seq = Sequence { notes: vec![n(60, -10), n(60, 0), n(62, 5), n(60, 9)], ctrls: Vec::new(), sysex: Vec::new() };
    assert_eq!(key_selection(&seq, &[2], 60, false), vec![1, 3], "hidden notes are not picked");
    assert_eq!(key_selection(&seq, &[2], 60, true), vec![1, 2, 3]);

    let (mut h, _) = harness();
    let c = key(&h, 60);
    click(&mut h, c, Modifiers::NONE);
    assert_eq!(h.state().midi.selected, vec![0, 2]);
    let g = key(&h, 67);
    click(&mut h, g, Modifiers::SHIFT);
    assert_eq!(h.state().midi.selected, vec![0, 2, 3]);
    let e = key(&h, 64);
    click(&mut h, e, Modifiers::NONE);
    assert_eq!(h.state().midi.selected, vec![1]);
}

#[test]
fn select_notes_by_rule_sets_the_editor_selection_and_mute_keeps_the_notes() {
    let (mut h, clip) = harness();
    {
        let app = h.state_mut();
        app.ops.midi_op = "select notes".into();
        app.ops.rule_velocity = (80, 127);
        app.ops.rule_pitch = (60, 64);
        app.ui.show_midi_ops = true;
    }
    h.run_steps(3);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().midi.selected, vec![0, 2]);
    // On beat 3 only.
    h.state_mut().ops.rule_beats = Some((3.0, 3.0));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().midi.selected, vec![2]);
    // Mute it: the note stays and shows as muted; the others play.
    h.state_mut().run("midi.notes_set", json!({"clip": clip, "indices": [2], "muted": true})).unwrap();
    h.run_steps(2);
    let s = h.state().engine.session();
    let soundcraft_model::ClipContent::Midi { sequence } = &s.find_clip(soundcraft_model::ClipId(clip)).unwrap().1.content else { panic!() };
    assert_eq!(sequence.notes.iter().map(|n| n.muted).collect::<Vec<_>>(), vec![false, false, true, false]);
}

use egui_kittest::kittest::Queryable;
