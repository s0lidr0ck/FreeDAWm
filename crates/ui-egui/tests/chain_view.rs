//! The channel chain in the Mix window: one tile per module, the "+" row, wide and slim strips,
//! drag and drop between strips, and the tile menu.

use egui::{Key, Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use soundcraft_model::{ModuleId, StripSize, TrackId};
use soundcraft_ui_egui::chain_view::{DropAction, drop_action};
use soundcraft_ui_egui::mix_window::strip_width;
use soundcraft_ui_egui::{ControlRequest, Services, SoundApp, control};

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

/// Sends one control-channel request and returns its result.
fn control(app: &mut SoundApp, tx: &std::sync::mpsc::Sender<ControlRequest>, method: &str, params: Value) -> Value {
    let (req, reply) = ControlRequest::new(method, params);
    tx.send(req).unwrap();
    control::drain(app, &egui::Context::default());
    let response = reply.try_recv().unwrap();
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}

fn kick(app: &SoundApp) -> soundcraft_model::Track {
    app.engine.session().track_by_name("Kick").cloned().unwrap()
}

fn chain(app: &SoundApp) -> Vec<String> {
    kick(app).mixer.inserts.iter().map(|i| i.plugin.clone()).collect()
}

fn texts(h: &Harness<SoundApp>) -> Vec<(String, egui::Rect)> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) => Some((t.galley.job.text.clone(), t.visual_bounding_rect())),
            _ => None,
        })
        .collect()
}

#[test]
fn chain_view_adds_moves_and_toggles_size() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default()).with_control(rx);
    let before = chain(&app);
    for plugin in ["compressor", "eq_7band"] {
        control(&mut app, &tx, "engine.execute", json!({"command": "mix.module_add", "params": {"track": "Kick", "plugin": plugin}}));
    }
    control(&mut app, &tx, "engine.execute", json!({"command": "mix.strip_size", "params": {"track": "Kick", "size": "slim"}}));
    control(&mut app, &tx, "ui.menu.invoke", json!({"id": "window.mix"}));
    let mut expected = before;
    expected.extend(["compressor".to_string(), "eq_7band".to_string()]);
    assert_eq!(chain(&app), expected, "chain order as added");
    assert_eq!(kick(&app).mixer.strip_size, StripSize::Slim);

    let h = harness(app);
    // Every module of the chain has a tile; the slim strip names them in short.
    let painted = texts(&h);
    let comp = soundcraft_dsp::plugin_info("compressor").unwrap().short_name.to_uppercase();
    assert!(painted.iter().filter(|(t, _)| *t == comp).count() >= 3, "two compressor tiles on Kick, one on Bass");
    // The slim strip is narrower than a wide one.
    assert_eq!(strip_width(StripSize::Slim, true), 80.0);
    assert_eq!(strip_width(StripSize::Wide, false), 150.0);
    assert_eq!(strip_width(StripSize::Auto, true), 150.0);
    assert_eq!(strip_width(StripSize::Auto, false), 80.0);

    let (a, b) = (TrackId(1), TrackId(2));
    let m = ModuleId(7);
    assert_eq!(drop_action((a, m), (a, 3), false), DropAction::Move { track: a, module: m, gap: 3 });
    assert_eq!(drop_action((a, m), (a, 0), true), DropAction::Move { track: a, module: m, gap: 0 });
    assert_eq!(drop_action((a, m), (b, 1), true), DropAction::Copy { from: a, module: m, to: b, at: 1 });
    assert_eq!(drop_action((a, m), (b, 2), false), DropAction::MoveAcross { from: a, module: m, to: b, at: 2 });
}

#[test]
fn drops_move_copy_and_move_across_in_one_undo_step() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let k = kick(&app);
    let bass = app.engine.session().track_by_name("Bass").cloned().unwrap();
    let (eq, comp) = (k.mixer.inserts[0].id, k.mixer.inserts[1].id);
    // Dropping the EQ after the compressor (gap 2) moves it to the end.
    soundcraft_ui_egui::chain_view::apply_drop(&mut app, drop_action((k.id, eq), (k.id, 2), false));
    assert_eq!(chain(&app), ["compressor", "eq_7band"]);
    let n_bass = bass.mixer.inserts.len();
    soundcraft_ui_egui::chain_view::apply_drop(&mut app, drop_action((k.id, comp), (bass.id, 0), true));
    assert_eq!(chain(&app), ["compressor", "eq_7band"], "Alt-drop copies");
    let bass_chain = |app: &SoundApp| app.engine.session().track(bass.id).unwrap().mixer.inserts.iter().map(|i| i.plugin.clone()).collect::<Vec<_>>();
    assert_eq!(bass_chain(&app).len(), n_bass + 1);
    assert_eq!(bass_chain(&app)[0], "compressor");
    app.engine
        .execute("automation.set_point", &json!({"track": k.id.0, "param": format!("plugin:m{}:mid_gain", eq.0), "at": 0, "value": 3.0}))
        .unwrap();
    let lanes = |app: &SoundApp, t: TrackId| -> Vec<String> {
        app.engine.session().track(t).unwrap().automation.iter().map(|l| l.param.id()).filter(|id| id.starts_with("plugin:")).collect()
    };
    let (kick_lanes, bass_lanes) = (lanes(&app, k.id), lanes(&app, bass.id));
    soundcraft_ui_egui::chain_view::apply_drop(&mut app, drop_action((k.id, eq), (bass.id, 1), false));
    assert_eq!(chain(&app), ["compressor"], "a plain drop on another strip moves");
    assert_eq!(bass_chain(&app)[1], "eq_7band");
    let moved = app.engine.session().track(bass.id).unwrap().mixer.inserts[1].id;
    assert!(lanes(&app, k.id).is_empty(), "the lane left Kick");
    assert!(lanes(&app, bass.id).contains(&format!("plugin:m{}:mid_gain", moved.0)), "and went with the module, re-keyed");
    assert!(app.engine.undo());
    assert_eq!(chain(&app), ["compressor", "eq_7band"], "the move across is one undo step");
    assert_eq!(bass_chain(&app).len(), n_bass + 1);
    assert_eq!((lanes(&app, k.id), lanes(&app, bass.id)), (kick_lanes, bass_lanes));
}

