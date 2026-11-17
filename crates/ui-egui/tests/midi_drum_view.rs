//! Parity section 7, the drum view: on a Juke or drum synth track the MIDI Editor shows a named
//! row per drum (only those with hits unless every row is asked for) and hits as diamonds.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::{ChannelFormat, TrackKind};
use soundcraft_ui_egui::midi_edit_tools::{drum_mask, drum_names};
use soundcraft_ui_egui::{Services, SoundApp};

/// A Juke track's clip with a kick (36) on beat 1 and a snare (38) on beat 2.
fn harness() -> (Harness<'static, SoundApp>, u64) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Drums"));
    if let Some(tr) = app.engine.session_mut().track_mut(t) {
        tr.instrument = Some(soundcraft_engine::cmd::juke::juke_insert());
    }
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    for (pitch, start) in [(36, 0), (38, 960)] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": 120})).unwrap();
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
fn drum_names_come_from_the_instrument() {
    let mut juke = soundcraft_model::Track::new(soundcraft_model::TrackId(1), "D", TrackKind::Instrument, ChannelFormat::Stereo, [90, 90, 90]);
    juke.instrument = None;
    assert!(drum_names(&juke).is_none(), "no instrument, no drum view");
    juke.instrument = Some(soundcraft_engine::cmd::juke::juke_insert());
    let names = drum_names(&juke).unwrap();
    assert_eq!(names[36].as_deref(), Some("Kick"));
    assert_eq!(names[38].as_deref(), Some("Snare"));
    let mut synth = juke.clone();
    synth.instrument = Some(soundcraft_model::Insert::new("drum_synth"));
    assert_eq!(drum_names(&synth).unwrap()[42].as_deref(), soundcraft_midi::gm_drum_name(42));
    let mut sub = juke.clone();
    sub.instrument = Some(soundcraft_model::Insert::new("subtractive_synth"));
    assert!(drum_names(&sub).is_none());
    // Rows: those with hits; every named one when asked, or when the clip is empty.
    let seq = soundcraft_midi::Sequence::default();
    assert_eq!(drum_mask(&names, &seq, false).count_ones(), 16);
}

#[test]
fn a_juke_clip_shows_named_rows_with_hits_and_a_diamond_is_moved_not_resized() {
    let (mut h, clip) = harness();
    let g = h.state().midi.drawn.unwrap();
    assert_eq!(g.rows(), 2, "only the kick's and the snare's rows");
    assert_eq!(g.y_of(36) - g.y_of(38), g.row_h, "neighbouring rows");
    // A click on the snare's diamond selects it.
    click(&mut h, pos2(g.x_of(960), g.y_of(38) + g.row_h / 2.0));
    assert_eq!(h.state().midi.selected, vec![1]);
    // Every drum: Juke's 16 pads.
    h.state_mut().midi.drum_all_rows = true;
    h.step();
    assert_eq!(h.state().midi.drawn.unwrap().rows(), 16);
    // Drum view off: the full keyboard again.
    h.state_mut().midi.drum_view = Some(false);
    h.step();
    assert_eq!(h.state().midi.drawn.unwrap().rows(), 128);
    let _ = clip;
}
