//! Pause holds the playhead where it stopped and resumes from there; a position typed into the
//! main counter locates there.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, toolbar};

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
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new(), time: 1.0 };
        r.frames(3);
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

    /// Where the main counter's number is drawn (the toolbar's 26-point text).
    fn counter(&self) -> Pos2 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.job.sections.first().is_some_and(|s| s.format.font_id.size == 26.0) => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("the main counter")
    }

    fn click(&mut self, at: Pos2) {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
    }
}

#[test]
fn pause_holds_the_position_and_resumes_from_it() {
    let mut r = Rig::new();
    r.app.run("options.insertion_follows_playback", json!({"value": true})).unwrap();
    r.app.run("edit.select", json!({"start": 48_000, "end": 48_000})).unwrap();
    r.app.run("transport.play", json!({})).unwrap();
    r.frames(30);
    assert!(r.app.engine.transport.playing);
    r.app.run("transport.pause", json!({})).unwrap();
    r.frame(vec![]);
    let held = r.app.position();
    assert!(!r.app.engine.transport.playing);
    assert!(held > 48_000, "paused at {held}");
    assert_eq!(r.app.engine.session().edit.selection.start, 48_000, "Pause leaves the selection alone");
    r.frames(10);
    assert_eq!(r.app.position(), held, "the playhead holds while paused");
    r.app.run("transport.pause", json!({})).unwrap();
    r.frame(vec![]);
    assert!(r.app.engine.transport.playing);
    assert!(r.app.position() >= held, "resumes from where it paused, not from the selection");
    // Stop forgets the pause: the next Pause plays from the selection.
    r.app.run("transport.stop", json!({})).unwrap();
    r.frame(vec![]);
    r.app.run("edit.select", json!({"start": 1_000, "end": 1_000})).unwrap();
    r.app.run("transport.pause", json!({})).unwrap();
    r.frame(vec![]);
    assert!(r.app.engine.transport.playing);
    assert!(r.app.position() < held);
}

#[test]
fn a_position_typed_into_the_main_counter_locates_there() {
    let mut r = Rig::new();
    r.app.run("view.main_counter", json!({"format": "samples"})).unwrap();
    toolbar::locate_typed(&mut r.app, "96000").unwrap();
    assert_eq!(r.app.position(), 96_000);
    assert!(toolbar::locate_typed(&mut r.app, "not a time").is_err());
    assert_eq!(r.app.position(), 96_000);

    // By the counter itself: click its number, type, Enter.
    r.frames(2);
    let at = r.counter();
    r.click(at);
    r.frame(vec![]);
    r.frame(vec![Event::Text("12345".into())]);
    r.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    r.frames(2);
    assert_eq!(r.app.position(), 12_345);
}

#[test]
fn held_fast_forward_winds_faster_and_a_click_jumps_a_second() {
    use egui::accesskit::Role;
    use egui_kittest::{Harness, kittest::Queryable};
    assert!(toolbar::wind_speed(0.1) <= toolbar::wind_speed(1.0) && toolbar::wind_speed(1.0) < toolbar::wind_speed(2.0));
    assert_eq!(toolbar::wind_speed(60.0), 32.0);
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({})).unwrap();
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    let sr = h.state().engine.session().sample_rate.as_f64() as i64;
    let at = h.get_by_role_and_label(Role::Button, "Fast Forward").rect().center();
    let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(Event::PointerMoved(at));
    h.run_steps(1);
    h.event(button(true));
    h.run_steps(1);
    h.event(button(false));
    h.run_steps(2);
    assert_eq!(h.state().position(), sr, "a click jumps one second");
    h.event(button(true));
    h.run_steps(45);
    let first = h.state().position() - sr;
    h.run_steps(45);
    let wound = h.state().position() - sr;
    assert!(first > 0 && wound - first > first, "it winds, faster the longer it is held: {first} then {wound}");
    h.event(button(false));
    h.run_steps(2);
    assert_eq!(h.state().position() - sr, wound, "letting go of a long hold does not jump again");
    assert!(wound > sr, "more than a click's second: {wound}");
}

#[test]
fn varispeed_is_set_from_its_dialog_and_turned_off_at_one() {
    let mut r = Rig::new();
    soundcraft_ui_egui::menus::invoke_menu(&mut r.app, "transport.varispeed", "Options > More > Varispeed...");
    assert!(matches!(r.app.dialogs.open, Some(soundcraft_ui_egui::dialogs::Dialog::Number { .. })), "the menu item opens its dialog");
    r.app.dialogs.open = None;
    r.app.run("transport.varispeed", json!({"speed": 0.8})).unwrap();
    r.frame(vec![]);
    assert_eq!(r.app.varispeed, Some(0.8));
    assert!(r.app.run("transport.varispeed", json!({"speed": 9.0})).is_err());
    assert!(r.app.run("transport.varispeed", json!({"speed": "fast"})).is_err());
    r.app.run("transport.varispeed", json!({"speed": 1.0})).unwrap();
    r.frame(vec![]);
    assert_eq!(r.app.varispeed, None, "1× is off");
}
