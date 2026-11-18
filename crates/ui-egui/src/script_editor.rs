//! The Script Editor: a Script MIDI effect's text, Compile, the error (with its line) or the
//! script's last printed lines, its parameters as sliders and the built-in examples
//! (`track.midi_script`; a slider's drag is one undo step). Opened from the MIDI Effects submenu
//! (Edit Script…) for one effect of one track. The text is kept in the window until Compile, so
//! typing makes no undo steps.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::vec2;
use serde_json::{Value, json};
use soundcraft_mix::midi_fx::script::{EXAMPLES, program_of};
use soundcraft_model::{MidiFxKind, TrackId};

/// The text being edited, and the script text it started from (a change there, by Undo or an
/// example, replaces it).
#[derive(Clone, Default)]
struct Draft {
    key: (u64, usize),
    from: String,
    text: String,
}

fn draft_id() -> egui::Id {
    egui::Id::new("script-editor-draft")
}

/// Opens the editor on `track`'s MIDI effect `index` (a Script).
pub(crate) fn open(app: &mut SoundApp, track: TrackId, index: usize) {
    app.ui.script_window = Some((track, index));
}

pub(crate) fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some((tid, index)) = app.ui.script_window else { return };
    let Some(track) = app.engine.session().track(tid) else {
        app.ui.script_window = None;
        return;
    };
    let Some(sc) =
        track.midi_fx.get(index).filter(|f| f.kind == MidiFxKind::Script).and_then(|f| track.midi_scripts.iter().find(|s| s.id == f.script)).cloned()
    else {
        app.ui.script_window = None;
        return;
    };
    let track_name = track.name.clone();
    let key = (tid.0, index);
    let mut d: Draft = ctx.data(|m| m.get_temp(draft_id())).unwrap_or_default();
    if d.key != key || d.from != sc.source {
        d = Draft { key, from: sc.source.clone(), text: sc.source.clone() };
    }
    let prog = program_of(&sc.runtime);
    let status = prog.as_ref().map(|p| p.status());
    let error = sc.runtime.error.clone().or_else(|| status.as_ref().and_then(|s| s.error.clone()));
    let mut asks: Vec<(Value, Option<String>)> = Vec::new();
    let mut stop_merge = false;
    let mut open = true;
    let t = Tokens::current();
    egui::Window::new(format!("Script · {} · {}", sc.name, track_name))
        .id(egui::Id::new("script-editor-window"))
        .open(&mut open)
        .default_size(vec2(520.0, 520.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let changed = d.text != sc.source;
                if ui.button("Compile").on_hover_text("Run this text from now (it starts afresh)").clicked() {
                    asks.push((json!({"source": d.text}), None));
                }
                if ui.add_enabled(changed, egui::Button::new("Revert")).on_hover_text("Back to the text that runs").clicked() {
                    d.text = sc.source.clone();
                }
                ui.menu_button("Examples", |ui| {
                    for (name, _) in EXAMPLES {
                        if ui.button(name).on_hover_text("Replaces the text (Undo brings it back)").clicked() {
                            asks.push((json!({"example": name}), None));
                            ui.close();
                        }
                    }
                });
                if changed {
                    ui.label(egui::RichText::new("Not compiled yet").color(t.text_dim));
                }
            });
            // The error, or that it runs.
            match &error {
                Some(e) => {
                    ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x50), format!("Bypassed: {e}"));
                }
                None if prog.is_some() => {
                    ui.label(egui::RichText::new("Running (heard one buffer late while playing live; on time in bounces)").color(t.text_dim));
                }
                None => {}
            }
            if let Some(s) = &status
                && (s.late > 0 || s.dropped > 0)
            {
                ui.label(egui::RichText::new(format!("{} event(s) late, {} left out", s.late, s.dropped)).small().color(t.text_dim));
            }
            // Parameters.
            if let Some(p) = &prog {
                let values = p.values(&sc);
                for (decl, v) in p.params().iter().zip(values) {
                    let mut x = v;
                    ui.horizontal(|ui| {
                        ui.label(&decl.name);
                        let r = ui.add(egui::Slider::new(&mut x, decl.min..=decl.max));
                        if r.changed() {
                            asks.push((json!({"values": {decl.name.clone(): x}}), Some(format!("midi-script.{}.{index}.{}", tid.0, decl.name))));
                        }
                        stop_merge |= r.drag_stopped();
                    });
                }
            }
            ui.separator();
            let h = (ui.available_height() - 70.0).max(120.0);
            egui::ScrollArea::vertical().max_height(h).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut d.text).code_editor().desired_width(f32::INFINITY).desired_rows(18));
            });
            if let Some(s) = &status
                && !s.log.is_empty()
            {
                ui.separator();
                for line in s.log.iter().rev().take(4).rev() {
                    ui.label(egui::RichText::new(line).monospace().small());
                }
            }
        });
    ctx.data_mut(|m| m.insert_temp(draft_id(), d));
    for (mut p, key) in asks {
        if let Some(o) = p.as_object_mut() {
            o.insert("tracks".into(), json!([tid.0]));
            o.insert("index".into(), json!(index));
        }
        let r = match key {
            Some(k) => app.engine.execute_merged("track.midi_script", &p, &k).map(|_| ()).map_err(|e| e.to_string()),
            None => app.run("track.midi_script", p).map(|_| ()),
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
    if stop_merge {
        app.engine.end_merge();
    }
    if !open {
        app.ui.script_window = None;
    }
}
