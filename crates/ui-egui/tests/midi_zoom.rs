//! Zoom and scroll in the MIDI Editor (docs/superpowers/specs/2026-10-26-key-ruler-midi-zoom.md,
//! part 2): Ctrl+wheel zooms time around the pointer, Ctrl+Alt+wheel the note height, Shift+wheel
//! scrolls time; Fit (button or F) fits the clip again; drawing at high zoom lands on the right
//! tick; each clip keeps its own view.

use egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect, Shape, TouchPhase, epaint::ClippedShape, vec2};
use serde_json::json;
use soundcraft_ui_egui::midi_editor::{MAX_PX_PER_TICK, RollGeometry, row_zoom, zoom_about};
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
    time: f64,
}

impl Rig {
    /// The demo's Keys clip in the MIDI Editor.
    fn new() -> Rig {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.ui.show_midi_editor = true;
        app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app, shapes: Vec::new(), time: 1.0 };
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

    fn geo(&self) -> RollGeometry {
        self.app.midi.drawn.expect("the roll was drawn")
    }

    fn wheel(&mut self, at: Pos2, dy: f32, modifiers: Modifiers) {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![
            Event::ModifiersChanged(modifiers),
            Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(0.0, dy), phase: TouchPhase::Move, modifiers },
        ]);
        self.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.frames(1);
    }

    fn click(&mut self, at: Pos2) {
        self.frames(30);
        self.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
        self.frames(2);
    }

    fn click_text(&mut self, label: &str) {
        let r = self
            .shapes
            .iter()
            .rev()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no `{label}`"));
        self.click(r.center());
    }
}

#[test]
fn zoom_about_keeps_the_tick_under_the_pointer() {
    let v = soundcraft_ui_egui::midi_editor::MidiView::default();
    let (w, len) = (1000.0, 10_000);
    let z = zoom_about(v, 4.0, 250.0, w, len);
    assert_eq!(z.zoom, Some(0.4));
    // 250 points in was tick 2500; it still is.
    assert!((z.left + 250.0 / 0.4 - 2500.0).abs() < 0.01, "{z:?}");
    // Never wider than fit, never closer than the maximum.
    assert_eq!(zoom_about(z, 0.01, 0.0, w, len).zoom, None);
    assert_eq!(zoom_about(z, 1e6, 0.0, w, len).zoom, Some(MAX_PX_PER_TICK));
    // Note height stays in 5-30.
    assert_eq!(row_zoom(v, 100.0, 0.0).row_h, 30.0);
    assert_eq!(row_zoom(v, 0.01, 0.0).row_h, 5.0);
}

