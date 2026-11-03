//! The sticky loop (docs/superpowers/specs/2026-10-28-sticky-loop.md): Play loops the loop when
//! the insertion is inside it, the dimmed strip on the main ruler reselects the range, its ends
//! drag the loop (not the selection), Esc cancels, and the Bounce dialog defaults to the loop.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
use soundcraft_time::Range;
use soundcraft_ui_egui::dialogs::{BounceRange, Dialog};
use soundcraft_ui_egui::edit_window::{rect, x_of};
use soundcraft_ui_egui::{Services, SoundApp};

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

    fn x(&self, at: i64) -> f32 {
        x_of(self.app.engine.session(), rect(self.app.edit_layout.timeline), at)
    }

    /// A point on the main (Min:Secs) ruler at sample `at`.
    fn on_ruler(&self, at: i64) -> Pos2 {
        pos2(self.x(at), self.ruler_y("Min:Secs"))
    }

    fn click(&mut self, at: Pos2) {
        self.frames(30);
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        self.frames(3);
    }

    fn press_and_drag(&mut self, from: Pos2, to: Pos2) {
        self.frames(30);
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        let mid = pos2((from.x + to.x) / 2.0, to.y);
        self.frame(vec![Event::PointerMoved(mid)]);
        self.frame(vec![Event::PointerMoved(to)]);
        self.frames(2);
    }

    fn release(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        self.frames(3);
    }

    fn key(&mut self, key: Key) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
    }

    /// Rects drawn exactly over the loop `start..end` on the main ruler (the strip and the
    /// dimmed fill), with their alpha.
    fn loop_rects(&self, start: i64, end: i64) -> Vec<(Rect, u8)> {
        let (x0, x1, y) = (self.x(start), self.x(end), self.ruler_y("Min:Secs"));
        self.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Rect(r) if (r.rect.min.x - x0).abs() < 1.0 && (r.rect.max.x - x1).abs() < 1.0 && (r.rect.center().y - y).abs() < 12.0 => {
                    Some((r.rect, r.fill.a()))
                }
                _ => None,
            })
            .collect()
    }

    fn select(&mut self, start: i64, end: i64) {
        self.app.engine.execute("edit.select", &json!({"start": start, "end": end})).unwrap();
        self.frames(2);
    }

    fn selection(&self) -> Range {
        self.app.engine.session().edit.selection
    }

    fn lp(&self) -> Option<Range> {
        self.app.engine.session().edit.loop_range
    }
}

#[test]
fn play_loops_the_loop_only_with_loop_playback_on_and_the_insertion_inside_it() {
    let mut r = Rig::new();
    r.select(48_000, 96_000);
    r.app.engine.execute("edit.select_none", &json!({})).unwrap();
    assert_eq!(r.lp(), Some(Range::new(48_000, 96_000)));
    // Loop Playback off: never loops.
    r.app.engine.execute("transport.locate", &json!({"at": 60_000})).unwrap();
    r.app.play_from_selection();
    assert_eq!(r.app.sim_play(), Some((60_000, None, None)));
    r.app.engine.execute("transport.stop", &json!({})).unwrap();
    r.frames(2);
    // On, insertion inside: the loop plays again from its start.
    r.app.engine.execute("options.loop_playback", &json!({})).unwrap();
    r.app.engine.execute("transport.locate", &json!({"at": 60_000})).unwrap();
    r.app.play_from_selection();
    assert_eq!(r.app.sim_play(), Some((48_000, None, Some(Range::new(48_000, 96_000)))));
    r.app.engine.execute("transport.stop", &json!({})).unwrap();
    r.frames(2);
    // Insertion outside (the end is exclusive): straight through.
    r.app.engine.execute("transport.locate", &json!({"at": 96_000})).unwrap();
    r.app.play_from_selection();
    assert_eq!(r.app.sim_play(), Some((96_000, None, None)));
    r.app.engine.execute("transport.stop", &json!({})).unwrap();
    r.frames(2);
    // A live range selection: unchanged from before (the selection is the loop).
    r.select(10_000, 20_000);
    r.app.play_from_selection();
    assert_eq!(r.app.sim_play(), Some((10_000, None, Some(Range::new(10_000, 20_000)))));
}

