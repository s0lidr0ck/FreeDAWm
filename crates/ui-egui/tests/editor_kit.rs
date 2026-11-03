//! The VST-faithful editors' frame and kit, through CleanQ's editor in the Mix window: its size
//! and scrolling, the narrow drawer behind the flag, gestures as undo steps, key routing, the VST
//! palette in both themes, and hostile state.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Shape, pos2, vec2};
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_ui_egui::editor_kit::frame_rect;
use soundcraft_ui_egui::editors::cleanq;

/// Band 1 of a CleanQ on `t`/`m` made active (a bell at 1 kHz), as a script would.
fn band_one(h: &mut egui_kittest::Harness<'static, soundcraft_ui_egui::SoundApp>, t: soundcraft_model::TrackId, m: soundcraft_model::ModuleId) {
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"b01_on": 2.0, "b01_type": 0.0}})).unwrap();
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "Band 1"})).unwrap();
    h.run_steps(4);
}

#[test]
fn a_short_mixer_scrolls_the_editor_not_squashes_it() {
    let (mut h, t, m) = open("fs_cleanq", vec2(1800.0, 400.0), "dark");
    let f = frame(&h, t, m);
    assert_eq!(f.size(), cleanq::SIZE, "full VST size in a short window");
    assert!(f.bottom() > 400.0, "its bottom is below the window before scrolling");
    // Wheel down over an empty part of the graph: the column scrolls.
    let at = pos2(f.left() + 300.0, f.top() + 200.0);
    for _ in 0..4 {
        wheel_notches(&mut h, at, -5.0);
    }
    h.run_steps(20);
    let f2 = frame(&h, t, m);
    assert_eq!(f2.size(), cleanq::SIZE, "still not squashed");
    assert!(f2.bottom() <= 400.0 + 0.5, "the editor's bottom row is reachable: {f2:?}");
}

#[test]
fn narrow_drawer_when_the_flag_is_off() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_cleanq");
    app.ui.drawers.insert(t, m);
    app.ui.wide_editors = false;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(3);
    assert!(frame_rect(&h.ctx, t, m).is_none());
    let pad = h.get_by_label("Pad strip").rect();
    let bass = h.get_by_label("Bass strip").rect();
    let gap = bass.min.x - pad.max.x;
    assert!(gap < soundcraft_ui_egui::module_drawer::DRAWER_W + 20.0, "the narrow column: {gap}");
    assert!(h.query_by_label("Fold the drawer").is_some());
    // The flag back on: the wide editor.
    h.state_mut().ui.wide_editors = true;
    h.run_steps(3);
    assert_eq!(frame(&h, t, m).size(), cleanq::SIZE);
}

#[test]
fn a_value_box_drag_is_one_undo_step() {
    let (mut h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), "dark");
    band_one(&mut h, t, m);
    let before = raw(&h, t, m, "b01_freq");
    let steps = h.state().engine.undo_history().len();
    let freq = h.get_by_label("Freq").rect().center();
    drag(&mut h, freq, freq - vec2(0.0, 80.0), Modifiers::NONE);
    let after = raw(&h, t, m, "b01_freq");
    assert!(after > before, "dragging up raises the frequency ({before} -> {after})");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "one drag, one undo step");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "b01_freq"), before);
}

#[test]
fn typing_in_an_editor_field_fires_no_shortcut() {
    let (mut h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), "dark");
    band_one(&mut h, t, m);
    let history = h.state().engine.undo_history();
    let requests = h.state().engine.transport_requests.len();
    let freq = h.get_by_label("Freq").rect().center();
    double_click(&mut h, freq);
    h.run_steps(2);
    assert!(h.ctx.egui_wants_keyboard_input(), "the value box is being typed in");
    for (k, text) in [(egui::Key::S, "s"), (egui::Key::D, "d"), (egui::Key::Space, " ")] {
        h.input_mut().events.push(egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.input_mut().events.push(egui::Event::Text(text.into()));
        h.run_steps(2);
    }
    key(&mut h, egui::Key::Delete, Modifiers::NONE);
    assert!(!h.state().engine.transport.playing);
    assert_eq!(h.state().engine.transport_requests.len(), requests, "Space did not start the transport");
    assert_eq!(raw(&h, t, m, "b01_on"), 2.0, "Delete did not remove the band");
    assert_eq!(raw(&h, t, m, "b01_dyn"), 0.0, "D did not toggle dynamics");
    assert_eq!(h.state().engine.undo_history(), history, "no command ran");
}

