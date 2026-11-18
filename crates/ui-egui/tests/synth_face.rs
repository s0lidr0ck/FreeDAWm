//! The built-in synths' instrument panels: every parameter is on the panel, a knob drag is one
//! undo step, a double-click returns a knob to its default, and the preset display steps through
//! the factory presets.

use egui::{Event, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{Services, SoundApp};

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

/// The app with an instrument track playing `plugin`, its window open.
fn harness(plugin: &str) -> (Harness<'static, SoundApp>, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.engine.execute("track.new", &json!({"kind": "instrument", "name": "Inst 1"})).unwrap();
    app.engine.execute("mix.instrument", &json!({"track": "Inst 1", "plugin": plugin})).unwrap();
    let t = app.engine.session().track_by_name("Inst 1").unwrap().id;
    app.ui.plugin_windows.push((t, soundcraft_mix::SlotKey::Instrument));
    let mut h = Harness::builder().with_size(vec2(1500.0, 1100.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    (h, t)
}

fn param(h: &Harness<'_, SoundApp>, t: TrackId, id: &str) -> f32 {
    *h.state().engine.session().track(t).unwrap().instrument.as_ref().unwrap().params.get(id).unwrap()
}

fn centre(h: &Harness<'_, SoundApp>, label: &str) -> Pos2 {
    h.get_by_label(label).rect().center()
}

fn press_drag_release(h: &mut Harness<'_, SoundApp>, from: Pos2, to: Pos2) {
    let ev = &mut h.state_mut().synthetic;
    ev.push(Event::PointerMoved(from));
    ev.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    step(h);
    for i in 1..=6 {
        let p = from + (to - from) * (i as f32 / 6.0);
        h.state_mut().synthetic.push(Event::PointerMoved(p));
        step(h);
    }
    h.state_mut().synthetic.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    step(h);
    step(h);
}

/// Every parameter of each synth with a panel has exactly one control on it, under its name.
#[test]
fn every_parameter_is_on_the_panel() {
    for plugin in ["subtractive_synth"] {
        let (h, _) = harness(plugin);
        let info = soundcraft_dsp::plugin_info(plugin).unwrap();
        for p in info.params {
            // `get_by_label` fails on none and on more than one.
            let _ = h.get_by_label(p.name);
        }
        let _ = h.get_by_label("Filter curve");
    }
}

/// A knob dragged is one undo step; a double-click returns it to its default.
#[test]
fn a_knob_drag_is_one_undo_step_and_a_double_click_resets() {
    let (mut h, t) = harness("subtractive_synth");
    let before = param(&h, t, "resonance");
    let c = centre(&h, "Resonance");
    press_drag_release(&mut h, c, c - vec2(0.0, 40.0));
    let after = param(&h, t, "resonance");
    assert!(after > before + 5.0, "{before} -> {after}");
    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(param(&h, t, "resonance"), before, "the whole drag is one step");
    h.state_mut().engine.execute("mix.instrument_param", &json!({"track": t.0, "param": "resonance", "value": 90.0})).unwrap();
    step(&mut h);
    let c = centre(&h, "Resonance");
    // Half a second on, so it isn't counted with the drag's click.
    for _ in 0..30 {
        step(&mut h);
    }
    h.input_mut().events.push(Event::PointerMoved(c));
    h.step();
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::PointerMoved(c));
        h.input_mut().events.push(Event::PointerButton { pos: c, button: PointerButton::Primary, pressed, modifiers: Default::default() });
        h.step();
    }
    step(&mut h);
    assert_eq!(param(&h, t, "resonance"), 20.0, "the default");
}

/// The preset display's next button loads the first factory preset and shows its name.
#[test]
fn the_preset_display_steps_through_the_factory_presets() {
    let (mut h, t) = harness("subtractive_synth");
    h.get_by_label("Next preset").click();
    step(&mut h);
    step(&mut h);
    let ins = h.state().engine.session().track(t).unwrap().instrument.clone().unwrap();
    assert_eq!(ins.preset, "Round Bass");
    assert_eq!(ins.params.get("osc2_semitones").copied(), Some(-12.0));
    let _ = h.get_by_label("Presets");
}
