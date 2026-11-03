//! The drawer: a built-in module opened sideways in the Mix window, with every control.

use egui::{Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::module_drawer::{DRAWER_W, GR_HISTORY_LEN, GR_TICK, GrHistory, KNOB_BELOW_LABEL, feed_gr_history, prune_drawers};
use soundcraft_ui_egui::{Services, SoundApp};
use std::collections::HashMap;

fn module(app: &SoundApp, track: &str, plugin: &str) -> (TrackId, ModuleId) {
    let t = app.engine.session().track_by_name(track).unwrap();
    (t.id, t.mixer.inserts.iter().find(|i| i.plugin == plugin).unwrap().id)
}

fn add(app: &mut SoundApp, track: &str, plugin: &str) -> (TrackId, ModuleId) {
    app.engine.execute("mix.module_add", &json!({"track": track, "plugin": plugin})).unwrap();
    module(app, track, plugin)
}

#[test]
fn drawer_follows_its_module() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let (kick, comp) = add(&mut app, "Kick", "compressor");
    let (snare, eq) = add(&mut app, "Snare", "eq_7band");
    let mut drawers = HashMap::from([(kick, comp), (snare, eq)]);
    prune_drawers(&mut drawers, app.engine.session());
    assert_eq!(drawers.len(), 2, "open drawers on live modules stay");

    // Removed by a command: its drawer goes, the other stays.
    app.engine.execute("mix.module_remove", &json!({"track": kick.0, "module": comp.0})).unwrap();
    prune_drawers(&mut drawers, app.engine.session());
    assert_eq!(drawers, HashMap::from([(snare, eq)]));

    // Undo brings it back; undoing its addition removes it again.
    app.engine.undo();
    let mut drawers = HashMap::from([(kick, comp), (snare, eq)]);
    prune_drawers(&mut drawers, app.engine.session());
    assert_eq!(drawers.len(), 2);
    app.engine.undo(); // the EQ on Snare
    prune_drawers(&mut drawers, app.engine.session());
    assert_eq!(drawers, HashMap::from([(kick, comp)]));

    // A deleted track takes its drawer with it.
    app.engine.execute("track.delete", &json!({"tracks": [kick.0]})).unwrap();
    prune_drawers(&mut drawers, app.engine.session());
    assert!(drawers.is_empty());
}

#[test]
fn gr_history_is_capped() {
    let mut h = GrHistory::default();
    for i in 0..1000 {
        feed_gr_history(&mut h, f64::from(i) * GR_TICK, i as f32);
    }
    assert_eq!(GR_HISTORY_LEN, 240);
    assert_eq!(h.values.len(), 240);
    assert_eq!(h.values.back().copied(), Some(999.0), "newest last");
    assert_eq!(h.values.front().copied(), Some(760.0));
    // Nonsense readings are stored as no reduction.
    feed_gr_history(&mut h, 1000.0 * GR_TICK, f32::NAN);
    assert_eq!(h.values.back().copied(), Some(0.0));
    assert_eq!(h.values.len(), 240);
}

#[test]
fn gr_history_spans_four_seconds_at_any_frame_rate() {
    assert!((GR_HISTORY_LEN as f64 * GR_TICK - 4.0).abs() < 1e-9);
    for fps in [30.0, 60.0, 144.0, 240.0] {
        let mut h = GrHistory::default();
        // Two seconds of frames: half the history, whatever the frame rate.
        let frames = (2.0 * fps) as usize;
        for i in 0..=frames {
            feed_gr_history(&mut h, i as f64 / fps, 3.0);
        }
        let n = h.values.len();
        assert!((119..=122).contains(&n), "{fps} fps: {n} points for 2 s");
    }
    // A long pause (window hidden) is not back-filled point by point.
    let mut h = GrHistory::default();
    feed_gr_history(&mut h, 0.0, 1.0);
    feed_gr_history(&mut h, 3600.0, 2.0);
    assert!(h.values.len() <= GR_HISTORY_LEN);
    assert_eq!(h.values, [2.0], "the stale points go");
}

#[test]
fn the_drawer_command_opens_and_folds() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let (kick, comp) = add(&mut app, "Kick", "compressor");
    app.run("ui.mix.drawer", json!({"track": kick.0, "module": comp.0})).unwrap();
    assert_eq!(app.ui.drawers.get(&kick), Some(&comp));
    // By name, and a second module on the same strip replaces the first.
    let (_, eq) = add(&mut app, "Kick", "eq_7band");
    app.run("ui.mix.drawer", json!({"track": "Kick", "module": eq.0})).unwrap();
    assert_eq!(app.ui.drawers.get(&kick), Some(&eq));
    // A plugin name picks its first module.
    app.run("ui.mix.drawer", json!({"track": "Kick", "module": "Comp"})).unwrap();
    assert_eq!(app.ui.drawers.get(&kick), Some(&comp));
    assert!(app.run("ui.mix.drawer", json!({"track": "Kick", "module": "reverb"})).is_err());
    // No module folds it.
    app.run("ui.mix.drawer", json!({"track": "Kick"})).unwrap();
    assert!(!app.ui.drawers.contains_key(&kick));
    // Bad input is an error, never a stray drawer.
    assert!(app.run("ui.mix.drawer", json!({"track": "Nope", "module": 1})).is_err());
    assert!(app.run("ui.mix.drawer", json!({"track": kick.0, "module": 999_999})).is_err());
    assert!(app.run("ui.mix.drawer", json!({})).is_err());
    assert!(app.ui.drawers.is_empty());
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

