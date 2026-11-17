//! Event › More MIDI › Step Input: notes entered one step at a time from a MIDI keyboard (any
//! input the target track listens to) with the transport stopped, at the insertion point on the
//! MIDI editor's clip (or the selected MIDI or instrument track): each key, or chord (keys held
//! together, entered when the last is let go), is a note of the chosen length, then the insertion
//! point moves on. Rest moves on without a note; Tie lengthens the notes just entered by a step.
//! Everything goes through `event.step_input` (one undo step per entry). The keyboard is heard
//! as usual; what it plays reaches here through Capture MIDI's listening clock
//! (`midi_in::LISTENING`), so only while the transport is stopped.

use crate::SoundApp;
use serde_json::json;
use soundcraft_model::{Route, TrackId};
use soundcraft_playback::midi_in::{LISTENING, RecordedMidi, TYPING_SLOT};

/// The step lengths offered, as `event.step_input` takes them.
pub const LENGTHS: [&str; 9] = ["1/1", "1/2", "1/4", "1/8", "1/16", "1/32", "1/4t", "1/8t", "1/16t"];

/// Step Input's window and the chord being played.
#[derive(Debug, Clone)]
pub struct StepInput {
    pub open: bool,
    /// Keys from MIDI inputs enter notes.
    pub on: bool,
    pub length: &'static str,
    /// Each note at this velocity; `None`: as played.
    pub velocity: Option<u8>,
    /// The chord held now: its pitches and the loudest velocity.
    chord: Vec<u8>,
    loudest: u8,
    held: usize,
}

impl Default for StepInput {
    fn default() -> Self {
        StepInput { open: false, on: true, length: "1/8", velocity: None, chord: Vec::new(), loudest: 0, held: 0 }
    }
}

/// The track steps go on: the MIDI editor's clip's, else the first selected MIDI or instrument
/// track.
pub fn target(app: &SoundApp) -> Option<TrackId> {
    if let Some(t) = crate::midi_editor::target_clip(app).and_then(|c| app.engine.session().find_clip(c).map(|(t, _)| t)) {
        return Some(t);
    }
    let s = app.engine.session();
    s.tracks.iter().find(|t| t.kind.is_midi() && s.edit.selected_tracks.contains(&t.id)).map(|t| t.id)
}

/// What the inputs played while the transport was stopped (from `midi_devices::collect_capture`):
/// keys and chords become steps while Step Input is open and on.
pub fn feed(app: &mut SoundApp, heard: &[RecordedMidi]) {
    if !app.step.open || !app.step.on || app.is_playing() || app.engine.transport.recording {
        return;
    }
    let Some(track) = target(app) else { return };
    // The track's own input: its port and channel.
    let input = match app.engine.session().track(track).map(|t| &t.mixer.input) {
        Some(Route::Midi(m)) => m.clone(),
        _ => soundcraft_model::MidiIn::all(),
    };
    let events = crate::midi_devices::recorded_events(app, heard);
    for (e, raw) in events.iter().zip(heard) {
        // Musical Typing's own step mode enters its keys itself.
        if raw.pass != LISTENING || (app.typing.step && usize::from(raw.port) == TYPING_SLOT) {
            continue;
        }
        if input.device.as_ref().is_some_and(|d| *d != e.port) || (input.channel != 0 && (e.msg[0] & 0x0F) + 1 != input.channel) {
            continue;
        }
        let Some(msg) = input.transform.apply(e.msg) else { continue };
        match msg[0] & 0xF0 {
            0x90 if msg[2] > 0 => {
                let s = &mut app.step;
                if !s.chord.contains(&msg[1]) && s.chord.len() < 16 {
                    s.chord.push(msg[1]);
                }
                s.loudest = s.loudest.max(msg[2]);
                s.held = s.held.saturating_add(1);
            }
            0x80 | 0x90 => {
                app.step.held = app.step.held.saturating_sub(1);
                if app.step.held == 0 && !app.step.chord.is_empty() {
                    let pitches = std::mem::take(&mut app.step.chord);
                    let velocity = app.step.velocity.unwrap_or(app.step.loudest).clamp(1, 127);
                    app.step.loudest = 0;
                    enter(app, track, json!({"pitches": pitches, "velocity": velocity}));
                }
            }
            _ => {}
        }
    }
}

/// One step on `track` from the insertion point (`extra`: the notes, a rest or a tie), then the
/// insertion point moves on.
fn enter(app: &mut SoundApp, track: TrackId, mut extra: serde_json::Value) {
    if let Some(o) = extra.as_object_mut() {
        o.insert("track".into(), json!(track.0));
        o.insert("duration".into(), json!(app.step.length));
    }
    match app.run("event.step_input", extra) {
        Ok(r) => {
            if let Some(next) = r.get("next").and_then(serde_json::Value::as_i64) {
                let _ = app.run("transport.locate", json!({"at": next}));
            }
        }
        Err(e) => app.ui.status = e,
    }
}

