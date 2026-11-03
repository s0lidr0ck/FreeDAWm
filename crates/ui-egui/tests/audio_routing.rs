//! Named inputs and outputs in the route menus and I/O Setup, the three-state I button, and the
//! low-latency monitoring (LLM) toggle.

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ChannelFormat, InputMonitor, InputPath, OutputPath, Session};
use soundcraft_ui_egui::mix_window::{input_choices, input_monitor_tip, output_choices};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn click(h: &mut Harness<'_, SoundApp>, pos: Pos2) {
    let m = Modifiers::NONE;
    h.state_mut().synthetic.extend([
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: m },
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: m },
    ]);
    for _ in 0..6 {
        step(h);
    }
}

fn click_label(h: &mut Harness<'_, SoundApp>, label: &str) {
    let at = h.get_by_label(label).rect().center();
    click(h, at);
}

fn app_with_vox() -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name": "Vox", "format": "mono"})).unwrap();
    app
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
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
    h
}

fn input(name: &str, first_channel: u16, format: ChannelFormat) -> InputPath {
    InputPath { name: name.into(), first_channel, format }
}

fn names(v: &[(String, bool)]) -> Vec<(&str, bool)> {
    v.iter().map(|(n, a)| (n.as_str(), *a)).collect()
}

#[test]
fn input_menu_lists_named_inputs_and_marks_missing_ones() {
    let mut s = Session::default();
    // No named inputs: the legacy "In N" names, one per device input (8 when it isn't open).
    assert_eq!(names(&input_choices(&s, Some(2))), [("In 1", true), ("In 2", true)]);
    let eight = input_choices(&s, None);
    assert_eq!(eight.len(), 8);
    assert_eq!(eight[7], ("In 8".to_string(), true));
    assert_eq!(input_choices(&s, Some(0)).len(), 8, "no device inputs reads as unknown");
    assert_eq!(input_choices(&s, Some(5000)).len(), 64, "bounded");

    s.inputs = vec![input("Vox", 0, ChannelFormat::Mono), input("Keys", 1, ChannelFormat::Stereo), input("X32 17", 16, ChannelFormat::Mono)];
    assert_eq!(names(&input_choices(&s, Some(2))), [("Vox", true), ("Keys", false), ("X32 17", false)]);
    assert_eq!(names(&input_choices(&s, Some(32))), [("Vox", true), ("Keys", true), ("X32 17", true)]);
    assert!(input_choices(&s, None).iter().all(|(_, a)| *a), "an unopened device marks nothing");
}

#[test]
fn output_choices_list_main_and_named_outputs() {
    let mut s = Session::default();
    s.outputs.push(OutputPath { name: "Mon 5-6".into(), first_channel: 4, format: ChannelFormat::Stereo });
    let main = s.outputs[0].name.clone();
    assert_eq!(names(&output_choices(&s, Some(2))), [(main.as_str(), true), ("Mon 5-6", false)]);
    assert_eq!(names(&output_choices(&s, Some(8))), [(main.as_str(), true), ("Mon 5-6", true)]);
    assert!(output_choices(&s, None).iter().all(|(_, a)| *a));
    // A main moved past the device is missing too (on its first channel it folds down instead).
    s.outputs[0].first_channel = 2;
    assert!(!output_choices(&s, Some(2))[0].1);
    s.outputs.clear();
    assert_eq!(names(&output_choices(&s, Some(2))), [("Out 1-2", true)], "a session with no paths still has the main");
}

#[test]
fn i_button_cycles_auto_on_off() {
    let monitor = |h: &Harness<'_, SoundApp>| h.state().engine.session().track_by_name("Vox").unwrap().mixer.input_monitor;
    for window in [MainWindow::Mix, MainWindow::Edit] {
        let mut app = app_with_vox();
        app.ui.window = window;
        let mut h = harness(app);
        assert_eq!(monitor(&h), InputMonitor::Auto);
        for next in [InputMonitor::On, InputMonitor::Off, InputMonitor::Auto] {
            let shown = monitor(&h);
            // The key's label names its state, so Auto and Off read differently.
            click_label(&mut h, input_monitor_tip(shown));
            assert_eq!(monitor(&h), next, "{window:?}: {shown:?} -> {next:?}");
            assert!(h.query_by_label(input_monitor_tip(next)).is_some(), "{window:?} shows {next:?}");
        }
    }
    assert_ne!(input_monitor_tip(InputMonitor::Auto), input_monitor_tip(InputMonitor::Off));
    assert_eq!(input_monitor_tip(InputMonitor::Off), "Input monitoring: Off (direct monitoring)");
    assert_eq!(input_monitor_tip(InputMonitor::Auto), "Input monitoring: Auto (armed tracks hear their input)");
}

