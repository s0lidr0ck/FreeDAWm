//! Plain Delete (or Backspace) in the Edit window is Clear for the clip selection, unless a ruler
//! item is picked, a text field has focus, or there is nothing to clear (`shortcuts::delete_clears`).

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, vec2};
use serde_json::json;
use soundcraft_model::{Clip, ClipId};
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::ruler_items::RulerItem;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
    time: f64,
}

impl Rig {
    /// The demo session, zoomed out, with a marker on ruler 1.
    fn new() -> Rig {
        let mut engine = soundcraft_engine::demo::demo_engine();
        engine.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
        engine.execute("markers.add", &json!({"name": "Pickme", "at": 0, "ruler": 1})).unwrap();
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

    fn label_pos(&self, label: &str) -> Pos2 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label} label"))
    }

    fn click(&mut self, at: Pos2) {
        self.frames(30);
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        self.frames(3);
    }

    fn key(&mut self, key: Key) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
    }

    fn kick(&self) -> Vec<Clip> {
        self.app.engine.session().track_by_name("Kick").unwrap().clips().to_vec()
    }

    fn has_clip(&self, id: ClipId) -> bool {
        self.app.engine.session().find_clip(id).is_some()
    }

    fn markers(&self) -> Vec<String> {
        self.app.engine.session().markers.iter().map(|m| m.name.clone()).collect()
    }

    fn undo_count(&self) -> usize {
        self.app.engine.undo_history().len()
    }
}

#[test]
fn delete_clears_two_selected_clips_in_one_undo_step() {
    for key in [Key::Delete, Key::Backspace] {
        let mut r = Rig::new();
        let before = r.kick();
        assert!(before.len() >= 2, "the demo Kick track has clips to clear");
        let (a, b) = (before[0].id, before[1].id);
        r.app.run("edit.select", json!({"clips": [a.0, b.0]})).unwrap();
        r.frames(2);
        let steps = r.undo_count();
        r.key(key);
        assert!(!r.has_clip(a) && !r.has_clip(b), "{key:?} clears both clips");
        assert_eq!(r.undo_count(), steps + 1, "{key:?}: one undo step");
        r.app.run("edit.undo", json!({})).unwrap();
        assert!(r.has_clip(a) && r.has_clip(b), "{key:?}: undo brings both back");
        assert_eq!(r.kick(), before);
    }
}

#[test]
fn delete_with_a_live_marker_pick_deletes_the_marker_not_the_clips() {
    let mut r = Rig::new();
    let before = r.kick();
    r.app.run("edit.select", json!({"clips": [before[0].id.0]})).unwrap();
    r.frames(2);
    // The pick comes after the clip selection, so it is live.
    r.click(r.label_pos("Pickme"));
    assert!(matches!(r.app.ui.ruler_item, Some(RulerItem::Marker(_))), "the click picks the marker");
    r.key(Key::Delete);
    assert!(!r.markers().iter().any(|m| m == "Pickme"), "the marker is gone");
    assert_eq!(r.kick(), before, "the clips stay");
}

#[test]
fn delete_in_a_rename_field_edits_the_text_only() {
    for key in [Key::Delete, Key::Backspace] {
        let mut r = Rig::new();
        let before = r.kick();
        let id = before[0].id;
        r.app.run("edit.select", json!({"clips": [id.0]})).unwrap();
        r.app.dialogs.open_rename_clip(id, "Kick 1");
        r.frames(3);
        r.key(key);
        // The field opens with the old name selected, so the key clears it.
        assert!(matches!(&r.app.dialogs.open, Some(Dialog::RenameClip { name, .. }) if name.is_empty()), "{key:?}: {:?}", r.app.dialogs.open_name());
        assert_eq!(r.kick(), before, "{key:?} left the clips alone");
    }
}

#[test]
fn delete_with_nothing_selected_changes_nothing() {
    let mut r = Rig::new();
    r.app.run("edit.select", json!({"clips": [], "tracks": []})).unwrap();
    r.frames(2);
    let session = r.app.engine.session().clone();
    let steps = r.undo_count();
    r.key(Key::Delete);
    assert!(*r.app.engine.session() == session, "nothing changed");
    assert_eq!(r.undo_count(), steps, "no undo step");
}

#[test]
fn delete_in_the_mix_window_does_nothing() {
    let mut r = Rig::new();
    let before = r.kick();
    r.app.run("edit.select", json!({"clips": [before[0].id.0]})).unwrap();
    r.app.run("window.mix", json!({})).unwrap();
    r.frames(2);
    r.key(Key::Delete);
    assert_eq!(r.kick(), before, "the Mix window has no Delete binding");
}
