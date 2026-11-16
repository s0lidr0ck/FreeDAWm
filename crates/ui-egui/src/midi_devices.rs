//! Setup › MIDI Input Devices: the MIDI input ports (USB keyboards and controllers, an audio
//! interface's 5-pin MIDI In) with an Enable checkbox and a live activity light each, the MIDI
//! output ports (`soundcraft_playback::midi_out`) with an Enable checkbox each, and MIDI
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
    hub.set_filter(filter_bits(app.engine.session()));
    if app.midi_inputs.is_none() && app.use_system_midi {
        app.midi_inputs = soundcraft_playback::midi_in::MidiInputs::system(std::sync::Arc::clone(&hub), app.ui.midi_inputs.clone());
    }
    if !app.midi_sent
        && let Some(p) = &app.player
    {
        p.set_midi(std::sync::Arc::clone(&hub));
        app.midi_sent = true;
    }
    if app.midi_outputs.is_none() && app.use_system_midi {
        app.midi_outputs = soundcraft_playback::midi_out::MidiOutputs::system(app.ui.midi_outputs.clone());
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(m) = &mut app.midi_inputs {
        m.tick(std::time::Instant::now());
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(m) = &mut app.midi_outputs {
        // The ports the session's tracks send to open even if never enabled in the window.
        m.set_wanted(wanted_outputs(app.engine.session()));
        m.tick(std::time::Instant::now());
    }
}

/// The session's MIDI input filter (`setup.midi_input_filter`) as the hub's bits.
pub fn filter_bits(s: &soundcraft_model::Session) -> u32 {
    soundcraft_playback::midi_in::FILTER_KINDS.iter().filter(|(k, _)| s.edit.flag(&format!("midi.filter.{k}"))).map(|(_, b)| b).sum()
}

/// The output ports the session's External MIDI instruments send to.
pub fn wanted_outputs(s: &soundcraft_model::Session) -> Vec<String> {
    let mut v: Vec<String> = s.tracks.iter().filter_map(|t| t.instrument.as_ref().and_then(soundcraft_engine::cmd::midi_out::output_port)).collect();
    v.sort();
    v.dedup();
    v
}

/// Recording starts: what the audio thread heard before is not part of the take.
pub(crate) fn start_take(app: &mut SoundApp) {
    app.midi_rec.clear();
    // The armed tracks as they are now: what a loop's finished passes are shown over.
    let s = app.engine.session();
    app.midi_loop_preview = Some(soundcraft_engine::midi_record::LoopPreview::new(s, &soundcraft_engine::midi_record::armed_midi_tracks(s)));
    if let Some(h) = &app.midi_hub {
        let mut stale = Vec::new();
        h.take_recorded(&mut stale);
        app.midi_rec_lost_at = h.record_dropped();
    }
    // The take's MIDI is spilled to the autosave folder as it comes (a crash keeps it); what was
    // spilled for an earlier take goes.
    let latency = take_latency(app);
    if let Some(root) = app.autosave_dir.clone() {
        app.autosaver.get_or_insert_with(|| soundcraft_engine::autosave::Autosaver::new(root)).midi_take(latency);
    }
}

/// Most events Capture MIDI keeps (the newest).
const CAPTURE_MAX: usize = 100_000;

/// While not recording (each frame): what the audio thread heard while playing, kept for Capture
/// MIDI (the newest [`CAPTURE_MAX`]).
pub(crate) fn collect_capture(app: &mut SoundApp) {
    let from = app.midi_capture.len();
    if let Some(h) = &app.midi_hub {
        h.take_captured(&mut app.midi_capture);
    }
    // Step Input enters what was played with the transport stopped.
    if app.step.open && app.midi_capture.len() > from {
        let new = app.midi_capture.get(from..).unwrap_or_default().to_vec();
        crate::step_input::feed(app, &new);
    }
    if app.midi_capture.len() > CAPTURE_MAX {
        let extra = app.midi_capture.len() - CAPTURE_MAX;
        app.midi_capture.drain(..extra);
    }
}

/// Event › More MIDI › Capture MIDI: what was played on the MIDI inputs without recording becomes
/// a take on the record-armed MIDI and instrument tracks (or, with none armed, the selected ones),
/// placed as a recorded take is (latency, input filter, record mode, input quantize). What was
/// played last counts: during the playback under way (or the last one), over the loop when it
/// looped, else from the bar where it began to the end of the bar where it ended; or, played with
/// the transport stopped (after the last playback), at the session's tempo from the bar the
/// playhead is in, its first note on that bar's downbeat. One undo step.
pub(crate) fn capture(app: &mut SoundApp) -> Result<serde_json::Value, String> {
    use soundcraft_playback::midi_in::LISTENING;
    collect_capture(app);
    let heard = std::mem::take(&mut app.midi_capture);
    // What was played last: while playing, or with the transport stopped.
    let listened = heard.last().is_some_and(|x| x.pass == LISTENING);
    let older = heard.iter().rposition(|x| (x.pass == LISTENING) != listened).map_or(0, |i| i + 1);
    let mut events = recorded_events(app, heard.get(older..).unwrap_or_default());
    if events.is_empty() {
        return Err("Nothing was played to capture: play along (stopped, or while the transport runs), then Capture MIDI.".into());
    }
    let latency = if listened { 0 } else { take_latency(app) };
    let s = app.engine.session();
    let (sr, tempo) = (s.sample_rate, &s.tempo);
    if listened {
        // On the session's timeline from the playhead's bar: the first note on its downbeat.
        let is_on = |m: [u8; 3]| m[0] & 0xF0 == 0x90 && m[2] > 0;
        let first = events.iter().filter(|e| is_on(e.msg)).map(|e| e.at).min().or_else(|| events.iter().map(|e| e.at).min()).unwrap_or(0);
        let bar = tempo.bar_beat_at_tick(tempo.samples_to_ticks(app.position().max(0), sr)).bar;
        let at = tempo.tick_to_samples(tempo.bar_start_tick(bar), sr);
        // (A pedal pressed before the first note is down from the downbeat.)
        for e in &mut events {
            e.at = at.saturating_add(e.at.saturating_sub(first)).max(at);
            e.pass = 0;
        }
    }
    let looped = app.play_loop.filter(|_| !listened && events.iter().any(|e| e.pass > 0));
    let take = match looped {
        Some(l) => soundcraft_engine::midi_record::MidiTake { start: l.start, end: l.end, looped: Some(l), punch: None, latency },
        None => {
            let first = events.iter().map(|e| e.at.saturating_sub(latency)).min().unwrap_or(0).max(0);
            let last = events.iter().map(|e| e.at).max().unwrap_or(first);
            let bar = |at: soundcraft_time::Samples| tempo.bar_beat_at_tick(tempo.samples_to_ticks(at, sr)).bar;
            let start = tempo.tick_to_samples(tempo.bar_start_tick(bar(first)), sr).min(first);
            let end = tempo.tick_to_samples(tempo.bar_start_tick(bar(last).saturating_add(1)), sr).max(last.saturating_add(1));
            soundcraft_engine::midi_record::MidiTake { start, end, looped: None, punch: None, latency }
        }
    };
    let armed = soundcraft_engine::midi_record::armed_midi_tracks(s);
    let tracks: Vec<soundcraft_model::TrackId> = if armed.is_empty() {
        s.tracks
            .iter()
            .filter(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi() && matches!(t.mixer.input, Route::Midi(_)))
            .map(|t| t.id)
            .collect()
    } else {
        armed
    };
    if tracks.is_empty() {
        app.midi_capture = heard;
        return Err("Capture MIDI needs a record-armed (or selected) MIDI or instrument track on a MIDI input.".into());
    }
    let ids = soundcraft_engine::midi_record::record_midi_on(&mut app.engine, &tracks, &take, &events);
    app.ui.status = format!("Captured {} clip(s)", ids.len());
    Ok(serde_json::json!({"clips": ids}))
}

/// Recorded MIDI events the take lost because the window did not collect them in time (it
/// stopped drawing, and the record ring filled).
pub(crate) fn lost_in_take(app: &SoundApp) -> u64 {
    app.midi_hub.as_ref().map_or(0, |h| h.record_dropped().saturating_sub(app.midi_rec_lost_at))
}

/// While recording (each frame): the live MIDI heard since the last frame.
pub(crate) fn collect(app: &mut SoundApp) {
    let from = app.midi_rec.len();
    if let Some(h) = &app.midi_hub {
        h.take_recorded(&mut app.midi_rec);
    }
    // Spilled to the autosave folder at once.
    if app.midi_rec.len() > from
        && let Some(w) = &app.autosaver
    {
        w.midi(recorded_events(app, app.midi_rec.get(from..).unwrap_or_default()));
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

/// Loop recording (each frame while recording, after [`collect`]): once the loop has wrapped
/// since the last showing, the passes finished so far go on the armed tracks, so the next pass
/// plays them (`soundcraft_engine::midi_record::LoopPreview`).
pub(crate) fn preview_loop(app: &mut SoundApp) {
    let pass = app.player.as_ref().map_or(0, soundcraft_playback::Player::loop_pass);
    if app.midi_loop_preview.as_ref().is_some_and(|p| pass > p.shown()) {
        show_passes(app, pass);
    }
}

/// Shows the passes before `upto` of the take so far.
fn show_passes(app: &mut SoundApp, upto: u16) {
    let Some(plan) = app.take_plan.clone() else { return };
    let take = take_for(app, &plan, app.position());
    let events = recorded_events(app, &app.midi_rec);
    let Some(mut p) = app.midi_loop_preview.take() else { return };
    if take.looped.is_some() {
        p.show(app.engine.session_mut(), &take, &events, upto);
    }
    app.midi_loop_preview = Some(p);
}

/// The loop passes shown while recording come off the tracks (Stop or Discard): the tracks are
/// as they were when the take began. What Undo Last Pass left out is still known to
/// [`finish_take`].
pub(crate) fn lift_preview(app: &mut SoundApp) {
    if let Some(mut p) = app.midi_loop_preview.take() {
        p.restore(app.engine.session_mut());
        app.midi_loop_preview = Some(p);
    }
}

/// Undo (Cmd+Z) while loop recording MIDI with a pass finished takes that pass back.
pub(crate) fn loop_undo_applies(app: &SoundApp) -> bool {
    app.engine.transport.recording && app.midi_loop_preview.as_ref().is_some_and(|p| p.shown() > 0)
}

/// Event › More MIDI › Undo Last Pass (and Undo while loop recording): the last finished loop
/// pass still in is left out of what plays and of the take; the loop runs on.
pub(crate) fn undo_last_pass(app: &mut SoundApp) -> Result<serde_json::Value, String> {
    if !app.engine.transport.recording {
        return Err("Undo Last Pass works while loop recording MIDI: it takes back the pass just played.".into());
    }
    let Some(p) = &mut app.midi_loop_preview else { return Err("No MIDI is being loop recorded.".into()) };
    let Some(pass) = p.undo_last_pass() else { return Err("No finished loop pass to undo yet.".into()) };
    let upto = p.shown();
    if let Some(w) = &app.autosaver {
        w.midi_drop(pass);
    }
    show_passes(app, upto);
    app.ui.status = format!("Loop pass {} undone", u32::from(pass) + 1);
    Ok(serde_json::json!({"pass": pass}))
}

/// The take as recorded so far (to `end`), with the plan's start, loop and punch.
fn take_for(app: &SoundApp, plan: &soundcraft_engine::io::Recording, end: soundcraft_time::Samples) -> soundcraft_engine::midi_record::MidiTake {
    soundcraft_engine::midi_record::MidiTake {
        start: plan.start,
        end,
        // Record pressed before a playing loop: the transport wraps there, and each pass after
        // the first is the loop (the player counts them).
        looped: plan
            .pass
            .map(|p| {
                let from = plan.loop_from.unwrap_or(plan.start);
                soundcraft_time::Range::new(from, from.saturating_add(p))
            })
            .or_else(|| app.play_loop.filter(|l| plan.start < l.start)),
        punch: plan.punch,
        latency: take_latency(app),
    }
}

/// Recording stopped at `end`: the take's live MIDI becomes clips on the armed MIDI tracks
/// (`soundcraft_engine::midi_record`), placed with the take's start, loop and punch. Loop passes
/// Undo Last Pass took back are left out.
pub(crate) fn finish_take(
    app: &mut SoundApp,
    plan: &soundcraft_engine::io::Recording,
    end: soundcraft_time::Samples,
) -> Vec<soundcraft_model::ClipId> {
    collect(app);
    let heard = std::mem::take(&mut app.midi_rec);
    let mut events = recorded_events(app, &heard);
    if let Some(p) = app.midi_loop_preview.take() {
        // (Off the tracks already: `lift_preview`.)
        events = p.keep(&events);
    }
    let take = take_for(app, plan, end);
    // A remote recorder sends this take to the booth (`net_ui`), even an empty one.
    app.last_midi_take = Some((take, events.clone()));
    if events.is_empty() {
        return Vec::new();
    }
    // QuickPunch: punched out and in again while recording, the take is kept to each punch.
    soundcraft_engine::midi_record::record_punches(&mut app.engine, &take, &plan.punches, &events)
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

/// Enables or disables an output port by name, and keeps the choice in the settings.
pub fn set_output_enabled(app: &mut SoundApp, name: &str, on: bool) {
    if let Some(m) = &mut app.midi_outputs {
        m.set_enabled(name, on);
        app.ui.midi_outputs = m.choices().clone();
    } else {
        app.ui.midi_outputs.insert(name.to_string(), on);
    }
}

/// The output ports a track can send to: those present and not a control surface's (one never
/// enabled opens when a track sends to it), and the track's own when it is not (marked so), as
/// `(name, available)`.
pub fn output_choices(app: &SoundApp, current: Option<&str>) -> Vec<(String, bool)> {
    let usable: Vec<String> =
        app.midi_outputs.as_ref().map(|m| m.ports().into_iter().filter(|p| p.present && !p.reserved).map(|p| p.name).collect()).unwrap_or_default();
    let mut out: Vec<(String, bool)> = usable.iter().map(|p| (p.clone(), true)).collect();
    if let Some(c) = current.filter(|c| !c.is_empty() && !usable.iter().any(|p| p == c)) {
        out.push((c.to_string(), false));
    }
    out
}

/// The output menu's MIDI section (instrument and MIDI tracks): send what the track plays to a
/// hardware MIDI output port (its instrument becomes External MIDI), and the channel.
pub(crate) fn output_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    let Some(track) = app.engine.session().track(id) else { return };
    if !track.kind.is_midi() {
        return;
    }
    let current = track.instrument.as_ref().and_then(soundcraft_engine::cmd::midi_out::output_port);
    let channel = track.instrument.as_ref().and_then(|i| i.params.get("channel").copied()).unwrap_or(0.0) as u8;
    ui.separator();
    ui.strong("MIDI Output");
    let dim = Tokens::current().text_dim;
    let choices = output_choices(app, current.as_deref());
    if choices.is_empty() {
        ui.label(egui::RichText::new("No MIDI output ports (Setup › MIDI Input Devices lists them)").small().color(dim));
    }
    for (name, available) in choices {
        let label = if available {
            egui::RichText::new(format!("MIDI: {name}"))
        } else {
            egui::RichText::new(format!("MIDI: {name} (not connected)")).color(dim)
        };
        if ui.add(egui::Button::selectable(current.as_deref() == Some(name.as_str()), label)).clicked() {
            // Chosen for a track: a port disabled in the window is enabled (the choice is
            // explicit); one never chosen opens because the track sends to it.
            if available && app.ui.midi_outputs.get(&name) == Some(&false) {
                set_output_enabled(app, &name, true);
            }
            if let Err(e) = app.run("mix.midi_output", json!({"track": id.0, "port": name})) {
                app.ui.status = e;
            }
            ui.close();
        }
    }
    let Some(port) = current else { return };
    ui.menu_button(if channel == 0 { "MIDI Channel: as played".to_string() } else { format!("MIDI Channel: {channel}") }, |ui| {
        for ch in 0..=16u8 {
            let label = if ch == 0 { "As played".to_string() } else { ch.to_string() };
            if ui.add(egui::Button::selectable(channel == ch, label)).clicked() {
                if let Err(e) = app.run("mix.midi_output", json!({"track": id.0, "port": port, "channel": ch})) {
                    app.ui.status = e;
                }
                ui.close();
            }
        }
    });
    if ui.button("No MIDI output").clicked() {
        if let Err(e) = app.run("mix.midi_output", json!({"track": id.0, "port": null})) {
            app.ui.status = e;
        }
        ui.close();
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
    transform_menu(app, ui, id, &m.transform);
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

/// The input menu's Input Transform submenu: what the track does to its MIDI input before its
/// instrument hears it and a take keeps it (`track.midi_input_transform`; a drag is one undo
/// step).
fn transform_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId, t: &soundcraft_model::MidiTransform) {
    let title = if t.is_identity() { "Input Transform".to_string() } else { "Input Transform (on)".to_string() };
    ui.menu_button(title, |ui| {
        let mut p = serde_json::Map::new();
        let (mut transpose, mut velocity, mut curve, mut fixed, mut low, mut high, mut channel) =
            (t.transpose, t.velocity, t.curve, t.fixed_velocity, t.key_low, t.key_high, t.channel);
        egui::Grid::new(("midi-transform", id.0)).num_columns(2).show(ui, |ui| {
            let mut row = |ui: &mut egui::Ui, label: &str, w: egui::DragValue, key: &str, v: serde_json::Value| {
                ui.label(label);
                if ui.add(w).changed() {
                    p.insert(key.into(), v);
                }
                ui.end_row();
            };
            row(ui, "Transpose", egui::DragValue::new(&mut transpose).range(-48..=48).suffix(" st"), "transpose", json!(0));
            row(ui, "Velocity", egui::DragValue::new(&mut velocity).range(1..=200).suffix(" %"), "velocity", json!(0));
            row(ui, "Curve", egui::DragValue::new(&mut curve).range(-100..=100), "curve", json!(0));
            row(
                ui,
                "Fixed velocity",
                egui::DragValue::new(&mut fixed).range(0..=127).custom_formatter(|v, _| if v < 1.0 { "Off".into() } else { format!("{v:.0}") }),
                "fixed_velocity",
                json!(0),
            );
            row(ui, "Lowest key", egui::DragValue::new(&mut low).range(0..=127), "key_low", json!(0));
            row(ui, "Highest key", egui::DragValue::new(&mut high).range(0..=127), "key_high", json!(0));
            row(
                ui,
                "Channel",
                egui::DragValue::new(&mut channel)
                    .range(0..=16)
                    .custom_formatter(|v, _| if v < 1.0 { "As played".into() } else { format!("{v:.0}") }),
                "channel",
                json!(0),
            );
        });
        // The values the drags left (the closure above only marked which moved).
        let now = [
            ("transpose", json!(transpose)),
            ("velocity", json!(velocity)),
            ("curve", json!(curve)),
            ("fixed_velocity", json!(fixed)),
            ("key_low", json!(low)),
            ("key_high", json!(high)),
            ("channel", json!(channel)),
        ];
        for (k, v) in now {
            if let Some(x) = p.get_mut(k) {
                *x = v;
            }
        }
        let reset = ui.add_enabled(!t.is_identity(), egui::Button::new("Reset")).clicked();
        if reset {
            p.clear();
            p.insert("reset".into(), json!(true));
        }
        if !p.is_empty() {
            p.insert("tracks".into(), json!([id.0]));
            let key = format!("midi-transform.{}", id.0);
            if let Err(e) = app.engine.execute_merged("track.midi_input_transform", &serde_json::Value::Object(p), &key) {
                app.ui.status = e.to_string();
            }
        }
        ui.label(egui::RichText::new("Applied to what the instrument hears and what a take records").small().color(Tokens::current().text_dim));
    });
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_inputs;
    if !open {
        app.midi_window_was_open = false;
        return;
    }
    if !std::mem::replace(&mut app.midi_window_was_open, true) {
        if let Some(m) = &mut app.midi_inputs {
            m.rescan();
        }
        if let Some(m) = &mut app.midi_outputs {
            m.rescan();
        }
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
        ui.strong("Outputs");
        let outs = app.midi_outputs.as_ref().map(|m| m.ports()).unwrap_or_default();
        if app.midi_outputs.is_none() {
            ui.label(egui::RichText::new("MIDI output is not available here.").color(t.text_dim));
        } else if outs.is_empty() {
            ui.label(egui::RichText::new("No MIDI output ports found.").small().color(t.text_dim));
        }
        let mut out_toggles = Vec::new();
        egui::Grid::new("midi-outputs").num_columns(3).spacing(vec2(12.0, 6.0)).striped(true).show(ui, |ui| {
            for p in &outs {
                let mut on = p.enabled;
                if ui.checkbox(&mut on, "").on_hover_text("Enable this output: tracks can send to it (track output menu › MIDI Output)").changed() {
                    out_toggles.push((p.name.clone(), on));
                }
                ui.label(&p.name);
                let state = match (&p.error, p.present, p.reserved, p.open) {
                    (Some(err), _, _, _) => format!("Could not open: {err}"),
                    (None, false, _, _) => "Not connected".into(),
                    (None, true, true, _) => "Used by a control surface".into(),
                    (None, true, false, true) if p.wanted && !app.ui.midi_outputs.contains_key(&p.name) => "Open (a track sends to it)".into(),
                    (None, true, false, true) => "Open".into(),
                    (None, true, false, false) if p.wanted => "Disabled, but a track sends to it: enable it to play".into(),
                    (None, true, false, false) => "Disabled".into(),
                };
                ui.label(egui::RichText::new(state).small().color(t.text_dim));
                ui.end_row();
            }
        });
        for (name, on) in out_toggles {
            set_output_enabled(app, &name, on);
        }
        ui.separator();
        // Setup › MIDI Input Filter: the session's, for every port.
        ui.horizontal_wrapped(|ui| {
            ui.label("Leave out:")
                .on_hover_text("Messages of these kinds from every input are not heard, recorded or captured (this session's setting)");
            let bits = filter_bits(app.engine.session());
            for (kind, bit, label) in [
                ("notes", 1, "Notes"),
                ("pitch_bend", 2, "Pitch bend"),
                ("mono_aftertouch", 4, "Aftertouch"),
                ("poly_aftertouch", 8, "Poly aftertouch"),
                ("program", 16, "Program"),
                ("cc", 32, "Controllers"),
            ] {
                let mut on = bits & bit != 0;
                if ui.checkbox(&mut on, label).changed() {
                    let key = if on { "filter" } else { "allow" };
                    if let Err(e) = app.run("setup.midi_input_filter", json!({key: [kind]})) {
                        app.ui.status = e;
                    }
                }
            }
        });
        ui.checkbox(&mut app.ui.midi_thru, "MIDI Thru")
            .on_hover_text("Record-enabled instrument and MIDI tracks play what comes in (input-monitored tracks always do)");
        ui.horizontal(|ui| {
            if ui.button("Rescan").clicked() {
                if let Some(m) = &mut app.midi_inputs {
                    m.rescan();
                }
                if let Some(m) = &mut app.midi_outputs {
                    m.rescan();
                }
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
pub(crate) mod tests {
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
        let gone = MidiIn { device: Some("Old Keys".into()), channel: 0, ..Default::default() };
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
        let plan = soundcraft_engine::io::Recording {
            start: 0,
            pass: None,
            punch: None,
            rate: 0,
            takes: Vec::new(),
            latency: 0,
            loop_from: None,
            punches: Vec::new(),
        };
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
    fn capture_midi_makes_a_take_of_what_was_just_played() {
        use soundcraft_playback::midi_in::{RecordedMidi, TYPING_SLOT};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": t, "input": "midi:Musical Typing"})).unwrap();
        assert!(app.run("ui.capture_midi", json!({})).is_err(), "nothing played yet");
        // Played (not recorded) a little into bar 2 at 120 bpm: a bar is 96,000 samples.
        let note = |at: i64, msg: [u8; 3]| RecordedMidi { at, port: TYPING_SLOT as u8, msg, pass: 0 };
        app.midi_capture = vec![note(100_000, [0x90, 60, 100]), note(130_000, [0x80, 60, 0])];
        app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
        let r = app.run("ui.capture_midi", json!({})).unwrap();
        let id = soundcraft_model::ClipId(r["clips"][0].as_u64().unwrap());
        let (_, c) = app.engine.session().find_clip(id).unwrap();
        assert_eq!((c.start, c.length), (96_000, 96_000), "the bar it was played in");
        let n = c.midi_playing().unwrap().notes[0];
        assert_eq!((n.pitch, n.length), (60, 30_000 * 960 / 24_000));
        assert!(app.midi_capture.is_empty(), "captured once");
        assert_eq!(app.engine.undo_label(), Some("Record"));
    }

    #[test]
    fn capture_midi_with_the_transport_stopped_starts_on_the_playheads_bar() {
        use soundcraft_playback::midi_in::{LISTENING, RecordedMidi, TYPING_SLOT};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": t, "input": "midi:Musical Typing"})).unwrap();
        app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
        let note = |at: i64, msg: [u8; 3], pass: u16| RecordedMidi { at, port: TYPING_SLOT as u8, msg, pass };
        // Played along with the transport earlier, then noodled with it stopped: the noodle is
        // what was played last. On the listening clock: a sustain pedal, then a note a quarter long.
        app.midi_capture = vec![
            note(500, [0x90, 40, 100], 0),
            note(5_000_000, [0xB0, 64, 127], LISTENING),
            note(5_010_000, [0x90, 60, 100], LISTENING),
            note(5_034_000, [0x80, 60, 0], LISTENING),
        ];
        // The playhead in bar 3 (bars are 96,000 samples at 120 bpm).
        app.engine.session_mut().edit.selection = soundcraft_time::Range::point(200_000);
        let r = app.run("ui.capture_midi", json!({})).unwrap();
        let id = soundcraft_model::ClipId(r["clips"][0].as_u64().unwrap());
        let (_, c) = app.engine.session().find_clip(id).unwrap();
        assert_eq!((c.start, c.length), (192_000, 96_000), "the playhead's bar");
        let seq = c.midi_playing().unwrap();
        assert_eq!(seq.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect::<Vec<_>>(), vec![(60, 0, 960)], "on the downbeat");
        assert_eq!(seq.ctrls[0].tick, 0, "the pedal pressed before it is down from the downbeat");
    }

    #[test]
    fn a_take_punched_out_and_in_on_the_fly_keeps_only_the_punches() {
        use soundcraft_playback::midi_in::{RecordedMidi, TYPING_SLOT};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": t, "input": "midi:Musical Typing"})).unwrap();
        app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
        start_take(&mut app);
        let note = |at: i64, pitch: u8| RecordedMidi { at, port: TYPING_SLOT as u8, msg: [0x90, pitch, 100], pass: 0 };
        // Punched out at 20,000, in again at 30,000, stopped at 48,000.
        app.midi_rec = vec![note(1_000, 60), note(25_000, 62), note(31_000, 64)];
        let plan = soundcraft_engine::io::Recording {
            start: 0,
            pass: None,
            punch: None,
            rate: 0,
            takes: Vec::new(),
            latency: 0,
            loop_from: None,
            punches: vec![soundcraft_time::Range::new(0, 20_000), soundcraft_time::Range::new(30_000, soundcraft_time::Samples::MAX)],
        };
        let ids = finish_take(&mut app, &plan, 48_000);
        assert_eq!(ids.len(), 2, "a clip per punch");
        let spans: Vec<(i64, i64, Vec<u8>)> = ids
            .iter()
            .map(|id| {
                let (_, c) = app.engine.session().find_clip(*id).unwrap();
                (c.start, c.end(), c.midi_playing().unwrap().notes.iter().map(|n| n.pitch).collect())
            })
            .collect();
        assert_eq!(spans, vec![(0, 20_000, vec![60]), (30_000, 48_000, vec![64])], "what was played punched out is left out");
    }

    #[test]
    fn loop_passes_are_heard_while_recording_and_undo_takes_the_last_back() {
        use soundcraft_playback::midi_in::{RecordedMidi, TYPING_SLOT};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": t, "input": "midi:Musical Typing"})).unwrap();
        app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
        start_take(&mut app);
        app.engine.transport.recording = true;
        // Loop recording over the first bar (96,000 samples at 120 bpm).
        let plan = soundcraft_engine::io::Recording {
            start: 0,
            pass: Some(96_000),
            punch: None,
            rate: 0,
            takes: Vec::new(),
            latency: 0,
            loop_from: Some(0),
            punches: Vec::new(),
        };
        app.take_plan = Some(plan.clone());
        let note = |at: i64, pitch: u8, pass: u16| RecordedMidi { at, port: TYPING_SLOT as u8, msg: [0x90, pitch, 100], pass };
        app.midi_rec = vec![note(1_000, 36, 0), note(30_000, 38, 1)];
        let pitches = |app: &SoundApp| -> Vec<u8> {
            let tr = app.engine.session().track(TrackId(t)).unwrap();
            tr.clips().iter().flat_map(|c| c.midi_playing().unwrap().notes.iter().map(|n| n.pitch)).collect()
        };
        assert!(pitches(&app).is_empty());
        // The loop wrapped into pass 2: passes 1 and 2 play (merged, the default).
        show_passes(&mut app, 2);
        assert_eq!(pitches(&app), vec![36, 38]);
        // Cmd+Z while loop recording: the last pass goes; the session's undo is untouched.
        let undo = app.engine.undo_label().map(str::to_string);
        assert_eq!(app.run("edit.undo", json!({})).unwrap()["pass"], 1);
        assert_eq!(pitches(&app), vec![36]);
        assert_eq!(app.engine.undo_label().map(str::to_string), undo);
        // Stop: what was undone stays out of the take, which is one undo step.
        lift_preview(&mut app);
        app.engine.transport.recording = false;
        let ids = finish_take(&mut app, &plan, 150_000);
        assert_eq!(ids.len(), 1);
        assert_eq!(pitches(&app), vec![36]);
        assert!(app.run("ui.midi_undo_pass", json!({})).is_err(), "not while stopped");
        app.run("edit.undo", json!({})).unwrap();
        assert!(pitches(&app).is_empty());
    }

    #[test]
    fn outputs_are_enabled_in_the_window_and_a_track_sends_to_one() {
        use soundcraft_playback::midi_out::{FakeOutBackend, MidiOutputs};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let fake = FakeOutBackend::with_ports(&["UI Test Juno", "UI Test Moog"]);
        let mut m = MidiOutputs::new(fake.boxed(), app.ui.midi_outputs.clone());
        m.rescan();
        app.midi_outputs = Some(m);
        // Outputs start disabled: on Windows a port is open in one program at a time.
        assert!(!fake.is_open("UI Test Juno") && !fake.is_open("UI Test Moog"));
        set_output_enabled(&mut app, "UI Test Moog", true);
        assert!(fake.is_open("UI Test Moog"));
        assert_eq!(serde_json::to_value(&app.ui).unwrap()["midi.outputs"]["UI Test Moog"], true);
        set_output_enabled(&mut app, "UI Test Moog", false);
        assert_eq!(serde_json::to_value(&app.ui).unwrap()["midi.outputs"]["UI Test Moog"], false);
        // A track can still pick any port there is.
        assert_eq!(output_choices(&app, None), vec![("UI Test Juno".to_string(), true), ("UI Test Moog".to_string(), true)]);
        assert_eq!(output_choices(&app, Some("Gone")).last(), Some(&("Gone".to_string(), false)));
        // A track that sends to a port never chosen opens it (the session wants it).
        let t = app.engine.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("mix.midi_output", json!({"track": t, "port": "UI Test Juno"})).unwrap();
        let ins = app.engine.session().track(TrackId(t)).unwrap().instrument.clone().unwrap();
        assert_eq!(soundcraft_engine::cmd::midi_out::output_port(&ins).as_deref(), Some("UI Test Juno"));
        tick(&mut app);
        assert!(fake.is_open("UI Test Juno"));
        assert!(!app.ui.midi_outputs.contains_key("UI Test Juno"), "nothing saved for it");
        // One disabled in the window stays closed; the window says a track wants it.
        app.run("mix.midi_output", json!({"track": t, "port": "UI Test Moog"})).unwrap();
        tick(&mut app);
        assert!(!fake.is_open("UI Test Moog") && !fake.is_open("UI Test Juno"));
        let moog = app.midi_outputs.as_ref().unwrap().ports().into_iter().find(|p| p.name == "UI Test Moog").unwrap();
        assert!(moog.wanted && !moog.open);
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
