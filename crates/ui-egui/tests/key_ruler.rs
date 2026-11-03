//! The Key ruler (docs/superpowers/specs/2026-10-26-key-ruler-midi-zoom.md, part 1): the key
//! picker (add and edit), pick and Delete, the right-click menus, Clear All behind an in-app
//! confirmation, dragging to move, and the empty ruler's text. Each action is one undo step.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::edit_window::{rect, x_of};
use soundcraft_ui_egui::ruler_items::RulerItem;
use soundcraft_ui_egui::{Services, SoundApp};

/// A bar of 4/4, in ticks.
const BAR: i64 = 3840;

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
        engine.execute("view.ruler", &json!({"ruler": "key", "visible": true})).unwrap();
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

    /// The Key ruler's centre line (its name, drawn first).
    fn ruler_y(&self) -> f32 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == "Key" => Some(t.galley.rect.translate(t.pos.to_vec2()).center().y),
                _ => None,
            })
            .expect("no Key ruler")
    }

    fn name_pos(&self) -> Pos2 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == "Key" => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                _ => None,
            })
            .expect("no Key ruler")
    }

    fn samples(&self, tick: i64) -> i64 {
        let s = self.app.engine.session();
        s.tempo.tick_to_samples(tick, s.sample_rate)
    }

    fn x(&self, tick: i64) -> f32 {
        let s = self.app.engine.session();
        x_of(s, rect(self.app.edit_layout.timeline), self.samples(tick))
    }

    /// On the key change at `tick` (its name, just right of its line).
    fn on_key(&self, tick: i64) -> Pos2 {
        pos2(self.x(tick) + 12.0, self.ruler_y())
    }

    fn at(&self, tick: i64) -> Pos2 {
        pos2(self.x(tick), self.ruler_y())
    }

    fn button(&mut self, at: Pos2, button: PointerButton, pressed: bool) {
        self.frame(vec![Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE }]);
    }

    fn press(&mut self, at: Pos2, button: PointerButton) {
        // Apart from earlier clicks, so they don't count as a double-click.
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

    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.frames(30);
        self.frame(vec![Event::PointerMoved(from)]);
        self.button(from, PointerButton::Primary, true);
        for k in 1..=6 {
            let p = from + (to - from) * (k as f32 / 6.0);
            self.frame(vec![Event::PointerMoved(p)]);
        }
        self.button(to, PointerButton::Primary, false);
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

    fn undo(&mut self) {
        self.app.run("edit.undo", json!({})).unwrap();
        self.frames(2);
    }

    fn keys(&self) -> Vec<(i64, String)> {
        self.app.engine.session().key_signatures.clone()
    }

    /// The grid line nearest `at`.
    fn grid(&self, at: i64) -> i64 {
        let s = self.app.engine.session();
        s.edit.grid.snap(at, s.sample_rate, &s.tempo, s.frame_rate)
    }
}

/// Key changes at bars 1, 3 and 5.
fn with_keys() -> Rig {
    let mut r = Rig::new();
    for (bar, k) in [(0, "C major"), (2, "A minor"), (4, "Eb major")] {
        let at = r.samples(BAR * bar);
        r.app.run("event.add_key_change", json!({"at": at, "key": k})).unwrap();
    }
    r.frames(3);
    r
}

#[test]
fn the_empty_ruler_says_no_key_set_or_shows_the_band_key_dimmed() {
    let mut r = Rig::new();
    assert!(r.has_text("No key set"), "{:?}", r.texts());
    assert!(!r.has_text("C major"));
    r.app.run("band.settings", json!({"key": "E", "scale": "minor"})).unwrap();
    r.frames(2);
    assert!(r.has_text("E minor (band key)"), "{:?}", r.texts());
    // The same rule as the Vocal Editor's "No key set".
    let s = soundcraft_model::Session::default();
    assert_eq!(soundcraft_ui_egui::session_key::empty_ruler_text(&s), "No key set");
    assert!(!soundcraft_ui_egui::vocal_editor::roll::has_key(&s));
}

#[test]
fn double_click_opens_the_key_picker_and_ok_adds_the_key() {
    let mut r = Rig::new();
    r.app.run("band.settings", json!({"key": "D", "scale": "major"})).unwrap();
    let p = r.at(2 * BAR + 300);
    r.double_click(p);
    assert_eq!(r.app.dialogs.open_name(), Some("key_picker"));
    r.frames(2);
    assert!(r.has_text("Bb") && r.has_text("Major") && r.has_text("Minor"), "{:?}", r.texts());
    // Cancel adds nothing.
    r.click_text("Cancel");
    assert!(r.app.dialogs.open.is_none());
    assert!(r.keys().is_empty());
    // It starts on the band key; pick E minor.
    r.double_click(p);
    r.frames(2);
    r.click_text("OK");
    let at = r.keys()[0].0;
    assert_eq!(r.keys(), [(at, "D major".to_string())], "the band key by default");
    r.undo();
    r.double_click(p);
    r.frames(2);
    r.click_text("Minor");
    r.click_text("Eb");
    r.click_text("OK");
    assert_eq!(r.keys().len(), 1);
    assert_eq!(r.keys()[0].1, "Eb minor", "minor keys spell their roots");
    r.undo();
    assert!(r.keys().is_empty(), "one undo step");
}

