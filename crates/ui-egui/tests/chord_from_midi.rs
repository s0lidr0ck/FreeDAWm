//! Chord writing from MIDI (docs/superpowers/specs/2026-10-30-midi-input.md, 7): in the Chords
//! ruler's picker, keys held on a MIDI input (here Musical Typing's port) are named live with the
//! Band's chord matcher, the lowest note the bass; letting go leaves the name; Enter adds it and
//! the picker moves on to the next chord slot, one undo step per chord.

use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
use serde_json::json;
use soundcraft_ui_egui::band::chord_picker::{held_chord_name, open};
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({})).unwrap();
        engine.execute("view.ruler", &json!({"ruler": "band_chords", "visible": true})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), time: 1.0 };
        r.frames(3);
        r
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
            events,
            time: Some(self.time),
            ..Default::default()
        };
        let mut out = self.ctx.run_ui(raw, |ui| {
            self.app.logic(ui.ctx());
            self.app.ui(ui);
        });
        out.textures_delta.clear();
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn play(&self, msgs: &[[u8; 3]]) {
        let keys = self.app.midi_hub.as_ref().unwrap().typing();
        for m in msgs {
            keys.send(m);
        }
    }

    fn picker_text(&self) -> Option<String> {
        match &self.app.dialogs.open {
            Some(Dialog::ChordPicker(p)) => Some(p.text.clone()),
            _ => None,
        }
    }
}

#[test]
fn held_notes_are_named_in_the_band_key_with_the_lowest_as_bass() {
    let s = soundcraft_model::Session::default();
    assert_eq!(held_chord_name(&s, &[57, 60, 64]).as_deref(), Some("Am"));
    assert_eq!(held_chord_name(&s, &[52, 60, 67]).as_deref(), Some("C/E"));
    assert_eq!(held_chord_name(&s, &[]), None);
}

#[test]
fn the_picker_names_played_chords_and_moves_on_after_each() {
    let mut r = Rig::new();
    open(&mut r.app, 0, None);
    r.frames(2);
    assert_eq!(r.picker_text().as_deref(), Some(""));
    // A minor, held: named as it is played.
    r.play(&[[0x90, 57, 90], [0x90, 60, 90], [0x90, 64, 90]]);
    r.frames(2);
    assert_eq!(r.picker_text().as_deref(), Some("Am"));
    // Let go: the name stays.
    r.play(&[[0x80, 57, 0], [0x80, 60, 0], [0x80, 64, 0]]);
    r.frames(2);
    assert_eq!(r.picker_text().as_deref(), Some("Am"));
    // Enter adds it and the picker goes on to the next slot.
    let undo_before = r.app.engine.undo_history().len();
    r.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    r.frames(2);
    let chords = r.app.engine.session().band.chords.clone();
    assert_eq!(chords.len(), 1, "{chords:?}");
    assert_eq!(chords[0].tick, 0);
    assert_eq!(r.app.engine.undo_history().len(), undo_before + 1, "one undo step");
    let Some(Dialog::ChordPicker(next)) = &r.app.dialogs.open else { panic!("the picker stays open for the next chord") };
    let next_at = next.at;
    assert!(next_at > 0, "at the next slot");
    // C over E in the next slot.
    r.play(&[[0x90, 52, 90], [0x90, 60, 90], [0x90, 67, 90]]);
    r.frames(2);
    assert_eq!(r.picker_text().as_deref(), Some("C/E"));
    r.play(&[[0x80, 52, 0], [0x80, 60, 0], [0x80, 67, 0]]);
    r.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    r.frames(2);
    assert_eq!(r.app.engine.session().band.chords.len(), 2);
}
