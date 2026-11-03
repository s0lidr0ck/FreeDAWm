use egui_kittest::Harness;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn a_long_plugin_window_stays_on_a_short_screen() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    let key = soundcraft_mix::SlotKey::Module(app.engine.session().track(kick).unwrap().mixer.inserts[0].id);
    app.ui.plugin_windows = vec![(kick, key)];
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 500.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let window = h.ctx.memory(|m| m.area_rect(egui::Id::new(("plugin", kick.0, key)))).unwrap();
    assert!(window.bottom() <= 500.0, "{window:?}");
}

#[test]
fn a_plugin_window_follows_its_module_when_the_chain_is_reordered() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    let comp = app.engine.session().track(kick).unwrap().mixer.inserts[1].clone();
    app.ui.plugin_windows = vec![(kick, soundcraft_mix::SlotKey::Module(comp.id))];
    app.engine.execute("mix.module_move", &serde_json::json!({"track": kick.0, "module": comp.id.0, "to": 0})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let name = soundcraft_dsp::plugin_info(&comp.plugin).unwrap().name;
    use egui_kittest::kittest::Queryable;
    assert!(h.query_by_label_contains(&format!("Kick · 1 · {name}")).is_some(), "the window shows the module at its new place");
    assert_eq!(h.state().ui.plugin_windows.len(), 1);
}

/// The generic window of a FireStation engine offers what its drawer does: not the engine's own
/// bypass, CleanQ's ceiling (drawn or learned since B3), and Filter's Tempo Source only once
/// something has set it off Host (so it can be put back).
#[test]
fn a_firestation_window_offers_what_its_drawer_does() {
    use egui_kittest::kittest::Queryable;
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    let mut keys = Vec::new();
    for plugin in ["fs_cleanq", "fs_filter"] {
        app.engine.execute("mix.module_add", &serde_json::json!({"track": kick.0, "plugin": plugin})).unwrap();
        let m = app.engine.session().track(kick).unwrap().mixer.inserts.iter().find(|i| i.plugin == plugin).unwrap().id;
        keys.push(m);
        app.ui.plugin_windows.push((kick, soundcraft_mix::SlotKey::Module(m)));
    }
    let mut h = Harness::builder().with_size(egui::vec2(1800.0, 1400.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let labels = |h: &Harness<SoundApp>, name: &str| h.query_all_by_label(name).count();
    let cq = soundcraft_dsp::plugin_info("fs_cleanq").unwrap();
    let filter = soundcraft_dsp::plugin_info("fs_filter").unwrap();
    let name = |info: &soundcraft_dsp::PluginInfo, id: &str| info.param(id).unwrap().name;
    assert!(labels(&h, name(cq, "outGain")) >= 1, "CleanQ's output is offered");
    assert!(labels(&h, name(cq, "ceilOn")) >= 1, "the ceiling is open (B3)");
    assert!(labels(&h, name(cq, "ceilThr")) >= 1);
    assert_eq!(labels(&h, name(filter, "tempoSrc")), 0, "Host: hidden");
    let bpm = labels(&h, name(filter, "bpm"));
    h.state_mut()
        .engine
        .execute("mix.module_param", &serde_json::json!({"track": kick.0, "module": keys[1].0, "param": "tempoSrc", "value": 1.0}))
        .unwrap();
    h.run_steps(4);
    assert_eq!(labels(&h, name(filter, "tempoSrc")), 1, "Manual: shown, so it can be put back");
    assert_eq!(labels(&h, name(filter, "bpm")), bpm + 1);
}