#[test]
fn two_open_editors_keep_separate_state_and_keys() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (pad, pq) = add(&mut app, "Pad", "fs_cleanq");
    let (bass, bq) = add(&mut app, "Bass", "fs_cleanq");
    app.ui.drawers.insert(pad, pq);
    app.ui.drawers.insert(bass, bq);
    let mut h = harness(app, vec2(3000.0, 1000.0), "dark");
    for (t, m) in [(pad, pq), (bass, bq)] {
        h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"b01_on": 2.0}})).unwrap();
    }
    h.state_mut().run("ui.mix.drawer", json!({"track": pad.0, "module": pq.0, "tab": "Band 1"})).unwrap();
    h.run_steps(4);
    let pad_f = frame(&h, pad, pq);
    let bass_f = frame(&h, bass, bq);
    assert!(!pad_f.intersects(bass_f));
    let cards: Vec<_> = h.query_all_by_label("Freq").map(|n| n.rect()).collect();
    assert_eq!(cards.len(), 1, "only Pad's editor has a band selected");
    assert!(pad_f.contains_rect(cards[0]));

    // D with the pointer over Bass's editor: Bass has no selection, so nothing happens to Pad.
    hover(&mut h, pos2(bass_f.left() + 300.0, bass_f.top() + 250.0));
    key(&mut h, egui::Key::D, Modifiers::NONE);
    assert_eq!(raw(&h, pad, pq, "b01_dyn"), 0.0, "Pad's band 1 is untouched");
    assert_eq!(raw(&h, bass, bq, "b01_dyn"), 0.0);
    assert_eq!(h.query_all_by_label("Freq").count(), 1, "Bass still has nothing selected");

    // D over Pad's editor toggles Pad's selected band.
    hover(&mut h, pos2(pad_f.left() + 300.0, pad_f.top() + 450.0));
    key(&mut h, egui::Key::D, Modifiers::NONE);
    assert_eq!(raw(&h, pad, pq, "b01_dyn"), 1.0);
    assert_eq!(raw(&h, bass, bq, "b01_dyn"), 0.0);
}

#[test]
fn vst_palette_ignores_the_daw_theme() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), theme);
        let f = frame(&h, t, m);
        let first = h
            .output()
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Rect(r) if f.expand(0.5).contains_rect(r.rect) && r.fill.a() > 0 => Some(r.fill),
                _ => None,
            })
            .unwrap();
        assert_eq!(first, cleanq::BACKGROUND, "{theme}");
    }
}

/// A CleanQ module state written before guides were carried.
const OLD_STATE: &str = "<ceiling noiseOn=\"10000\" noiseManual=\"00000\" noiseLevel=\"-50.00 -50.00 -50.00 -50.00 -50.00\" avg=\"0.0\" smooth=\"3\" snap=\"1\" pinned=\"0\" amount=\"1.0\" maxDb=\"12.0\" maxBands=\"16\" cutsOnly=\"0\" generated=\"2 5\"/>";

#[test]
fn old_and_corrupt_state_still_opens() {
    let (mut h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "Ceiling"})).unwrap();
    h.run_steps(30);
    assert_eq!(frame(&h, t, m).size(), cleanq::SIZE);

    let b64 = soundcraft_model::b64::encode(OLD_STATE.as_bytes());
    h.state_mut().run("mix.module_state", json!({"track": t.0, "module": m.0, "state": b64})).unwrap();
    h.run_steps(30);
    assert_eq!(frame(&h, t, m).size(), cleanq::SIZE);
}

// ---------------------------------------------------------------------------- Task 19

const FIRESTATION: [&str; 18] = [
    "fs_cleanq",
    "fs_sat",
    "fs_tape",
    "fs_comp",
    "fs_twine",
    "fs_vint",
    "fs_pitch",
    "fs_mod",
    "fs_dist",
    "fs_transient",
    "fs_filter",
    "fs_delay",
    "fs_reverb",
    "fs_swale",
    "fs_chamber",
    "fs_amp",
    "fs_shale",
    "fs_levelup",
];

#[test]
fn every_firestation_module_has_a_wide_editor() {
    for id in FIRESTATION {
        assert!(soundcraft_ui_egui::editors::has_wide(id), "{id}");
    }
}

