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

/// Said in the window when the audio engine has not taken the ports' messages for a while.
pub const ENGINE_IDLE: &str =
    "The audio engine is not taking MIDI in: nothing played reaches the tracks. Check Setup > Playback Engine (Reconnect audio device).";

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
    if let Some(m) = &mut app.midi_inputs {
        m.set_offsets(app.ui.midi_offsets.clone());
        m.tick(std::time::Instant::now());
    }
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
    // The port MIDI clock or timecode goes to, while either is on.
    if s.edit.flag("midi.beat_clock") || s.edit.flag("midi.beat_clock.mtc") {
        v.extend(sync_destination(s));
    }
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

/// Event › Capture MIDI: what was played on the MIDI inputs without recording becomes
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
    heard
        .iter()
        .map(|r| soundcraft_engine::midi_record::RecordedEvent {
            at: r.at,
            port: name(r.port),
            msg: r.msg,
            pass: r.pass,
            sysex: r.sysex.as_deref().map(<[u8]>::to_vec).unwrap_or_default(),
        })
        .collect()
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

/// Event › Undo Last Pass (and Undo while loop recording): the last finished loop
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

/// Event › Erase Held Notes (while loop recording MIDI): the keys held now are taken
/// out of the take so far, every note of theirs in the finished passes and the one under way
/// (the held keys too: they are not recorded), and what plays follows. Keys are told apart by
/// channel and pitch. The take spilled for crash recovery still has them.
pub(crate) fn erase_held_notes(app: &mut SoundApp) -> Result<serde_json::Value, String> {
    if !app.engine.transport.recording || app.midi_loop_preview.is_none() {
        return Err("Erase Held Notes works while loop recording MIDI: hold the keys whose notes should go.".into());
    }
    collect(app);
    let is_on = |m: [u8; 3]| m[0] & 0xF0 == 0x90 && m[2] > 0;
    let is_note = |m: [u8; 3]| matches!(m[0] & 0xF0, 0x80 | 0x90);
    let key = |m: [u8; 3]| (m[0] & 0x0F, m[1] & 0x7F);
    let mut held: Vec<(u8, u8)> = Vec::new();
    for e in &app.midi_rec {
        if is_on(e.msg) {
            if !held.contains(&key(e.msg)) {
                held.push(key(e.msg));
            }
        } else if is_note(e.msg) {
            held.retain(|k| *k != key(e.msg));
        }
    }
    if held.is_empty() {
        return Err("No key is held: hold the keys whose notes should go, then Erase Held Notes.".into());
    }
    app.midi_rec.retain(|e| !(is_note(e.msg) && held.contains(&key(e.msg))));
    let upto = app.midi_loop_preview.as_ref().map_or(0, soundcraft_engine::midi_record::LoopPreview::shown);
    show_passes(app, upto);
    let names: Vec<String> = held.iter().map(|(_, p)| soundcraft_midi::note_name(*p)).collect();
    app.ui.status = format!("Erased {} from the take", names.join(", "));
    Ok(serde_json::json!({"erased": held.iter().map(|(c, p)| serde_json::json!({"channel": c + 1, "pitch": p})).collect::<Vec<_>>()}))
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

/// The output menu's "Play Instrument On" part (instrument and MIDI tracks): play the instrument
/// of another instrument or MIDI track (several tracks on their own channels play one
/// multi-timbral instrument), the channel, and taking it off.
fn to_track_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    let s = app.engine.session();
    let Some(track) = s.track(id) else { return };
    let current = track.instrument.as_ref().and_then(soundcraft_engine::cmd::midi_out::target_track);
    let channel = track.instrument.as_ref().filter(|_| current.is_some()).and_then(|i| i.params.get("channel").copied()).unwrap_or(0.0) as u8;
    let targets: Vec<(TrackId, String)> =
        s.tracks.iter().filter(|t| t.id != id && t.kind.is_midi() && t.instrument.is_some()).map(|t| (t.id, t.name.clone())).collect();
    let title = match current.and_then(|c| s.track(c)) {
        Some(t) => format!("Play Instrument On: {}", t.name),
        None => "Play Instrument On".to_string(),
    };
    ui.menu_button(title, |ui| {
        if targets.is_empty() {
            ui.label(egui::RichText::new("No other instrument tracks").small().color(Tokens::current().text_dim));
        }
        for (t, name) in &targets {
            if ui.add(egui::Button::selectable(current == Some(*t), name.as_str())).clicked() {
                if let Err(e) = app.run("mix.midi_to_track", json!({"track": id.0, "target": t.0})) {
                    app.ui.status = e;
                }
                ui.close();
            }
        }
        let Some(target) = current else { return };
        ui.separator();
        ui.menu_button(if channel == 0 { "Channel: as played".to_string() } else { format!("Channel: {channel}") }, |ui| {
            for ch in 0..=16u8 {
                let label = if ch == 0 { "As played".to_string() } else { ch.to_string() };
                if ui.add(egui::Button::selectable(channel == ch, label)).clicked() {
                    if let Err(e) = app.run("mix.midi_to_track", json!({"track": id.0, "target": target.0, "channel": ch})) {
                        app.ui.status = e;
                    }
                    ui.close();
                }
            }
        });
        let mut on_target = app.engine.session().track(id).is_some_and(|t| t.midi_record_on_target);
        if ui
            .checkbox(&mut on_target, "Record on the Target")
            .on_hover_text("A take on this track goes onto the tracks it plays, as each heard them (after this track's MIDI effects, each layer's range, the channel); off, it stays here as played")
            .changed()
            && let Err(e) = app.run("track.midi_record_on_target", json!({"tracks": [id.0], "value": on_target}))
        {
            app.ui.status = e;
        }
        if ui.button("Not on Another Track").clicked() {
            if let Err(e) = app.run("mix.midi_to_track", json!({"track": id.0, "target": null})) {
                app.ui.status = e;
            }
            ui.close();
        }
    });
    layers_menu(app, ui, id);
}