#[test]
fn the_picker_starts_on_the_key_in_force() {
    let mut r = with_keys();
    r.double_click(r.at(3 * BAR + BAR / 2));
    r.frames(2);
    r.click_text("OK");
    assert_eq!(r.keys().len(), 4);
    assert_eq!(r.keys()[2].1, "A minor", "A minor is in force at bar 4: {:?}", r.keys());
}

#[test]
fn a_clicked_key_goes_with_delete() {
    let mut r = with_keys();
    let am = r.samples(2 * BAR);
    r.click(r.on_key(2 * BAR));
    assert_eq!(r.app.ui.ruler_item, Some(RulerItem::Key(am)), "the click picks the key");
    r.key(Key::Delete);
    assert_eq!(r.keys().len(), 2);
    assert!(!r.keys().iter().any(|(t, _)| *t == am));
    assert_eq!(r.app.ui.ruler_item, None);
    r.undo();
    assert_eq!(r.keys().len(), 3);
    // A selection change drops the pick: Delete then leaves the keys alone.
    r.click(r.on_key(2 * BAR));
    assert!(r.app.ui.ruler_item.is_some());
    let track = r.app.engine.session().tracks[0].id.0;
    let (a, b) = (r.samples(0), r.samples(BAR));
    r.app.run("edit.select", json!({"tracks": [track], "start": a, "end": b})).unwrap();
    r.frames(2);
    assert_eq!(r.app.ui.ruler_item, None);
    r.key(Key::Delete);
    assert_eq!(r.keys().len(), 3);
    // Outside the Edit window too.
    r.click(r.on_key(2 * BAR));
    r.app.run("window.mix", json!({})).unwrap();
    r.key(Key::Delete);
    assert_eq!(r.keys().len(), 3);
}

#[test]
fn the_key_menu_edits_and_deletes() {
    let mut r = with_keys();
    r.right_click(r.on_key(2 * BAR));
    for item in ["Edit Key…", "Delete Key"] {
        assert!(r.has_text(item), "{item}: {:?}", r.texts());
    }
    r.click_text("Delete Key");
    assert_eq!(r.keys().len(), 2);
    r.undo();
    r.right_click(r.on_key(2 * BAR));
    r.click_text("Edit Key…");
    assert_eq!(r.app.dialogs.open_name(), Some("key_picker"));
    r.frames(2);
    r.click_text("Major");
    r.click_text("F");
    r.click_text("OK");
    assert_eq!(r.keys()[1], (r.samples(2 * BAR), "F major".to_string()));
    r.undo();
    assert_eq!(r.keys()[1].1, "A minor");
    // A double-click on a key edits it too (it starts on that key).
    r.double_click(r.on_key(4 * BAR));
    r.frames(2);
    r.click_text("Minor");
    r.click_text("OK");
    assert_eq!(r.keys().len(), 3, "edited, not added");
    assert_eq!(r.keys()[2].1, "Eb minor");
}

#[test]
fn the_ruler_name_clears_all_key_changes_after_asking() {
    let mut r = with_keys();
    r.right_click(pos2(r.name_pos().x, r.ruler_y()));
    r.click_text("Clear All Key Changes…");
    assert_eq!(r.app.dialogs.open_name(), Some("confirm"));
    r.frames(2);
    assert!(r.has_text("Remove 3 key changes from the Key ruler? Undo brings them back."), "{:?}", r.texts());
    r.click_text("Clear All");
    assert!(r.keys().is_empty());
    r.undo();
    assert_eq!(r.keys().len(), 3, "one undo step");
}

#[test]
fn a_dragged_key_moves_to_the_grid_in_one_undo_step() {
    let mut r = with_keys();
    let from = r.on_key(2 * BAR);
    let to = pos2(r.x(3 * BAR) + 12.0 + 7.0, from.y);
    r.drag(from, to);
    // Where the drag lands, from the key's own line, snapped to the grid.
    let want = {
        let s = r.app.engine.session();
        let tl = rect(r.app.edit_layout.timeline);
        let raw = soundcraft_ui_egui::edit_window::sample_at(s, tl, r.x(3 * BAR) + 7.0);
        r.grid(raw)
    };
    let ats: Vec<i64> = r.keys().iter().map(|k| k.0).collect();
    assert_eq!(ats, [0, want, r.samples(4 * BAR)], "moved: {:?}", r.keys());
    assert_eq!(r.keys()[1].1, "A minor");
    r.undo();
    assert_eq!(r.keys()[1].0, r.samples(2 * BAR), "one undo step");
}

#[test]
fn a_key_dragged_onto_another_is_refused() {
    let mut r = with_keys();
    let before = r.keys();
    let from = r.on_key(2 * BAR);
    r.drag(from, pos2(r.x(4 * BAR) + 12.0, from.y));
    assert_eq!(r.keys(), before, "nothing moved or lost");
    assert!(r.app.ui.status.contains("A key change is already there"), "{}", r.app.ui.status);
}

#[test]
fn the_empty_ruler_spells_the_band_key_as_the_picker_does() {
    let mut s = soundcraft_model::Session::default();
    s.band.key = 10;
    assert_eq!(soundcraft_ui_egui::session_key::empty_ruler_text(&s), "Bb major (band key)");
}
