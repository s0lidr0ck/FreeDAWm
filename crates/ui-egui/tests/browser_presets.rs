//! The plugin browser's Presets tab: the selected plugin's presets, searched, dragged onto a
//! track (the plugin inserted with the preset, one undo step) or onto an insert already holding
//! that plugin (the preset loaded into it), and double-clicked.
mod editor_support;

use editor_support::{add, app_with_tracks, double_click, drag};
use egui::accesskit::Role;
use egui::{Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::{Map, Value, json};
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::browser::BrowserTab;
use soundcraft_ui_egui::browser::catalog::build_catalog;
use soundcraft_ui_egui::browser::drop::{DropTarget, PluginDrag, apply_drop};
use soundcraft_ui_egui::browser::presets::{PresetRef, presets_for};

const COMP: &str = "fs_comp";

/// "Voice" (audio), with the inserts column shown.
fn app() -> SoundApp {
    let mut app = app_with_tracks(&["Voice"]);
    app.engine.execute("view.edit_inserts_ae", &json!({"value": true})).unwrap();
    app.browser_catalog = Some(build_catalog(soundcraft_dsp::plugins(), &[], &[], &[], Vec::new()));
    app
}

/// An Edit window harness with the browser on its Presets tab and `plugin`'s card selected.
fn browser(mut app: SoundApp, plugin: Option<&str>) -> Harness<'static, SoundApp> {
    app.ui.browser.tab = BrowserTab::Presets;
    app.ui.browser.selected = plugin.map(str::to_string);
    app.run("ui.set", json!({"theme": "dark"})).unwrap();
    app.run("window.plugin_browser", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1700.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn item(app: &SoundApp, id: &str) -> soundcraft_ui_egui::browser::catalog::BrowserItem {
    app.browser_catalog.as_ref().unwrap().items.iter().find(|i| i.id == id).unwrap().clone()
}

/// The first factory preset of the Comp: (group, name, its parameters).
fn first_factory() -> (String, String, Map<String, Value>) {
    first_factory_of(COMP)
}

fn first_factory_of(plugin: &str) -> (String, String, Map<String, Value>) {
    let info = soundcraft_dsp::plugin_info(plugin).unwrap();
    let (group, list) = soundcraft_ui_egui::fs_presets::factory_groups(info).first().unwrap();
    let (name, json) = list.first().unwrap();
    let (_, params) = soundcraft_ui_egui::fs_presets::factory_recall(info, name, json).unwrap();
    (group.clone(), name.to_string(), params)
}

fn undo_len(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

fn voice(h: &Harness<SoundApp>) -> soundcraft_model::Track {
    h.state().engine.session().track_by_name("Voice").unwrap().clone()
}

/// The module's parameters hold every value of the preset.
fn holds(ins: &soundcraft_model::Insert, params: &Map<String, Value>) {
    for (k, v) in params {
        assert!((ins.params[k] - v.as_f64().unwrap() as f32).abs() < 1e-4, "{k}: {} vs {v}", ins.params[k]);
    }
}

/// The preset row `name` (searched for, so it is in view).
fn row(h: &mut Harness<SoundApp>, name: &str) -> Pos2 {
    h.state_mut().ui.browser.preset_text = name.into();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, name).rect().center()
}

fn header(h: &Harness<SoundApp>, name: &str) -> Pos2 {
    let id = h.state().engine.session().track_by_name(name).unwrap().id.0;
    let r = h.state().edit_layout.rows.iter().find(|(t, _)| *t == id).unwrap().1;
    let r = Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]));
    pos2(r.min.x + 40.0, r.center().y)
}