/// The output menu's "Layers & Splits" part: the track plays up to four instruments at once, each
/// over a key range and a velocity range (`mix.midi_layers`). Each layer's ranges are drag
/// values (a drag is one undo step); a layer is added from the other instrument tracks.
fn layers_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    use soundcraft_mix::track_midi::{MAX_ZONES, Zone};
    let s = app.engine.session();
    let Some(track) = s.track(id) else { return };
    let zones: Vec<Zone> = track.instrument.as_ref().map(soundcraft_engine::cmd::midi_out::layers).unwrap_or_default();
    let targets: Vec<(TrackId, String)> =
        s.tracks.iter().filter(|t| t.id != id && t.kind.is_midi() && t.instrument.is_some()).map(|t| (t.id, t.name.clone())).collect();
    let name = |t: TrackId| s.track(t).map_or_else(|| "(gone)".to_string(), |t| t.name.clone());
    let names: Vec<String> = zones.iter().map(|z| name(z.track)).collect();
    let title = if zones.len() > 1 { format!("Layers & Splits: {}", zones.len()) } else { "Layers & Splits".to_string() };
    let as_json = |zs: &[Zone]| -> Vec<serde_json::Value> {
        zs.iter().map(|z| json!({"target": z.track.0, "keys": [z.keys.0, z.keys.1], "velocities": [z.vel.0, z.vel.1]})).collect()
    };
    ui.menu_button(title, |ui| {
        let mut next = zones.clone();
        let mut merge: Option<String> = None;
        let mut remove = None;
        for (i, (z, n)) in next.iter_mut().zip(&names).enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(n).strong());
                let mut drag = |ui: &mut egui::Ui, v: &mut u8, lo: u8, hi: u8, what: &str, note: bool| {
                    let r = ui.add(egui::DragValue::new(v).range(lo..=hi).custom_formatter(move |x, _| {
                        if note { soundcraft_midi::note_name(x.round().clamp(0.0, 127.0) as u8) } else { format!("{x:.0}") }
                    }));
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, format!("{n} {what}")));
                    if r.changed() {
                        merge = Some(format!("midi_layers:{}:{i}:{what}", id.0));
                    }
                };
                ui.label("Keys");
                drag(ui, &mut z.keys.0, 0, 127, "lowest key", true);
                drag(ui, &mut z.keys.1, 0, 127, "highest key", true);
                ui.label("Vel");
                drag(ui, &mut z.vel.0, 1, 127, "lowest velocity", false);
                drag(ui, &mut z.vel.1, 1, 127, "highest velocity", false);
                if ui.small_button("✕").on_hover_text("Take this layer off").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(key) = merge
            && let Err(e) = app.engine.execute_merged("mix.midi_layers", &json!({"track": id.0, "layers": as_json(&next)}), &key)
        {
            app.ui.status = e.to_string();
        }
        if let Some(i) = remove {
            next.remove(i);
            if let Err(e) = app.run("mix.midi_layers", json!({"track": id.0, "layers": as_json(&next)})) {
                app.ui.status = e;
            }
        }
        if !zones.is_empty() {
            ui.separator();
        }
        ui.add_enabled_ui(zones.len() < MAX_ZONES, |ui| {
            ui.menu_button("Add Layer", |ui| {
                if targets.is_empty() {
                    ui.label(egui::RichText::new("No other instrument tracks").small().color(Tokens::current().text_dim));
                }
                for (t, n) in &targets {
                    if ui.button(n).clicked() {
                        let mut more = zones.clone();
                        more.push(Zone::all(*t));
                        if let Err(e) = app.run("mix.midi_layers", json!({"track": id.0, "layers": as_json(&more)})) {
                            app.ui.status = e;
                        }
                        ui.close();
                    }
                }
            });
        })
        .response
        .on_disabled_hover_text(format!("A track plays at most {MAX_ZONES} layers"));
    });
}

/// The window's Studio Setup: what is connected to each output port (a device's name and the
/// channel it listens on), kept on this computer. The track output menus list the devices under
/// their port, so a track picks "JV-1080 (ch 3)" rather than a port and a channel.
fn studio_setup(app: &mut SoundApp, ui: &mut egui::Ui, ports: &[String]) {
    let t = Tokens::current();
    let mut all: Vec<String> = ports.to_vec();
    all.extend(app.ui.midi_studio.keys().filter(|k| !ports.contains(k)).cloned());
    if all.is_empty() {
        return;
    }
    egui::CollapsingHeader::new("Studio Setup").id_salt("midi-studio").show(ui, |ui| {
        ui.label(
            egui::RichText::new("What is connected to each output (on this computer): the track output menus list these devices")
                .small()
                .color(t.text_dim),
        );
        for port in &all {
            let gone = !ports.contains(port);
            ui.horizontal(|ui| {
                ui.strong(port);
                if gone {
                    ui.label(egui::RichText::new("(not connected)").small().color(t.text_dim));
                }
            });
            let mut list = app.ui.midi_studio.get(port).cloned().unwrap_or_default();
            let before = list.clone();
            let mut remove = None;
            for (i, d) in list.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let r = ui.add(egui::TextEdit::singleline(&mut d.name).hint_text("Device").desired_width(140.0));
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, format!("{port} device {}", i + 1)));
                    ui.label("Channel");
                    ui.add(
                        egui::DragValue::new(&mut d.channel)
                            .range(0..=16)
                            .custom_formatter(|v, _| if v < 0.5 { "all".into() } else { format!("{v:.0}") }),
                    );
                    if ui.small_button("✕").on_hover_text("Take this device off").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                list.remove(i);
            }
            if list.len() < MAX_STUDIO_DEVICES
                && ui
                    .horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.small_button("Add Device").on_hover_text(format!("A device connected to {port}"))
                    })
                    .inner
                    .clicked()
            {
                list.push(StudioDevice { name: String::new(), channel: 0 });
            }
            if list != before {
                for d in &mut list {
                    d.name = d.name.chars().filter(|c| !c.is_control()).take(64).collect();
                    d.channel = d.channel.min(16);
                }
                if list.is_empty() {
                    app.ui.midi_studio.remove(port);
                } else {
                    app.ui.midi_studio.insert(port.clone(), list);
                }
            }
        }
    });
}

