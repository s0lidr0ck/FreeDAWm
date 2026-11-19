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
    for plugin in ["subtractive_synth", "wavetable_synth", "fm_synth"] {
        let (mut h, _) = harness(plugin);
        let info = soundcraft_dsp::plugin_info(plugin).unwrap();
        // Steeple shows one operator's controls at a time: its OP keys bring each one up.
        let op_of = |id: &str| id.strip_prefix("op").and_then(|r| r.chars().next()).and_then(|c| c.to_digit(10));
        for p in info.params.iter().filter(|p| op_of(p.id).is_none()) {
            // `get_by_label` fails on none and on more than one.
            let _ = h.get_by_label(p.name);
        }
        for k in 1..=4 {
            let mine: Vec<_> = info.params.iter().filter(|p| op_of(p.id) == Some(k)).collect();
            if mine.is_empty() {
                continue;
            }
            h.get_by_label(&format!("OP {k}")).click();
            step(&mut h);
            step(&mut h);
            for p in mine {
                let _ = h.get_by_label(p.name);
            }
        }
        // The sources to patch with, and the panel's own switches.
        for s in ["Amp Env", "Env 2", "LFO", "Velocity", "Mod Wheel", "Key", "Random", "Surprise", "Front", "Back", "Pin", "Follow"] {
            let _ = h.get_by_label(s);
        }
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

fn slots(h: &Harness<'_, SoundApp>, t: TrackId) -> Vec<(String, String, f32)> {
    let ins = h.state().engine.session().track(t).unwrap().instrument.clone().unwrap();
    ins.modulation.slots.iter().map(|s| (s.source.clone(), s.dest.clone(), s.amount)).collect()
}

/// A source chip dragged onto a knob patches it (one undo step); dropped on a knob that takes no
/// modulation, nothing changes.
#[test]
fn a_source_dragged_onto_a_knob_patches_it() {
    let (mut h, t) = harness("subtractive_synth");
    let (from, to) = (centre(&h, "LFO"), centre(&h, "Cutoff"));
    press_drag_release(&mut h, from, to);
    assert_eq!(slots(&h, t), vec![("lfo".to_string(), "cutoff".to_string(), 0.25)]);
    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert!(slots(&h, t).is_empty(), "one undo step");
    for _ in 0..30 {
        step(&mut h);
    }
    let (from, to) = (centre(&h, "Velocity"), centre(&h, "Filter Type"));
    press_drag_release(&mut h, from, to);
    assert!(slots(&h, t).is_empty(), "the filter type steps: not a destination");
}

/// Surprise adds two or three patches as one undo step; Back turns the panel round to its
/// cables and Front back again.
#[test]
fn surprise_is_one_step_and_back_shows_the_cables() {
    let (mut h, t) = harness("wavetable_synth");
    h.get_by_label("Surprise").click();
    step(&mut h);
    step(&mut h);
    let n = slots(&h, t).len();
    assert!((2..=3).contains(&n), "{n}");
    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert!(slots(&h, t).is_empty());
    h.get_by_label("Back").click();
    step(&mut h);
    step(&mut h);
    let _ = h.get_by_label("Patch cables");
    assert!(h.query_by_label("Cutoff").is_none(), "the knobs are on the front");
    h.get_by_label("Front").click();
    step(&mut h);
    step(&mut h);
    let _ = h.get_by_label("Cutoff");
}

/// On the Back, a cable dragged from a source's output to a socket patches it, and pulled off
/// the socket into the panel removes it (each one undo step).
#[test]
fn on_the_back_a_cable_patches_and_pulling_it_out_removes_it() {
    let (mut h, t) = harness("subtractive_synth");
    h.get_by_label("Back").click();
    step(&mut h);
    step(&mut h);
    let (from, to) = (centre(&h, "LFO out"), centre(&h, "Cutoff socket"));
    press_drag_release(&mut h, from, to);
    assert_eq!(slots(&h, t), vec![("lfo".to_string(), "cutoff".to_string(), 0.25)]);
    for _ in 0..30 {
        step(&mut h);
    }
    let away = to + vec2(0.0, -60.0);
    press_drag_release(&mut h, to, away);
    assert!(slots(&h, t).is_empty(), "pulled out");
    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(slots(&h, t).len(), 1, "the pull is one undo step");
}

/// The preset display's next button loads the first factory preset and shows its name.
#[test]
fn the_preset_display_steps_through_the_factory_presets() {
    let (mut h, t) = harness("subtractive_synth");
    h.get_by_label("Next preset").click();
    step(&mut h);
    step(&mut h);
    let ins = h.state().engine.session().track(t).unwrap().instrument.clone().unwrap();
    // The menu's first preset: Init, at the suite's level.
    assert_eq!(ins.preset, "Init");
    assert_eq!(ins.params.get("osc2_semitones").copied(), Some(0.0));
    // The next one names its macros and brings their patch with it.
    h.get_by_label("Next preset").click();
    step(&mut h);
    step(&mut h);
    let ins = h.state().engine.session().track(t).unwrap().instrument.clone().unwrap();
    assert_eq!(ins.preset, "Init for Patching");
    assert_eq!(ins.modulation.macro_name(0), "Tone");
    let _ = h.get_by_label("Presets");
}

/// Every filled circle painted this frame (shapes nested in lists included): (centre, radius,
/// colour).
fn circles(h: &Harness<'_, SoundApp>) -> Vec<(Pos2, f32, egui::Color32)> {
    fn walk(s: &egui::Shape, out: &mut Vec<(Pos2, f32, egui::Color32)>) {
        match s {
            egui::Shape::Circle(c) => out.push((c.center, c.radius, c.fill)),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.output().shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// While a voice sounds, a patched knob shows where its value is now: a bright dot riding its
/// arc at the position the synth reads back, off the knob's own pointer (which stays where it was
/// set). With Reduce motion the arcs stay static (no dot).
#[test]
fn a_patched_knob_shows_its_live_value_as_a_dot() {
    use soundcraft_dsp::plugins::synth_mod as sm;
    use soundcraft_ui_egui::synth_face::kit;
    let (mut h, t) = harness("subtractive_synth");
    h.state_mut().engine.execute("mix.mod_add", &json!({"track": t.0, "source": "lfo", "dest": "cutoff", "amount": 0.5})).unwrap();
    // The synth's read-back as the audio thread leaves it: a voice sounding, the LFO near its
    // top and the cutoff at 90 % of its travel.
    let a = std::sync::Arc::new(soundcraft_dsp::analysis::Analysis::with_live(sm::LIVE_LEN));
    let d = sm::dests("subtractive_synth").iter().position(|x| *x == "cutoff").unwrap();
    a.live.set(sm::LIVE_SOUNDING, 1.0);
    a.live.set(sm::LIVE_SOURCES + 2, 0.8);
    a.live.set(sm::LIVE_DESTS + d, 0.9);
    let mut meter = soundcraft_mix::StripMeter::default();
    meter.analysis.push((soundcraft_mix::INSTRUMENT_ANALYSIS, a.clone()));
    h.state_mut().preview_meters = Some(soundcraft_playback::MeterSnapshot { tracks: [(t, meter)].into_iter().collect(), main: Default::default() });
    for _ in 0..3 {
        step(&mut h);
    }
    let c = centre(&h, "Cutoff");
    let dots = |h: &Harness<'_, SoundApp>| -> Vec<Pos2> {
        circles(h).into_iter().filter(|(p, _, col)| *col == kit::LIVE_DOT && p.distance(c) < kit::KNOB * 0.6).map(|(p, _, _)| p).collect()
    };
    let got = dots(&h);
    assert_eq!(got.len(), 1, "one dot on the cutoff knob: {got:?}");
    let want = kit::live_dot_at(c, kit::KNOB, 0.9);
    assert!(got[0].distance(want) < 1.0, "at 90 % of the travel: {:?} against {want:?}", got[0]);
    let knob = soundcraft_dsp::plugin_info("subtractive_synth").unwrap().param("cutoff").unwrap();
    let set = knob.to_normalized(param_or_default(&h, t, "cutoff"));
    assert!(got[0].distance(kit::live_dot_at(c, kit::KNOB, set)) > 5.0, "off the knob's own pointer");
    // The LFO moves on: the dot follows.
    a.live.set(sm::LIVE_DESTS + d, 0.4);
    step(&mut h);
    assert!(dots(&h).first().is_some_and(|p| p.distance(kit::live_dot_at(c, kit::KNOB, 0.4)) < 1.0));
    // Reduce motion: static arcs only.
    h.state_mut().extra.reduce_motion = true;
    step(&mut h);
    step(&mut h);
    assert!(dots(&h).is_empty(), "no moving dot with Reduce motion");
}

fn param_or_default(h: &Harness<'_, SoundApp>, t: TrackId, id: &str) -> f32 {
    let ins = h.state().engine.session().track(t).unwrap().instrument.clone().unwrap();
    ins.params.get(id).copied().unwrap_or_else(|| soundcraft_dsp::plugin_info(&ins.plugin).unwrap().param(id).unwrap().default)
}
