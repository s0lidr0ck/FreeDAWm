//! Scout ▸ Profiles: the sound-profile library (spec 2026-10-29-scout-profiles §1). Lists the
//! profiles in the library folder (`Services::profiles_dir`, else the engine's default) with
//! search and a role filter; renames (double-click), duplicates, exports and deletes them; imports a
//! profile file or measures a reference audio file (File ▸ Import ▸ Reference…), the selected clip
//! or time range, or every stem of the selected track ("Capture per stem", on a worker thread).
//!
//! UI commands: `ui.scout.import_reference {path?}`, `ui.scout.profile_import {path?}`,
//! `ui.scout.profile_export {name, path?}`, `ui.scout.capture_stems {track?, clip?, name?}`.

use super::package::derive;
use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use serde_json::{Value, json};
use soundcraft_model::{PROFILE_ROLES, ProfileKind, ScoutProfile};

/// The empty library.
pub const NO_PROFILES: &str = "No profiles yet: save one from a capture's strip, the selection, a reference file or a song's stems";

/// One library entry as the panel keeps it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub file: String,
    pub profile: ScoutProfile,
}

/// The Profiles tab's state (not saved).
#[derive(Default)]
pub struct ProfilesUi {
    pub query: String,
    /// "" = every role.
    pub role: String,
    /// The library as last read (`None`: read on the next frame).
    pub entries: Option<Vec<Entry>>,
    /// The profile being renamed, and the name typed so far.
    pub renaming: Option<(String, String)>,
    /// Capture per stem running on a worker thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub stems: Option<std::sync::mpsc::Receiver<Result<Value, String>>>,
}

impl ProfilesUi {
    pub fn busy(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.stems.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }
}

/// `p` with the library folder the host chose (`dir`), when it chose one.
pub(crate) fn with_dir(app: &SoundApp, mut p: Value) -> Value {
    if let (Some(d), Some(o)) = (&app.services.profiles_dir, p.as_object_mut())
        && !o.contains_key("dir")
    {
        o.insert("dir".into(), json!(d.to_string_lossy()));
    }
    p
}

/// Runs a `scout.profile_*` command on the library and re-reads it.
pub(crate) fn profile_cmd(app: &mut SoundApp, id: &str, p: Value) -> Result<Value, String> {
    let p = with_dir(app, p);
    let r = app.run(id, p);
    app.scout.profiles.entries = None;
    if let Err(e) = &r {
        app.ui.status = e.clone();
    }
    r
}

/// The library (read when needed).
pub fn entries(app: &mut SoundApp) -> Vec<Entry> {
    if app.scout.profiles.entries.is_none() {
        let p = with_dir(app, json!({}));
        let list = app.engine.execute("scout.profile_list", &p).ok();
        let v: Vec<Entry> = list
            .as_ref()
            .and_then(|v| v["profiles"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|x| {
                let file = x["file"].as_str().unwrap_or("").to_string();
                serde_json::from_value::<ScoutProfile>(x.clone()).ok().map(|profile| Entry { file, profile })
            })
            .collect();
        app.scout.profiles.entries = Some(v);
    }
    app.scout.profiles.entries.clone().unwrap_or_default()
}

/// UI commands of the Profiles tab.
pub fn run(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.scout.import_reference" => {
            let path = p.get("path").and_then(Value::as_str).map(str::to_string).or_else(|| {
                app.services.pick_open.as_ref().and_then(|pick| pick("scout.import_reference", &["wav", "aif", "aiff", "flac", "mp3", "ogg", "m4a"]))
            });
            match path {
                None => Ok(json!({"path": null})),
                Some(path) => {
                    let mut q = json!({"from": "file", "path": path});
                    for k in ["name", "role", "kind", "tags"] {
                        if let (Some(v), Some(o)) = (p.get(k), q.as_object_mut()) {
                            o.insert(k.into(), v.clone());
                        }
                    }
                    let r = profile_cmd(app, "scout.profile_capture", q);
                    if r.is_ok() {
                        app.scout.show = true;
                        app.scout.tab = super::Tab::Profiles;
                        app.ui.status = "Reference measured and saved as a profile".into();
                    }
                    r
                }
            }
        }
        "ui.scout.profile_import" => {
            let path = p
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| app.services.pick_open.as_ref().and_then(|pick| pick("scout.profile_import", &["json"])));
            match path {
                None => Ok(json!({"path": null})),
                Some(path) => profile_cmd(app, "scout.profile_import", json!({"path": path})),
            }
        }
        "ui.scout.profile_export" => {
            let Some(name) = p.get("name").and_then(Value::as_str).map(str::to_string) else { return Some(Err("`name` must name a profile".into())) };
            let path = p.get("path").and_then(Value::as_str).map(str::to_string).or_else(|| {
                let file = format!("{}.json", soundcraft_model::profile_file_stem(&name));
                app.services.pick_save.as_ref().and_then(|pick| pick("scout.profile_export", &file))
            });
            match path {
                None => Ok(json!({"path": null})),
                Some(path) => profile_cmd(app, "scout.profile_export", json!({"name": name, "path": path})),
            }
        }
        "ui.scout.capture_stems" => capture_stems(app, p),
        _ => return None,
    })
}

