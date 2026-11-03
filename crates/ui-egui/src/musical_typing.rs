//! Musical Typing: Window › MIDI Keyboard's on-screen keys and the computer keyboard as a live
//! MIDI input, the port "Musical Typing" (`midi_in::TYPING_NAME`). It plays and records through
//! the same path as a hardware port: armed or input-monitored instrument tracks whose input is
//! MIDI: All Inputs or MIDI: Musical Typing hear it, and a take records it.
//!
//! Computer keys (while "Computer keys" is on, Ctrl+Alt+K / Cmd+Alt+K): A S D F G H J K L ; are
//! the white keys from C, W E T Y U O P the black keys; Z / X an octave down / up; C / V velocity
//! down / up. While it is on, those letter keys don't run their shortcuts (Space and the rest
//! still do). The window's Play / Step switch: Play sends the notes live; Step enters a note
//! into the selected MIDI clip at the insertion point and moves on (as before).

use crate::SoundApp;
use egui::Key;
use serde_json::json;

/// The computer keys that play, as (key, semitones above the octave's C).
pub const NOTE_KEYS: [(Key, u8); 17] = [
    (Key::A, 0),
    (Key::W, 1),
    (Key::S, 2),
    (Key::E, 3),
    (Key::D, 4),
    (Key::F, 5),
    (Key::T, 6),
    (Key::G, 7),
    (Key::Y, 8),
    (Key::H, 9),
    (Key::U, 10),
    (Key::J, 11),
    (Key::K, 12),
    (Key::O, 13),
    (Key::L, 14),
    (Key::P, 15),
    (Key::Semicolon, 16),
];

/// Musical Typing's state (not saved).
#[derive(Debug, Clone)]
pub struct TypingState {
    /// The computer keys play (and don't run their shortcuts).
    pub capture: bool,
    /// Step entry instead of live play.
    pub step: bool,
    /// The note the A key plays (a C).
    pub base: u8,
    pub velocity: u8,
    /// Computer keys held down and the notes they started (an octave change mid-hold still
    /// lets the right note go).
    pub held: Vec<(Key, u8)>,
    /// The on-screen key held by the pointer.
    pub mouse: Option<u8>,
}

impl Default for TypingState {
    fn default() -> Self {
        TypingState { capture: false, step: false, base: 60, velocity: 100, held: Vec::new(), mouse: None }
    }
}

/// The note a computer key plays at `base`, if it is a note key.
pub fn note_of(key: Key, base: u8) -> Option<u8> {
    NOTE_KEYS.iter().find(|(k, _)| *k == key).and_then(|(_, d)| base.checked_add(*d)).filter(|n| *n < 128)
}

/// Whether Musical Typing takes `key` before the shortcuts: plain note, octave and velocity
/// keys while computer keys play.
pub fn owns_key(app: &SoundApp, key: Key, m: egui::Modifiers) -> bool {
    app.typing.capture && !(m.command || m.ctrl || m.alt) && (note_of(key, 0).is_some() || matches!(key, Key::Z | Key::X | Key::C | Key::V))
}

/// Sends a live message on the Musical Typing port (nothing where there is no MIDI hub).
fn send(app: &SoundApp, msg: [u8; 3]) {
    if let Some(h) = &app.midi_hub {
        h.typing().send(&msg);
    }
}

/// Plays `note` (Play mode: live; Step mode: into the selected clip).
pub fn press(app: &mut SoundApp, note: u8) {
    if app.typing.step {
        step_enter(app, note);
    } else {
        send(app, [0x90, note & 0x7F, app.typing.velocity.clamp(1, 127)]);
    }
}

/// Lets `note` go (Play mode).
pub fn release(app: &mut SoundApp, note: u8) {
    if !app.typing.step {
        send(app, [0x80, note & 0x7F, 0]);
    }
}

/// Lets every held note go (capture turned off, the window closed).
pub fn release_all(app: &mut SoundApp) {
    let notes: Vec<u8> = app.typing.held.drain(..).map(|(_, n)| n).chain(app.typing.mouse.take()).collect();
    for n in notes {
        release(app, n);
    }
}

/// Step input: the note at the insertion point (relative to the selected MIDI clip), an eighth
/// long, then the insertion point moves on.
pub fn step_enter(app: &mut SoundApp, note: u8) {
    let Some(cid) = crate::midi_editor::target_clip(app) else { return };
    let s = app.engine.session();
    let (clip_start, at) = (s.find_clip(cid).map_or(0, |(_, c)| c.start), s.edit.selection.start);
    let rel = s.tempo.samples_to_ticks(at, s.sample_rate) - s.tempo.samples_to_ticks(clip_start, s.sample_rate);
    let len = soundcraft_time::TICKS_PER_QUARTER / 2;
    let _ = app.run("midi.note_add", json!({"clip": cid.0, "pitch": note, "start_ticks": rel.max(0), "length_ticks": len}));
    let s = app.engine.session();
    let next = s.tempo.tick_to_samples(s.tempo.samples_to_ticks(at, s.sample_rate) + len, s.sample_rate);
    let _ = app.run("transport.locate", json!({"at": next}));
}

