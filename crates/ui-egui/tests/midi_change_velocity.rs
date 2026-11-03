//! Event › More MIDI › Change Velocity…: a dialog with set, add, scale and ramp, acting on the
//! notes selected in the MIDI Editor, else on the selected MIDI clips.

use serde_json::json;
use soundcraft_model::{ChannelFormat, ClipId, TrackKind};
use soundcraft_ui_egui::dialogs::{Dialog, VelocityDialog, VelocityMode, velocity_action};
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn app_with_clip() -> (SoundApp, ClipId) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    for (pitch, start) in [(60, 0), (64, 480), (67, 960)] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start})).unwrap();
    }
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    (app, ClipId(clip))
}

fn velocities(app: &mut SoundApp, clip: ClipId) -> Vec<u64> {
    let n = app.run("midi.notes", json!({"clip": clip.0})).unwrap();
    n["notes"].as_array().unwrap().iter().map(|x| x["velocity"].as_u64().unwrap()).collect()
}

fn run(app: &mut SoundApp, d: &VelocityDialog) {
    let (cmd, params) = velocity_action(app, d);
    app.run(&cmd, params).unwrap();
}

#[test]
fn the_menu_item_opens_the_dialog() {
    let (mut app, _) = app_with_clip();
    menus::invoke_menu(&mut app, "event.change_velocity", "Event > More MIDI > Change Velocity...");
    assert!(matches!(app.dialogs.open, Some(Dialog::ChangeVelocity(_))));
}

#[test]
fn it_acts_on_the_selected_clips_with_each_mode() {
    let (mut app, clip) = app_with_clip();
    run(&mut app, &VelocityDialog { mode: VelocityMode::Set, set: 80, ..Default::default() });
    assert_eq!(velocities(&mut app, clip), vec![80, 80, 80]);
    run(&mut app, &VelocityDialog { mode: VelocityMode::Add, add: -30, ..Default::default() });
    assert_eq!(velocities(&mut app, clip), vec![50, 50, 50]);
    run(&mut app, &VelocityDialog { mode: VelocityMode::Scale, scale: 200.0, ..Default::default() });
    assert_eq!(velocities(&mut app, clip), vec![100, 100, 100]);
    run(&mut app, &VelocityDialog { mode: VelocityMode::Ramp, from: 20, to: 120, ..Default::default() });
    assert_eq!(velocities(&mut app, clip), vec![20, 70, 120]);
}

#[test]
fn with_the_editor_open_it_acts_on_the_selected_notes_only() {
    let (mut app, clip) = app_with_clip();
    app.ui.show_midi_editor = true;
    app.midi.clip = Some(clip);
    app.midi.selected = vec![1, 2];
    run(&mut app, &VelocityDialog { mode: VelocityMode::Ramp, from: 10, to: 30, ..Default::default() });
    assert_eq!(velocities(&mut app, clip), vec![100, 10, 30]);
}
