//! The Sampler in its instrument window, all on one page: the sample (Load Sample, From
//! Selection, Clear, the detected root note), the waveform with its start, end and loop markers
//! (which drag), the Pitch, Filter, Amp Envelope, LFO and Output knob panels, and a keyboard
//! with the key range and root note; and audio files dropped onto the window or onto an
//! instrument track. Every control goes through `mix.instrument_param` or a sampler command, so
//! it undoes and scripts like any edit.

use crate::SoundApp;
use crate::theme::{Tokens, mono, regular};
use egui::{Align2, Color32, CursorIcon, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{Insert, TrackId};
use std::path::Path;

/// The built-in Sampler's plugin id.
pub const SAMPLER: &str = "sampler";

/// File extensions a sample can come from (what the audio decoder reads).
const AUDIO_EXTS: &[&str] = &["wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "aac", "mp4", "caf"];

/// Markers on the sample strip: the parameter each one sets, and whether it needs a loop.
const MARKERS: [(&str, bool); 4] = [("start", false), ("end", false), ("loop_start", true), ("loop_end", true)];

/// Whether a file name looks like audio the sampler can load.
pub fn is_audio_name(name: &str) -> bool {
    Path::new(name).extension().and_then(|x| x.to_str()).is_some_and(|x| AUDIO_EXTS.contains(&x.to_ascii_lowercase().as_str()))
}

/// Sampler parameters that are MIDI notes.
const NOTE_PARAMS: [&str; 3] = ["root_note", "key_low", "key_high"];

/// Whether a built-in instrument's parameter only takes whole numbers (notes and velocities).
pub fn whole_number(plugin: &str, param: &str) -> bool {
    plugin == SAMPLER && (NOTE_PARAMS.contains(&param) || param == "vel_low" || param == "vel_high")
}

/// A built-in instrument's parameter as its window shows it: the sampler's notes by name.
pub fn format_param(plugin: &str, p: &soundcraft_dsp::ParamInfo, value: f32) -> String {
    if plugin == SAMPLER && NOTE_PARAMS.contains(&p.id) {
        let n = value.round().clamp(0.0, 127.0) as u8;
        return format!("{} ({n})", soundcraft_midi::note_name(n));
    }
    if whole_number(plugin, p.id) {
        return format!("{:.0}", value.round());
    }
    p.format(value)
}

fn param(ins: &Insert, id: &str) -> f32 {
    let default = soundcraft_dsp::plugin_info(SAMPLER).and_then(|i| i.param(id)).map_or(0.0, |p| p.default);
    ins.params.get(id).copied().unwrap_or(default)
}

/// Remembers `r` as a place where a dropped audio file loads into track `t`'s sampler, and
/// outlines it while files are dragged over it.
pub fn drop_target(app: &mut SoundApp, ctx: &egui::Context, r: Rect, t: TrackId) {
    app.sample_drop_targets.push((r, t));
    let over = ctx.input(|i| !i.raw.hovered_files.is_empty() && i.pointer.latest_pos().is_some_and(|p| r.contains(p)));
    if over {
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("sample_drop", t.0)))).rect_stroke(
            r,
            2.0,
            Stroke::new(2.0, Tokens::current().accent),
            StrokeKind::Inside,
        );
    }
}

/// An audio file dropped on a sampler target whose bytes the shell has to read itself (in the
/// browser, files arrive by name and are read asynchronously). Hand the bytes to
/// [`SoundApp::load_sample_bytes`].
#[derive(Debug, Clone)]
pub struct SampleRead {
    pub track: TrackId,
    pub file: egui::DroppedFileHandle,
}

impl SampleRead {
    /// The dropped file's name.
    pub fn name(&self) -> String {
        self.file.path().file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string()
    }
}