/// The output port MIDI clock and timecode go to (`setup.midi_beat_clock`'s destination).
fn sync_destination(s: &soundcraft_model::Session) -> Option<String> {
    s.edit.flags.iter().find_map(|f| f.strip_prefix("midi.beat_clock.destination=")).map(str::to_string)
}

/// Setup › MIDI Beat Clock, in the window's Outputs part: MIDI clock and MIDI timecode out, and
/// the port they go to (`setup.midi_beat_clock`).
fn sync_row(app: &mut SoundApp, ui: &mut egui::Ui, outs: &[soundcraft_playback::midi_out::OutPortInfo]) {
    let s = app.engine.session();
    let (mut clock, mut mtc, mut mmc) = (s.edit.flag("midi.beat_clock"), s.edit.flag("midi.beat_clock.mtc"), s.edit.flag("midi.mmc_out"));
    let dest = sync_destination(s).unwrap_or_default();
    let (mode, source) = crate::midi_sync_in::sync_in_mode(s);
    let (mode, source) = (mode.to_string(), source.unwrap_or_default().to_string());
    ui.horizontal_wrapped(|ui| {
        ui.label("Sync out:");
        let mut changed = false;
        changed |=
            ui.checkbox(&mut clock, "MIDI clock").on_hover_text("24 clocks a quarter note with Start, Stop, Continue and Song Position").changed();
        changed |= ui.checkbox(&mut mtc, "MIDI timecode").on_hover_text("MTC quarter frames at the session's frame rate").changed();
        changed |=
            ui.checkbox(&mut mmc, "MMC").on_hover_text("MIDI Machine Control: Play, Stop, Record and Locate as the transport does them").changed();
        let mut to = dest.clone();
        egui::ComboBox::from_id_salt("midi-sync-dest").selected_text(if to.is_empty() { "Choose a port".to_string() } else { to.clone() }).show_ui(
            ui,
            |ui| {
                for p in outs {
                    ui.selectable_value(&mut to, p.name.clone(), &p.name);
                }
            },
        );
        if changed || to != dest {
            let mut p = json!({"enabled": clock, "mtc": mtc, "mmc": mmc});
            if !to.is_empty() {
                p["destination"] = json!(to);
            }
            if let Err(e) = app.run("setup.midi_beat_clock", p) {
                app.ui.status = e;
            }
            // Chosen for sync: the port is enabled, as one chosen for a track is.
            if (clock || mtc || mmc) && !to.is_empty() && app.ui.midi_outputs.get(&to) == Some(&false) {
                set_output_enabled(app, &to, true);
            }
        }
    });
    // Following another device's clock or timecode.
    let inputs: Vec<String> = app.midi_inputs.as_ref().map(|m| m.ports().into_iter().map(|p| p.name).collect()).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label("Sync in:");
        let (mut m, mut src) = (mode.clone(), source.clone());
        egui::ComboBox::from_id_salt("midi-sync-in")
            .selected_text(match m.as_str() {
                "clock" => "MIDI clock",
                "mtc" => "MIDI timecode",
                _ => "Off",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut m, "off".to_string(), "Off");
                ui.selectable_value(&mut m, "clock".to_string(), "MIDI clock")
                    .on_hover_text("Start, Stop, Continue and Song Position run the transport");
                ui.selectable_value(&mut m, "mtc".to_string(), "MIDI timecode").on_hover_text("The transport chases the timecode coming in");
            });
        egui::ComboBox::from_id_salt("midi-sync-in-port").selected_text(if src.is_empty() { "Any input".to_string() } else { src.clone() }).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut src, String::new(), "Any input");
                for n in &inputs {
                    ui.selectable_value(&mut src, n.clone(), n);
                }
            },
        );
        if let Some(bpm) = app.sync.follower.tempo().filter(|_| m == "clock") {
            ui.label(format!("{bpm:.1} bpm"));
        }
        if (m != mode || src != source)
            && let Err(e) = app.run("setup.midi_beat_clock", json!({"sync_in": m, "source": src}))
        {
            app.ui.status = e;
        }
    });
}

