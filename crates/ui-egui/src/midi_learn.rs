//! MIDI learn: right-click a fader, a pan knob, a send level or a plug-in knob › Learn MIDI
//! Controller, move a knob or press a key on a MIDI controller, and it moves that control from
//! then on. Setup › MIDI Mappings… lists them (range, invert, delete) and learns transport keys.
//!
//! - Parameter mappings are saved with the session (`Session::midi_maps`, the `midi.map_*`
//!   commands): they name its tracks and plug-ins, which mean nothing in another song.
//! - Transport mappings (Play, Stop, Record, a footswitch that punches in and out, …) are app
//!   settings (`ui.json`: `midi.transport_maps`): they belong to the controller on this desk,
//!   whatever the song. MIDI Machine Control (MMC) from any input runs the transport without
//!   learning.
//! - A mapped message never plays or records: the hub keeps it out of the ports' rings
//!   (`soundcraft_playback::midi_in::MidiHub::set_claims`) and hands it here instead, as it
//!   hands over what is moved while learning.

use crate::SoundApp;
use crate::theme::Tokens;
use serde_json::json;
use soundcraft_engine::cmd::midi_learn;
use soundcraft_model::{AutoParam, MidiMap, MidiSource, MidiSourceKind, MidiTarget, TrackId, TransportAction};

/// The control a right-click opened the learn menu on, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnMenu {
    pub target: MidiTarget,
    pub at: egui::Pos2,
}

/// The port name of each input slot.
fn slot_names(app: &SoundApp) -> Vec<(usize, String)> {
    let mut v: Vec<(usize, String)> = app.midi_inputs.as_ref().map(|m| m.ports().into_iter().map(|p| (p.slot, p.name)).collect()).unwrap_or_default();
    v.push((soundcraft_playback::midi_in::TYPING_SLOT, soundcraft_playback::midi_in::TYPING_NAME.to_string()));
    v
}

