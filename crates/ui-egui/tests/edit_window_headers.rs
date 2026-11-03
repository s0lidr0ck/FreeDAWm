use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_model::{Track, TrackId};
use soundcraft_ui_egui::edit_window::HEADER_W;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

const CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn press(h: &mut Harness<'_, SoundApp>, pos: Pos2, button: PointerButton, modifiers: Modifiers) {
    h.state_mut().synthetic.extend([
        Event::ModifiersChanged(modifiers),
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button, pressed: true, modifiers },
        Event::PointerButton { pos, button, pressed: false, modifiers },
        Event::ModifiersChanged(Modifiers::NONE),
    ]);
    for _ in 0..10 {
        step(h);
    }
}

fn click(h: &mut Harness<'_, SoundApp>, pos: Pos2) {
    press(h, pos, PointerButton::Primary, Modifiers::NONE);
}

fn kick(h: &Harness<'_, SoundApp>) -> Track {
    h.state().engine.session().track_by_name("Kick").cloned().unwrap()
}

fn insert(h: &Harness<'_, SoundApp>, slot: usize) -> Option<String> {
    kick(h).mixer.inserts.get(slot).cloned().map(|i| i.plugin)
}

fn bypassed(h: &Harness<'_, SoundApp>, slot: usize) -> bool {
    kick(h).mixer.inserts.get(slot).cloned().is_some_and(|i| i.bypass)
}

fn harness(track: &str, height: &str, view: &str) -> (Harness<'static, SoundApp>, TrackId, [f32; 4]) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run(view, json!({})).unwrap();
    app.run("track.height", json!({"tracks": [track], "height": height})).unwrap();
    let id = app.engine.session().track_by_name(track).unwrap().id;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
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
    let row = h.state().edit_layout.rows.iter().find(|(t, _)| *t == id.0).map(|(_, r)| *r).unwrap();
    (h, id, row)
}

#[test]
fn header_keys_freeze_silence_and_show_automation() {
    let (mut h, _, row) = harness("Kick", "medium", "view.edit_io");
    let at = |x: f32, y: f32| pos2(row[0] + x, row[1] + y);

    click(&mut h, at(23.5, 65.5));
    assert!(kick(&h).frozen);

    click(&mut h, at(46.5, 65.5));
    let off = h.get_by_label("off").rect().center();
    click(&mut h, off);
    assert!(kick(&h).inactive);

    // The lane key shows the automation lanes (volume when nothing is automated) under the
    // waveform, which stays.
    click(&mut h, at(7.5, row[3] - row[1] - 7.5));
    assert_eq!(kick(&h).auto_lanes, ["volume"]);
    assert_eq!(kick(&h).view, "waveform");

    click(&mut h, at(HEADER_W + 81.5, 29.5));
    assert_eq!(h.state().ui.window, MainWindow::Mix);
}

#[test]
fn edit_window_insert_list_opens_changes_and_adds_plugins() {
    let (mut h, kick, row) = harness("Kick", "medium", "view.edit_inserts_ae");
    let slot = |i: usize| pos2(row[0] + HEADER_W + 37.5, row[1] + 8.0 + i as f32 * 16.0);

    // A built-in opens in its drawer, in the Mix window.
    click(&mut h, slot(0));
    let first = h.state().engine.session().track(kick).unwrap().mixer.inserts[0].id;
    assert_eq!(h.state().ui.drawers.get(&kick), Some(&first));
    assert_eq!(h.state().ui.window, MainWindow::Mix);
    h.state_mut().ui.drawers.clear();
    h.state_mut().ui.window = MainWindow::Edit;
    for _ in 0..4 {
        step(&mut h);
    }

    press(&mut h, slot(0), PointerButton::Primary, CMD);
    assert!(bypassed(&h, 0));
    assert!(h.state().ui.drawers.is_empty());

    // Right-click ▸ Open in Window: the generic window, for built-ins.
    press(&mut h, slot(0), PointerButton::Secondary, Modifiers::NONE);
    let open = h.get_by_label("Open in Window").rect().center();
    click(&mut h, open);
    assert_eq!(h.state().ui.plugin_windows, vec![(kick, soundcraft_mix::SlotKey::Module(first))]);
    assert!(h.state().ui.drawers.is_empty());
    h.state_mut().ui.plugin_windows.clear();
    for _ in 0..4 {
        step(&mut h);
    }

    press(&mut h, slot(1), PointerButton::Secondary, Modifiers::NONE);
    let remove = h.get_by_label("Remove").rect().center();
    click(&mut h, remove);
    assert_eq!(insert(&h, 1), None);

    // The row after the last module adds one at the end.
    let plugin = soundcraft_dsp::listed_plugins().find(|p| !p.is_instrument && p.category == soundcraft_dsp::Category::Other).unwrap();
    click(&mut h, slot(1));
    let category = h.get_by_label("Utilities ⏵").rect().center();
    h.state_mut().synthetic.push(Event::PointerMoved(category));
    for _ in 0..10 {
        step(&mut h);
    }
    let pick = h.get_by_label(plugin.name).rect().center();
    click(&mut h, pick);
    assert_eq!(insert(&h, 1).as_deref(), Some(plugin.id));
    assert_eq!(insert(&h, 2), None);
}

