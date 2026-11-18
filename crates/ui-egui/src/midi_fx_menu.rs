//! The MIDI Effects submenu of an instrument or MIDI track's input menu: the track's MIDI effects
//! ahead of its instrument (arpeggiator, chord, scale, note repeat, a script), each with its
//! settings (a script's in the Script Editor), a bypass, a move up or down and a remove, and Add for a new one (`track.midi_fx_add`,
//! `track.midi_fx_set`, `track.midi_fx_remove`; a drag is one undo step), and Record Effects'
//! Output (`track.midi_fx_record`): takes keep what the effects made rather than the keys.
//! A MIDI clip's context menu has the same submenu for the clip's own effects (`clips`).

use crate::SoundApp;
use crate::theme::Tokens;
use serde_json::{Value, json};
use soundcraft_model::{ARP_PATTERNS, CHORD_SHAPES, ClipId, MAX_MIDI_FX, MIDI_FX_RATES, MidiFx, MidiFxKind, ROOT_NAMES, SCALES, TrackId};

/// Whose effects the menu shows: a track's or a clip's own.
#[derive(Clone, Copy)]
enum Owner {
    Track(TrackId),
    Clip(ClipId),
}

impl Owner {
    /// The commands' parameter naming it.
    fn param(self) -> (&'static str, Value) {
        match self {
            Owner::Track(t) => ("tracks", json!([t.0])),
            Owner::Clip(c) => ("clips", json!([c.0])),
        }
    }

    /// `p` with the owner added.
    fn with(self, mut p: Value) -> Value {
        let (k, v) = self.param();
        if let Some(o) = p.as_object_mut() {
            o.insert(k.into(), v);
        }
        p
    }

    fn key(self) -> String {
        match self {
            Owner::Track(t) => format!("t{}", t.0),
            Owner::Clip(c) => format!("c{}", c.0),
        }
    }
}

/// What the menu asks for: a command, its parameters and, for a drag, the key that merges it into
/// one undo step.
type Ask = (&'static str, Value, Option<String>);

/// A submenu of named choices; picks `key = index`.
fn choice(ui: &mut egui::Ui, label: &str, names: &[&str], now: u8, key: &'static str, set: &mut Vec<(&'static str, Value)>) {
    let shown = names.get(usize::from(now)).copied().unwrap_or("");
    ui.menu_button(format!("{label}: {shown}"), |ui| {
        for (i, n) in names.iter().enumerate() {
            if ui.add(egui::Button::selectable(usize::from(now) == i, *n)).clicked() {
                set.push((key, json!(i)));
                ui.close();
            }
        }
    });
}

