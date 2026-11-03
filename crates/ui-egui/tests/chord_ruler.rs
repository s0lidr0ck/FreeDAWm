//! Editing chords on the Chords ruler (docs/superpowers/specs/2026-10-26-key-ruler-midi-zoom.md,
//! 1b): the chord picker (add and edit, typed or from the key's quick buttons), Delete, move and
//! resize on the half-bar grid or off it with Alt, and Clear All. Each is one undo step.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::edit_window::{rect, x_of};
use soundcraft_ui_egui::ruler_items::RulerItem;
use soundcraft_ui_egui::{Services, SoundApp};

/// A half bar of 4/4, in ticks (the lane's grid).
const HALF: i64 = 1920;

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({})).unwrap();
        engine.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
        engine.execute("view.ruler", &json!({"ruler": "band_chords", "visible": true})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new(), time: 1.0 };
        r.frames(4);
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
        self.shapes = out.shapes;
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn text_rect(&self, label: &str) -> Option<Rect> {
        self.shapes.iter().rev().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2())),
            _ => None,
        })
    }

    fn has_text(&self, label: &str) -> bool {
        self.text_rect(label).is_some()
    }

    fn texts(&self) -> Vec<String> {
        self.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// The Chords ruler's name label (drawn first).
    fn name_pos(&self) -> Pos2 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == "Chords" => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                _ => None,
            })
            .expect("no Chords ruler")
    }

    fn ruler_y(&self) -> f32 {
        self.name_pos().y
    }

    fn samples(&self, tick: i64) -> i64 {
        let s = self.app.engine.session();
        s.tempo.tick_to_samples(tick, s.sample_rate)
    }

    fn x(&self, tick: i64) -> f32 {
        let s = self.app.engine.session();
        x_of(s, rect(self.app.edit_layout.timeline), self.samples(tick))
    }

    fn at(&self, tick: i64) -> Pos2 {
        pos2(self.x(tick), self.ruler_y())
    }

    fn button(&mut self, at: Pos2, button: PointerButton, pressed: bool) {
        self.frame(vec![Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE }]);
    }

    fn press(&mut self, at: Pos2, button: PointerButton) {
        self.frames(30);
        self.frame(vec![Event::PointerMoved(at)]);
        self.button(at, button, true);
        self.button(at, button, false);
        self.frames(3);
    }

    fn click(&mut self, at: Pos2) {
        self.press(at, PointerButton::Primary);
    }

    fn right_click(&mut self, at: Pos2) {
        self.press(at, PointerButton::Secondary);
    }

    fn double_click(&mut self, at: Pos2) {
        self.frames(30);
        self.frame(vec![Event::PointerMoved(at)]);
        for _ in 0..2 {
            self.button(at, PointerButton::Primary, true);
            self.button(at, PointerButton::Primary, false);
        }
        self.frames(3);
    }

    fn drag(&mut self, from: Pos2, to: Pos2, mods: Modifiers) {
        self.frames(30);
        let m = Event::ModifiersChanged(mods);
        self.frame(vec![m.clone(), Event::PointerMoved(from)]);
        self.frame(vec![m.clone(), Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: mods }]);
        for k in 1..=6 {
            let p = from + (to - from) * (k as f32 / 6.0);
            self.frame(vec![m.clone(), Event::PointerMoved(p)]);
        }
        self.frame(vec![m, Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: mods }]);
        self.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.frames(3);
    }

    fn click_text(&mut self, label: &str) {
        let r = self.text_rect(label).unwrap_or_else(|| panic!("no `{label}` in {:?}", self.texts()));
        self.click(r.center());
    }

    fn key(&mut self, key: Key) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
    }

    fn type_text(&mut self, text: &str) {
        self.frame(vec![Event::Text(text.into())]);
        self.frames(2);
    }

    fn undo(&mut self) {
        self.app.run("edit.undo", json!({})).unwrap();
        self.frames(2);
    }

    /// The chords as (tick, length, name in the band's key).
    fn chords(&self) -> Vec<(i64, i64, String)> {
        let st = &self.app.engine.session().band;
        let (k, sc) = (i32::from(st.key), i32::from(st.scale));
        st.chords
            .iter()
            .map(|c| (c.tick, c.length_ticks, soundcraft_band::chord_theory::name_for(i32::from(c.slot), k, sc, i32::from(c.bass))))
            .collect()
    }

    fn chord(&self, name: &str) -> (i64, i64, String) {
        self.chords().into_iter().find(|c| c.2 == name).unwrap_or_else(|| panic!("no {name} in {:?}", self.chords()))
    }
}

fn span(c: &(i64, i64, String)) -> (i64, i64) {
    (c.0, c.1)
}

