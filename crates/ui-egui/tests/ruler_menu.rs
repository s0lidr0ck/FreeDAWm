//! Right-click in the Edit window's ruler area: a menu of every View > Rulers lane (checked when shown),
//! "Hide This Ruler", and the lane list as a submenu of a ruler's own menu.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
    time: f64,
}

impl Rig {
    fn new(rulers: &[&str]) -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({})).unwrap();
        engine.execute("view.ruler_minimal", &json!({})).unwrap();
        for r in rulers {
            engine.execute("view.ruler", &json!({"ruler": r, "visible": true})).unwrap();
        }
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

    /// The centre of the lowest painted text equal to `text`: the ruler's name (the toolbar has its own
    /// Tempo and Meter), or, for a menu, its item (popups paint last).
    fn find(&self, text: &str) -> Pos2 {
        self.shapes
            .iter()
            .rev()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == text => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {text:?} in {:?}", self.texts()))
    }

    fn click(&mut self, at: Pos2, button: PointerButton) {
        self.frames(1, vec![Event::PointerMoved(at)]);
        let m = Modifiers::NONE;
        self.frames(1, vec![Event::PointerButton { pos: at, button, pressed: true, modifiers: m }]);
        self.frames(1, vec![Event::PointerButton { pos: at, button, pressed: false, modifiers: m }]);
        self.frames(4, vec![]);
    }

    fn shown(&self, ruler: &str) -> bool {
        self.app.engine.session().edit.rulers.iter().any(|r| r == ruler)
    }
}

// Key Signature Staff left View › Rulers in the menus clean-up (a flag nothing reads); Ruler Lines
// moved in from Marker Displays.
const ALL: [&str; 21] = [
    "Bars|Beats",
    "Min:Secs",
    "Timecode",
    "Timecode 2",
    "Feet+Frames",
    "Samples",
    "Tempo",
    "Tempo Editor",
    "Meter",
    "Key Signature",
    "Sections",
    "Chords",
    "Markers",
    "Markers 2",
    "Markers 3",
    "Markers 4",
    "Markers 5",
    "All Marker Rulers",
    "Ruler Lines",
    "All",
    "Minimal",
];

#[test]
fn right_click_a_ruler_label_lists_every_ruler_and_toggles() {
    let mut r = Rig::new(&["tempo"]);
    let label = r.find("Tempo");
    r.click(label, PointerButton::Secondary);
    let texts = r.texts();
    for name in ALL {
        let shown = texts.iter().any(|t| t == name || t == &format!("✔ {name}"));
        assert!(shown, "{name} missing from {texts:?}");
    }
    assert!(texts.iter().any(|t| t == "Hide This Ruler"));
    // Order matches View > Rulers.
    let top = texts.iter().position(|t| t == "Hide This Ruler").unwrap();
    let pos = |n: &str| top + texts[top..].iter().position(|t| t == n || t == &format!("✔ {n}")).unwrap();
    assert!(ALL.windows(2).all(|w| pos(w[0]) < pos(w[1])), "{texts:?}");
    assert!(texts.iter().any(|t| t == "✔ Tempo"), "{texts:?}");
    assert!(!texts.iter().any(|t| t == "✔ Meter"));

    assert!(!r.shown("meter"));
    let meter = r.find("Meter");
    r.click(meter, PointerButton::Primary);
    assert!(r.shown("meter"));
    assert!(r.shown("tempo"));
}

#[test]
fn hide_this_ruler_hides_tempo() {
    let mut r = Rig::new(&["tempo"]);
    let label = r.find("Tempo");
    r.click(label, PointerButton::Secondary);
    let hide = r.find("Hide This Ruler");
    r.click(hide, PointerButton::Primary);
    assert!(!r.shown("tempo"));
    assert!(r.shown("markers"));
}

#[test]
fn the_ruler_content_and_the_band_lanes_keep_their_menus() {
    let mut r = Rig::new(&["sections", "markers"]);
    // The marker ruler's row: the lane list too.
    let marker_label = r.find("Markers");
    r.click(pos2(600.0, marker_label.y), PointerButton::Secondary);
    assert!(r.texts().iter().any(|t| t == "Hide This Ruler"), "{:?}", r.texts());
    assert!(r.texts().iter().any(|t| t == "Chords"));
    r.frames(2, vec![Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    r.frames(2, vec![]);

    // The Sections lane keeps "Draw sections as" and gets Hide + a Rulers submenu.
    let label = r.find("Sections");
    r.click(pos2(600.0, label.y), PointerButton::Secondary);
    let texts = r.texts();
    assert!(texts.iter().any(|t| t == "Draw sections as"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "Hide This Ruler"));
    assert!(texts.iter().any(|t| t == "Rulers"));
    assert!(!texts.iter().any(|t| t == "Feet+Frames"), "the list is behind the submenu");
    let rulers = r.find("Rulers");
    r.click(rulers, PointerButton::Primary);
    assert!(r.texts().iter().any(|t| t == "Feet+Frames"), "{:?}", r.texts());
}
