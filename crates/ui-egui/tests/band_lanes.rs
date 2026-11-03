//! The Band's Sections and Chords rulers in the Edit window: gestures run `band.*` commands
//! and land on the half-bar grid.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ChordEvent, SectionBlock};
use soundcraft_ui_egui::edit_window::{rect, x_of};
use soundcraft_ui_egui::{Services, SoundApp};
use std::time::{Duration, Instant};

/// A half bar of 4/4, in ticks (the band's grid).
const HALF: i64 = 1920;

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
    mods: Modifiers,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({})).unwrap();
        // 400 samples a point: a half bar (one second at 120 bpm) is 120 points wide.
        engine.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
        for ruler in ["sections", "band_chords"] {
            engine.execute("view.ruler", &json!({"ruler": ruler, "visible": true})).unwrap();
        }
        Rig::with(engine)
    }

    fn with(engine: soundcraft_engine::Engine) -> Rig {
        let mut r = Rig {
            ctx: egui::Context::default(),
            app: SoundApp::new(engine, None, Services::default()),
            shapes: Vec::new(),
            mods: Modifiers::NONE,
            time: 1.0,
        };
        r.frames(4);
        r
    }

    fn frame(&mut self, mut events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        events.insert(0, Event::ModifiersChanged(self.mods));
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

    fn texts(&self) -> Vec<String> {
        self.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// The centre line of the ruler labelled `label`.
    fn ruler_y(&self, label: &str) -> f32 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2()).center().y),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label} ruler"))
    }

    /// Where `tick` sits on the timeline.
    fn x(&self, tick: i64) -> f32 {
        let s = self.app.engine.session();
        x_of(s, rect(self.app.edit_layout.timeline), s.tempo.tick_to_samples(tick, s.sample_rate))
    }

    fn at(&self, label: &str, tick: i64) -> Pos2 {
        pos2(self.x(tick), self.ruler_y(label))
    }

    fn button(&self, at: Pos2, button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button, pressed, modifiers: self.mods }
    }

    fn drag(&mut self, from: Pos2, to: Pos2, mods: Modifiers) {
        self.mods = mods;
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![self.button(from, PointerButton::Primary, true)]);
        for k in 1..=6 {
            let p = from + (to - from) * (k as f32 / 6.0);
            self.frame(vec![Event::PointerMoved(p)]);
        }
        self.frame(vec![self.button(to, PointerButton::Primary, false)]);
        self.mods = Modifiers::NONE;
        self.frames(2);
    }

    fn double_click(&mut self, at: Pos2) {
        // Half a second on, so it isn't counted with earlier clicks.
        self.frames(30);
        self.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false, true, false] {
            self.frame(vec![self.button(at, PointerButton::Primary, pressed)]);
        }
        self.frames(2);
    }

    fn type_and_enter(&mut self, text: &str) {
        self.frame(vec![Event::Text(text.into())]);
        self.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
    }

    fn blocks(&self) -> Vec<SectionBlock> {
        self.app.engine.session().band.sections.clone()
    }

    fn chords(&self) -> Vec<ChordEvent> {
        self.app.engine.session().band.chords.clone()
    }
}

fn spans(blocks: &[SectionBlock]) -> Vec<(i64, i64, char)> {
    blocks.iter().map(|b| (b.start_tick, b.length_ticks, b.section)).collect()
}