impl SoundApp {
    /// Takes this frame's dropped files: audio dropped on an instrument track or a Sampler window
    /// loads into that track's sampler (the sampler becomes its instrument). A file with a full
    /// path loads straight away; one without (the browser) waits in [`SoundApp::take_sample_reads`]
    /// for the shell to read. Everything else is returned for the shell to open or import.
    pub fn take_dropped_files(&mut self, ctx: &egui::Context) -> Vec<egui::DroppedFileHandle> {
        let (files, pos) = ctx.input(|i| (i.raw.dropped_files.clone(), i.pointer.latest_pos()));
        if files.is_empty() {
            return files;
        }
        let Some(track) = self.drop_track(pos) else { return files };
        let mut rest = Vec::new();
        let mut claimed = false;
        for f in files {
            if claimed || !is_audio_name(&f.path().to_string_lossy()) {
                rest.push(f);
                continue;
            }
            claimed = true;
            if f.path().is_absolute() {
                let path = f.path().to_string_lossy().into_owned();
                let r = self.run("mix.sampler_load", json!({"track": track.0, "path": path}));
                self.sample_loaded(track, &path, r);
            } else {
                self.sample_reads.push(SampleRead { track, file: f });
            }
        }
        rest
    }

    /// The track a drop at `pos` loads into. Without a position (some platforms report none
    /// during a drag), a single open Sampler window takes it.
    fn drop_track(&self, pos: Option<egui::Pos2>) -> Option<TrackId> {
        if let Some(p) = pos {
            return self.sample_drop_targets.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, t)| *t);
        }
        let s = self.engine.session();
        let mut open = self.ui.plugin_windows.iter().filter(|(t, slot)| {
            *slot == soundcraft_mix::SlotKey::Instrument && s.track(*t).and_then(|x| x.instrument.as_ref()).is_some_and(|i| i.plugin == SAMPLER)
        });
        match (open.next(), open.next()) {
            (Some((t, _)), None) => Some(*t),
            _ => None,
        }
    }

    /// Dropped files waiting for the shell to read their bytes.
    pub fn take_sample_reads(&mut self) -> Vec<SampleRead> {
        std::mem::take(&mut self.sample_reads)
    }

    /// Loads a dropped file's bytes (or reports why they could not be read) into its track's
    /// sampler, as one undo step.
    pub fn load_sample_bytes(&mut self, read: &SampleRead, bytes: Result<Vec<u8>, String>) {
        let name = read.name();
        let r = bytes.and_then(|b| soundcraft_engine::io::load_sampler_bytes(&mut self.engine, read.track, &name, &b).map_err(|e| e.to_string()));
        self.sample_loaded(read.track, &name, r);
    }

    fn sample_loaded(&mut self, track: TrackId, what: &str, r: Result<serde_json::Value, String>) {
        match r {
            Ok(v) => {
                if !self.ui.plugin_windows.contains(&(track, soundcraft_mix::SlotKey::Instrument)) {
                    self.ui.plugin_windows.push((track, soundcraft_mix::SlotKey::Instrument));
                }
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or(what);
                self.ui.status = format!("{name} loaded into the sampler");
            }
            Err(e) => self.ui.status = format!("{what}: {e}"),
        }
    }
}

/// Width the Sampler's window opens at: its five knob panels side by side.
pub const EDITOR_WIDTH: f32 = 860.0;

/// Knob diameter in the Sampler's panels.
const KNOB: f32 = 30.0;

/// The Sampler's editor, all on one page: the sample and its buttons, the waveform with its
/// markers, the Pitch, Filter, Amp Envelope, LFO and Output panels, and the keyboard with the key
/// range and root note. Every control runs `mix.instrument_param` (or a sampler command), so it
/// undoes and scripts like any edit.
pub fn editor(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    ui.set_min_width(EDITOR_WIDTH - 24.0);
    header(app, ui, tid, ins);
    ui.add_space(4.0);
    sample_strip(app, ui, tid, ins);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Loop").color(Tokens::current().text_dim));
        segmented(app, ui, tid, ins, "loop_mode");
        ui.add_space(16.0);
        ui.label(egui::RichText::new("Play").color(Tokens::current().text_dim));
        segmented(app, ui, tid, ins, "play_mode");
        ui.add_space(16.0);
        ui.label(egui::RichText::new("Voices").color(Tokens::current().text_dim));
        segmented(app, ui, tid, ins, "voice_mode");
    });
    ui.add_space(6.0);
    panels(app, ui, tid, ins);
    ui.add_space(6.0);
    keyboard(app, ui, tid, ins);
}

