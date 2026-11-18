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

    /// A press and its release (egui counts a second press of a key still held as a repeat).
    fn key(&mut self, key: Key) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
    }

    fn key_with(&mut self, key: Key, mods: Modifiers) {
        self.frame(vec![Event::ModifiersChanged(mods), Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods }]);
        self.frame(vec![
            Event::ModifiersChanged(Modifiers::NONE),
            Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: mods },
        ]);
        self.frames(2);
    }

    /// One event as the window system sends it (`Event::Copy` for Ctrl+C…), then a few frames.
    fn send(&mut self, e: Event) {
        self.frame(vec![e]);
        self.frames(2);
    }

    fn shift_click(&mut self, at: Pos2) {
        self.frames(30);
        let m = Event::ModifiersChanged(Modifiers::SHIFT);
        self.frame(vec![m.clone(), Event::PointerMoved(at)]);
        self.frame(vec![m.clone(), Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT }]);
        self.frame(vec![m, Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::SHIFT }]);
        self.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.frames(3);
    }

    /// The insertion point to `tick` (a click on the timeline does this).
    fn insert_at(&mut self, tick: i64) {
        let at = self.samples(tick);
        self.app.run("edit.select", json!({"start": at, "end": at})).unwrap();
        self.frames(2);
    }

    /// The chords picked, by name, in time order.
    fn picked(&self) -> Vec<String> {
        let ids = soundcraft_ui_egui::ruler_items::picked_chords(&self.app);
        let mut v: Vec<(i64, String)> = Vec::new();
        let st = &self.app.engine.session().band;
        let (k, sc) = (i32::from(st.key), i32::from(st.scale));
        for c in st.chords.iter().filter(|c| ids.contains(&c.id)) {
            v.push((c.tick, soundcraft_band::chord_theory::name_for(i32::from(c.slot), k, sc, i32::from(c.bass))));
        }
        v.sort();
        v.into_iter().map(|(_, n)| n).collect()
    }

    /// (tick, name) of every chord, in time order.
    fn lane(&self) -> Vec<(i64, String)> {
        let mut v: Vec<(i64, String)> = self.chords().into_iter().map(|c| (c.0, c.2)).collect();
        v.sort();
        v
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

// ---- copying chords ---------------------------------------------------------------------------

fn copy_keys(r: &mut Rig, key: Key) {
    r.key_with(key, Modifiers::COMMAND);
}

#[test]
fn copy_and_paste_a_chord_at_the_insertion_on_the_grid() {
    let mut r = with_chords();
    r.click(r.at(3 * HALF));
    copy_keys(&mut r, Key::C);
    assert_eq!(r.app.engine.clipboard.chords.len(), 1, "Am is copied");
    // The insertion a little past bar 6 (where a click on the timeline puts it): the copy lands on
    // bar 6, a bar long, and becomes the pick.
    r.insert_at(10 * HALF + 100);
    copy_keys(&mut r, Key::V);
    assert!(r.chords().contains(&(10 * HALF, 2 * HALF, "Am".into())), "{:?}", r.chords());
    assert_eq!(r.picked(), ["Am"]);
    assert_eq!(r.chords().len(), 5);
    r.undo();
    assert_eq!(r.chords().len(), 4, "one undo step");

    // The copy, cut and paste events the window system sends for the keys do the same.
    r.click(r.at(5 * HALF));
    r.send(Event::Copy);
    r.insert_at(12 * HALF);
    r.send(Event::Paste("F".into()));
    assert!(r.lane().contains(&(12 * HALF, "F".into())), "{:?}", r.lane());

    // Cut: G goes onto the clipboard and off the lane; pasted over C at bar 1 it replaces it, as
    // a chord dragged there does.
    r.click(r.at(7 * HALF));
    r.send(Event::Cut);
    assert!(!r.lane().contains(&(6 * HALF, "G".into())), "{:?}", r.lane());
    assert_eq!(r.app.ui.ruler_item, None);
    r.insert_at(0);
    copy_keys(&mut r, Key::V);
    assert_eq!(r.lane()[0], (0, "G".into()));
    assert!(!r.lane().iter().any(|c| c.1 == "C"), "the paste cut what it landed on: {:?}", r.lane());
}

#[test]
fn shift_click_picks_a_run_that_copies_with_its_gaps() {
    let mut r = with_chords();
    // Am out: a gap between C and F.
    let am = r.app.engine.session().band.chords.iter().find(|c| c.tick == 2 * HALF).unwrap().id;
    r.app.run("band.chord.remove", json!({"chord": am})).unwrap();
    r.frames(2);
    r.click(r.at(HALF));
    r.shift_click(r.at(7 * HALF));
    assert_eq!(r.picked(), ["C", "F", "G"], "the run from C to G");
    r.shift_click(r.at(5 * HALF));
    assert_eq!(r.picked(), ["C", "F"], "a second Shift-click moves the run's end");
    copy_keys(&mut r, Key::C);
    r.insert_at(16 * HALF);
    copy_keys(&mut r, Key::V);
    let lane = r.lane();
    assert!(lane.contains(&(16 * HALF, "C".into())) && lane.contains(&(20 * HALF, "F".into())), "the gap kept: {lane:?}");
    assert_eq!(r.picked(), ["C", "F"], "the pasted run is picked");
    // Delete takes the whole run.
    r.key(Key::Delete);
    assert_eq!(r.chords().len(), 3, "{:?}", r.lane());
    // A plain click picks one chord again.
    r.click(r.at(5 * HALF));
    assert_eq!(r.picked(), ["F"]);
}

#[test]
fn duplicate_puts_a_copy_after_and_repeats() {
    let mut r = with_chords();
    r.click(r.at(3 * HALF));
    copy_keys(&mut r, Key::D);
    // Straight after Am, over F; the copy is the pick, so again goes on over G.
    assert_eq!(r.lane(), [(0, "C".to_string()), (2 * HALF, "Am".to_string()), (4 * HALF, "Am".to_string()), (6 * HALF, "G".to_string())]);
    copy_keys(&mut r, Key::D);
    assert_eq!(r.lane().last(), Some(&(6 * HALF, "Am".into())));
    r.undo();
    r.undo();
    assert_eq!(
        r.lane(),
        [(0, "C".to_string()), (2 * HALF, "Am".to_string()), (4 * HALF, "F".to_string()), (6 * HALF, "G".to_string())],
        "a step each"
    );
    // A run: after its last chord.
    r.click(r.at(HALF));
    r.shift_click(r.at(3 * HALF));
    copy_keys(&mut r, Key::D);
    assert_eq!(r.lane(), [(0, "C".to_string()), (2 * HALF, "Am".to_string()), (4 * HALF, "C".to_string()), (6 * HALF, "Am".to_string())]);
    assert_eq!(r.picked(), ["C", "Am"]);
}

#[test]
fn arrows_move_and_extend_the_pick() {
    let mut r = with_chords();
    // Nothing picked: the arrows don't pick anything.
    r.key(Key::ArrowRight);
    assert_eq!(r.app.ui.ruler_item, None);
    r.click(r.at(3 * HALF));
    r.key(Key::ArrowRight);
    assert_eq!(r.picked(), ["F"]);
    r.key_with(Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(r.picked(), ["F", "G"]);
    r.key_with(Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(r.picked(), ["F", "G"], "the lane's end");
    r.key_with(Key::ArrowLeft, Modifiers::SHIFT);
    r.key_with(Key::ArrowLeft, Modifiers::SHIFT);
    assert_eq!(r.picked(), ["Am", "F"]);
    r.key(Key::ArrowLeft);
    assert_eq!(r.picked(), ["C"]);
    r.key(Key::ArrowLeft);
    assert_eq!(r.picked(), ["C"], "the lane's start");
    assert_eq!(r.chords().len(), 4, "the arrows change no chord");
}

#[test]
fn ctrl_drag_drops_a_copy_and_leaves_the_chord() {
    let mut r = with_chords();
    let steps = r.app.engine.undo_history().len();
    // Am's body, Ctrl held: a copy at bar 6 (on the grid).
    r.drag(r.at(3 * HALF), r.at(11 * HALF + HALF / 5), Modifiers::CTRL);
    assert!(r.lane().contains(&(2 * HALF, "Am".into())), "the original stays: {:?}", r.lane());
    assert!(r.chords().contains(&(10 * HALF, 2 * HALF, "Am".into())), "{:?}", r.chords());
    assert_eq!(r.app.engine.undo_history().len(), steps + 1, "one undo step");
    assert_eq!(r.picked(), ["Am"]);
    r.undo();
    assert_eq!(r.chords().len(), 4);
    // Ctrl and Alt: a copy off the grid.
    let d = HALF * 7 / 10;
    r.drag(r.at(3 * HALF), r.at(3 * HALF + 4 * HALF + d), Modifiers::CTRL | Modifiers::ALT);
    let copy = r.chords().into_iter().find(|c| c.2 == "Am" && c.0 != 2 * HALF).expect("a copy");
    assert!((copy.0 - (6 * HALF + d)).abs() < 40 && copy.0 % HALF != 0, "off the grid: {copy:?}");
    assert!(r.lane().contains(&(2 * HALF, "Am".into())));
}

/// A drag of Am's body with `before` held until halfway and `after` from then to the release.
fn drag_changing_mods(r: &mut Rig, before: Modifiers, after: Modifiers) {
    let (from, to) = (r.at(3 * HALF), r.at(11 * HALF));
    r.frames(30);
    r.frame(vec![Event::ModifiersChanged(before), Event::PointerMoved(from)]);
    r.frame(vec![Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: before }]);
    for k in 1..=6 {
        let m = if k > 3 { after } else { before };
        r.frame(vec![Event::ModifiersChanged(m), Event::PointerMoved(from + (to - from) * (k as f32 / 6.0))]);
    }
    r.frame(vec![Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: after }]);
    r.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    r.frames(3);
}

#[test]
fn ctrl_pressed_or_let_go_mid_drag_decides_at_the_drop() {
    let mut r = with_chords();
    drag_changing_mods(&mut r, Modifiers::NONE, Modifiers::CTRL);
    assert_eq!(r.chords().len(), 5, "pressed mid-drag: a copy {:?}", r.lane());
    assert!(r.lane().contains(&(2 * HALF, "Am".into())));
    r.undo();
    drag_changing_mods(&mut r, Modifiers::CTRL, Modifiers::NONE);
    assert_eq!(r.chords().len(), 4, "let go mid-drag: a move {:?}", r.lane());
    assert!(r.lane().contains(&(10 * HALF, "Am".into())) && !r.lane().contains(&(2 * HALF, "Am".into())));
}

#[test]
fn clip_copy_and_paste_are_untouched_without_a_chord_pick() {
    let mut r = with_chords();
    let t = r.app.run("track.new", json!({"kind": "instrument", "name": "Keys"})).unwrap()["tracks"][0].as_u64().unwrap();
    let bar = r.samples(2 * HALF);
    r.app.run("midi.new_clip", json!({"track": t, "start": 0, "length": bar})).unwrap();
    let clips = |r: &Rig| r.app.engine.session().track(soundcraft_model::TrackId(t)).map_or(0, |t| t.clips().len());
    // A chord picked, then the clip's range selected: the pick is dropped, so Copy copies the clip.
    r.click(r.at(3 * HALF));
    r.app.run("edit.select", json!({"tracks": [t], "start": 0, "end": bar})).unwrap();
    r.frames(2);
    copy_keys(&mut r, Key::C);
    assert!(r.app.engine.clipboard.chords.is_empty() && !r.app.engine.clipboard.tracks.is_empty(), "the clip is copied");
    r.app.run("edit.select", json!({"tracks": [t], "start": 4 * bar, "end": 4 * bar})).unwrap();
    r.frames(2);
    copy_keys(&mut r, Key::V);
    assert_eq!(clips(&r), 2, "the clip is pasted");
    assert_eq!(r.chords().len(), 4, "no chord moved");
}

#[test]
fn the_chord_menu_copies_cuts_pastes_here_and_duplicates() {
    let mut r = with_chords();
    r.right_click(r.at(3 * HALF));
    for item in ["Copy Chord", "Cut Chord", "Paste Here", "Duplicate Chord", "Delete Chord"] {
        assert!(r.has_text(item), "{item}: {:?}", r.texts());
    }
    r.click_text("Copy Chord");
    assert_eq!(r.app.engine.clipboard.chords.len(), 1);
    // Paste Here on empty space, where the menu opened (snapped to the half bar). Zoomed out so
    // there is empty ruler past G.
    r.app.run("view.zoom_set", json!({"samples_per_px": 800.0})).unwrap();
    r.frames(3);
    r.right_click(r.at(12 * HALF + HALF / 5));
    r.click_text("Paste Here");
    assert!(r.lane().contains(&(12 * HALF, "Am".into())), "{:?}", r.lane());
    r.right_click(r.at(HALF));
    r.click_text("Duplicate Chord");
    assert_eq!(r.lane()[..2], [(0, "C".to_string()), (2 * HALF, "C".to_string())]);
    r.right_click(r.at(7 * HALF));
    r.click_text("Cut Chord");
    assert!(!r.lane().iter().any(|c| c.1 == "G"), "{:?}", r.lane());
    assert_eq!(r.app.engine.clipboard.chords.len(), 1);
}