#[test]
fn drag_creates_moves_resizes_and_copies_blocks() {
    let mut r = Rig::new();
    // Drawn on the empty lane: from about one half to about four, with the default letter A.
    let (from, to) = (r.at("Sections", HALF * 9 / 10), r.at("Sections", HALF * 42 / 10));
    r.drag(from, to, Modifiers::NONE);
    assert_eq!(spans(&r.blocks()), vec![(HALF, 3 * HALF, 'A')]);
    assert!(r.texts().iter().any(|t| t == "A · Verse"), "the block shows its letter and name");
    let id = r.blocks()[0].id;

    // The body moves by whole halves: grabbed in the middle and pulled about two halves on.
    let (from, to) = (r.at("Sections", HALF * 25 / 10), r.at("Sections", HALF * 46 / 10));
    r.drag(from, to, Modifiers::NONE);
    assert_eq!(spans(&r.blocks()), vec![(3 * HALF, 3 * HALF, 'A')]);
    assert_eq!(r.blocks()[0].id, id, "a move keeps the block");

    // The right edge resizes, snapped: out to about seven and a half halves.
    let (from, to) = (r.at("Sections", 6 * HALF) - vec2(2.0, 0.0), r.at("Sections", HALF * 77 / 10));
    r.drag(from, to, Modifiers::NONE);
    assert_eq!(spans(&r.blocks()), vec![(3 * HALF, 5 * HALF, 'A')]);

    // The left edge too.
    let (from, to) = (r.at("Sections", 3 * HALF) + vec2(2.0, 0.0), r.at("Sections", HALF * 39 / 10));
    r.drag(from, to, Modifiers::NONE);
    assert_eq!(spans(&r.blocks()), vec![(4 * HALF, 4 * HALF, 'A')]);

    // Alt-drag copies: the original stays and the copy shares its definition.
    let (from, to) = (r.at("Sections", 5 * HALF), r.at("Sections", HALF * 111 / 10));
    r.drag(from, to, Modifiers::ALT);
    let blocks = r.blocks();
    assert_eq!(spans(&blocks), vec![(4 * HALF, 4 * HALF, 'A'), (10 * HALF, 4 * HALF, 'A')]);
    assert_eq!(blocks[0].id, id);
    assert!(blocks.iter().all(|b| b.start_tick % HALF == 0 && b.length_ticks % HALF == 0));
}

#[test]
fn typed_chord_lands_on_the_lane() {
    let mut r = Rig::new();
    r.double_click(r.at("Chords", HALF * 21 / 10));
    r.type_and_enter("Am7");
    let chords = r.chords();
    assert_eq!(chords.len(), 1, "{chords:?}");
    assert_eq!((chords[0].tick, chords[0].length_ticks), (2 * HALF, 2 * HALF), "on the grid, one bar long");
    let st = &r.app.engine.session().band;
    let name = soundcraft_band::chord_theory::name_for(i32::from(chords[0].slot), i32::from(st.key), i32::from(st.scale), i32::from(chords[0].bass));
    assert_eq!(name, "Am7");
    assert!(r.texts().iter().any(|t| t == "Am7"), "the lane shows the chord's name");
    assert!(!r.texts().iter().any(|t| t.contains("nearest")));

    // A chord the key doesn't hold lands on the nearest one, and says so.
    r.double_click(r.at("Chords", HALF * 61 / 10));
    r.type_and_enter("F#");
    assert_eq!(r.chords().len(), 2, "{} {:?}", r.app.ui.status, r.chords());
    assert_eq!(r.chords()[1].tick, 6 * HALF);
    assert!(r.texts().iter().any(|t| t.contains("≈ nearest in key")), "{:?}", r.texts());
}

