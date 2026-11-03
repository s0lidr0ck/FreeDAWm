//! Deleting markers and Band chords from the rulers in the Edit window (docs/superpowers/specs/
//! 2026-10-25-presets-markers-templates.md, part 2): click to pick and Delete, the right-click
//! menus, Delete in Selection, and Clear All behind an in-app confirmation. Each is one undo step.

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

    /// The centre line of the ruler labelled `label` (its name, drawn first).
    fn ruler_y(&self, label: &str) -> f32 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2()).center().y),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label} ruler"))
    }

    fn label_pos(&self, label: &str) -> Pos2 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label} label"))
    }

    fn x(&self, tick: i64) -> f32 {
        let s = self.app.engine.session();
        x_of(s, rect(self.app.edit_layout.timeline), s.tempo.tick_to_samples(tick, s.sample_rate))
    }

    fn at(&self, label: &str, tick: i64) -> Pos2 {
        pos2(self.x(tick), self.ruler_y(label))
    }

    fn press(&mut self, at: Pos2, button: PointerButton) {
        // Apart from earlier clicks, so they don't count as a double-click.
        self.frames(30);
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Event::PointerButton { pos: at, button, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos: at, button, pressed: false, modifiers: Modifiers::NONE }]);
        self.frames(3);
    }

    fn click(&mut self, at: Pos2) {
        self.press(at, PointerButton::Primary);
    }

    fn right_click(&mut self, at: Pos2) {
        self.press(at, PointerButton::Secondary);
    }

    /// Click the item labelled `label` (a menu item or a button).
    fn click_text(&mut self, label: &str) {
        let r = self.text_rect(label).unwrap_or_else(|| panic!("no `{label}` in {:?}", self.texts()));
        self.click(r.center());
    }

    fn key(&mut self, key: Key) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
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

    fn chords(&self) -> usize {
        self.app.engine.session().band.chords.len()
    }

    fn markers(&self) -> Vec<String> {
        self.app.engine.session().markers.iter().map(|m| m.name.clone()).collect()
    }

    fn samples(&self, tick: i64) -> i64 {
        let s = self.app.engine.session();
        s.tempo.tick_to_samples(tick, s.sample_rate)
    }

    fn undo(&mut self) {
        self.app.run("edit.undo", json!({})).unwrap();
        self.frames(2);
    }
}

fn with_chords() -> Rig {
    let mut r = Rig::new();
    for (k, c) in ["C", "Am", "F", "G"].iter().enumerate() {
        let at = r.samples(BAR * k as i64);
        r.app.run("band.chord.add", json!({"at": at, "end": at + r.samples(BAR), "chord": c})).unwrap();
    }
    r.frames(3);
    r
}

fn with_markers() -> Rig {
    let mut r = Rig::new();
    for (name, bar) in [("Intro", 0), ("Verse", 2), ("Chorus", 4)] {
        let at = r.samples(BAR * bar);
        r.app.run("markers.add", json!({"name": name, "at": at, "ruler": 1})).unwrap();
    }
    r.frames(3);
    r
}

#[test]
fn a_clicked_chord_goes_with_delete_and_comes_back_with_undo() {
    let mut r = with_chords();
    r.click(r.at("Chords", BAR + BAR / 2));
    let am = r.app.engine.session().band.chords.iter().find(|c| c.tick == BAR).unwrap().id;
    assert_eq!(r.app.ui.ruler_item, Some(RulerItem::Chord(am)), "the click picks the chord");
    r.key(Key::Delete);
    assert_eq!(r.chords(), 3);
    assert!(!r.app.engine.session().band.chords.iter().any(|c| c.id == am));
    assert_eq!(r.app.ui.ruler_item, None);
    r.undo();
    assert_eq!(r.chords(), 4);
    // A click elsewhere drops the pick: Delete then leaves the chords alone.
    r.click(r.at("Chords", BAR / 2));
    r.click(pos2(800.0, 600.0));
    assert_eq!(r.app.ui.ruler_item, None);
    r.key(Key::Delete);
    assert_eq!(r.chords(), 4);
}

