//! Snap to Scale in the MIDI Editor and the Event menu: it acts on the selected notes and the
//! selection stays on the same notes.

use serde_json::json;
use soundcraft_model::{ChannelFormat, ClipId, TrackKind};
use soundcraft_ui_egui::{Services, SoundApp, menus, midi_editor};

fn app_with_clip(notes: &[(u8, i64)]) -> (SoundApp, ClipId) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    for (pitch, start) in notes {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start})).unwrap();
    }
    app.engine.session_mut().key_signatures.push((0, "C major".into()));
    (app, ClipId(clip))
}

fn pitches(app: &mut SoundApp, clip: ClipId) -> Vec<u64> {
    let n = app.run("midi.notes", json!({"clip": clip.0})).unwrap();
    n["notes"].as_array().unwrap().iter().map(|x| x["pitch"].as_u64().unwrap()).collect()
}

#[test]
fn the_editor_snaps_the_selected_notes_and_keeps_the_selection() {
    let (mut app, clip) = app_with_clip(&[(61, 0), (63, 0), (66, 480)]);
    app.midi.selected = vec![1, 2];
    midi_editor::snap_selection(&mut app, clip);
    // C# is not selected; D# and F# snap down.
    assert_eq!(pitches(&mut app, clip), vec![61, 62, 65]);
    assert_eq!(app.midi.selected, vec![1, 2]);
}

#[test]
fn the_editor_snaps_every_note_when_none_is_selected() {
    let (mut app, clip) = app_with_clip(&[(61, 0), (63, 480)]);
    midi_editor::snap_selection(&mut app, clip);
    assert_eq!(pitches(&mut app, clip), vec![60, 62]);
}

#[test]
fn the_event_menu_item_snaps_the_selected_clips() {
    let (mut app, clip) = app_with_clip(&[(61, 0), (63, 480)]);
    app.engine.session_mut().edit.selected_clips = vec![clip];
    menus::invoke_menu(&mut app, "midi.snap_to_scale", "Event > Snap Notes to Scale");
    assert_eq!(pitches(&mut app, clip), vec![60, 62]);
}
