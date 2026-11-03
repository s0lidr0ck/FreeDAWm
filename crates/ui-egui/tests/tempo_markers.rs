//! The Tempo ruler's markers: drag left/right moves a tempo change (grid snap), up/down changes its
//! BPM, double-click types it, double-click on empty ruler adds one, right-click › Delete / Ramp to
//! Next; each gesture is one undo step.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

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
        engine.execute("view.ruler_minimal", &json!({})).unwrap();
        engine.execute("view.ruler", &json!({"ruler": "tempo", "visible": true})).unwrap();
        engine.execute("event.tempo", &json!({"bpm": 100.0, "at": 0})).unwrap();
        let at = engine.session().tempo.tick_to_samples(2 * BAR, engine.session().sample_rate);
        engine.execute("event.tempo", &json!({"bpm": 140.0, "at": at})).unwrap();
        engine.execute("edit.mode", &json!({"mode": "grid"})).unwrap();
        engine.execute("view.grid", &json!({"value": "1 bar"})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new(), time: 1.0 };
        r.frames(4, vec![]);
        r
    }

    fn frames(&mut self, n: usize, mut events: Vec<Event>) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
                events: std::mem::take(&mut events),
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

    /// The left-centre of the last painted text equal to `text`.
    fn find(&self, text: &str) -> Pos2 {
        self.shapes
            .iter()
            .rev()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == text => {
                    let r = t.galley.rect.translate(t.pos.to_vec2());
                    Some(pos2(r.min.x, r.center().y))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {text:?} in {:?}", self.texts()))
    }

    fn press(&mut self, at: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) {
        self.frames(1, vec![Event::PointerButton { pos: at, button, pressed, modifiers }]);
    }

    fn click(&mut self, at: Pos2, button: PointerButton) {
        self.frames(1, vec![Event::PointerMoved(at)]);
        self.press(at, button, true, Modifiers::NONE);
        self.press(at, button, false, Modifiers::NONE);
        self.frames(3, vec![]);
    }

    fn double_click(&mut self, at: Pos2) {
        self.frames(1, vec![Event::PointerMoved(at)]);
        for _ in 0..2 {
            self.press(at, PointerButton::Primary, true, Modifiers::NONE);
            self.press(at, PointerButton::Primary, false, Modifiers::NONE);
        }
        self.frames(3, vec![]);
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        self.frames(1, vec![Event::PointerMoved(from)]);
        self.press(from, PointerButton::Primary, true, modifiers);
        for k in 1..=8 {
            let p = from + (to - from) * (k as f32 / 8.0);
            self.frames(1, vec![Event::PointerMoved(p), Event::ModifiersChanged(modifiers)]);
        }
        self.press(to, PointerButton::Primary, false, modifiers);
        self.frames(3, vec![]);
    }

    fn tempos(&self) -> Vec<(i64, f64, bool)> {
        self.app.engine.session().tempo.tempos().iter().map(|t| (t.tick, t.bpm, t.ramp)).collect()
    }
}

/// The flag of the marker labelled `label`: just left of its text.
fn flag(r: &Rig, label: &str) -> Pos2 {
    r.find(label) - vec2(6.0, 0.0)
}

#[test]
fn drag_sideways_moves_the_change_on_the_grid_and_undoes() {
    let mut r = Rig::new();
    let f = flag(&r, "140");
    r.drag(f, f + vec2(160.0, 0.0), Modifiers::NONE);
    let t = r.tempos();
    assert_eq!(t.len(), 2, "{t:?}");
    assert_eq!(t[1].1, 140.0, "the tempo stays");
    assert!(t[1].0 > 2 * BAR, "moved later: {t:?}");
    assert_eq!(t[1].0 % BAR, 0, "on a bar line (grid 1 bar): {t:?}");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.tempos()[1].0, 2 * BAR, "one undo step");
}

#[test]
fn drag_up_raises_the_bpm_and_shift_is_fine() {
    let mut r = Rig::new();
    let f = flag(&r, "140");
    r.drag(f, f - vec2(0.0, 40.0), Modifiers::NONE);
    assert_eq!(r.tempos()[1], (2 * BAR, 150.0, false));
    r.frames(2, vec![]);
    let f = flag(&r, "150");
    r.drag(f, f + vec2(0.0, 40.0), Modifiers::SHIFT);
    assert_eq!(r.tempos()[1].1, 149.0);
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.tempos()[1].1, 150.0);
}

#[test]
fn double_click_types_the_bpm_or_adds_a_change() {
    let mut r = Rig::new();
    let f = flag(&r, "140");
    r.double_click(f);
    assert_eq!(r.app.dialogs.open_name(), Some("tempo_change"));
    r.app.dialogs.open = None;
    r.frames(40, vec![]);
    // Empty ruler after the last change: a change there at the tempo in effect.
    r.double_click(pos2(f.x + 150.0, f.y));
    let t = r.tempos();
    assert_eq!(t.len(), 3, "{t:?}");
    assert_eq!(t[2].1, 140.0);
    assert_eq!(t[2].0 % BAR, 0, "on the grid");
}

#[test]
fn right_click_deletes_or_ramps() {
    let mut r = Rig::new();
    let a = flag(&r, "100");
    r.click(a, PointerButton::Secondary);
    let ramp = r.find("Ramp to Next");
    r.click(ramp + vec2(4.0, 0.0), PointerButton::Primary);
    assert!(r.tempos()[0].2, "ramp on: {:?}", r.tempos());
    r.frames(2, vec![]);
    let f = flag(&r, "140");
    r.click(f, PointerButton::Secondary);
    let del = r.find("Delete");
    r.click(del + vec2(4.0, 0.0), PointerButton::Primary);
    assert_eq!(r.tempos().len(), 1, "deleted");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.tempos().len(), 2);
}