/// C, Am, F, G a bar each from bar 1.
fn with_chords() -> Rig {
    let mut r = Rig::new();
    for (k, c) in ["C", "Am", "F", "G"].iter().enumerate() {
        let at = r.samples(2 * HALF * k as i64);
        r.app.run("band.chord.add", json!({"at": at, "end": at + r.samples(2 * HALF), "chord": c})).unwrap();
    }
    r.frames(3);
    r
}

#[test]
fn diatonic_quick_chords_spell_as_the_key_does_and_parse() {
    use soundcraft_ui_egui::band::chord_picker::diatonic;
    let (t, s) = diatonic(0, 0);
    assert_eq!(t, ["C", "Dm", "Em", "F", "G", "Am", "Bdim"]);
    assert_eq!(s, ["Cmaj7", "Dm7", "Em7", "Fmaj7", "G7", "Am7", "Bm7b5"]);
    let (t, _) = diatonic(3, 0);
    assert_eq!(t, ["Eb", "Fm", "Gm", "Ab", "Bb", "Cm", "Ddim"], "flat keys use flats");
    let (t, s) = diatonic(9, 1);
    assert_eq!(t, ["Am", "Bdim", "C", "Dm", "Em", "F", "G"]);
    assert_eq!(s[0], "Am7");
    for key in 0..12 {
        for scale in 0..2 {
            let (t, s) = diatonic(key, scale);
            for n in t.iter().chain(&s) {
                assert!(soundcraft_band::chord_match::parse_chord_name(n).is_some(), "{n}");
            }
        }
    }
}

#[test]
fn the_picker_adds_a_chord_to_the_bar_end_or_the_next_chord() {
    let mut r = Rig::new();
    let p = r.at(HALF * 21 / 10);
    r.double_click(p);
    assert_eq!(r.app.dialogs.open_name(), Some("chord_picker"));
    r.frames(2);
    for b in ["C", "Dm", "G7", "Am7", "Bm7b5"] {
        assert!(r.has_text(b), "{b}: {:?}", r.texts());
    }
    r.click_text("Cancel");
    assert!(r.chords().is_empty(), "Cancel adds nothing");
    // A quick button, then OK: from the half bar to the bar's end.
    r.double_click(p);
    r.frames(2);
    r.click_text("Am7");
    assert!(r.has_text("→ Am7 · exact (in C major)"), "{:?}", r.texts());
    r.click_text("OK");
    assert_eq!(r.chords(), [(2 * HALF, 2 * HALF, "Am7".to_string())]);
    // Typed, before it: up to that chord. A chord the key doesn't hold says so first.
    r.double_click(r.at(HALF / 10));
    r.frames(2);
    r.type_text("F#");
    assert!(r.texts().iter().any(|t| t.starts_with("→ nearest:")), "{:?}", r.texts());
    r.key(Key::Enter);
    assert_eq!(r.chords().len(), 2, "{} {:?}", r.app.ui.status, r.chords());
    assert!(r.chords().iter().any(|c| span(c) == (0, 2 * HALF)), "up to the next chord: {:?}", r.chords());
    assert!(r.texts().iter().any(|t| t.contains("≈ nearest in key")), "{:?}", r.texts());
    r.undo();
    assert_eq!(r.chords().len(), 1, "one undo step");
    // Unreadable text can't be laid.
    r.double_click(r.at(HALF * 61 / 10));
    r.frames(2);
    r.type_text("Qzz");
    assert!(r.has_text("Can't read that chord"), "{:?}", r.texts());
    r.key(Key::Enter);
    assert_eq!(r.chords().len(), 1);
}

#[test]
fn the_picker_edits_a_chord() {
    let mut r = with_chords();
    // A double-click on Am opens it.
    r.double_click(r.at(3 * HALF));
    r.frames(2);
    assert!(r.has_text("Edit Chord"), "{:?}", r.texts());
    r.click_text("Dm");
    r.click_text("OK");
    assert_eq!(r.chords()[1], (2 * HALF, 2 * HALF, "Dm".to_string()), "edited in place");
    r.undo();
    assert_eq!(r.chords()[1].2, "Am");
    // Right-click › Edit Chord…
    r.right_click(r.at(5 * HALF));
    r.click_text("Edit Chord…");
    r.frames(2);
    r.click_text("G7");
    r.click_text("OK");
    assert_eq!(r.chords()[2].2, "G7");
}