/// The sample's name and facts, Load / From Selection / Clear, and the detected root note.
fn header(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    let t = Tokens::current();
    let s = app.engine.session();
    let source = ins.sample.and_then(|id| s.source(id)).cloned();
    let audio = ins.sample.and_then(|id| s.pool.get(id)).cloned();
    let selection = !s.edit.selection.is_empty();
    let info = match (&source, &audio) {
        (Some(src), Some(a)) => {
            let rate = a.sample_rate().max(1);
            let secs = a.frames() as f64 / f64::from(rate);
            let width = match a.num_channels() {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                n => format!("{n} ch"),
            };
            format!("{} · {secs:.2} s · {width} · {:.1} kHz", src.name, f64::from(rate) / 1000.0)
        }
        (Some(src), None) => format!("{}: missing", src.name),
        _ => "No sample".to_string(),
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(info).font(mono(11.0)).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ins.sample.is_some() && ui.button("Clear").clicked() {
                let _ = app.run("mix.sampler_clear", json!({"track": tid.0}));
            }
            let from = ui.add_enabled(selection, egui::Button::new("From Selection"));
            let from = from
                .on_hover_text("Load the selected range of an audio track, as it plays")
                .on_disabled_hover_text("Select a range on an audio track first");
            if from.clicked() {
                match app.run("mix.sampler_from_selection", json!({"track": tid.0})) {
                    Ok(v) => {
                        let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("The selection").to_string();
                        app.ui.status = format!("{name} loaded into the sampler");
                    }
                    Err(e) => app.ui.status = e,
                }
            }
            let can_pick = app.services.pick_open.is_some();
            let load = ui.add_enabled(can_pick, egui::Button::new("Load Sample…"));
            let load = if can_pick { load } else { load.on_disabled_hover_text("Drop an audio file on this window") };
            if load.clicked()
                && let Some(pick) = &app.services.pick_open
                && let Some(path) = pick("mix.sampler_load", AUDIO_EXTS)
            {
                let _ = app.run("mix.sampler_load", json!({"track": tid.0, "path": path}));
            }
        });
    });
    // The detected root note, worked out once per sample.
    let Some((src, audio)) = ins.sample.zip(audio) else { return };
    let key = ui.id().with(("sampler_root", tid.0, src.0));
    let root = match ui.ctx().data(|d| d.get_temp::<Option<f32>>(key)) {
        Some(r) => r,
        None => {
            let r = detect_root(&audio);
            ui.ctx().data_mut(|d| d.insert_temp(key, r));
            r
        }
    };
    let current = param(ins, "root_note").round().clamp(0.0, 127.0) as u8;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("Root {}", soundcraft_midi::note_name(current))).color(t.text_dim));
        match root {
            Some(m) => {
                let n = m.round().clamp(0.0, 127.0) as u8;
                let cents = ((m - m.round()) * 100.0).round() as i32;
                let off = if cents == 0 { String::new() } else { format!(" {cents:+}¢") };
                ui.label(egui::RichText::new(format!("· detected {}{off}", soundcraft_midi::note_name(n))).color(t.text_dim));
                if n != current
                    && ui.button("Use as Root").on_hover_text("Play the sample in tune: set its root note to the detected pitch").clicked()
                {
                    set(app, tid, "root_note", f32::from(n));
                }
            }
            None => {
                ui.label(egui::RichText::new("· no clear pitch").color(t.text_dim));
            }
        }
    });
}

/// The pitch a sample sounds at, from its first seconds (both sides mixed).
fn detect_root(audio: &soundcraft_model::SourceAudio) -> Option<f32> {
    let rate = audio.sample_rate();
    let n = audio.frames().min(rate as usize * 4);
    let w = audio.window(0, n);
    let mono: Vec<f32> = match (w.first(), w.get(1)) {
        (Some(a), Some(b)) => a.iter().zip(b).map(|(x, y)| 0.5 * (x + y)).collect(),
        (Some(a), None) => a.clone(),
        _ => return None,
    };
    soundcraft_dsp::pitch_detect::sample_root(&mono, rate as f32)
}