/// The input menu's MPE Zone: off, the lower or upper zone, its member channels and bend ranges
/// (`track.mpe`), sent to the instrument as MPE's configuration message.
fn mpe_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    let now = app.engine.session().track(id).and_then(|t| t.mpe);
    let title = match now {
        None => "MPE Zone: Off".to_string(),
        Some(z) => format!("MPE Zone: {} ({} channels)", if z.upper { "Upper" } else { "Lower" }, z.members),
    };
    let mut ask: Option<serde_json::Value> = None;
    ui.menu_button(title, |ui| {
        for (label, zone) in [("Off", "off"), ("Lower (master channel 1)", "lower"), ("Upper (master channel 16)", "upper")] {
            let on = match now {
                None => zone == "off",
                Some(z) => zone == if z.upper { "upper" } else { "lower" },
            };
            if ui.add(egui::Button::selectable(on, label)).clicked() {
                ask = Some(json!({"tracks": [id.0], "zone": zone}));
            }
        }
        if let Some(z) = now {
            ui.separator();
            let (mut members, mut bend, mut master) = (z.members, z.bend, z.master_bend);
            let a = ui.add(egui::Slider::new(&mut members, 1..=15).text("member channels")).changed();
            let b = ui.add(egui::Slider::new(&mut bend, 0..=96).text("member bend (semitones)")).changed();
            let c = ui.add(egui::Slider::new(&mut master, 0..=96).text("master bend (semitones)")).changed();
            if a || b || c {
                ask = Some(json!({"tracks": [id.0], "members": members, "bend": bend, "master_bend": master}));
            }
        }
        ui.label(
            egui::RichText::new("Sent to the instrument as MPE's configuration message and pitch-bend ranges")
                .small()
                .color(Tokens::current().text_dim),
        );
    });
    if let Some(p) = ask {
        let r = if p.get("zone").is_some() {
            app.run("track.mpe", p).map(|_| ())
        } else {
            app.engine.execute_merged("track.mpe", &p, &format!("mpe-{}", id.0)).map(|_| ()).map_err(|e| e.to_string())
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
}

/// The input menu's Articulations (sound variations, an expression map): the track's
/// articulations with their keyswitches (`track.articulations`), and one set in the selected
/// MIDI clip at the insertion point (a point on the articulation lane, `midi.ctrl_add`).
fn articulations_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    let now = app.engine.session().track(id).and_then(|t| t.articulations.clone()).unwrap_or_default();
    let mut asks: Vec<(&str, serde_json::Value)> = Vec::new();
    let title = if now.items.is_empty() { "Articulations".to_string() } else { format!("Articulations ({})", now.items.len()) };
    ui.menu_button(title, |ui| {
        for (i, a) in now.items.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(&a.name);
                let mut k = a.keyswitch;
                if ui
                    .add(egui::DragValue::new(&mut k).range(0..=127).custom_formatter(|v, _| soundcraft_midi::note_name(v as u8)))
                    .on_hover_text("Its keyswitch")
                    .changed()
                {
                    asks.push(("track.articulations", json!({"tracks": [id.0], "index": i, "keyswitch": k})));
                }
                if ui.small_button("Set Here").on_hover_text("From the insertion point on, the selected clip plays this way").clicked() {
                    asks.push(("articulation.here", json!(i)));
                    ui.close();
                }
                if ui.small_button("✕").on_hover_text("Remove").clicked() {
                    asks.push(("track.articulations", json!({"tracks": [id.0], "index": i, "remove": true})));
                }
            });
        }
        let key = egui::Id::new(("articulation-new", id.0));
        let mut name: String = ui.data(|d| d.get_temp(key)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut name).hint_text("New articulation").desired_width(110.0));
            let next = now.items.last().map_or(24, |a| a.keyswitch.saturating_add(1).min(127));
            if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Add")).clicked() {
                asks.push(("track.articulations", json!({"tracks": [id.0], "add": {"name": name.trim(), "keyswitch": next}})));
                name.clear();
            }
        });
        ui.data_mut(|d| d.insert_temp(key, name));
        ui.label(
            egui::RichText::new(format!(
                "Chosen in clips by controller {} (value = the articulation's number, from 0); its keyswitch is sent in its place",
                now.cc
            ))
            .small()
            .color(Tokens::current().text_dim),
        );
    });
    for (cmd, p) in asks {
        let r =
            if cmd == "articulation.here" { set_articulation_here(app, id, now.cc, p.as_u64().unwrap_or(0)) } else { app.run(cmd, p).map(|_| ()) };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
}

/// The selected MIDI clip on `track` (or the one under the insertion point) gets articulation
/// `index` from the insertion point on.
fn set_articulation_here(app: &mut SoundApp, track: TrackId, cc: u8, index: u64) -> Result<(), String> {
    let s = app.engine.session();
    let at = s.edit.selection.start;
    let tr = s.track(track).ok_or("no such track")?;
    let clip = tr
        .clips()
        .iter()
        .filter(|c| c.midi_playing().is_some())
        .find(|c| s.edit.selected_clips.contains(&c.id) && c.range().contains(at))
        .or_else(|| tr.clips().iter().filter(|c| c.midi_playing().is_some()).find(|c| c.start <= at && at < c.end()))
        .ok_or("Put the insertion point in one of the track's MIDI clips first")?;
    let tick = s.tempo.samples_to_ticks(at, s.sample_rate) - s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    let cid = clip.id.0;
    app.run("midi.ctrl_add", json!({"clip": cid, "lane": format!("cc{cc}"), "tick": tick.max(0), "value": index})).map(|_| ())
}

