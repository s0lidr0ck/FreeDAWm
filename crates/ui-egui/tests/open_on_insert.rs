//! #486: picking a plugin for an insert slot opens it straight away, as DAWs do, from the Mix
//! window's slots and the Edit window's Inserts column alike.

use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn harness(engine: soundcraft_engine::Engine, window: MainWindow) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(engine, None, Services::default());
    app.ui.window = window;
    app.run("view.edit_inserts_ae", json!({})).unwrap();
    app.run("track.height", json!({"tracks": ["Vox"], "height": "medium"})).unwrap();
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

fn vox() -> soundcraft_engine::Engine {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    engine
}

fn vox_id(h: &Harness<SoundApp>) -> TrackId {
    h.state().engine.session().tracks[0].id
}

/// Two listed utilities (the stock effects are hidden), and their group's menu label.
fn two_effects() -> (String, &'static soundcraft_dsp::PluginInfo, &'static soundcraft_dsp::PluginInfo) {
    let utility = |p: &&soundcraft_dsp::PluginInfo| !p.is_instrument && p.category == soundcraft_dsp::Category::Other;
    let first = soundcraft_dsp::listed_plugins().find(utility).unwrap();
    let other = soundcraft_dsp::listed_plugins().filter(utility).find(|p| p.id != first.id).unwrap();
    ("Utilities ⏵".to_string(), first, other)
}

/// Opens `slot`'s menu (a click on an empty slot, a right-click on a filled one) and picks
/// `plugin` from its category.
fn pick(h: &mut Harness<SoundApp>, slot: &str, filled: bool, category: &str, plugin: &str) {
    let node = h.get_by_label(slot);
    if filled {
        node.click_secondary();
    } else {
        node.click();
    }
    h.run_steps(3);
    h.get_by_label(category).hover();
    h.run_steps(3);
    h.get_by_label(plugin).click();
    h.run_steps(3);
}

fn inserted(h: &Harness<SoundApp>) -> Option<String> {
    h.state().engine.session().tracks[0].mixer.inserts.first().map(|i| i.plugin.clone())
}

/// The module that opened: a built-in opens in its drawer.
fn opened(h: &Harness<SoundApp>) -> Option<soundcraft_model::ModuleId> {
    h.state().ui.drawers.get(&vox_id(h)).copied()
}

fn first_module(h: &Harness<SoundApp>) -> Option<soundcraft_model::ModuleId> {
    h.state().engine.session().tracks[0].mixer.inserts.first().map(|i| i.id)
}

#[test]
fn picking_a_plugin_from_the_plus_row_opens_it() {
    let (category, plugin, _) = two_effects();
    let mut h = harness(vox(), MainWindow::Mix);
    pick(&mut h, "Add module", false, &category, plugin.name);
    assert_eq!(inserted(&h).as_deref(), Some(plugin.id));
    assert_eq!(opened(&h), first_module(&h), "the new module's drawer opened");
    assert!(opened(&h).is_some());
}

#[test]
fn picking_from_the_search_opens_it_too() {
    let mut h = harness(vox(), MainWindow::Mix);
    h.get_by_label("Add module").click();
    h.run_steps(3);
    h.event(egui::Event::Text("reverb".into()));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(inserted(&h).is_some(), "Enter inserted a reverb");
    assert_eq!(opened(&h), first_module(&h));
}

#[test]
fn replacing_a_module_opens_the_new_plugin() {
    let (category, first, second) = two_effects();
    let mut engine = vox();
    let track = engine.session().tracks[0].id;
    engine.execute("mix.module_add", &json!({"track": track.0, "plugin": first.id})).unwrap();
    let mut h = harness(engine, MainWindow::Mix);
    assert!(opened(&h).is_none(), "nothing opens before a pick");
    h.get_by_label(&format!("{} module", first.short_name)).click_secondary();
    h.run_steps(3);
    h.get_by_label("Replace ⏵").hover();
    h.run_steps(3);
    h.get_by_label(&category).hover();
    h.run_steps(3);
    h.get_by_label(second.name).click();
    h.run_steps(3);
    assert_eq!(inserted(&h).as_deref(), Some(second.id));
    assert_eq!(h.state().engine.session().tracks[0].mixer.inserts.len(), 1, "replaced in place");
    assert_eq!(opened(&h), first_module(&h));
    assert!(h.state_mut().engine.undo());
    assert_eq!(inserted(&h).as_deref(), Some(first.id), "one undo step");
}

#[test]
fn the_edit_windows_inserts_column_opens_it_too() {
    let (category, plugin, _) = two_effects();
    let mut h = harness(vox(), MainWindow::Edit);
    pick(&mut h, "Add insert", false, &category, plugin.name);
    assert_eq!(inserted(&h).as_deref(), Some(plugin.id));
    assert_eq!(opened(&h), first_module(&h));
}
