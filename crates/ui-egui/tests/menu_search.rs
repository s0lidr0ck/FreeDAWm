//! Opening an insert, instrument, send or I/O menu gives its search field the keyboard: typing
//! filters at once, Up/Down move through the matches and Enter picks one.

use egui::accesskit::Role;
use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::Route;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn harness(engine: soundcraft_engine::Engine) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(engine, None, Services::default());
    app.ui.window = MainWindow::Mix;
    let mut h = Harness::builder().with_size(vec2(1400.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn open(h: &mut Harness<SoundApp>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

fn type_text(h: &mut Harness<SoundApp>, text: &str) {
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
}

fn press(h: &mut Harness<SoundApp>, key: Key) {
    h.key_press(key);
    h.run_steps(2);
}

/// The built-ins the picker lists for `text`, in its order.
fn built_in(instruments: bool, text: &str) -> Vec<&'static soundcraft_dsp::PluginInfo> {
    soundcraft_ui_egui::mix_window::ranked_built_ins(text, instruments)
}

fn vox() -> soundcraft_engine::Engine {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    engine
}

#[test]
fn an_insert_menu_searches_as_soon_as_it_opens() {
    // "tr": FireStation's Transient, then the Trim utility.
    let reverbs = built_in(false, "tr");
    assert!(reverbs.len() >= 2, "the test needs two matches");
    let mut h = harness(vox());
    open(&mut h, "Add module");
    type_text(&mut h, "tr");
    assert!(h.query_by_label(reverbs[0].name).is_some(), "typing went to the search");
    assert!(h.query_by_label_contains("Utilities").is_none(), "the matches replace the groups");

    h.get_by_role(Role::TextInput).click();
    h.run_steps(3);
    assert!(h.query_by_label(reverbs[0].name).is_some(), "a click in the field keeps the menu open");

    press(&mut h, Key::ArrowUp);
    press(&mut h, Key::ArrowDown);
    press(&mut h, Key::Enter);
    let insert = h.state().engine.session().tracks[0].mixer.inserts.first().cloned().expect("Enter inserts the highlighted plugin");
    assert_eq!(insert.plugin, reverbs[1].id);
    assert!(h.query_by_label(reverbs[0].name).is_none(), "picking closes the menu");
}

#[test]
fn a_menu_opens_with_an_empty_search() {
    let mut h = harness(vox());
    open(&mut h, "Add module");
    type_text(&mut h, "tr");
    press(&mut h, Key::Escape);
    assert!(h.query_by_label(built_in(false, "tr")[0].name).is_none(), "Escape closes the menu");

    open(&mut h, "Add module");
    assert!(h.query_by_label_contains("Utilities").is_some(), "the old search is gone, so the groups show");
    type_text(&mut h, "zzzz");
    assert!(h.query_by_label("No matches").is_some());
    press(&mut h, Key::Enter);
    assert!(h.state().engine.session().tracks[0].mixer.inserts.is_empty(), "Enter without a match picks nothing");
}

#[test]
fn picks_without_the_search_still_close_the_menu() {
    let first =
        soundcraft_dsp::listed_plugins().find(|p| !p.is_instrument && !p.with_engines() && p.category == soundcraft_dsp::Category::Other).unwrap();
    let category = "Utilities ⏵".to_string();
    let mut h = harness(vox());
    h.get_by_label("Add module").click();
    h.run_steps(3);
    h.get_by_label(&category).hover();
    h.run_steps(3);
    h.get_by_label(first.name).click();
    h.run_steps(3);
    assert_eq!(h.state().engine.session().tracks[0].mixer.inserts.iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), [first.id]);
    assert!(h.query_by_label(&category).is_none(), "a plugin from a group closes the menu");

    let tile = format!("{} module", first.short_name);
    for (item, bypassed) in [("Bypass", Some(true)), ("Remove", None)] {
        h.get_by_label(&tile).click_secondary();
        h.run_steps(3);
        h.get_by_label(item).click();
        h.run_steps(3);
        assert_eq!(h.state().engine.session().tracks[0].mixer.inserts.first().map(|i| i.bypass), bypassed);
        assert!(h.query_by_label(item).is_none(), "{item} closes the menu");
    }
}

#[test]
fn an_instrument_menu_searches_as_soon_as_it_opens() {
    let drums = built_in(true, "drum");
    assert!(!drums.is_empty(), "the test needs a built-in drum instrument");
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let mut h = harness(engine);
    h.state_mut().ui.mix_views.push("instruments".into());
    h.run_steps(2);
    // The track has an instrument: a click opens it, the picker is on right-click.
    h.get_by_label("Instrument").click_secondary();
    h.run_steps(3);
    type_text(&mut h, "drum");
    press(&mut h, Key::Enter);
    let instrument = h.state().engine.session().tracks[0].instrument.clone().unwrap();
    assert_eq!(instrument.plugin, drums[0].id);
    assert!(h.query_by_label(drums[0].name).is_none(), "picking closes the menu");
}

#[test]
fn send_and_output_menus_search_buses_as_soon_as_they_open() {
    let mut engine = vox();
    let vox = engine.session().tracks[0].id;
    for (slot, bus) in [(0, "Verb"), (1, "Delay")] {
        engine.execute("mix.send", &json!({"track": vox.0, "slot": slot, "bus": bus})).unwrap();
    }
    let mut h = harness(engine);
    let bus = |h: &Harness<SoundApp>, name: &str| h.state().engine.session().busses.iter().find(|b| b.name == name).unwrap().id;

    open(&mut h, "Send C");
    type_text(&mut h, "del");
    assert!(h.query_by_label("Verb").is_none());
    press(&mut h, Key::Enter);
    let send = h.state().engine.session().tracks[0].mixer.sends[2].clone().expect("Enter picks the bus");
    assert_eq!(send.target, Route::Bus(bus(&h, "Delay")));

    open(&mut h, "Output");
    type_text(&mut h, "ver");
    press(&mut h, Key::Enter);
    assert_eq!(h.state().engine.session().tracks[0].mixer.output, Route::Bus(bus(&h, "Verb")));
}

#[test]
fn scripted_keys_reach_the_search_with_the_window_in_the_background() {
    let plates = built_in(false, "trim");
    let mut h = harness(vox());
    h.input_mut().focused = false;
    open(&mut h, "Add module");
    type_text(&mut h, "trim");
    press(&mut h, Key::Enter);
    let insert = h.state().engine.session().tracks[0].mixer.inserts.first().cloned().expect("Enter works without window focus, as typing does");
    assert_eq!(insert.plugin, plates[0].id);
}
