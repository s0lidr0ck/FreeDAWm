//! Dragging a marker on its ruler moves it (one undo step); Esc cancels.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({})).unwrap();
        engine.execute("markers.add", &json!({"name": "Verse", "at": 96_000})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new() };
        r.frames(4);
        r
    }

    fn frame(&mut self, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
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

    /// The flag of the ruler marker named `name` (its label starts 8pt to the right).
    fn flag(&self, name: &str) -> Pos2 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == name => Some(pos2(t.pos.x - 8.0, t.galley.rect.translate(t.pos.to_vec2()).center().y)),
                _ => None,
            })
            .expect("marker not on the ruler")
    }

    fn drag(&mut self, from: Pos2, to: Pos2, cancel: bool) {
        self.frame(vec![
            Event::PointerMoved(from),
            Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        ]);
        for k in 1..=6 {
            self.frame(vec![Event::PointerMoved(from + (to - from) * (k as f32 / 6.0))]);
        }
        if cancel {
            self.frame(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
            self.frame(vec![Event::PointerMoved(to + vec2(5.0, 0.0))]);
        }
        self.frame(vec![Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
    }

    fn at(&self) -> i64 {
        self.app.engine.session().markers[0].start
    }
}

#[test]
fn dragging_a_ruler_marker_moves_it_in_one_undo_step() {
    let mut r = Rig::new();
    r.app.engine.execute("edit.mode", &json!({"mode": "slip"})).unwrap();
    let flag = r.flag("Verse");
    r.drag(flag, flag + vec2(200.0, 0.0), false);
    let spp = r.app.engine.session().edit.zoom.samples_per_px;
    let moved = r.at() - 96_000;
    assert!((moved as f64 - 200.0 * spp).abs() <= 2.0 * spp, "moved {moved}, want about {}", 200.0 * spp);
    assert_eq!(r.app.engine.session().markers.len(), 1, "a drag never adds a marker");
    r.app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(r.at(), 96_000, "one undo step");
}

#[test]
fn escape_cancels_a_marker_drag() {
    let mut r = Rig::new();
    let flag = r.flag("Verse");
    r.drag(flag, flag + vec2(200.0, 0.0), true);
    assert_eq!(r.at(), 96_000);
    assert_eq!(r.app.engine.session().markers.len(), 1);
}