#[test]
fn the_drawer_sits_right_of_its_strip_with_every_parameter() {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    engine.execute("track.new", &json!({"name": "Gtr"})).unwrap();
    let mut app = SoundApp::new(engine, None, Services::default());
    let (vox, comp) = add(&mut app, "Vox", "compressor");
    app.run("window.mix", json!({})).unwrap();
    let mut h = mix(app);
    let gtr_before = h.get_by_label("Gtr strip").rect();
    h.state_mut().run("ui.mix.drawer", json!({"track": vox.0, "module": comp.0})).unwrap();
    h.run_steps(4);
    let gtr_after = h.get_by_label("Gtr strip").rect();
    assert!((gtr_after.min.x - gtr_before.min.x - (DRAWER_W + 1.0)).abs() < 2.0, "the next strip shifts right by the drawer");

    let shown = texts(&h);
    let info = soundcraft_dsp::plugin_info("compressor").unwrap();
    for p in info.params {
        assert!(shown.iter().any(|(t, _)| t.eq_ignore_ascii_case(p.name)), "{} not shown", p.name);
    }
    assert!(shown.iter().any(|(t, _)| t == "VOX · COMP"), "header names track and module");

    // Fold closes it.
    h.get_by_label("Fold the drawer").click();
    h.run_steps(3);
    assert!(h.state().ui.drawers.is_empty());
}

#[test]
fn a_drawer_knob_drag_is_one_undo_step() {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    let mut app = SoundApp::new(engine, None, Services::default());
    let (vox, comp) = add(&mut app, "Vox", "compressor");
    app.run("window.mix", json!({})).unwrap();
    app.ui.drawers.insert(vox, comp);
    let mut h = mix(app);
    let undo_before = h.state().engine.undo_history().len();
    let label = texts(&h).into_iter().find(|(t, _)| t.eq_ignore_ascii_case("Attack")).unwrap().1;
    let knob = egui::pos2(label.center().x, label.max.y + KNOB_BELOW_LABEL);
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.event(egui::Event::PointerMoved(knob));
    h.run_steps(1);
    h.event(button(knob, true));
    h.run_steps(1);
    for dy in 1..=6 {
        h.event(egui::Event::PointerMoved(knob - vec2(0.0, dy as f32 * 8.0)));
        h.run_steps(1);
    }
    h.event(button(knob - vec2(0.0, 48.0), false));
    h.run_steps(3);
    let s = h.state().engine.session();
    let attack = s.track(vox).unwrap().mixer.module(comp).unwrap().params["attack"];
    assert!(attack > 10.0, "the drag turned it up ({attack})");
    assert_eq!(h.state().engine.undo_history().len(), undo_before + 1);
}

/// The Mix window never offers a built-in's generic window: its INSTRUMENT row (a built-in
/// instrument on a MIDI track) keeps its old menu; the Edit window's inserts list has the item.
#[test]
fn only_the_edit_list_opens_a_built_in_in_a_window() {
    let synth = soundcraft_dsp::plugins().iter().find(|p| p.is_instrument).unwrap();
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Seq", "kind": "midi"})).unwrap();
    engine.execute("mix.module_add", &json!({"track": "Seq", "plugin": synth.id})).unwrap();
    let mut app = SoundApp::new(engine, None, Services::default());
    app.run("view.mix_section", json!({"section": "instruments"})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    let mut h = mix(app);
    h.get_by_label("Insert A").click_secondary();
    h.run_steps(3);
    assert!(h.query_by_label("Bypass").is_some(), "the row's menu is open");
    assert!(h.query_by_label("Open in Window").is_none(), "no generic window from the mixer");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);

    h.state_mut().run("view.edit_inserts_ae", json!({})).unwrap();
    h.state_mut().run("window.edit", json!({})).unwrap();
    h.run_steps(4);
    h.get_by_label(&format!("{} insert", synth.short_name)).click_secondary();
    h.run_steps(3);
    h.get_by_label("Open in Window").click();
    h.run_steps(3);
    let seq = h.state().engine.session().track_by_name("Seq").unwrap().clone();
    assert_eq!(h.state().ui.plugin_windows, vec![(seq.id, soundcraft_mix::SlotKey::Module(seq.mixer.inserts[0].id))]);
}