/// Turns computer-key capture on or off (`ui.musical_typing`).
pub fn set_capture(app: &mut SoundApp, on: bool) {
    if app.typing.capture && !on {
        let keys: Vec<u8> = app.typing.held.drain(..).map(|(_, n)| n).collect();
        for n in keys {
            release(app, n);
        }
    }
    app.typing.capture = on;
}

/// Each frame, before the shortcuts: the computer keys' presses and releases while capture is
/// on (not while a text field has the keyboard).
pub fn handle_keys(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.typing.capture || ctx.egui_wants_keyboard_input() {
        return;
    }
    let events: Vec<(Key, bool, bool, egui::Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed, repeat, modifiers, .. } => Some((*key, *pressed, *repeat, *modifiers)),
                _ => None,
            })
            .collect()
    });
    for (key, pressed, repeat, m) in events {
        if !pressed {
            if let Some(i) = app.typing.held.iter().position(|(k, _)| *k == key) {
                let (_, n) = app.typing.held.remove(i);
                release(app, n);
            }
            continue;
        }
        if repeat || !owns_key(app, key, m) {
            continue;
        }
        match key {
            Key::Z => app.typing.base = app.typing.base.saturating_sub(12),
            Key::X => app.typing.base = app.typing.base.saturating_add(12).min(108),
            Key::C => app.typing.velocity = app.typing.velocity.saturating_sub(16).max(1),
            Key::V => app.typing.velocity = app.typing.velocity.saturating_add(16).min(127),
            _ => {
                if let Some(n) = note_of(key, app.typing.base)
                    && !app.typing.held.iter().any(|(k, _)| *k == key)
                {
                    app.typing.held.push((key, n));
                    press(app, n);
                }
            }
        }
    }
}

/// The notes sounding from Musical Typing now (for the keyboard's highlight).
pub fn sounding(app: &SoundApp) -> Vec<u8> {
    app.typing.held.iter().map(|(_, n)| *n).chain(app.typing.mouse).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_mix::LiveMidi;
    use soundcraft_playback::midi_in::TYPING_SLOT;

    fn key(k: Key, pressed: bool) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    fn heard(app: &SoundApp) -> Vec<[u8; 3]> {
        let hub = app.midi_hub.clone().unwrap();
        let mut live = LiveMidi::new();
        hub.drain_into(&mut live, hub.now_us(), 256, 48_000.0);
        assert!(live.events().iter().all(|e| usize::from(e.port) == TYPING_SLOT));
        live.events().iter().map(|e| e.msg).collect()
    }

    fn run(app: &mut SoundApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let raw = egui::RawInput { events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| handle_keys(app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn computer_keys_play_notes_octaves_and_velocity() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let ctx = egui::Context::default();
        // Off: the keys are the shortcuts'.
        run(&mut app, &ctx, vec![key(Key::A, true), key(Key::A, false)]);
        assert!(heard(&app).is_empty());
        assert!(!owns_key(&app, Key::A, egui::Modifiers::NONE));
        set_capture(&mut app, true);
        assert!(owns_key(&app, Key::A, egui::Modifiers::NONE) && !owns_key(&app, Key::Space, egui::Modifiers::NONE));
        assert!(!owns_key(&app, Key::A, egui::Modifiers::COMMAND), "Ctrl+A stays Select All");
        run(&mut app, &ctx, vec![key(Key::A, true), key(Key::W, true)]);
        assert_eq!(heard(&app), vec![[0x90, 60, 100], [0x90, 61, 100]]);
        // An octave up and softer while A is held: A's release lets the note it started go.
        run(&mut app, &ctx, vec![key(Key::X, true), key(Key::C, true), key(Key::A, false), key(Key::Semicolon, true)]);
        assert_eq!(heard(&app), vec![[0x80, 60, 0], [0x90, 88, 84]]);
        // Capture off lets the held notes go.
        set_capture(&mut app, false);
        assert_eq!(heard(&app), vec![[0x80, 61, 0], [0x80, 88, 0]]);
    }

    #[test]
    fn step_mode_enters_notes_into_the_selected_clip() {
        let mut engine = soundcraft_engine::Engine::default();
        let t = engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let clip = engine.execute("midi.new_clip", &json!({"track": t, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        engine.execute("edit.select", &json!({"clips": [clip]})).unwrap();
        let mut app = SoundApp::new(engine, None, crate::Services::default());
        app.typing.step = true;
        press(&mut app, 64);
        press(&mut app, 67);
        let notes: Vec<u8> = app
            .engine
            .session()
            .find_clip(soundcraft_model::ClipId(clip))
            .and_then(|(_, c)| c.midi_playing().map(|m| m.notes.iter().map(|n| n.pitch).collect()))
            .unwrap_or_default();
        assert_eq!(notes, vec![64, 67]);
        assert!(heard(&app).is_empty(), "step entry plays nothing live");
    }
}
