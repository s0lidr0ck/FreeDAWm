//! The ⤢ "open full editor" button on every insert's tile (spec 2026-11-02 item 2) and plugin
//! windows that always open on screen (item 6).

use egui::{Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_playback::SlotKey;
use soundcraft_ui_egui::widgets::fit_on_screen;
use soundcraft_ui_egui::{Services, SoundApp};

const SCREEN: egui::Vec2 = vec2(1600.0, 1000.0);

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(SCREEN).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// The demo in the Mix window with `plugin` first on Kick.
fn with(plugin: &str) -> (SoundApp, TrackId, ModuleId) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let r = app.engine.execute("mix.module_add", &json!({"track": "Kick", "plugin": plugin, "at": 0})).unwrap();
    let m = ModuleId(u32::try_from(r["module"].as_u64().unwrap()).unwrap());
    let t = app.engine.session().track_by_name("Kick").unwrap().id;
    app.run("window.mix", json!({})).unwrap();
    (app, t, m)
}

fn click_at(h: &mut Harness<SoundApp>, at: Pos2) {
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.run_steps(1);
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(1);
    }
}

fn window_rect(h: &Harness<SoundApp>, t: TrackId, m: ModuleId) -> Option<Rect> {
    h.ctx.memory(|mem| mem.area_rect(egui::Id::new(("full-editor", t.0, m.0))))
}

#[test]
fn the_tile_button_opens_a_built_ins_full_editor_in_its_own_window() {
    let (app, t, m) = with("fs_cleanq");
    let mut h = harness(app);
    let at = h.get_by_label("CleanQ full editor").rect().center();
    click_at(&mut h, at);
    h.run_steps(6);
    assert_eq!(h.state().ui.full_editors, vec![(t, m)], "status: {}", h.state().ui.status);
    assert!(soundcraft_ui_egui::editor_kit::frame_rect(&h.ctx, t, m).is_some(), "the VST-faithful editor drew");
    let r = window_rect(&h, t, m).expect("its window");
    assert!(r.width() > 600.0, "full size, not the slim drawer: {r:?}");
    assert!(Rect::from_min_size(Pos2::ZERO, SCREEN).contains_rect(r), "on screen: {r:?}");
    // The drawers' slim setting is left as it was.
    assert!(!h.state().ui.slim_editors);
    // Closing it through the command.
    h.state_mut().run("ui.mix.full_editor", json!({"track": t.0, "module": m.0, "open": false})).unwrap();
    h.run_steps(2);
    assert!(h.state().ui.full_editors.is_empty());
}

#[test]
fn a_module_without_a_full_editor_opens_its_parameter_window_and_a_hosted_one_its_own() {
    let (mut app, t, m) = with("compressor");
    let r = app.run("ui.mix.full_editor", json!({"track": "Kick", "module": m.0})).unwrap();
    assert_eq!(r["window"], "parameters");
    assert!(app.ui.plugin_windows.contains(&(t, SlotKey::Module(m))));
    assert!(app.ui.full_editors.is_empty());
    // A hosted plugin (none installed here, no player): its editor, else its window.
    let hm = ModuleId(9_999);
    let mut ins = app.engine.session().track(t).unwrap().mixer.module(m).unwrap().clone();
    ins.id = hm;
    ins.plugin = "clap:test.full.editor".into();
    app.engine.session_mut().track_mut(t).unwrap().mixer.inserts.push(ins);
    let r = app.run("ui.mix.full_editor", json!({"track": "Kick", "module": hm.0})).unwrap();
    assert_eq!(r["window"], "hosted");
    assert!(app.ui.plugin_windows.contains(&(t, SlotKey::Module(hm))));
    assert!(app.run("ui.mix.full_editor", json!({"track": "Kick"})).is_err(), "a module is needed");
}

#[test]
fn every_tile_has_the_button() {
    let (mut app, _, _) = with("fs_comp");
    app.engine.execute("mix.module_add", &json!({"track": "Kick", "plugin": "chorus"})).unwrap();
    let h = harness(app);
    for name in ["Comp full editor", "Chorus full editor"] {
        assert!(h.query_all_by_label(name).next().is_some(), "no {name}");
    }
}

#[test]
fn an_editor_opened_near_the_right_edge_slides_onto_the_screen() {
    let (app, t, m) = with("fs_cleanq");
    let mut h = harness(app);
    // As if its ⤢ were clicked at the screen's bottom-right corner.
    let at = pos2(SCREEN.x - 5.0, SCREEN.y - 5.0);
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::new(("full-editor", t.0, m.0)).with("anchor"), at));
    h.state_mut().run("ui.mix.full_editor", json!({"track": t.0, "module": m.0})).unwrap();
    h.run_steps(6);
    let r = window_rect(&h, t, m).expect("its window");
    let screen = Rect::from_min_size(Pos2::ZERO, SCREEN);
    assert!(screen.contains_rect(r), "{r:?}");
    // Dragged (or remembered) off screen: back on screen next frame.
    let id = egui::Id::new(("full-editor", t.0, m.0));
    let title = pos2(r.center().x, r.min.y + 10.0);
    let to = pos2(SCREEN.x + 400.0, title.y);
    h.input_mut().events.push(egui::Event::PointerMoved(title));
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: title,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerMoved(to));
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(4);
    let r = h.ctx.memory(|mem| mem.area_rect(id)).unwrap();
    assert!(screen.contains_rect(r.shrink(0.5)), "{r:?}");
}

#[test]
fn fit_on_screen_slides_in_and_shrinks_what_is_too_big() {
    let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0));
    // Near the right and bottom edges: slides left and up, same size.
    let r = fit_on_screen(Rect::from_min_size(pos2(900.0, 700.0), vec2(300.0, 200.0)), screen);
    assert_eq!(r, Rect::from_min_size(pos2(700.0, 600.0), vec2(300.0, 200.0)));
    // Off the left/top (a monitor unplugged): back in.
    let r = fit_on_screen(Rect::from_min_size(pos2(-2000.0, -50.0), vec2(300.0, 200.0)), screen);
    assert_eq!(r.min, pos2(0.0, 0.0));
    // Bigger than the screen: the top-left stays, the size is the screen's.
    let r = fit_on_screen(Rect::from_min_size(pos2(100.0, 100.0), vec2(1400.0, 900.0)), screen);
    assert_eq!(r, screen);
    // A second monitor's area works the same.
    let right = Rect::from_min_size(pos2(1000.0, 0.0), vec2(800.0, 600.0));
    let r = fit_on_screen(Rect::from_min_size(pos2(1700.0, 100.0), vec2(300.0, 200.0)), right);
    assert_eq!(r.min, pos2(1500.0, 100.0));
    // Hostile numbers pass through untouched.
    let bad = Rect::from_min_size(pos2(f32::NAN, 0.0), vec2(10.0, 10.0));
    assert!(fit_on_screen(bad, screen).min.x.is_nan());
}
