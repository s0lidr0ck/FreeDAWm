//! Deleting markers, Band chords and key changes from the rulers.
//!
//! A click on a marker or a chord picks it ([`RulerItem`], drawn outlined); Delete (or Backspace)
//! then removes it. A marker's or a chord's right-click menu deletes it, the ones starting inside
//! the edit selection, or all of them on that ruler after an in-app confirmation; a ruler's name
//! label offers Clear All too. Every action is an engine command (`markers.delete`,
//! `band.chord.remove`, `band.chords.delete`, `event.key_change_delete`), one undo step each.

use crate::SoundApp;
use crate::dialogs::Dialog;
use serde_json::json;
use soundcraft_model::MarkerId;

/// The marker or Band chord picked on a ruler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulerItem {
    Marker(MarkerId),
    /// A chord on the Band's chord lane, by id.
    Chord(u32),
    /// A key change on the Key ruler, by its position (samples).
    Key(soundcraft_time::Samples),
}

/// The marker ruler lane (1-5) of a marker ruler id.
pub fn marker_lane(ruler: &str) -> Option<u8> {
    match ruler {
        "markers" => Some(1),
        "markers2" => Some(2),
        "markers3" => Some(3),
        "markers4" => Some(4),
        "markers5" => Some(5),
        _ => None,
    }
}

/// Is the marker on ruler lane `lane` (lane 1 also shows markers from before lanes existed)?
pub fn on_lane(m: &soundcraft_model::MemoryLocation, lane: u8) -> bool {
    m.ruler == lane || (lane == 1 && m.ruler == 0)
}

/// Pick `item` (a click or right-click on it), remembering the selection it was picked with.
pub fn pick(app: &mut SoundApp, item: RulerItem) {
    let ed = &app.engine.session().edit;
    app.ui.ruler_pick_sel = Some((ed.selection, ed.selected_clips.clone()));
    app.ui.ruler_item = Some(item);
}

/// Drop the pick.
pub fn unpick(app: &mut SoundApp) {
    app.ui.ruler_item = None;
    app.ui.ruler_pick_sel = None;
}

/// Is the pick still what Delete should remove: the Edit window is showing, the item still
/// exists, and neither the edit selection nor the clip selection changed since it was picked?
pub fn pick_is_live(app: &SoundApp) -> bool {
    let Some(item) = app.ui.ruler_item else { return false };
    let s = app.engine.session();
    let exists = match item {
        RulerItem::Marker(id) => s.markers.iter().any(|m| m.id == id),
        RulerItem::Chord(id) => s.band.chords.iter().any(|c| c.id == id),
        RulerItem::Key(at) => s.key_signatures.iter().any(|(t, _)| *t == at),
    };
    let same_selection = app.ui.ruler_pick_sel.as_ref().is_some_and(|(r, clips)| *r == s.edit.selection && *clips == s.edit.selected_clips);
    app.ui.window == crate::MainWindow::Edit && exists && same_selection
}

/// Drop a pick that is no longer live (`pick_is_live`).
pub fn prune(app: &mut SoundApp) {
    if app.ui.ruler_item.is_some() && !pick_is_live(app) {
        unpick(app);
    }
}