#[test]
fn the_chord_menu_deletes_one_the_selection_or_all_after_asking() {
    let mut r = with_chords();
    r.right_click(r.at("Chords", BAR * 2 + BAR / 2));
    for item in ["Edit Chord…", "Delete Chord", "Delete Chords in Selection", "Clear All Chords…"] {
        assert!(r.has_text(item), "{item}: {:?}", r.texts());
    }
    r.click_text("Delete Chord");
    assert_eq!(r.chords(), 3, "F is gone");
    r.undo();

    // In the edit selection: from bar 2 to bar 4 (Am and F start inside).
    let (a, b) = (r.samples(BAR), r.samples(3 * BAR));
    r.app.run("edit.select", json!({"start": a, "end": b})).unwrap();
    r.right_click(r.at("Chords", BAR / 2));
    r.click_text("Delete Chords in Selection");
    let left: Vec<i64> = r.app.engine.session().band.chords.iter().map(|c| c.tick).collect();
    assert_eq!(left, [0, 3 * BAR]);
    r.undo();

    // Clear All asks first, in the app; Cancel keeps them.
    r.right_click(r.at("Chords", BAR / 2));
    r.click_text("Clear All Chords…");
    assert_eq!(r.app.dialogs.open_name(), Some("confirm"));
    r.frames(2);
    r.click_text("Cancel");
    assert_eq!(r.chords(), 4);
    assert!(r.app.dialogs.open.is_none());
    r.right_click(r.at("Chords", BAR / 2));
    r.click_text("Clear All Chords…");
    r.frames(2);
    assert!(r.has_text("Remove 4 chords from the Chords ruler? Undo brings them back."), "{:?}", r.texts());
    r.click_text("Clear All");
    assert_eq!(r.chords(), 0);
    r.undo();
    assert_eq!(r.chords(), 4, "one undo step");
}

#[test]
fn edit_chord_opens_the_chord_box() {
    let mut r = with_chords();
    r.right_click(r.at("Chords", BAR / 2));
    r.click_text("Edit Chord…");
    r.frames(2);
    // The box holds the chord's name, ready to retype.
    r.frame(vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }]);
    r.frame(vec![Event::Text("Dm".into())]);
    r.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    r.frames(2);
    let st = &r.app.engine.session().band;
    let c = st.chords.iter().find(|c| c.tick == 0).unwrap();
    let name = soundcraft_band::chord_theory::name_for(i32::from(c.slot), i32::from(st.key), i32::from(st.scale), i32::from(c.bass));
    assert_eq!(name, "Dm");
}

#[test]
fn a_clicked_marker_goes_with_delete() {
    let mut r = with_markers();
    let verse = r.label_pos("Verse");
    r.click(verse);
    let id = r.app.engine.session().markers.iter().find(|m| m.name == "Verse").unwrap().id;
    assert_eq!(r.app.ui.ruler_item, Some(RulerItem::Marker(id)), "a click on the name picks it too");
    r.key(Key::Backspace);
    assert_eq!(r.markers(), ["Intro", "Chorus"]);
    r.undo();
    assert_eq!(r.markers().len(), 3);
}

#[test]
fn the_marker_menu_and_the_ruler_name_clear_markers() {
    let mut r = with_markers();
    r.right_click(r.label_pos("Chorus"));
    for item in ["Edit Marker…", "Delete Marker", "Delete Markers in Selection", "Clear All Markers…"] {
        assert!(r.has_text(item), "{item}: {:?}", r.texts());
    }
    r.click_text("Delete Marker");
    assert_eq!(r.markers(), ["Intro", "Verse"]);
    r.undo();

    let (a, b) = (r.samples(BAR), r.samples(5 * BAR));
    r.app.run("edit.select", json!({"start": a, "end": b})).unwrap();
    r.right_click(r.label_pos("Intro"));
    r.click_text("Delete Markers in Selection");
    assert_eq!(r.markers(), ["Intro"]);
    r.undo();

    // The ruler's name: Clear All Markers…, confirmed.
    let name = pos2(r.label_pos("Markers").x, r.ruler_y("Markers"));
    r.right_click(name);
    r.click_text("Clear All Markers…");
    r.frames(2);
    assert!(r.has_text("Remove 3 markers from the Markers ruler? Undo brings them back."), "{:?}", r.texts());
    r.click_text("Clear All");
    assert!(r.markers().is_empty());
    r.undo();
    assert_eq!(r.markers().len(), 3);
}