/// Sets one Sampler parameter (merged into one undo step while dragging).
fn set(app: &mut SoundApp, tid: TrackId, id: &str, value: f32) {
    if let Err(e) = app.engine.execute_merged(
        "mix.instrument_param",
        &json!({"track": tid.0, "param": id, "value": value}),
        &format!("instrument_param:{}:{id}", tid.0),
    ) {
        app.ui.status = e.to_string();
    }
}

fn info_of(id: &str) -> Option<&'static soundcraft_dsp::ParamInfo> {
    soundcraft_dsp::plugin_info(SAMPLER).and_then(|i| i.param(id))
}

/// A choice parameter as a row of buttons.
fn segmented(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert, id: &str) {
    let Some(p) = info_of(id) else { return };
    let now = param(ins, id).round().max(0.0) as usize;
    ui.spacing_mut().item_spacing.x = 2.0;
    for (i, c) in p.choices.iter().enumerate() {
        if ui.selectable_label(i == now, *c).clicked() && i != now {
            set(app, tid, id, i as f32);
        }
    }
    ui.spacing_mut().item_spacing.x = 8.0;
}

/// Width of one knob cell (its name, knob and value) in a panel.
const CELL: f32 = 60.0;

/// A choice parameter as a drop-down.
fn combo(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert, id: &str) {
    let Some(p) = info_of(id) else { return };
    let now = param(ins, id).round().max(0.0) as usize;
    let mut pick = now;
    egui::ComboBox::from_id_salt(("lift", id, tid.0))
        .width(ui.available_width() - 8.0)
        .selected_text(p.choices.get(now).copied().unwrap_or("?"))
        .show_ui(ui, |ui| {
            for (i, c) in p.choices.iter().enumerate() {
                ui.selectable_value(&mut pick, i, *c);
            }
        });
    if pick != now {
        set(app, tid, id, pick as f32);
    }
}

/// One knob with its name above and its value below, in a cell of its own.
fn knob(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert, id: &str, name: &str) {
    let Some(p) = info_of(id) else { return };
    let t = Tokens::current();
    let v = param(ins, id);
    let layout = egui::Layout::top_down(egui::Align::Center);
    ui.allocate_ui_with_layout(vec2(CELL, 76.0), layout, |ui| {
        ui.set_width(CELL);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add(egui::Label::new(egui::RichText::new(name).font(regular(9.5)).color(t.text_dim)).truncate());
        let r = crate::widgets::knob(ui, p, v, KNOB, t.accent, crate::widgets::KnobStyle::Dark);
        if let Some(x) = r.changed {
            let x = if whole_number(SAMPLER, id) { x.round() } else { x };
            set(app, tid, id, x);
        }
        ui.add(egui::Label::new(egui::RichText::new(format_param(SAMPLER, p, v)).font(mono(9.5)).color(t.text)).truncate());
    });
}

/// A titled panel of knobs (rows of parameter and label), `cols` cells wide.
fn panel(
    app: &mut SoundApp,
    ui: &mut Ui,
    tid: TrackId,
    ins: &Insert,
    title: &str,
    cols: usize,
    extra: impl FnOnce(&mut SoundApp, &mut Ui),
    rows: &[&[(&str, &str)]],
) {
    let t = Tokens::current();
    let width = CELL * cols as f32 + 2.0 * (cols.saturating_sub(1)) as f32;
    egui::Frame::new().fill(t.module_tile_bg).stroke(Stroke::new(1.0, t.module_tile_border)).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
        ui.set_width(width);
        ui.set_min_height(PANEL_H);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(title).font(crate::theme::bold(10.5)).color(t.section_label));
            extra(app, ui);
            for row in rows {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (id, name) in *row {
                        knob(app, ui, tid, ins, id, name);
                    }
                });
            }
        });
    });
}