fn effect(ui: &mut egui::Ui, i: usize, n: usize, fx: &MidiFx, asks: &mut Vec<Ask>, owner: Owner) {
    let mut set: Vec<(&'static str, Value)> = Vec::new();
    let mut drag: Vec<(&'static str, Value)> = Vec::new();
    ui.horizontal(|ui| {
        let mut on = !fx.bypass;
        if ui.checkbox(&mut on, "").on_hover_text("On (off: bypassed, the notes pass as played)").changed() {
            set.push(("bypass", json!(!on)));
        }
        ui.strong(fx.kind.name());
        if ui.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text("Earlier in the list").clicked() {
            set.push(("to", json!(i - 1)));
        }
        if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).on_hover_text("Later in the list").clicked() {
            set.push(("to", json!(i + 1)));
        }
        if ui.add(egui::Button::new("Remove").small()).clicked() {
            asks.push(("track.midi_fx_remove", owner.with(json!({"index": i})), None));
        }
    });
    let rates: Vec<&str> = MIDI_FX_RATES.iter().map(|r| r.0).collect();
    match fx.kind {
        MidiFxKind::Arpeggiator => {
            choice(ui, "Rate", &rates, fx.rate, "rate", &mut set);
            choice(ui, "Pattern", &ARP_PATTERNS, fx.pattern, "pattern", &mut set);
            let mut oct = fx.octaves;
            ui.horizontal(|ui| {
                ui.label("Octaves");
                if ui.add(egui::DragValue::new(&mut oct).range(1..=4)).changed() {
                    drag.push(("octaves", json!(oct)));
                }
            });
            let mut latch = fx.latch;
            if ui.checkbox(&mut latch, "Latch").on_hover_text("Keeps playing the last keys until new ones are pressed").changed() {
                set.push(("latch", json!(latch)));
            }
        }
        MidiFxKind::Chord => {
            let shapes: Vec<&str> = CHORD_SHAPES.iter().map(|c| c.0).collect();
            choice(ui, "Chord", &shapes, fx.chord, "chord", &mut set);
        }
        MidiFxKind::Scale => {
            choice(ui, "Follow", &soundcraft_model::FOLLOW_MODES, fx.follow, "follow", &mut set);
            choice(ui, "Root", &ROOT_NAMES, fx.root, "root", &mut set);
            let scales: Vec<&str> = SCALES.iter().map(|c| c.0).collect();
            choice(ui, "Scale", &scales, fx.scale, "scale", &mut set);
            let mut drop = fx.drop;
            if ui.checkbox(&mut drop, "Leave out notes outside it").on_hover_text("Off: moved to the nearest note in the scale").changed() {
                set.push(("drop", json!(drop)));
            }
        }
        MidiFxKind::NoteRepeat => {
            choice(ui, "Rate", &rates, fx.rate, "rate", &mut set);
        }
        MidiFxKind::Script => {
            if let Owner::Track(t) = owner
                && ui.button("Edit Script...").on_hover_text("The script's text, its parameters and the examples").clicked()
            {
                asks.push(("ui.script_editor", json!({"track": t.0, "index": i}), None));
                ui.close();
            }
        }
    }
    if matches!(fx.kind, MidiFxKind::Arpeggiator | MidiFxKind::NoteRepeat) {
        let mut gate = fx.gate;
        ui.horizontal(|ui| {
            ui.label("Gate");
            if ui.add(egui::DragValue::new(&mut gate).range(5..=100).suffix(" %")).changed() {
                drag.push(("gate", json!(gate)));
            }
        });
    }
    let params = |k: &str, v: Value| {
        let mut p = serde_json::Map::new();
        let (ok, ov) = owner.param();
        p.insert(ok.into(), ov);
        p.insert("index".into(), json!(i));
        p.insert(k.into(), v);
        Value::Object(p)
    };
    for (k, v) in set {
        asks.push(("track.midi_fx_set", params(k, v), None));
    }
    for (k, v) in drag {
        let key = format!("midi-fx.{}.{i}.{k}", owner.key());
        asks.push(("track.midi_fx_set", params(k, v), Some(key)));
    }
}

/// The MIDI Effects submenu (instrument and MIDI tracks only).
pub(crate) fn menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    let Some(track) = app.engine.session().track(id) else { return };
    if !track.kind.is_midi() {
        return;
    }
    let list = track.midi_fx.clone();
    let mut print = track.midi_fx_record;
    let title = if list.is_empty() { "MIDI Effects".to_string() } else { format!("MIDI Effects ({})", list.len()) };
    let mut asks: Vec<Ask> = Vec::new();
    ui.menu_button(title, |ui| {
        for (i, fx) in list.iter().enumerate() {
            effect(ui, i, list.len(), fx, &mut asks, Owner::Track(id));
            ui.separator();
        }
        if list.len() < MAX_MIDI_FX {
            ui.menu_button("Add", |ui| {
                for k in MidiFxKind::ALL {
                    if ui.button(k.name()).clicked() {
                        asks.push(("track.midi_fx_add", json!({"tracks": [id.0], "kind": k.key()}), None));
                        ui.close();
                    }
                }
            });
        }
        if ui
            .checkbox(&mut print, "Record Effects' Output")
            .on_hover_text("A take keeps what the effects made (and plays past them); off, the keys as played")
            .changed()
        {
            asks.push(("track.midi_fx_record", json!({"tracks": [id.0], "value": print}), None));
        }
        ui.label(
            egui::RichText::new("In list order, on clips, live input and routed MIDI, before the instrument; a take keeps the keys as played unless Record Effects' Output is on")
                .small()
                .color(Tokens::current().text_dim),
        );
    });
    run(app, asks);
}

/// The MIDI Effects submenu of a MIDI clip's context menu: the clip's own effects, run on its
/// notes only, ahead of the instrument (the track's effects pass what they make by).
pub(crate) fn clip_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: ClipId) {
    let Some((_, clip)) = app.engine.session().find_clip(id) else { return };
    if clip.midi_playing().is_none() {
        return;
    }
    let list = clip.midi_fx.clone();
    let title = if list.is_empty() { "MIDI Effects on Clip".to_string() } else { format!("MIDI Effects on Clip ({})", list.len()) };
    let mut asks: Vec<Ask> = Vec::new();
    ui.menu_button(title, |ui| {
        for (i, fx) in list.iter().enumerate() {
            effect(ui, i, list.len(), fx, &mut asks, Owner::Clip(id));
            ui.separator();
        }
        if list.len() < MAX_MIDI_FX {
            ui.menu_button("Add", |ui| {
                // (A script goes on the track, not a clip.)
                for k in MidiFxKind::ALL.into_iter().filter(|k| *k != MidiFxKind::Script) {
                    if ui.button(k.name()).clicked() {
                        asks.push(("track.midi_fx_add", json!({"clips": [id.0], "kind": k.key()}), None));
                        ui.close();
                    }
                }
            });
        }
        ui.label(
            egui::RichText::new("On this clip's notes only, in list order; what they make goes to the instrument past the track's effects")
                .small()
                .color(Tokens::current().text_dim),
        );
    });
    run(app, asks);
}

fn run(app: &mut SoundApp, asks: Vec<Ask>) {
    for (cmd, p, key) in asks {
        if cmd == "ui.script_editor" {
            let t = TrackId(p["track"].as_u64().unwrap_or(0));
            crate::script_editor::open(app, t, p["index"].as_u64().map_or(0, |i| i as usize));
            continue;
        }
        let r = match key {
            Some(k) => app.engine.execute_merged(cmd, &p, &k).map(|_| ()).map_err(|e| e.to_string()),
            None => app.run(cmd, p).map(|_| ()),
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
}
