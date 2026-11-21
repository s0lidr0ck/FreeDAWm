//! The MIDI Event List (Window › Editors › MIDI Event List): every event of the edited MIDI clip, notes,
//! controllers (CC, pitch bend, aftertouch, poly pressure, program change) and SysEx (its bytes
//! as hex, typed anew to change them), in time order, with
//! typed values to change, a row to insert a new event, and filters by kind and channel.
//! Each change is one undo step (a drag on a value is one).

use crate::SoundApp;
use crate::theme::mono;
use serde_json::json;
use soundcraft_engine::cmd::midi_ctrl::{lane_id, lane_range};
use soundcraft_midi::{CtrlKind, Sequence};
use soundcraft_model::{ClipContent, ClipId};

/// What the list shows and the kind the Insert row adds.
#[derive(Debug, Clone, PartialEq)]
pub struct EventListState {
    pub notes: bool,
    pub ccs: bool,
    pub bend: bool,
    pub pressure: bool,
    pub program: bool,
    pub sysex: bool,
    /// Only this channel (0-15); `None`: every channel.
    pub channel: Option<u8>,
    /// The Insert row: its kind (`note`, `cc`, `pitchbend`, `aftertouch`, `polyat`, `program`,
    /// `sysex`),
    /// number (CC number, or a poly pressure's key) and channel.
    pub insert_kind: String,
    pub insert_number: u8,
    pub insert_channel: u8,
    /// A position being changed: the field's key and its value, written when the drag or the
    /// typing ends (a position moves its row, so it is not written frame by frame).
    pub pending: Option<(String, i64)>,
    /// A SysEx's bytes being typed: the field's key and its text, written when the typing ends.
    pub pending_bytes: Option<(String, String)>,
}

impl Default for EventListState {
    fn default() -> Self {
        EventListState {
            notes: true,
            ccs: true,
            bend: true,
            pressure: true,
            program: true,
            sysex: true,
            channel: None,
            insert_kind: "note".into(),
            insert_number: 1,
            insert_channel: 0,
            pending: None,
            pending_bytes: None,
        }
    }
}

/// A row of the list: a note, a controller event or a SysEx, by its index in the clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Note(usize),
    Ctrl(usize),
    SysEx(usize),
}

/// The SysEx the Insert row adds: General MIDI System On.
pub const INSERTED_SYSEX: &str = "F0 7E 7F 09 01 F7";

/// The rows `f` shows of `seq`, in time order (at one tick: SysEx, as it plays first, then
/// notes, then controllers), hidden events (before the clip's start) left out. A SysEx has no
/// channel: the channel filter leaves it in.
pub fn rows(seq: &Sequence, f: &EventListState) -> Vec<(i64, Row)> {
    let ch = |c: u8| f.channel.is_none_or(|x| x == c);
    let mut v: Vec<(i64, Row)> = Vec::new();
    if f.notes {
        v.extend(seq.notes.iter().enumerate().filter(|(_, n)| n.start >= 0 && ch(n.channel)).map(|(i, n)| (n.start, Row::Note(i))));
    }
    v.extend(seq.ctrls.iter().enumerate().filter(|(_, c)| c.tick >= 0 && ch(c.channel)).filter_map(|(i, c)| {
        let shown = match c.kind {
            CtrlKind::Cc(_) => f.ccs,
            CtrlKind::PitchBend => f.bend,
            CtrlKind::ChannelPressure | CtrlKind::PolyPressure(_) => f.pressure,
            CtrlKind::Program => f.program,
        };
        shown.then_some((c.tick, Row::Ctrl(i)))
    }));
    if f.sysex {
        v.extend(seq.sysex.iter().enumerate().filter(|(_, x)| x.tick >= 0).map(|(i, x)| (x.tick, Row::SysEx(i))));
    }
    v.sort_by_key(|(t, r)| {
        (
            *t,
            match r {
                Row::SysEx(_) => 0,
                Row::Note(_) => 1,
                Row::Ctrl(_) => 2,
            },
        )
    });
    v
}