#[test]
fn rulers_toggle_with_view_commands() {
    let mut r = Rig::with(soundcraft_engine::Engine::default());
    let shown = |r: &Rig| {
        let rulers = &r.app.engine.session().edit.rulers;
        (rulers.iter().any(|x| x == "sections"), rulers.iter().any(|x| x == "band_chords"))
    };
    assert_eq!(shown(&r), (false, false), "a new session hides them");
    r.app.run("view.ruler", json!({"ruler": "sections"})).unwrap();
    r.frames(2);
    assert_eq!(shown(&r), (true, false));
    assert!(r.texts().iter().any(|t| t == "Sections"));
    r.app.run("view.ruler", json!({"ruler": "sections"})).unwrap();
    r.app.run("view.ruler_all", json!({})).unwrap();
    assert_eq!(shown(&r), (true, true));
    r.app.run("view.ruler_minimal", json!({})).unwrap();
    assert_eq!(shown(&r), (false, false));

    // View › Rulers lists both, right after Key Signature (the old Chord Symbols ruler is gone),
    // and they toggle through view.ruler; the chord lane's ruler reads "Chords".
    let tree = soundcraft_ui_egui::menus::tree();
    let view = tree.iter().find(|n| n.label == "View").unwrap();
    let rulers = view.children.iter().find(|n| n.label == "Rulers").unwrap();
    let labels: Vec<&str> = rulers.children.iter().map(|n| n.label.as_str()).collect();
    assert!(!labels.contains(&"Chord Symbols") && !labels.contains(&"Band Chords"), "{labels:?}");
    let at = labels.iter().position(|l| *l == "Key Signature").unwrap();
    assert_eq!(labels[at + 1..at + 3], ["Sections", "Chords"]);
    assert!(r.app.run("view.ruler", json!({"ruler": "chords"})).is_err(), "the chords ruler id is gone");
    let extra = soundcraft_ui_egui::menus::ui_aliases();
    for (path, ruler) in [("View > Rulers > Sections", "sections"), ("View > Rulers > Chords", "band_chords")] {
        assert_eq!(soundcraft_ui_egui::menus::command_for_path(path, &extra).as_deref(), Some("view.ruler"));
        assert!(!soundcraft_ui_egui::menus::item_state(&r.app, path, Some("view.ruler")).1);
        r.app.run("view.ruler", json!({"ruler": ruler})).unwrap();
        assert!(soundcraft_ui_egui::menus::item_state(&r.app, path, Some("view.ruler")).1, "{path} is checked");
    }

    // Lane content turns hidden rulers on.
    r.app.run("view.ruler_minimal", json!({})).unwrap();
    r.app.run("band.section.add", json!({"section": "A", "start": 0, "end": 96_000})).unwrap();
    assert_eq!(shown(&r), (true, true));
    r.app.run("view.ruler_minimal", json!({})).unwrap();
    r.app.run("band.chord.add", json!({"at": 0, "chord": "C"})).unwrap();
    assert_eq!(shown(&r), (true, true));
}

#[test]
fn lanes_draw_with_ten_thousand_items_in_budget() {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({})).unwrap();
    {
        let band = &mut engine.session_mut().band;
        band.chords = (0..soundcraft_model::MAX_CHORDS as u32)
            .map(|i| ChordEvent { id: i + 1, tick: i64::from(i) * HALF, length_ticks: HALF, slot: (i % 112) as u8, bass: -1 })
            .collect();
        band.sections = (0..soundcraft_model::MAX_BLOCKS as u32)
            .map(|i| SectionBlock { id: 10_000 + i, start_tick: i64::from(i) * 8 * HALF, length_ticks: 8 * HALF, section: 'A', energy: 0, take: 0 })
            .collect();
        band.next_id = 20_000;
    }
    for ruler in ["sections", "band_chords"] {
        engine.execute("view.ruler", &json!({"ruler": ruler, "visible": true})).unwrap();
    }
    let mut r = Rig::with(engine);
    let frame_ms = |r: &mut Rig| {
        r.frames(3);
        (0..5)
            .map(|_| {
                let t0 = Instant::now();
                r.frame(vec![]);
                t0.elapsed()
            })
            .min()
            .unwrap_or_default()
    };
    // Every zoom from one chord per half bar on screen to all 4096 at once.
    for spp in [400.0, 10_000.0, 150_000.0] {
        r.app.run("view.zoom_set", json!({"samples_per_px": spp})).unwrap();
        let t = frame_ms(&mut r);
        // The Edit window's repaint budget: 30 frames a second.
        assert!(t < Duration::from_millis(33), "{spp} samples/pt: {t:?} a frame");
    }
    r.app.run("view.zoom_set", json!({"samples_per_px": 400.0})).unwrap();
    r.frames(2);
    assert!(r.texts().iter().any(|t| t == "C") && r.texts().iter().any(|t| t == "A · Verse"), "the lanes are drawn");
}