#[test]
fn the_strip_is_lit_while_selected_and_dimmed_after_deselecting() {
    let mut r = Rig::new();
    assert!(r.loop_rects(48_000, 96_000).is_empty());
    r.select(48_000, 96_000);
    let lit: Vec<_> = r.loop_rects(48_000, 96_000);
    assert_eq!(lit.len(), 1, "{lit:?}");
    assert_eq!(lit[0].1, 255);
    assert!((lit[0].0.height() - 3.0).abs() < 0.01);
    r.app.engine.execute("edit.select_none", &json!({})).unwrap();
    r.frames(2);
    let dim = r.loop_rects(48_000, 96_000);
    assert_eq!(dim.len(), 2, "a faint fill and the dimmed strip: {dim:?}");
    assert!(dim.iter().all(|(_, a)| *a < 255), "{dim:?}");
}

#[test]
fn clicking_the_dimmed_strip_reselects_the_exact_range() {
    let mut r = Rig::new();
    r.select(48_000, 96_000);
    r.app.engine.execute("transport.locate", &json!({"at": 200_000})).unwrap();
    r.frames(2);
    assert!(r.selection().is_empty());
    let inside = r.on_ruler(70_000);
    r.click(inside);
    assert_eq!(r.selection(), Range::new(48_000, 96_000));
    assert_eq!(r.app.engine.session().edit.selected_tracks.len(), 1, "no selected tracks: all tracks");
    // Off the strip, a click still locates.
    r.click(r.on_ruler(200_000));
    assert!(r.selection().is_empty());
    assert_eq!(r.selection().start, 200_000);
    assert_eq!(r.lp(), Some(Range::new(48_000, 96_000)));
}

#[test]
fn dragging_an_end_moves_the_loop_not_the_selection_and_esc_cancels() {
    let mut r = Rig::new();
    r.select(48_000, 96_000);
    r.app.engine.execute("transport.locate", &json!({"at": 200_000})).unwrap();
    r.frames(2);
    let (from, to) = (r.on_ruler(96_000), r.on_ruler(120_000));
    r.press_and_drag(from, to);
    assert_eq!(r.lp(), Some(Range::new(48_000, 120_000)));
    assert_eq!(r.selection(), Range::point(200_000), "the selection stays");
    r.release(to);
    assert_eq!(r.lp(), Some(Range::new(48_000, 120_000)));
    assert_eq!(r.app.engine.session().edit.playhead, 200_000, "the drag did not locate");
    assert!(r.app.engine.is_dirty(), "an end drag marks the session modified");
    // Esc mid-drag goes back.
    let (from, to) = (r.on_ruler(48_000), r.on_ruler(20_000));
    r.press_and_drag(from, to);
    assert_eq!(r.lp(), Some(Range::new(20_000, 120_000)));
    r.key(Key::Escape);
    assert_eq!(r.lp(), Some(Range::new(48_000, 120_000)));
    r.release(to);
    assert_eq!(r.lp(), Some(Range::new(48_000, 120_000)));
}

#[test]
fn the_bounce_dialog_defaults_to_the_loop_else_the_whole_session_and_markers_give_their_positions() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    assert!(matches!(app.dialogs.open, Some(Dialog::Bounce { range: BounceRange::Whole, .. })), "{:?}", app.dialogs.open);
    app.engine.execute("edit.select", &json!({"start": 48_000, "end": 96_000})).unwrap();
    app.engine.execute("edit.select_none", &json!({})).unwrap();
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    assert!(matches!(app.dialogs.open, Some(Dialog::Bounce { range: BounceRange::Loop, .. })), "{:?}", app.dialogs.open);
    let s = app.engine.session();
    assert_eq!(BounceRange::Loop.range(s), Some(Range::new(48_000, 96_000)));
    let verse = app.engine.execute("markers.add", &json!({"at": 10_000, "name": "Verse"})).unwrap()["id"].as_u64().unwrap();
    let chorus = app.engine.execute("markers.add", &json!({"at": 30_000, "name": "Chorus", "ruler": 2})).unwrap()["id"].as_u64().unwrap();
    let s = app.engine.session();
    let (verse, chorus) = (soundcraft_model::MarkerId(verse), soundcraft_model::MarkerId(chorus));
    assert_eq!(BounceRange::markers(s).iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Verse", "Chorus"], "every marker ruler, in time order");
    assert_eq!(BounceRange::Markers { from: verse, to: Some(chorus) }.range(s), Some(Range::new(10_000, 30_000)));
    assert_eq!(BounceRange::Markers { from: chorus, to: None }.range(s), Some(Range::new(30_000, s.content_end().max(30_000))));
    // The dialog shows the three choices.
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    use egui_kittest::kittest::Queryable;
    for label in ["Loop", "Between markers", "Whole session"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
}