/// The kind column's text for a controller event.
pub fn kind_text(k: CtrlKind) -> String {
    match k {
        CtrlKind::Cc(n) => format!("CC {n}"),
        CtrlKind::PitchBend => "Pitch Bend".into(),
        CtrlKind::ChannelPressure => "Aftertouch".into(),
        CtrlKind::PolyPressure(p) => format!("Poly Pressure {}", soundcraft_midi::note_name(p.min(127))),
        CtrlKind::Program => "Program".into(),
    }
}

/// The lane id the Insert row adds to.
fn insert_lane(f: &EventListState) -> Option<String> {
    match f.insert_kind.as_str() {
        "cc" => lane_id(CtrlKind::Cc(f.insert_number.min(127))),
        "pitchbend" => lane_id(CtrlKind::PitchBend),
        "aftertouch" => lane_id(CtrlKind::ChannelPressure),
        "polyat" => lane_id(CtrlKind::PolyPressure(f.insert_number.min(127))),
        "program" => lane_id(CtrlKind::Program),
        _ => None,
    }
}

/// Runs `id` as one undo step per gesture: a drag's frames join the first.
fn edit(app: &mut SoundApp, id: &str, p: serde_json::Value, key: &str, r: &egui::Response) {
    if r.changed()
        && let Err(e) = app.engine.execute_merged(id, &p, key)
    {
        app.ui.status = e.to_string();
    }
    if r.drag_stopped() || r.lost_focus() {
        app.engine.end_merge();
    }
}

/// A position field: changed freely, written once when the drag or the typing ends.
fn deferred(app: &mut SoundApp, ui: &mut egui::Ui, key: &str, current: i64) -> Option<i64> {
    let mut v = match &app.midi.event_list.pending {
        Some((k, x)) if k == key => *x,
        _ => current,
    };
    let r = ui.add(egui::DragValue::new(&mut v).range(0..=i64::MAX / 4));
    if r.changed() {
        app.midi.event_list.pending = Some((key.to_string(), v));
    }
    if (r.drag_stopped() || r.lost_focus())
        && let Some((k, x)) = app.midi.event_list.pending.clone()
        && k == key
    {
        app.midi.event_list.pending = None;
        return (x != current).then_some(x);
    }
    None
}

pub fn show(app: &mut SoundApp, ui: &mut egui::Ui) {
    let Some(cid) = crate::midi_editor::target_clip(app) else {
        ui.label("Select a MIDI clip.");
        return;
    };
    let (seq, base) = {
        let s = app.engine.session();
        match s.find_clip(cid) {
            Some((_, c)) => match &c.content {
                ClipContent::Midi { sequence } => (sequence.clone(), s.tempo.samples_to_ticks(c.start, s.sample_rate)),
                _ => return,
            },
            None => return,
        }
    };
    filters(app, ui);
    insert_row(app, ui, cid);
    ui.separator();
    let shown = rows(&seq, &app.midi.event_list);
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("evl").num_columns(8).striped(true).show(ui, |ui| {
            for h in ["Position", "Ticks", "Kind", "Ch", "Note / No.", "Value", "Length", ""] {
                ui.label(egui::RichText::new(h).strong());
            }
            ui.end_row();
            for (tick, row) in shown {
                let pos = crate::midi_edit_tools::position_text(app.engine.session(), base.saturating_add(tick));
                ui.label(egui::RichText::new(pos).font(mono(11.0)));
                match row {
                    Row::Note(i) => note_row(app, ui, cid, i, &seq),
                    Row::Ctrl(i) => ctrl_row(app, ui, cid, i, &seq),
                    Row::SysEx(i) => sysex_row(app, ui, cid, i, &seq),
                }
                ui.end_row();
            }
        });
    });
}

fn filters(app: &mut SoundApp, ui: &mut egui::Ui) {
    let f = &mut app.midi.event_list;
    ui.horizontal_wrapped(|ui| {
        ui.label("Show");
        ui.checkbox(&mut f.notes, "Notes");
        ui.checkbox(&mut f.ccs, "Controllers");
        ui.checkbox(&mut f.bend, "Pitch Bend");
        ui.checkbox(&mut f.pressure, "Aftertouch");
        ui.checkbox(&mut f.program, "Program");
        ui.checkbox(&mut f.sysex, "SysEx");
        let text = f.channel.map_or("All channels".to_string(), |c| format!("Channel {}", u16::from(c) + 1));
        egui::ComboBox::from_id_salt("evl_channel").selected_text(text).show_ui(ui, |ui| {
            ui.selectable_value(&mut f.channel, None, "All channels");
            for c in 0u8..16 {
                ui.selectable_value(&mut f.channel, Some(c), format!("Channel {}", u16::from(c) + 1));
            }
        });
    });
}