#[test]
fn llm_toggle_adds_no_undo_step() {
    let mut h = harness(app_with_vox());
    let steps = h.state().engine.undo_history().len();
    let rev = h.state().engine.revision;
    let flag = |h: &Harness<'_, SoundApp>| h.state().engine.session().edit.flag("options.low_latency_monitoring");
    assert!(!flag(&h));
    click_label(&mut h, "Low-latency monitoring: bypass latency-adding plugins on live tracks");
    assert!(flag(&h), "the transport's LLM key turns it on");
    assert_eq!(h.state().engine.undo_history().len(), steps, "a monitoring mode, not an edit");
    // A new revision is what sends the session to the player.
    assert_ne!(h.state().engine.revision, rev);
    click_label(&mut h, "Low-latency monitoring: bypass latency-adding plugins on live tracks");
    assert!(!flag(&h));
    assert_eq!(h.state().engine.undo_history().len(), steps);
    // The Options menu shows the same switch checked.
    h.state_mut().run("options.low_latency_monitoring", json!({"value": true})).unwrap();
    let (_, checked) = soundcraft_ui_egui::menus::item_state(h.state(), "Options > Low Latency Monitoring", Some("options.low_latency_monitoring"));
    assert!(checked);
}

#[test]
fn io_setup_edits_go_through_setup_io() {
    let mut app = app_with_vox();
    app.run("setup.io", json!({"action": "create_input", "name": "Kick In", "format": "mono", "first_channel": 2})).unwrap();
    app.ui.show_io_setup = true;
    let mut h = harness(app);
    assert!(h.query_by_label("Inputs").is_some() && h.query_by_label("Outputs").is_some() && h.query_by_label("Busses").is_some());
    assert!(h.query_all_by_value("Kick In").next().is_some(), "the input's name is in its field");

    let steps = h.state().engine.undo_history().len();
    click_label(&mut h, "New Input");
    let inputs = |h: &Harness<'_, SoundApp>| h.state().engine.session().inputs.clone();
    assert_eq!(inputs(&h).len(), 2);
    assert_eq!(inputs(&h)[1].name, "Input 2");
    assert_eq!(inputs(&h)[1].first_channel, 3, "the next free channel");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1);

    // (Before the inputs fill the window: New Output then scrolls out of view.)
    click_label(&mut h, "New Output");
    let outputs = h.state().engine.session().outputs.clone();
    assert_eq!(outputs.len(), 2);
    assert_eq!((outputs[1].name.as_str(), outputs[1].first_channel), ("Out 3-4", 2));

    click_label(&mut h, "Add In 1–8");
    assert_eq!(inputs(&h).len(), 10);
    assert_eq!(h.state().engine.undo_history().len(), steps + 3, "one step");

    // Delete buttons: the first is the first input's.
    let del = h.query_all_by_label("Delete").next().unwrap().rect().center();
    click(&mut h, del);
    assert_eq!(inputs(&h).len(), 9);
    assert!(inputs(&h).iter().all(|i| i.name != "Kick In"));

    // The format and channel edits are setup.io actions too.
    h.state_mut().run("setup.io", json!({"action": "set_input_format", "name": "Input 2", "format": "stereo"})).unwrap();
    assert_eq!(inputs(&h)[0].format, ChannelFormat::Stereo);
    assert!(h.state_mut().run("setup.io", json!({"action": "set_input_format", "name": "Input 2", "format": "5.1"})).is_err());
    h.state_mut().run("setup.io", json!({"action": "set_output_format", "name": "Out 3-4", "format": "mono"})).unwrap();
    assert_eq!(h.state().engine.session().outputs[1].format, ChannelFormat::Mono);
    for _ in 0..3 {
        step(&mut h);
    }
}

#[test]
fn device_channels_are_unknown_without_devices() {
    let app = app_with_vox();
    assert_eq!(app.device_channels(), (None, None));
}
