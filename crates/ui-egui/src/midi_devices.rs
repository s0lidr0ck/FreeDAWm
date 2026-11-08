//! Setup › MIDI Input Devices: the MIDI input ports (USB keyboards and controllers, an audio
//! interface's 5-pin MIDI In) with an Enable checkbox and a live activity light each, and MIDI
//! Thru. The choices are app settings (`ui.json`: `midi.inputs` by port name, `midi.thru`), not
//! the session's. The ports themselves are `soundcraft_playback::midi_in::MidiInputs`, rescanned
//! when the window opens and every 2 s.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::{Color32, Sense, vec2};
use serde_json::json;
use soundcraft_model::{MidiIn, Route, TrackId};
use std::time::Duration;

/// How long a port's light stays lit after a message.
const LIGHT: Duration = Duration::from_millis(150);

/// Once per frame (from `logic`): the system ports open once the host asked for them, the
/// player has the hub, the settings reach the ports, and the 2 s rescan runs.
pub fn tick(app: &mut SoundApp) {
    let Some(hub) = app.midi_hub.clone() else { return };
    hub.set_thru(app.ui.midi_thru);
    if app.midi_inputs.is_none() && app.use_system_midi {
        app.midi_inputs = soundcraft_playback::midi_in::MidiInputs::system(std::sync::Arc::clone(&hub), app.ui.midi_inputs.clone());
    }
    if !app.midi_sent
        && let Some(p) = &app.player
    {
        p.set_midi(std::sync::Arc::clone(&hub));
        app.midi_sent = true;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(m) = &mut app.midi_inputs {
        m.tick(std::time::Instant::now());
    }
}

/// Recording starts: what the audio thread heard before is not part of the take.
pub(crate) fn start_take(app: &mut SoundApp) {
    app.midi_rec.clear();
    if let Some(h) = &app.midi_hub {
        let mut stale = Vec::new();
        h.take_recorded(&mut stale);
    }
}

/// While recording (each frame): the live MIDI heard since the last frame.
pub(crate) fn collect(app: &mut SoundApp) {
    if let Some(h) = &app.midi_hub {
        h.take_recorded(&mut app.midi_rec);
    }
}

/// How much earlier a recorded event belongs than where it arrived: the device's output latency
/// and the mix's delay (see `soundcraft_engine::midi_record`; the player places each event from its
/// own age, so the block every live event waits is already off).
pub fn take_latency(app: &SoundApp) -> soundcraft_time::Samples {
    let l = app.reported_latency();
    i64::from(l.output) + i64::from(l.mix)
}

/// The port name of each hub slot (hardware ports as listed, and Musical Typing).
fn slot_names(app: &SoundApp) -> Vec<(usize, String)> {
    let mut v: Vec<(usize, String)> = app.midi_inputs.as_ref().map(|m| m.ports().into_iter().map(|p| (p.slot, p.name)).collect()).unwrap_or_default();
    v.push((soundcraft_playback::midi_in::TYPING_SLOT, soundcraft_playback::midi_in::TYPING_NAME.to_string()));
    v
}

/// What the audio thread heard, with each event's port by name.
pub(crate) fn recorded_events(
    app: &SoundApp,
    heard: &[soundcraft_playback::midi_in::RecordedMidi],
) -> Vec<soundcraft_engine::midi_record::RecordedEvent> {
    let names = slot_names(app);
    let name = |slot: u8| names.iter().find(|(s, _)| *s == usize::from(slot)).map(|(_, n)| n.clone()).unwrap_or_default();
    heard.iter().map(|r| soundcraft_engine::midi_record::RecordedEvent { at: r.at, port: name(r.port), msg: r.msg, pass: r.pass }).collect()
}

/// Recording stopped at `end`: the take's live MIDI becomes clips on the armed MIDI tracks
/// (`soundcraft_engine::midi_record`), placed with the take's start, loop and punch.
pub(crate) fn finish_take(
    app: &mut SoundApp,
    plan: &soundcraft_engine::io::Recording,
    end: soundcraft_time::Samples,
) -> Vec<soundcraft_model::ClipId> {
    collect(app);
    let heard = std::mem::take(&mut app.midi_rec);
    let events = recorded_events(app, &heard);
    let take = soundcraft_engine::midi_record::MidiTake {
        start: plan.start,
        end,
        looped: plan.pass.map(|p| soundcraft_time::Range::new(plan.start, plan.start.saturating_add(p))),
        punch: plan.punch,
        latency: take_latency(app),
    };
    // A remote recorder sends this take to the booth (`net_ui`), even an empty one.
    app.last_midi_take = Some((take, events.clone()));
    if events.is_empty() {
        return Vec::new();
    }
    soundcraft_engine::midi_record::record_live_midi(&mut app.engine, &take, &events)
}

/// The names of the input ports present now (for the track input menus).
pub fn present_ports(app: &SoundApp) -> Vec<String> {
    app.midi_inputs.as_ref().map(|m| m.present()).unwrap_or_default()
}

/// Enables or disables a port by name, and keeps the choice in the settings.
pub fn set_enabled(app: &mut SoundApp, name: &str, on: bool) {
    if let Some(m) = &mut app.midi_inputs {
        m.set_enabled(name, on);
        app.ui.midi_inputs = m.choices().clone();
    } else {
        app.ui.midi_inputs.insert(name.to_string(), on);
    }
}

/// The MIDI inputs a track's input menu offers: All Inputs, then each enabled port present now
/// (and the track's own port when it is unplugged or disabled, marked so), as `(label, device,
/// available)`; `device` is `None` for All Inputs.
pub fn input_choices(app: &SoundApp, current: Option<&MidiIn>) -> Vec<(String, Option<String>, bool)> {
    let mut out = vec![("MIDI: All Inputs".to_string(), None, true)];
    let ports: Vec<String> = app.midi_inputs.as_ref().map(|m| m.ports().into_iter().filter(|p| p.open).map(|p| p.name).collect()).unwrap_or_default();
    for p in &ports {
        out.push((format!("MIDI: {p}"), Some(p.clone()), true));
    }
    // Musical Typing's on-screen keys and the computer keys (Window › Musical Typing).
    let typing = soundcraft_playback::midi_in::TYPING_NAME.to_string();
    if app.midi_hub.is_some() {
        out.push((format!("MIDI: {typing}"), Some(typing.clone()), true));
    }
    // The remote recorders' ports (Setup › Network): `Remote: <PC> › <port>`.
    #[cfg(not(target_arch = "wasm32"))]
    let remote = app.net.as_ref().map(|n| n.hub.remote_ports()).unwrap_or_default();
    #[cfg(target_arch = "wasm32")]
    let remote: Vec<String> = Vec::new();
    for p in &remote {
        out.push((format!("MIDI: {p}"), Some(p.clone()), true));
    }
    if let Some(dev) = current.and_then(|m| m.device.as_ref())
        && !ports.contains(dev)
        && *dev != typing
        && !remote.contains(dev)
    {
        out.push((format!("MIDI: {dev} (not connected)"), Some(dev.clone()), false));
    }
    out
}

/// The input menu's MIDI section (instrument and MIDI tracks): the inputs, then a channel
/// submenu (Omni, 1-16) for the MIDI input the track has.
pub(crate) fn input_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    let Some(track) = app.engine.session().track(id) else { return };
    if !track.kind.is_midi() {
        return;
    }
    let current = match &track.mixer.input {
        Route::Midi(m) => Some(m.clone()),
        _ => None,
    };
    ui.separator();
    ui.strong("MIDI Input");
    let dim = Tokens::current().text_dim;
    for (label, device, available) in input_choices(app, current.as_ref()) {
        let picked = current.as_ref().is_some_and(|m| m.device == device);
        let text = if available { egui::RichText::new(label) } else { egui::RichText::new(label).color(dim) };
        if ui.add(egui::Button::selectable(picked, text)).clicked() {
            let input = device.map_or_else(|| "midi:all".to_string(), |d| format!("midi:{d}"));
            let channel = current.as_ref().map_or(0, |m| m.channel);
            if let Err(e) = app.run("track.input", json!({"track": id.0, "input": input, "channel": channel})) {
                app.ui.status = e;
            }
            ui.close();
        }
    }
    let Some(m) = current else { return };
    ui.menu_button(if m.channel == 0 { "Channel: Omni".to_string() } else { format!("Channel: {}", m.channel) }, |ui| {
        for ch in 0..=16u8 {
            let label = if ch == 0 { "Omni".to_string() } else { ch.to_string() };
            if ui.add(egui::Button::selectable(m.channel == ch, label)).clicked() {
                let input = m.device.as_ref().map_or_else(|| "midi:all".to_string(), |d| format!("midi:{d}"));
                if let Err(e) = app.run("track.input", json!({"track": id.0, "input": input, "channel": ch})) {
                    app.ui.status = e;
                }
                ui.close();
            }
        }
    });
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_inputs;
    if !open {
        app.midi_window_was_open = false;
        return;
    }
    if !std::mem::replace(&mut app.midi_window_was_open, true)
        && let Some(m) = &mut app.midi_inputs
    {
        m.rescan();
    }
    let t = Tokens::current();
    egui::Window::new("MIDI Input Devices").open(&mut open).default_size(vec2(440.0, 300.0)).resizable(true).show(ctx, |ui| {
        let ports = app.midi_inputs.as_ref().map(|m| m.ports()).unwrap_or_default();
        if app.midi_inputs.is_none() {
            ui.label(egui::RichText::new("MIDI input is not available here.").color(t.text_dim));
        } else if ports.is_empty() {
            ui.label("No MIDI input ports found.");
            ui.label(
                egui::RichText::new("Plug in a USB MIDI keyboard, or connect a cable to your audio interface's MIDI In; ports appear within 2 s.")
                    .small()
                    .color(t.text_dim),
            );
        }
        let mut toggles = Vec::new();
        egui::Grid::new("midi-inputs").num_columns(3).spacing(vec2(12.0, 6.0)).striped(true).show(ui, |ui| {
            for p in &ports {
                let mut on = p.enabled;
                if ui.checkbox(&mut on, "").on_hover_text("Enable this input").changed() {
                    toggles.push((p.name.clone(), on));
                }
                let lit = app.midi_inputs.as_ref().and_then(|m| m.since_activity(&p.name)).is_some_and(|d| d < LIGHT);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    let fill = if lit { Color32::from_rgb(90, 220, 110) } else { t.panel_bg };
                    ui.painter().circle(r.center(), 4.5, fill, egui::Stroke::new(1.0, t.border));
                    ui.label(&p.name);
                });
                let state = match (&p.error, p.present, p.open) {
                    (Some(err), _, _) => format!("Could not open: {err}"),
                    (None, false, _) => "Not connected".into(),
                    (None, true, true) => "Open".into(),
                    (None, true, false) => "Disabled".into(),
                };
                ui.label(egui::RichText::new(state).small().color(t.text_dim));
                ui.end_row();
            }
        });
        for (name, on) in toggles {
            set_enabled(app, &name, on);
        }
        ui.separator();
        ui.checkbox(&mut app.ui.midi_thru, "MIDI Thru")
            .on_hover_text("Record-enabled instrument and MIDI tracks play what comes in (input-monitored tracks always do)");
        ui.horizontal(|ui| {
            if ui.button("Rescan").clicked()
                && let Some(m) = &mut app.midi_inputs
            {
                m.rescan();
            }
            let dropped = app.midi_hub.as_ref().map_or(0, |h| h.dropped());
            if dropped > 0 {
                ui.label(egui::RichText::new(format!("{dropped} messages dropped")).small().color(t.text_dim));
            }
        });
    });
    app.ui.show_midi_inputs = open;
    // The activity lights.
    ctx.request_repaint_after(Duration::from_millis(50));
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_playback::midi_in::{FakeBackend, MidiInputs};

    pub(crate) fn with_fake(app: &mut SoundApp, ports: &[&str]) -> FakeBackend {
        let fake = FakeBackend::with_ports(ports);
        let hub = app.midi_hub.clone().unwrap();
        let mut m = MidiInputs::new(fake.boxed(), hub, app.ui.midi_inputs.clone());
        m.rescan();
        app.midi_inputs = Some(m);
        fake
    }

    #[test]
    fn the_window_enables_a_port_and_saves_the_choice() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let fake = with_fake(&mut app, &["USB Keys", "Focusrite USB MIDI"]);
        assert!(fake.is_open("USB Keys"), "a port never chosen is enabled");
        set_enabled(&mut app, "Focusrite USB MIDI", false);
        assert!(!fake.is_open("Focusrite USB MIDI"));
        assert_eq!(app.ui.midi_inputs.get("Focusrite USB MIDI"), Some(&false));
        // Saved by name in the settings, which a new list starts from.
        let json = serde_json::to_value(&app.ui).unwrap();
        assert_eq!(json["midi.inputs"]["Focusrite USB MIDI"], false);
        assert_eq!(json["midi.thru"], true);
        set_enabled(&mut app, "Focusrite USB MIDI", true);
        assert!(fake.is_open("Focusrite USB MIDI"));
        assert_eq!(present_ports(&app), vec!["USB Keys".to_string(), "Focusrite USB MIDI".to_string()]);
    }

    #[test]
    fn the_input_menu_offers_all_inputs_and_each_open_port() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        with_fake(&mut app, &["USB Keys", "Focusrite USB MIDI"]);
        set_enabled(&mut app, "Focusrite USB MIDI", false);
        let labels = |app: &SoundApp, cur: Option<&MidiIn>| input_choices(app, cur).into_iter().map(|c| (c.0, c.2)).collect::<Vec<_>>();
        assert_eq!(
            labels(&app, None),
            vec![("MIDI: All Inputs".to_string(), true), ("MIDI: USB Keys".to_string(), true), ("MIDI: Musical Typing".to_string(), true)]
        );
        // A track on a port that is gone still shows it, marked.
        let gone = MidiIn { device: Some("Old Keys".into()), channel: 0 };
        assert_eq!(labels(&app, Some(&gone)).last().cloned(), Some(("MIDI: Old Keys (not connected)".to_string(), false)));
    }

    #[test]
    fn a_take_heard_on_the_audio_thread_becomes_a_clip_on_the_armed_track() {
        use soundcraft_playback::midi_in::{RecordedMidi, TYPING_SLOT};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        with_fake(&mut app, &["USB Keys"]);
        let keys = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let typed = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": keys, "input": "midi:USB Keys"})).unwrap();
        app.run("track.input", json!({"track": typed, "input": "midi:Musical Typing"})).unwrap();
        app.run("mix.record_arm", json!({"tracks": [keys, typed], "value": true})).unwrap();
        // Stale events from before the take are dropped when it starts.
        app.midi_rec.push(RecordedMidi { at: 1, port: 0, msg: [0x90, 1, 1], pass: 0 });
        start_take(&mut app);
        assert!(app.midi_rec.is_empty());
        app.midi_rec = vec![
            RecordedMidi { at: 1_000, port: 0, msg: [0x90, 60, 100], pass: 0 },
            RecordedMidi { at: 9_000, port: 0, msg: [0x80, 60, 0], pass: 0 },
            RecordedMidi { at: 2_000, port: TYPING_SLOT as u8, msg: [0x90, 72, 100], pass: 0 },
        ];
        let plan = soundcraft_engine::io::Recording { start: 0, pass: None, punch: None, rate: 0, takes: Vec::new(), latency: 0 };
        let ids = finish_take(&mut app, &plan, 48_000);
        assert_eq!(ids.len(), 2);
        let pitch = |t: u64| {
            let tr = app.engine.session().track(soundcraft_model::TrackId(t)).unwrap();
            tr.clips().first().and_then(|c| c.midi_playing()).map(|m| m.notes[0].pitch)
        };
        assert_eq!((pitch(keys), pitch(typed)), (Some(60), Some(72)));
        assert!(app.midi_rec.is_empty());
    }

    #[test]
    fn menu_opens_the_window_and_it_draws() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        with_fake(&mut app, &["USB Keys"]);
        crate::menus::invoke_menu(&mut app, "window.midi_inputs", "Setup > MIDI Input Devices...");
        assert!(app.ui.show_midi_inputs);
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for i in 0..4 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 900.0))),
                time: Some(1.0 + f64::from(i) / 60.0),
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
            texts = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>();
        }
        for want in ["MIDI Input Devices", "USB Keys", "Open", "MIDI Thru"] {
            assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
        }
        assert!(app.ui.show_midi_inputs);
    }
}