/// Insert: a new event of the chosen kind at the insertion point (in the clip; its start when
/// the point is outside it).
fn insert_row(app: &mut SoundApp, ui: &mut egui::Ui, cid: ClipId) {
    ui.horizontal(|ui| {
        let f = &mut app.midi.event_list;
        egui::ComboBox::from_id_salt("evl_kind").selected_text(f.insert_kind.clone()).show_ui(ui, |ui| {
            for k in ["note", "cc", "pitchbend", "aftertouch", "polyat", "program", "sysex"] {
                ui.selectable_value(&mut f.insert_kind, k.to_string(), k);
            }
        });
        if matches!(f.insert_kind.as_str(), "cc" | "polyat") {
            ui.add(egui::DragValue::new(&mut f.insert_number).range(0..=127).prefix(if f.insert_kind == "cc" { "CC " } else { "key " }));
        }
        ui.add(egui::DragValue::new(&mut f.insert_channel).range(0..=15).custom_formatter(|v, _| format!("ch {}", v as u16 + 1)).custom_parser(|t| {
            t.trim().trim_start_matches("ch").trim().parse::<f64>().ok().map(|v| v - 1.0)
        }));
        if ui.button("Insert").on_hover_text("Add an event of this kind at the insertion point").clicked() {
            let s = app.engine.session();
            let at = s.find_clip(cid).map_or(0, |(_, c)| {
                let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
                let len = s.tempo.samples_to_ticks(c.end(), s.sample_rate).saturating_sub(base);
                s.tempo.samples_to_ticks(s.edit.selection.start, s.sample_rate).saturating_sub(base).clamp(0, len.saturating_sub(1).max(0))
            });
            let f = app.midi.event_list.clone();
            let r = match insert_lane(&f) {
                None if f.insert_kind == "sysex" => app.run("midi.sysex_add", json!({"clip": cid.0, "tick": at, "bytes": INSERTED_SYSEX})),
                Some(lane) => {
                    let value = if lane == "pitchbend" { 0 } else { 64 };
                    app.run("midi.ctrl_add", json!({"clip": cid.0, "lane": lane, "tick": at, "value": value, "channel": f.insert_channel}))
                }
                None => app.run(
                    "midi.notes_add",
                    json!({"clip": cid.0, "notes": [{"pitch": 60, "start": at, "length": 480, "velocity": app.midi.last_velocity, "channel": f.insert_channel}]}),
                ),
            };
            if let Err(e) = r {
                app.ui.status = e;
            }
        }
    });
}

fn note_row(app: &mut SoundApp, ui: &mut egui::Ui, cid: ClipId, i: usize, seq: &Sequence) {
    let Some(n) = seq.notes.get(i).copied() else { return };
    let set = |field: &str, v: i64| json!({"clip": cid.0, "indices": [i], field: v});
    let key = |field: &str| format!("evl.{}.n{i}.{field}", cid.0);
    if let Some(start) = deferred(app, ui, &key("start"), n.start)
        && let Err(e) = app.run("midi.notes_set", set("start", start))
    {
        app.ui.status = e;
    }
    ui.label(if n.muted { "Note (muted)" } else { "Note" });
    let mut ch = i64::from(n.channel);
    let r = ui.add(
        egui::DragValue::new(&mut ch)
            .range(0..=15)
            .custom_formatter(|v, _| format!("{}", v as i64 + 1))
            .custom_parser(|t| t.trim().parse::<f64>().ok().map(|v| v - 1.0)),
    );
    edit(app, "midi.notes_set", set("channel", ch), &key("channel"), &r);
    let mut pitch = i64::from(n.pitch);
    let r = ui.add(egui::DragValue::new(&mut pitch).range(0..=127).custom_formatter(|v, _| soundcraft_midi::note_name(v as u8)).custom_parser(|t| {
        let t = t.trim();
        t.parse::<f64>().ok().or_else(|| (0..=127u8).find(|p| soundcraft_midi::note_name(*p).eq_ignore_ascii_case(t)).map(f64::from))
    }));
    edit(app, "midi.notes_set", set("pitch", pitch), &key("pitch"), &r);
    let mut vel = i64::from(n.velocity);
    let r = ui.add(egui::DragValue::new(&mut vel).range(1..=127));
    edit(app, "midi.note_edit", json!({"clip": cid.0, "index": i, "velocity": vel}), &key("velocity"), &r);
    let mut len = n.length;
    let r = ui.add(egui::DragValue::new(&mut len).range(1..=i64::MAX / 4));
    edit(app, "midi.notes_set", set("length", len), &key("length"), &r);
    if ui.small_button("✕").on_hover_text("Delete this note").clicked() {
        let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": [i]}));
    }
}