#[test]
fn the_insert_list_shows_every_module_of_a_long_chain() {
    let (mut h, kick_id, row) = harness("Kick", "medium", "view.edit_inserts_ae");
    for _ in 0..18 {
        h.state_mut().engine.execute("mix.module_add", &json!({"track": kick_id.0, "plugin": "gain"})).unwrap();
    }
    h.state_mut().engine.execute("mix.module_add", &json!({"track": kick_id.0, "plugin": "compressor"})).unwrap();
    let n = kick_len(&h);
    assert!(n > 10, "past the old ten slots");
    for _ in 0..4 {
        step(&mut h);
    }
    // Every module is a row of the list, then the row that adds one.
    assert_eq!(h.query_all_by_label("Gain insert").count(), 18);
    // Gain is on Kick only: its lowest row stands for the end of the list.
    let add = |h: &Harness<'_, SoundApp>| h.query_all_by_label("Gain insert").map(|n| n.rect()).max_by(|a, b| a.min.y.total_cmp(&b.min.y)).unwrap();
    let before = add(&h);
    assert!(before.min.y > row[3], "the list is longer than the track");
    let list = pos2(row[0] + HEADER_W + 37.5, row[1] + 40.0);
    h.state_mut().synthetic.push(Event::PointerMoved(list));
    for _ in 0..3 {
        step(&mut h);
    }
    for _ in 0..20 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -200.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        step(&mut h);
    }
    for _ in 0..10 {
        step(&mut h);
    }
    // Scrolled down to its end: the last rows come into the track's view.
    let after = add(&h);
    assert!(after.min.y < before.min.y && after.max.y <= row[3] + 1.0, "{before:?} → {after:?}");
}

fn kick_len(h: &Harness<'_, SoundApp>) -> usize {
    kick(h).mixer.inserts.len()
}

#[test]
fn the_views_menu_stays_open_until_a_click_elsewhere() {
    let (mut h, _, _) = harness("Kick", "medium", "view.edit_inserts_ae");
    let views = h.get_by_label("Views").rect().center();
    click(&mut h, views);
    for column in ["Sends A-E", "I/O"] {
        let item = h.get_by_label(column).rect().center();
        click(&mut h, item);
    }
    let edit = &h.state().engine.session().edit;
    assert!(edit.flag("edit_view.sends_ae") && edit.flag("edit_view.io"));
    click(&mut h, pos2(800.0, 990.0));
    assert!(h.query_by_label("Sends A-E").is_none());
}

#[test]
fn the_io_columns_route_menus_stay_open_while_searching_buses() {
    let (mut h, _, row) = harness("Kick", "medium", "view.edit_io");
    click(&mut h, pos2(row[0] + HEADER_W + 40.0, row[1] + 29.5));
    let bus = h.get_by_label("Bus").rect();
    let search = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .map(|node| node.rect())
        .filter(|r| r.min.y >= bus.max.y)
        .min_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .unwrap()
        .center();
    click(&mut h, search);
    assert!(h.query_by_label("no output").is_some(), "a click in the bus search closed the Output menu");
}

#[test]
fn only_a_sends_fader_icon_opens_its_level() {
    let (mut h, snare, row) = harness("Snare", "medium", "view.edit_sends_ae");
    let send_a = |x: f32| pos2(row[0] + HEADER_W + x, row[1] + 8.5);

    // The right of the name, where the level used to start, picks the destination.
    click(&mut h, send_a(50.0));
    assert!(h.query_by_label("DESTINATION").is_some());
    assert_eq!(h.state().ui.send_window, None);
    click(&mut h, pos2(800.0, 990.0));

    click(&mut h, send_a(65.5));
    assert_eq!(h.state().ui.send_window, Some((snare, 0)));
    assert!(h.query_by_label("DESTINATION").is_none());
}

/// The Edit header's freeze key is the Mix strip's: Live / Partly / Whole, a tooltip naming the
/// freeze point, and a click on a partly frozen track unfreezes it.
#[test]
fn header_freeze_key_tells_the_mix_strips_story() {
    let (mut h, _, row) = harness("Kick", "medium", "view.edit_io");
    let gain = h.state_mut().run("mix.module_add", json!({"track": "Kick", "plugin": "gain"})).unwrap()["module"].as_u64().unwrap();
    h.state_mut().run("track.freeze", json!({"tracks": ["Kick"], "upto": gain})).unwrap();
    for _ in 0..4 {
        step(&mut h);
    }
    let key = pos2(row[0] + 23.5, row[1] + 65.5);
    assert!(kick(&h).frozen_audio().is_some(), "{}", h.state().ui.status);
    assert!(h.get_by_label("Kick freeze").accesskit_node().toggled() == Some(egui::accesskit::Toggled::True), "lit while frozen");
    h.state_mut().synthetic.push(Event::PointerMoved(key));
    for _ in 0..30 {
        step(&mut h);
    }
    assert!(h.query_by_label_contains("Frozen up to Gain").is_some(), "the tooltip names the freeze point");
    click(&mut h, key);
    assert!(kick(&h).frozen_audio().is_none(), "a click unfreezes a partly frozen track");
    assert_ne!(h.get_by_label("Kick freeze").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
}
