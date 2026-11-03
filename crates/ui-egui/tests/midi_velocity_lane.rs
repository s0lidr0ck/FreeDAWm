//! The MIDI Editor's velocity lane: a chord's stems stand side by side so each note can be
//! grabbed; with notes selected only they are edited, and dragging one of several selected stems
//! scales them all; a drag from empty space draws a line the notes ramp along.

use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::{ChannelFormat, TrackKind};
use soundcraft_ui_egui::{Services, SoundApp, midi_editor};

/// A Keys clip with a C major chord (velocities 100, 60, 30) at the start and a note a beat later,
/// open in the MIDI Editor.
fn harness() -> (Harness<'static, SoundApp>, u64) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    for (pitch, start, velocity) in [(60, 0, 100), (64, 0, 60), (67, 0, 30), (72, 960, 100)] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start, "velocity": velocity})).unwrap();
    }
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
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
    (h, clip)
}

fn lane(h: &Harness<'_, SoundApp>) -> Rect {
    h.ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("soundcraft-midi-vel"))).unwrap()
}

/// Where note `i`'s stem head is drawn.
fn head(h: &Harness<'_, SoundApp>, i: usize) -> Pos2 {
    let vel = lane(h);
    let geo = h.state().midi.drawn.unwrap();
    let notes = velocities_and_starts(h);
    let offsets = midi_editor::stem_offsets(&notes);
    let n = notes[i];
    let x = geo.x_of(n.start) + 1.0 + offsets[i];
    pos2(x, vel.max.y - 2.0 - (vel.height() - 4.0) * f32::from(n.velocity) / 127.0)
}

fn velocities_and_starts(h: &Harness<'_, SoundApp>) -> Vec<soundcraft_midi::Note> {
    let s = h.state().engine.session();
    let (_, c) = s.find_clip(h.state().midi.clip.unwrap()).unwrap();
    match &c.content {
        soundcraft_model::ClipContent::Midi { sequence } => sequence.notes.clone(),
        _ => unreachable!(),
    }
}

fn velocities(h: &Harness<'_, SoundApp>) -> Vec<u8> {
    velocities_and_starts(h).iter().map(|n| n.velocity).collect()
}

/// The lane height that reads as velocity `v`.
fn y_for(h: &Harness<'_, SoundApp>, v: f32) -> f32 {
    let vel = lane(h);
    vel.max.y - 2.0 - (vel.height() - 4.0) * v / 127.0
}

fn drag(h: &mut Harness<'_, SoundApp>, a: Pos2, b: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(a));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=6 {
        let p = a + (b - a) * (k as f32 / 6.0);
        h.input_mut().events.push(Event::PointerMoved(p));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    h.step();
}

#[test]
fn a_chord_note_hidden_behind_another_is_grabbed_by_its_own_stem() {
    let (mut h, _) = harness();
    let a = head(&h, 1);
    let to = pos2(a.x, y_for(&h, 20.0));
    drag(&mut h, a, to);
    let v = velocities(&h);
    assert_eq!(v[0], 100);
    assert!((18..=22).contains(&v[1]), "{v:?}");
    assert_eq!(v[2], 30);
    assert_eq!(v[3], 100);
    // One drag is one undo step.
    assert!(h.state_mut().engine.undo());
    assert_eq!(velocities(&h), vec![100, 60, 30, 100]);
}

#[test]
fn with_notes_selected_only_they_change_and_a_drag_scales_them_together() {
    let (mut h, _) = harness();
    h.state_mut().midi.selected = vec![0, 1];
    h.step();
    // Grab the 100 stem and pull it down to about half: both selected notes halve.
    let a = head(&h, 0);
    let to = pos2(a.x, y_for(&h, 50.0));
    drag(&mut h, a, to);
    let v = velocities(&h);
    assert!((48..=52).contains(&v[0]), "{v:?}");
    assert!((28..=32).contains(&v[1]), "{v:?}");
    assert_eq!((v[2], v[3]), (30, 100), "unselected notes stay");
}

#[test]
fn a_line_from_empty_space_ramps_the_notes_under_it() {
    let (mut h, _) = harness();
    let vel = lane(&h);
    // Start just over the first stem's head (empty space: above every head), at velocity 126;
    // end right of the last stem, at 20.
    let a = pos2(head(&h, 0).x, y_for(&h, 126.0));
    let b = pos2(head(&h, 3).x + 3.0, y_for(&h, 20.0));
    assert!(vel.contains(a) && vel.contains(b));
    drag(&mut h, a, b);
    let v = velocities(&h);
    // Falling from left to right; the chord's stems are a few points apart, so nearly equal.
    assert!(v[0] > v[3], "{v:?}");
    assert!(v[0] >= v[1] && v[1] >= v[2], "{v:?}");
    assert!(v[3] <= 30, "{v:?}");
    assert!(v[0] >= 110, "{v:?}");
    // One undo step.
    assert!(h.state_mut().engine.undo());
    assert_eq!(velocities(&h), vec![100, 60, 30, 100]);
}

#[test]
fn stems_of_a_chord_stand_apart() {
    let notes: Vec<soundcraft_midi::Note> = [(60, 0), (64, 0), (67, 0), (72, 960)]
        .iter()
        .map(|(p, s)| soundcraft_midi::Note { pitch: *p, velocity: 100, release_velocity: 64, channel: 0, start: *s, length: 240 })
        .collect();
    assert_eq!(midi_editor::stem_offsets(&notes), vec![0.0, 4.0, 8.0, 0.0]);
}

#[test]
fn pure_helpers() {
    let heads = [(0, pos2(10.0, 50.0)), (1, pos2(14.0, 80.0)), (2, pos2(40.0, 60.0))];
    assert_eq!(midi_editor::pick_stem(&heads, pos2(12.0, 78.0), false), Some(1));
    assert_eq!(midi_editor::pick_stem(&heads, pos2(12.0, 52.0), false), Some(0));
    assert_eq!(midi_editor::pick_stem(&heads, pos2(25.0, 52.0), false), None);
    // Above every head is empty space for a drag; a click there still takes the column.
    assert_eq!(midi_editor::pick_stem(&heads, pos2(12.0, 20.0), false), None);
    assert_eq!(midi_editor::pick_stem(&heads, pos2(12.0, 20.0), true), Some(0));
    let v = midi_editor::line_values(&[(0, 0.0), (1, 50.0), (2, 100.0), (3, 200.0)], pos2(100.0, 0.0), pos2(0.0, 100.0), |y| y as i64);
    assert_eq!(v, vec![(0, 100), (1, 50), (2, 0)]);
    assert_eq!(midi_editor::scaled(&[(0, 100), (1, 60)], 100, 50), vec![(0, 50), (1, 30)]);
    assert_eq!(midi_editor::scaled(&[(0, 100), (1, 120)], 100, 127), vec![(0, 127), (1, 127)]);
}
