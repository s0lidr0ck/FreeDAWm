//! Notes edited directly on a MIDI track showing Notes: a drag on a note moves it in time and
//! pitch (one undo step), the Pencil adds a note, Alt-click deletes one.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_ui_egui::{Services, SoundApp, edit_window, track_notes};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    clip: Value,
    mods: Modifiers,
}

impl Rig {
    fn new() -> Rig {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"name": "Keys", "kind": "midi"})).unwrap();
        app.run("track.height", json!({"track": "Keys", "height": "large"})).unwrap();
        let clip = app.run("midi.new_clip", json!({"track": "Keys", "start": 0, "end": 192_000})).unwrap()["clip"].clone();
        app.run("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": 0, "length_ticks": 960})).unwrap();
        app.run("midi.note_add", json!({"clip": clip, "pitch": 67, "start_ticks": 1920, "length_ticks": 960})).unwrap();
        app.run("view.grid", json!({"value": "1/4"})).unwrap();
        app.run("view.zoom_set", json!({"samples_per_px": 400.0})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app, clip, mods: Modifiers::NONE };
        for _ in 0..4 {
            r.frame(vec![]);
        }
        r
    }

    fn frame(&mut self, events: Vec<Event>) {
        let events = std::iter::once(Event::ModifiersChanged(self.mods)).chain(events).collect();
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
        self.ctx
            .run_ui(raw, |ui| {
                self.app.logic(ui.ctx());
                self.app.ui(ui);
            })
            .textures_delta
            .clear();
    }

    /// The track's lane and timeline on screen.
    fn lane(&self) -> (Rect, Rect) {
        let s = self.app.engine.session();
        let t = s.track_by_name("Keys").unwrap();
        let tl = edit_window::rect(self.app.edit_layout.timeline);
        let row = self.app.edit_layout.rows.iter().find(|(id, _)| *id == t.id.0).unwrap().1;
        (Rect::from_min_max(pos2(tl.min.x, row[1]), pos2(tl.max.x, row[1] + t.height.points())), tl)
    }

    /// Note `i`'s rectangle on the track, and the screen points per semitone and per beat.
    fn note(&self, i: usize) -> (Rect, f32, f32) {
        let s = self.app.engine.session();
        let t = s.track_by_name("Keys").unwrap();
        let c = &t.clips()[0];
        let (lane, tl) = self.lane();
        let body = track_notes::body_rect(s, c, tl, lane);
        let r = track_notes::note_rects(s, c, tl, body).into_iter().find(|(k, _)| *k == i).unwrap().1;
        let soundcraft_engine::model::ClipContent::Midi { sequence } = &c.content else { panic!() };
        let beat = edit_window::x_of(s, tl, s.tempo.tick_to_samples(960, s.sample_rate)) - edit_window::x_of(s, tl, 0);
        (r, track_notes::row_height(sequence, body), beat)
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        self.mods = modifiers;
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers }]);
        for k in 1..=6 {
            let p = from + (to - from) * (k as f32 / 6.0);
            self.frame(vec![Event::PointerMoved(p)]);
        }
        self.frame(vec![Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers }]);
        self.frame(vec![]);
    }

    fn click(&mut self, at: Pos2, modifiers: Modifiers) {
        for _ in 0..40 {
            self.frame(vec![]);
        }
        self.mods = modifiers;
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers }]);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers }]);
        self.mods = Modifiers::NONE;
        self.frame(vec![]);
    }

    fn notes(&mut self) -> Vec<(i64, i64, i64)> {
        let v = self.app.run("midi.notes", json!({"clip": self.clip})).unwrap();
        v["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (n["pitch"].as_i64().unwrap(), n["start"].as_i64().unwrap(), n["length"].as_i64().unwrap()))
            .collect()
    }
}

#[test]
fn a_note_dragged_on_the_track_moves_a_beat_and_a_row() {
    let mut r = Rig::new();
    let (note, row, beat) = r.note(0);
    assert!(beat > 20.0 && row > 1.0, "beat {beat} row {row}");
    let from = pos2(note.min.x + 6.0, note.center().y);
    r.drag(from, from + vec2(beat, -row), Modifiers::NONE);
    assert_eq!(r.notes(), vec![(61, 960, 960), (67, 1920, 960)]);
    let clip = r.app.engine.session().track_by_name("Keys").unwrap().clips()[0].clone();
    assert!(r.app.engine.session().edit.selected_clips.contains(&clip.id), "the note's clip is selected");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.notes(), vec![(60, 0, 960), (67, 1920, 960)], "one undo step");
}

#[test]
fn the_right_end_of_a_note_resizes_it() {
    let mut r = Rig::new();
    let (note, _, beat) = r.note(0);
    let from = pos2(note.max.x - 2.0, note.center().y);
    r.drag(from, from + vec2(beat, 0.0), Modifiers::NONE);
    assert_eq!(r.notes()[0], (60, 0, 1920));
}

#[test]
fn the_pencil_adds_a_note_and_alt_click_deletes_one() {
    let mut r = Rig::new();
    r.app.run("edit.tool", json!({"tool": "pencil"})).unwrap();
    let (first, row, beat) = r.note(0);
    // Two rows above the first note, three beats in.
    let at = pos2(first.min.x + 3.0 * beat + 4.0, first.center().y - 2.0 * row);
    r.click(at, Modifiers::NONE);
    let notes = r.notes();
    assert_eq!(notes.len(), 3, "{notes:?}");
    assert!(notes.contains(&(62, 2880, 960)), "{notes:?}");
    r.app.run("edit.tool", json!({"tool": "smart"})).unwrap();
    let (second, _, _) = r.note(1);
    r.click(second.center(), Modifiers::ALT);
    assert_eq!(r.notes().len(), 2);
}

#[test]
fn a_clip_shown_as_blocks_is_not_note_edited() {
    let mut r = Rig::new();
    let (note, row, beat) = r.note(0);
    r.app.run("track.view", json!({"track": "Keys", "view": "blocks"})).unwrap();
    for _ in 0..3 {
        r.frame(vec![]);
    }
    let from = pos2(note.min.x + 6.0, note.center().y);
    r.drag(from, from + vec2(beat, -row), Modifiers::NONE);
    assert_eq!(r.notes()[0], (60, 0, 960), "the notes stay; the clip may move");
}