/// Height every panel is drawn at, so they line up.
const PANEL_H: f32 = 236.0;

/// The five panels, left to right in the order the sound flows.
fn panels(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        panel(
            app,
            ui,
            tid,
            ins,
            "PITCH",
            2,
            |_, _| {},
            &[&[("root_note", "Root"), ("transpose", "Transpose")], &[("fine_tune", "Fine"), ("glide", "Glide")]],
        );
        panel(
            app,
            ui,
            tid,
            ins,
            "FILTER",
            3,
            |app, ui| {
                combo(app, ui, tid, ins, "filter_type");
                filter_curve(ui, ins);
            },
            &[
                &[("cutoff", "Cutoff"), ("resonance", "Resonance"), ("filter_env", "Env Amount")],
                &[("filter_attack", "Env Attack"), ("filter_decay", "Env Decay")],
            ],
        );
        panel(
            app,
            ui,
            tid,
            ins,
            "AMP ENVELOPE",
            3,
            |app, ui| adsr_graph(app, ui, tid, ins),
            &[&[("attack", "Attack"), ("decay", "Decay"), ("sustain", "Sustain")], &[("release", "Release"), ("velocity", "Velocity")]],
        );
        panel(
            app,
            ui,
            tid,
            ins,
            "LFO",
            3,
            |app, ui| {
                combo(app, ui, tid, ins, "lfo_shape");
            },
            &[&[("lfo_rate", "Rate"), ("lfo_pitch", "To Pitch"), ("lfo_filter", "To Filter")], &[("lfo_volume", "To Volume")]],
        );
        panel(
            app,
            ui,
            tid,
            ins,
            "OUTPUT",
            2,
            |_, _| {},
            &[&[("level", "Level"), ("pan", "Pan")], &[("vel_low", "Vel Low"), ("vel_high", "Vel High")]],
        );
    });
}

/// The filter's response, drawn from the same state-variable filter the Sampler runs.
fn filter_curve(ui: &mut Ui, ins: &Insert) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().max(120.0), 44.0), Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, 3.0, t.graph_bg);
    let kind = param(ins, "filter_type").round() as i32;
    let fc = param(ins, "cutoff").max(1.0);
    let k = 2.0 - 1.92 * (param(ins, "resonance") / 100.0).clamp(0.0, 1.0);
    let db_of = |f: f32| -> f32 {
        // |H| of the analogue prototype the filter models, at f / fc.
        let w = f / fc;
        let den = ((1.0 - w * w).powi(2) + (k * w).powi(2)).sqrt().max(1e-9);
        let mag = match kind {
            1 => 1.0 / den,
            2 => w * w / den,
            3 => k * w / den,
            _ => 1.0,
        };
        20.0 * mag.max(1e-6).log10()
    };
    let n = r.width().max(2.0) as usize;
    let pts: Vec<egui::Pos2> = (0..n)
        .map(|i| {
            let f = 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32);
            let db = db_of(f).clamp(-36.0, 24.0);
            pos2(r.min.x + i as f32, r.center().y - 4.0 - db / 36.0 * (r.height() * 0.5))
        })
        .collect();
    let col = if kind == 0 { t.text_dim } else { t.accent };
    p.add(egui::Shape::line(pts, Stroke::new(1.5, col)));
}