/// The output menu's Instrument Outputs part (an instrument with outputs besides its main one,
/// Juke's Out 2-8): each output on a track of its own (`mix.new_output_track`), or back on the
/// main output (`mix.instrument_output`).
fn instrument_outputs_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    let s = app.engine.session();
    let Some(track) = s.track(id) else { return };
    let names = track.instrument.as_ref().map(soundcraft_engine::cmd::instrument::output_names).unwrap_or_default();
    if names.is_empty() {
        return;
    }
    let taken: Vec<(u8, String)> = track
        .instrument_outs
        .iter()
        .map(|o| (o.output, s.busses.iter().find(|b| b.id == o.bus).map(|b| b.name.clone()).unwrap_or_default()))
        .collect();
    let mut ask: Option<(&'static str, serde_json::Value)> = None;
    ui.menu_button("Instrument Outputs", |ui| {
        for (i, name) in names.iter().enumerate() {
            let Ok(k) = u8::try_from(i) else { continue };
            match taken.iter().find(|t| t.0 == k) {
                Some((_, bus)) => {
                    ui.menu_button(format!("{name} › {bus}"), |ui| {
                        if ui.button("Back on the Main Output").clicked() {
                            ask = Some(("mix.instrument_output", json!({"track": id.0, "output": i, "bus": null})));
                            ui.close();
                        }
                    });
                }
                None => {
                    if ui.button(format!("New Track for {name}")).on_hover_text("A bus and an aux track that plays this output").clicked() {
                        ask = Some(("mix.new_output_track", json!({"track": id.0, "output": i})));
                        ui.close();
                    }
                }
            }
        }
        ui.label(
            egui::RichText::new("A pad or part set to an output nobody takes plays on the main output").small().color(Tokens::current().text_dim),
        );
    });
    if let Some((cmd, p)) = ask
        && let Err(e) = app.run(cmd, p)
    {
        app.ui.status = e;
    }
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
        let devices: Vec<StudioDevice> = studio_devices(app, &name).into_iter().cloned().collect();
        let here = current.as_deref() == Some(name.as_str());
        let mut pick: Option<Option<u8>> = None;
        if ui.add(egui::Button::selectable(here && (devices.is_empty() || studio_device_for(app, &name, channel).is_none()), label)).clicked() {
            pick = Some(None);
        }
        // The studio setup's devices on this port: each sets the port and its channel.
        for d in &devices {
            let on = here && studio_device_for(app, &name, channel) == Some(d);
            let text = egui::RichText::new(format!("    {}", studio_label(d)));
            if ui.add(egui::Button::selectable(on, if available { text } else { text.color(dim) })).on_hover_text(format!("On {name}")).clicked() {
                pick = Some(Some(d.channel));
            }
        }
        if let Some(ch) = pick {
            // Chosen for a track: a port disabled in the window is enabled (the choice is
            // explicit); one never chosen opens because the track sends to it.
            if available && app.ui.midi_outputs.get(&name) == Some(&false) {
                set_output_enabled(app, &name, true);
            }
            let p = match ch {
                Some(c) => json!({"track": id.0, "port": name, "channel": c}),
                None => json!({"track": id.0, "port": name}),
            };
            if let Err(e) = app.run("mix.midi_output", p) {
                app.ui.status = e;
            }
            ui.close();
        }
    }
    to_track_menu(app, ui, id);
    instrument_outputs_menu(app, ui, id);
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

/// One device of the studio setup: a hardware synth, drum machine or module connected to an
/// output port, and the channel it listens on (0: every channel).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StudioDevice {
    pub name: String,
    #[serde(default)]
    pub channel: u8,
}

/// Most devices the studio setup keeps on one port.
pub const MAX_STUDIO_DEVICES: usize = 16;

/// The devices named on output port `port`, in order, with a name.
pub fn studio_devices<'a>(app: &'a SoundApp, port: &str) -> Vec<&'a StudioDevice> {
    app.ui.midi_studio.get(port).map(|v| v.iter().filter(|d| !d.name.trim().is_empty()).collect()).unwrap_or_default()
}

/// A studio device as the output menu names it: "JV-1080 (ch 1)", or just its name when it
/// takes every channel.
pub fn studio_label(d: &StudioDevice) -> String {
    if d.channel == 0 { d.name.clone() } else { format!("{} (ch {})", d.name, d.channel) }
}

/// The studio device a track sending to `port` on `channel` plays, when one is set up there: one
/// on that channel, else one taking every channel.
pub fn studio_device_for<'a>(app: &'a SoundApp, port: &str, channel: u8) -> Option<&'a StudioDevice> {
    let devs = studio_devices(app, port);
    devs.iter().find(|d| d.channel != 0 && d.channel == channel).or_else(|| devs.iter().find(|d| d.channel == 0)).copied()
}

/// What the input port `name` is called here: its alias, else its name.
pub fn alias<'a>(app: &'a SoundApp, name: &'a str) -> &'a str {
    app.ui.midi_aliases.get(name).map(String::as_str).filter(|a| !a.trim().is_empty()).unwrap_or(name)
}

