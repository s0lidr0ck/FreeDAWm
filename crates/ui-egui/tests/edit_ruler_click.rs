//! A click on an Edit window timebase ruler puts the playhead there, also inside the timeline
//! selection (`transport.locate` alone keeps a selection that contains the time, and a stopped
//! playhead is drawn at, and plays from, its start).

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::demo::demo_engine();
        engine.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
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
}

#[test]
fn ruler_clicks_inside_the_selection_move_the_playhead() {
    let mut r = Rig::new();
    let y = r.label_pos("Bars|Beats").y;
    // A selection over the whole visible timeline (not the sticky loop: a click inside the
    // dimmed loop reselects it, `sticky_loop.rs`).
    r.app.run("edit.select", json!({"start": 0, "end": 48_000 * 600})).unwrap();
    r.app.run("loop.clear", json!({})).unwrap();
    r.frames(2);
    let mut last = None;
    for x in [700.0, 1000.0, 820.0] {
        r.click(Pos2::new(x, y));
        let s = r.app.engine.session();
        let sel = s.edit.selection;
        assert_eq!(sel.start, sel.end, "x {x}: an insertion point, not the old range");
        assert_eq!(sel.start, s.edit.playhead, "x {x}: the playhead is where the click was");
        assert!(sel.start > 0 && last != Some(sel.start), "x {x}: moved ({:?} → {})", last, sel.start);
        last = Some(sel.start);
    }
}
