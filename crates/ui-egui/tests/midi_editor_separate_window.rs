//! Window › MIDI Editor in Separate Window (and the editor's Window button): the editor leaves
//! the Edit window's dock for its own window (embedded as a floating window here, where the test
//! context has no second OS window), remembers the window in ui.json, and docks back when that
//! window closes.

use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};
use serde_json::json;
use soundcraft_ui_egui::docking::Panel;
use soundcraft_ui_egui::midi_editor_window::separate_closed;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), time: 1.0 };
        r.app.run("window.midi_editor", json!({"value": true})).unwrap();
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
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn click(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        self.frame(vec![press(true)]);
        self.frame(vec![press(false)]);
        self.frames(2);
    }

    fn docked(&self) -> bool {
        self.app.ui.docking.layout.contains(&Panel::Midi)
    }

    fn button(&self) -> Rect {
        self.ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("soundcraft-midi-separate"))).expect("the Window button is drawn")
    }
}

#[test]
fn the_menu_command_moves_the_editor_to_its_own_window_and_closing_docks_it_back() {
    let mut r = Rig::new();
    assert!(r.docked());
    let docked_button = r.button();
    r.app.run("window.midi_editor_separate", json!({})).unwrap();
    r.frames(4);
    assert!(r.app.ui.midi_editor_separate && r.app.ui.show_midi_editor);
    assert!(!r.docked(), "the dock's MIDI Editor tab is gone");
    assert_ne!(r.button(), docked_button, "the editor is drawn in its window now");
    separate_closed(&mut r.app);
    r.frames(4);
    assert!(!r.app.ui.midi_editor_separate && r.app.ui.show_midi_editor);
    assert!(r.docked(), "back in the dock");
}

#[test]
fn the_title_row_button_toggles_it() {
    let mut r = Rig::new();
    let b = r.button();
    r.click(b.center());
    assert!(r.app.ui.midi_editor_separate);
    assert!(!r.docked());
    let b = r.button();
    r.click(b.center());
    r.frames(2);
    assert!(!r.app.ui.midi_editor_separate);
    assert!(r.docked());
}

#[test]
fn the_window_is_remembered_in_ui_json() {
    let mut r = Rig::new();
    r.app.run("window.midi_editor_separate", json!({})).unwrap();
    r.app.ui.midi_editor_window = Some([100.0, 80.0, 900.0, 600.0]);
    let text = serde_json::to_string(&r.app.ui).unwrap();
    assert!(text.contains("\"midi_editor.separate\":true"), "{text}");
    assert!(text.contains("\"midi_editor.window\":[100.0,80.0,900.0,600.0]"), "{text}");
    let back: soundcraft_ui_egui::UiState = serde_json::from_str(&text).unwrap();
    assert!(back.midi_editor_separate);
    assert_eq!(back.midi_editor_window, Some([100.0, 80.0, 900.0, 600.0]));
    let old: soundcraft_ui_egui::UiState =
        serde_json::from_str(&text.replace("\"midi_editor.separate\":true,", "").replace("\"midi_editor.window\":[100.0,80.0,900.0,600.0],", ""))
            .unwrap();
    assert!(!old.midi_editor_separate && old.midi_editor_window.is_none());
}
