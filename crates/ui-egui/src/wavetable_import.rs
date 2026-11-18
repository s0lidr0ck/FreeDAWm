//! Import Wavetable: making the Wavetable Synth's table from the user's own audio. The window
//! (opened from the synth's panel, or by dropping an audio file on its wave display) takes a file
//! (picked or dropped), the way to cut it (Fixed frames, Pitch-tracked cycles, Spectral), and that
//! mode's options; it shows the waves the import would make as a small stack to step through
//! (`mix.wavetable_preview`, which changes nothing), and Import makes them the synth's table
//! (`mix.wavetable_import`: one undo step). The panel draws the current table through
//! [`user_table`].

use crate::SoundApp;
use crate::theme::Tokens;
use egui::{Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::plugins::wavetable::user::UserTable;
use soundcraft_model::{Insert, TrackId};
use std::sync::Arc;

/// The Wavetable Synth's plugin id.
pub const WAVETABLE: &str = "wavetable_synth";
/// The modes as the window names them, and as the command takes them.
pub const MODES: [(&str, &str); 3] = [("Fixed frames", "fixed"), ("Pitch-tracked", "pitch"), ("Spectral", "spectral")];
/// What each mode does, for its tooltip.
const MODE_HELP: [&str; 3] = [
    "The file is a row of single-cycle frames, as wavetable files are (2048 samples each by convention).",
    "One cycle cut at each point at the pitch found there: for a sung, played or hummed note.",
    "One clean cycle made from the spectrum at each point: works on anything, noise, speech, drums.",
];
/// Frame sizes offered for Fixed (`None`: from the file).
const FRAME_SIZES: [Option<usize>; 7] = [None, Some(256), Some(512), Some(1024), Some(2048), Some(4096), Some(8192)];

/// The open Import Wavetable window.
#[derive(Debug, Clone)]
pub struct ImportWindow {
    pub track: TrackId,
    pub path: String,
    /// Index into [`MODES`].
    pub mode: usize,
    pub frame_size: Option<usize>,
    pub waves: usize,
    pub name: String,
    /// The wave shown large in the stack.
    pub step: usize,
    preview: Option<Preview>,
    /// The parameters the preview was made with (a change makes it again).
    made_for: Option<Value>,
    /// What the synth plays now (`mix.wavetable_info`), and the state it was read for.
    current: Option<(Option<String>, Value)>,
}

#[derive(Debug, Clone)]
struct Preview {
    result: Result<PreviewWaves, String>,
}

#[derive(Debug, Clone)]
struct PreviewWaves {
    name: String,
    waves: Vec<Vec<f32>>,
    notes: Vec<String>,
    frame_size: usize,
}

impl ImportWindow {
    /// The parameters for `mix.wavetable_import` (and its preview).
    pub fn params(&self) -> Value {
        let mut p = json!({"track": self.track.0, "path": self.path, "mode": MODES.get(self.mode).map_or("fixed", |m| m.1)});
        match self.mode {
            0 => {
                if let Some(n) = self.frame_size {
                    p["frame_size"] = json!(n);
                }
            }
            _ => p["waves"] = json!(self.waves),
        }
        if !self.name.trim().is_empty() {
            p["name"] = json!(self.name.trim());
        }
        p
    }
}

/// Opens the window for `track`'s Wavetable Synth, with `path` (a dropped or picked file) when
/// given.
pub fn open(app: &mut SoundApp, track: TrackId, path: Option<String>) {
    let keep = app.wavetable_import.as_ref().filter(|w| w.track == track).cloned();
    let mut w = keep.unwrap_or(ImportWindow {
        track,
        path: String::new(),
        mode: 0,
        frame_size: None,
        waves: 32,
        name: String::new(),
        step: 0,
        preview: None,
        made_for: None,
        current: None,
    });
    if let Some(p) = path {
        w.path = p;
        w.name.clear();
        w.step = 0;
    }
    app.wavetable_import = Some(w);
}

/// The selected track Import Wavetable… acts on: the first one playing the Wavetable Synth.
pub fn target(app: &SoundApp) -> Option<TrackId> {
    let s = app.engine.session();
    app.engine.target_tracks().into_iter().find(|t| s.track(*t).and_then(|x| x.instrument.as_ref()).is_some_and(|i| i.plugin == WAVETABLE))
}

/// Track › Import Wavetable…: the window for the selected track's synth.
pub fn open_for_selection(app: &mut SoundApp) -> Result<Value, String> {
    let t = target(app).ok_or("Select a track playing the Wavetable Synth")?;
    open(app, t, None);
    Ok(json!({"track": t.0, "open": true}))
}

/// Remembers `r` (the synth's wave display) as a place where a dropped audio file opens the
/// import window for `track`, and outlines it while files are dragged over it.
pub fn drop_target(app: &mut SoundApp, ctx: &egui::Context, r: Rect, track: TrackId) {
    app.wavetable_drop_targets.push((r, track));
    let over = ctx.input(|i| !i.raw.hovered_files.is_empty() && i.pointer.latest_pos().is_some_and(|p| r.contains(p)));
    if over {
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("wavetable_drop", track.0)))).rect_stroke(
            r,
            2.0,
            Stroke::new(2.0, Tokens::current().accent),
            StrokeKind::Inside,
        );
    }
}

