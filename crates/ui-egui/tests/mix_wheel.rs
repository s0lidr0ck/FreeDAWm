//! Knobs in the Mix window ignore the mouse wheel (spec 2026-11-02 item 3): scrolling through
//! the strips never turns a knob; a click-and-drag still does. The wide editors' knobs in a
//! drawer: `editor_tape.rs`, `editor_pitch.rs`.

use egui::{Modifiers, PointerButton, Pos2, Shape, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// Where the painted text `label` is (its first one).
fn text_at(h: &Harness<SoundApp>, label: &str) -> Option<egui::Rect> {
    h.output().shapes.iter().find_map(|s| match &s.shape {
        Shape::Text(t) if t.galley.job.text == label => Some(t.visual_bounding_rect()),
        _ => None,
    })
}

fn threshold(h: &Harness<SoundApp>, module: soundcraft_model::ModuleId) -> f32 {
    let s = h.state().engine.session();
    let k = s.track_by_name("Kick").unwrap();
    let ins = k.mixer.module(module).unwrap();
    let info = soundcraft_dsp::plugin_info(&ins.plugin).unwrap();
    let p = info.params.iter().find(|p| p.id == "threshold").unwrap();
    ins.params.get("threshold").copied().unwrap_or(p.default)
}

fn wheel(h: &mut Harness<SoundApp>, at: Pos2, notches: f32) {
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.run_steps(1);
    for unit in [egui::MouseWheelUnit::Line, egui::MouseWheelUnit::Point] {
        let delta = if unit == egui::MouseWheelUnit::Line { vec2(0.0, notches) } else { vec2(0.0, notches * 40.0) };
        h.input_mut().events.push(egui::Event::MouseWheel { unit, delta, phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE });
        h.run_steps(3);
    }
}

#[test]
fn the_wheel_over_a_strip_knob_leaves_it_alone_and_a_drag_still_turns_it() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let r = app.engine.execute("mix.module_add", &json!({"track": "Kick", "plugin": "compressor", "at": 0})).unwrap();
    let module = soundcraft_model::ModuleId(u32::try_from(r["module"].as_u64().unwrap()).unwrap());
    app.engine.execute("mix.strip_size", &json!({"track": "Kick", "size": "wide"})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    let mut h = harness(app);
    // The Comp tile's Thresh knob sits just above its name.
    let label = text_at(&h, "Thresh").expect("the Thresh knob's name");
    let knob = label.center_top() - vec2(0.0, 12.0);
    let before = threshold(&h, module);
    let undo = h.state().engine.undo_history().len();
    wheel(&mut h, knob, 3.0);
    wheel(&mut h, knob, -5.0);
    assert!((threshold(&h, module) - before).abs() < 1e-6, "the wheel turned the knob");
    assert_eq!(h.state().engine.undo_history().len(), undo);
    assert!(!soundcraft_ui_egui::widgets::wheel_scrolls(&h.ctx), "only while the strips draw");
    // Click-and-drag still turns it.
    h.input_mut().events.push(egui::Event::PointerMoved(knob));
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton { pos: knob, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=6 {
        h.input_mut().events.push(egui::Event::PointerMoved(knob - vec2(0.0, 5.0 * k as f32)));
        h.run_steps(1);
    }
    let end = knob - vec2(0.0, 30.0);
    h.input_mut().events.push(egui::Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert!((threshold(&h, module) - before).abs() > 1e-3, "a drag turns it");
}