#[test]
fn builtin_presets_listed_for_selected() {
    let app = app();
    let list = presets_for(&app, &item(&app, COMP));
    let info = soundcraft_dsp::plugin_info(COMP).unwrap();
    let groups = soundcraft_ui_egui::fs_presets::factory_groups(info);
    let factory: usize = groups.iter().map(|(_, l)| l.len()).sum();
    assert_eq!(list.len(), factory, "no user preset folder in tests");
    let (group, name, _) = first_factory();
    assert_eq!(list[0], (group.clone(), name.clone(), PresetRef::Builtin { list: group.clone(), name: name.clone() }));
    // The tab shows the selected card's groups and presets, and the search narrows them.
    let mut h = browser(app, Some(COMP));
    assert!(h.query_by_label(&group).is_some(), "group {group}");
    assert!(h.query_by_role_and_label(Role::Button, &name).is_some(), "preset {name}");
    let other = &groups.last().unwrap().1.last().unwrap().0;
    h.state_mut().ui.browser.preset_text = name.clone();
    h.run_steps(3);
    assert!(h.query_by_role_and_label(Role::Button, &name).is_some());
    if !other.to_lowercase().contains(&name.to_lowercase()) {
        assert!(h.query_by_role_and_label(Role::Button, other).is_none(), "{other} filtered out");
    }
    // No card selected: the plugin of the open module drawer.
    h.state_mut().ui.browser.preset_text.clear();
    h.state_mut().ui.browser.selected = None;
    let (t, m) = add(h.state_mut(), "Voice", COMP);
    h.state_mut().ui.drawers.insert(t, m);
    h.run_steps(3);
    assert!(h.query_by_role_and_label(Role::Button, &name).is_some(), "the focused module's presets");
}

#[test]
fn dropping_a_builtin_preset_inserts_with_it() {
    let (_, name, params) = first_factory();
    let mut h = browser(app(), Some(COMP));
    let undo = undo_len(&h);
    let from = row(&mut h, &name);
    let to = header(&h, "Voice");
    drag(&mut h, from, to, Modifiers::NONE);
    let v = voice(&h);
    assert_eq!(v.mixer.inserts.len(), 1);
    let ins = &v.mixer.inserts[0];
    assert_eq!((ins.plugin.as_str(), ins.preset.as_str()), (COMP, name.as_str()));
    holds(ins, &params);
    assert_eq!(undo_len(&h), undo + 1, "one undo step");
    editor_support::undo(&mut h);
    assert!(voice(&h).mixer.inserts.is_empty());
    // The placed module opened, so the tab followed it; with it undone, the card is clicked again.
    h.state_mut().ui.browser.selected = Some(COMP.into());
    // Double-clicked with the track selected: the same.
    let id = voice(&h).id.0;
    h.state_mut().run("edit.select", json!({"tracks": [id]})).unwrap();
    h.run_steps(40);
    let undo = undo_len(&h);
    let at = row(&mut h, &name);
    double_click(&mut h, at);
    let v = voice(&h);
    assert_eq!(v.mixer.inserts.len(), 1);
    holds(&v.mixer.inserts[0], &params);
    assert_eq!(undo_len(&h), undo + 1);
}

#[test]
fn preset_onto_matching_insert_loads_into_it() {
    let (group, name, params) = first_factory();
    let mut app = app();
    let (t, m) = add(&mut app, "Voice", COMP);
    let undo = app.engine.undo_history().len();
    let drag_of = |preset| PluginDrag { plugin: COMP.into(), kind: soundcraft_ui_egui::browser::catalog::Kind::Effect, preset };
    apply_drop(&mut app, &drag_of(Some(PresetRef::Builtin { list: group.clone(), name: name.clone() })), DropTarget::InsertSlot(t, 0)).unwrap();
    let ins = app.engine.session().track(t).unwrap().mixer.inserts.clone();
    assert_eq!(ins.len(), 1);
    assert_eq!((ins[0].id, ins[0].preset.as_str()), (m, name.as_str()), "the same module, now with the preset");
    holds(&ins[0], &params);
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    // Onto another plugin's insert: replaced by the plugin with the preset (one step).
    app.engine.execute("mix.module_add", &json!({"track": t.0, "plugin": "gain"})).unwrap();
    let undo = app.engine.undo_history().len();
    apply_drop(&mut app, &drag_of(Some(PresetRef::Builtin { list: group.clone(), name: name.clone() })), DropTarget::InsertSlot(t, 1)).unwrap();
    let ins = app.engine.session().track(t).unwrap().mixer.inserts.clone();
    assert_eq!(ins.iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), [COMP, COMP]);
    holds(&ins[1], &params);
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    // A preset that is not there: an error, nothing placed.
    let session = app.engine.session().clone();
    let bad = drag_of(Some(PresetRef::Builtin { list: group, name: "No Such Preset".into() }));
    assert!(apply_drop(&mut app, &bad, DropTarget::Track(t)).is_err());
    assert_eq!(app.engine.session().tracks, session.tracks);
}