#[test]
fn the_wide_editor_flag_is_a_command() {
    let (mut h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), "dark");
    assert!(soundcraft_ui_egui::UiState::default().wide_editors, "wide editors are the default");
    let r = h.state_mut().run("ui.mix.wide_editors", json!({"on": false})).unwrap();
    assert_eq!(r, json!({"on": false}));
    h.run_steps(3);
    assert!(frame_rect(&h.ctx, t, m).is_none(), "the narrow drawer");
    // The setting survives a UiState save and load.
    let saved = serde_json::to_string(&h.state().ui).unwrap();
    let loaded: soundcraft_ui_egui::UiState = serde_json::from_str(&saved).unwrap();
    assert!(!loaded.wide_editors);
    let r = h.state_mut().run("ui.mix.wide_editors", json!({"on": true})).unwrap();
    assert_eq!(r, json!({"on": true}));
    h.run_steps(3);
    assert_eq!(frame(&h, t, m).size(), cleanq::SIZE);
    // Without `on` it flips (the Mix view menu's item).
    assert_eq!(h.state_mut().run("ui.mix.wide_editors", json!({})).unwrap(), json!({"on": false}));
    assert!(h.state_mut().run("ui.mix.wide_editors", json!({"on": 3})).is_err());
    // The Mix view menu's item: there, enabled, checked while on, and a click flips it.
    use soundcraft_ui_egui::menus;
    let path = "View > Mix Window Views > Plugin Editors as in the VST";
    let view = menus::tree().into_iter().find(|n| n.label == "View").unwrap();
    let views = view.children.iter().find(|n| n.label == "Mix Window Views").unwrap();
    assert!(views.children.iter().any(|n| n.path == path));
    let id = menus::command_for_path(path, &menus::ui_aliases()).unwrap();
    assert_eq!(id, "ui.mix.wide_editors");
    assert_eq!(menus::item_state(h.state(), path, Some(&id)), (true, false));
    menus::invoke_menu(h.state_mut(), &id, path);
    assert!(h.state().ui.wide_editors);
    assert_eq!(menus::item_state(h.state(), path, Some(&id)), (true, true));
}

fn dirty(h: &egui_kittest::Harness<soundcraft_ui_egui::SoundApp>, t: soundcraft_model::TrackId, m: soundcraft_model::ModuleId) -> bool {
    h.state().ui.preset_dirty.get(&(t, m)).is_some_and(|d| d.dirty())
}

fn preset_of(h: &egui_kittest::Harness<soundcraft_ui_egui::SoundApp>, t: soundcraft_model::TrackId, m: soundcraft_model::ModuleId) -> String {
    h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().preset.clone()
}

/// Opens the browser and clicks the first factory preset's card, then closes the browser.
fn recall_first(
    h: &mut egui_kittest::Harness<'static, soundcraft_ui_egui::SoundApp>,
    plugin: &str,
    t: soundcraft_model::TrackId,
    m: soundcraft_model::ModuleId,
) -> &'static str {
    let info = soundcraft_dsp::plugin_info(plugin).unwrap();
    let first = soundcraft_ui_egui::fs_presets::factory_groups(info)[0].1[0].0;
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "Presets"})).unwrap();
    h.run_steps(4);
    let card = h.get_by_label(&format!("Preset card {first}")).rect().center();
    click(h, card);
    let close = h.get_by_label("Close").rect().center();
    click(h, close);
    h.run_steps(3);
    first
}

#[test]
fn the_dirty_star_is_any_change_since_the_last_load() {
    let (mut h, t, m) = open("fs_pitch", vec2(1800.0, 1000.0), "dark");
    let first = recall_first(&mut h, "fs_pitch", t, m);
    assert_eq!(preset_of(&h, t, m), first);
    assert!(!dirty(&h, t, m), "a load is clean");
    // A preset parameter moved, then moved back: still dirty, as the VST's `markPresetDirty`.
    let was = raw(&h, t, m, "dry");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "dry", "value": if was > 0.5 { 0.1 } else { 0.9 }})).unwrap();
    h.run_steps(2);
    assert!(dirty(&h, t, m));
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "dry", "value": was})).unwrap();
    h.run_steps(2);
    assert!(dirty(&h, t, m), "moved back is still a change");
    // Loading it again clears it.
    recall_first(&mut h, "fs_pitch", t, m);
    assert!(!dirty(&h, t, m));
    // A parameter outside the presets (Pitch's level) leaves it clean.
    let level = raw(&h, t, m, "level");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "level", "value": level * 0.5 + 0.1})).unwrap();
    h.run_steps(2);
    assert!(!dirty(&h, t, m), "level is not a preset parameter");
}

#[test]
fn a_browser_recall_pushes_shales_own_undo() {
    let (mut h, t, m) = open("fs_shale", vec2(1800.0, 1000.0), "dark");
    let before = h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().params.clone();
    recall_first(&mut h, "fs_shale", t, m);
    let after = h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().params.clone();
    assert_ne!(before, after, "the preset changed something");
    let undo = h.get_by_label("Undo").rect().center();
    click(&mut h, undo);
    h.run_steps(2);
    for (k, v) in &before {
        assert!((raw(&h, t, m, k) - v).abs() < 1e-5, "{k} back by the editor's Undo");
    }
}