/// Capture per stem: the separation runs on a worker thread over a copy of the session (nothing in
/// the session changes); the library is re-read when it is done.
fn capture_stems(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    if app.scout.profiles.busy() {
        return Err("Capture per stem is already running".into());
    }
    let mut q = with_dir(app, p.clone());
    if let Some(o) = q.as_object_mut() {
        o.insert("from".into(), json!("stems"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let s = app.engine.session().clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("scout-stems".into())
            .spawn(move || {
                let mut e = soundcraft_engine::Engine::new(s);
                let r = e.execute("scout.profile_capture", &q).map_err(|e| e.to_string());
                let _ = tx.send(r);
            })
            .map_err(|e| e.to_string())?;
        app.scout.profiles.stems = Some(rx);
        app.ui.status = "Separating stems for profiles…".into();
        Ok(json!({"started": true}))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = q;
        Err("Capture per stem needs the desktop app".into())
    }
}

/// Each frame: a finished Capture per stem re-reads the library (desktop only: the web build has
/// no Capture per stem).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn poll(app: &mut SoundApp) {
    let done = app.scout.profiles.stems.as_ref().and_then(|rx| match rx.try_recv() {
        Ok(r) => Some(r),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("Capture per stem stopped".into())),
    });
    if let Some(r) = done {
        app.scout.profiles.stems = None;
        app.scout.profiles.entries = None;
        app.ui.status = match r {
            Ok(v) => format!("Saved {} profiles", v["profiles"].as_array().map_or(0, Vec::len)),
            Err(e) => e,
        };
    }
}

fn chip(ui: &mut Ui, text: &str, fg: Color32, stroke: Color32) {
    egui::Frame::NONE
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| ui.label(RichText::new(text).font(regular(10.5)).color(fg)));
}

/// The profile's key numbers in one line.
pub fn summary(p: &ScoutProfile) -> String {
    let m = &p.metrics;
    let d = derive(m);
    format!(
        "{:.1} LUFS · LRA {:.1} · crest {:.1} dB · width {:.2} · tilt {:+.1} dB/oct",
        m.lufs_integrated, m.lra, m.crest_factor_db, m.stereo_width, d.tilt_db_per_octave
    )
}

/// A small spectrum curve of `m` in `size` (63 Hz..16 kHz bands).
pub(crate) fn spectrum(ui: &mut Ui, spectrum31: &[f32], size: egui::Vec2, colour: Color32) -> egui::Rect {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(3), t.panel_bg);
    let vals: Vec<f32> = spectrum31.iter().copied().skip(4).take(25).collect();
    let max = vals.iter().copied().fold(-200.0f32, f32::max);
    let n = vals.len().max(2) as f32 - 1.0;
    let pts: Vec<egui::Pos2> = vals
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let y = ((max - v) / 48.0).clamp(0.0, 1.0);
            egui::pos2(r.left() + 2.0 + (r.width() - 4.0) * i as f32 / n, r.top() + 2.0 + (r.height() - 4.0) * y)
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(1.5, colour)));
    r
}