#[test]
fn a_picked_chord_goes_with_delete_and_clear_all_asks() {
    let mut r = with_chords();
    r.click(r.at(3 * HALF));
    assert!(matches!(r.app.ui.ruler_item, Some(RulerItem::Chord(_))));
    r.key(Key::Delete);
    assert_eq!(r.chords().len(), 3);
    r.undo();
    r.right_click(pos2(r.name_pos().x, r.ruler_y()));
    r.click_text("Clear All Chords…");
    r.frames(2);
    r.click_text("Clear All");
    assert!(r.chords().is_empty());
    r.undo();
    assert_eq!(r.chords().len(), 4);
}

#[test]
fn drags_move_and_resize_on_the_grid_or_off_it_with_alt() {
    let mut r = with_chords();
    // Move Am (bar 2) by about a half bar and a bit: snapped.
    r.drag(r.at(3 * HALF), r.at(4 * HALF + HALF / 5), Modifiers::NONE);
    assert_eq!(span(&r.chord("Am")), (3 * HALF, 2 * HALF), "{:?}", r.chords());
    r.undo();
    assert_eq!(span(&r.chord("Am")), (2 * HALF, 2 * HALF));
    // With Alt: off the grid, by the pointer's ticks.
    let d = HALF * 7 / 10;
    r.drag(r.at(3 * HALF), r.at(3 * HALF + d), Modifiers::ALT);
    let moved = r.chord("Am");
    assert_eq!(moved.1, 2 * HALF, "a move keeps the length");
    assert!((moved.0 - (2 * HALF + d)).abs() < 40, "off the grid near {}: {:?}", 2 * HALF + d, r.chords());
    assert_ne!(moved.0 % HALF, 0);
    r.undo();
    // Resize G's right edge with Alt: off the grid; plain: on it.
    let edge = r.at(8 * HALF) - vec2(2.0, 0.0);
    r.drag(edge, r.at(8 * HALF + HALF / 3), Modifiers::ALT);
    let g = r.chord("G");
    assert_eq!(g.0, 6 * HALF);
    assert!((g.1 - (2 * HALF + HALF / 3)).abs() < 40, "{g:?}");
    r.undo();
    r.drag(edge, r.at(9 * HALF + HALF / 5), Modifiers::NONE);
    let g = r.chord("G");
    assert_eq!((g.0, g.1), (6 * HALF, 3 * HALF));
    r.undo();
    assert_eq!(span(&r.chord("G")), (6 * HALF, 2 * HALF), "one undo step each");
}

/// Off-grid chords (Detect Chords "As played"): A at 0.1-1.45 s, B at 2.34-3.65 s (120 bpm).
fn with_played_chords() -> Rig {
    let mut r = Rig::new();
    for (a, b, c) in [(192, 2784, "Am"), (4500, 7000, "F")] {
        let (a, b) = (r.samples(a), r.samples(b));
        r.app.run("band.chord.add", json!({"at": a, "end": b, "chord": c, "snap": false})).unwrap();
    }
    r.frames(3);
    assert_eq!(r.chords().iter().map(span).collect::<Vec<_>>(), [(192, 2592), (4500, 2500)]);
    r
}

#[test]
fn the_picker_adds_between_off_grid_chords_without_moving_them() {
    let mut r = with_played_chords();
    // Just after Am's end: the half bar before it would cut Am, so the chord starts where Am
    // ends and runs to F, which keeps its place.
    r.double_click(r.at(2850));
    r.frames(2);
    r.click_text("C");
    r.click_text("OK");
    let mut spans: Vec<(i64, i64)> = r.chords().iter().map(span).collect();
    spans.sort();
    assert_eq!(spans, [(192, 2592), (2784, 4500 - 2784), (4500, 2500)], "{:?}", r.chords());
    r.undo();
    // Further on: on the half bar, up to F.
    r.double_click(r.at(3700));
    r.frames(2);
    r.click_text("C");
    r.click_text("OK");
    assert_eq!(span(&r.chord("C")), (2 * HALF, 4500 - 2 * HALF));
    assert_eq!(span(&r.chord("F")), (4500, 2500));
}

#[test]
fn a_plain_resize_moves_only_the_edge_dragged() {
    let mut r = with_played_chords();
    // Am's right edge out to about bar 2: the end snaps, the start stays at 192.
    let edge = r.at(2784) - vec2(2.0, 0.0);
    r.drag(edge, r.at(2 * HALF + 60), Modifiers::NONE);
    assert_eq!(span(&r.chord("Am")), (192, 2 * HALF - 192), "{:?}", r.chords());
    r.undo();
    // F's left edge: the start snaps, the end stays at 7000.
    let edge = r.at(4500) + vec2(2.0, 0.0);
    r.drag(edge, r.at(2 * HALF + 80), Modifiers::NONE);
    assert_eq!(span(&r.chord("F")), (2 * HALF, 7000 - 2 * HALF), "{:?}", r.chords());
}