#[test]
fn more_panels_are_saved_with_the_module() {
    // Swale reopens at `max (640, requiredHeight())` with More shown, as its constructor sizes it.
    for (plugin, closed, open_h, reopened) in [("fs_shale", 640.0, 896.0, 896.0), ("fs_swale", 640.0, 739.0, 640.0)] {
        let (mut h, t, m) = open(plugin, vec2(1800.0, 1000.0), "dark");
        assert_eq!(frame(&h, t, m).height(), closed, "{plugin}");
        let steps = h.state().engine.undo_history().len();
        let more = h.get_by_label("More").rect().center();
        click(&mut h, more);
        h.run_steps(2);
        assert_eq!(frame(&h, t, m).height(), open_h, "{plugin}");
        assert_eq!(h.state().engine.undo_history().len(), steps, "outside the undo history");
        let ins = h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().clone();
        let ui_state: serde_json::Value = serde_json::from_slice(&ins.state_bytes().unwrap()).unwrap();
        assert_eq!(ui_state["more"], json!(true), "{plugin}: `uiState`");
        // The session carries it: a fresh app on the same session opens with More.
        let session = (*h.state().engine.session()).clone();
        let mut engine = soundcraft_engine::Engine::default();
        engine.replace_session(session);
        let mut app = soundcraft_ui_egui::SoundApp::new(engine, None, soundcraft_ui_egui::Services::default());
        app.ui.drawers.insert(t, m);
        let mut h2 = harness(app, vec2(1800.0, 1000.0), "dark");
        h2.run_steps(4);
        assert_eq!(frame(&h2, t, m).height(), reopened, "{plugin} reopened");
        assert_eq!(h2.get_by_label("More").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True), "{plugin}: More shown");
    }
}

#[test]
fn chambermade_steps_back_from_init_to_the_last_preset() {
    let (mut h, t, m) = open("fs_chamber", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // ◀: `resized()` puts it 179 in from the header's left (pad 14), the header 16..56.
    click(&mut h, pos2(f.left() + 14.0 + 179.0 + 21.0, f.top() + 16.0 + 20.0));
    let info = soundcraft_dsp::plugin_info("fs_chamber").unwrap();
    let last = soundcraft_ui_egui::fs_presets::factory_groups(info).last().unwrap().1.last().unwrap().0;
    assert_eq!(preset_of(&h, t, m), last);
}

#[test]
fn a_new_session_does_not_inherit_the_dirty_flags() {
    let (mut h, t, m) = open("fs_pitch", vec2(1800.0, 1000.0), "dark");
    recall_first(&mut h, "fs_pitch", t, m);
    // Moved and back: dirty (the VST's flag), though the values equal the preset's.
    let was = raw(&h, t, m, "dry");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "dry", "value": if was > 0.5 { 0.1 } else { 0.9 }})).unwrap();
    h.run_steps(2);
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "dry", "value": was})).unwrap();
    h.run_steps(2);
    assert!(dirty(&h, t, m));
    // The same session opened again: same ids, same preset name, values equal to the preset's.
    let session = (*h.state().engine.session()).clone();
    h.state_mut().engine.replace_session(session);
    h.state_mut().ui.drawers.insert(t, m);
    h.run_steps(4);
    assert!(!dirty(&h, t, m), "the old session's flag is gone");
}

#[test]
fn chambermade_and_shale_save_whatever_the_box_says_silently() {
    for plugin in ["fs_chamber", "fs_shale"] {
        let dir = std::env::temp_dir().join(format!("sc-editor-quiet-save-{plugin}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = app_with_tracks(&["Pad", "Bass"]);
        app.preset_dir = Some(dir.clone());
        let (t, m) = add(&mut app, "Pad", plugin);
        app.ui.drawers.insert(t, m);
        let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
        let info = soundcraft_dsp::plugin_info(plugin).unwrap();
        let factory = soundcraft_ui_egui::fs_presets::factory_groups(info)[0].1[0].0;
        let folder = dir.join(soundcraft_ui_egui::preset_folder_name(plugin));
        // An empty name ("Preset", `sanitiseFileName`), then a factory name: both are written.
        for (typed, file) in [("", "Preset"), (factory, factory)] {
            h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "Presets"})).unwrap();
            h.run_steps(4);
            let name_box =
                h.query_all_by_role(egui::accesskit::Role::TextInput).map(|n| n.rect()).max_by(|a, b| a.min.y.total_cmp(&b.min.y)).unwrap().center();
            click(&mut h, name_box);
            key(&mut h, egui::Key::A, Modifiers::COMMAND);
            key(&mut h, egui::Key::Backspace, Modifiers::NONE);
            if !typed.is_empty() {
                type_text(&mut h, typed);
            }
            let save = h.get_by_label("Save").rect().center();
            click(&mut h, save);
            h.run_steps(2);
            assert!(folder.join(format!("{file}.json")).exists(), "{plugin}: saved as {file}");
            assert_eq!(preset_of(&h, t, m), file, "{plugin}");
            let close = h.get_by_label("Close").rect().center();
            click(&mut h, close);
            h.run_steps(2);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