/// Delete the picked item (the Delete key). False when nothing live is picked, so the key does
/// what it would do otherwise.
pub fn delete_picked(app: &mut SoundApp) -> bool {
    prune(app);
    let Some(item) = app.ui.ruler_item else { return false };
    unpick(app);
    let r = match item {
        RulerItem::Marker(id) => app.run("markers.delete", json!({"id": id.0})),
        RulerItem::Chord(id) => app.run("band.chord.remove", json!({"chord": id})),
        RulerItem::Key(at) => app.run("event.key_change_delete", json!({"at": at})),
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
    true
}

/// The edit selection as a `range` parameter, when it has a length.
pub fn selection_range(app: &SoundApp) -> Option<serde_json::Value> {
    let r = app.engine.session().edit.selection;
    (r.end > r.start).then(|| json!({"start": r.start, "end": r.end}))
}

fn run_reporting(app: &mut SoundApp, id: &str, p: serde_json::Value) {
    if let Err(e) = app.run(id, p) {
        app.ui.status = e;
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// "Delete Markers in Selection" and "Clear All Markers…" for marker ruler `lane` (`label` names
/// the ruler in the question).
pub fn marker_clear_items(app: &mut SoundApp, ui: &mut egui::Ui, lane: u8, label: &str) {
    let s = app.engine.session();
    let range = selection_range(app);
    let inside = range.as_ref().map_or(0, |r| {
        let (a, b) = (r["start"].as_i64().unwrap_or(0), r["end"].as_i64().unwrap_or(0));
        s.markers.iter().filter(|m| on_lane(m, lane) && m.start >= a && m.start < b).count()
    });
    let count = s.markers.iter().filter(|m| on_lane(m, lane)).count();
    if ui.add_enabled(inside > 0, egui::Button::new("Delete Markers in Selection")).clicked() {
        if let Some(r) = range {
            run_reporting(app, "markers.delete", json!({"range": r, "ruler": lane}));
        }
        ui.close();
    }
    if ui.add_enabled(count > 0, egui::Button::new("Clear All Markers…")).clicked() {
        ask_clear_markers(app, lane, label, count);
        ui.close();
    }
}

/// A marker's right-click menu: Edit Marker…, Delete Marker, then the clear items.
pub fn marker_menu(app: &mut SoundApp, ui: &mut egui::Ui, lane: u8, label: &str, marker: MarkerId) {
    let found = app.engine.session().markers.iter().find(|m| m.id == marker).map(|m| m.name.clone());
    if let Some(name) = found {
        if ui.button("Edit Marker…").clicked() {
            app.dialogs.open_rename_marker(marker, &name);
            ui.close();
        }
        if ui.add(egui::Button::new("Delete Marker").shortcut_text("Del")).clicked() {
            unpick(app);
            run_reporting(app, "markers.delete", json!({"id": marker.0}));
            ui.close();
        }
        ui.separator();
    }
    marker_clear_items(app, ui, lane, label);
}

/// "Delete Chords in Selection" and "Clear All Chords…".
pub fn chord_clear_items(app: &mut SoundApp, ui: &mut egui::Ui) {
    let s = app.engine.session();
    let range = selection_range(app);
    let inside = range.as_ref().map_or(0, |r| {
        let tick = |k: &str| s.tempo.samples_to_ticks(r[k].as_i64().unwrap_or(0).max(0), s.sample_rate);
        let (a, b) = (tick("start"), tick("end"));
        s.band.chords.iter().filter(|c| c.tick >= a && c.tick < b).count()
    });
    let count = s.band.chords.len();
    if ui.add_enabled(inside > 0, egui::Button::new("Delete Chords in Selection")).clicked() {
        if let Some(r) = range {
            run_reporting(app, "band.chords.delete", json!({"range": r}));
        }
        ui.close();
    }
    if ui.add_enabled(count > 0, egui::Button::new("Clear All Chords…")).clicked() {
        ask_clear_chords(app, count);
        ui.close();
    }
}

/// A key change's right-click menu: Edit Key… (the key picker) and Delete Key.
pub fn key_menu(app: &mut SoundApp, ui: &mut egui::Ui, at: soundcraft_time::Samples) {
    let Some(name) = app.engine.session().key_signatures.iter().find(|(t, _)| *t == at).map(|(_, k)| k.clone()) else { return };
    if ui.button("Edit Key…").clicked() {
        crate::key_ruler::open_picker(app, at, Some(&name));
        ui.close();
    }
    if ui.add(egui::Button::new("Delete Key").shortcut_text("Del")).clicked() {
        unpick(app);
        run_reporting(app, "event.key_change_delete", json!({"at": at}));
        ui.close();
    }
    ui.separator();
}

/// Ask before removing every key change.
fn ask_clear_keys(app: &mut SoundApp, count: usize) {
    app.dialogs.open = Some(Dialog::Confirm {
        title: "Clear All Key Changes".into(),
        message: format!("Remove {} from the Key ruler? Undo brings them back.", plural(count, "key change")),
        ok: "Clear All".into(),
        cmd: "event.key_change_delete".into(),
        params: json!({"all": true}),
    });
}

/// Ask before removing every marker on ruler `lane`.
fn ask_clear_markers(app: &mut SoundApp, lane: u8, label: &str, count: usize) {
    app.dialogs.open = Some(Dialog::Confirm {
        title: "Clear All Markers".into(),
        message: format!("Remove {} from the {label} ruler? Undo brings them back.", plural(count, "marker")),
        ok: "Clear All".into(),
        cmd: "markers.delete".into(),
        params: json!({"all": true, "ruler": lane}),
    });
}

/// Ask before removing every chord on the chord lane.
fn ask_clear_chords(app: &mut SoundApp, count: usize) {
    app.dialogs.open = Some(Dialog::Confirm {
        title: "Clear All Chords".into(),
        message: format!("Remove {} from the Chords ruler? Undo brings them back.", plural(count, "chord")),
        ok: "Clear All".into(),
        cmd: "band.chords.delete".into(),
        params: json!({"all": true}),
    });
}

/// A ruler name label's right-click menu head: Clear All for a marker, chord or Key ruler.
pub fn label_menu_head(app: &mut SoundApp, ui: &mut egui::Ui, ruler: &str, label: &str) {
    if let Some(lane) = marker_lane(ruler) {
        let count = app.engine.session().markers.iter().filter(|m| on_lane(m, lane)).count();
        if ui.add_enabled(count > 0, egui::Button::new("Clear All Markers…")).clicked() {
            ask_clear_markers(app, lane, label, count);
            ui.close();
        }
        ui.separator();
    } else if ruler == "key" {
        let count = app.engine.session().key_signatures.len();
        if ui.add_enabled(count > 0, egui::Button::new("Clear All Key Changes…")).clicked() {
            ask_clear_keys(app, count);
            ui.close();
        }
        ui.separator();
    } else if ruler == "band_chords" {
        let count = app.engine.session().band.chords.len();
        if ui.add_enabled(count > 0, egui::Button::new("Clear All Chords…")).clicked() {
            ask_clear_chords(app, count);
            ui.close();
        }
        ui.separator();
    }
}