/// The amp envelope's shape. Drag the attack peak sideways for the attack, the decay corner for
/// the decay (sideways) and sustain (up and down), and the release end sideways for the release.
fn adsr_graph(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(120.0), 44.0), Sense::click_and_drag());
    let p = ui.painter_at(r);
    p.rect_filled(r, 3.0, t.graph_bg);
    let norm = |id: &str| info_of(id).map_or(0.0, |i| i.to_normalized(param(ins, id)));
    // Each time segment takes up to a quarter of the width; sustain holds for a fixed eighth.
    let seg = r.width() * 0.27;
    let top = r.min.y + 4.0;
    let bottom = r.max.y - 3.0;
    let sus = (param(ins, "sustain") / 100.0).clamp(0.0, 1.0);
    let x0 = r.min.x + 2.0;
    let xa = x0 + seg * norm("attack");
    let xd = xa + seg * norm("decay");
    let xs = xd + r.width() * 0.12;
    let xr = xs + seg * norm("release");
    let ys = bottom - (bottom - top) * sus;
    let pts = vec![pos2(x0, bottom), pos2(xa, top), pos2(xd, ys), pos2(xs, ys), pos2(xr, bottom)];
    p.add(egui::Shape::line(pts.clone(), Stroke::new(1.5, t.accent)));
    for q in [pts[1], pts[2], pts[4]] {
        p.circle_filled(q, 3.0, t.accent);
    }
    let grab = ui.id().with(("adsr", tid.0));
    if resp.drag_started()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let near = [(1usize, pts[1]), (2, pts[2]), (4, pts[4])]
            .into_iter()
            .map(|(i, q)| (i, q.distance(at)))
            .filter(|(_, d)| *d < 12.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        ui.ctx().data_mut(|d| d.insert_temp(grab, near));
    }
    let held = ui.ctx().data(|d| d.get_temp::<Option<usize>>(grab)).flatten();
    if resp.dragged()
        && let Some(i) = held
    {
        let d = resp.drag_delta();
        let nudge = |app: &mut SoundApp, id: &str, by: f32| {
            if let Some(info) = info_of(id) {
                let v = info.from_normalized(info.to_normalized(param(ins, id)) + by);
                set(app, tid, id, v);
            }
        };
        match i {
            1 => nudge(app, "attack", d.x / seg),
            2 => {
                nudge(app, "decay", d.x / seg);
                let v = (sus - d.y / (bottom - top)).clamp(0.0, 1.0) * 100.0;
                set(app, tid, "sustain", v);
            }
            _ => nudge(app, "release", d.x / seg),
        }
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<usize>>(grab));
    }
}

/// The keyboard: the playable key range lit, the root note marked. Drag a range edge to move it;
/// double-click a key to make it the root.
fn keyboard(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), 46.0), Sense::click_and_drag());
    let p = ui.painter_at(r);
    let lo = param(ins, "key_low").round().clamp(0.0, 127.0) as u8;
    let hi = param(ins, "key_high").round().clamp(0.0, 127.0) as u8;
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let root = param(ins, "root_note").round().clamp(0.0, 127.0) as u8;
    let black = |n: u8| matches!(n % 12, 1 | 3 | 6 | 8 | 10);
    let whites = (0..=127u8).filter(|n| !black(*n)).count() as f32;
    let ww = r.width() / whites;
    // x range of each key.
    let mut keys: Vec<(u8, Rect)> = Vec::with_capacity(128);
    let mut wi = 0.0f32;
    for n in 0..=127u8 {
        if black(n) {
            let x = r.min.x + wi * ww - ww * 0.3;
            keys.push((n, Rect::from_min_size(pos2(x, r.min.y), vec2(ww * 0.6, r.height() * 0.6))));
        } else {
            keys.push((n, Rect::from_min_size(pos2(r.min.x + wi * ww, r.min.y), vec2(ww, r.height()))));
            wi += 1.0;
        }
    }
    let in_range = |n: u8| n >= lo && n <= hi;
    for (n, k) in keys.iter().filter(|(n, _)| !black(*n)) {
        let fill = if in_range(*n) { Color32::from_gray(225) } else { Color32::from_gray(120) };
        p.rect_filled(k.shrink2(vec2(0.5, 0.0)), 1.0, fill);
        if n % 12 == 0 && ww > 5.0 {
            p.text(pos2(k.center().x, k.max.y - 2.0), Align2::CENTER_BOTTOM, soundcraft_midi::note_name(*n), regular(7.5), t.text_dark);
        }
    }
    for (n, k) in keys.iter().filter(|(n, _)| black(*n)) {
        let fill = if in_range(*n) { Color32::from_gray(30) } else { Color32::from_gray(70) };
        p.rect_filled(*k, 1.0, fill);
    }
    if let Some((_, k)) = keys.iter().find(|(n, _)| *n == root) {
        p.circle_filled(pos2(k.center().x, k.max.y - if black(root) { 5.0 } else { 14.0 }), (ww * 0.3).clamp(2.0, 4.0), t.accent);
    }
    // Range edges.
    let edge = |n: u8, upper: bool| keys.iter().find(|(k, _)| *k == n).map_or(r.min.x, |(_, k)| if upper { k.max.x } else { k.min.x });
    let (xl, xh) = (edge(lo, false), edge(hi, true));
    for x in [xl, xh] {
        p.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(2.0, t.accent));
    }
    let key_at = |x: f32, y: f32| {
        keys.iter()
            .filter(|(n, k)| black(*n) && k.contains(pos2(x, y)))
            .map(|(n, _)| *n)
            .next()
            .or_else(|| keys.iter().find(|(n, k)| !black(*n) && x >= k.min.x && x < k.max.x).map(|(n, _)| *n))
    };
    if resp.double_clicked()
        && let Some(at) = resp.interact_pointer_pos()
        && let Some(n) = key_at(at.x, at.y)
    {
        set(app, tid, "root_note", f32::from(n));
    }
    let grab = ui.id().with(("keyrange", tid.0));
    if resp.drag_started()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let which = if (at.x - xl).abs() <= (at.x - xh).abs() { "key_low" } else { "key_high" };
        let near = (at.x - xl).abs().min((at.x - xh).abs()) < 10.0;
        ui.ctx().data_mut(|d| d.insert_temp(grab, near.then(|| which.to_string())));
    }
    let held = ui.ctx().data(|d| d.get_temp::<Option<String>>(grab)).flatten();
    if let Some(h) = resp.hover_pos()
        && ((h.x - xl).abs() < 10.0 || (h.x - xh).abs() < 10.0)
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.dragged()
        && let (Some(id), Some(at)) = (held.as_deref(), resp.interact_pointer_pos())
        && let Some(n) = key_at(at.x.clamp(r.min.x, r.max.x - 1.0), r.max.y - 2.0)
    {
        set(app, tid, id, f32::from(n));
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<String>>(grab));
    }
}