/// Event › More MIDI › Step Input… (and the MIDI editor's Step button): opens or closes the
/// window.
pub fn toggle(app: &mut SoundApp) {
    app.step.open = !app.step.open;
    app.step.chord.clear();
    app.step.held = 0;
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.step.open {
        return;
    }
    let mut open = true;
    let target = target(app);
    let name = target.and_then(|t| app.engine.session().track(t).map(|t| t.name.clone()));
    egui::Window::new("Step Input").open(&mut open).default_width(300.0).resizable(false).show(ctx, |ui| {
        ui.checkbox(&mut app.step.on, "Enter notes from MIDI keyboards")
            .on_hover_text("With the transport stopped, each key or chord played is a step; a chord is entered when its last key is let go");
        ui.horizontal_wrapped(|ui| {
            ui.label("Length");
            for l in LENGTHS {
                if ui.selectable_label(app.step.length == l, l).clicked() {
                    app.step.length = l;
                }
            }
        });
        ui.horizontal(|ui| {
            let mut fixed = app.step.velocity.is_some();
            if ui.checkbox(&mut fixed, "Fixed velocity").changed() {
                app.step.velocity = fixed.then_some(100);
            }
            if let Some(v) = &mut app.step.velocity {
                ui.add(egui::DragValue::new(v).range(1..=127));
            } else {
                ui.label(egui::RichText::new("as played").small());
            }
        });
        ui.horizontal(|ui| {
            let can = target.is_some();
            if ui.add_enabled(can, egui::Button::new("Rest")).on_hover_text("Move on a step without a note").clicked()
                && let Some(t) = target
            {
                enter(app, t, json!({"rest": true}));
            }
            if ui.add_enabled(can, egui::Button::new("Tie")).on_hover_text("Lengthen the notes just entered by a step").clicked()
                && let Some(t) = target
            {
                enter(app, t, json!({"tie": true}));
            }
        });
        let dim = crate::theme::Tokens::current().text_dim;
        let where_ = match name {
            Some(n) => format!("On {n}, from the insertion point"),
            None => "Select a MIDI or instrument track, or open a clip in the MIDI editor".into(),
        };
        ui.label(egui::RichText::new(where_).small().color(dim));
        if app.is_playing() {
            ui.label(egui::RichText::new("Stop the transport to enter steps").small().color(dim));
        }
    });
    if !open {
        toggle(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_chords_from_a_midi_keyboard_become_steps_with_rests_and_ties() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.engine.session_mut().edit.selected_tracks = vec![TrackId(t)];
        app.run("window.step_input", json!({})).unwrap();
        assert!(app.step.open);
        app.step.length = "1/4";
        let key = |msg: [u8; 3]| RecordedMidi { at: 0, port: TYPING_SLOT as u8, msg, pass: LISTENING, sysex: None };
        // A key, then a chord (entered when its last key is let go), then a rest and a tie.
        feed(&mut app, &[key([0x90, 60, 80]), key([0x80, 60, 0])]);
        feed(&mut app, &[key([0x90, 64, 70]), key([0x90, 67, 100]), key([0x80, 64, 0])]);
        let notes = |app: &SoundApp| -> Vec<(u8, u8, i64, i64)> {
            let tr = app.engine.session().track(TrackId(t)).unwrap();
            let mut v: Vec<_> = tr
                .clips()
                .iter()
                .flat_map(|c| c.midi_playing().unwrap().notes.iter().map(|n| (n.pitch, n.velocity, n.start, n.length)).collect::<Vec<_>>())
                .collect();
            v.sort();
            v
        };
        assert_eq!(notes(&app).len(), 1, "the chord waits for its last key");
        feed(&mut app, &[key([0x80, 67, 0])]);
        let q = soundcraft_time::TICKS_PER_QUARTER;
        assert_eq!(notes(&app), vec![(60, 80, 0, q), (64, 100, q, q), (67, 100, q, q)]);
        enter(&mut app, TrackId(t), json!({"tie": true}));
        enter(&mut app, TrackId(t), json!({"rest": true}));
        feed(&mut app, &[key([0x90, 72, 90]), key([0x80, 72, 0])]);
        assert_eq!(notes(&app), vec![(60, 80, 0, q), (64, 100, q, 2 * q), (67, 100, q, 2 * q), (72, 90, 4 * q, q)]);
        // Played while the transport rolls (Capture MIDI's, not the listening clock): no step.
        feed(&mut app, &[RecordedMidi { pass: 0, ..key([0x90, 50, 90]) }, RecordedMidi { pass: 0, ..key([0x80, 50, 0]) }]);
        assert_eq!(notes(&app).len(), 4);
    }
}