/// Review #1: a pick goes stale once the selection moves on, so Delete no longer reaches it.
#[test]
fn a_selection_change_drops_the_pick() {
    let mut r = with_markers();
    r.click(r.label_pos("Verse"));
    assert!(matches!(r.app.ui.ruler_item, Some(RulerItem::Marker(_))));
    // Select a clip (as a click on the timeline or an agent would): the marker is not what
    // Delete means any more.
    let track = r.app.engine.session().tracks[0].id.0;
    let (a, b) = (r.samples(0), r.samples(BAR));
    r.app.run("edit.select", json!({"tracks": [track], "start": a, "end": b})).unwrap();
    r.frames(2);
    assert_eq!(r.app.ui.ruler_item, None, "the pick is dropped");
    r.key(Key::Delete);
    assert_eq!(r.markers(), ["Intro", "Verse", "Chorus"], "the marker stays");
}

#[test]
fn the_mix_window_drops_the_pick() {
    let mut r = with_markers();
    r.click(r.label_pos("Verse"));
    assert!(r.app.ui.ruler_item.is_some());
    r.app.run("window.mix", json!({})).unwrap();
    r.key(Key::Delete);
    assert_eq!(r.markers(), ["Intro", "Verse", "Chorus"], "Delete in the Mix window leaves the marker");
    assert_eq!(r.app.ui.ruler_item, None);
}

#[test]
fn a_pick_whose_item_went_away_or_a_new_session_lets_delete_through() {
    let mut r = with_markers();
    r.click(r.label_pos("Verse"));
    let id = r.app.engine.session().markers.iter().find(|m| m.name == "Verse").unwrap().id.0;
    // Deleted another way (Memory Locations, an agent): the pick goes, Delete deletes nothing more.
    r.app.run("markers.delete", json!({"id": id})).unwrap();
    assert!(!soundcraft_ui_egui::ruler_items::delete_picked(&mut r.app), "nothing live to delete");
    assert_eq!(r.markers(), ["Intro", "Chorus"]);
    r.click(r.label_pos("Chorus"));
    assert!(r.app.ui.ruler_item.is_some());
    r.app.run_sync("session.new", json!({})).unwrap();
    assert_eq!(r.app.ui.ruler_item, None, "a new session drops the pick");
}

/// Review #1, with a clip: once a clip is selected after the pick, the pick is dropped and Delete
/// goes to the normal key handling, which is Clear for the selected clip (the marker stays).
#[test]
fn delete_falls_through_to_a_clip_selected_after_the_pick() {
    let mut engine = soundcraft_engine::demo::demo_engine();
    engine.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
    let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new(), time: 1.0 };
    r.frames(4);
    r.app.run("markers.add", json!({"name": "Pickme", "at": 0, "ruler": 1})).unwrap();
    r.frames(3);
    r.click(r.label_pos("Pickme"));
    assert!(matches!(r.app.ui.ruler_item, Some(RulerItem::Marker(_))));
    let kick = |r: &Rig| r.app.engine.session().track_by_name("Kick").unwrap().clips().to_vec();
    let before = kick(&r);
    let clip = before[0].id;
    r.app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    r.frames(2);
    r.key(Key::Delete);
    assert_eq!(r.app.ui.ruler_item, None, "selecting the clip dropped the pick");
    assert!(r.markers().iter().any(|m| m == "Pickme"), "the marker stays");
    assert!(!kick(&r).iter().any(|c| c.id == clip), "plain Delete clears the selected clip");
    assert_eq!(kick(&r).len(), before.len() - 1);
}