/// The track a drop at `pos` goes to (a wave display, or the open import window), if any.
pub(crate) fn drop_track(app: &SoundApp, pos: Option<Pos2>) -> Option<TrackId> {
    let p = pos?;
    app.wavetable_drop_targets.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, t)| *t)
}

/// The imported table `ins` (a Wavetable Synth) plays, decoded once per change (`None`: it plays
/// a built-in table, or the stored one is damaged).
pub fn user_table(ins: &Insert) -> Option<Arc<UserTable>> {
    use std::hash::{Hash, Hasher};
    thread_local! {
        static CACHE: std::cell::RefCell<Option<(u64, Option<Arc<UserTable>>)>> = const { std::cell::RefCell::new(None) };
    }
    if ins.plugin != WAVETABLE {
        return None;
    }
    let state = ins.state.as_ref()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    state.hash(&mut h);
    let key = h.finish();
    CACHE.with(|c| {
        if let Some((k, t)) = c.borrow().as_ref()
            && *k == key
        {
            return t.clone();
        }
        let t = ins.state_bytes().and_then(|b| UserTable::decode(&b).ok()).map(Arc::new);
        *c.borrow_mut() = Some((key, t.clone()));
        t
    })
}

/// One cycle `w` as points across `r` (peak ±1 at the top and bottom).
fn wave_points(w: &[f32], r: Rect) -> Vec<Pos2> {
    let last = w.len().saturating_sub(1).max(1) as f32;
    w.iter().enumerate().map(|(i, v)| pos2(r.min.x + r.width() * i as f32 / last, r.center().y - r.height() * 0.5 * v.clamp(-1.0, 1.0))).collect()
}