fn vox_mix() -> Harness<'static, SoundApp> {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    let mut app = SoundApp::new(engine, None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    harness(app)
}

fn vox_chain(h: &Harness<SoundApp>) -> Vec<soundcraft_model::Insert> {
    h.state().engine.session().tracks[0].mixer.inserts.clone()
}

#[test]
fn the_plus_row_adds_at_the_end_and_tiles_bypass_open_and_duplicate() {
    let mut h = vox_mix();
    h.get_by_label("Add module").click();
    h.run_steps(3);
    h.event(egui::Event::Text("gain".into()));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(4);
    let chain = vox_chain(&h);
    assert_eq!(chain.iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), ["gain"]);
    let vox = h.state().engine.session().tracks[0].id;
    let comp = chain[0].id;
    assert_eq!(h.state().ui.drawers.get(&vox), Some(&comp), "a new built-in opens its drawer");

    h.get_by_label("Add module").click();
    h.run_steps(3);
    h.event(egui::Event::Text("trim".into()));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(4);
    assert_eq!(vox_chain(&h).iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), ["gain", "trim"], "added at the end");

    // The LED bypasses.
    h.get_by_label("Bypass Gain").click();
    h.run_steps(3);
    assert!(vox_chain(&h)[0].bypass);

    // The title toggles the drawer.
    h.state_mut().ui.drawers.clear();
    h.get_by_label("Gain module").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.drawers.get(&vox), Some(&comp));
    h.get_by_label("Gain module").click();
    h.run_steps(3);
    assert!(!h.state().ui.drawers.contains_key(&vox));

    // Right-click ▸ Duplicate puts a copy right after it, Remove takes it out.
    h.get_by_label("Gain module").click_secondary();
    h.run_steps(3);
    h.get_by_label("Duplicate").click();
    h.run_steps(3);
    assert_eq!(vox_chain(&h).iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), ["gain", "gain", "trim"]);
    h.get_by_label("Trim module").click_secondary();
    h.run_steps(3);
    h.get_by_label("Remove").click();
    h.run_steps(3);
    assert_eq!(vox_chain(&h).iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), ["gain", "gain"]);
}

#[test]
fn the_strip_name_menu_sets_the_strip_size() {
    let mut h = vox_mix();
    h.get_by_label("Vox strip").click_secondary();
    h.run_steps(3);
    // The menu's Slim (the Mix view's Channel width bar has one too, drawn first).
    h.get_all_by_label("Slim").last().unwrap().click();
    h.run_steps(3);
    assert_eq!(h.state().engine.session().tracks[0].mixer.strip_size, StripSize::Slim);
}

#[test]
fn the_global_toggle_makes_auto_strips_slim() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    assert!(!app.ui.narrow_mix, "wide by default");
    app.run("view.mix_slim", json!({})).unwrap();
    assert!(app.ui.narrow_mix);
    let ui = serde_json::to_value(&app.ui).unwrap();
    let back: soundcraft_ui_egui::UiState = serde_json::from_value(ui).unwrap();
    assert!(back.narrow_mix);
}

#[test]
fn drawers_and_plugin_windows_survive_ui_set() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let k = kick(&app);
    let m = k.mixer.inserts[1].id;
    app.ui.drawers.insert(k.id, m);
    app.ui.plugin_windows.push((k.id, soundcraft_mix::SlotKey::Module(m)));
    app.run("ui.set", json!({"show_transport": true})).unwrap();
    assert_eq!(app.ui.drawers.get(&k.id), Some(&m));
    assert_eq!(app.ui.plugin_windows, vec![(k.id, soundcraft_mix::SlotKey::Module(m))]);
}

#[test]
fn a_wide_tile_spells_out_the_name_a_slim_one_shortens() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.execute("mix.module_add", &json!({"track": "Kick", "plugin": "fs_transient"})).unwrap();
    app.engine.execute("mix.strip_size", &json!({"track": "Kick", "size": "wide"})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    let mut h = harness(app);
    let painted = texts(&h);
    assert!(painted.iter().any(|(t, _)| t == "TRANSIENT"), "wide: the full name");
    assert!(!painted.iter().any(|(t, _)| t == "TRANSNT"));
    // Its controls keep the short name either way.
    assert!(h.query_by_label("Transnt module").is_some());
    // A name too long for the header beside the preset falls back to the short one.
    assert!(!painted.iter().any(|(t, _)| t == "COMPRESSOR/LIMITER"));

    h.state_mut().engine.execute("mix.strip_size", &json!({"track": "Kick", "size": "slim"})).unwrap();
    h.run_steps(4);
    let painted = texts(&h);
    assert!(painted.iter().any(|(t, _)| t == "TRANSNT"), "slim: the short name");
    assert!(!painted.iter().any(|(t, _)| t == "TRANSIENT"));
}