/// The waveform with its markers: S and E (where a note starts and, unless it loops, stops) and
/// the loop's start and end. Drag a marker to move it.
pub fn sample_strip(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    let t = Tokens::current();
    let s = app.engine.session();
    let audio = ins.sample.and_then(|id| s.pool.get(id)).cloned();
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), 150.0), Sense::click_and_drag());
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, t.playlist_bg);
    p.rect_stroke(r, 4.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let Some(audio) = audio else {
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            "Drop an audio file here, or select a range on an audio track and press From Selection",
            regular(12.0),
            t.text_dim,
        );
        return;
    };
    let frames = audio.frames().max(1);
    let x_of = |pct: f32| r.min.x + r.width() * (pct / 100.0).clamp(0.0, 1.0);
    let looping = param(ins, "loop_mode") >= 0.5;
    let (ls, le) = {
        let (a, b) = (param(ins, "loop_start"), param(ins, "loop_end"));
        (a.min(b), a.max(b))
    };
    if looping {
        p.rect_filled(Rect::from_x_y_ranges(x_of(ls)..=x_of(le), r.y_range()), 0.0, t.accent.gamma_multiply(0.18));
    }
    // One min/max column per point, a lane per channel (two at most).
    let lanes = audio.peaks.len().clamp(1, 2);
    let lane_h = r.height() / lanes as f32;
    let cols = r.width().max(1.0) as usize;
    let wave = t.text.gamma_multiply(0.8);
    for (c, peaks) in audio.peaks.iter().take(lanes).enumerate() {
        let mid = r.min.y + lane_h * (c as f32 + 0.5);
        let half = lane_h * 0.45;
        let samples = audio.channel(c).unwrap_or_default();
        for i in 0..cols {
            let a = frames.saturating_mul(i) / cols;
            let b = (frames.saturating_mul(i + 1) / cols).max(a + 1);
            let (lo, hi) = peaks.minmax(samples, a, b);
            let x = r.min.x + i as f32 + 0.5;
            p.line_segment([pos2(x, mid - hi.clamp(-1.0, 1.0) * half), pos2(x, mid - lo.clamp(-1.0, 1.0) * half)], Stroke::new(1.0, wave));
        }
    }
    // What a note does not play (before the start, after the end unless it loops) is dimmed.
    let (st, en) = {
        let (a, b) = (param(ins, "start"), param(ins, "end"));
        (a.min(b), a.max(b))
    };
    let shade = t.playlist_bg.gamma_multiply(0.7);
    p.rect_filled(Rect::from_x_y_ranges(r.min.x..=x_of(st), r.y_range()), 0.0, shade);
    if !looping {
        p.rect_filled(Rect::from_x_y_ranges(x_of(en)..=r.max.x, r.y_range()), 0.0, shade);
    }
    // Markers: S and E on top, the loop's [ and ] at the bottom when looping.
    let shown: Vec<(&str, f32, Color32, &str, bool)> = MARKERS
        .iter()
        .filter(|(_, needs_loop)| !needs_loop || looping)
        .map(|(id, _)| {
            let (col, tag, bottom) = match *id {
                "start" => (t.playhead, "S", false),
                "end" => (t.playhead, "E", false),
                "loop_start" => (t.accent, "[", true),
                _ => (t.accent, "]", true),
            };
            (*id, param(ins, id), col, tag, bottom)
        })
        .collect();
    for (_, pct, col, tag, bottom) in &shown {
        let x = x_of(*pct);
        p.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.5, *col));
        let y = if *bottom { r.max.y - 12.0 } else { r.min.y };
        let flag = Rect::from_min_size(pos2(x, y), vec2(12.0, 12.0));
        p.rect_filled(flag, 1.0, *col);
        p.text(flag.center(), Align2::CENTER_CENTER, *tag, regular(9.0), t.text_dark);
    }
    // Dragging the nearest marker sets its parameter (loop markers grab in the lower half).
    let grab_id = ui.id().with(("sample_marker", tid.0));
    let nearest = |pos: egui::Pos2| {
        let lower = pos.y > r.center().y;
        shown
            .iter()
            .map(|(id, pct, _, _, bottom)| (*id, (x_of(*pct) - pos.x).abs() + if *bottom == lower { 0.0 } else { 6.0 }))
            .filter(|(_, d)| *d <= 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    };
    if let Some(h) = resp.hover_pos()
        && nearest(h).is_some()
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let grabbed = nearest(pos).map(str::to_string);
        ui.ctx().data_mut(|d| d.insert_temp(grab_id, grabbed));
    }
    let grabbed = ui.ctx().data(|d| d.get_temp::<Option<String>>(grab_id)).flatten();
    if resp.dragged()
        && let (Some(id), Some(pos)) = (grabbed.as_deref(), resp.interact_pointer_pos())
    {
        let pct = ((pos.x - r.min.x) / r.width().max(1.0) * 100.0).clamp(0.0, 100.0);
        set(app, tid, id, pct);
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<String>>(grab_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_show_by_name() {
        let info = soundcraft_dsp::plugin_info(SAMPLER).unwrap();
        let root = info.param("root_note").unwrap();
        assert_eq!(format_param(SAMPLER, root, 60.0), format!("{} (60)", soundcraft_midi::note_name(60)));
        assert_eq!(format_param(SAMPLER, info.param("vel_high").unwrap(), 126.6), "127");
        assert_eq!(format_param(SAMPLER, info.param("attack").unwrap(), 1.0), "1.0 ms");
        assert!(whole_number(SAMPLER, "key_low") && !whole_number(SAMPLER, "attack") && !whole_number("subtractive_synth", "root_note"));
    }

    #[test]
    fn audio_names() {
        for n in ["a.wav", "B.FLAC", "x.mp3", "y.ogg", "z.aiff", "/tmp/k.Wav"] {
            assert!(is_audio_name(n), "{n}");
        }
        for n in ["song.scraft", "beat.mid", "noext", "", "pic.png"] {
            assert!(!is_audio_name(n), "{n}");
        }
    }
}