/// Makes the preview again when the parameters changed.
fn refresh(app: &mut SoundApp) {
    let Some(w) = app.wavetable_import.as_ref() else { return };
    if w.path.is_empty() {
        return;
    }
    let mut params = w.params();
    params["points"] = json!(160);
    if w.made_for.as_ref() == Some(&params) {
        return;
    }
    let result = app.engine.execute("mix.wavetable_preview", &params).map_err(|e| e.to_string()).map(|v| PreviewWaves {
        name: v["name"].as_str().unwrap_or("").to_string(),
        waves: v["waves"]
            .as_array()
            .map(|a| {
                a.iter().map(|w| w.as_array().map(|x| x.iter().map(|y| y.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_default()).collect()
            })
            .unwrap_or_default(),
        notes: v["notes"].as_array().map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        frame_size: v["frame_size"].as_u64().unwrap_or(0) as usize,
    });
    if let Some(w) = app.wavetable_import.as_mut() {
        w.step = w.step.min(result.as_ref().map_or(0, |p| p.waves.len().saturating_sub(1)));
        w.preview = Some(Preview { result });
        w.made_for = Some(params);
    }
}

/// Draws the stack: every wave faint, stepped up and to the right, the chosen one bold in front.
fn stack(ui: &mut Ui, r: Rect, waves: &[Vec<f32>], step: usize) {
    let t = Tokens::current();
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, t.counter_bg);
    let n = waves.len().max(1);
    let shift = vec2(80.0 / n as f32, -40.0 / n as f32);
    let base = Rect::from_min_max(pos2(r.min.x + 10.0, r.min.y + 52.0), pos2(r.max.x - 92.0, r.max.y - 10.0));
    for (i, w) in waves.iter().enumerate().rev() {
        let at = base.translate(shift * i as f32);
        let (stroke, colour) = if i == step { (2.0, t.accent) } else { (1.0, t.text_dim.gamma_multiply(0.35)) };
        p.add(egui::Shape::line(wave_points(w, at.shrink2(vec2(0.0, 6.0))), Stroke::new(stroke, colour)));
    }
}

/// Draws the window when open (each frame).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(track) = app.wavetable_import.as_ref().map(|w| w.track) else { return };
    // The synth's track went, or it plays something else now: the window closes.
    let ok = app.engine.session().track(track).and_then(|t| t.instrument.as_ref()).is_some_and(|i| i.plugin == WAVETABLE);
    if !ok {
        app.wavetable_import = None;
        return;
    }
    refresh(app);
    let state = app.engine.session().track(track).and_then(|t| t.instrument.as_ref()).and_then(|i| i.state.clone());
    if app.wavetable_import.as_ref().is_some_and(|w| w.current.as_ref().is_none_or(|c| c.0 != state)) {
        let info = app.engine.execute("mix.wavetable_info", &json!({"track": track.0})).unwrap_or(Value::Null);
        if let Some(w) = app.wavetable_import.as_mut() {
            w.current = Some((state, info));
        }
    }
    let track_name = app.engine.session().track(track).map(|t| t.name.clone()).unwrap_or_default();
    let mut open = true;
    let mut action: Option<&str> = None;
    let mut pick = false;
    let can_pick = app.services.pick_open.is_some();
    let can_save = app.services.pick_save.is_some();
    let mut table_action: Option<&str> = None;
    let resp = egui::Window::new(format!("Import Wavetable · {track_name}"))
        .id(egui::Id::new("wavetable_import"))
        .open(&mut open)
        .default_width(520.0)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let Some(w) = app.wavetable_import.as_mut() else { return };
            let info = w.current.as_ref().map_or(Value::Null, |c| c.1.clone());
            let user = info["user"].as_bool() == Some(true);
            ui.horizontal(|ui| {
                let name = info["name"].as_str().unwrap_or("");
                let now = if user { format!("Playing: {name} (imported, {} waves)", info["waves"]) } else { format!("Playing: {name} (built-in)") };
                ui.label(now);
                if ui.add_enabled(can_pick, egui::Button::new("Load Table...")).on_hover_text("A wavetable WAV of 2048-sample frames").clicked() {
                    table_action = Some("mix.wavetable_load");
                }
                if ui.add_enabled(user && can_save, egui::Button::new("Save Table...")).on_disabled_hover_text("Import a table first").clicked() {
                    table_action = Some("mix.wavetable_save");
                }
                if ui
                    .add_enabled(user, egui::Button::new("Clear Table"))
                    .on_hover_text("Back to the built-in tables")
                    .on_disabled_hover_text("It plays a built-in table")
                    .clicked()
                {
                    table_action = Some("mix.wavetable_clear");
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Audio file:");
                let shown = if w.path.is_empty() { "none: pick one, or drop one here".to_string() } else { w.path.clone() };
                ui.add(egui::Label::new(shown).truncate()).on_hover_text(&w.path);
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(can_pick, egui::Button::new("Choose File...")).on_disabled_hover_text("Drop an audio file on this window").clicked()
                {
                    pick = true;
                }
                ui.label("Name:");
                ui.add(egui::TextEdit::singleline(&mut w.name).hint_text("the file's name").desired_width(180.0));
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Mode:");
                for (i, (label, _)) in MODES.iter().enumerate() {
                    ui.selectable_value(&mut w.mode, i, *label).on_hover_text(MODE_HELP[i]);
                }
            });
            ui.horizontal(|ui| match w.mode {
                0 => {
                    ui.label("Frame size:");
                    let text = |f: Option<usize>| f.map_or("From the file".to_string(), |n| format!("{n} samples"));
                    egui::ComboBox::from_id_salt("wt_frame").selected_text(text(w.frame_size)).show_ui(ui, |ui| {
                        for f in FRAME_SIZES {
                            ui.selectable_value(&mut w.frame_size, f, text(f));
                        }
                    });
                }
                _ => {
                    ui.label("Waves:");
                    ui.add(egui::Slider::new(&mut w.waves, 1..=32));
                }
            });
            let (r, _) = ui.allocate_exact_size(vec2(500.0, 190.0), Sense::hover());
            match w.preview.as_ref().map(|p| &p.result) {
                Some(Ok(p)) if !p.waves.is_empty() => {
                    let n = p.waves.len();
                    w.step = w.step.min(n - 1);
                    stack(ui, r, &p.waves, w.step);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(w.step > 0, egui::Button::new("◀")).on_hover_text("Previous wave").clicked() {
                            w.step -= 1;
                        }
                        ui.label(format!("Wave {} of {n}", w.step + 1));
                        if ui.add_enabled(w.step + 1 < n, egui::Button::new("▶")).on_hover_text("Next wave").clicked() {
                            w.step += 1;
                        }
                        if w.mode == 0 && p.frame_size > 0 {
                            ui.label(format!("· frames of {}", p.frame_size));
                        }
                        ui.label(format!("· {}", p.name));
                    });
                    for note in &p.notes {
                        ui.label(egui::RichText::new(note).small().color(Tokens::current().text_dim));
                    }
                }
                Some(Err(e)) => {
                    ui.painter().rect_filled(r, 4.0, Tokens::current().counter_bg);
                    ui.colored_label(Tokens::current().meter_red, e);
                }
                _ => {
                    ui.painter().rect_filled(r, 4.0, Tokens::current().counter_bg);
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        "Choose or drop an audio file",
                        egui::FontId::proportional(13.0),
                        Tokens::current().text_dim,
                    );
                }
            }
            ui.separator();
            let ready = matches!(w.preview.as_ref().map(|p| &p.result), Some(Ok(p)) if !p.waves.is_empty());
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, egui::Button::new("Import")).on_disabled_hover_text("Choose a file that makes waves first").clicked() {
                    action = Some("mix.wavetable_import");
                }
                if ui.button("Cancel").clicked() {
                    action = Some("cancel");
                }
            });
        });
    if let Some(r) = resp {
        app.wavetable_drop_targets.push((r.response.rect, track));
    }
    if pick
        && let Some(pick) = &app.services.pick_open
        && let Some(path) = pick("mix.wavetable_import", crate::sampler_window::AUDIO_EXTS)
    {
        open_file(app, track, path);
    }
    match table_action {
        Some("mix.wavetable_load") => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|pick| pick("mix.wavetable_load", &["wav", "wave"])) {
                match app.run("mix.wavetable_load", json!({"track": track.0, "path": path})) {
                    Ok(v) => {
                        app.ui.status = format!("{} loaded into the Wavetable Synth ({} waves)", v["name"].as_str().unwrap_or("Table"), v["waves"])
                    }
                    Err(e) => app.ui.status = e,
                }
            }
        }
        Some("mix.wavetable_save") => {
            let file = format!(
                "{}.wav",
                app.wavetable_import.as_ref().and_then(|w| w.current.as_ref()).and_then(|c| c.1["name"].as_str()).unwrap_or("Wavetable")
            );
            if let Some(path) = app.services.pick_save.as_ref().and_then(|pick| pick("mix.wavetable_save", &file)) {
                match app.run("mix.wavetable_save", json!({"track": track.0, "path": path})) {
                    Ok(v) => app.ui.status = format!("Table saved to {}", v["path"].as_str().unwrap_or("")),
                    Err(e) => app.ui.status = e,
                }
            }
        }
        Some(cmd) => {
            if let Err(e) = app.run(cmd, json!({"track": track.0})) {
                app.ui.status = e;
            }
        }
        None => {}
    }
    match action {
        Some("cancel") => open = false,
        Some(cmd) => {
            if let Some(params) = app.wavetable_import.as_ref().map(ImportWindow::params) {
                match app.run(cmd, params) {
                    Ok(v) => {
                        app.ui.status = format!("{} imported into the Wavetable Synth ({} waves)", v["name"].as_str().unwrap_or("Table"), v["waves"]);
                        open = false;
                    }
                    Err(e) => app.ui.status = e,
                }
            }
        }
        None => {}
    }
    if !open {
        app.wavetable_import = None;
    }
}

fn open_file(app: &mut SoundApp, track: TrackId, path: String) {
    open(app, track, Some(path));
}

/// Takes dropped files meant for a wavetable (its wave display or the open import window): the
/// first audio file opens the window with it. Returns the files it did not take.
pub(crate) fn take_drop(app: &mut SoundApp, files: Vec<egui::DroppedFileHandle>, pos: Option<Pos2>) -> Vec<egui::DroppedFileHandle> {
    let Some(track) = drop_track(app, pos) else { return files };
    let mut rest = Vec::new();
    let mut taken = false;
    for f in files {
        let path = f.path().to_string_lossy().into_owned();
        if !taken && crate::sampler_window::is_audio_name(&path) {
            taken = true;
            open(app, track, Some(path));
        } else {
            rest.push(f);
        }
    }
    rest
}