fn ctrl_row(app: &mut SoundApp, ui: &mut egui::Ui, cid: ClipId, i: usize, seq: &Sequence) {
    let Some(c) = seq.ctrls.get(i).copied() else { return };
    let key = |field: &str| format!("evl.{}.c{i}.{field}", cid.0);
    if let Some(tick) = deferred(app, ui, &key("tick"), c.tick)
        && let Err(e) = app.run("midi.ctrl_edit", json!({"clip": cid.0, "index": i, "tick": tick}))
    {
        app.ui.status = e;
    }
    ui.label(kind_text(c.kind));
    let mut ch = i64::from(c.channel);
    let r = ui.add(
        egui::DragValue::new(&mut ch)
            .range(0..=15)
            .custom_formatter(|v, _| format!("{}", v as i64 + 1))
            .custom_parser(|t| t.trim().parse::<f64>().ok().map(|v| v - 1.0)),
    );
    edit(app, "midi.ctrl_edit", json!({"clip": cid.0, "index": i, "channel": ch}), &key("channel"), &r);
    ui.label(match c.kind {
        CtrlKind::Cc(n) => n.to_string(),
        CtrlKind::PolyPressure(p) => soundcraft_midi::note_name(p.min(127)),
        _ => String::new(),
    });
    let (lo, hi) = lane_range(c.kind);
    let mut v = c.value;
    let r = ui.add(egui::DragValue::new(&mut v).range(lo..=hi));
    edit(app, "midi.ctrl_edit", json!({"clip": cid.0, "index": i, "value": v}), &key("value"), &r);
    ui.label("");
    if ui.small_button("✕").on_hover_text("Delete this event").clicked() {
        let _ = app.run("midi.ctrl_delete", json!({"clip": cid.0, "indices": [i]}));
    }
}

fn sysex_row(app: &mut SoundApp, ui: &mut egui::Ui, cid: ClipId, i: usize, seq: &Sequence) {
    let Some(x) = seq.sysex.get(i) else { return };
    let key = |field: &str| format!("evl.{}.x{i}.{field}", cid.0);
    if let Some(tick) = deferred(app, ui, &key("tick"), x.tick)
        && let Err(e) = app.run("midi.sysex_edit", json!({"clip": cid.0, "index": i, "tick": tick}))
    {
        app.ui.status = e;
    }
    ui.label("SysEx");
    ui.label("");
    ui.label(format!("{} bytes", x.data.len()));
    // The bytes as hex: typed anew, written when the typing ends.
    let k = key("bytes");
    let mut text = match &app.midi.event_list.pending_bytes {
        Some((pk, t)) if *pk == k => t.clone(),
        _ => soundcraft_engine::cmd::midi_sysex::hex(&x.data),
    };
    let r = ui.add(egui::TextEdit::singleline(&mut text).font(mono(11.0)).desired_width(220.0));
    if r.changed() {
        app.midi.event_list.pending_bytes = Some((k.clone(), text.clone()));
    }
    if r.lost_focus()
        && let Some((pk, t)) = app.midi.event_list.pending_bytes.clone()
        && pk == k
    {
        app.midi.event_list.pending_bytes = None;
        if let Err(e) = app.run("midi.sysex_edit", json!({"clip": cid.0, "index": i, "bytes": t})) {
            app.ui.status = e;
        }
    }
    ui.label("");
    if ui.small_button("✕").on_hover_text("Delete this SysEx").clicked() {
        let _ = app.run("midi.sysex_delete", json!({"clip": cid.0, "indices": [i]}));
    }
}