/// The MIDI inputs a track's input menu offers: All Inputs, then each enabled port present now
/// (and the track's own port when it is unplugged or disabled, marked so), as `(label, device,
/// available)`; `device` is `None` for All Inputs.
pub fn input_choices(app: &SoundApp, current: Option<&MidiIn>) -> Vec<(String, Option<String>, bool)> {
    let mut out = vec![("MIDI: All Inputs".to_string(), None, true)];
    let ports: Vec<String> = app.midi_inputs.as_ref().map(|m| m.ports().into_iter().filter(|p| p.open).map(|p| p.name).collect()).unwrap_or_default();
    for p in &ports {
        out.push((format!("MIDI: {}", alias(app, p)), Some(p.clone()), true));
    }
    // Musical Typing's on-screen keys and the computer keys (Window › Editors › Musical Typing).
    let typing = soundcraft_playback::midi_in::TYPING_NAME.to_string();
    if app.midi_hub.is_some() {
        out.push((format!("MIDI: {typing}"), Some(typing.clone()), true));
    }
    // The remote recorders' ports (Setup › Network): `Remote: <PC> › <port>`.
    let remote = app.net.as_ref().map(|n| n.hub.remote_ports()).unwrap_or_default();
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
    crate::midi_fx_menu::menu(app, ui, id);
    mpe_menu(app, ui, id);
    articulations_menu(app, ui, id);
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

/// An open input's diagnostics in a line: "Nothing received yet", or "152 received, 150 passed
/// on; last: Note on C3 vel 80, ch 1, 2 s ago" (passed on: taken by the audio engine towards the
/// tracks, after the input filter), with how late the driver stamped it when that is more than a
/// little.
pub fn diag_line(d: &soundcraft_playback::midi_in::PortDiag) -> String {
    let alive = d.keepalive && d.heard.is_some_and(|h| h < Duration::from_secs(2));
    let Some(last) = &d.last else {
        return if alive { "Nothing played yet (the device is connected: its keep-alive comes)".into() } else { "Nothing received yet".into() };
    };
    let mut s = format!("{} received, {} passed on; last: {last}", d.received, d.taken);
    if let Some(since) = d.since {
        s.push_str(&if since.as_secs() >= 1 { format!(", {} s ago", since.as_secs()) } else { ", just now".into() });
    }
    if let Some(lag) = d.lag.filter(|l| *l >= Duration::from_millis(20)) {
        s.push_str(&format!(" (the driver dated it {} ms before it came)", lag.as_millis()));
    }
    s
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
        egui::Grid::new("midi-inputs").num_columns(5).spacing(vec2(12.0, 6.0)).striped(true).show(ui, |ui| {
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
                let diag = app.midi_inputs.as_ref().and_then(|m| m.diag(&p.name));
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(state).small().color(t.text_dim));
                    // What the system handed over, counted before any filter: 0 while playing
                    // means nothing reaches FreeDAWm from this port.
                    if p.open
                        && let Some(d) = &diag
                    {
                        ui.label(egui::RichText::new(diag_line(d)).small().color(t.text_dim)).on_hover_text(
                            "Messages this port's driver handed to FreeDAWm, counted before the input filter. \
                             If it stays at 0 while you play, the computer is not passing the device's MIDI on: press Rescan, \
                             switch the device off and on, or close other music programs.",
                        );
                    }
                    if p.revived > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "It stopped sending and was reopened ({}×). If it stays silent, switch it off and on.",
                                p.revived
                            ))
                            .small()
                            .color(t.rec),
                        );
                    }
                });
                // What it is called in the track input menus, and how late its messages arrive.
                let mut name = app.ui.midi_aliases.get(&p.name).cloned().unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut name).hint_text("Alias").desired_width(110.0)).changed() {
                    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
                    if name.trim().is_empty() {
                        app.ui.midi_aliases.remove(&p.name);
                    } else {
                        app.ui.midi_aliases.insert(p.name.clone(), name);
                    }
                }
                let mut ms = app.ui.midi_offsets.get(&p.name).copied().unwrap_or(0.0);
                if ui
                    .add(egui::DragValue::new(&mut ms).range(0.0..=100.0).speed(0.1).suffix(" ms"))
                    .on_hover_text("This device's messages arrive this late: they are placed that much earlier")
                    .changed()
                {
                    if ms > 0.0 {
                        app.ui.midi_offsets.insert(p.name.clone(), ms);
                    } else {
                        app.ui.midi_offsets.remove(&p.name);
                    }
                }
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
        let names: Vec<String> = outs.iter().map(|p| p.name.clone()).collect();
        studio_setup(app, ui, &names);
        sync_row(app, ui, &outs);
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
        let mut thin = app.engine.session().edit.flag(soundcraft_engine::midi_record::THIN_FLAG);
        if ui
            .checkbox(&mut thin, "Thin controllers")
            .on_hover_text("Takes keep only the controller, pitch bend and pressure points that shape a move; the ones between become ramps")
            .changed()
            && let Err(e) = app.run("setup.midi_input_filter", json!({"thin": thin}))
        {
            app.ui.status = e;
        }
        let mut auto = app.engine.session().edit.flag(soundcraft_engine::auto_arm::FLAG);
        if ui
            .checkbox(&mut auto, "Auto-arm the selected MIDI track")
            .on_hover_text("Selecting an instrument or MIDI track arms it, so it is heard and ready to record; the one it armed before is disarmed")
            .changed()
            && let Err(e) = app.run("options.midi_auto_arm", json!({"value": auto}))
        {
            app.ui.status = e;
        }
        ui.checkbox(&mut app.ui.midi_thru, "MIDI Thru")
            .on_hover_text("Record-enabled instrument and MIDI tracks play what comes in (input-monitored tracks always do)");
        ui.horizontal(|ui| {
            if ui.button("Rescan").on_hover_text("Look for ports again, and close and reopen the open inputs").clicked() {
                if let Some(m) = &mut app.midi_inputs {
                    m.reopen();
                }
                if let Some(m) = &mut app.midi_outputs {
                    m.rescan();
                }
            }
            // The audio engine takes what the ports send to the tracks; while it does not, nothing
            // played is heard or recorded.
            if app.player.is_some()
                && ports.iter().any(|p| p.open)
                && app.midi_hub.as_ref().is_some_and(|h| h.since_drain().is_none_or(|d| d > Duration::from_secs(2)))
            {
                ui.label(egui::RichText::new(ENGINE_IDLE).small().color(t.rec));
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
    fn an_alias_names_a_port_in_the_input_menu_and_an_offset_reaches_it() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let fake = with_fake(&mut app, &["USB MIDI Device", "USB MIDI Device"]);
        app.ui.midi_aliases.insert("USB MIDI Device #2".into(), "Drum Pads".into());
        app.ui.midi_offsets.insert("USB MIDI Device #2".into(), 3.0);
        let labels: Vec<(String, Option<String>)> = input_choices(&app, None).into_iter().map(|c| (c.0, c.1)).collect();
        assert!(labels.contains(&("MIDI: USB MIDI Device".to_string(), Some("USB MIDI Device".to_string()))));
        assert!(labels.contains(&("MIDI: Drum Pads".to_string(), Some("USB MIDI Device #2".to_string()))), "{labels:?}");
        tick(&mut app);
        // Saved per computer by port name.
        let json = serde_json::to_value(&app.ui).unwrap();
        assert_eq!(json["midi.aliases"]["USB MIDI Device #2"], "Drum Pads");
        assert_eq!(json["midi.offsets"]["USB MIDI Device #2"], 3.0);
        assert!(fake.is_open("USB MIDI Device #2"));
    }

    /// The studio setup names devices on an output port: a track on that port and channel plays
    /// the one set up there (else the one taking every channel); kept per computer by port name;
    /// a device without a name is not offered.
    #[test]
    fn the_studio_setup_names_the_devices_on_an_output_port() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let dev = |name: &str, channel| StudioDevice { name: name.into(), channel };
        app.ui.midi_studio.insert("MIDI Out 1".into(), vec![dev("JV-1080", 0), dev("Drum Box", 10), dev(" ", 3)]);
        let names: Vec<String> = studio_devices(&app, "MIDI Out 1").into_iter().map(studio_label).collect();
        assert_eq!(names, vec!["JV-1080".to_string(), "Drum Box (ch 10)".to_string()]);
        assert_eq!(studio_device_for(&app, "MIDI Out 1", 10).map(|d| d.name.as_str()), Some("Drum Box"));
        assert_eq!(studio_device_for(&app, "MIDI Out 1", 2).map(|d| d.name.as_str()), Some("JV-1080"));
        assert!(studio_devices(&app, "MIDI Out 2").is_empty());
        let json = serde_json::to_value(&app.ui).unwrap();
        assert_eq!(json["midi.studio"]["MIDI Out 1"][1]["name"], "Drum Box");
        assert_eq!(json["midi.studio"]["MIDI Out 1"][1]["channel"], 10);
        // Settings saved before it existed have none.
        let back: crate::UiState = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(back.midi_studio.is_empty());
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
        app.midi_rec.push(RecordedMidi { at: 1, port: 0, msg: [0x90, 1, 1], pass: 0, sysex: None });
        start_take(&mut app);
        assert!(app.midi_rec.is_empty());
        app.midi_rec = vec![
            RecordedMidi { at: 1_000, port: 0, msg: [0x90, 60, 100], pass: 0, sysex: None },
            RecordedMidi { at: 9_000, port: 0, msg: [0x80, 60, 0], pass: 0, sysex: None },
            RecordedMidi { at: 2_000, port: TYPING_SLOT as u8, msg: [0x90, 72, 100], pass: 0, sysex: None },
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
            pre: 0,
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
    fn changing_a_tracks_input_leaves_the_port_open_and_what_is_played_is_recorded() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let fake = with_fake(&mut app, &["XPIANOGT", "StudioRack", "SG Device I/O"]);
        let hub = app.midi_hub.clone().unwrap();
        let inst = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let midi = app.engine.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.ui.midi_thru = true;
        tick(&mut app);
        assert_eq!(fake.opens("XPIANOGT"), 1);
        // Inputs changed back and forth, armed and disarmed, frames between: no port is touched.
        for input in ["midi:all", "midi:StudioRack", "midi:XPIANOGT", "midi:Musical Typing", "midi:XPIANOGT"] {
            app.run("track.input", json!({"track": inst, "input": input})).unwrap();
            app.run("track.input", json!({"track": midi, "input": input, "channel": 1})).unwrap();
            app.run("mix.record_arm", json!({"tracks": [inst, midi], "value": true})).unwrap();
            tick(&mut app);
            app.run("mix.record_arm", json!({"tracks": [inst, midi], "value": false})).unwrap();
            tick(&mut app);
        }
        for p in ["XPIANOGT", "StudioRack", "SG Device I/O"] {
            assert_eq!(fake.opens(p), 1, "{p} opened once, never closed");
            assert!(fake.is_open(p));
        }
        app.run("mix.record_arm", json!({"tracks": [inst, midi], "value": true})).unwrap();
        start_take(&mut app);
        // The piano plays; the audio thread drains the port's ring and keeps it for the take.
        assert!(fake.send("XPIANOGT", &[0x90, 60, 80]));
        assert!(fake.send("XPIANOGT", &[0x80, 60, 0]));
        let d = app.midi_inputs.as_ref().and_then(|m| m.diag("XPIANOGT")).unwrap();
        assert_eq!(d.received, 2);
        assert_eq!(diag_line(&d), "2 received, 0 passed on; last: Note off C3, ch 1, just now");
        assert!(app.midi_inputs.as_ref().and_then(|m| m.since_activity("XPIANOGT")).is_some(), "the light is lit");
        let mut live = soundcraft_mix::LiveMidi::new();
        let now = hub.now_us();
        hub.drain_into(&mut live, now, 480, 48_000.0);
        assert_eq!(live.events().len(), 2);
        assert_eq!(app.midi_inputs.as_ref().and_then(|m| m.diag("XPIANOGT")).map(|d| d.taken), Some(2));
        assert!(hub.since_drain().is_some());
        hub.record_block(&live, 24_000, now, 48_000.0, 0, 0);
        collect(&mut app);
        assert_eq!(app.midi_rec.len(), 2);
        let plan = soundcraft_engine::io::Recording {
            start: 0,
            pass: None,
            punch: None,
            rate: 0,
            takes: Vec::new(),
            latency: 0,
            loop_from: None,
            punches: Vec::new(),
            pre: 0,
        };
        let ids = finish_take(&mut app, &plan, 48_000);
        assert_eq!(ids.len(), 2, "a clip on each armed track");
        for t in [inst, midi] {
            let tr = app.engine.session().track(soundcraft_model::TrackId(t)).unwrap();
            let notes = tr.clips().first().and_then(|c| c.midi_playing()).map(|m| m.notes.len());
            assert_eq!(notes, Some(1), "track {t}");
        }
    }

    #[test]
    fn capture_midi_makes_a_take_of_what_was_just_played() {
        use soundcraft_playback::midi_in::{RecordedMidi, TYPING_SLOT};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": t, "input": "midi:Musical Typing"})).unwrap();
        assert!(app.run("ui.capture_midi", json!({})).is_err(), "nothing played yet");
        // Played (not recorded) a little into bar 2 at 120 bpm: a bar is 96,000 samples.
        let note = |at: i64, msg: [u8; 3]| RecordedMidi { at, port: TYPING_SLOT as u8, msg, pass: 0, sysex: None };
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
        let note = |at: i64, msg: [u8; 3], pass: u16| RecordedMidi { at, port: TYPING_SLOT as u8, msg, pass, sysex: None };
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
        let note = |at: i64, pitch: u8| RecordedMidi { at, port: TYPING_SLOT as u8, msg: [0x90, pitch, 100], pass: 0, sysex: None };
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
            pre: 0,
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
            pre: 0,
        };
        app.take_plan = Some(plan.clone());
        let note = |at: i64, pitch: u8, pass: u16| RecordedMidi { at, port: TYPING_SLOT as u8, msg: [0x90, pitch, 100], pass, sysex: None };
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
    fn an_articulation_is_set_in_the_clip_at_the_insertion_point() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let c = app.run("midi.new_clip", json!({"track": t, "start": 48_000, "end": 144_000})).unwrap()["clip"].as_u64().unwrap();
        app.run("track.articulations", json!({"tracks": [t], "add": {"name": "Legato", "keyswitch": 24}})).unwrap();
        app.run("track.articulations", json!({"tracks": [t], "add": {"name": "Staccato", "keyswitch": 25}})).unwrap();
        assert!(set_articulation_here(&mut app, TrackId(t), 3, 1).is_err(), "the insertion point is outside the clip");
        app.engine.session_mut().edit.selection = soundcraft_time::Range::point(72_000);
        set_articulation_here(&mut app, TrackId(t), 3, 1).unwrap();
        let (_, clip) = app.engine.session().find_clip(soundcraft_model::ClipId(c)).unwrap();
        let ev = &clip.midi_playing().unwrap().ctrls;
        assert_eq!(
            ev.iter().map(|e| (e.tick, e.kind, e.value)).collect::<Vec<_>>(),
            vec![(960, soundcraft_midi::CtrlKind::Cc(3), 1)],
            "a beat into the clip"
        );
    }

    #[test]
    fn held_keys_are_erased_from_every_pass_while_loop_recording() {
        use soundcraft_playback::midi_in::{RecordedMidi, TYPING_SLOT};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let t = app.engine.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("track.input", json!({"track": t, "input": "midi:Musical Typing"})).unwrap();
        app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
        assert!(app.run("ui.midi_erase_held", json!({})).is_err(), "not while stopped");
        start_take(&mut app);
        app.engine.transport.recording = true;
        let plan = soundcraft_engine::io::Recording {
            start: 0,
            pass: Some(96_000),
            punch: None,
            rate: 0,
            takes: Vec::new(),
            latency: 0,
            loop_from: Some(0),
            punches: Vec::new(),
            pre: 0,
        };
        app.take_plan = Some(plan.clone());
        let ev = |at: i64, msg: [u8; 3], pass: u16| RecordedMidi { at, port: TYPING_SLOT as u8, msg, pass, sysex: None };
        // Kick and hat in pass 1, the hat again in pass 2, then the hat key held down in pass 3.
        app.midi_rec = vec![
            ev(1_000, [0x90, 36, 100], 0),
            ev(2_000, [0x80, 36, 0], 0),
            ev(10_000, [0x90, 42, 100], 0),
            ev(11_000, [0x80, 42, 0], 0),
            ev(20_000, [0x90, 42, 90], 1),
            ev(21_000, [0x80, 42, 0], 1),
            ev(50_000, [0x90, 42, 80], 2),
        ];
        show_passes(&mut app, 2);
        let pitches = |app: &SoundApp| -> Vec<u8> {
            let tr = app.engine.session().track(TrackId(t)).unwrap();
            tr.clips().iter().flat_map(|c| c.midi_playing().unwrap().notes.iter().map(|n| n.pitch)).collect()
        };
        assert_eq!(pitches(&app), vec![36, 42, 42]);
        let r = app.run("ui.midi_erase_held", json!({})).unwrap();
        assert_eq!(r["erased"][0]["pitch"], 42);
        assert_eq!(pitches(&app), vec![36], "the held key's notes are gone from what plays");
        // Let go: its note-off has no note; nothing comes back at Stop.
        app.midi_rec.push(ev(60_000, [0x80, 42, 0], 2));
        assert!(app.run("ui.midi_erase_held", json!({})).is_err(), "nothing held now");
        lift_preview(&mut app);
        app.engine.transport.recording = false;
        finish_take(&mut app, &plan, 250_000);
        assert_eq!(pitches(&app), vec![36]);
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