/// CleanQ's Ceiling tab (B3): Learn, Draw and Clear act through `mix.module_ceiling` (undoable),
/// Draw starts the ceiling on the capture (never a flat line), the nodes are dragged on the graph
/// as one undo step, and the dynamic ceiling switches on.
#[test]
fn the_cleanq_ceiling_tab_learns_draws_and_clears() {
    use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling_state::CeilingState;
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    let mut app = SoundApp::new(engine, None, Services::default());
    let (vox, cq) = add(&mut app, "Vox", "fs_cleanq");
    // The narrow drawer's Ceiling tab (the spike's wide editor is behind `ui.wide_editors`).
    app.ui.wide_editors = false;
    app.run("window.mix", json!({})).unwrap();
    app.ui.drawers.insert(vox, cq);
    let mut h = mix(app);
    let shape = |h: &Harness<SoundApp>| {
        let ins = h.state().engine.session().track(vox).unwrap().mixer.module(cq).unwrap().clone();
        ins.state_bytes().and_then(|b| CeilingState::read(&b)).unwrap_or_default().shape
    };
    assert!(h.query_by_label("Draw").is_none(), "the buttons live on the Ceiling tab");
    h.get_by_label("Ceiling").click();
    h.run_steps(3);
    assert!(texts(&h).iter().any(|(t, _)| t == "No ceiling yet"));

    // Draw with nothing captured: nothing to start from, and it says so (no flat line).
    h.get_by_label("Draw").click();
    h.run_steps(3);
    assert!(shape(&h).pts.is_empty());
    assert!(h.state().ui.status.starts_with("Nothing captured yet"), "{}", h.state().ui.status);

    // Draw while learning: the 16 nodes on the capture, as Auto Generate lays them.
    let ring = soundcraft_dsp::analysis::AnalysisRing::new();
    let mut seed = 3u32;
    let noise: Vec<f32> = (0..8192)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            0.05 * ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0) + 0.3 * (std::f32::consts::TAU * 700.0 * i as f32 / 48_000.0).sin()
        })
        .collect();
    let mut learn = soundcraft_ui_egui::cleanq_ceiling::Learn::default();
    for k in 0..8 {
        ring.push_slice(&noise);
        learn.feed(&ring, f64::from(k), 48_000.0, 0.0, 3);
    }
    let on_capture = soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling::auto_ceiling(learn.trace());
    assert!(on_capture.is_valid());
    h.state_mut().ui.cq_learn.insert((vox, cq), learn);
    h.run_steps(2);
    h.get_by_label("Draw").click();
    h.run_steps(3);
    assert_eq!(shape(&h), on_capture);
    assert_eq!(h.state().engine.undo_label(), Some("Set CleanQ Ceiling"));
    h.state_mut().ui.cq_learn.remove(&(vox, cq));
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Ceiling node 1").count(), 1);
    assert!(h.query_by_label("Ceiling node 16").is_some());

    // Dragging node 6 down moves it and the ones after it, in one undo step.
    let steps = h.state().engine.undo_history().len();
    let before = shape(&h);
    let node = h.get_by_label("Ceiling node 6").rect().center();
    let press = |h: &mut Harness<SoundApp>, pos: egui::Pos2, pressed: Option<bool>| {
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        if let Some(pressed) = pressed {
            let button = egui::PointerButton::Primary;
            h.input_mut().events.push(egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE });
        }
        h.step();
    };
    press(&mut h, node, None);
    press(&mut h, node, Some(true));
    for k in 1..=6 {
        press(&mut h, node + vec2(0.0, 5.0 * k as f32), None);
    }
    press(&mut h, node + vec2(0.0, 30.0), Some(false));
    h.run_steps(3);
    let c = shape(&h);
    assert!(c.pts[5].db < before.pts[5].db - 1.0 && c.pts[4].db == before.pts[4].db, "{c:?}");
    assert!(c.pts[15].db < before.pts[15].db - 1.0 || c.pts[15].db <= -96.0, "{c:?}");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "one drag, one undo step");

    // Learn with nothing playing: Done says so and leaves the ceiling as it was.
    h.get_by_label("Learn").click();
    h.run_steps(3);
    assert!(texts(&h).iter().any(|(t, _)| t.starts_with("Learning")));
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.status, "Nothing captured yet: play something first.");
    assert_eq!(shape(&h), c);

    // The dynamic ceiling switches on from its tab now.
    h.get_by_label("Ceiling Dynamic").click();
    h.run_steps(3);
    let on = h.state().engine.session().track(vox).unwrap().mixer.module(cq).unwrap().params.get("ceilOn").copied().unwrap_or(0.0);
    assert_eq!(on, 1.0);

    // Clear, and undo brings the drawn ceiling back.
    h.get_by_label("Clear").click();
    h.run_steps(3);
    assert!(shape(&h).pts.is_empty());
    assert!(h.query_by_label("Ceiling node 1").is_none());
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(shape(&h), c);
}
