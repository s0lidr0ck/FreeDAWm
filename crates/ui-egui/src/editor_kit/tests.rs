//! The kit's controls through a tiny test editor: `Dial`'s gestures as JUCE has them (each one
//! undo step), and the JUCE wheel mapping.

use super::controls::DialOpts;
use super::family::{Family, FamilyState, dial_param};
use super::*;
use crate::Services;
use egui::{Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;

/// A test editor: one dial on CleanQ's `outGain` at (50, 20), 100 x 130.
#[derive(Clone, Default)]
struct DialEditor {
    fam: FamilyState,
}

const FAM: Family = Family::new("CleanQ", &[]);

const DIAL_AT: Vec2 = vec2(50.0, 20.0);

impl VstEditor for DialEditor {
    const PLUGIN: &'static str = "test-dial";
    fn size(&self, _ins: &Insert) -> Vec2 {
        vec2(300.0, 200.0)
    }
    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        let r = Rect::from_min_size(host.rect.min + DIAL_AT, vec2(100.0, 130.0));
        let o = DialOpts { label: "Out", value_text: "x", default: 0.5, ..DialOpts::default() };
        dial_param(app, ui, host, &mut self.fam, &FAM, r, &VstPalette::THEME_DEFAULTS, "outGain", &o);
    }
}

fn harness() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Pad"})).unwrap_or_default();
    engine.execute("mix.module_add", &json!({"track": "Pad", "plugin": "fs_cleanq"})).unwrap_or_default();
    let t = engine.session().tracks[0].id;
    let m = engine.session().tracks[0].mixer.inserts[0].id;
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = Harness::builder().with_size(vec2(600.0, 400.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        move |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            if !app.fonts_ready {
                return;
            }
            let s = app.engine.session_arc();
            if let Some(track) = s.track(t) {
                run::<DialEditor>(app, ui, track, m, None);
            }
        },
        app,
    );
    h.run_steps(3);
    (h, t, m)
}

fn value(h: &Harness<SoundApp>, t: TrackId, m: ModuleId) -> f32 {
    let s = h.state().engine.session();
    let ins = s.track(t).and_then(|x| x.mixer.module(m)).cloned().unwrap_or_else(|| Insert::new("fs_cleanq"));
    let info = soundcraft_dsp::plugin_info("fs_cleanq").and_then(|i| i.params.iter().find(|p| p.id == "outGain"));
    info.map_or(f32::NAN, |p| p.to_normalized(p.clamp(ins.params.get("outGain").copied().unwrap_or(p.default))))
}

fn knob_centre(h: &Harness<SoundApp>, t: TrackId, m: ModuleId) -> Pos2 {
    let f = frame_rect(&h.ctx, t, m).unwrap_or(Rect::NOTHING);
    // `getKnobBounds` (Below): a 100 px square at the top of the 100 x 130 dial.
    f.min + DIAL_AT + vec2(50.0, 50.0)
}

fn ev(h: &mut Harness<SoundApp>, pos: Pos2, pressed: Option<bool>, mods: Modifiers) {
    h.input_mut().events.push(egui::Event::ModifiersChanged(mods));
    h.input_mut().events.push(egui::Event::PointerMoved(pos));
    if let Some(pressed) = pressed {
        h.input_mut().events.push(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: mods });
    }
    h.step();
}

fn drag_up(h: &mut Harness<SoundApp>, at: Pos2, dy: f32, mods: Modifiers) {
    ev(h, at, None, mods);
    ev(h, at, Some(true), mods);
    for k in 1..=10 {
        ev(h, at - vec2(0.0, dy * k as f32 / 10.0), None, mods);
    }
    ev(h, at - vec2(0.0, dy), Some(false), mods);
    ev(h, at - vec2(0.0, dy), None, Modifiers::NONE);
    h.run_steps(2);
}

#[test]
fn dial_gestures_match_juce() {
    let (mut h, t, m) = harness();
    let c = knob_centre(&h, t, m);
    let undo_len = |h: &Harness<SoundApp>| h.state().engine.undo_history().len();
    assert!((value(&h, t, m) - 0.5).abs() < 1e-6);

    // Drag up 50 px: + 50 * 0.008 = 0.4, one undo step.
    let n = undo_len(&h);
    drag_up(&mut h, c, 50.0, Modifiers::NONE);
    assert!((value(&h, t, m) - 0.9).abs() < 1e-4, "{}", value(&h, t, m));
    assert_eq!(undo_len(&h), n + 1);
    h.state_mut().engine.undo();
    h.run_steps(2);
    assert!((value(&h, t, m) - 0.5).abs() < 1e-6, "undo restores it");

    // With Shift: 50 * 0.002 = 0.1.
    drag_up(&mut h, c, 50.0, Modifiers::SHIFT);
    assert!((value(&h, t, m) - 0.6).abs() < 1e-4, "{}", value(&h, t, m));
    h.state_mut().engine.undo();
    h.run_steps(2);

    // One wheel notch: deltaY (120 / 256) * 0.25 = 0.1171875, one undo step.
    let n = undo_len(&h);
    ev(&mut h, c, None, Modifiers::NONE);
    h.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, 1.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    assert!((value(&h, t, m) - (0.5 + JUCE_NOTCH_DELTA_Y * 0.25)).abs() < 1e-4, "{}", value(&h, t, m));
    assert_eq!(undo_len(&h), n + 1);

    // Double-click: back to the default, one undo step.
    let n = undo_len(&h);
    for pressed in [true, false, true, false] {
        ev(&mut h, c, Some(pressed), Modifiers::NONE);
    }
    h.run_steps(2);
    assert!((value(&h, t, m) - 0.5).abs() < 1e-6, "{}", value(&h, t, m));
    assert_eq!(undo_len(&h), n + 1);
    h.state_mut().engine.undo();
    h.run_steps(2);
    assert!((value(&h, t, m) - (0.5 + JUCE_NOTCH_DELTA_Y * 0.25)).abs() < 1e-4, "undo brings the wheel's value back");
}

#[test]
fn a_press_beside_the_knob_does_nothing() {
    let (mut h, t, m) = harness();
    let c = knob_centre(&h, t, m) + vec2(0.0, 60.0); // on the readout, below the knob
    drag_up(&mut h, c, 50.0, Modifiers::NONE);
    assert!((value(&h, t, m) - 0.5).abs() < 1e-6);
}

#[test]
fn juce_wheel_units() {
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, crate::widgets::KNOB_NOTCH_PX * 2.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    let mut got = 0.0;
    let mut out = ctx.run_ui(raw, |ui| got = juce_wheel_delta_y(ui));
    out.textures_delta.clear();
    assert!((got - 2.0 * JUCE_NOTCH_DELTA_Y).abs() < 1e-6);
}