/// The Profiles tab.
pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let busy = app.scout.profiles.busy();
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut app.scout.profiles.query).desired_width(220.0).hint_text("Search profiles…"));
        let role = if app.scout.profiles.role.is_empty() { "All roles".to_string() } else { app.scout.profiles.role.clone() };
        egui::ComboBox::from_id_salt("scout_profile_role").selected_text(role).width(110.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.scout.profiles.role, String::new(), "All roles");
            for r in PROFILE_ROLES {
                ui.selectable_value(&mut app.scout.profiles.role, r.to_string(), r);
            }
        });
        ui.add_space(8.0);
        if ui.button("Import…").on_hover_text("Add a profile file to the library").clicked() {
            let _ = app.run("ui.scout.profile_import", json!({}));
        }
        if ui.button("Import Reference…").on_hover_text("Measure a reference audio file (not added to the session)").clicked() {
            let _ = app.run("ui.scout.import_reference", json!({}));
        }
        let s = app.engine.session_arc();
        let has_sel = !s.edit.selected_clips.is_empty() || (!s.edit.selected_tracks.is_empty() && !s.edit.selection.is_empty());
        if ui
            .add_enabled(has_sel, egui::Button::new("From Selection"))
            .on_hover_text("Measure the selected clip, or the selected range of the selected track")
            .clicked()
        {
            let from = if s.edit.selected_clips.is_empty() { "range" } else { "clip" };
            let _ = profile_cmd(app, "scout.profile_capture", json!({"from": from}));
        }
        let has_track = !s.edit.selected_tracks.is_empty() || !s.edit.selected_clips.is_empty();
        let label = if busy { "Separating…" } else { "Capture per Stem" };
        if ui
            .add_enabled(has_track && !busy, egui::Button::new(label))
            .on_hover_text("Separate the selected track's song into stems and save one profile per stem plus the mix")
            .clicked()
            && let Err(e) = app.run("ui.scout.capture_stems", json!({}))
        {
            app.ui.status = e;
        }
        if busy {
            ui.spinner();
        }
    });
    ui.add_space(6.0);
    let all = entries(app);
    let (q, role) = (app.scout.profiles.query.clone(), app.scout.profiles.role.clone());
    let shown: Vec<&Entry> = all.iter().filter(|e| e.profile.matches(&q, &role)).collect();
    egui::ScrollArea::vertical().id_salt("scout_profiles").auto_shrink([false, false]).show(ui, |ui| {
        if shown.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(NO_PROFILES).font(regular(12.0)).color(t.text_dim)));
            return;
        }
        for e in shown {
            egui::Frame::NONE
                .fill(t.panel_bg2)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    card(app, ui, e);
                });
            ui.add_space(6.0);
        }
    });
}

fn card(app: &mut SoundApp, ui: &mut Ui, e: &Entry) {
    let t = Tokens::current();
    let p = &e.profile;
    ui.horizontal(|ui| {
        let colour = if p.kind == ProfileKind::Mix { t.meter_yellow } else { t.accent };
        spectrum(ui, &p.metrics.spectrum31, vec2(110.0, 34.0), colour);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let renaming = app.scout.profiles.renaming.as_ref().is_some_and(|(n, _)| *n == p.name);
                if renaming {
                    let mut done = false;
                    if let Some((_, name)) = &mut app.scout.profiles.renaming {
                        let r = ui.add(egui::TextEdit::singleline(name).desired_width(220.0).font(bold(12.5)));
                        r.request_focus();
                        done = r.lost_focus();
                    }
                    if done && let Some((old, to)) = app.scout.profiles.renaming.take() {
                        let _ = profile_cmd(app, "scout.profile_rename", json!({"name": old, "to": to}));
                    }
                } else {
                    let r = ui.add(egui::Label::new(RichText::new(&p.name).font(bold(12.5)).color(t.text)).sense(Sense::click()));
                    let r = r.on_hover_text("Double-click to rename; right-click for role and kind");
                    if r.double_clicked() {
                        app.scout.profiles.renaming = Some((p.name.clone(), p.name.clone()));
                    }
                    r.context_menu(|ui| {
                        ui.label(RichText::new("Role").font(bold(10.5)).color(t.text_dim));
                        for role in PROFILE_ROLES {
                            if ui.selectable_label(p.role == role, role).clicked() {
                                let _ = profile_cmd(app, "scout.profile_edit", json!({"name": p.name, "role": role}));
                                ui.close();
                            }
                        }
                        ui.separator();
                        for kind in [ProfileKind::Track, ProfileKind::Mix] {
                            if ui.selectable_label(p.kind == kind, format!("Kind: {}", kind.as_str())).clicked() {
                                let _ = profile_cmd(app, "scout.profile_edit", json!({"name": p.name, "kind": kind.as_str()}));
                                ui.close();
                            }
                        }
                    });
                }
                chip(ui, p.kind.as_str(), t.text_dim, t.border_light);
                if !p.role.is_empty() {
                    chip(ui, &p.role, t.text, t.border_light);
                }
                for tag in &p.tags {
                    chip(ui, tag, t.text_dim, t.border);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("×").on_hover_text("Delete this profile").clicked() {
                        let _ = profile_cmd(app, "scout.profile_delete", json!({"name": p.name}));
                    }
                    if ui.small_button("Export…").clicked() {
                        let _ = app.run("ui.scout.profile_export", json!({"name": p.name}));
                    }
                    if ui.small_button("Duplicate").clicked() {
                        let _ = profile_cmd(app, "scout.profile_duplicate", json!({"name": p.name}));
                    }
                });
            });
            ui.label(RichText::new(summary(p)).font(regular(11.0)).color(t.text));
            let src = p.source_text();
            if !src.is_empty() {
                ui.label(RichText::new(src).font(regular(10.5)).color(t.text_dim));
            }
        });
    });
}