#[test]
fn tab_says_no_presets() {
    let h = browser(app(), Some("gain"));
    let name = soundcraft_dsp::plugin_info("gain").unwrap().name;
    assert!(h.query_by_label(&format!("No presets for {name}")).is_some());
    let h = browser(app(), None);
    assert!(h.query_by_label("Select a plugin card to see its presets.").is_some());
}

/// A VST3 `.vstpreset` resolves to the plugin's state (component + controller) under the file's
/// name; one made for another plugin is refused.
#[test]
fn vstpreset_files_resolve_to_a_state() {
    let fixture =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../vst3-host/tests/fixture/presets/Minimal.vstpreset");
    let (name, state) = soundcraft_ui_egui::browser::presets::vstpreset_state(&fixture, "0123456789ABCDEF0123456789ABCDEF").unwrap();
    assert_eq!(name, "Minimal");
    assert_eq!(state, soundcraft_vst3_host::vstpreset::pack_state(b"component state", b"ctrl").unwrap());
    let err = soundcraft_ui_egui::browser::presets::vstpreset_state(&fixture, "FFFFFFFF89ABCDEF0123456789ABCDEF").unwrap_err();
    assert!(err.contains("different plugin"), "{err}");
}

/// The tab follows whichever came last: a module opened after a card was clicked takes over (and
/// a double-click loads into it), a card clicked after that takes over again. The clicked card is
/// not saved, and one the catalog no longer has falls back to the open module.
#[test]
fn the_tab_follows_whatever_came_last() {
    let mut h = browser(app(), Some(COMP));
    let (t, m) = add(h.state_mut(), "Voice", "fs_dist");
    h.state_mut().ui.drawers.insert(t, m);
    h.run_steps(3);
    assert_eq!(h.state().ui.browser.selected, None, "the module opened later wins");
    assert!(h.query_by_label_contains("the open module").is_some());
    let (_, name, params) = first_factory_of("fs_dist");
    let undo = undo_len(&h);
    let at = row(&mut h, &name);
    double_click(&mut h, at);
    let v = voice(&h);
    assert_eq!(v.mixer.inserts.len(), 1, "loaded into the open module, nothing placed");
    holds(&v.mixer.inserts[0], &params);
    assert_eq!(undo_len(&h), undo + 1);
    // A card clicked afterwards takes over (the drawer stays open).
    h.state_mut().ui.browser.selected = Some(COMP.into());
    h.state_mut().ui.browser.preset_text.clear();
    h.run_steps(3);
    assert!(h.query_by_label_contains("the open module").is_none());
    assert_eq!(h.state().ui.browser.selected.as_deref(), Some(COMP));
    // Not saved with the window state.
    let saved = serde_json::to_value(&h.state().ui).unwrap();
    assert!(saved["browser"].get("selected").is_none());
    // A card the catalog no longer has: the open module's presets.
    h.state_mut().ui.browser.selected = Some("vst3:gone".into());
    h.run_steps(3);
    assert!(h.query_by_label_contains("the open module").is_some());
}