/// The (slot, kind, channel, number) every mapping claims: on its port's slot, or on every slot
/// for "any port".
fn claims(app: &SoundApp) -> Vec<(usize, u8, u8, u8)> {
    let names = slot_names(app);
    let maps = app.engine.session().midi_maps.iter().chain(app.ui.midi_transport_maps.iter());
    let mut out = Vec::new();
    for m in maps {
        let kind = match m.source.kind {
            MidiSourceKind::Cc => 0,
            MidiSourceKind::Note => 1,
        };
        for (slot, name) in &names {
            if m.source.port.as_ref().is_none_or(|p| p == name) {
                out.push((*slot, kind, m.source.channel, m.source.number));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Every frame (from `logic`): the hub knows what is claimed and whether a control is being
/// learnt; what it handed over learns a mapping, moves a parameter or runs the transport.
pub fn tick(app: &mut SoundApp) {
    let Some(hub) = app.midi_hub.clone() else { return };
    hub.set_learning(app.learning.is_some());
    let claims = claims(app);
    if claims != app.learn_claims {
        hub.set_claims(&claims);
        app.learn_claims = claims;
    }
    let mut got = Vec::new();
    hub.take_control(&mut got);
    if got.is_empty() {
        return;
    }
    let names = slot_names(app);
    let name = |slot: u8| names.iter().find(|(s, _)| *s == usize::from(slot)).map(|(_, n)| n.clone()).unwrap_or_default();
    for c in got {
        let port = name(c.slot);
        if let Some(cmd) = c.mmc.and_then(|m| midi_learn::mmc_command(&m)) {
            let _ = app.run(cmd, json!({}));
            continue;
        }
        if c.learn {
            if let Some(target) = app.learning.take()
                && let Some(source) = MidiSource::of(&port, c.msg)
            {
                learn(app, source, target);
            }
            continue;
        }
        handle(app, &port, c.msg);
    }
}

/// `source` now moves `target`.
pub fn learn(app: &mut SoundApp, source: MidiSource, target: MidiTarget) {
    let label = source.label();
    match &target {
        MidiTarget::Param { track, param } => {
            let kind = if source.kind == MidiSourceKind::Cc { "cc" } else { "note" };
            let src = json!({"kind": kind, "channel": source.channel, "number": source.number, "port": source.port});
            match app.run("midi.map_add", json!({"source": src, "track": track.0, "param": param.id()})) {
                Ok(_) => app.ui.status = format!("{label} → {}", midi_learn::target_label(app.engine.session(), &target)),
                Err(e) => app.ui.status = e,
            }
        }
        MidiTarget::Transport(a) => {
            app.ui.midi_transport_maps.retain(|m| m.source != source);
            app.ui.midi_transport_maps.push(MidiMap::new(source, target.clone()));
            app.ui.status = format!("{label} → {}", a.label());
        }
    }
}

/// A claimed message from `port`: the mappings it moves.
pub fn handle(app: &mut SoundApp, port: &str, msg: [u8; 3]) {
    let value = match msg[0] & 0xF0 {
        0x80 => 0,
        _ => msg[2],
    };
    for i in midi_learn::matching(app.engine.session(), port, msg) {
        let Some(m) = app.engine.session().midi_maps.get(i).cloned() else { continue };
        if let MidiTarget::Param { track, param } = &m.target {
            // A knob held still is not let go; it stays touched for a moment after each move.
            app.engine.automation_changed(*track, param.clone());
        }
        if let Err(e) = midi_learn::apply(&mut app.engine, &m, value) {
            app.ui.status = e.to_string();
        }
    }
    let maps: Vec<MidiMap> = app.ui.midi_transport_maps.iter().filter(|m| m.source.matches(port, msg)).cloned().collect();
    for m in maps {
        let MidiTarget::Transport(a) = m.target else { continue };
        // A key or a footswitch acts as it goes down (a CC switch crossing the middle).
        let key = (port.to_string(), msg[0] & 0x0F, msg[1], m.source.kind == MidiSourceKind::Cc);
        let down = m.position(value) >= 0.5;
        let was = app.learn_switches.get(&key).copied().unwrap_or(false);
        app.learn_switches.insert(key, down);
        if down && !was {
            let _ = app.run(midi_learn::transport_command(a), json!({}));
        }
    }
}

/// A right-click on `rect` opens the learn menu for `target` (for controls that have no context
/// menu of their own).
pub fn context_at(app: &mut SoundApp, ui: &egui::Ui, rect: egui::Rect, target: MidiTarget) {
    let (clicked, at) = ui.input(|i| (i.pointer.secondary_clicked(), i.pointer.interact_pos()));
    if clicked && let Some(p) = at.filter(|p| rect.contains(*p) && ui.is_rect_visible(rect)) {
        app.learn_menu = Some(LearnMenu { target, at: p });
    }
}

/// The learn items for `target`, inside an existing context menu.
pub fn menu_items(app: &mut SoundApp, ui: &mut egui::Ui, target: MidiTarget) {
    let mapped: Vec<usize> = app.engine.session().midi_maps.iter().enumerate().filter(|(_, m)| m.target == target).map(|(i, _)| i).collect();
    if ui.button("Learn MIDI Controller").on_hover_text("Then move a knob or press a key on your MIDI controller").clicked() {
        app.ui.status = format!("Move a MIDI control for {}…", midi_learn::target_label(app.engine.session(), &target));
        app.learning = Some(target.clone());
        ui.close();
    }
    if !mapped.is_empty() && ui.button("Forget MIDI Controller").clicked() {
        for i in mapped.into_iter().rev() {
            let _ = app.run("midi.map_remove", json!({"index": i}));
        }
        ui.close();
    }
    if ui.button("MIDI Mappings…").clicked() {
        app.ui.show_midi_maps = true;
        ui.close();
    }
}

/// The learn menu a right-click opened ([`context_at`]), and the mapping list window.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if let Some(menu) = app.learn_menu.clone() {
        let mut close = false;
        let area = egui::Area::new(egui::Id::new("midi-learn-menu")).order(egui::Order::Foreground).fixed_pos(menu.at).show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                let before = app.learning.clone();
                let maps = app.engine.session().midi_maps.len();
                let shown = app.ui.show_midi_maps;
                menu_items(app, ui, menu.target.clone());
                close = app.learning != before || app.engine.session().midi_maps.len() != maps || app.ui.show_midi_maps != shown;
            });
        });
        let outside = ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() && !area.response.hovered();
        if close || (outside && ctx.input(|i| i.pointer.primary_pressed())) {
            app.learn_menu = None;
        }
    }
    let mut open = app.ui.show_midi_maps;
    if !open {
        return;
    }
    let t = Tokens::current();
    egui::Window::new("MIDI Mappings").open(&mut open).default_size(egui::vec2(560.0, 300.0)).resizable(true).show(ctx, |ui| {
        if let Some(target) = &app.learning {
            ui.label(
                egui::RichText::new(format!("Learning: move a MIDI control for {}", midi_learn::target_label(app.engine.session(), target))).strong(),
            );
            if ui.button("Cancel").clicked() {
                app.learning = None;
            }
            ui.separator();
        }
        ui.strong("This session");
        ui.label(egui::RichText::new("Saved with the session: they name its tracks and plug-ins.").small().color(t.text_dim));
        let maps = app.engine.session().midi_maps.clone();
        if maps.is_empty() {
            ui.label(
                egui::RichText::new("None yet: right-click a fader, pan knob, send level or plug-in knob › Learn MIDI Controller.")
                    .small()
                    .color(t.text_dim),
            );
        }
        let mut edits: Vec<(&str, serde_json::Value)> = Vec::new();
        egui::Grid::new("midi-maps").num_columns(6).spacing(egui::vec2(10.0, 4.0)).striped(true).show(ui, |ui| {
            for (i, m) in maps.iter().enumerate() {
                ui.label(m.source.label());
                ui.label(midi_learn::target_label(app.engine.session(), &m.target));
                let (mut lo, mut hi, mut inv) = (m.min * 100.0, m.max * 100.0, m.invert);
                if ui.add(egui::DragValue::new(&mut lo).range(0.0..=100.0).suffix("%").prefix("from ")).changed() {
                    edits.push(("midi.map_set", json!({"index": i, "min": lo / 100.0})));
                }
                if ui.add(egui::DragValue::new(&mut hi).range(0.0..=100.0).suffix("%").prefix("to ")).changed() {
                    edits.push(("midi.map_set", json!({"index": i, "max": hi / 100.0})));
                }
                if ui.checkbox(&mut inv, "Invert").changed() {
                    edits.push(("midi.map_set", json!({"index": i, "invert": inv})));
                }
                if ui.button("Delete").clicked() {
                    edits.push(("midi.map_remove", json!({"index": i})));
                }
                ui.end_row();
            }
        });
        for (id, p) in edits {
            if let Err(e) = app.run(id, p) {
                app.ui.status = e;
            }
        }
        ui.separator();
        ui.strong("Transport keys (this computer)");
        ui.label(
            egui::RichText::new(
                "Kept with the app's settings: the controller on this desk, whatever the song. MMC from any input runs the transport too.",
            )
            .small()
            .color(t.text_dim),
        );
        let mut remove = None;
        let mut transport = app.ui.midi_transport_maps.clone();
        egui::Grid::new("midi-transport-maps").num_columns(4).spacing(egui::vec2(10.0, 4.0)).striped(true).show(ui, |ui| {
            for (i, m) in transport.iter_mut().enumerate() {
                ui.label(m.source.label());
                ui.label(midi_learn::target_label(app.engine.session(), &m.target));
                ui.checkbox(&mut m.invert, "Invert");
                if ui.button("Delete").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove
            && i < transport.len()
        {
            transport.remove(i);
        }
        app.ui.midi_transport_maps = transport;
        ui.horizontal_wrapped(|ui| {
            ui.label("Learn a key for:");
            for a in TransportAction::ALL {
                if ui.button(a.label()).clicked() {
                    app.learning = Some(MidiTarget::Transport(a));
                    app.ui.status = format!("Press a key or footswitch for {}…", a.label());
                }
            }
        });
    });
    app.ui.show_midi_maps = open;
}

/// The fader's learn target.
pub fn volume(track: TrackId) -> MidiTarget {
    MidiTarget::Param { track, param: AutoParam::Volume }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_playback::midi_in::TYPING_SLOT;

    #[test]
    fn a_control_is_learnt_from_the_next_message_and_then_moves_without_playing() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let fake = crate::midi_devices::tests::with_fake(&mut app, &["Knobs"]);
        let t = TrackId(app.engine.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap()["tracks"][0].as_u64().unwrap());
        let hub = app.midi_hub.clone().unwrap();
        // Learn: the next CC is the knob.
        app.learning = Some(volume(t));
        tick(&mut app);
        assert!(fake.send("Knobs", &[0xB0, 21, 30]));
        tick(&mut app);
        assert!(app.learning.is_none());
        assert_eq!(app.engine.session().midi_maps.len(), 1);
        assert_eq!(app.engine.session().midi_maps[0].source.label(), "CC 21 ch 1 (Knobs)");
        // Now claimed: the knob moves the fader and never reaches the tracks (takes, Thru). (The
        // move that was learnt played, as anything played while learning does.)
        tick(&mut app);
        hub.discard();
        assert!(fake.send("Knobs", &[0xB0, 21, 127]));
        let mut live = soundcraft_mix::LiveMidi::new();
        hub.drain_into(&mut live, hub.now_us(), 512, 48_000.0);
        assert!(live.is_empty(), "a mapped message is not played or recorded");
        tick(&mut app);
        let db = app.engine.session().track(t).unwrap().mixer.volume_db;
        assert!((db - soundcraft_model::fader_pos_to_db(1.0)).abs() < 0.01, "{db}");
        // An unmapped CC still plays.
        assert!(fake.send("Knobs", &[0xB0, 22, 5]));
        hub.drain_into(&mut live, hub.now_us(), 512, 48_000.0);
        assert_eq!(live.events().len(), 1);
        // A transport key, learnt, runs Play as it goes down (once), not as it comes up.
        app.learning = Some(MidiTarget::Transport(TransportAction::Play));
        tick(&mut app);
        let typing = hub.typing();
        typing.send(&[0x90, 36, 100]);
        tick(&mut app);
        assert_eq!(app.ui.midi_transport_maps.len(), 1);
        assert_eq!(serde_json::to_value(&app.ui).unwrap()["midi.transport_maps"][0]["target"]["transport"], "play");
        app.engine.transport_requests.clear();
        tick(&mut app);
        typing.send(&[0x90, 36, 100]);
        typing.send(&[0x80, 36, 0]);
        tick(&mut app);
        assert_eq!(app.engine.transport_requests.len(), 1);
        let _ = TYPING_SLOT;
        // MMC Stop from any input.
        app.engine.transport_requests.clear();
        assert!(fake.send("Knobs", &[0xF0, 0x7F, 0x7F, 0x06, 0x01, 0xF7]));
        tick(&mut app);
        assert_eq!(app.engine.transport_requests.len(), 1);
    }
}