#[test]
fn ctrl_wheel_zooms_around_the_pointer_and_fit_restores() {
    let mut r = Rig::new();
    let g = r.geo();
    assert_eq!(g.px_per_tick, g.fit, "a clip opens fitted");
    let p = g.roll.min + vec2(g.roll.width() * 0.3, g.roll.height() / 2.0);
    let under = g.tick_at(p.x);
    r.wheel(p, 3.0, Modifiers::CTRL);
    let z = r.geo();
    assert!(z.px_per_tick > g.px_per_tick * 1.05, "{} -> {}", g.px_per_tick, z.px_per_tick);
    assert!((z.tick_at(p.x) - under).abs() < 1.0, "the tick under the pointer stays: {under} -> {}", z.tick_at(p.x));
    // Shift+wheel scrolls time; the plain wheel scrolls the notes, not time.
    let left = z.left;
    r.wheel(p, -2.0, Modifiers::SHIFT);
    assert!(r.geo().left > left, "{left} -> {}", r.geo().left);
    let (left, scroll) = (r.geo().left, r.geo().scroll);
    r.wheel(p, -2.0, Modifiers::NONE);
    assert_eq!(r.geo().left, left);
    assert_ne!(r.geo().scroll, scroll);
    // Fit.
    r.click_text("Fit");
    assert_eq!(r.geo().px_per_tick, r.geo().fit);
    assert_eq!(r.geo().left, 0.0);
    // F over the roll fits too (and is not the Edit window's F).
    r.wheel(p, 3.0, Modifiers::CTRL);
    assert!(r.geo().px_per_tick > r.geo().fit);
    let before = r.app.engine.session().clone();
    r.frame(vec![Event::PointerMoved(p)]);
    r.frame(vec![Event::Key { key: Key::F, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    r.frames(2);
    assert_eq!(r.geo().px_per_tick, r.geo().fit);
    assert!(*r.app.engine.session() == before, "F did nothing else");
}

#[test]
fn ctrl_alt_wheel_and_the_buttons_change_the_note_height() {
    let mut r = Rig::new();
    let g = r.geo();
    let p = g.roll.center();
    r.wheel(p, 3.0, Modifiers::CTRL | Modifiers::ALT);
    assert!(r.geo().row_h > g.row_h, "{} -> {}", g.row_h, r.geo().row_h);
    assert_eq!(r.geo().px_per_tick, g.px_per_tick, "time zoom unchanged");
    // The Height "+": the first "+" right of the Height label (Fit Zoom − + Height − +), up to 30 points.
    let height_plus = |r: &Rig| {
        let texts = |label: &str| -> Vec<Rect> {
            r.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2())),
                    _ => None,
                })
                .collect()
        };
        let label = texts("Height")[0];
        texts("+")
            .into_iter()
            .filter(|b| b.min.x > label.max.x && (b.center().y - label.center().y).abs() < 6.0)
            .min_by(|a, b| a.min.x.total_cmp(&b.min.x))
            .unwrap()
            .center()
    };
    let (tracks, h, zoom) = (r.app.engine.session().tracks.len(), r.geo().row_h, r.geo().px_per_tick);
    r.click(height_plus(&r));
    assert!(r.geo().row_h > h, "{h} -> {}", r.geo().row_h);
    assert_eq!(r.geo().px_per_tick, zoom);
    for _ in 0..10 {
        r.click(height_plus(&r));
    }
    assert_eq!(r.geo().row_h, 30.0);
    assert_eq!(r.app.engine.session().tracks.len(), tracks, "no other + pressed");
}

#[test]
fn a_note_drawn_at_high_zoom_lands_on_the_right_tick() {
    let mut r = Rig::new();
    r.app.run("edit.tool", json!({"tool": "pencil"})).unwrap();
    r.frames(2);
    let g = r.geo();
    let p = g.roll.min + vec2(g.roll.width() * 0.6, 0.0);
    for _ in 0..6 {
        r.wheel(p, 5.0, Modifiers::CTRL);
    }
    let z = r.geo();
    assert!(z.px_per_tick > g.px_per_tick * 4.0, "zoomed in: {} -> {}", g.px_per_tick, z.px_per_tick);
    // An empty row near the top of the view, a little after a sixteenth's line.
    let grid = 240;
    let tick = (z.tick_at(z.roll.center().x) as i64 / grid) * grid + grid / 3;
    let pitch = z.pitch_at(z.roll.min.y + 2.0);
    let at = Pos2::new(z.x_of(tick), z.y_of(pitch) + z.row_h / 2.0);
    let clip = z.clip;
    let count = |r: &Rig| match &r.app.engine.session().find_clip(clip).unwrap().1.content {
        soundcraft_model::ClipContent::Midi { sequence } => sequence.notes.clone(),
        _ => panic!("midi"),
    };
    let before = count(&r).len();
    r.click(at);
    let notes = count(&r);
    assert_eq!(notes.len(), before + 1, "{}", r.app.ui.status);
    let n =
        notes.iter().find(|n| i32::from(n.pitch) == pitch && n.start == tick - grid / 3).unwrap_or_else(|| panic!("no note at {tick}: {notes:?}"));
    assert_eq!(n.length, grid);
}

#[test]
fn each_clip_keeps_its_view() {
    let mut r = Rig::new();
    let keys = r.geo().clip;
    let p = r.geo().roll.center();
    r.wheel(p, 4.0, Modifiers::CTRL);
    let zoomed = r.geo().px_per_tick;
    // The Lead clip opens fitted; back on Keys, its zoom is still there.
    let lead = r.app.engine.session().track_by_name("Lead").unwrap().clips()[0].id;
    r.app.run("edit.select", json!({"clips": [lead.0]})).unwrap();
    r.frames(3);
    assert_eq!(r.geo().clip, lead);
    assert_eq!(r.geo().px_per_tick, r.geo().fit);
    r.app.run("edit.select", json!({"clips": [keys.0]})).unwrap();
    r.frames(3);
    assert_eq!(r.geo().clip, keys);
    assert_eq!(r.geo().px_per_tick, zoomed);
}
