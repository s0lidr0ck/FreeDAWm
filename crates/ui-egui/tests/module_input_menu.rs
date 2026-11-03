//! The input menu's "MIDI from" submenu: a checkbox per module that sends MIDI; several make a
//! set; the input button reads `Kick › Shale` and dims (with its tooltip) while MIDI Out is off.

use egui::{Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleRef, Route, TrackId};
use soundcraft_ui_egui::band::midi_sources;
use soundcraft_ui_egui::{Services, SoundApp};

/// "Kick" and "Snare" (audio, a Shale each) and "Drums" (instrument).
fn app() -> SoundApp {
    let mut engine = soundcraft_engine::Engine::default();
    for name in ["Kick", "Snare"] {
        engine.execute("track.new", &json!({"name": name})).unwrap();
        engine.execute("mix.module_add", &json!({"track": name, "plugin": "fs_shale"})).unwrap();
    }
    engine.execute("track.new", &json!({"name": "Drums", "kind": "instrument"})).unwrap();
    let mut app = SoundApp::new(engine, None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    app
}

fn track(app: &SoundApp, name: &str) -> TrackId {
    app.engine.session().track_by_name(name).unwrap().id
}

fn shale(app: &SoundApp, name: &str) -> ModuleRef {
    let t = app.engine.session().track_by_name(name).unwrap();
    ModuleRef { track: t.id, module: t.mixer.inserts[0].id }
}

fn mix(app: SoundApp) -> Harness<'static, SoundApp> {
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

fn texts(h: &Harness<SoundApp>) -> Vec<String> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) => Some(t.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

fn input_of(h: &Harness<SoundApp>, name: &str) -> Route {
    h.state().engine.session().track_by_name(name).unwrap().mixer.input.clone()
}

/// The Drums strip's Input button (the rightmost of the three).
fn open_drums_input(h: &mut Harness<SoundApp>) {
    let mut inputs: Vec<egui::Rect> = h.get_all_by_label("Input").map(|n| n.rect()).collect();
    inputs.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    let last = *inputs.last().unwrap();
    h.get_all_by_label("Input").find(|n| n.rect() == last).unwrap().click();
    h.run_steps(3);
}

/// The "MIDI from" submenu open (the input menu stays open across checkbox clicks; it is
/// opened again only when it closed).
fn show_midi_from(h: &mut Harness<SoundApp>) {
    if h.query_by_label("MIDI from ⏵").is_none() {
        open_drums_input(h);
    }
    h.get_by_label("MIDI from ⏵").hover();
    h.run_steps(3);
}

#[test]
fn midi_from_lists_every_sending_module_as_checkboxes_and_makes_a_set() {
    let app = app();
    let (kick, snare) = (shale(&app, "Kick"), shale(&app, "Snare"));
    let drums = track(&app, "Drums");
    let listed = midi_sources(app.engine.session(), drums);
    assert_eq!(listed.iter().map(|(m, n)| (*m, n.as_str())).collect::<Vec<_>>(), vec![(kick, "Kick › Shale"), (snare, "Snare › Shale")]);
    assert!(midi_sources(app.engine.session(), kick.track).iter().all(|(m, _)| *m != kick), "a track never lists its own module");
    let mut h = mix(app);
    open_drums_input(&mut h);
    assert!(h.query_by_label("MIDI from ⏵").is_some(), "the submenu is there: {:?}", texts(&h));
    show_midi_from(&mut h);
    assert!(h.query_by_label("Kick › Shale").is_some(), "{:?}", texts(&h));
    h.get_by_label("Kick › Shale").click();
    h.run_steps(3);
    assert_eq!(input_of(&h, "Drums"), Route::Modules(vec![kick]));
    // A second one joins the set; the first again leaves it.
    show_midi_from(&mut h);
    h.get_by_label("Snare › Shale").click();
    h.run_steps(3);
    assert_eq!(input_of(&h, "Drums"), Route::Modules(vec![kick, snare]));
    show_midi_from(&mut h);
    h.get_by_label("Kick › Shale").click();
    h.run_steps(3);
    assert_eq!(input_of(&h, "Drums"), Route::Modules(vec![snare]));
    // Unchecking the last leaves no input; undo brings the set back.
    show_midi_from(&mut h);
    h.get_by_label("Snare › Shale").click();
    h.run_steps(3);
    assert_eq!(input_of(&h, "Drums"), Route::None);
    h.state_mut().engine.undo();
    assert_eq!(input_of(&h, "Drums"), Route::Modules(vec![snare]));
}

#[test]
fn the_input_button_names_the_module_and_dims_while_midi_out_is_off() {
    let mut app = app();
    let kick = shale(&app, "Kick");
    let drums = track(&app, "Drums");
    app.run("track.input", json!({"track": drums.0, "input": "module:Kick/Shale"})).unwrap();
    let t = app.engine.session().track(drums).cloned().unwrap();
    assert_eq!(soundcraft_ui_egui::mix_window::input_name(&app, &t), "Kick › Shale");
    let (tip, dim) = soundcraft_ui_egui::mix_window::input_tooltip(&app, &t);
    assert!(dim && tip.starts_with("Shale's MIDI Out is off"), "{tip}");
    app.run("mix.module_param", json!({"track": kick.track.0, "module": kick.module.0, "param": "midiOut", "value": 1.0})).unwrap();
    let t = app.engine.session().track(drums).cloned().unwrap();
    let (tip, dim) = soundcraft_ui_egui::mix_window::input_tooltip(&app, &t);
    assert!(!dim && tip == "MIDI from: Kick › Shale", "{tip}");
    app.run("track.input", json!({"track": drums.0, "input": ["module:Kick/Shale", "module:Snare/Shale"]})).unwrap();
    let t = app.engine.session().track(drums).cloned().unwrap();
    assert_eq!(soundcraft_ui_egui::mix_window::input_name(&app, &t), "2 modules");
    let (tip, dim) = soundcraft_ui_egui::mix_window::input_tooltip(&app, &t);
    assert!(!dim && tip.contains("Kick › Shale, Snare › Shale") && tip.contains("MIDI Out off: Snare › Shale"), "{tip}");
    let h = mix(app);
    assert!(texts(&h).iter().any(|t| t == "2 modules"), "{:?}", texts(&h));
}
