//! Options › Recording › Wait for Note: Record pressed with the transport stopped waits, and the
//! take starts at the first note played on an armed MIDI or instrument track's input, that note
//! as its first event (placed at the take's start). Record or Stop again stops waiting.
//!
//! The listening buffer Capture MIDI keeps (`SoundApp::midi_capture`) is where the note is seen:
//! it is filled every frame while not recording, so nothing new crosses to the audio thread.

use crate::SoundApp;
use soundcraft_model::{Route, Session};
use soundcraft_playback::midi_in::{RecordedMidi, TYPING_NAME, TYPING_SLOT};

/// The session's switch (`options.wait_for_note`).
pub const FLAG: &str = "options.wait_for_note";

/// The armed MIDI and instrument tracks on a MIDI input: their port (`None`: all) and channel
/// (0: all).
fn listeners(s: &Session) -> Vec<(Option<String>, u8)> {
    s.tracks
        .iter()
        .filter(|t| t.mixer.record_arm && t.kind.is_midi() && !t.inactive)
        .filter_map(|t| match &t.mixer.input {
            Route::Midi(m) => Some((m.device.clone(), m.channel)),
            _ => None,
        })
        .collect()
}

/// Record pressed: with Wait for Note on, the transport stopped and an armed track on a MIDI
/// input, starts waiting (true) instead of recording.
pub(crate) fn begin(app: &mut SoundApp) -> bool {
    let s = app.engine.session();
    if !s.edit.flag(FLAG) || app.is_playing() || listeners(s).is_empty() {
        return false;
    }
    app.wait_note = Some(app.midi_capture.len());
    app.ui.status = "Waiting for a note: the take starts when you play (Record or Stop stops waiting)".into();
    true
}

/// Stops waiting (Record again, Stop); false when it was not waiting.
pub(crate) fn cancel(app: &mut SoundApp) -> bool {
    if app.wait_note.take().is_none() {
        return false;
    }
    app.ui.status = "Stopped waiting for a note: nothing recorded".into();
    true
}

/// The name of the port in `slot`.
fn port_name(app: &SoundApp, slot: u8) -> Option<String> {
    if usize::from(slot) == TYPING_SLOT {
        return Some(TYPING_NAME.to_string());
    }
    app.midi_inputs.as_ref()?.ports().into_iter().find(|p| p.slot == usize::from(slot)).map(|p| p.name)
}

/// Each frame while not recording (after the listening buffer is collected): the first note-on an
/// armed track hears starts the take, with that note in it.
pub(crate) fn tick(app: &mut SoundApp) {
    let Some(from) = app.wait_note else { return };
    let from = from.min(app.midi_capture.len());
    let ears = listeners(app.engine.session());
    let hit = app.midi_capture.get(from..).unwrap_or_default().iter().find(|e| {
        let on = e.msg[0] & 0xF0 == 0x90 && e.msg[2] > 0;
        on && ears.iter().any(|(port, ch)| {
            (*ch == 0 || (e.msg[0] & 0x0F) + 1 == *ch) && port.as_deref().is_none_or(|p| port_name(app, e.port).as_deref() == Some(p))
        })
    });
    let Some(first) = hit.cloned() else {
        app.wait_note = Some(app.midi_capture.len());
        return;
    };
    app.wait_note = None;
    app.start_recording(false);
    if app.engine.transport.recording {
        // The note that started it begins the take (heard before the transport rolled).
        app.midi_rec.insert(0, RecordedMidi { at: app.record_start, pass: 0, ..first });
        app.ui.status = "Recording: started at the first note".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_first_note_played_starts_the_take_and_is_in_it() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": t, "input": "midi:Musical Typing"})).unwrap();
        app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
        assert!(!begin(&mut app), "off by default: Record records at once");
        app.run("options.wait_for_note", json!({"value": true})).unwrap();
        assert!(begin(&mut app));
        let ev =
            |msg: [u8; 3]| RecordedMidi { at: 9_000_000, port: TYPING_SLOT as u8, msg, pass: soundcraft_playback::midi_in::LISTENING, sysex: None };
        // A controller is not a note: still waiting.
        app.midi_capture.push(ev([0xB0, 64, 127]));
        tick(&mut app);
        assert!(!app.engine.transport.recording && app.wait_note.is_some());
        app.midi_capture.push(ev([0x90, 62, 90]));
        tick(&mut app);
        assert!(app.engine.transport.recording, "the note started the take");
        assert!(app.wait_note.is_none());
        assert_eq!(app.midi_rec.first().map(|e| (e.at, e.msg)), Some((app.record_start, [0x90, 62, 90])), "the note is the take's first event");
        app.stop_play();
        assert!(!app.engine.transport.recording);
    }

    #[test]
    fn record_again_stops_waiting_and_other_ports_do_not_start_it() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": t, "input": "midi:Musical Typing"})).unwrap();
        app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
        app.run("options.wait_for_note", json!({"value": true})).unwrap();
        assert!(begin(&mut app));
        app.midi_capture.push(RecordedMidi { at: 1, port: 3, msg: [0x90, 60, 100], pass: 0, sysex: None });
        tick(&mut app);
        assert!(!app.engine.transport.recording, "a port the track does not listen to");
        assert!(cancel(&mut app));
        assert!(!cancel(&mut app), "no longer waiting");
    }
}
